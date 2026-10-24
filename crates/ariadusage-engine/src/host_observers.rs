// Ported from CodexBar Sources/CodexBar/AgentSessionsStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+AgentSessions.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/AdaptiveRefreshPolicy.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/SettingsStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/LocalAgentSessionScanner.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/AgentSession.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeDesktopProjectsLocator.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/PiFamilySessionScanner.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/PiFamilySessionRootResolver.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Host power, thermal, and local coding-activity observers feeding [`crate::EngineRuntime`].
//!
//! - **Power & thermal**:
//!   - macOS: `NSProcessInfo` low-power and thermal notifications are bridged in
//!     `ariadusage-macos/src/appkit/` and delivered over IPC (`setLowPowerMode.on` /
//!     `setLowPowerMode.off`, `setThermalPressure.nominal` / `setThermalPressure.constrained`);
//!     [`map_macos_thermal_state`] ports upstream `AdaptiveRefreshSignalCollector.currentThermalPressure`.
//!   - Linux: queries `org.freedesktop.UPower` and `org.freedesktop.UPower.PowerProfiles` over
//!     D-Bus (`zbus`) and reads `/sys/class/thermal/thermal_zone*` via
//!     [`read_linux_thermal_pressure_from_sysfs`].
//! - **Coding activity**:
//!   - [`LocalAgentSessionScanner::scan_with_activity`] enumerates same-user agent processes
//!     through [`LocalProbe`] / `procscan` and inspects live Claude / Pi transcripts and recent
//!     Codex rollout modification dates under a shared [`DirectoryMetadataScanBudget`].
//!   - [`AgentSessionRefreshGate`] coalesces overlapping refreshes and gates local scanning on
//!     `!low_power_mode_enabled && thermal_pressure == ThermalPressure::Nominal`, running on the
//!     30-second periodic cadence (`LOCAL_ACTIVITY_SCAN_INTERVAL`) and on menu-open / settings
//!     transitions.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ariadusage_core::pipeline::{BoxFuture, FetchInteraction};
use ariadusage_core::refresh::ThermalPressure;
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use tokio_util::sync::CancellationToken;

use crate::brokers::call::BrokerCall;
use crate::brokers::local_probe::LocalProbe;
use crate::brokers::process::{
    self, AbsolutePath, Command, LaunchMode, ProcessEnv, StdinSpec, StreamPolicy, procscan,
};

/// Upstream 30-second periodic local activity scan cadence (`AgentSessionsStore.localActivityScanInterval`).
pub const LOCAL_ACTIVITY_SCAN_INTERVAL: Duration = Duration::from_secs(30);

/// Default Linux sysfs thermal directory (`/sys/class/thermal`).
pub const DEFAULT_LINUX_THERMAL_SYSFS_ROOT: &str = "/sys/class/thermal";

/// Default fallback serious thermal threshold in millidegrees Celsius (85°C) when a thermal zone
/// exposes `temp` without usable `trip_point_*_temp` thresholds.
pub const DEFAULT_LINUX_THERMAL_CONSTRAINED_MILLIDEGREES: i64 = 85_000;

const LSOF_TIMEOUT: Duration = Duration::from_secs(2);
const LSOF_OUTPUT_CAP: usize = 256 * 1024;
const CLAUDE_DESKTOP_LOCATOR_MAX_DEPTH: usize = 4;

/// Maps macOS `NSProcessInfoThermalState` raw integer values (`0 = nominal`, `1 = fair`,
/// `2 = serious`, `3 = critical`) to [`ThermalPressure`]
/// (`AdaptiveRefreshSignalCollector.currentThermalPressure`, `AdaptiveRefreshPolicy.swift:31-37`).
pub const fn map_macos_thermal_state(raw_state: isize) -> ThermalPressure {
    match raw_state {
        0 | 1 => ThermalPressure::Nominal,
        _ => ThermalPressure::Constrained,
    }
}

/// Evaluates whether Linux UPower / PowerProfiles state indicates low-power mode.
///
/// - If `power_saver_active` is `Some(true)` (`ActiveProfile == "power-saver"`), returns `true`.
/// - If `power_saver_active` is `Some(false)` and the system is not on battery, returns `false`.
/// - When on battery (`on_battery == Some(true)`), returns `true` if UPower reports a warning
///   level of Low/Critical/Action (`warning_level >= 3`) or battery percentage `<= 20.0`.
pub fn evaluate_upower_low_power_mode(
    power_saver_active: Option<bool>,
    on_battery: Option<bool>,
    warning_level: Option<u32>,
    battery_percentage: Option<f64>,
) -> bool {
    if power_saver_active == Some(true) {
        return true;
    }
    if on_battery == Some(true) {
        if warning_level.is_some_and(|level| level >= 3) {
            return true;
        }
        if battery_percentage.is_some_and(|pct| pct.is_finite() && pct <= 20.0) {
            return true;
        }
    }
    false
}

/// Reads Linux thermal pressure from `sysfs_thermal_root` (`/sys/class/thermal/thermal_zone*`).
///
/// A zone is considered [`ThermalPressure::Constrained`] when its `temp` (in millidegrees Celsius)
/// reaches or exceeds any positive `critical`, `hot`, or `passive` trip point (`trip_point_*_temp`),
/// or when no trip point is triggered and `temp >= 85_000` (85°C). Symlinked zone directories are
/// skipped without following them.
pub fn read_linux_thermal_pressure_from_sysfs(sysfs_thermal_root: &Path) -> ThermalPressure {
    let Ok(entries) = fs::read_dir(sysfs_thermal_root) else {
        return ThermalPressure::Nominal;
    };
    let mut zone_dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with("thermal_zone") {
                return None;
            }
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).ok()?;
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return None;
            }
            Some(path)
        })
        .collect();
    zone_dirs.sort();

    for zone_dir in zone_dirs {
        let Some(temp) = read_sysfs_i64(&zone_dir.join("temp")) else {
            continue;
        };
        if temp <= 0 {
            continue;
        }
        let mut constrained_by_trip = false;
        let mut has_usable_trip = false;
        if let Ok(zone_entries) = fs::read_dir(&zone_dir) {
            for entry in zone_entries.filter_map(Result::ok) {
                let file_name = entry.file_name();
                let name = file_name.to_string_lossy();
                let Some(prefix) = name.strip_suffix("_type") else {
                    continue;
                };
                if !prefix.starts_with("trip_point_") {
                    continue;
                }
                let Ok(trip_type) = fs::read_to_string(entry.path()) else {
                    continue;
                };
                let trip_type = trip_type.trim();
                if !matches!(trip_type, "critical" | "hot" | "passive") {
                    continue;
                }
                let temp_path = zone_dir.join(format!("{prefix}_temp"));
                let Some(trip_temp) = read_sysfs_i64(&temp_path) else {
                    continue;
                };
                if trip_temp <= 0 {
                    continue;
                }
                has_usable_trip = true;
                if temp >= trip_temp {
                    constrained_by_trip = true;
                    break;
                }
            }
        }
        if constrained_by_trip
            || (!has_usable_trip && temp >= DEFAULT_LINUX_THERMAL_CONSTRAINED_MILLIDEGREES)
        {
            return ThermalPressure::Constrained;
        }
    }

    ThermalPressure::Nominal
}

