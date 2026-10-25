// Ported from CodexBar Sources/CodexBar/AgentSessionsStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/LocalAgentSessionScanner.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/AgentSession.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/CodexThreadMetadataReader.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeDesktopProjectsLocator.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/PiFamilySessionScanner.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/PiFamilySessionRootResolver.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLISessionsCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Shared local coding-agent session scanner, rollout/transcript correlation, directory metadata
//! scan budget, and refresh gate.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ariadusage_core::gates::LaunchGate;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_core::refresh::ThermalPressure;
use ariadusage_protocol::secret::SecretString;
use jiff::civil::{Date, DateTime, Time};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::brokers::browser::sqlite_copy::copy_database;
use crate::brokers::call::BrokerCall;
use crate::brokers::exec_resolver::{RealResolverFs, resolve_absolute_candidates};
use crate::brokers::local_probe::LocalProbe;
use crate::brokers::process::{
    self, AbsolutePath, Command, LaunchMode, Output as BrokerOutput, ProcessEnv, ProcessRegistry,
    StdinSpec, StreamPolicy, process_ppid_and_start_secs, procscan,
};
use crate::private_tempdir::{create_in, ensure_private_0700_dir};

/// Upstream 30-second periodic local activity scan cadence (`AgentSessionsStore.localActivityScanInterval`).
pub const LOCAL_ACTIVITY_SCAN_INTERVAL: Duration = Duration::from_secs(30);

const DEFAULT_LSOF_CANDIDATES: &[&str] = &["/usr/sbin/lsof", "/usr/bin/lsof"];
const DEFAULT_HOSTNAME_CANDIDATES: &[&str] = &["/bin/hostname", "/usr/bin/hostname"];
const LSOF_TIMEOUT: Duration = Duration::from_secs(5);
const LSOF_OUTPUT_CAP: usize = 256 * 1024;

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

/// Agent session provider kind (`AgentSession.Provider`, `AgentSession.swift:4-8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSessionProvider {
    Codex,
    Claude,
    Pi,
}

impl AgentSessionProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Pi => "pi",
        }
    }
}

/// Pi-family session dialect (`AgentSession.Dialect`, `AgentSession.swift:10-13`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSessionDialect {
    Pi,
    Omp,
}

impl AgentSessionDialect {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Omp => "omp",
        }
    }
}

/// Agent session launch source (`AgentSession.Source`, `AgentSession.swift:15-20`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentSessionSource {
    Cli,
    DesktopApp,
    Ide,
    Unknown,
}

impl AgentSessionSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::DesktopApp => "desktopApp",
            Self::Ide => "ide",
            Self::Unknown => "unknown",
        }
    }
}

/// Agent session activity state (`AgentSession.State`, `AgentSession.swift:22-25`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentSessionState {
    Active,
    Idle,
}

impl AgentSessionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Idle => "idle",
        }
    }
}

/// Local or remote coding-agent session (`AgentSession`, `AgentSession.swift:3-70`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSession {
    pub id: String,
    pub provider: AgentSessionProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialect: Option<AgentSessionDialect>,
    pub source: AgentSessionSource,
    pub state: AgentSessionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    pub host: String,
}

impl AgentSession {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        provider: AgentSessionProvider,
        dialect: Option<AgentSessionDialect>,
        source: AgentSessionSource,
        state: AgentSessionState,
        pid: Option<i32>,
        cwd: Option<String>,
        project_name: Option<String>,
        session_name: Option<String>,
        started_at: Option<Timestamp>,
        last_activity_at: Option<Timestamp>,
        transcript_path: Option<String>,
        host: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            provider,
            dialect,
            source,
            state,
            pid,
            cwd,
            project_name,
            session_name,
            started_at,
            last_activity_at,
            transcript_path,
            host: host.into(),
        }
    }
}

/// Type alias preserving compatibility with `host_observers`.
pub type ObservedAgentSession = AgentSession;

/// Result of [`LocalAgentSessionScanner::scan_with_activity`] (`LocalAgentSessionScanner.ScanResult`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalSessionScanResult {
    pub sessions: Vec<AgentSession>,
    pub latest_rollout_activity_at: Option<Timestamp>,
}

/// Backwards-compatible alias for [`LocalSessionScanResult`].
pub type LocalActivityScanResult = LocalSessionScanResult;

impl LocalSessionScanResult {
    /// Returns the newest coding-activity timestamp across live sessions and rollout files
    /// (`AgentSessionsStore.latestActivityAt`, `AgentSessionsStore.swift:28-36`).
    pub fn latest_activity_at(&self) -> Option<Timestamp> {
        Self::latest_activity_in(&self.sessions, self.latest_rollout_activity_at)
    }

    pub fn latest_activity_in(
        sessions: &[AgentSession],
        rollout_activity_at: Option<Timestamp>,
    ) -> Option<Timestamp> {
        let session_latest = sessions.iter().filter_map(|s| s.last_activity_at).max();
        match (session_latest, rollout_activity_at) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }
}

/// Snapshot of a candidate agent process for `host_observers` injection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProcessSnapshot {
    pub pid: i32,
    pub provider: AgentSessionProvider,
    pub command: String,
    pub executable_name: Option<String>,
}

/// Literal port of upstream `SessionScanConfig` (`AgentSession.swift:72-109`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionScanConfig {
    pub active_window: f64,
    pub file_only_window: f64,
    pub max_process_count: usize,
    pub max_codex_rollout_count: usize,
    pub max_claude_transcript_count_per_project: usize,
    pub max_directory_entry_count: usize,
    pub max_directory_depth: usize,
    pub directory_scan_budget: f64,
    pub adaptive_directory_scan_budget: f64,
}

impl Default for SessionScanConfig {
    fn default() -> Self {
        Self {
            active_window: 120.0,
            file_only_window: 30.0 * 60.0,
            max_process_count: 64,
            max_codex_rollout_count: 128,
            max_claude_transcript_count_per_project: 64,
            max_directory_entry_count: 512,
            max_directory_depth: 1,
            directory_scan_budget: 0.25,
            adaptive_directory_scan_budget: 0.15,
        }
    }
}

impl SessionScanConfig {
    pub fn state(
        &self,
        last_activity_at: Option<Timestamp>,
        now: Timestamp,
        has_live_process: bool,
    ) -> AgentSessionState {
        let Some(last_activity_at) = last_activity_at else {
            return if has_live_process {
                AgentSessionState::Active
            } else {
                AgentSessionState::Idle
            };
        };
        let elapsed = (now.as_millisecond() - last_activity_at.as_millisecond()) as f64 / 1000.0;
        if elapsed <= self.active_window {
            AgentSessionState::Active
        } else {
            AgentSessionState::Idle
        }
    }
}

/// Shared metadata scan budget across Claude transcripts, Pi sessions, and Codex rollout files
/// (`DirectoryMetadataScanBudget`, `AgentSession.swift:111-193`).
#[derive(Clone)]
pub struct DirectoryMetadataScanBudget {
    remaining_entry_count: usize,
    pub max_depth: usize,
    deadline: Timestamp,
    did_visit_entry: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for DirectoryMetadataScanBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectoryMetadataScanBudget")
            .field("remaining_entry_count", &self.remaining_entry_count)
            .field("max_depth", &self.max_depth)
            .field("deadline", &self.deadline)
            .finish()
    }
}

impl DirectoryMetadataScanBudget {
    pub fn new(
        max_entry_count: usize,
        max_depth: usize,
        time_limit: f64,
        started_at: Timestamp,
    ) -> Self {
        let clamped_secs = time_limit.max(0.0);
        let deadline = if clamped_secs.is_infinite() || clamped_secs >= 315_360_000_000.0 {
            Timestamp::MAX
        } else {
            let nanos = (clamped_secs * 1_000_000_000.0).round() as i64;
            started_at
                .checked_add(SignedDuration::from_nanos(nanos))
                .unwrap_or(Timestamp::MAX)
        };
        Self {
            remaining_entry_count: max_entry_count,
            max_depth,
            deadline,
            did_visit_entry: None,
        }
    }

    pub fn with_did_visit_entry(
        mut self,
        did_visit_entry: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        self.did_visit_entry = did_visit_entry;
        self
    }

    pub fn has_time_remaining(&self) -> bool {
        self.has_time_remaining_with(Timestamp::now)
    }

    pub fn has_time_remaining_with(&self, mut clock: impl FnMut() -> Timestamp) -> bool {
        clock() < self.deadline
    }

    pub fn files(&mut self, directory: &Path) -> Vec<PathBuf> {
        self.files_with_clock(directory, Timestamp::now)
    }

    pub fn files_with_clock(
        &mut self,
        directory: &Path,
        clock: impl FnMut() -> Timestamp,
    ) -> Vec<PathBuf> {
        self.entries(directory, clock)
            .into_iter()
            .filter_map(|(url, is_dir)| (!is_dir).then_some(url))
            .collect()
    }

    pub fn child_directories(&mut self, directory: &Path) -> Vec<PathBuf> {
        self.child_directories_with_clock(directory, Timestamp::now)
    }

    pub fn child_directories_with_clock(
        &mut self,
        directory: &Path,
        clock: impl FnMut() -> Timestamp,
    ) -> Vec<PathBuf> {
        self.entries(directory, clock)
            .into_iter()
            .filter_map(|(url, is_dir)| is_dir.then_some(url))
            .collect()
    }

    pub fn compact_map_while_time_remains<T, U>(
        &self,
        urls: &[T],
        mut transform: impl FnMut(&T) -> Option<U>,
    ) -> Vec<U> {
        self.compact_map_while_time_remains_with_clock(urls, Timestamp::now, &mut transform)
    }

    pub fn compact_map_while_time_remains_with_clock<T, U>(
        &self,
        urls: &[T],
        mut clock: impl FnMut() -> Timestamp,
        mut transform: impl FnMut(&T) -> Option<U>,
    ) -> Vec<U> {
        let mut results = Vec::new();
        for url in urls {
            if !self.has_time_remaining_with(&mut clock) {
                break;
            }
            if let Some(result) = transform(url) {
                results.push(result);
            }
        }
        results
    }

    fn entries(
        &mut self,
        directory: &Path,
        mut clock: impl FnMut() -> Timestamp,
    ) -> Vec<(PathBuf, bool)> {
        if self.max_depth == 0 || self.remaining_entry_count == 0 || clock() >= self.deadline {
            return Vec::new();
        }
        let Ok(meta) = fs::symlink_metadata(directory) else {
            return Vec::new();
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Vec::new();
        }
        let Ok(read_dir) = fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut raw_entries: Vec<PathBuf> = read_dir
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                if name.to_string_lossy().starts_with('.') {
                    return None;
                }
                Some(entry.path())
            })
            .collect();
        raw_entries.sort();

        let mut entries = Vec::new();
        let mut iter = raw_entries.into_iter();
        while self.remaining_entry_count > 0 && clock() < self.deadline {
            let Some(url) = iter.next() else {
                break;
            };
            self.remaining_entry_count -= 1;
            if let Some(ref cb) = self.did_visit_entry {
                cb();
            }
            if !self.has_time_remaining_with(&mut clock) {
                break;
            }
            let Ok(entry_meta) = fs::symlink_metadata(&url) else {
                continue;
            };
            if entry_meta.file_type().is_symlink() {
                continue;
            }
            let is_directory = entry_meta.is_dir();
            entries.push((url, is_directory));
        }
        entries
    }
}

/// Clamps future file modification dates using one path-free clamp anchor across scans
/// (`FutureModificationDateClamp`, `LocalAgentSessionScanner.swift:6-24`).
#[derive(Debug, Default)]
pub struct FutureModificationDateClamp {
    clamp_date: Mutex<Option<Timestamp>>,
}

impl FutureModificationDateClamp {
    pub fn new(clamp_date: Option<Timestamp>) -> Self {
        Self {
            clamp_date: Mutex::new(clamp_date),
        }
    }

