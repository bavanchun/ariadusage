// Ported from CodexBar Sources/CodexBarCore/AgentSession.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/LocalAgentSessionScanner.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLISessionsCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::pipeline::FetchInteraction;
use jiff::Timestamp;
use jiff::civil::{Date, DateTime, Time};
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::brokers::call::BrokerCall;
use crate::brokers::local_probe::LocalProbe;
use crate::brokers::process::{
    AbsolutePath, Command as BrokerCommand, Output as BrokerOutput, ProcessEnv, ProcessRegistry,
    StreamPolicy, process_ppid_and_start_secs, run as run_process,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
        let elapsed = (now.as_second() - last_activity_at.as_second()) as f64;
        if elapsed <= self.active_window {
            AgentSessionState::Active
        } else {
            AgentSessionState::Idle
        }
    }
}

pub(crate) struct DirectoryMetadataScanBudget {
    remaining_entry_count: usize,
    max_depth: usize,
    deadline: Instant,
}

impl DirectoryMetadataScanBudget {
    pub(crate) fn new(max_entry_count: usize, max_depth: usize, time_limit: f64) -> Self {
        Self {
            remaining_entry_count: max_entry_count,
            max_depth,
            deadline: Instant::now() + Duration::from_secs_f64(time_limit.max(0.0)),
        }
    }

    pub(crate) fn has_time_remaining(&self) -> bool {
        Instant::now() < self.deadline
    }

    pub(crate) fn files(&mut self, directory: &Path) -> Vec<PathBuf> {
        if self.max_depth == 0 || self.remaining_entry_count == 0 || !self.has_time_remaining() {
            return Vec::new();
        }
        let Ok(meta) = fs::symlink_metadata(directory) else {
            return Vec::new();
        };
        if !meta.file_type().is_dir() {
            return Vec::new();
        }
        let Ok(read_dir) = fs::read_dir(directory) else {
            return Vec::new();
        };
        let mut files = Vec::new();
        for entry in read_dir {
            if self.remaining_entry_count == 0 || !self.has_time_remaining() {
                break;
            }
            let Ok(entry) = entry else {
                continue;
            };
            let file_name = entry.file_name();
            if file_name.to_string_lossy().starts_with('.') {
                continue;
            }
            self.remaining_entry_count -= 1;
            if !self.has_time_remaining() {
                break;
            }
            let path = entry.path();
            let Ok(entry_meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if entry_meta.file_type().is_file() {
                files.push(path);
            }
        }
        files
    }

    pub(crate) fn compact_map_while_time_remains<T, F>(
        &self,
        urls: Vec<PathBuf>,
        mut transform: F,
    ) -> Vec<T>
    where
        F: FnMut(PathBuf) -> Option<T>,
    {
        let mut results = Vec::new();
        for url in urls {
            if !self.has_time_remaining() {
                break;
            }
            if let Some(result) = transform(url) {
                results.push(result);
            }
        }
        results
    }
}

#[derive(Debug, Default)]
struct FutureModificationDateClamp {
    clamp_date: Mutex<Option<Timestamp>>,
}

impl FutureModificationDateClamp {
    fn clamp(&self, modified_at: Timestamp, now: Timestamp) -> Timestamp {
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
        dt.to_zoned(TimeZone::system())
            .ok()
            .map(|zoned| zoned.timestamp())
    }
}

fn run_broker_command(
    executable: &str,
    args: &[&str],
    env: ProcessEnv,
    timeout: Duration,
    stdout_cap: usize,
) -> Option<BrokerOutput> {
    let exe_path = AbsolutePath::new(Path::new(executable)).ok()?;
    let mut command = BrokerCommand::new(exe_path, args.iter().map(OsString::from).collect())
        .with_env(env)
        .timeout(timeout);
    command
        .set_stdout(StreamPolicy::Capture { cap: stdout_cap })
        .set_stderr(StreamPolicy::Discard);

    let handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        let call = BrokerCall {
            interaction: FetchInteraction::Background,
            cancel: CancellationToken::new(),
            request_id: "session-scan".to_owned(),
        };
        let registry = ProcessRegistry::new();
        let gate = LaunchGate::new(Instant::now);
        rt.block_on(run_process(call, command, &registry, &gate))
            .ok()
    });
    handle.join().ok().flatten()
}

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
            Self::default_process_is_trusted,
            Self::default_app_is_trusted,
        )
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
        let resolved = resolve_path(actual_path);
        if !AgentPSOutputParser::CHATGPT_CODEX_EXECUTABLE_PATHS.contains(&resolved.as_str()) {
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

    fn default_process_is_trusted(pid: i32) -> bool {
        #[cfg(target_os = "macos")]
        {
            let pid_str = pid.to_string();
            run_broker_command(
                "/usr/bin/codesign",
                &["--verify", "-R", Self::REQUIREMENT, &pid_str],
                ProcessEnv::empty(),
                Duration::from_secs(5),
                16 * 1024,
            )
            .is_some_and(|out| out.status.success())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = pid;
            false
        }
    }

    fn default_app_is_trusted(bundle_path: String) -> bool {
        #[cfg(target_os = "macos")]
        {
            run_broker_command(
                "/usr/bin/codesign",
                &["--verify", "-R", Self::REQUIREMENT, &bundle_path],
                ProcessEnv::empty(),
                Duration::from_secs(5),
                16 * 1024,
            )
            .is_some_and(|out| out.status.success())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = bundle_path;
            false
        }
    }
}

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

