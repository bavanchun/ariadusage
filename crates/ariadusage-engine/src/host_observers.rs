// Ported from CodexBar Sources/CodexBar/AgentSessionsStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+AgentSessions.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/AdaptiveRefreshPolicy.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/SettingsStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Host power, thermal, and local coding-activity observers feeding [`crate::EngineRuntime`].
//!
//! - **Power & thermal**:
//!   - macOS: `NSProcessInfo` `isLowPowerModeEnabled` and `thermalState` are sampled on the main
//!     thread in `ariadusage-macos/src/appkit/` and forwarded over IPC (`setLowPowerMode.on` /
//!     `setLowPowerMode.off`, `setThermalPressure.nominal` / `setThermalPressure.constrained`)
//!     when changed; [`map_macos_thermal_state`] ports upstream
//!     `AdaptiveRefreshSignalCollector.currentThermalPressure`.
//!   - Linux: queries `org.freedesktop.UPower.PowerProfiles` (`ActiveProfile == "power-saver"`)
//!     over D-Bus (`zbus`) and reads `/sys/class/thermal/thermal_zone*` (following symlinks into
//!     `/sys/devices/`) via [`read_linux_thermal_pressure_from_sysfs`].
//! - **Coding activity**:
//!   - [`LocalAgentSessionScanner::scan_with_activity`] (owned by [`crate::sessions`]) enumerates
//!     same-user agent processes through [`crate::brokers::local_probe::LocalProbe`] / `procscan`
//!     and inspects live Claude / Pi transcripts and recent Codex rollout modification dates under
//!     a shared [`DirectoryMetadataScanBudget`].
//!   - [`AgentSessionRefreshGate`] coalesces overlapping refreshes and gates local scanning on
//!     `!low_power_mode_enabled && thermal_pressure == ThermalPressure::Nominal`, running on the
//!     30-second periodic cadence (`LOCAL_ACTIVITY_SCAN_INTERVAL`) and on menu-open / settings
//!     transitions.

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::time::Duration;

use ariadusage_core::pipeline::BoxFuture;
use ariadusage_core::refresh::ThermalPressure;

pub use crate::sessions::{
    AgentPSOutputParser, AgentProcessSnapshot, AgentSessionProvider, AgentSessionRefreshGate,
    ClaudeDesktopProjectsLocator, ClaudeSessionProjectMapper, DirectoryMetadataScanBudget,
    FutureModificationDateClamp, LOCAL_ACTIVITY_SCAN_INTERVAL, LocalActivityScanResult,
    LocalAgentSessionScanner, LocalSessionScanResult, ObservedAgentSession, SessionScanConfig,
    encode_pi_cwd_bucket, recent_codex_day_directories, resolve_pi_sessions_root,
};

/// Default Linux sysfs thermal directory (`/sys/class/thermal`).
pub const DEFAULT_LINUX_THERMAL_SYSFS_ROOT: &str = "/sys/class/thermal";

/// Default Linux canonical devices prefix (`/sys/devices`) required for symlinked
/// `/sys/class/thermal/thermal_zone*` entries.
pub const DEFAULT_LINUX_DEVICES_PREFIX: &str = "/sys/devices";

/// Default fallback serious thermal threshold in millidegrees Celsius (85°C) when a thermal zone
/// exposes `temp` without usable `trip_point_*_temp` thresholds.
pub const DEFAULT_LINUX_THERMAL_CONSTRAINED_MILLIDEGREES: i64 = 85_000;

/// Maps macOS `NSProcessInfoThermalState` raw integer values (`0 = nominal`, `1 = fair`,
/// `2 = serious`, `3 = critical`) to [`ThermalPressure`]
/// (`AdaptiveRefreshSignalCollector.currentThermalPressure`, `AdaptiveRefreshPolicy.swift:31-37`).
pub const fn map_macos_thermal_state(raw_state: isize) -> ThermalPressure {
    match raw_state {
        0 | 1 => ThermalPressure::Nominal,
        _ => ThermalPressure::Constrained,
    }
}

/// Evaluates whether the Linux `org.freedesktop.UPower.PowerProfiles` `ActiveProfile` indicates
/// low-power mode (`ActiveProfile == "power-saver"`).
pub fn evaluate_upower_low_power_mode(active_profile: Option<&str>) -> bool {
    active_profile.is_some_and(|profile| profile.trim() == "power-saver")
}