    pub fn clamp(&self, _url: &Path, modified_at: Timestamp, now: Timestamp) -> Timestamp {
        if modified_at <= now {
            return modified_at;
        }
        let mut guard = self
            .clamp_date
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(clamp_date) = *guard {
            clamp_date.min(now)
        } else {
            *guard = Some(now);
            now
        }
    }
}

/// Locates Claude Desktop project roots under `~/Library/Application Support/Claude`
/// (`ClaudeDesktopProjectsLocator.swift`).
pub struct ClaudeDesktopProjectsLocator;

impl ClaudeDesktopProjectsLocator {
    const SESSION_DIRECTORY_NAMES: &[&str] = &["local-agent-mode-sessions", "claude-code-sessions"];

    const SKIPPED_DIRECTORY_NAMES: &[&str] = &[
        ".build",
        ".git",
        "build",
        "DerivedData",
        "node_modules",
        "outputs",
        "target",
    ];

    pub fn roots(home_directory: &Path) -> Vec<PathBuf> {
        let mut budget =
            DirectoryMetadataScanBudget::new(usize::MAX, 4, f64::INFINITY, Timestamp::now());
        Self::roots_with_budget(home_directory, &mut budget, false)
    }

    pub fn roots_with_budget(
        home_directory: &Path,
        budget: &mut DirectoryMetadataScanBudget,
        include_audit_files: bool,
    ) -> Vec<PathBuf> {
        let claude_support = home_directory
            .join("Library")
            .join("Application Support")
            .join("Claude");
        let session_roots: Vec<PathBuf> = Self::SESSION_DIRECTORY_NAMES
            .iter()
            .map(|name| claude_support.join(name))
            .collect();

        let mut roots = Vec::new();
        let mut queue: Vec<(PathBuf, usize)> =
            session_roots.iter().cloned().map(|u| (u, 0)).collect();
        let mut visited: HashSet<PathBuf> = session_roots
            .iter()
            .map(|p| standardize_path_buf(p))
            .collect();
        let mut next_index = 0;

        while next_index < queue.len() && budget.has_time_remaining() {
            let (current_url, current_depth) = queue[next_index].clone();
            next_index += 1;
            if let Some(projects) = Self::projects_root(&current_url) {
                roots.push(projects);
            }

            if include_audit_files {
                let audit = current_url.join("audit.jsonl");
                if let Ok(meta) = fs::symlink_metadata(&audit)
                    && !meta.file_type().is_symlink()
                    && meta.is_file()
                {
                    roots.push(standardize_path_buf(&audit));
                }
            }

            if current_depth >= budget.max_depth {
                continue;
            }
            for child in budget.child_directories(&current_url) {
                let Some(name) = child.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if Self::SKIPPED_DIRECTORY_NAMES.contains(&name) {
                    continue;
                }
                let Ok(meta) = fs::symlink_metadata(&child) else {
                    continue;
                };
                if meta.file_type().is_symlink() {
                    continue;
                }
                let standardized = standardize_path_buf(&child);
                if visited.insert(standardized.clone()) {
                    queue.push((standardized, current_depth + 1));
                }
            }
        }
        roots
    }

    fn projects_root(base: &Path) -> Option<PathBuf> {
        let projects = base.join(".claude").join("projects");
        let meta = fs::symlink_metadata(&projects).ok()?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return None;
        }
        Some(standardize_path_buf(&projects))
    }
}

/// Discovered Claude transcript file and its clamped modification timestamp
/// (`ClaudeSessionProjectMapper.Transcript`, `AgentSession.swift:455-463`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeTranscript {
    pub url: PathBuf,
    pub modified_at: Timestamp,
}

impl ClaudeTranscript {
    pub fn new(url: impl Into<PathBuf>, modified_at: Timestamp) -> Self {
        Self {
            url: url.into(),
            modified_at,
        }
    }
}

/// Maps working directory paths to Claude's escaped project directory names and scans transcripts
/// (`ClaudeSessionProjectMapper`, `AgentSession.swift:454-563`).
pub struct ClaudeSessionProjectMapper;

impl ClaudeSessionProjectMapper {
    /// Replaces every non-ASCII-alphanumeric scalar with `'-'` (`AgentSession.swift:465-473`).
    pub fn escaped_cwd(cwd: &str) -> String {
        cwd.chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect()
    }

    /// Alias kept for `host_observers` callers.
    pub fn escaped_directory_name(cwd: &str) -> String {
        Self::escaped_cwd(cwd)
    }

    pub fn project_directories(cwd: &str, home_directory: &Path) -> Vec<PathBuf> {
        let standard_root = home_directory.join(".claude").join("projects");
        let escaped = Self::escaped_cwd(cwd);
        std::iter::once(standard_root)
            .chain(ClaudeDesktopProjectsLocator::roots(home_directory))
            .map(|root| root.join(&escaped))
            .collect()
    }

    pub fn project_directories_with_budget(
        cwd: &str,
        home_directory: &Path,
        budget: &mut DirectoryMetadataScanBudget,
    ) -> Vec<PathBuf> {
        let standard_root = home_directory.join(".claude").join("projects");
        let escaped = Self::escaped_cwd(cwd);
        let mut roots = vec![standard_root];
        roots.extend(ClaudeDesktopProjectsLocator::roots_with_budget(
            home_directory,
            budget,
            false,
        ));
        roots.into_iter().map(|root| root.join(&escaped)).collect()
    }

    pub fn newest_transcript(cwd: &str, home_directory: &Path) -> Option<ClaudeTranscript> {
        Self::transcripts(cwd, home_directory, None, Timestamp::now())
            .into_iter()
            .next()
    }

    pub fn transcripts(
        cwd: &str,
        home_directory: &Path,
        limit: Option<usize>,
        now: Timestamp,
    ) -> Vec<ClaudeTranscript> {
        let mut budget = DirectoryMetadataScanBudget::new(4096, 1, 1.0, Timestamp::now());
        Self::transcripts_with_budget(
            cwd,
            home_directory,
            limit,
            now,
            &mut budget,
            |_url, modified_at, now_ts| modified_at.min(now_ts),
        )
    }

    pub fn transcripts_with_budget(
        cwd: &str,
        home_directory: &Path,
        limit: Option<usize>,
        now: Timestamp,
        budget: &mut DirectoryMetadataScanBudget,
        mut clamp_modification_date: impl FnMut(&Path, Timestamp, Timestamp) -> Timestamp,
    ) -> Vec<ClaudeTranscript> {
        let directories = Self::project_directories_with_budget(cwd, home_directory, budget);
        let candidate_files: Vec<PathBuf> = directories
            .into_iter()
            .flat_map(|dir| budget.files(&dir))
            .collect();
        let mut transcripts: Vec<ClaudeTranscript> = candidate_files
            .into_iter()
            .filter_map(|file| {
                if !budget.has_time_remaining() {
                    return None;
                }
                if file.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    return None;
                }
                let meta = fs::symlink_metadata(&file).ok()?;
                if meta.file_type().is_symlink() || !meta.is_file() {
                    return None;
                }
                let modified_at = system_time_to_timestamp(meta.modified().ok()?)?;
                let clamped = clamp_modification_date(&file, modified_at, now);
                Some(ClaudeTranscript::new(file, clamped))
            })
            .collect();
        transcripts.sort_by_key(|lhs| std::cmp::Reverse(lhs.modified_at));
        if let Some(limit) = limit {
            transcripts.truncate(limit);
        }
        transcripts
    }
}

/// Process record parsed from `ps` or OS process enumeration (`AgentProcessRecord`, `AgentSession.swift:222-264`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProcessRecord {
    pub pid: i32,
    pub ppid: i32,
    pub started_at: Option<Timestamp>,
    pub command: String,
    pub arguments: Option<Vec<String>>,
    pub pi_selector_environment: Option<BTreeMap<String, String>>,
}

impl AgentProcessRecord {
    pub fn new(
        pid: i32,
        ppid: i32,
        started_at: Option<Timestamp>,
        command: impl Into<String>,
    ) -> Self {
        Self {
            pid,
            ppid,
            started_at,
            command: command.into(),
            arguments: None,
            pi_selector_environment: None,
        }
    }

    pub fn with_arguments(mut self, arguments: Option<Vec<String>>) -> Self {
        self.arguments = arguments;
        self
    }

    pub fn with_pi_selector_environment(
        mut self,
        pi_selector_environment: Option<BTreeMap<String, String>>,
    ) -> Self {
        self.pi_selector_environment = pi_selector_environment;
        self
    }

    pub fn executable_basename(&self) -> String {
        let first_token = self
            .arguments
            .as_ref()
            .and_then(|args| args.first())
            .map(String::as_str)
            .or_else(|| self.command.split_whitespace().next())
            .unwrap_or("");
        let first_basename = AgentPSOutputParser::basename(first_token);
        let executable = self
            .arguments
            .as_ref()
            .and_then(|args| args.first())
            .map(String::as_str)
            .unwrap_or(&self.command);
        if !first_basename.eq_ignore_ascii_case("disclaimer")
            && executable.contains("Application Support/Claude/claude-code/claude")
        {
            return AgentSessionProvider::Claude.as_str().to_owned();
        }
        first_basename
    }
}

/// Parses `ps` output and classifies live agent processes (`AgentPSOutputParser`, `AgentSession.swift:266-431`).
pub struct AgentPSOutputParser;