fn read_sysfs_i64(path: &Path) -> Option<i64> {
    let meta = fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return None;
    }
    let raw = fs::read_to_string(path).ok()?;
    raw.trim().parse::<i64>().ok()
}

/// Queries Linux UPower / PowerProfiles over the system D-Bus (`zbus`) to determine whether
/// low-power mode is active. Returns `None` if the system D-Bus is unavailable.
#[cfg(target_os = "linux")]
pub async fn query_linux_upower_low_power_mode() -> Option<bool> {
    tokio::time::timeout(Duration::from_secs(2), async {
        let conn = zbus::Connection::system().await.ok()?;

        let power_saver_active = query_dbus_string_property(
            &conn,
            "org.freedesktop.UPower.PowerProfiles",
            "/org/freedesktop/UPower/PowerProfiles",
            "org.freedesktop.UPower.PowerProfiles",
            "ActiveProfile",
        )
        .await
        .map(|profile| profile.trim() == "power-saver");

        let power_saver_active = match power_saver_active {
            Some(val) => Some(val),
            None => query_dbus_string_property(
                &conn,
                "net.hadess.PowerProfiles",
                "/net/hadess/PowerProfiles",
                "net.hadess.PowerProfiles",
                "ActiveProfile",
            )
            .await
            .map(|profile| profile.trim() == "power-saver"),
        };

        let on_battery = query_dbus_bool_property(
            &conn,
            "org.freedesktop.UPower",
            "/org/freedesktop/UPower",
            "org.freedesktop.UPower",
            "OnBattery",
        )
        .await;

        let warning_level = query_dbus_u32_property(
            &conn,
            "org.freedesktop.UPower",
            "/org/freedesktop/UPower/devices/DisplayDevice",
            "org.freedesktop.UPower.Device",
            "WarningLevel",
        )
        .await;

        let battery_percentage = query_dbus_f64_property(
            &conn,
            "org.freedesktop.UPower",
            "/org/freedesktop/UPower/devices/DisplayDevice",
            "org.freedesktop.UPower.Device",
            "Percentage",
        )
        .await;

        if power_saver_active.is_none() && on_battery.is_none() {
            return None;
        }
        Some(evaluate_upower_low_power_mode(
            power_saver_active,
            on_battery,
            warning_level,
            battery_percentage,
        ))
    })
    .await
    .ok()
    .flatten()
}

#[cfg(target_os = "linux")]
async fn query_dbus_property_value(
    conn: &zbus::Connection,
    destination: &str,
    path: &str,
    interface: &str,
    property: &str,
) -> Option<zbus::zvariant::OwnedValue> {
    let reply = conn
        .call_method(
            Some(destination),
            path,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(interface, property),
        )
        .await
        .ok()?;
    reply
        .body()
        .deserialize::<zbus::zvariant::OwnedValue>()
        .ok()
}

#[cfg(target_os = "linux")]
async fn query_dbus_string_property(
    conn: &zbus::Connection,
    destination: &str,
    path: &str,
    interface: &str,
    property: &str,
) -> Option<String> {
    let val = query_dbus_property_value(conn, destination, path, interface, property).await?;
    String::try_from(val).ok()
}

#[cfg(target_os = "linux")]
async fn query_dbus_bool_property(
    conn: &zbus::Connection,
    destination: &str,
    path: &str,
    interface: &str,
    property: &str,
) -> Option<bool> {
    let val = query_dbus_property_value(conn, destination, path, interface, property).await?;
    bool::try_from(val).ok()
}

#[cfg(target_os = "linux")]
async fn query_dbus_u32_property(
    conn: &zbus::Connection,
    destination: &str,
    path: &str,
    interface: &str,
    property: &str,
) -> Option<u32> {
    let val = query_dbus_property_value(conn, destination, path, interface, property).await?;
    u32::try_from(val).ok()
}

#[cfg(target_os = "linux")]
async fn query_dbus_f64_property(
    conn: &zbus::Connection,
    destination: &str,
    path: &str,
    interface: &str,
    property: &str,
) -> Option<f64> {
    let val = query_dbus_property_value(conn, destination, path, interface, property).await?;
    f64::try_from(val).ok()
}

/// Seam for sampling host low-power mode and thermal pressure in [`crate::EngineRuntime`].
pub trait HostPowerThermalProbe: Send + Sync {
    /// Samples `(low_power_mode, thermal_pressure)` from the host if available.
    fn sample<'a>(&'a self) -> BoxFuture<'a, (Option<bool>, Option<ThermalPressure>)>;
}

/// Default host power and thermal probe:
/// - On Linux: queries UPower / PowerProfiles over D-Bus and reads thermal sysfs (`sysfs_thermal_root`).
/// - On other platforms: reads thermal sysfs only if `sysfs_thermal_root` is explicitly injected
///   for testing; macOS production power/thermal updates arrive from `ariadusage-macos` over IPC.
#[derive(Debug, Clone)]
pub struct DefaultHostPowerThermalProbe {
    sysfs_thermal_root: Option<PathBuf>,
}

impl Default for DefaultHostPowerThermalProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl DefaultHostPowerThermalProbe {
    pub fn new() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self {
                sysfs_thermal_root: Some(PathBuf::from(DEFAULT_LINUX_THERMAL_SYSFS_ROOT)),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self {
                sysfs_thermal_root: None,
            }
        }
    }

    pub fn with_sysfs_thermal_root(sysfs_thermal_root: PathBuf) -> Self {
        Self {
            sysfs_thermal_root: Some(sysfs_thermal_root),
        }
    }
}