pub struct ClaudeSessionProjectMapper;

impl ClaudeSessionProjectMapper {
    pub fn project_directory_name(cwd: &str) -> String {
        let standardized = Self::standardize_path(cwd);
        let mut result = String::with_capacity(standardized.len());
        for ch in standardized.chars() {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                result.push(ch);
            } else {
                result.push('-');
            }
        }
        result
    }

    fn standardize_path(path: &str) -> String {
        if path == "/" {
            return "/".to_owned();
        }
        path.trim_end_matches('/').to_owned()
    }
}

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
        let mut pids_by_cwd: HashMap<&str, Vec<i32>> = HashMap::new();
        for process in processes {
            if let Some(cwd) = cwd_by_pid.get(&process.pid) {
                pids_by_cwd
                    .entry(cwd.as_str())
                    .or_default()
                    .push(process.pid);
            }
        }

        let mut assignments = HashMap::new();
        for (cwd, pids) in pids_by_cwd {
            if pids.len() != 1 {
                continue;
            }
            if let Some(transcripts) = transcripts_by_cwd.get(cwd)
                && let Some(newest) = transcripts.iter().max_by_key(|t| t.modified_at)
            {
                assignments.insert(pids[0], newest.clone());
            }
        }
        assignments
    }

    pub fn unambiguous_process_ids(
        processes: &[AgentProcessRecord],
        cwd_by_pid: &HashMap<i32, String>,
    ) -> HashMap<String, i32> {
        let mut pids_by_cwd: HashMap<String, Vec<i32>> = HashMap::new();
        for process in processes {
            if let Some(cwd) = cwd_by_pid.get(&process.pid) {
                pids_by_cwd
                    .entry(Self::normalize_working_directory(cwd))
                    .or_default()
                    .push(process.pid);
            }
        }
        pids_by_cwd
            .into_iter()
            .filter_map(|(cwd, pids)| (pids.len() == 1).then_some((cwd, pids[0])))
            .collect()
    }

    pub fn codex_working_directories_match(lhs: Option<&str>, rhs: Option<&str>) -> bool {
        let (Some(lhs), Some(rhs)) = (lhs, rhs) else {
            return false;
        };
        Self::normalize_working_directory(lhs) == Self::normalize_working_directory(rhs)
    }

    pub fn file_only_codex_source(
        raw_source: Option<&str>,
        originator: Option<&str>,
    ) -> AgentSessionSource {
        let normalized_source = raw_source.map(|s| s.trim().to_ascii_lowercase());
        let normalized_originator = originator.map(|s| s.trim().to_ascii_lowercase());

        if matches!(normalized_source.as_deref(), Some("cli" | "exec"))
            || normalized_originator.as_deref() == Some("codex_cli_rs")
        {
            return AgentSessionSource::Cli;
        }

        if matches!(normalized_source.as_deref(), Some("vscode" | "ide"))
            || normalized_originator
                .as_deref()
                .is_some_and(|orig| orig.contains("vscode"))
        {
            return AgentSessionSource::Ide;
        }

        if normalized_originator
            .as_deref()
            .is_some_and(|orig| orig.contains("desktop") || orig.contains("app"))
        {
            return AgentSessionSource::DesktopApp;
        }

        AgentSessionSource::Unknown
    }

    fn normalize_working_directory(path: &str) -> String {
        let trimmed = path.trim();
        let resolved = fs::canonicalize(trimmed)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| {
                if let Some(rest) = trimmed.strip_prefix("/private/var/") {
                    format!("/var/{rest}")
                } else if let Some(rest) = trimmed.strip_prefix("/private/tmp/") {
                    format!("/tmp/{rest}")
                } else {
                    trimmed.to_owned()
                }
            });
        if resolved.len() > 1 {
            resolved.trim_end_matches('/').to_owned()
        } else {
            resolved
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexRolloutMetadata {
    pub id: Option<String>,
    pub timestamp: Option<Timestamp>,
    pub cwd: Option<String>,
    pub originator: Option<String>,
    pub source: Option<String>,
}

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
        let id = payload
            .get("session_id")
            .and_then(|v| v.as_str())
            .or_else(|| payload.get("id").and_then(|v| v.as_str()))
            .map(ToOwned::to_owned);
        let timestamp = payload
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Timestamp>().ok());
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
        Some(CodexRolloutMetadata {
            id,
            timestamp,
            cwd,
            originator,
            source,
        })
    }

    pub fn read(path: &Path) -> Option<CodexRolloutMetadata> {
        let meta = fs::symlink_metadata(path).ok()?;
        if !meta.file_type().is_file() {
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
}

pub type ProcessOutputProvider = Arc<dyn Fn(&[&str]) -> Option<String> + Send + Sync>;
pub type CwdProvider =
    Arc<dyn Fn(&[i32], &HashMap<String, String>) -> HashMap<i32, String> + Send + Sync>;

#[derive(Clone, Default)]
pub struct LocalAgentSessionScanner {
    pub config: SessionScanConfig,
    future_modification_date_clamp: Arc<FutureModificationDateClamp>,
    process_output_provider: Option<ProcessOutputProvider>,
    cwd_provider: Option<CwdProvider>,
}

impl LocalAgentSessionScanner {
    pub fn new(
        process_output_provider: impl Fn(&[&str]) -> Option<String> + Send + Sync + 'static,
        cwd_provider: impl Fn(&[i32], &HashMap<String, String>) -> HashMap<i32, String>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            config: SessionScanConfig::default(),
            future_modification_date_clamp: Arc::new(FutureModificationDateClamp::default()),
            process_output_provider: Some(Arc::new(process_output_provider)),
            cwd_provider: Some(Arc::new(cwd_provider)),
        }
    }

    pub async fn scan(
        &self,
        environment: &HashMap<String, String>,
        include_file_only_sessions: bool,
    ) -> Vec<AgentSession> {
        self.scan_sync(Timestamp::now(), environment, include_file_only_sessions)
    }

    pub fn scan_sync(
        &self,
        now: Timestamp,
        environment: &HashMap<String, String>,
        include_file_only_sessions: bool,
    ) -> Vec<AgentSession> {
        let records = if let Some(provider) = &self.process_output_provider {
            let ps_args = ["-axo", "pid=,ppid=,lstart=,command="];
            let Some(ps_output) = provider(&ps_args) else {
                return Vec::new();
            };
            AgentPSOutputParser::parse(&ps_output)
        } else {
            Self::default_process_records()
        };

        let mut agents = AgentSessionCorrelation::newest_processes_first(
            &AgentPSOutputParser::agent_processes(&records),
        );
        agents.truncate(self.config.max_process_count);

        let pids: Vec<i32> = agents.iter().map(|r| r.pid).collect();
        let cwd_by_pid = if pids.is_empty() {
            HashMap::new()
        } else if let Some(provider) = &self.cwd_provider {
            provider(&pids, environment)
        } else {
            Self::default_cwd_by_pid(&pids, environment)
        };

        let host = Self::host_name(environment);
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
        );

        let mut sessions = Vec::new();

        for record in &agents {
            let Some(provider) = AgentPSOutputParser::provider(record) else {
                continue;
            };
            let dialect = if provider == AgentSessionProvider::Pi {
                AgentPSOutputParser::pi_dialect(record)
            } else {
                None
            };
            let source = AgentPSOutputParser::source(record);
            let cwd = cwd_by_pid.get(&record.pid).cloned();
            let project_name = cwd.as_deref().and_then(|path| {
                Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            });
            sessions.push(AgentSession {
                id: format!("pid:{}", record.pid),
                provider,
                dialect,
                source,
                state: self.config.state(None, now, true),
                pid: Some(record.pid),
                cwd,
                project_name,
                session_name: None,
                started_at: record.started_at,
                last_activity_at: None,
                transcript_path: None,
                host: host.clone(),
            });
        }

        if include_file_only_sessions
            && (AgentPSOutputParser::has_codex_app_server(&records)
                || AgentPSOutputParser::has_trusted_chatgpt_codex_app_server(&records, |r| {
                    ChatGPTCodexProcessTrust::is_trusted(r.pid)
                }))
        {
            let file_sessions = self.scan_recent_codex_rollouts(
                now,
                environment,
                &host,
                &sessions,
                &mut directory_budget,
            );
            sessions.extend(file_sessions);
        }

        sessions.sort_by(|lhs, rhs| {
            let lhs_state = matches!(lhs.state, AgentSessionState::Active);
            let rhs_state = matches!(rhs.state, AgentSessionState::Active);
            rhs_state
                .cmp(&lhs_state)
                .then_with(|| {
                    rhs.last_activity_at
                        .or(rhs.started_at)
                        .cmp(&lhs.last_activity_at.or(lhs.started_at))
                })
                .then_with(|| lhs.id.cmp(&rhs.id))
        });

        let mut seen = HashSet::new();
        sessions
            .into_iter()
            .filter(|s| seen.insert(format!("{}:{}", s.host, s.id)))
            .collect()
    }

    fn scan_recent_codex_rollouts(
        &self,
        now: Timestamp,
        environment: &HashMap<String, String>,
        host: &str,
        live_sessions: &[AgentSession],
        directory_budget: &mut DirectoryMetadataScanBudget,
    ) -> Vec<AgentSession> {
        let codex_home = environment.get("CODEX_HOME").cloned().or_else(|| {
            environment
                .get("HOME")
                .map(|h| Path::new(h).join(".codex").to_string_lossy().into_owned())
        });
        let Some(codex_home) = codex_home else {
            return Vec::new();
        };
        let candidates =
            self.codex_rollout_candidates(now, Path::new(&codex_home), directory_budget);

        let now_secs = now.as_second();
        let file_only_cutoff = now_secs - (self.config.file_only_window as i64);

        let mut results = Vec::new();
        for (path, modified_ts) in candidates {
            if !directory_budget.has_time_remaining() {
                break;
            }
            if modified_ts.as_second() < file_only_cutoff {
                continue;
            }
            let Some(rollout) = CodexRolloutFirstLineParser::read(&path) else {
                continue;
            };
            let Some(id) = rollout.id else {
                continue;
            };
            if live_sessions.iter().any(|s| {
                s.provider == AgentSessionProvider::Codex
                    && AgentSessionCorrelation::codex_working_directories_match(
                        s.cwd.as_deref(),
                        rollout.cwd.as_deref(),
                    )
            }) {
                continue;
            }
            let state = self.config.state(Some(modified_ts), now, false);
            let source = AgentSessionCorrelation::file_only_codex_source(
                rollout.source.as_deref(),
                rollout.originator.as_deref(),
            );
            let project_name = rollout.cwd.as_deref().and_then(|cwd| {
                Path::new(cwd)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            });
            results.push(AgentSession {
                id,
                provider: AgentSessionProvider::Codex,
                dialect: None,
                source,
                state,
                pid: None,
                cwd: rollout.cwd,
                project_name,
                session_name: None,
                started_at: rollout.timestamp,
                last_activity_at: Some(modified_ts),
                transcript_path: Some(path.to_string_lossy().into_owned()),
                host: host.to_owned(),
            });
        }
        results
    }

    fn codex_rollout_candidates(
        &self,
        now: Timestamp,
        codex_home: &Path,
        directory_budget: &mut DirectoryMetadataScanBudget,
    ) -> Vec<(PathBuf, Timestamp)> {
        let root = codex_home.join("sessions");
        let yesterday = now
            .checked_sub(jiff::SignedDuration::from_hours(24))
            .unwrap_or(now);
        let mut day_dirs = Vec::new();
        for day_ts in [now, yesterday] {
            let zoned = day_ts.to_zoned(TimeZone::system());
            let d = zoned.date();
            let rel = format!("{:04}/{:02}/{:02}", d.year(), d.month(), d.day());
            let dir = root.join(rel);
            if !day_dirs.contains(&dir) {
                day_dirs.push(dir);
            }
        }

        let mut candidates: Vec<(PathBuf, Timestamp)> = Vec::new();
        for directory in day_dirs {
            let files = directory_budget.files(&directory);
            let day_candidates = directory_budget.compact_map_while_time_remains(files, |file| {
                let name = file.file_name()?.to_str()?;
                if !name.starts_with("rollout-") || file.extension()? != "jsonl" {
                    return None;
                }
                let meta = fs::symlink_metadata(&file).ok()?;
                if !meta.file_type().is_file() {
                    return None;
                }
                let modified = meta.modified().ok()?;
                let modified_ts = system_time_to_timestamp(modified)?;
                let clamped = self.future_modification_date_clamp.clamp(modified_ts, now);
                Some((file, clamped))
            });
            candidates.extend(day_candidates);
        }
        candidates.sort_by_key(|lhs| std::cmp::Reverse(lhs.1));
        candidates.truncate(self.config.max_codex_rollout_count);
        candidates
    }

    fn default_process_records() -> Vec<AgentProcessRecord> {
        let handle = std::thread::spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()?;
            let probe = LocalProbe::new();
            let call = BrokerCall {
                interaction: FetchInteraction::Background,
                cancel: CancellationToken::new(),
                request_id: "session-process-scan".to_owned(),
            };
            let processes = rt
                .block_on(probe.same_user_processes(&call, |_| true))
                .ok()?;
            let proc_root = probe.proc_root();
            let records = processes
                .into_iter()
                .map(|proc| {
                    let (ppid, start_secs) =
                        process_ppid_and_start_secs(proc_root, proc.identity.pid)
                            .unwrap_or((1, None));
                    let started_at = start_secs.and_then(|secs| Timestamp::from_second(secs).ok());
                    let argv = proc.argv.into_inner();
                    let command = argv.join(" ");
                    AgentProcessRecord::new(proc.identity.pid, ppid, started_at, command)
                        .with_arguments(Some(argv))
                })
                .collect::<Vec<_>>();
            Some(records)
        });
        handle.join().ok().flatten().unwrap_or_default()
    }

    fn default_cwd_by_pid(
        pids: &[i32],
        environment: &HashMap<String, String>,
    ) -> HashMap<i32, String> {
        let pid_list = pids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let env =
            ProcessEnv::from_allowlist(environment.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        for candidate in ["/usr/sbin/lsof", "/usr/bin/lsof"] {
            if let Some(output) = run_broker_command(
                candidate,
                &["-a", "-d", "cwd", "-Fn", "-p", &pid_list],
                env.clone(),
                Duration::from_secs(5),
                256 * 1024,
            ) && let Ok(stdout) = String::from_utf8(output.stdout.to_vec())
            {
                let parsed = LSOFCWDOutputParser::parse(&stdout);
                if !parsed.is_empty() || output.status.success() {
                    return parsed;
                }
            }
        }
        pids.iter()
            .filter_map(|&pid| {
                let target = fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
                Some((pid, target.to_string_lossy().into_owned()))
            })
            .collect()
    }

    fn host_name(environment: &HashMap<String, String>) -> String {
        if let Some(host) = environment.get("HOSTNAME").filter(|s| !s.is_empty()) {
            return host.clone();
        }
        for candidate in ["/bin/hostname", "/usr/bin/hostname"] {
            if let Some(out) = run_broker_command(
                candidate,
                &[],
                ProcessEnv::empty(),
                Duration::from_secs(2),
                4096,
            ) && out.status.success()
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

pub fn system_time_to_timestamp(time: SystemTime) -> Option<Timestamp> {
    let dur = time.duration_since(UNIX_EPOCH).ok()?;
    Timestamp::from_duration(jiff::SignedDuration::from_secs(dur.as_secs() as i64)).ok()
}

pub fn duration_from_secs_f64(secs: f64) -> Duration {
    Duration::from_secs_f64(secs.max(0.0))
}