impl AgentPSOutputParser {
    pub const CHATGPT_CODEX_EXECUTABLE_PATHS: &'static [&'static str] = &[
        "/Applications/ChatGPT.app/Contents/Resources/codex",
        "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
    ];

    pub(crate) fn basename(path: &str) -> String {
        Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn parse(output: &str) -> Vec<AgentProcessRecord> {
        output
            .lines()
            .filter_map(|raw_line| {
                let line = raw_line.trim();
                if line.is_empty() {
                    return None;
                }
                let (tokens, command_start) = Self::first_tokens_and_command_index(line, 7)?;
                let pid = tokens[0].parse::<i32>().ok()?;
                let ppid = tokens[1].parse::<i32>().ok()?;
                let started_prefix = tokens[2..7].join(" ");
                let started_at = Self::parse_ps_start_date(&started_prefix);
                let command = line[command_start..].trim().to_owned();
                if command.is_empty() {
                    return None;
                }
                Some(AgentProcessRecord::new(pid, ppid, started_at, command))
            })
            .collect()
    }

    pub fn agent_processes(records: &[AgentProcessRecord]) -> Vec<AgentProcessRecord> {
        let candidates: Vec<AgentProcessRecord> = records
            .iter()
            .filter(|record| {
                let basename = record.executable_basename().to_ascii_lowercase();
                if Self::pi_dialect(record).is_some() {
                    return !Self::is_obvious_pi_family_helper(&record.command);
                }
                let executable = record
                    .arguments
                    .as_ref()
                    .and_then(|a| a.first())
                    .map(String::as_str)
                    .unwrap_or(&record.command);
                if basename == AgentSessionProvider::Codex.as_str() {
                    let arguments = Self::arguments(record);
                    return Self::is_codex_agent_executable(executable)
                        && !arguments
                            .iter()
                            .any(|a| a == "app-server" || a == "--help" || a == "--version");
                }
                if basename == AgentSessionProvider::Claude.as_str() {
                    return Self::is_claude_agent_executable(executable)
                        && !Self::is_obvious_claude_helper(&record.command);
                }
                basename == "disclaimer"
                    && record
                        .command
                        .contains(AgentSessionProvider::Claude.as_str())
            })
            .cloned()
            .collect();

        let candidate_pids: HashSet<i32> = candidates.iter().map(|r| r.pid).collect();
        candidates
            .iter()
            .filter(|record| {
                if !record
                    .executable_basename()
                    .eq_ignore_ascii_case("disclaimer")
                {
                    return true;
                }
                !candidates.iter().any(|child| {
                    child.ppid == record.pid
                        && child
                            .executable_basename()
                            .eq_ignore_ascii_case(AgentSessionProvider::Claude.as_str())
                        && Self::normalized_claude_arguments(&child.command)
                            == Self::normalized_claude_arguments(&record.command)
                }) && !candidate_pids.contains(&record.ppid)
            })
            .cloned()
            .collect()
    }

    pub fn provider(record: &AgentProcessRecord) -> Option<AgentSessionProvider> {
        let basename = record.executable_basename().to_ascii_lowercase();
        if basename == "codex" {
            return Some(AgentSessionProvider::Codex);
        }
        if basename == "claude" || basename == "disclaimer" {
            return Some(AgentSessionProvider::Claude);
        }
        if Self::pi_dialect(record).is_some() && !Self::is_obvious_pi_family_helper(&record.command)
        {
            return Some(AgentSessionProvider::Pi);
        }
        None
    }

    pub fn pi_dialect(record: &AgentProcessRecord) -> Option<AgentSessionDialect> {
        Self::pi_dialect_from(&record.executable_basename(), &Self::arguments(record))
    }

    fn pi_dialect_from(
        executable_basename: &str,
        arguments: &[String],
    ) -> Option<AgentSessionDialect> {
        let first_basename = Self::basename(executable_basename).to_ascii_lowercase();
        if first_basename == "pi" {
            return Some(AgentSessionDialect::Pi);
        }
        if first_basename == "omp" {
            return Some(AgentSessionDialect::Omp);
        }
        if first_basename != "bun" {
            return None;
        }
        arguments
            .iter()
            .any(|arg| Self::basename(arg).eq_ignore_ascii_case("omp"))
            .then_some(AgentSessionDialect::Omp)
    }

    pub fn source(record: &AgentProcessRecord) -> AgentSessionSource {
        if Self::provider(record) != Some(AgentSessionProvider::Claude) {
            return AgentSessionSource::Cli;
        }
        let command = if record
            .executable_basename()
            .eq_ignore_ascii_case("disclaimer")
        {
            record.command.as_str()
        } else {
            record
                .arguments
                .as_ref()
                .and_then(|a| a.first())
                .map(String::as_str)
                .unwrap_or(&record.command)
        };
        if command.contains("Application Support/Claude/claude-code") {
            AgentSessionSource::DesktopApp
        } else {
            AgentSessionSource::Cli
        }
    }

    pub fn has_codex_app_server(records: &[AgentProcessRecord]) -> bool {
        records.iter().any(|record| {
            record
                .executable_basename()
                .eq_ignore_ascii_case(AgentSessionProvider::Codex.as_str())
                && Self::is_codex_agent_executable(
                    record
                        .arguments
                        .as_ref()
                        .and_then(|a| a.first())
                        .map(String::as_str)
                        .unwrap_or(&record.command),
                )
                && Self::arguments(record).iter().any(|a| a == "app-server")
        })
    }

    pub fn has_trusted_chatgpt_codex_app_server<F>(
        records: &[AgentProcessRecord],
        mut validator: F,
    ) -> bool
    where
        F: FnMut(&AgentProcessRecord) -> bool,
    {
        records.iter().any(|record| {
            let executable = record
                .arguments
                .as_ref()
                .and_then(|a| a.first())
                .map(String::as_str)
                .or_else(|| record.command.split_whitespace().next())
                .unwrap_or("");
            Self::CHATGPT_CODEX_EXECUTABLE_PATHS.contains(&executable)
                && Self::arguments(record).iter().any(|a| a == "app-server")
                && validator(record)
        })
    }

    fn arguments(record: &AgentProcessRecord) -> Vec<String> {
        if let Some(args) = &record.arguments {
            args.iter().skip(1).cloned().collect()
        } else {
            record
                .command
                .split_whitespace()
                .skip(1)
                .map(ToOwned::to_owned)
                .collect()
        }
    }

    fn normalized_claude_arguments(command: &str) -> Vec<String> {
        let arguments: Vec<String> = command
            .split_whitespace()
            .skip(1)
            .map(ToOwned::to_owned)
            .collect();
        if let Some(idx) = arguments.iter().position(|arg| {
            Self::basename(arg).eq_ignore_ascii_case(AgentSessionProvider::Claude.as_str())
        }) {
            arguments.into_iter().skip(idx + 1).collect()
        } else {
            arguments
        }
    }

    fn is_obvious_claude_helper(command: &str) -> bool {
        let lowercased = command.to_ascii_lowercase();
        lowercased.contains("--version")
            || lowercased.contains("--help")
            || lowercased.contains("claude-code-acp")
    }

    fn is_codex_agent_executable(command: &str) -> bool {
        let lowercased = command.to_ascii_lowercase();
        if !lowercased.contains(".app/") {
            return true;
        }
        let executable = lowercased.split_whitespace().next().unwrap_or("");
        executable == "/applications/codex.app/contents/resources/codex"
            || executable == "/applications/chatgpt.app/contents/resources/codex"
    }

    fn is_claude_agent_executable(command: &str) -> bool {
        let lowercased = command.to_ascii_lowercase();
        !lowercased.contains(".app/")
            || lowercased.contains("application support/claude/claude-code/claude")
    }

    fn is_obvious_pi_family_helper(command: &str) -> bool {
        let lowercased = command.to_ascii_lowercase();
        lowercased.contains("--help")
            || lowercased.contains("--version")
            || lowercased.contains("--smoke-test")
            || lowercased.contains("__omp_worker_")
    }

    fn first_tokens_and_command_index(
        line: &str,
        token_count: usize,
    ) -> Option<(Vec<&str>, usize)> {
        let mut tokens = Vec::with_capacity(token_count);
        let bytes = line.as_bytes();
        let mut idx = 0;
        let len = bytes.len();

        while tokens.len() < token_count {
            while idx < len && bytes[idx].is_ascii_whitespace() {
                idx += 1;
            }
            if idx >= len {
                return None;
            }
            let start = idx;
            while idx < len && !bytes[idx].is_ascii_whitespace() {
                idx += 1;
            }
            tokens.push(&line[start..idx]);
        }

        while idx < len && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= len {
            return None;
        }
        Some((tokens, idx))
    }

    fn parse_ps_start_date(value: &str) -> Option<Timestamp> {
        let parts: Vec<&str> = value.split_whitespace().collect();
        if parts.len() != 5 {
            return None;
        }
        let month = match parts[1] {
            "Jan" => 1,
            "Feb" => 2,
            "Mar" => 3,
            "Apr" => 4,
            "May" => 5,
            "Jun" => 6,
            "Jul" => 7,
            "Aug" => 8,
            "Sep" => 9,
            "Oct" => 10,
            "Nov" => 11,
            "Dec" => 12,
            _ => return None,
        };
        let day = parts[2].parse::<i8>().ok()?;
        let mut time_parts = parts[3].split(':');
        let hour = time_parts.next()?.parse::<i8>().ok()?;
        let minute = time_parts.next()?.parse::<i8>().ok()?;
        let second = time_parts.next()?.parse::<i8>().ok()?;
        if time_parts.next().is_some() {
            return None;
        }
        let year = parts[4].parse::<i16>().ok()?;
        let date = Date::new(year, month, day).ok()?;
        let time = Time::new(hour, minute, second, 0).ok()?;
        let dt = DateTime::from_parts(date, time);
        let tz = TimeZone::try_system().unwrap_or(TimeZone::UTC);
        dt.to_zoned(tz).ok().map(|zoned| zoned.timestamp())
    }
}

/// Validates ChatGPT.app bundled Codex `app-server` process trust (`ChatGPTCodexProcessTrust`, `LocalAgentSessionScanner.swift:26-62`).
pub struct ChatGPTCodexProcessTrust;

impl ChatGPTCodexProcessTrust {
    pub const OUTER_BUNDLE_PATH: &'static str = "/Applications/ChatGPT.app";
    pub const REQUIREMENT: &'static str =
        "anchor apple generic and certificate leaf[subject.OU] = \"2DC432GLL2\"";

    pub fn is_trusted(pid: i32) -> bool {
        Self::is_trusted_with(
            pid,
            Self::default_executable_path,
            Self::default_resolve_path,
            |_| false,
            |_| false,
        )
    }

    pub async fn is_trusted_async(
        pid: i32,
        local_probe: &LocalProbe,
        process_registry: &ProcessRegistry,
        launch_gate: &LaunchGate,
        cancel: &CancellationToken,
    ) -> bool {
        if pid <= 0 {
            return false;
        }
        let Some(identity) = local_probe.owned_process_identity(pid) else {
            return false;
        };
        let Some(actual_path) = local_probe
            .executable_path(identity)
            .map(|p| p.to_string_lossy().into_owned())
        else {
            return false;
        };
        if !AgentPSOutputParser::CHATGPT_CODEX_EXECUTABLE_PATHS.contains(&actual_path.as_str()) {
            return false;
        }
        let resolved = Self::default_resolve_path(actual_path);
        if !AgentPSOutputParser::CHATGPT_CODEX_EXECUTABLE_PATHS.contains(&resolved.as_str()) {
            return false;
        }
        #[cfg(target_os = "macos")]
        {
            let pid_str = pid.to_string();
            let proc_ok = run_shared_broker_command(
                "/usr/bin/codesign",
                &["--verify", "-R", Self::REQUIREMENT, &pid_str],
                ProcessEnv::empty(),
                Duration::from_secs(5),
                16 * 1024,
                process_registry,
                launch_gate,
                cancel,
            )
            .await
            .is_some_and(|out| out.status.success());
            if !proc_ok {
                return false;
            }
            run_shared_broker_command(
                "/usr/bin/codesign",
                &["--verify", "-R", Self::REQUIREMENT, Self::OUTER_BUNDLE_PATH],
                ProcessEnv::empty(),
                Duration::from_secs(5),
                16 * 1024,
                process_registry,
                launch_gate,
                cancel,
            )
            .await
            .is_some_and(|out| out.status.success())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (process_registry, launch_gate, cancel);
            false
        }
    }

    pub fn is_trusted_with<E, R, P, A>(
        pid: i32,
        mut executable_path: E,
        mut resolve_path: R,
        mut process_is_trusted: P,
        mut app_is_trusted: A,
    ) -> bool
    where
        E: FnMut(i32) -> Option<String>,
        R: FnMut(String) -> String,
        P: FnMut(i32) -> bool,
        A: FnMut(String) -> bool,
    {
        if pid <= 0 {
            return false;
        }
        let Some(actual_path) = executable_path(pid) else {
            return false;
        };
        if !AgentPSOutputParser::CHATGPT_CODEX_EXECUTABLE_PATHS.contains(&actual_path.as_str()) {
            return false;
        }
        let resolved = resolve_path(actual_path.clone());
        if resolved != actual_path {
            return false;
        }
        process_is_trusted(pid) && app_is_trusted(Self::OUTER_BUNDLE_PATH.to_owned())
    }

    fn default_executable_path(pid: i32) -> Option<String> {
        let probe = LocalProbe::new();
        let identity = probe.owned_process_identity(pid)?;
        probe
            .executable_path(identity)
            .map(|path| path.to_string_lossy().into_owned())
    }

    fn default_resolve_path(path: String) -> String {
        fs::canonicalize(&path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(path)
    }
}

/// Parses `lsof -a -d cwd -Fn -p ...` output (`LSOFCWDOutputParser`, `AgentSession.swift:433-452`).
pub struct LSOFCWDOutputParser;

impl LSOFCWDOutputParser {
    pub fn parse(output: &str) -> HashMap<i32, String> {
        let mut current_pid: Option<i32> = None;
        let mut cwd_by_pid = HashMap::new();
        for line in output.lines() {
            if let Some(pid_str) = line.strip_prefix('p') {
                current_pid = pid_str.trim().parse::<i32>().ok();
            } else if let Some(path) = line.strip_prefix('n')
                && let Some(pid) = current_pid
                && !path.is_empty()
            {
                cwd_by_pid.insert(pid, path.to_owned());
            }
        }
        cwd_by_pid
    }
}

/// Correlation rules for live processes, Claude transcripts, and Codex rollouts
/// (`AgentSessionCorrelation`, `AgentSession.swift:565-629`).
pub struct AgentSessionCorrelation;

impl AgentSessionCorrelation {
    pub fn newest_processes_first(processes: &[AgentProcessRecord]) -> Vec<AgentProcessRecord> {
        let mut sorted = processes.to_vec();
        sorted.sort_by(|lhs, rhs| match (lhs.started_at, rhs.started_at) {
            (Some(l), Some(r)) if l != r => r.cmp(&l),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            _ => rhs.pid.cmp(&lhs.pid),
        });
        sorted
    }