impl HostPowerThermalProbe for DefaultHostPowerThermalProbe {
    fn sample<'a>(&'a self) -> BoxFuture<'a, (Option<bool>, Option<ThermalPressure>)> {
        Box::pin(async move {
            let thermal = self
                .sysfs_thermal_root
                .as_deref()
                .map(read_linux_thermal_pressure_from_sysfs);
            #[cfg(target_os = "linux")]
            let low_power = query_linux_upower_low_power_mode().await;
            #[cfg(not(target_os = "linux"))]
            let low_power: Option<bool> = None;
            (low_power, thermal)
        })
    }
}

// -----------------------------------------------------------------------------
// AgentSessionsStore refresh gate & LocalAgentSessionScanner (`AgentSessionsStore.swift`,
// `LocalAgentSessionScanner.swift`, `AgentSession.swift`)
// -----------------------------------------------------------------------------

/// Coalesces overlapping local coding-activity scan requests and gates scanning on power and
/// thermal state (`AgentSessionRefreshGate`, `AgentSessionsStore.swift:5-26`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AgentSessionRefreshGate {
    is_refreshing: bool,
    has_queued_refresh: bool,
}

impl AgentSessionRefreshGate {
    pub const fn new() -> Self {
        Self {
            is_refreshing: false,
            has_queued_refresh: false,
        }
    }

    /// Marks a refresh as started. If a refresh is already in flight, queues one follow-up
    /// refresh and returns `false`.
    pub fn begin_refresh(&mut self) -> bool {
        if self.is_refreshing {
            self.has_queued_refresh = true;
            return false;
        }
        self.is_refreshing = true;
        true
    }

    /// Marks the active refresh as completed and returns `true` if a queued follow-up refresh
    /// should run immediately.
    pub fn complete_refresh(&mut self) -> bool {
        let should_rerun = self.has_queued_refresh;
        self.has_queued_refresh = false;
        self.is_refreshing = false;
        should_rerun
    }

    /// Returns whether background local activity scanning is permitted under the current power
    /// and thermal conditions (`AgentSessionsStore.shouldScanLocally`, `AgentSessionsStore.swift:128-137`).
    pub const fn should_scan_locally(
        adaptive_activity_scanning_enabled: bool,
        low_power_mode_enabled: bool,
        thermal_pressure: ThermalPressure,
    ) -> bool {
        if !adaptive_activity_scanning_enabled {
            return false;
        }
        !low_power_mode_enabled && matches!(thermal_pressure, ThermalPressure::Nominal)
    }
}

/// Literal port of upstream `SessionScanConfig` (`AgentSession.swift:262-292`).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionScanConfig {
    pub active_window_seconds: f64,
    pub file_only_window_seconds: f64,
    pub max_process_count: usize,
    pub max_codex_rollout_count: usize,
    pub max_claude_transcript_count_per_project: usize,
    pub max_directory_entry_count: usize,
    pub max_directory_depth: usize,
    pub max_directory_scan_duration: Duration,
}

impl Default for SessionScanConfig {
    fn default() -> Self {
        Self {
            active_window_seconds: 120.0,
            file_only_window_seconds: 30.0 * 60.0,
            max_process_count: 64,
            max_codex_rollout_count: 128,
            max_claude_transcript_count_per_project: 64,
            max_directory_entry_count: 512,
            max_directory_depth: 1,
            max_directory_scan_duration: Duration::from_millis(250),
        }
    }
}

/// Shared metadata scan budget across Claude transcripts, Pi sessions, and Codex rollout files
/// (`DirectoryMetadataScanBudget`, `AgentSession.swift:294-350`).
#[derive(Debug)]
pub struct DirectoryMetadataScanBudget {
    max_entry_count: usize,
    max_depth: usize,
    started_at: Instant,
    time_limit: Duration,
    entries_visited: usize,
}

impl DirectoryMetadataScanBudget {
    pub fn new(max_entry_count: usize, max_depth: usize, time_limit: Duration) -> Self {
        Self {
            max_entry_count,
            max_depth,
            started_at: Instant::now(),
            time_limit: time_limit.min(Duration::from_millis(150)),
            entries_visited: 0,
        }
    }

    pub const fn max_depth(&self) -> usize {
        self.max_depth
    }

    pub const fn entries_visited(&self) -> usize {
        self.entries_visited
    }

    pub fn can_visit_next_entry(&mut self) -> bool {
        if self.entries_visited >= self.max_entry_count {
            return false;
        }
        if self.started_at.elapsed() >= self.time_limit {
            return false;
        }
        self.entries_visited += 1;
        true
    }

    pub fn should_continue(&self) -> bool {
        self.entries_visited < self.max_entry_count && self.started_at.elapsed() < self.time_limit
    }
}

/// Clamps file modification timestamps that are slightly in the future (up to 5 seconds) to `now`
/// and replaces timestamps further in the future with the previous valid timestamp
/// (`FutureModificationDateClamp`, `LocalAgentSessionScanner.swift:6-30`).
#[derive(Debug, Clone)]
pub struct FutureModificationDateClamp {
    now: Timestamp,
    tolerance_ms: i64,
    previous_valid_date: Option<Timestamp>,
}

impl FutureModificationDateClamp {
    pub const DEFAULT_TOLERANCE_SECONDS: f64 = 5.0;

    pub fn new(now: Timestamp) -> Self {
        Self::with_tolerance(now, Self::DEFAULT_TOLERANCE_SECONDS)
    }

    pub fn with_tolerance(now: Timestamp, tolerance_seconds: f64) -> Self {
        let tolerance_ms = (tolerance_seconds * 1000.0).round() as i64;
        Self {
            now,
            tolerance_ms,
            previous_valid_date: None,
        }
    }

    pub fn clamp(&mut self, modified_at: Timestamp) -> Option<Timestamp> {
        let now_ms = self.now.as_millisecond();
        let mod_ms = modified_at.as_millisecond();
        if mod_ms <= now_ms {
            self.previous_valid_date = Some(modified_at);
            return Some(modified_at);
        }
        if mod_ms <= now_ms.saturating_add(self.tolerance_ms) {
            self.previous_valid_date = Some(self.now);
            return Some(self.now);
        }
        self.previous_valid_date
    }
}