/// Reads Linux thermal pressure from `sysfs_thermal_root` (`/sys/class/thermal/thermal_zone*`),
/// accepting symlinked thermal zones whose canonical target resides under `/sys/devices/` (or
/// `sysfs_thermal_root` when testing with non-symlinked directories under `sysfs_thermal_root`).
pub fn read_linux_thermal_pressure_from_sysfs(sysfs_thermal_root: &Path) -> ThermalPressure {
    read_linux_thermal_pressure_from_sysfs_with_devices_prefix(
        sysfs_thermal_root,
        Path::new(DEFAULT_LINUX_DEVICES_PREFIX),
    )
}

/// Reads Linux thermal pressure from `sysfs_thermal_root` (`/sys/class/thermal/thermal_zone*`),
/// following `thermal_zone*` symlinks and accepting each entry only when its canonical path starts
/// with `devices_prefix` (or when it is a real directory directly inside `sysfs_thermal_root`).
///
/// A zone is considered [`ThermalPressure::Constrained`] when its `temp` (in millidegrees Celsius)
/// reaches or exceeds any positive `critical`, `hot`, or `passive` trip point (`trip_point_*_temp`),
/// or when no trip point is usable and `temp >= 85_000` (85°C).
pub fn read_linux_thermal_pressure_from_sysfs_with_devices_prefix(
    sysfs_thermal_root: &Path,
    devices_prefix: &Path,
) -> ThermalPressure {
    let Ok(entries) = fs::read_dir(sysfs_thermal_root) else {
        return ThermalPressure::Nominal;
    };
    let canonical_devices_prefix =
        fs::canonicalize(devices_prefix).unwrap_or_else(|_| devices_prefix.to_path_buf());
    let canonical_sysfs_root =
        fs::canonicalize(sysfs_thermal_root).unwrap_or_else(|_| sysfs_thermal_root.to_path_buf());

    let mut zone_dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with("thermal_zone") {
                return None;
            }
            let path = entry.path();
            let sym_meta = fs::symlink_metadata(&path).ok()?;
            let canonical = fs::canonicalize(&path).ok()?;
            if !canonical.is_dir() {
                return None;
            }
            if sym_meta.file_type().is_symlink() {
                if !canonical.starts_with(&canonical_devices_prefix) {
                    return None;
                }
            } else if !canonical.starts_with(&canonical_devices_prefix)
                && !canonical.starts_with(&canonical_sysfs_root)
            {
                return None;
            }
            Some(canonical)
        })
        .collect();
    zone_dirs.sort();
    zone_dirs.dedup();

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
                let entry_path = entry.path();
                let Ok(entry_meta) = fs::symlink_metadata(&entry_path) else {
                    continue;
                };
                if entry_meta.file_type().is_symlink() || !entry_meta.is_file() {
                    continue;
                }
                let Ok(trip_type) = fs::read_to_string(&entry_path) else {
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

/// Queries Linux `org.freedesktop.UPower.PowerProfiles` (or legacy `net.hadess.PowerProfiles`) over
/// the system D-Bus (`zbus`) to determine whether the `power-saver` profile is active. Returns
/// `None` if the system D-Bus or PowerProfiles service is unavailable.
#[cfg(target_os = "linux")]
pub async fn query_linux_upower_low_power_mode() -> Option<bool> {
    tokio::time::timeout(Duration::from_secs(2), async {
        let conn = zbus::Connection::system().await.ok()?;

        let active_profile = match query_dbus_string_property(
            &conn,
            "org.freedesktop.UPower.PowerProfiles",
            "/org/freedesktop/UPower/PowerProfiles",
            "org.freedesktop.UPower.PowerProfiles",
            "ActiveProfile",
        )
        .await
        {
            Some(profile) => Some(profile),
            None => {
                query_dbus_string_property(
                    &conn,
                    "net.hadess.PowerProfiles",
                    "/net/hadess/PowerProfiles",
                    "net.hadess.PowerProfiles",
                    "ActiveProfile",
                )
                .await
            }
        };

        active_profile.map(|profile| evaluate_upower_low_power_mode(Some(&profile)))
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
    devices_prefix: PathBuf,
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
                devices_prefix: PathBuf::from(DEFAULT_LINUX_DEVICES_PREFIX),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self {
                sysfs_thermal_root: None,
                devices_prefix: PathBuf::from(DEFAULT_LINUX_DEVICES_PREFIX),
            }
        }
    }

    pub fn with_sysfs_thermal_root(sysfs_thermal_root: PathBuf) -> Self {
        Self {
            sysfs_thermal_root: Some(sysfs_thermal_root),
            devices_prefix: PathBuf::from(DEFAULT_LINUX_DEVICES_PREFIX),
        }
    }

    pub fn with_sysfs_and_devices_prefix(
        sysfs_thermal_root: PathBuf,
        devices_prefix: PathBuf,
    ) -> Self {
        Self {
            sysfs_thermal_root: Some(sysfs_thermal_root),
            devices_prefix,
        }
    }
}

impl HostPowerThermalProbe for DefaultHostPowerThermalProbe {
    fn sample<'a>(&'a self) -> BoxFuture<'a, (Option<bool>, Option<ThermalPressure>)> {
        Box::pin(async move {
            let thermal = self.sysfs_thermal_root.as_deref().map(|root| {
                read_linux_thermal_pressure_from_sysfs_with_devices_prefix(
                    root,
                    &self.devices_prefix,
                )
            });
            #[cfg(target_os = "linux")]
            let low_power = query_linux_upower_low_power_mode().await;
            #[cfg(not(target_os = "linux"))]
            let low_power: Option<bool> = None;
            (low_power, thermal)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ariadusage_protocol::secret::SecretString;
    use jiff::Timestamp;
    use jiff::tz::TimeZone;

    use super::*;
    use crate::brokers::local_probe::LocalProbe;

    // Rust-added: smoke test for macOS raw thermal state mapping (`AdaptiveRefreshPolicy.swift:31-37`).
    #[test]
    fn macos_thermal_state_maps_nominal_fair_and_constrained() {
        assert_eq!(map_macos_thermal_state(0), ThermalPressure::Nominal);
        assert_eq!(map_macos_thermal_state(1), ThermalPressure::Nominal);
        assert_eq!(map_macos_thermal_state(2), ThermalPressure::Constrained);
        assert_eq!(map_macos_thermal_state(3), ThermalPressure::Constrained);
    }

    // Rust-added: smoke test for Linux thermal sysfs symlinked thermal_zone* canonicalization
    // under `/sys/devices/`, trip-point evaluation, and 85°C fallback threshold.
    #[test]
    fn linux_thermal_sysfs_follows_symlinks_under_devices_prefix_and_rejects_outside_targets() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sys_class_thermal = tmp.path().join("sys/class/thermal");
        let sys_devices = tmp.path().join("sys/devices");
        let real_zone0 = sys_devices.join("virtual/thermal/thermal_zone0");
        let outside_zone = tmp.path().join("outside/thermal_zone_evil");

        fs::create_dir_all(&sys_class_thermal).expect("sys/class/thermal");
        fs::create_dir_all(&real_zone0).expect("real_zone0");
        fs::create_dir_all(&outside_zone).expect("outside_zone");

        fs::write(real_zone0.join("temp"), "52000\n").expect("temp");
        fs::write(real_zone0.join("trip_point_0_type"), "passive\n").expect("type");
        fs::write(real_zone0.join("trip_point_0_temp"), "80000\n").expect("trip temp");

        // Outside zone has critical temperature, but its symlink points outside `sys/devices`
        // so it must be ignored.
        fs::write(outside_zone.join("temp"), "99000\n").expect("outside temp");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real_zone0, sys_class_thermal.join("thermal_zone0"))
                .expect("symlink zone0");
            std::os::unix::fs::symlink(&outside_zone, sys_class_thermal.join("thermal_zone1"))
                .expect("symlink zone1");
        }

        assert_eq!(
            read_linux_thermal_pressure_from_sysfs_with_devices_prefix(
                &sys_class_thermal,
                &sys_devices,
            ),
            ThermalPressure::Nominal
        );

        fs::write(real_zone0.join("temp"), "82000\n").expect("constrained temp");
        assert_eq!(
            read_linux_thermal_pressure_from_sysfs_with_devices_prefix(
                &sys_class_thermal,
                &sys_devices,
            ),
            ThermalPressure::Constrained
        );
    }

    // Rust-added: smoke test for PowerProfiles `power-saver`-only low-power evaluation.
    #[test]
    fn upower_low_power_evaluation_matches_power_saver_profile_only() {
        assert!(evaluate_upower_low_power_mode(Some("power-saver")));
        assert!(evaluate_upower_low_power_mode(Some(" power-saver\n")));
        assert!(!evaluate_upower_low_power_mode(Some("balanced")));
        assert!(!evaluate_upower_low_power_mode(Some("performance")));
        assert!(!evaluate_upower_low_power_mode(None));
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