    pub fn assign_claude_transcripts(
        processes: &[AgentProcessRecord],
        cwd_by_pid: &HashMap<i32, String>,
        transcripts_by_cwd: &HashMap<String, Vec<ClaudeTranscript>>,
    ) -> HashMap<i32, ClaudeTranscript> {
        let mut assigned = HashMap::new();
        let mut used_paths = HashSet::new();
        let unambiguous_pids = Self::unambiguous_process_ids(processes, cwd_by_pid);
        for process in Self::newest_processes_first(processes) {
            if !unambiguous_pids.contains(&process.pid) {
                continue;
            }
            let Some(cwd) = cwd_by_pid.get(&process.pid) else {
                continue;
            };
            let Some(candidates) = transcripts_by_cwd.get(cwd) else {
                continue;
            };
            let candidate = candidates.iter().find(|transcript| {
                if used_paths.contains(&transcript.url) {
                    return false;
                }
                let Some(started_at) = process.started_at else {
                    return true;
                };
                transcript.modified_at >= started_at
            });
            if let Some(candidate) = candidate {
                used_paths.insert(candidate.url.clone());
                assigned.insert(process.pid, candidate.clone());
            }
        }
        assigned
    }

    pub fn unambiguous_process_ids(
        processes: &[AgentProcessRecord],
        cwd_by_pid: &HashMap<i32, String>,
    ) -> HashSet<i32> {
        let mut grouped: HashMap<String, Vec<i32>> = HashMap::new();
        for process in processes {
            let Some(cwd) = cwd_by_pid.get(&process.pid) else {
                continue;
            };
            let standardized = standardize_path_str(cwd);
            grouped.entry(standardized).or_default().push(process.pid);
        }
        grouped
            .into_values()
            .filter_map(|pids| (pids.len() == 1).then_some(pids[0]))
            .collect()
    }

    pub fn codex_working_directories_match(lhs: Option<&str>, rhs: Option<&str>) -> bool {
        let (Some(lhs), Some(rhs)) = (lhs, rhs) else {
            return false;
        };
        standardize_path_str(lhs) == standardize_path_str(rhs)
    }

    pub fn file_only_codex_source(
        metadata_source: AgentSessionSource,
        app_server_present: bool,
    ) -> AgentSessionSource {
        if metadata_source == AgentSessionSource::Unknown && app_server_present {
            AgentSessionSource::DesktopApp
        } else {
            metadata_source
        }
    }
}

/// Thread metadata resolved from `session_index.jsonl` and `state_*.sqlite`
/// (`CodexThreadMetadata`, `CodexThreadMetadataReader.swift:8-16`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexThreadMetadata {
    pub title: Option<String>,
    pub agent_path: Option<String>,
}

impl CodexThreadMetadata {
    pub fn new(title: Option<String>, agent_path: Option<String>) -> Self {
        Self { title, agent_path }
    }
}

/// Reads Codex thread titles and agent paths from `session_index.jsonl` and copied `state_*.sqlite`
/// (`CodexThreadMetadataReader`, `CodexThreadMetadataReader.swift:18-235`).
pub struct CodexThreadMetadataReader {
    pub database_url: PathBuf,
    copy_parent: Option<PathBuf>,
}

impl CodexThreadMetadataReader {
    pub fn new(
        codex_home_directory: &Path,
        environment: &HashMap<String, String>,
        resolved_working_directory: Option<&Path>,
    ) -> Self {
        let sqlite_home = Self::sqlite_home_directory(
            codex_home_directory,
            environment,
            resolved_working_directory,
        );
        let database_url = Self::database_url(&sqlite_home);
        let home_dir = environment
            .get("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && p != Path::new("/"));
        let env_btree: BTreeMap<String, String> = environment
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let copy_parent = if let Some(ref home) = home_dir {
            let cache_root = crate::cost::store::resolve_cache_root(&env_btree, Some(home));
            (!cache_root.as_os_str().is_empty())
                .then(|| crate::cost::store::default_sqlite_copy_parent(Some(&cache_root)))
        } else {
            environment
                .get("XDG_CACHE_HOME")
                .map(|s| Path::new(s.trim()))
                .filter(|p| p.is_absolute())
                .map(|xdg| {
                    crate::cost::store::default_sqlite_copy_parent(Some(&xdg.join("ariadusage")))
                })
        };
        Self {
            database_url,
            copy_parent,
        }
    }

    fn database_url(sqlite_home_directory: &Path) -> PathBuf {
        let mut best: Option<(usize, PathBuf)> = None;
        if let Ok(entries) = fs::read_dir(sqlite_home_directory) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("sqlite") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let Some(ver_str) = stem.strip_prefix("state_") else {
                    continue;
                };
                let ver = ver_str.parse::<usize>().unwrap_or(0);
                if best.as_ref().is_none_or(|(best_ver, _)| ver > *best_ver) {
                    best = Some((ver, path));
                }
            }
        }
        best.map(|(_, path)| path)
            .unwrap_or_else(|| sqlite_home_directory.join("state_5.sqlite"))
    }

    fn sqlite_home_directory(
        codex_home_directory: &Path,
        environment: &HashMap<String, String>,
        resolved_working_directory: Option<&Path>,
    ) -> PathBuf {
        if let Some(configured) = Self::configured_sqlite_home(codex_home_directory) {
            return configured;
        }
        if let Some(raw_path) = environment
            .get("CODEX_SQLITE_HOME")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            let path = Path::new(raw_path);
            if path.is_absolute() {
                return path.to_path_buf();
            }
            if let Some(cwd) = resolved_working_directory {
                return cwd.join(raw_path);
            }
        }
        codex_home_directory.to_path_buf()
    }

    fn configured_sqlite_home(codex_home_directory: &Path) -> Option<PathBuf> {
        let config_url = codex_home_directory.join("config.toml");
        let meta = fs::symlink_metadata(&config_url).ok()?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return None;
        }
        let file = File::open(&config_url).ok()?;
        let mut buf = Vec::new();
        file.take(256 * 1024).read_to_end(&mut buf).ok()?;
        let contents = String::from_utf8(buf).ok()?;
        let raw_path = Self::top_level_sqlite_home(&contents)?;
        let path = Path::new(&raw_path);
        path.is_absolute().then(|| path.to_path_buf())
    }

    fn top_level_sqlite_home(contents: &str) -> Option<String> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum MultilineQuote {
            Basic,
            Literal,
        }
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum ScanState {
            Normal,
            Basic,
            Literal,
            MultilineBasic,
            MultilineLiteral,
        }

        fn has_triple_quote(quote: char, idx: usize, chars: &[char]) -> bool {
            idx + 2 < chars.len()
                && chars[idx] == quote
                && chars[idx + 1] == quote
                && chars[idx + 2] == quote
        }

        fn multiline_quote_after_scanning(
            line: &str,
            starting_with: Option<MultilineQuote>,
        ) -> Option<MultilineQuote> {
            let chars: Vec<char> = line.chars().collect();
            let mut state = match starting_with {
                Some(MultilineQuote::Basic) => ScanState::MultilineBasic,
                Some(MultilineQuote::Literal) => ScanState::MultilineLiteral,
                None => ScanState::Normal,
            };
            let mut idx = 0;
            while idx < chars.len() {
                let ch = chars[idx];
                match state {
                    ScanState::Normal => {
                        if ch == '#' {
                            idx = chars.len();
                        } else if ch == '"' {
                            if has_triple_quote('"', idx, &chars) {
                                state = ScanState::MultilineBasic;
                                idx += 3;
                            } else {
                                state = ScanState::Basic;
                                idx += 1;
                            }
                        } else if ch == '\'' {
                            if has_triple_quote('\'', idx, &chars) {
                                state = ScanState::MultilineLiteral;
                                idx += 3;
                            } else {
                                state = ScanState::Literal;
                                idx += 1;
                            }
                        } else {
                            idx += 1;
                        }
                    }
                    ScanState::Basic => {
                        if ch == '\\' {
                            idx = (idx + 2).min(chars.len());
                        } else {
                            if ch == '"' {
                                state = ScanState::Normal;
                            }
                            idx += 1;
                        }
                    }
                    ScanState::Literal => {
                        if ch == '\'' {
                            state = ScanState::Normal;
                        }
                        idx += 1;
                    }
                    ScanState::MultilineBasic => {
                        if ch == '\\' {
                            idx = (idx + 2).min(chars.len());
                        } else if has_triple_quote('"', idx, &chars) {
                            state = ScanState::Normal;
                            idx += 3;
                        } else {
                            idx += 1;
                        }
                    }
                    ScanState::MultilineLiteral => {
                        if has_triple_quote('\'', idx, &chars) {
                            state = ScanState::Normal;
                            idx += 3;
                        } else {
                            idx += 1;
                        }
                    }
                }
            }
            match state {
                ScanState::MultilineBasic => Some(MultilineQuote::Basic),
                ScanState::MultilineLiteral => Some(MultilineQuote::Literal),
                ScanState::Normal | ScanState::Basic | ScanState::Literal => None,
            }
        }

        fn toml_string(raw_value: &str) -> Option<String> {
            let value = raw_value.trim();
            let quote = value.chars().next()?;
            if quote == '\'' {
                let rest = &value[1..];
                let end = rest.find('\'')?;
                return Some(rest[..end].to_owned());
            }
            if quote != '"' {
                return None;
            }
            let mut escaped = false;
            let mut end_byte = None;
            for (idx, ch) in value.char_indices().skip(1) {
                if ch == '"' && !escaped {
                    end_byte = Some(idx);
                    break;
                }
                if ch == '\\' {
                    escaped = !escaped;
                } else {
                    escaped = false;
                }
            }
            let end_byte = end_byte?;
            let token = &value[..=end_byte];
            serde_json::from_str::<String>(token).ok()
        }

        let lines: Vec<&str> = contents.lines().collect();
        let mut multiline_quote: Option<MultilineQuote> = None;
        for raw_line in lines {
            if multiline_quote.is_some() {
                multiline_quote = multiline_quote_after_scanning(raw_line, multiline_quote);
                continue;
            }
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                return None;
            }
            let Some((raw_key, raw_val)) = line.split_once('=') else {
                multiline_quote = multiline_quote_after_scanning(raw_line, None);
                continue;
            };
            let key = raw_key.trim();
            let parsed_key = if key == "sqlite_home" {
                Some("sqlite_home".to_owned())
            } else {
                toml_string(key)
            };
            if parsed_key.as_deref() != Some("sqlite_home") {
                multiline_quote = multiline_quote_after_scanning(raw_line, None);
                continue;
            }
            return toml_string(raw_val);
        }
        None
    }

    pub fn indexed_thread_names(
        codex_home_directory: &Path,
        session_ids: &HashSet<String>,
    ) -> HashMap<String, String> {
        if session_ids.is_empty() {
            return HashMap::new();
        }
        let session_index_url = codex_home_directory.join("session_index.jsonl");
        let Ok(meta) = fs::symlink_metadata(&session_index_url) else {
            return HashMap::new();
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            return HashMap::new();
        }
        let Ok(file) = File::open(&session_index_url) else {
            return HashMap::new();
        };
        let reader = BufReader::new(file.take(64 * 1024));
        let mut names = HashMap::new();
        for line in reader.lines().map_while(Result::ok) {
            if line.len() > 64 * 1024 {
                continue;
            }
            let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let Some(id) = val.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            if !session_ids.contains(id) {
                continue;
            }
            let Some(thread_name) = val
                .get("thread_name")
                .or_else(|| val.get("threadName"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            names.insert(id.to_owned(), thread_name.to_owned());
        }
        names
    }

    pub fn metadata(
        &self,
        session_ids: &HashSet<String>,
        indexed_names: &HashMap<String, String>,
    ) -> HashMap<String, CodexThreadMetadata> {
        if session_ids.is_empty() {
            return HashMap::new();
        }
        let mut result: HashMap<String, CodexThreadMetadata> = indexed_names
            .iter()
            .filter(|(k, _)| session_ids.contains(*k))
            .map(|(k, v)| (k.clone(), CodexThreadMetadata::new(Some(v.clone()), None)))
            .collect();

        let Some(ref copy_parent) = self.copy_parent else {
            return result;
        };
        let Some(temp_parent) = ensure_private_0700_dir(copy_parent) else {
            return result;
        };
        let Ok(temp_dir) = create_in(&temp_parent) else {
            return result;
        };
        let Ok(copied_path) = copy_database(&self.database_url, temp_dir.path()) else {
            return result;
        };
        #[cfg(unix)]
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        #[cfg(not(unix))]
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let Ok(conn) = Connection::open_with_flags(&copied_path, flags) else {
            return result;
        };
        let _ = conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true);
        let _ = conn.pragma_update(None, "trusted_schema", false);
        let _ = conn.busy_timeout(Duration::from_millis(100));

        let queries = [
            "SELECT substr(title, 1, 256), agent_path FROM threads WHERE id = ?1 LIMIT 1",
            "SELECT substr(title, 1, 256), NULL FROM threads WHERE id = ?1 LIMIT 1",
        ];
        let mut stmt = None;
        for q in queries {
            if let Ok(prepared) = conn.prepare(q) {
                stmt = Some(prepared);
                break;
            }
        }
        let Some(mut stmt) = stmt else {
            return result;
        };

        for session_id in session_ids {
            let Ok(row) = stmt.query_row([session_id.as_str()], |row| {
                let db_title: Option<String> = row.get(0).ok().flatten();
                let db_agent_path: Option<String> = row.get(1).ok().flatten();
                Ok((db_title, db_agent_path))
            }) else {
                continue;
            };
            let title = indexed_names
                .get(session_id)
                .cloned()
                .or_else(|| row.0.filter(|s| !s.is_empty()));
            let agent_path = row.1.filter(|s| !s.is_empty());
            if title.is_some() || agent_path.is_some() {
                result.insert(
                    session_id.clone(),
                    CodexThreadMetadata::new(title, agent_path),
                );
            }
        }
        result
    }
}