/// Agent session provider kind (`AgentSessionProvider`, `AgentSession.swift:6-17`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AgentSessionProvider {
    Codex,
    Claude,
    Pi,
}

/// Observed local agent session activity (`LocalAgentSessionScanner.swift`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedAgentSession {
    pub provider: AgentSessionProvider,
    pub pid: Option<i32>,
    pub cwd: Option<String>,
    pub last_activity_at: Option<Timestamp>,
}

/// Result of [`LocalAgentSessionScanner::scan_with_activity`] (`LocalAgentSessionScanner.ScanResult`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalActivityScanResult {
    pub sessions: Vec<ObservedAgentSession>,
    pub latest_rollout_activity_at: Option<Timestamp>,
}

impl LocalActivityScanResult {
    /// Returns the newest coding-activity timestamp across live sessions and Codex rollout files
    /// (`AgentSessionsStore.LocalScanResult.latestActivityAt`, `AgentSessionsStore.swift:28-36`).
    pub fn latest_activity_at(&self) -> Option<Timestamp> {
        let session_latest = self
            .sessions
            .iter()
            .filter_map(|s| s.last_activity_at)
            .max();
        match (session_latest, self.latest_rollout_activity_at) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }
}

/// Snapshot of a candidate agent process (`AgentPSOutputParser.ProcessSnapshot`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProcessSnapshot {
    pub pid: i32,
    pub provider: AgentSessionProvider,
    pub command: String,
    pub executable_name: Option<String>,
}

/// Maps working directory paths to Claude's escaped project directory names
/// (`ClaudeSessionProjectMapper`, `AgentSession.swift:412-432`).
pub struct ClaudeSessionProjectMapper;

impl ClaudeSessionProjectMapper {
    pub fn escaped_directory_name(cwd: &str) -> String {
        let trimmed = cwd.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        let normalized = if trimmed.len() > 1 {
            trimmed.trim_end_matches('/')
        } else {
            trimmed
        };
        normalized
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' {
                    ch
                } else {
                    '-'
                }
            })
            .collect()
    }
}

/// Locates Claude Desktop project roots under `~/Library/Application Support/Claude`
/// (`ClaudeDesktopProjectsLocator.swift`).
pub struct ClaudeDesktopProjectsLocator;

impl ClaudeDesktopProjectsLocator {
    const SKIPPED_DIRECTORY_NAMES: &[&str] = &[
        ".build",
        ".git",
        "build",
        "DerivedData",
        "node_modules",
        "outputs",
        "target",
    ];

    pub fn roots(home_dir: &Path, budget: &mut DirectoryMetadataScanBudget) -> Vec<PathBuf> {
        let claude_support = home_dir
            .join("Library")
            .join("Application Support")
            .join("Claude");
        let base_roots = [
            claude_support.join("local-agent-mode-sessions"),
            claude_support.join("claude-code-sessions"),
        ];
        let mut discovered = BTreeSet::new();
        for base in base_roots {
            Self::walk_for_projects(&base, 0, budget, &mut discovered);
        }
        discovered.into_iter().collect()
    }

    fn walk_for_projects(
        dir: &Path,
        depth: usize,
        budget: &mut DirectoryMetadataScanBudget,
        discovered: &mut BTreeSet<PathBuf>,
    ) {
        if depth > CLAUDE_DESKTOP_LOCATOR_MAX_DEPTH || !budget.should_continue() {
            return;
        }
        let Ok(meta) = fs::symlink_metadata(dir) else {
            return;
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut subdirs = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            if !budget.can_visit_next_entry() {
                break;
            }
            let path = entry.path();
            let Ok(entry_meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if entry_meta.file_type().is_symlink() || !entry_meta.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if Self::SKIPPED_DIRECTORY_NAMES.contains(&name_str.as_ref()) {
                continue;
            }
            if name_str == "projects" {
                discovered.insert(path);
            } else if depth < CLAUDE_DESKTOP_LOCATOR_MAX_DEPTH {
                subdirs.push(path);
            }
        }
        subdirs.sort();
        for subdir in subdirs {
            if !budget.should_continue() {
                break;
            }
            Self::walk_for_projects(&subdir, depth + 1, budget, discovered);
        }
    }
}

/// Parses process command lines to identify live `codex`, `claude`, and `pi` / `omp` sessions
/// (`AgentPSOutputParser`, `AgentSession.swift:684-800`).
pub struct AgentPSOutputParser;

impl AgentPSOutputParser {
    const WRAPPER_BINARIES: &[&str] = &[
        "sh",
        "bash",
        "zsh",
        "fish",
        "dash",
        "ksh",
        "csh",
        "tcsh",
        "env",
        "sudo",
        "login",
        "nohup",
        "time",
        "nice",
        "arch",
        "caffeinate",
        "xcrun",
        "node",
        "nodejs",
        "bun",
        "npx",
        "pnpm",
        "yarn",
        "corepack",
        "deno",
        "tsx",
    ];

    const NON_INTERACTIVE_SUBCOMMANDS: &[&str] = &[
        "mcp",
        "mcp-server",
        "app-server",
        "exec",
        "review",
        "doctor",
        "login",
        "logout",
        "version",
        "help",
        "--version",
        "--help",
        "-v",
        "-h",
        "-V",
        "completion",
        "completions",
        "config",
        "auth",
        "update",
        "upgrade",
        "install",
        "uninstall",
        "migrate",
        "setup-token",
        "daemon",
        "usage",
        "cost",
        "status",
        "diagnose",
        "sessions",
        "cache",
        "cookie",
        "secret",
        "serve",
    ];

    pub fn parse_process(pid: i32, command: &str) -> Option<AgentProcessSnapshot> {
        let tokens: Vec<&str> = command.split_whitespace().collect();
        if tokens.is_empty() {
            return None;
        }
        let mut idx = 0;
        while idx < tokens.len() {
            let token = tokens[idx];
            if idx > 0 && token.contains('=') && !token.starts_with('-') {
                idx += 1;
                continue;
            }
            let base = Path::new(token)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(token);
            let lower = base.to_ascii_lowercase();
            if Self::WRAPPER_BINARIES.contains(&lower.as_str()) {
                idx += 1;
                while idx < tokens.len() && tokens[idx].starts_with('-') {
                    idx += 1;
                }
                continue;
            }
            let provider = Self::classify_binary_token(token, &lower)?;
            for &arg in &tokens[idx + 1..] {
                let arg_lower = arg.to_ascii_lowercase();
                if Self::NON_INTERACTIVE_SUBCOMMANDS.contains(&arg_lower.as_str()) {
                    return None;
                }
                if !arg.starts_with('-') {
                    break;
                }
            }
            return Some(AgentProcessSnapshot {
                pid,
                provider,
                command: command.to_string(),
                executable_name: Some(lower),
            });
        }
        None
    }