/// First-line rollout metadata (`CodexRolloutMetadata`, `AgentSession.swift:631-683`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexRolloutMetadata {
    pub session_id: String,
    pub cwd: Option<String>,
    pub originator: Option<String>,
    pub source: Option<String>,
    pub agent_path: Option<String>,
    pub is_guardian: bool,
}

impl CodexRolloutMetadata {
    pub fn new(
        session_id: impl Into<String>,
        cwd: Option<String>,
        originator: Option<String>,
        source: Option<String>,
        agent_path: Option<String>,
        is_guardian: bool,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            cwd,
            originator,
            source,
            agent_path,
            is_guardian,
        }
    }

    pub fn session_source(&self) -> AgentSessionSource {
        let value = [self.originator.as_deref(), self.source.as_deref()]
            .into_iter()
            .flatten()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>()
            .join(" ");
        if value.contains("desktop") || value.contains("app-server") {
            return AgentSessionSource::DesktopApp;
        }
        if value.contains("ide")
            || value.contains("vscode")
            || value.contains("cursor")
            || value.contains("zed")
        {
            return AgentSessionSource::Ide;
        }
        if value.contains("codex_exec") || value.contains("exec") || value.contains("cli") {
            return AgentSessionSource::Cli;
        }
        AgentSessionSource::Unknown
    }

    pub fn descriptive_name(
        &self,
        thread_metadata: Option<&CodexThreadMetadata>,
    ) -> Option<String> {
        let task_name: Option<String> = if self.is_guardian {
            Some("Approval review".to_owned())
        } else if let Some(agent_path) = thread_metadata
            .and_then(|m| m.agent_path.as_deref())
            .or(self.agent_path.as_deref())
        {
            AgentSessionNameFormatter::agent_path(agent_path)
        } else {
            None
        };
        AgentSessionNameFormatter::session_name(
            thread_metadata.and_then(|m| m.title.as_deref()),
            task_name.as_deref(),
            64,
        )
    }
}

struct AgentSessionNameFormatter;

impl AgentSessionNameFormatter {
    const UPPERCASE_WORDS: &[&str] = &[
        "ai", "api", "ci", "cli", "db", "pr", "qa", "seo", "sql", "ui", "ux",
    ];

    fn agent_path(path: &str) -> Option<String> {
        let component = path.split('/').rfind(|s| !s.is_empty())?;
        let mut normalized = String::new();
        let mut previous: Option<char> = None;
        for ch in component.chars() {
            if ch == '_' || ch == '-' {
                if !normalized.ends_with(' ') {
                    normalized.push(' ');
                }
                previous = None;
                continue;
            }
            if let Some(prev) = previous
                && ((ch.is_numeric() && !prev.is_numeric())
                    || (ch.is_alphabetic() && prev.is_numeric())
                    || (ch.is_uppercase() && prev.is_lowercase()))
            {
                normalized.push(' ');
            }
            normalized.push(ch);
            previous = Some(ch);
        }

        let words: Vec<&str> = normalized.split_whitespace().collect();
        if words.is_empty() {
            return None;
        }
        Some(
            words
                .into_iter()
                .enumerate()
                .map(|(index, word)| {
                    let lower = word.to_lowercase();
                    if Self::UPPERCASE_WORDS.contains(&lower.as_str()) {
                        return lower.to_uppercase();
                    }
                    if index == 0 {
                        let mut chars = word.chars();
                        match chars.next() {
                            Some(first) => {
                                format!("{}{}", first.to_uppercase(), chars.as_str())
                            }
                            None => String::new(),
                        }
                    } else {
                        lower
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    fn title(title: &str, maximum_length: usize) -> Option<String> {
        let lines: Vec<&str> = title
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let line = lines
            .iter()
            .copied()
            .find(|l| !l.starts_with('/'))
            .or_else(|| lines.first().copied())?;
        if let Some(rest) = line.strip_prefix('/')
            && let Some(command) = Self::agent_path(rest)
        {
            return Some(command);
        }
        let compact = line.split_whitespace().collect::<Vec<_>>().join(" ");
        Some(Self::truncated(&compact, maximum_length))
    }

    fn session_name(
        title: Option<&str>,
        task_name: Option<&str>,
        maximum_length: usize,
    ) -> Option<String> {
        let chat_name = title.and_then(|t| Self::title(t, usize::MAX));
        let Some(chat_name) = chat_name else {
            return task_name.map(|t| Self::truncated(t, maximum_length));
        };
        let Some(task_name) = task_name else {
            return Some(Self::truncated(&chat_name, maximum_length));
        };
        if chat_name.eq_ignore_ascii_case(task_name) {
            return Some(Self::truncated(&chat_name, maximum_length));
        }

        let separator = " · ";
        let sep_count = separator.chars().count();
        let combined = format!("{chat_name}{separator}{task_name}");
        if combined.chars().count() <= maximum_length {
            return Some(combined);
        }

        let task_char_count = task_name.chars().count();
        let chat_char_count = chat_name.chars().count();
        let initial_task_len = task_char_count.min(maximum_length / 3);
        let mut compact_task = Self::truncated(task_name, initial_task_len);
        let compact_task_len = compact_task.chars().count();
        let chat_len = maximum_length
            .saturating_sub(sep_count + compact_task_len)
            .max(1);
        let compact_chat = Self::truncated(&chat_name, chat_len);
        let compact_chat_len = compact_chat.chars().count();
        if compact_chat_len == chat_char_count {
            let task_len = maximum_length
                .saturating_sub(sep_count + compact_chat_len)
                .max(1);
            compact_task = Self::truncated(task_name, task_len);
        }
        Some(format!("{compact_chat}{separator}{compact_task}"))
    }

    fn truncated(value: &str, maximum_length: usize) -> String {
        let char_count = value.chars().count();
        if char_count <= maximum_length {
            return value.to_owned();
        }
        if maximum_length <= 1 {
            return value.chars().take(maximum_length).collect();
        }
        let prefix: String = value.chars().take(maximum_length - 1).collect();
        format!("{}…", prefix.trim())
    }
}

/// Parses the first line of a Codex rollout JSONL file (`CodexRolloutFirstLineParser`, `AgentSession.swift:771-833`).
pub struct CodexRolloutFirstLineParser;

impl CodexRolloutFirstLineParser {
    pub const MAX_FIRST_LINE_BYTES: usize = 256 * 1024;

    pub fn parse(line: &str) -> Option<CodexRolloutMetadata> {
        let root: serde_json::Value = serde_json::from_str(line).ok()?;
        let obj = root.as_object()?;
        if obj.get("type")?.as_str()? != "session_meta" {
            return None;
        }
        let payload = obj.get("payload")?.as_object()?;
        let session_id = payload
            .get("session_id")
            .and_then(|v| v.as_str())
            .or_else(|| payload.get("id").and_then(|v| v.as_str()))?
            .to_owned();
        let cwd = payload
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let originator = payload
            .get("originator")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let source = payload
            .get("source")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let source_obj = payload.get("source").and_then(|v| v.as_object());
        let subagent = source_obj
            .and_then(|o| o.get("subagent"))
            .and_then(|v| v.as_object());
        let thread_spawn = subagent
            .and_then(|o| o.get("thread_spawn"))
            .and_then(|v| v.as_object());
        let agent_path = payload
            .get("agent_path")
            .and_then(|v| v.as_str())
            .or_else(|| {
                thread_spawn
                    .and_then(|o| o.get("agent_path"))
                    .and_then(|v| v.as_str())
            })
            .map(ToOwned::to_owned);
        let is_guardian = subagent
            .and_then(|o| o.get("other"))
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("guardian"));
        Some(CodexRolloutMetadata {
            session_id,
            cwd,
            originator,
            source,
            agent_path,
            is_guardian,
        })
    }

    pub fn read(path: &Path) -> Option<CodexRolloutMetadata> {
        let meta = fs::symlink_metadata(path).ok()?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return None;
        }
        let mut file = File::open(path).ok()?;
        let mut data = Vec::new();
        let mut chunk = [0u8; 4096];
        while data.len() < Self::MAX_FIRST_LINE_BYTES {
            let max_read = (Self::MAX_FIRST_LINE_BYTES - data.len()).min(chunk.len());
            let n = file.read(&mut chunk[..max_read]).ok()?;
            if n == 0 {
                break;
            }
            if let Some(newline_idx) = chunk[..n].iter().position(|&b| b == b'\n') {
                data.extend_from_slice(&chunk[..newline_idx]);
                break;
            }
            data.extend_from_slice(&chunk[..n]);
        }
        let line = String::from_utf8(data).ok()?;
        Self::parse(line.trim())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn make_session(
        metadata: &CodexRolloutMetadata,
        transcript_url: &Path,
        modified_at: Timestamp,
        pid: Option<i32>,
        started_at: Option<Timestamp>,
        host: &str,
        config: &SessionScanConfig,
        now: Timestamp,
    ) -> Option<AgentSession> {
        let age_secs = (now.as_millisecond() - modified_at.as_millisecond()) as f64 / 1000.0;
        if pid.is_none() && age_secs > config.file_only_window {
            return None;
        }
        let cwd = metadata.cwd.clone();
        let project_name = cwd.as_deref().and_then(project_name_from_cwd);
        Some(AgentSession {
            id: metadata.session_id.clone(),
            provider: AgentSessionProvider::Codex,
            dialect: None,
            source: metadata.session_source(),
            state: config.state(Some(modified_at), now, pid.is_some()),
            pid,
            cwd,
            project_name,
            session_name: None,
            started_at,
            last_activity_at: Some(modified_at),
            transcript_path: Some(transcript_url.to_string_lossy().into_owned()),
            host: host.to_owned(),
        })
    }
}

#[derive(Debug, Clone)]
struct Rollout {
    url: PathBuf,
    modified_at: Timestamp,
    metadata: CodexRolloutMetadata,
}

struct ScanContext {
    home_directory: PathBuf,
    host: String,
    now: Timestamp,
    codex_app_server_present: bool,
    include_unmatched_codex_rollouts: bool,
    thread_metadata: HashMap<String, CodexThreadMetadata>,
    pi_family_sessions: Vec<AgentSession>,
}

pub type ProcessOutputProvider = Arc<dyn Fn(&HashMap<String, String>) -> String + Send + Sync>;
pub type CwdProvider =
    Arc<dyn Fn(&[i32], &HashMap<String, String>) -> HashMap<i32, String> + Send + Sync>;
pub type AppServerTrustValidator = Arc<dyn Fn(&AgentProcessRecord) -> bool + Send + Sync>;
pub type RolloutMetadataReader = Arc<dyn Fn(&Path) -> Option<CodexRolloutMetadata> + Send + Sync>;
pub type DirectoryScanStartedAt = Arc<dyn Fn() -> Timestamp + Send + Sync>;
pub type DidVisitDirectoryEntry = Arc<dyn Fn() + Send + Sync>;

/// Scanner for local coding-agent sessions and rollout activity (`LocalAgentSessionScanner.swift`).
#[derive(Clone)]
pub struct LocalAgentSessionScanner {
    pub config: SessionScanConfig,
    local_probe: LocalProbe,
    environment: BTreeMap<String, SecretString>,
    home_dir: Option<PathBuf>,
    lsof_candidates: Vec<PathBuf>,
    process_registry: Arc<ProcessRegistry>,
    launch_gate: Arc<LaunchGate>,
    cancel: CancellationToken,
    future_modification_date_clamp: Arc<FutureModificationDateClamp>,
    process_output_provider: Option<ProcessOutputProvider>,
    cwd_provider: Option<CwdProvider>,
    app_server_trust_validator: Option<AppServerTrustValidator>,
    rollout_metadata_reader: Option<RolloutMetadataReader>,
    directory_scan_started_at: Option<DirectoryScanStartedAt>,
    did_visit_directory_entry: Option<DidVisitDirectoryEntry>,
    injected_processes: Option<Vec<AgentProcessSnapshot>>,
    injected_cwds: Option<BTreeMap<i32, String>>,
    injected_process_envs: Option<BTreeMap<i32, BTreeMap<String, String>>>,
}

impl std::fmt::Debug for LocalAgentSessionScanner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalAgentSessionScanner")
            .field("config", &self.config)
            .field("home_dir", &self.home_dir)
            .finish()
    }
}

impl Default for LocalAgentSessionScanner {
    fn default() -> Self {
        Self::with_config(SessionScanConfig::default())
    }
}

impl LocalAgentSessionScanner {
    /// Creates a scanner with the host-observers signature (`runtime.rs` / `host_observers.rs`).
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
            lsof_candidates: DEFAULT_LSOF_CANDIDATES.iter().map(PathBuf::from).collect(),
            process_registry: Arc::new(ProcessRegistry::new()),
            launch_gate: Arc::new(LaunchGate::default()),
            cancel: CancellationToken::new(),
            future_modification_date_clamp: Arc::new(FutureModificationDateClamp::default()),
            process_output_provider: None,
            cwd_provider: None,
            app_server_trust_validator: None,
            rollout_metadata_reader: None,
            directory_scan_started_at: None,
            did_visit_directory_entry: None,
            injected_processes: None,
            injected_cwds: None,
            injected_process_envs: None,
        }
    }

    /// Creates a scanner with the given [`SessionScanConfig`].
    pub fn with_config(config: SessionScanConfig) -> Self {
        Self::new(config, LocalProbe::new(), BTreeMap::new(), None)
    }

    /// Creates a scanner with injected `ps` output and `cwd` providers for unit/integration tests.
    pub fn with_providers(
        process_output_provider: impl Fn(&HashMap<String, String>) -> String + Send + Sync + 'static,
        cwd_provider: impl Fn(&[i32], &HashMap<String, String>) -> HashMap<i32, String>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::default()
            .with_process_output_provider(process_output_provider)
            .with_cwd_provider(cwd_provider)
    }

    pub fn with_process_output_provider(
        mut self,
        provider: impl Fn(&HashMap<String, String>) -> String + Send + Sync + 'static,
    ) -> Self {
        self.process_output_provider = Some(Arc::new(provider));
        self
    }

    pub fn with_cwd_provider(
        mut self,
        provider: impl Fn(&[i32], &HashMap<String, String>) -> HashMap<i32, String>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.cwd_provider = Some(Arc::new(provider));
        self
    }

    pub fn with_app_server_trust_validator(
        mut self,
        validator: impl Fn(&AgentProcessRecord) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.app_server_trust_validator = Some(Arc::new(validator));
        self
    }

    pub fn with_rollout_metadata_reader(
        mut self,
        reader: impl Fn(&Path) -> Option<CodexRolloutMetadata> + Send + Sync + 'static,
    ) -> Self {
        self.rollout_metadata_reader = Some(Arc::new(reader));
        self
    }

    pub fn with_directory_scan_started_at(
        mut self,
        started_at: impl Fn() -> Timestamp + Send + Sync + 'static,
    ) -> Self {
        self.directory_scan_started_at = Some(Arc::new(started_at));
        self
    }

    pub fn with_did_visit_directory_entry(
        mut self,
        callback: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        self.did_visit_directory_entry = Some(Arc::new(callback));
        self
    }

    /// Attaches the runtime's shared [`ProcessRegistry`], [`LaunchGate`], and cancellable
    /// [`CancellationToken`] for `lsof` fallback subprocesses and process enumeration.
    pub fn with_process_context(
        mut self,
        process_registry: Arc<ProcessRegistry>,
        launch_gate: Arc<LaunchGate>,
        cancel: CancellationToken,
    ) -> Self {
        self.process_registry = process_registry;
        self.launch_gate = launch_gate;
        self.cancel = cancel;
        self
    }