    fn classify_binary_token(token: &str, lower_basename: &str) -> Option<AgentSessionProvider> {
        if lower_basename == "codex" || lower_basename.starts_with("codex-") {
            if lower_basename.contains("codexbar") {
                return None;
            }
            return Some(AgentSessionProvider::Codex);
        }
        if lower_basename == "claude" || lower_basename.starts_with("claude-") {
            return Some(AgentSessionProvider::Claude);
        }
        if lower_basename == "pi"
            || lower_basename == "omp"
            || lower_basename == "pi-coding-agent"
            || lower_basename == "oh-my-pi"
            || token.contains("@mariozechner/pi-coding-agent")
            || token.contains("oh-my-pi")
        {
            return Some(AgentSessionProvider::Pi);
        }
        None
    }
}

/// Scanner for local coding-agent sessions and rollout activity (`LocalAgentSessionScanner.swift`).
#[derive(Debug, Clone)]
pub struct LocalAgentSessionScanner {
    config: SessionScanConfig,
    local_probe: LocalProbe,
    environment: BTreeMap<String, SecretString>,
    home_dir: Option<PathBuf>,
    lsof_executable: Option<PathBuf>,
    injected_processes: Option<Vec<AgentProcessSnapshot>>,
    injected_cwds: Option<BTreeMap<i32, String>>,
    injected_process_envs: Option<BTreeMap<i32, BTreeMap<String, String>>>,
}

impl LocalAgentSessionScanner {
    pub fn new(
        config: SessionScanConfig,
        local_probe: LocalProbe,
        environment: BTreeMap<String, SecretString>,
        home_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            config,
            local_probe,
            environment,
            home_dir,
            lsof_executable: default_lsof_executable(),
            injected_processes: None,
            injected_cwds: None,
            injected_process_envs: None,
        }
    }

    pub fn with_injected_processes(
        mut self,
        processes: Vec<AgentProcessSnapshot>,
        cwds: BTreeMap<i32, String>,
    ) -> Self {
        self.injected_processes = Some(processes);
        self.injected_cwds = Some(cwds);
        self
    }

    pub fn with_injected_process_envs(
        mut self,
        envs: BTreeMap<i32, BTreeMap<String, String>>,
    ) -> Self {
        self.injected_process_envs = Some(envs);
        self
    }