    pub fn with_lsof_candidates(mut self, candidates: Vec<PathBuf>) -> Self {
        self.lsof_candidates = candidates;
        self
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

    pub const fn should_scan_session_metadata(
        has_agent_processes: bool,
        include_file_only_sessions: bool,
        has_trusted_codex_app_server: bool,
    ) -> bool {
        has_agent_processes || include_file_only_sessions || has_trusted_codex_app_server
    }

    /// Scans sessions using the provided environment map (`LocalAgentSessionScanner.scan`, `LocalAgentSessionScanner.swift:189-199`).
    pub async fn scan_with_env(
        &self,
        now: Timestamp,
        environment: &HashMap<String, String>,
        include_file_only_sessions: bool,
    ) -> Vec<AgentSession> {
        self.scan_with_activity_and_env(now, environment, include_file_only_sessions, false)
            .await
            .sessions
    }

    /// Scans sessions and rollout activity using the scanner's configured environment
    /// (`runtime.rs` / `host_observers.rs`).
    pub async fn scan_with_activity(
        &self,
        now: Timestamp,
        include_file_only_sessions: bool,
        include_rollout_activity: bool,
    ) -> LocalSessionScanResult {
        let env = self.merged_environment_map();
        self.scan_with_activity_and_env(
            now,
            &env,
            include_file_only_sessions,
            include_rollout_activity,
        )
        .await
    }

    /// Full port of `LocalAgentSessionScanner.scanWithActivity` (`LocalAgentSessionScanner.swift:203-282`).
    pub async fn scan_with_activity_and_env(
        &self,
        now: Timestamp,
        environment: &HashMap<String, String>,
        include_file_only_sessions: bool,
        include_rollout_activity: bool,
    ) -> LocalSessionScanResult {
        let all_processes = self.process_records(environment).await;
        let mut processes = AgentSessionCorrelation::newest_processes_first(
            &AgentPSOutputParser::agent_processes(&all_processes),
        );
        processes.truncate(self.config.max_process_count);

        let home_directory = environment
            .get("HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| self.home_dir.clone())
            .unwrap_or_else(|| PathBuf::from("/"));

        let read_rollout_metadata = include_file_only_sessions || !include_rollout_activity;
        let trusted_codex_app_server_present = if read_rollout_metadata {
            self.has_trusted_chatgpt_codex_app_server(&all_processes)
                .await
        } else {
            false
        };

        if !Self::should_scan_session_metadata(
            !processes.is_empty(),
            include_file_only_sessions,
            trusted_codex_app_server_present,
        ) && !include_rollout_activity
        {
            return LocalSessionScanResult::default();
        }

        let codex_app_server_present = AgentPSOutputParser::has_codex_app_server(&all_processes)
            || trusted_codex_app_server_present;
        let pids: Vec<i32> = processes.iter().map(|p| p.pid).collect();
        let cwd_by_pid = self.cwd_by_pid(&pids, environment).await;
        let codex_cwds: Vec<String> = processes
            .iter()
            .filter(|p| AgentPSOutputParser::provider(p) == Some(AgentSessionProvider::Codex))
            .filter_map(|p| cwd_by_pid.get(&p.pid).cloned())
            .collect();
        let codex_home_directory = environment
            .get("CODEX_HOME")
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home_directory.join(".codex"));
        let host = self.host_name(environment).await;

        let started_at = self
            .directory_scan_started_at
            .as_ref()
            .map_or_else(Timestamp::now, |f| f());
        let time_limit = if include_file_only_sessions {
            self.config.directory_scan_budget
        } else {
            self.config
                .directory_scan_budget
                .min(self.config.adaptive_directory_scan_budget)
        };
        let mut directory_budget = DirectoryMetadataScanBudget::new(
            self.config.max_directory_entry_count,
            self.config.max_directory_depth,
            time_limit,
            started_at,
        )
        .with_did_visit_entry(self.did_visit_directory_entry.clone());

        let mut pi_family_directory_budget = directory_budget.clone();
        let pi_family_sessions = self.scan_pi_family_sessions(
            &processes,
            &cwd_by_pid,
            environment,
            &home_directory,
            now,
            &host,
            &mut pi_family_directory_budget,
        );

        let include_unmatched_codex_rollouts =
            include_file_only_sessions || trusted_codex_app_server_present;
        let enrich_rollouts =
            read_rollout_metadata && (include_unmatched_codex_rollouts || !codex_cwds.is_empty());
        let mut candidates = if enrich_rollouts {
            self.codex_rollout_candidates(now, &codex_home_directory, &mut directory_budget)
        } else {
            Vec::new()
        };
        let rollouts = if enrich_rollouts {
            self.codex_rollouts(
                &candidates,
                (!include_unmatched_codex_rollouts).then_some(codex_cwds.as_slice()),
                &directory_budget,
            )
        } else {
            Vec::new()
        };
        let thread_metadata = if rollouts.is_empty() {
            HashMap::new()
        } else {
            Self::codex_thread_metadata(&rollouts, &codex_home_directory, environment)
        };
        let sessions = self.sessions(
            &processes,
            &cwd_by_pid,
            &rollouts,
            ScanContext {
                home_directory,
                host,
                now,
                codex_app_server_present,
                include_unmatched_codex_rollouts,
                thread_metadata,
                pi_family_sessions,
            },
            &mut directory_budget,
        );
        if include_rollout_activity && !enrich_rollouts {
            candidates =
                self.codex_rollout_candidates(now, &codex_home_directory, &mut directory_budget);
        }
        LocalSessionScanResult {
            sessions,
            latest_rollout_activity_at: include_rollout_activity
                .then(|| candidates.first().map(|c| c.1))
                .flatten(),
        }
    }

    fn sessions(
        &self,
        processes: &[AgentProcessRecord],
        cwd_by_pid: &HashMap<i32, String>,
        rollouts: &[Rollout],
        context: ScanContext,
        directory_budget: &mut DirectoryMetadataScanBudget,
    ) -> Vec<AgentSession> {
        let mut sessions = Vec::new();
        let mut matched_rollout_paths: HashSet<PathBuf> = HashSet::new();

        let claude_processes: Vec<AgentProcessRecord> = processes
            .iter()
            .filter(|p| AgentPSOutputParser::provider(p) == Some(AgentSessionProvider::Claude))
            .cloned()
            .collect();
        let mut claude_cwds: Vec<String> = claude_processes
            .iter()
            .filter_map(|p| cwd_by_pid.get(&p.pid).cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        claude_cwds.sort();

        let mut claude_transcripts_by_cwd: HashMap<String, Vec<ClaudeTranscript>> = HashMap::new();
        for cwd in claude_cwds {
            let transcripts = ClaudeSessionProjectMapper::transcripts_with_budget(
                &cwd,
                &context.home_directory,
                Some(self.config.max_claude_transcript_count_per_project),
                context.now,
                directory_budget,
                |url, modified_at, now_ts| {
                    self.future_modification_date_clamp
                        .clamp(url, modified_at, now_ts)
                },
            );
            claude_transcripts_by_cwd.insert(cwd, transcripts);
        }
        let claude_transcripts = AgentSessionCorrelation::assign_claude_transcripts(
            &claude_processes,
            cwd_by_pid,
            &claude_transcripts_by_cwd,
        );

        let codex_processes: Vec<AgentProcessRecord> = processes
            .iter()
            .filter(|p| AgentPSOutputParser::provider(p) == Some(AgentSessionProvider::Codex))
            .cloned()
            .collect();
        let codex_descriptive_name_pids =
            AgentSessionCorrelation::unambiguous_process_ids(&codex_processes, cwd_by_pid);

        for process in processes {
            let Some(provider) = AgentPSOutputParser::provider(process) else {
                continue;
            };
            if provider == AgentSessionProvider::Pi {
                continue;
            }
            let process_cwd = cwd_by_pid.get(&process.pid).cloned();
            let rollout = if provider == AgentSessionProvider::Codex {
                rollouts.iter().find(|candidate| {
                    !matched_rollout_paths.contains(&candidate.url)
                        && AgentSessionCorrelation::codex_working_directories_match(
                            candidate.metadata.cwd.as_deref(),
                            process_cwd.as_deref(),
                        )
                })
            } else {
                None
            };
            if let Some(rollout) = rollout {
                matched_rollout_paths.insert(rollout.url.clone());
            }
            let transcript = if provider == AgentSessionProvider::Claude {
                claude_transcripts.get(&process.pid)
            } else {
                None
            };
            let modified_at = rollout
                .map(|r| r.modified_at)
                .or_else(|| transcript.map(|t| t.modified_at));
            let cwd = process_cwd.or_else(|| rollout.and_then(|r| r.metadata.cwd.clone()));
            let rollout_source = rollout.map(|r| r.metadata.session_source());
            let id = rollout
                .map(|r| r.metadata.session_id.clone())
                .or_else(|| {
                    transcript
                        .and_then(|t| t.url.file_stem().map(|s| s.to_string_lossy().into_owned()))
                })
                .unwrap_or_else(|| format!("pid:{}", process.pid));
            let source = if provider == AgentSessionProvider::Claude {
                AgentPSOutputParser::source(process)
            } else {
                match rollout_source {
                    Some(s) if s != AgentSessionSource::Unknown => s,
                    _ => AgentSessionSource::Cli,
                }
            };
            let session_name = if codex_descriptive_name_pids.contains(&process.pid) {
                rollout.and_then(|r| {
                    r.metadata
                        .descriptive_name(context.thread_metadata.get(&r.metadata.session_id))
                })
            } else {
                None
            };
            let transcript_path = rollout
                .map(|r| r.url.to_string_lossy().into_owned())
                .or_else(|| transcript.map(|t| t.url.to_string_lossy().into_owned()));

            sessions.push(AgentSession {
                id,
                provider,
                dialect: None,
                source,
                state: self.config.state(modified_at, context.now, true),
                pid: Some(process.pid),
                project_name: cwd.as_deref().and_then(project_name_from_cwd),
                cwd,
                session_name,
                started_at: process.started_at,
                last_activity_at: modified_at,
                transcript_path,
                host: context.host.clone(),
            });
        }

        if context.include_unmatched_codex_rollouts {
            for rollout in rollouts {
                if matched_rollout_paths.contains(&rollout.url) {
                    continue;
                }
                let Some(mut session) = CodexRolloutFirstLineParser::make_session(
                    &rollout.metadata,
                    &rollout.url,
                    rollout.modified_at,
                    None,
                    None,
                    &context.host,
                    &self.config,
                    context.now,
                ) else {
                    continue;
                };
                session.session_name = rollout
                    .metadata
                    .descriptive_name(context.thread_metadata.get(&rollout.metadata.session_id));
                session.source = AgentSessionCorrelation::file_only_codex_source(
                    session.source,
                    context.codex_app_server_present,
                );
                sessions.push(session);
            }
        }

        sessions.extend(context.pi_family_sessions);

        sessions.sort_by(|lhs, rhs| {
            let lhs_active = lhs.state == AgentSessionState::Active;
            let rhs_active = rhs.state == AgentSessionState::Active;
            if lhs_active != rhs_active {
                return rhs_active.cmp(&lhs_active);
            }
            let lhs_ts = lhs
                .last_activity_at
                .or(lhs.started_at)
                .unwrap_or(Timestamp::MIN);
            let rhs_ts = rhs
                .last_activity_at
                .or(rhs.started_at)
                .unwrap_or(Timestamp::MIN);
            rhs_ts.cmp(&lhs_ts).then_with(|| lhs.id.cmp(&rhs.id))
        });

        let mut seen = HashSet::new();
        sessions
            .into_iter()
            .filter(|s| seen.insert(format!("{}:{}", s.host, s.id)))
            .collect()
    }

    pub fn codex_rollout_candidates(
        &self,
        now: Timestamp,
        codex_home_directory: &Path,
        directory_budget: &mut DirectoryMetadataScanBudget,
    ) -> Vec<(PathBuf, Timestamp)> {
        let root = codex_home_directory.join("sessions");
        let day_dirs = recent_codex_day_directories(&root, now);

        let mut candidates: Vec<(PathBuf, Timestamp)> = Vec::new();
        for directory in day_dirs {
            let files = directory_budget.files(&directory);
            let day_candidates = directory_budget.compact_map_while_time_remains(&files, |file| {
                let name = file.file_name()?.to_str()?;
                if !name.starts_with("rollout-") || file.extension()? != "jsonl" {
                    return None;
                }
                let meta = fs::symlink_metadata(file).ok()?;
                if meta.file_type().is_symlink() || !meta.is_file() {
                    return None;
                }
                let modified_ts = system_time_to_timestamp(meta.modified().ok()?)?;
                let clamped = self
                    .future_modification_date_clamp
                    .clamp(file, modified_ts, now);
                Some((file.clone(), clamped))
            });
            candidates.extend(day_candidates);
        }
        candidates.sort_by_key(|lhs| std::cmp::Reverse(lhs.1));
        candidates.truncate(self.config.max_codex_rollout_count);
        candidates
    }

    fn codex_rollouts(
        &self,
        candidates: &[(PathBuf, Timestamp)],
        matching_cwds: Option<&[String]>,
        directory_budget: &DirectoryMetadataScanBudget,
    ) -> Vec<Rollout> {
        let mut remaining_cwds: Vec<String> = matching_cwds.map_or_else(Vec::new, |s| s.to_vec());
        let mut rollouts = Vec::new();
        for (url, modified_at) in candidates {
            if !directory_budget.has_time_remaining() {
                break;
            }
            let metadata = if let Some(ref reader) = self.rollout_metadata_reader {
                reader(url)
            } else {
                CodexRolloutFirstLineParser::read(url)
            };
            let Some(metadata) = metadata else {
                continue;
            };
            if let Some(idx) = remaining_cwds.iter().position(|cwd| {
                AgentSessionCorrelation::codex_working_directories_match(
                    metadata.cwd.as_deref(),
                    Some(cwd.as_str()),
                )
            }) {
                remaining_cwds.remove(idx);
                rollouts.push(Rollout {
                    url: url.clone(),
                    modified_at: *modified_at,
                    metadata,
                });
                if matching_cwds.is_some() && remaining_cwds.is_empty() {
                    break;
                }
            } else {
                rollouts.push(Rollout {
                    url: url.clone(),
                    modified_at: *modified_at,
                    metadata,
                });
            }
        }
        rollouts
    }

    fn codex_thread_metadata(
        rollouts: &[Rollout],
        codex_home_directory: &Path,
        environment: &HashMap<String, String>,
    ) -> HashMap<String, CodexThreadMetadata> {
        let session_ids: HashSet<String> = rollouts
            .iter()
            .map(|r| r.metadata.session_id.clone())
            .collect();
        let indexed_names =
            CodexThreadMetadataReader::indexed_thread_names(codex_home_directory, &session_ids);
        let mut groups: HashMap<PathBuf, (CodexThreadMetadataReader, HashSet<String>)> =
            HashMap::new();
        for rollout in rollouts {
            let resolved_cwd = rollout
                .metadata
                .cwd
                .as_deref()
                .map(|c| PathBuf::from(standardize_path_str(c)));
            let reader = CodexThreadMetadataReader::new(
                codex_home_directory,
                environment,
                resolved_cwd.as_deref(),
            );
            let key = reader.database_url.clone();
            groups
                .entry(key)
                .or_insert_with(|| (reader, HashSet::new()))
                .1
                .insert(rollout.metadata.session_id.clone());
        }

        let mut metadata = HashMap::new();
        for (_, (reader, ids)) in groups {
            for (k, v) in reader.metadata(&ids, &indexed_names) {
                metadata.insert(k, v);
            }
        }
        metadata
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_pi_family_sessions(
        &self,
        processes: &[AgentProcessRecord],
        cwd_by_pid: &HashMap<i32, String>,
        environment: &HashMap<String, String>,
        home_dir: &Path,
        now: Timestamp,
        host: &str,
        budget: &mut DirectoryMetadataScanBudget,
    ) -> Vec<AgentSession> {
        let mut sessions = Vec::new();
        let fallback_env: BTreeMap<String, SecretString> = environment
            .iter()
            .map(|(k, v)| (k.clone(), SecretString::new(v.clone())))
            .collect();

        for process in processes {
            if AgentPSOutputParser::provider(process) != Some(AgentSessionProvider::Pi) {
                continue;
            }
            let dialect = AgentPSOutputParser::pi_dialect(process);
            let cwd = cwd_by_pid.get(&process.pid).cloned();
            let last_activity_at = if let Some(ref cwd_str) = cwd
                && budget.has_time_remaining()
            {
                let proc_env = if let Some(ref injected) = self.injected_process_envs {
                    injected.get(&process.pid).cloned().unwrap_or_default()
                } else if let Some(ref env_map) = process.pi_selector_environment {
                    env_map.clone()
                } else {
                    procscan::read_selected_environment(
                        self.local_probe.proc_root(),
                        process.pid,
                        &["PI_CODING_AGENT_DIR", "OMP_CODING_AGENT_DIR"],
                    )
                };
                let exe_name = dialect.map(|d| d.as_str());
                let sessions_root =
                    resolve_pi_sessions_root(home_dir, exe_name, &proc_env, &fallback_env);
                let bucket_dir = sessions_root.join(encode_pi_cwd_bucket(cwd_str));
                let files = budget.files(&bucket_dir);
                budget
                    .compact_map_while_time_remains(&files, |path| {
                        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                            return None;
                        }
                        let meta = fs::symlink_metadata(path).ok()?;
                        if meta.file_type().is_symlink() || !meta.is_file() {
                            return None;
                        }
                        let modified_ts = system_time_to_timestamp(meta.modified().ok()?)?;
                        Some(
                            self.future_modification_date_clamp
                                .clamp(path, modified_ts, now),
                        )
                    })
                    .into_iter()
                    .max()
            } else {
                None
            };

            sessions.push(AgentSession {
                id: format!("pid:{}", process.pid),
                provider: AgentSessionProvider::Pi,
                dialect,
                source: AgentSessionSource::Cli,
                state: self.config.state(last_activity_at, now, true),
                pid: Some(process.pid),
                project_name: cwd.as_deref().and_then(project_name_from_cwd),
                cwd,
                session_name: None,
                started_at: process.started_at,
                last_activity_at,
                transcript_path: None,
                host: host.to_owned(),
            });
        }
        sessions
    }

    async fn has_trusted_chatgpt_codex_app_server(&self, records: &[AgentProcessRecord]) -> bool {
        if let Some(ref validator) = self.app_server_trust_validator {
            return AgentPSOutputParser::has_trusted_chatgpt_codex_app_server(records, |r| {
                validator(r)
            });
        }
        for record in records {
            let executable = record
                .arguments
                .as_ref()
                .and_then(|a| a.first())
                .map(String::as_str)
                .or_else(|| record.command.split_whitespace().next())
                .unwrap_or("");
            if AgentPSOutputParser::CHATGPT_CODEX_EXECUTABLE_PATHS.contains(&executable)
                && AgentPSOutputParser::arguments(record)
                    .iter()
                    .any(|a| a == "app-server")
                && ChatGPTCodexProcessTrust::is_trusted_async(
                    record.pid,
                    &self.local_probe,
                    &self.process_registry,
                    &self.launch_gate,
                    &self.cancel,
                )
                .await
            {
                return true;
            }
        }
        false
    }

    async fn process_records(
        &self,
        environment: &HashMap<String, String>,
    ) -> Vec<AgentProcessRecord> {
        if let Some(ref injected) = self.injected_processes {
            return injected
                .iter()
                .map(|snap| {
                    AgentProcessRecord::new(snap.pid, 1, None, snap.command.clone())
                        .with_arguments(snap.executable_name.as_ref().map(|exe| vec![exe.clone()]))
                })
                .collect();
        }
        if let Some(ref provider) = self.process_output_provider {
            return AgentPSOutputParser::parse(&provider(environment));
        }
        let call = BrokerCall {
            interaction: FetchInteraction::Background,
            cancel: self.cancel.clone(),
            request_id: "session-process-scan".to_owned(),
        };
        let Ok(processes) = self.local_probe.same_user_processes(&call, |_| true).await else {
            return Vec::new();
        };
        let proc_root = self.local_probe.proc_root();
        processes
            .into_iter()
            .map(|proc_entry| {
                let (ppid, start_secs) =
                    process_ppid_and_start_secs(proc_root, proc_entry.identity.pid)
                        .unwrap_or((1, None));
                let started_at = start_secs.and_then(|secs| Timestamp::from_second(secs).ok());
                let argv = proc_entry.argv.into_inner();
                let command = argv.join(" ");
                AgentProcessRecord::new(proc_entry.identity.pid, ppid, started_at, command)
                    .with_arguments(Some(argv))
            })
            .collect()
    }

    async fn cwd_by_pid(
        &self,
        pids: &[i32],
        environment: &HashMap<String, String>,
    ) -> HashMap<i32, String> {
        if let Some(ref injected) = self.injected_cwds {
            return injected
                .iter()
                .filter(|(pid, _)| pids.contains(pid))
                .map(|(pid, cwd)| (*pid, cwd.clone()))
                .collect();
        }
        if let Some(ref provider) = self.cwd_provider {
            return provider(pids, environment);
        }
        if pids.is_empty() {
            return HashMap::new();
        }
        let mut resolved = HashMap::new();
        let mut missing = Vec::new();
        let proc_root = self.local_probe.proc_root();
        for &pid in pids {
            if let Some(cwd) = procscan::process_cwd(proc_root, pid)
                && let Some(cwd_str) = cwd.to_str()
                && !cwd_str.is_empty()
            {
                resolved.insert(pid, cwd_str.to_owned());
            } else {
                missing.push(pid);
            }
        }
        if !missing.is_empty()
            && let Ok(abs_lsof) =
                resolve_absolute_candidates(&self.lsof_candidates, &RealResolverFs)
        {
            let pid_arg = missing
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let args = vec![
                OsString::from("-a"),
                OsString::from("-d"),
                OsString::from("cwd"),
                OsString::from("-Fn"),
                OsString::from("-p"),
                OsString::from(pid_arg),
            ];
            let env = ProcessEnv::from_allowlist(
                environment.iter().map(|(k, v)| (k.as_str(), v.as_str())),
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
                cancel: self.cancel.clone(),
                request_id: "coding-activity-lsof-cwd".to_owned(),
            };
            if let Ok(output) =
                process::run(call, cmd, &self.process_registry, &self.launch_gate).await
                && let Ok(text) = String::from_utf8(output.stdout.to_vec())
            {
                resolved.extend(LSOFCWDOutputParser::parse(&text));
            }
        }
        resolved
    }

    async fn host_name(&self, environment: &HashMap<String, String>) -> String {
        if let Some(host) = environment.get("HOSTNAME").filter(|s| !s.is_empty()) {
            return host.clone();
        }
        for candidate in DEFAULT_HOSTNAME_CANDIDATES {
            if let Some(out) = run_shared_broker_command(
                candidate,
                &[],
                ProcessEnv::empty(),
                Duration::from_secs(2),
                4096,
                &self.process_registry,
                &self.launch_gate,
                &self.cancel,
            )
            .await
                && out.status.success()
                && let Ok(text) = String::from_utf8(out.stdout.to_vec())
            {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_owned();
                }
            }
        }
        "localhost".to_owned()
    }

    fn merged_environment_map(&self) -> HashMap<String, String> {
        let mut map: HashMap<String, String> = self
            .environment
            .iter()
            .map(|(k, v)| (k.clone(), v.expose_secret().to_owned()))
            .collect();
        if !map.contains_key("HOME")
            && let Some(ref home) = self.home_dir
        {
            map.insert("HOME".to_owned(), home.to_string_lossy().into_owned());
        }
        map
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_shared_broker_command(
    executable: &str,
    args: &[&str],
    env: ProcessEnv,
    timeout: Duration,
    stdout_cap: usize,
    process_registry: &ProcessRegistry,
    launch_gate: &LaunchGate,
    cancel: &CancellationToken,
) -> Option<BrokerOutput> {
    let exe_path = AbsolutePath::new(Path::new(executable)).ok()?;
    let mut command = Command::new(exe_path, args.iter().map(OsString::from).collect())
        .with_env(env)
        .timeout(timeout);
    command.set_stdin(StdinSpec::Null);
    command
        .set_stdout(StreamPolicy::Capture { cap: stdout_cap })
        .set_stderr(StreamPolicy::Discard);
    command.set_launch_mode(LaunchMode::Group);
    let call = BrokerCall {
        interaction: FetchInteraction::Background,
        cancel: cancel.clone(),
        request_id: "session-scan-cmd".to_owned(),
    };
    process::run(call, command, process_registry, launch_gate)
        .await
        .ok()
}

pub fn recent_codex_day_directories(sessions_root: &Path, now: Timestamp) -> Vec<PathBuf> {
    let tz = TimeZone::try_system().unwrap_or(TimeZone::UTC);
    let today = now.to_zoned(tz).date();
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

pub fn resolve_pi_sessions_root(
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

pub fn encode_pi_cwd_bucket(cwd: &str) -> String {
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

/// Filters sessions for JSON output (`CodexBarCLI.sessionsForJSON(_:includePiFamily:)`).
///
/// Legacy v1 JSON (`--json`) keeps only Codex and Claude rows so closed-enum v1
/// decoders continue to succeed; `--json-v2` sets `include_pi_family = true`.
pub fn sessions_for_json(sessions: &[AgentSession], include_pi_family: bool) -> Vec<AgentSession> {
    if include_pi_family {
        return sessions.to_vec();
    }
    sessions
        .iter()
        .filter(|session| {
            matches!(
                session.provider,
                AgentSessionProvider::Codex | AgentSessionProvider::Claude
            )
        })
        .cloned()
        .collect()
}

/// Renders the human-readable sessions table (`CodexBarCLI.renderSessionsTable(_:now:)`).
pub fn render_sessions_table(sessions: &[AgentSession], now: Timestamp) -> String {
    if sessions.is_empty() {
        return "No agent sessions found.".to_owned();
    }
    let rows: Vec<[String; 7]> = sessions
        .iter()
        .map(|session| {
            let state = session.state.as_str();
            let provider = session.provider.as_str();
            let dialect = session.dialect.map_or("—", |d| d.as_str());
            let source = session.source.as_str();
            let project = session.project_name.as_deref().unwrap_or("—");
            let age = session_age(session, now);
            [
                state.to_owned(),
                provider.to_owned(),
                dialect.to_owned(),
                source.to_owned(),
                project.to_owned(),
                age,
                session.id.clone(),
            ]
        })
        .collect();

    let headers = [
        "STATE", "PROVIDER", "DIALECT", "SOURCE", "PROJECT", "ACTIVITY", "ID",
    ];
    let mut widths = [0usize; 7];
    for (idx, header) in headers.iter().enumerate() {
        let max_row = rows
            .iter()
            .map(|r| r[idx].chars().count())
            .max()
            .unwrap_or(0);
        widths[idx] = header.chars().count().max(max_row);
    }

    let render_row = |cols: &[&str; 7]| -> String {
        cols.iter()
            .enumerate()
            .map(|(idx, col)| {
                let len = col.chars().count();
                let pad = widths[idx].saturating_sub(len);
                format!("{col}{}", " ".repeat(pad))
            })
            .collect::<Vec<_>>()
            .join("  ")
    };

    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(render_row(&headers));
    for row in &rows {
        let refs = [
            row[0].as_str(),
            row[1].as_str(),
            row[2].as_str(),
            row[3].as_str(),
            row[4].as_str(),
            row[5].as_str(),
            row[6].as_str(),
        ];
        lines.push(render_row(&refs));
    }
    lines.join("\n")
}

fn session_age(session: &AgentSession, now: Timestamp) -> String {
    let Some(date) = session.last_activity_at.or(session.started_at) else {
        return "now".to_owned();
    };
    let seconds = (now.as_second() - date.as_second()).max(0);
    if seconds < 60 {
        return format!("{seconds}s");
    }
    if seconds < 3600 {
        return format!("{}m", seconds / 60);
    }
    if seconds < 86_400 {
        return format!("{}h", seconds / 3600);
    }
    format!("{}d", seconds / 86_400)
}

fn project_name_from_cwd(cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    Path::new(cwd)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
}

fn standardize_path_buf(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from("/")
    } else {
        normalized
    }
}

fn standardize_path_str(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let path = standardize_path_buf(Path::new(trimmed));
    let text = path.to_string_lossy().into_owned();
    if let Some(rest) = text.strip_prefix("/private/var/") {
        return format!("/var/{rest}");
    }
    if let Some(rest) = text.strip_prefix("/private/tmp/") {
        return format!("/tmp/{rest}");
    }
    text
}

pub fn system_time_to_timestamp(time: SystemTime) -> Option<Timestamp> {
    let duration = time.duration_since(UNIX_EPOCH).ok()?;
    let millis = i64::try_from(duration.as_millis()).ok()?;
    Timestamp::from_millisecond(millis).ok()
}