    /// Scans live agent processes, Claude/Pi session transcripts, and recent Codex rollout files,
    /// returning both observed sessions and `latest_rollout_activity_at`
    /// (`LocalAgentSessionScanner.scanWithActivity`, `LocalAgentSessionScanner.swift:66-156`).
    pub async fn scan_with_activity(
        &self,
        now: Timestamp,
        include_file_only_sessions: bool,
        include_rollout_activity: bool,
    ) -> LocalActivityScanResult {
        let mut processes = self.enumerate_agent_processes().await;
        processes.sort_by_key(|a| std::cmp::Reverse(a.pid));
        processes.truncate(self.config.max_process_count);

        let pids: Vec<i32> = processes.iter().map(|p| p.pid).collect();
        let cwds_by_pid = self.resolve_cwds(&pids).await;

        let mut directory_scan_budget = DirectoryMetadataScanBudget::new(
            self.config.max_directory_entry_count,
            self.config.max_directory_depth,
            self.config.max_directory_scan_duration,
        );

        let home_dir = self.resolved_home_dir();

        // 1. Live Claude processes and their transcript activity (`LocalAgentSessionScanner.swift:92-122`).
        let claude_processes: Vec<&AgentProcessSnapshot> = processes
            .iter()
            .filter(|p| p.provider == AgentSessionProvider::Claude)
            .collect();
        let mut claude_cwds: Vec<String> = claude_processes
            .iter()
            .filter_map(|p| cwds_by_pid.get(&p.pid).cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        claude_cwds.sort();

        let claude_transcripts_by_cwd = if !claude_cwds.is_empty()
            && let Some(ref home) = home_dir
        {
            self.scan_claude_transcripts_by_cwd(home, &claude_cwds, now, &mut directory_scan_budget)
        } else {
            BTreeMap::new()
        };

        let mut sessions = Vec::new();

        // Group live Claude processes by CWD so unambiguous transcripts attach to live sessions.
        let mut claude_by_cwd: BTreeMap<Option<String>, Vec<&AgentProcessSnapshot>> =
            BTreeMap::new();
        for proc_snap in &claude_processes {
            let cwd = cwds_by_pid.get(&proc_snap.pid).cloned();
            claude_by_cwd.entry(cwd).or_default().push(*proc_snap);
        }
        for (cwd_opt, group) in claude_by_cwd {
            let transcripts = cwd_opt
                .as_ref()
                .and_then(|cwd| claude_transcripts_by_cwd.get(cwd))
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            // Upstream (`AgentSessionCorrelation.correlateClaudeSessions`): when the number of
            // transcripts is >= group.len() (or group.len() == 1 with >= 1 transcript), pair
            // newest processes with newest transcripts; if ambiguous (multiple processes, fewer
            // transcripts), leave `last_activity_at` as `None`.
            let unambiguous = transcripts.len() >= group.len();
            for (idx, proc_snap) in group.iter().enumerate() {
                let last_activity_at = if unambiguous {
                    transcripts.get(idx).copied()
                } else {
                    None
                };
                sessions.push(ObservedAgentSession {
                    provider: AgentSessionProvider::Claude,
                    pid: Some(proc_snap.pid),
                    cwd: cwd_opt.clone(),
                    last_activity_at,
                });
            }
        }

        // 2. Live Pi/OMP processes and their transcript activity (`PiFamilySessionScanner.swift`).
        let pi_processes: Vec<&AgentProcessSnapshot> = processes
            .iter()
            .filter(|p| p.provider == AgentSessionProvider::Pi)
            .collect();
        if !pi_processes.is_empty()
            && let Some(ref home) = home_dir
        {
            let pi_sessions = self.scan_pi_sessions(
                home,
                &pi_processes,
                &cwds_by_pid,
                now,
                &mut directory_scan_budget,
            );
            sessions.extend(pi_sessions);
        }

        // 3. Live Codex processes + Codex rollout activity (`LocalAgentSessionScanner.swift:137-155`).
        for proc_snap in processes
            .iter()
            .filter(|p| p.provider == AgentSessionProvider::Codex)
        {
            sessions.push(ObservedAgentSession {
                provider: AgentSessionProvider::Codex,
                pid: Some(proc_snap.pid),
                cwd: cwds_by_pid.get(&proc_snap.pid).cloned(),
                last_activity_at: None,
            });
        }

        let latest_rollout_activity_at = if (include_file_only_sessions || include_rollout_activity)
            && let Some(ref home) = home_dir
        {
            let candidates = self.codex_rollout_candidates(home, now, &mut directory_scan_budget);
            candidates.first().copied()
        } else {
            None
        };

        LocalActivityScanResult {
            sessions,
            latest_rollout_activity_at,
        }
    }

    fn resolved_home_dir(&self) -> Option<PathBuf> {
        if let Some(ref home) = self.home_dir {
            return Some(home.clone());
        }
        self.environment
            .get("HOME")
            .map(|s| PathBuf::from(s.expose_secret()))
            .filter(|p| !p.as_os_str().is_empty())
    }

    fn resolved_codex_sessions_root(&self, home_dir: &Path) -> PathBuf {
        if let Some(codex_home) = self
            .environment
            .get("CODEX_HOME")
            .map(|s| s.expose_secret().trim())
            .filter(|s| !s.is_empty())
        {
            return PathBuf::from(codex_home).join("sessions");
        }
        home_dir.join(".codex").join("sessions")
    }

    async fn enumerate_agent_processes(&self) -> Vec<AgentProcessSnapshot> {
        if let Some(ref injected) = self.injected_processes {
            return injected.clone();
        }
        let call = BrokerCall {
            interaction: FetchInteraction::Background,
            cancel: CancellationToken::new(),
            request_id: "coding-activity-procscan".to_string(),
        };
        let Ok(probe_processes) = self
            .local_probe
            .same_user_processes(&call, |view| {
                let cmd = view.command_line();
                AgentPSOutputParser::parse_process(1, &cmd).is_some()
            })
            .await
        else {
            return Vec::new();
        };

        probe_processes
            .into_iter()
            .filter_map(|proc_entry| {
                let cmd = proc_entry.argv.command_line();
                AgentPSOutputParser::parse_process(proc_entry.identity.pid, &cmd)
            })
            .collect()
    }

    async fn resolve_cwds(&self, pids: &[i32]) -> BTreeMap<i32, String> {
        if let Some(ref injected) = self.injected_cwds {
            return injected
                .iter()
                .filter(|(pid, _)| pids.contains(pid))
                .map(|(pid, cwd)| (*pid, cwd.clone()))
                .collect();
        }
        if pids.is_empty() {
            return BTreeMap::new();
        }
        let mut resolved = BTreeMap::new();
        let mut missing = Vec::new();
        let proc_root = self.local_probe.proc_root();
        for &pid in pids {
            if let Some(cwd) = procscan::process_cwd(proc_root, pid)
                && let Some(cwd_str) = cwd.to_str()
                && !cwd_str.is_empty()
            {
                resolved.insert(pid, cwd_str.to_string());
            } else {
                missing.push(pid);
            }
        }
        if !missing.is_empty()
            && let Some(ref lsof_path) = self.lsof_executable
            && let Ok(abs_lsof) = AbsolutePath::new(lsof_path.clone())
        {
            let pid_arg = missing
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let args = vec![
                std::ffi::OsString::from("-a"),
                std::ffi::OsString::from("-p"),
                std::ffi::OsString::from(pid_arg),
                std::ffi::OsString::from("-d"),
                std::ffi::OsString::from("cwd"),
                std::ffi::OsString::from("-Fn"),
            ];
            let env = ProcessEnv::from_allowlist(
                self.environment
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.expose_secret())),
            );
            let mut cmd = Command::new(abs_lsof, args)
                .with_env(env)
                .timeout(LSOF_TIMEOUT);
            cmd.set_stdin(StdinSpec::Null);
            cmd.set_stdout(StreamPolicy::Capture {
                cap: LSOF_OUTPUT_CAP,
            });
            cmd.set_stderr(StreamPolicy::Discard);
            cmd.set_launch_mode(LaunchMode::Group);
            let call = BrokerCall {
                interaction: FetchInteraction::Background,
                cancel: CancellationToken::new(),
                request_id: "coding-activity-lsof-cwd".to_string(),
            };
            let registry = crate::brokers::process::ProcessRegistry::new();
            let gate = ariadusage_core::gates::LaunchGate::default();
            if let Ok(output) = process::run(call, cmd, &registry, &gate).await
                && let Ok(text) = String::from_utf8(output.stdout.to_vec())
            {
                resolved.extend(parse_lsof_cwd_output(&text));
            }
        }
        resolved
    }

    fn scan_claude_transcripts_by_cwd(
        &self,
        home_dir: &Path,
        cwds: &[String],
        now: Timestamp,
        budget: &mut DirectoryMetadataScanBudget,
    ) -> BTreeMap<String, Vec<Timestamp>> {
        let mut roots = vec![home_dir.join(".claude").join("projects")];
        roots.extend(ClaudeDesktopProjectsLocator::roots(home_dir, budget));

        let mut clamp = FutureModificationDateClamp::new(now);
        let mut result: BTreeMap<String, Vec<Timestamp>> = BTreeMap::new();

        for cwd in cwds {
            if !budget.should_continue() {
                break;
            }
            let escaped = ClaudeSessionProjectMapper::escaped_directory_name(cwd);
            if escaped.is_empty() {
                continue;
            }
            let mut mtimes = Vec::new();
            for root in &roots {
                if !budget.should_continue() {
                    break;
                }
                let project_dir = root.join(&escaped);
                let Ok(dir_meta) = fs::symlink_metadata(&project_dir) else {
                    continue;
                };
                if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() {
                    continue;
                }
                let Ok(entries) = fs::read_dir(&project_dir) else {
                    continue;
                };
                for entry in entries.filter_map(Result::ok) {
                    if !budget.can_visit_next_entry() {
                        break;
                    }
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    let Ok(meta) = fs::symlink_metadata(&path) else {
                        continue;
                    };
                    if meta.file_type().is_symlink() || !meta.is_file() {
                        continue;
                    }
                    let Some(modified_ts) = system_time_to_timestamp(meta.modified().ok()) else {
                        continue;
                    };
                    if let Some(clamped) = clamp.clamp(modified_ts) {
                        mtimes.push(clamped);
                    }
                }
            }
            mtimes.sort_by(|a, b| b.cmp(a));
            mtimes.truncate(self.config.max_claude_transcript_count_per_project);
            if !mtimes.is_empty() {
                result.insert(cwd.clone(), mtimes);
            }
        }

        result
    }

    fn scan_pi_sessions(
        &self,
        home_dir: &Path,
        pi_processes: &[&AgentProcessSnapshot],
        cwds_by_pid: &BTreeMap<i32, String>,
        now: Timestamp,
        budget: &mut DirectoryMetadataScanBudget,
    ) -> Vec<ObservedAgentSession> {
        let mut clamp = FutureModificationDateClamp::new(now);
        let mut sessions = Vec::new();

        for proc_snap in pi_processes {
            let cwd = cwds_by_pid.get(&proc_snap.pid).cloned();
            let last_activity_at = if let Some(ref cwd_str) = cwd
                && budget.should_continue()
            {
                let proc_env = if let Some(ref injected) = self.injected_process_envs {
                    injected.get(&proc_snap.pid).cloned().unwrap_or_default()
                } else {
                    procscan::read_selected_environment(
                        self.local_probe.proc_root(),
                        proc_snap.pid,
                        &["PI_CODING_AGENT_DIR", "OMP_CODING_AGENT_DIR"],
                    )
                };
                let sessions_root = resolve_pi_sessions_root(
                    home_dir,
                    proc_snap.executable_name.as_deref(),
                    &proc_env,
                    &self.environment,
                );
                let bucket_dir = sessions_root.join(encode_pi_cwd_bucket(cwd_str));
                let mut mtimes = Vec::new();
                if let Ok(dir_meta) = fs::symlink_metadata(&bucket_dir)
                    && !dir_meta.file_type().is_symlink()
                    && dir_meta.is_dir()
                    && let Ok(entries) = fs::read_dir(&bucket_dir)
                {
                    for entry in entries.filter_map(Result::ok) {
                        if !budget.can_visit_next_entry() {
                            break;
                        }
                        let path = entry.path();
                        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                            continue;
                        }
                        let Ok(meta) = fs::symlink_metadata(&path) else {
                            continue;
                        };
                        if meta.file_type().is_symlink() || !meta.is_file() {
                            continue;
                        }
                        if let Some(modified_ts) = system_time_to_timestamp(meta.modified().ok())
                            && let Some(clamped) = clamp.clamp(modified_ts)
                        {
                            mtimes.push(clamped);
                        }
                    }
                }
                mtimes.into_iter().max()
            } else {
                None
            };
            sessions.push(ObservedAgentSession {
                provider: AgentSessionProvider::Pi,
                pid: Some(proc_snap.pid),
                cwd,
                last_activity_at,
            });
        }

        sessions
    }

    fn codex_rollout_candidates(
        &self,
        home_dir: &Path,
        now: Timestamp,
        budget: &mut DirectoryMetadataScanBudget,
    ) -> Vec<Timestamp> {
        let sessions_root = self.resolved_codex_sessions_root(home_dir);
        let Ok(root_meta) = fs::symlink_metadata(&sessions_root) else {
            return Vec::new();
        };
        if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
            return Vec::new();
        }

        let day_dirs = recent_codex_day_directories(&sessions_root, now);
        let mut clamp = FutureModificationDateClamp::new(now);
        let mut candidates = Vec::new();

        for day_dir in day_dirs {
            if !budget.should_continue() {
                break;
            }
            let Ok(day_meta) = fs::symlink_metadata(&day_dir) else {
                continue;
            };
            if day_meta.file_type().is_symlink() || !day_meta.is_dir() {
                continue;
            }
            let Ok(entries) = fs::read_dir(&day_dir) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                if !budget.can_visit_next_entry() {
                    break;
                }
                let file_name = entry.file_name();
                let name = file_name.to_string_lossy();
                if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                    continue;
                }
                let path = entry.path();
                let Ok(meta) = fs::symlink_metadata(&path) else {
                    continue;
                };
                if meta.file_type().is_symlink() || !meta.is_file() {
                    continue;
                }
                if let Some(modified_ts) = system_time_to_timestamp(meta.modified().ok())
                    && let Some(clamped) = clamp.clamp(modified_ts)
                {
                    candidates.push(clamped);
                }
            }
        }

        candidates.sort_by(|a, b| b.cmp(a));
        candidates.truncate(self.config.max_codex_rollout_count);
        candidates
    }
}

fn recent_codex_day_directories(sessions_root: &Path, now: Timestamp) -> Vec<PathBuf> {
    let tz = TimeZone::try_system().unwrap_or(TimeZone::UTC);
    let today = now.to_zoned(tz.clone()).date();
    let yesterday = today.yesterday().unwrap_or(today);
    let mut dirs = Vec::with_capacity(2);
    for date in [today, yesterday] {
        let day_path = sessions_root
            .join(format!("{:04}", date.year()))
            .join(format!("{:02}", date.month()))
            .join(format!("{:02}", date.day()));
        if !dirs.contains(&day_path) {
            dirs.push(day_path);
        }
    }
    dirs
}

fn resolve_pi_sessions_root(
    home_dir: &Path,
    executable_name: Option<&str>,
    proc_env: &BTreeMap<String, String>,
    fallback_env: &BTreeMap<String, SecretString>,
) -> PathBuf {
    let is_omp = executable_name.is_some_and(|name| name == "omp" || name.contains("oh-my-pi"));
    let env_key = if is_omp {
        "OMP_CODING_AGENT_DIR"
    } else {
        "PI_CODING_AGENT_DIR"
    };
    if let Some(dir) = proc_env
        .get(env_key)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return expand_tilde(dir, home_dir).join("sessions");
    }
    if let Some(dir) = fallback_env
        .get(env_key)
        .map(|s| s.expose_secret().trim())
        .filter(|s| !s.is_empty())
    {
        return expand_tilde(dir, home_dir).join("sessions");
    }
    let dot_dir = if is_omp { ".omp" } else { ".pi" };
    home_dir.join(dot_dir).join("agent").join("sessions")
}

fn expand_tilde(raw: &str, home_dir: &Path) -> PathBuf {
    if raw == "~" {
        return home_dir.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home_dir.join(rest);
    }
    PathBuf::from(raw)
}

fn encode_pi_cwd_bucket(cwd: &str) -> String {
    let stripped = cwd.trim_start_matches(['/', '\\']);
    let replaced: String = stripped
        .chars()
        .map(|ch| {
            if matches!(ch, '/' | '\\' | ':') {
                '-'
            } else {
                ch
            }
        })
        .collect();
    format!("--{replaced}--")
}

fn parse_lsof_cwd_output(output: &str) -> BTreeMap<i32, String> {
    let mut map = BTreeMap::new();
    let mut current_pid: Option<i32> = None;
    for line in output.lines() {
        if let Some(pid_str) = line.strip_prefix('p') {
            current_pid = pid_str.trim().parse::<i32>().ok();
        } else if let Some(path) = line.strip_prefix('n')
            && let Some(pid) = current_pid
            && !path.trim().is_empty()
        {
            map.insert(pid, path.trim().to_string());
        }
    }
    map
}

fn default_lsof_executable() -> Option<PathBuf> {
    for candidate in ["/usr/sbin/lsof", "/usr/bin/lsof"] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn system_time_to_timestamp(time: Option<SystemTime>) -> Option<Timestamp> {
    let duration = time?.duration_since(UNIX_EPOCH).ok()?;
    let millis = i64::try_from(duration.as_millis()).ok()?;
    Timestamp::from_millisecond(millis).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Rust-added: smoke test for macOS raw thermal state mapping (`AdaptiveRefreshPolicy.swift:31-37`).
    #[test]
    fn macos_thermal_state_maps_nominal_fair_and_constrained() {
        assert_eq!(map_macos_thermal_state(0), ThermalPressure::Nominal);
        assert_eq!(map_macos_thermal_state(1), ThermalPressure::Nominal);
        assert_eq!(map_macos_thermal_state(2), ThermalPressure::Constrained);
        assert_eq!(map_macos_thermal_state(3), ThermalPressure::Constrained);
    }

    // Rust-added: smoke test for Linux thermal sysfs trip-point and fallback threshold evaluation.
    #[test]
    fn linux_thermal_sysfs_detects_constrained_and_nominal_zones() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let zone0 = tmp.path().join("thermal_zone0");
        fs::create_dir_all(&zone0).expect("zone0");
        fs::write(zone0.join("temp"), "52000\n").expect("temp");
        fs::write(zone0.join("trip_point_0_type"), "passive\n").expect("type");
        fs::write(zone0.join("trip_point_0_temp"), "80000\n").expect("trip temp");

        assert_eq!(
            read_linux_thermal_pressure_from_sysfs(tmp.path()),
            ThermalPressure::Nominal
        );

        fs::write(zone0.join("temp"), "82000\n").expect("constrained temp");
        assert_eq!(
            read_linux_thermal_pressure_from_sysfs(tmp.path()),
            ThermalPressure::Constrained
        );
    }

    // Rust-added: smoke test for UPower / PowerProfiles low-power evaluation.
    #[test]
    fn upower_low_power_evaluation_respects_power_saver_and_battery_thresholds() {
        assert!(evaluate_upower_low_power_mode(
            Some(true),
            Some(false),
            Some(1),
            Some(95.0)
        ));
        assert!(!evaluate_upower_low_power_mode(
            Some(false),
            Some(true),
            Some(1),
            Some(65.0)
        ));
        assert!(evaluate_upower_low_power_mode(
            Some(false),
            Some(true),
            Some(3),
            Some(40.0)
        ));
        assert!(evaluate_upower_low_power_mode(
            None,
            Some(true),
            None,
            Some(15.0)
        ));
    }

    // Rust-added: smoke test for `LocalAgentSessionScanner::scan_with_activity` with live Claude
    // transcripts, Codex rollout mtimes, future mtime clamping, and shared metadata scan budget.
    #[tokio::test]
    async fn local_agent_session_scanner_observes_claude_and_codex_rollout_activity() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path();
        let now = Timestamp::now();

        let project_cwd = "/Users/alex/work/repo-alpha";
        let escaped = ClaudeSessionProjectMapper::escaped_directory_name(project_cwd);
        let claude_project_dir = home.join(".claude").join("projects").join(escaped);
        fs::create_dir_all(&claude_project_dir).expect("claude project dir");
        let transcript_path = claude_project_dir.join("session-1.jsonl");
        fs::write(&transcript_path, "{}\n").expect("write transcript");

        let tz = TimeZone::try_system().unwrap_or(TimeZone::UTC);
        let today = now.to_zoned(tz).date();
        let codex_day_dir = home
            .join(".codex")
            .join("sessions")
            .join(format!("{:04}", today.year()))
            .join(format!("{:02}", today.month()))
            .join(format!("{:02}", today.day()));
        fs::create_dir_all(&codex_day_dir).expect("codex day dir");
        let rollout_path = codex_day_dir.join("rollout-2026-10-25T05-00-00-abc.jsonl");
        fs::write(&rollout_path, "not-read-by-activity-only-scan\n").expect("write rollout");

        let mut env = BTreeMap::new();
        env.insert(
            "HOME".to_string(),
            SecretString::new(home.to_string_lossy().into_owned()),
        );

        let mut cwds = BTreeMap::new();
        cwds.insert(4242, project_cwd.to_string());
        let scanner = LocalAgentSessionScanner::new(
            SessionScanConfig::default(),
            LocalProbe::new(),
            env,
            Some(home.to_path_buf()),
        )
        .with_injected_processes(
            vec![AgentProcessSnapshot {
                pid: 4242,
                provider: AgentSessionProvider::Claude,
                command: "claude".to_string(),
                executable_name: Some("claude".to_string()),
            }],
            cwds,
        );

        let scan_now = Timestamp::from_millisecond(now.as_millisecond() + 1_000).expect("scan_now");
        let result = scanner.scan_with_activity(scan_now, false, true).await;
        assert_eq!(result.sessions.len(), 1);
        assert!(result.sessions[0].last_activity_at.is_some());
        assert!(result.latest_rollout_activity_at.is_some());
        assert!(result.latest_activity_at().is_some());
    }
}
