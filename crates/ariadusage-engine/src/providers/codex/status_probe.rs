// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexStatusProbe.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexCLISession.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexStatusMarkers.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexStatusProbeIsolation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/TextParsing.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::pipeline::FetchInteraction;
use jiff::Timestamp;
use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::brokers::call::BrokerCall;
use crate::brokers::exec_resolver::{RealResolverFs, ResolverInputs, resolve_codex_for_rpc};
use crate::brokers::process::session_records::{
    FileSessionLaunchLock, FileSessionRecordStore, ProcSessionRecordIdentity, SessionLaunchLock,
    SessionRecord, SessionRecordIdentity, SessionRecordStore,
};
use crate::brokers::process::{
    AbsolutePath, BoundedOutputBuffer, Command, DEFAULT_OUTPUT_CAP, DrainClassification,
    ProcessEnv, ProcessRegistry, PtyError, PtySession, PtySize, StreamScanBuffer,
};

use super::cli_launch_gate::CodexCLILaunchGate;
use super::cli_rpc::{CodexStatusProbeError, CodexStatusSnapshot};
use super::credits::CodexCreditLimitSnapshot;

/// Byte markers emitted by the interactive Codex CLI during `/status` and update prompts.
pub struct CodexStatusMarkers;

impl CodexStatusMarkers {
    pub const STATUS: &'static [&'static [u8]] =
        &[b"Credits:", b"5h limit", b"5-hour limit", b"Weekly limit"];

    pub const UPDATE_PROMPT: &'static [&'static [u8]] = &[
        b"update available!",
        b"run bun install -g @openai/codex",
        b"0.60.1 ->",
    ];

    pub const LONGEST_STATUS: usize = 12;
    pub const LONGEST_UPDATE_PROMPT: usize = 32;
}

/// Thread-store and working-directory isolation for the Codex `/status` PTY probe.
pub struct CodexStatusProbeIsolation;

impl CodexStatusProbeIsolation {
    /// Resolves and verifies `<base>/ariadusage/CodexStatusProbe` as a private `0700` directory
    /// owned by the current effective UID, trying `XDG_RUNTIME_DIR` in `environment` or
    /// `paths::trusted_runtime_dir()` first and falling back to the state directory
    /// (`paths::resolve_state_dir` / `paths::state_dir()`).
    /// Fails closed if neither resolves as a valid support directory.
    pub fn support_directory(environment: &BTreeMap<String, String>) -> std::io::Result<PathBuf> {
        let env_home = environment
            .get("HOME")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let fallback_home = if env_home.is_none() {
            etcetera::home_dir().ok()
        } else {
            None
        };
        Self::support_directory_with_home(
            environment,
            env_home.as_deref().or(fallback_home.as_deref()),
            None,
        )
    }

    /// Resolves `<base>/ariadusage/CodexStatusProbe` from an injected `runtime_dir` (or
    /// `XDG_RUNTIME_DIR` in `environment`, or `paths::trusted_runtime_dir()` when neither is set)
    /// first and falls back to `paths::resolve_state_dir` from `environment` and `home_dir`.
    pub fn support_directory_with_home(
        environment: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        runtime_dir: Option<&Path>,
    ) -> std::io::Result<PathBuf> {
        Self::support_directory_with_home_and_trusted_runtime_resolver(
            environment,
            home_dir,
            runtime_dir,
            crate::paths::trusted_runtime_dir,
        )
    }

    /// Resolves `<base>/ariadusage/CodexStatusProbe` like [`Self::support_directory_with_home`],
    /// calling `trusted_runtime_resolver` when neither `runtime_dir` nor `XDG_RUNTIME_DIR` in
    /// `environment` provides a runtime directory.
    pub fn support_directory_with_home_and_trusted_runtime_resolver(
        environment: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        runtime_dir: Option<&Path>,
        trusted_runtime_resolver: impl FnOnce() -> Option<PathBuf>,
    ) -> std::io::Result<PathBuf> {
        let env_home = home_dir.map(Path::to_path_buf).or_else(|| {
            environment
                .get("HOME")
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        });
        let resolved_runtime_dir = runtime_dir
            .map(Path::to_path_buf)
            .or_else(|| {
                crate::paths::resolve_runtime_dir(
                    |key| environment.get(key).map(OsString::from),
                    env_home.as_deref(),
                )
            })
            .or_else(trusted_runtime_resolver);
        let state_dir = crate::paths::resolve_state_dir(
            |key| environment.get(key).map(OsString::from),
            env_home.as_deref(),
        )
        .ok();
        Self::resolve_support_directory(resolved_runtime_dir.as_deref(), state_dir.as_deref())
    }

    /// Resolves and verifies `<base>/ariadusage/CodexStatusProbe` from an explicit runtime-directory
    /// candidate and state-directory fallback.
    ///
    /// For the base directory itself (following symlinks on the base), only checks that the canonical
    /// target is an existing directory owned by the current effective UID and is not group- or
    /// world-writable (`(mode & 0o022) == 0`), without `chmod`ing the base or creating its parents.
    /// Applies [`crate::private_tempdir::ensure_private_0700_dir`] from `<base>/ariadusage` down.
    pub fn resolve_support_directory(
        runtime_dir: Option<&Path>,
        state_dir: Option<&Path>,
    ) -> std::io::Result<PathBuf> {
        if let Some(dir) = runtime_dir.and_then(Self::ensure_private_probe_dir) {
            return Ok(dir);
        }
        if let Some(dir) = state_dir.and_then(Self::ensure_private_probe_dir) {
            return Ok(dir);
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "CodexStatusProbe support directory unavailable or failed 0700 ownership/symlink checks",
        ))
    }

    fn verify_support_base_dir(base_dir: &Path) -> Option<PathBuf> {
        #[cfg(unix)]
        {
            use rustix::fs::{FileType, stat};
            use rustix::process::geteuid;

            if !base_dir.is_absolute() {
                return None;
            }
            let canonical = std::fs::canonicalize(base_dir).ok()?;
            let dir_stat = stat(&canonical).ok()?;
            if FileType::from_raw_mode(dir_stat.st_mode) != FileType::Directory {
                return None;
            }
            if dir_stat.st_uid != geteuid().as_raw() {
                return None;
            }
            if (dir_stat.st_mode & 0o022) != 0 {
                return None;
            }
            Some(canonical)
        }
        #[cfg(not(unix))]
        {
            let _ = base_dir;
            None
        }
    }

    fn ensure_private_probe_dir(base_dir: &Path) -> Option<PathBuf> {
        let base = Self::verify_support_base_dir(base_dir)?;
        let app_dir = crate::private_tempdir::ensure_private_0700_dir(&base.join("ariadusage"))?;
        crate::private_tempdir::ensure_private_0700_dir(&app_dir.join("CodexStatusProbe"))
    }

    #[must_use]
    pub fn working_directory(environment: &BTreeMap<String, String>) -> Option<PathBuf> {
        let home = environment.get("HOME")?.trim();
        if home.is_empty() {
            return None;
        }
        Some(PathBuf::from(home))
    }

    #[must_use]
    pub fn codex_arguments(state_home: &Path) -> Vec<String> {
        let escaped = Self::toml_escaped(&state_home.to_string_lossy());
        vec![
            "-s".to_owned(),
            "read-only".to_owned(),
            "-a".to_owned(),
            "never".to_owned(),
            "-c".to_owned(),
            "history.persistence=\"none\"".to_owned(),
            "-c".to_owned(),
            "experimental_thread_store={type=\"in_memory\",id=\"codexbar-status\"}".to_owned(),
            "-c".to_owned(),
            format!("sqlite_home=\"{escaped}\""),
        ]
    }

    #[must_use]
    pub fn toml_escaped(value: &str) -> String {
        value.replace('\\', "\\\\").replace('"', "\\\"")
    }
}

/// Terminal text parsing utilities ported from `TextParsing.swift`.
pub struct TextParsing;

impl TextParsing {
    /// Removes ANSI CSI escape sequences (`\x1b\[[0-?]*[ -/]*[@-~]`) so parsing works on colored terminal output.
    #[must_use]
    pub fn strip_ansi_codes(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == 0x1B && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                let mut j = i + 2;
                while j < bytes.len() && (0x30..=0x3F).contains(&bytes[j]) {
                    j += 1;
                }
                while j < bytes.len() && (0x20..=0x2F).contains(&bytes[j]) {
                    j += 1;
                }
                if j < bytes.len() && (0x40..=0x7E).contains(&bytes[j]) {
                    i = j + 1;
                    continue;
                }
            }
            let ch = text[i..].chars().next().expect("valid char slice");
            out.push(ch);
            i += ch.len_utf8();
        }
        out
    }

    #[must_use]
    pub fn parse_number(raw: &str) -> Option<f64> {
        let mut text = raw.trim().replace(['\u{00A0}', '\u{202F}', ' '], "");

        let has_comma = text.contains(',');
        let has_dot = text.contains('.');

        if has_comma && has_dot {
            if let (Some(last_comma), Some(last_dot)) = (text.rfind(','), text.rfind('.')) {
                if last_comma > last_dot {
                    text = text.replace('.', "").replace(',', ".");
                } else {
                    text = text.replace(',', "");
                }
            }
        } else if has_comma {
            if Self::is_thousands_grouped(&text, ',') {
                text = text.replace(',', "");
            } else {
                text = text.replace(',', ".");
            }
        } else if has_dot && Self::is_thousands_grouped(&text, '.') {
            text = text.replace('.', "");
        }

        text.parse::<f64>().ok()
    }

    fn is_thousands_grouped(text: &str, sep: char) -> bool {
        let parts: Vec<&str> = text.split(sep).collect();
        if parts.len() < 2 {
            return false;
        }
        let first = parts[0];
        if !(1..=3).contains(&first.len()) || !first.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        parts[1..]
            .iter()
            .all(|part| part.len() == 3 && part.bytes().all(|b| b.is_ascii_digit()))
    }

    #[must_use]
    pub fn first_credits_number(text: &str) -> Option<f64> {
        let lower = text.to_ascii_lowercase();
        let mut search_from = 0;
        while let Some(pos) = lower[search_from..].find("credits:") {
            let after_label = search_from + pos + "credits:".len();
            let rest = &text[after_label..];
            let trimmed = rest.trim_start();
            let bytes = trimmed.as_bytes();
            if let Some(&first) = bytes.first()
                && first.is_ascii_digit()
            {
                let len = bytes
                    .iter()
                    .take_while(|&&b| b.is_ascii_digit() || b == b'.' || b == b',')
                    .count();
                if let Some(val) = Self::parse_number(&trimmed[..len]) {
                    return Some(val);
                }
            }
            search_from = after_label;
        }
        None
    }

    #[must_use]
    pub fn first_line_starting_with(prefix: &str, text: &str) -> Option<String> {
        let lower_text = text.to_ascii_lowercase();
        let lower_prefix = prefix.to_ascii_lowercase();
        let pos = lower_text.find(&lower_prefix)?;
        let rest = &text[pos..];
        let line_end = rest.find('\n').unwrap_or(rest.len());
        Some(rest[..line_end].trim_end_matches('\r').to_owned())
    }

    #[must_use]
    pub fn percent_left(line: &str) -> Option<i64> {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i].is_ascii_digit() {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let digit_len = i - start;
                if (1..=3).contains(&digit_len) && i < bytes.len() && bytes[i] == b'%' {
                    let after_pct = &line[i + 1..];
                    let ws_len = after_pct.len() - after_pct.trim_start().len();
                    if ws_len >= 1 {
                        let after_ws = after_pct.trim_start();
                        if after_ws
                            .get(..4)
                            .is_some_and(|s| s.eq_ignore_ascii_case("left"))
                        {
                            return Self::parse_number(&line[start..i]).map(|v| v as i64);
                        }
                    }
                }
            } else {
                i += 1;
            }
        }
        None
    }

    #[must_use]
    pub fn reset_string(line: &str) -> Option<String> {
        let lower = line.to_ascii_lowercase();
        let mut search_from = 0;
        while let Some(pos) = lower[search_from..].find("reset") {
            let idx = search_from + pos;
            let after_reset = &line[idx + "reset".len()..];
            let after_kw = if after_reset
                .as_bytes()
                .first()
                .is_some_and(|b| b.eq_ignore_ascii_case(&b's'))
            {
                &after_reset[1..]
            } else {
                after_reset
            };
            if after_kw.chars().next().is_some_and(char::is_whitespace) {
                let tail = after_kw.trim();
                if !tail.is_empty() {
                    return Some(tail.to_owned());
                }
            }
            search_from = idx + "reset".len();
        }
        None
    }
}

/// Errors produced by [`CodexCLISession`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodexCLISessionError {
    #[error("Failed to launch Codex CLI session: {0}")]
    LaunchFailed(String),
    #[error("Codex CLI session timed out.")]
    TimedOut,
    #[error("Codex CLI session exited.")]
    ProcessExited,
    #[error("Codex CLI session produced more output than AriadUsage can safely process.")]
    OutputTooLarge,
    #[error("Codex CLI session was cancelled.")]
    Cancelled,
}

/// Capture options for [`CodexCLISession::capture_status`].
#[derive(Clone)]
pub struct CodexCLISessionCaptureOptions {
    pub timeout: Duration,
    pub rows: u16,
    pub cols: u16,
    pub environment: BTreeMap<String, String>,
    pub extra_args: Vec<String>,
    pub working_directory: Option<PathBuf>,
    pub initial_delay: Duration,
    pub settle_after_status: Duration,
    pub output_cap: usize,
}

impl CodexCLISessionCaptureOptions {
    #[must_use]
    pub fn new(
        timeout: Duration,
        rows: u16,
        cols: u16,
        environment: BTreeMap<String, String>,
        extra_args: Vec<String>,
        working_directory: Option<PathBuf>,
    ) -> Self {
        Self {
            timeout,
            rows,
            cols,
            environment,
            extra_args,
            working_directory,
            initial_delay: Duration::from_millis(400),
            settle_after_status: Duration::from_millis(250),
            output_cap: DEFAULT_OUTPUT_CAP,
        }
    }
}

impl fmt::Debug for CodexCLISessionCaptureOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexCLISessionCaptureOptions")
            .field("timeout", &self.timeout)
            .field("rows", &self.rows)
            .field("cols", &self.cols)
            .field(
                "environment",
                &format!(
                    "ProcessEnvironment({} entries; redacted)",
                    self.environment.len()
                ),
            )
            .field("extra_args", &self.extra_args)
            .field("working_directory", &self.working_directory)
            .finish()
    }
}

struct HeldLaunchLock(Arc<dyn SessionLaunchLock>);

impl Drop for HeldLaunchLock {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[derive(Clone, PartialEq, Eq)]
struct WarmSessionKey {
    binary_path: PathBuf,
    rows: u16,
    cols: u16,
    environment: BTreeMap<String, String>,
    arguments: Vec<String>,
    working_directory: Option<PathBuf>,
}

struct WarmSessionState {
    pty: Option<PtySession>,
    key: Option<WarmSessionKey>,
    started_at: Option<Instant>,
    record: Option<SessionRecord>,
}

impl WarmSessionState {
    fn new() -> Self {
        Self {
            pty: None,
            key: None,
            started_at: None,
            record: None,
        }
    }
}

/// Interactive warm PTY session manager for the Codex CLI (`/status`).
#[derive(Clone)]
pub struct CodexCLISession {
    state: Arc<Mutex<WarmSessionState>>,
    registry: Arc<ProcessRegistry>,
    gate: Arc<LaunchGate>,
    record_store: Option<Arc<dyn SessionRecordStore>>,
    identity_provider: Option<Arc<dyn SessionRecordIdentity>>,
    launch_lock: Option<Arc<dyn SessionLaunchLock>>,
}

impl fmt::Debug for CodexCLISession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexCLISession")
            .field("has_record_store", &self.record_store.is_some())
            .finish()
    }
}

impl CodexCLISession {
    #[must_use]
    pub fn shared() -> &'static Self {
        static SHARED: OnceLock<CodexCLISession> = OnceLock::new();
        SHARED.get_or_init(|| {
            Self::live(
                Arc::new(ProcessRegistry::new()),
                Arc::new(LaunchGate::default()),
            )
        })
    }

    #[must_use]
    pub fn live(registry: Arc<ProcessRegistry>, gate: Arc<LaunchGate>) -> Self {
        Self::new(registry, gate).with_state_directory(crate::paths::state_dir())
    }

    #[must_use]
    pub fn new(registry: Arc<ProcessRegistry>, gate: Arc<LaunchGate>) -> Self {
        Self {
            state: Arc::new(Mutex::new(WarmSessionState::new())),
            registry,
            gate,
            record_store: None,
            identity_provider: None,
            launch_lock: None,
        }
    }

    #[must_use]
    pub fn process_registry(&self) -> &Arc<ProcessRegistry> {
        &self.registry
    }

    #[must_use]
    pub fn launch_gate(&self) -> &Arc<LaunchGate> {
        &self.gate
    }

    #[must_use]
    pub fn with_process_registry(mut self, registry: Arc<ProcessRegistry>) -> Self {
        self.registry = registry;
        self
    }

    #[must_use]
    pub fn with_launch_gate(mut self, gate: Arc<LaunchGate>) -> Self {
        self.gate = gate;
        self
    }

    #[must_use]
    pub fn with_process_broker(
        mut self,
        registry: Arc<ProcessRegistry>,
        gate: Arc<LaunchGate>,
    ) -> Self {
        self.registry = registry;
        self.gate = gate;
        self
    }

    #[must_use]
    pub fn with_state_directory(
        self,
        state_directory: Result<PathBuf, crate::paths::PathError>,
    ) -> Self {
        let Ok(state_directory) = state_directory else {
            return self;
        };
        let state_root = state_directory.join("ariadusage").join("codex");
        self.with_session_coordination(
            Arc::new(FileSessionRecordStore::new(
                state_root.join("codex-session.json"),
            )),
            Arc::new(ProcSessionRecordIdentity),
            FileSessionLaunchLock::new(state_root.join("codex-session.lock")),
        )
    }

    #[must_use]
    pub fn with_session_coordination(
        mut self,
        record_store: Arc<dyn SessionRecordStore>,
        identity_provider: Arc<dyn SessionRecordIdentity>,
        launch_lock: Arc<dyn SessionLaunchLock>,
    ) -> Self {
        self.record_store = Some(record_store);
        self.identity_provider = Some(identity_provider);
        self.launch_lock = Some(launch_lock);
        self
    }

    /// Builds a sanitized [`ProcessEnv`] for Codex PTY execution:
    /// - Starts from [`ProcessEnv::from_allowlist`] (stripping `ARIADUSAGE_*`, `OPENAI_*`, and `CODEX_*` secrets)
    /// - Preserves non-secret paths (`CODEX_HOME`, `BUN_INSTALL`, `FNM_DIR`, `VOLTA_HOME`, `PNPM_HOME`, `TZ`)
    /// - Applies `TTYCommandRunner.enrichedEnvironment` defaults (`TERM`, `COLORTERM`, `LANG`, `CI`, `PWD`)
    #[must_use]
    pub fn launch_process_env(
        base_env: &BTreeMap<String, String>,
        working_directory: Option<&Path>,
    ) -> ProcessEnv {
        let mut env =
            ProcessEnv::from_allowlist(base_env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        for key in [
            "CODEX_HOME",
            "BUN_INSTALL",
            "FNM_DIR",
            "VOLTA_HOME",
            "PNPM_HOME",
            "TZ",
        ] {
            if let Some(val) = base_env.get(key) {
                env = env.with(key, val);
            }
        }
        if env.get("TERM").is_none_or(|v| v.is_empty()) {
            env = env.with("TERM", "xterm-256color");
        }
        if env.get("COLORTERM").is_none_or(|v| v.is_empty()) {
            env = env.with("COLORTERM", "truecolor");
        }
        if env.get("LANG").is_none_or(|v| v.is_empty()) {
            env = env.with("LANG", "en_US.UTF-8");
        }
        let ci = base_env.get("CI").map(String::as_str).unwrap_or("0");
        env = env.with("CI", ci);
        if let Some(cwd) = working_directory {
            env = env.with("PWD", cwd.as_os_str());
        }
        env
    }

    pub async fn pid(&self) -> Option<u32> {
        let state = self.state.lock().await;
        state
            .pty
            .as_ref()
            .filter(|pty| pty.is_running())
            .and_then(PtySession::pid)
    }

    pub async fn is_running(&self) -> bool {
        self.pid().await.is_some()
    }

    pub async fn reset(&self) {
        let mut state = self.state.lock().await;
        self.cleanup_locked(&mut state).await;
        drop(state);
        self.reap_recorded_sessions(None).await;
    }

    pub async fn capture_status(
        &self,
        binary: &Path,
        options: CodexCLISessionCaptureOptions,
    ) -> Result<String, CodexCLISessionError> {
        let call = BrokerCall {
            interaction: FetchInteraction::UserInitiated,
            cancel: CancellationToken::new(),
            request_id: "codex.pty.status".to_owned(),
        };
        self.capture_status_with_call(call, binary, options).await
    }

    pub async fn capture_status_with_call(
        &self,
        call: BrokerCall,
        binary: &Path,
        options: CodexCLISessionCaptureOptions,
    ) -> Result<String, CodexCLISessionError> {
        if call.cancel.is_cancelled() {
            return Err(CodexCLISessionError::Cancelled);
        }
        let mut state = tokio::select! {
            biased;
            () = call.cancel.cancelled() => return Err(CodexCLISessionError::Cancelled),
            guard = self.state.lock() => guard,
        };
        if call.cancel.is_cancelled() {
            return Err(CodexCLISessionError::Cancelled);
        }

        self.ensure_started_locked(&mut state, call.clone(), binary, &options)
            .await?;

        if let Some(started_at) = state.started_at {
            let elapsed = started_at.elapsed();
            if elapsed < options.initial_delay {
                let remaining = options.initial_delay - elapsed;
                tokio::select! {
                    biased;
                    () = call.cancel.cancelled() => {
                        self.cleanup_locked(&mut state).await;
                        return Err(CodexCLISessionError::Cancelled);
                    }
                    () = tokio::time::sleep(remaining) => {}
                }
            }
        }

        if let Some(pty) = state.pty.as_mut() {
            let _ = pty.drain_output().await;
        }

        let result = run_codex_status_loop(
            state
                .pty
                .as_mut()
                .ok_or(CodexCLISessionError::ProcessExited)?,
            &call.cancel,
            options.timeout,
            options.settle_after_status,
            options.output_cap,
            true,
        )
        .await;

        if matches!(
            result,
            Err(CodexCLISessionError::OutputTooLarge
                | CodexCLISessionError::ProcessExited
                | CodexCLISessionError::Cancelled)
        ) {
            self.cleanup_locked(&mut state).await;
        }

        result
    }

    async fn ensure_started_locked(
        &self,
        state: &mut WarmSessionState,
        call: BrokerCall,
        binary: &Path,
        options: &CodexCLISessionCaptureOptions,
    ) -> Result<(), CodexCLISessionError> {
        let key = WarmSessionKey {
            binary_path: binary.to_path_buf(),
            rows: options.rows,
            cols: options.cols,
            environment: options.environment.clone(),
            arguments: options.extra_args.clone(),
            working_directory: options.working_directory.clone(),
        };

        let current_pid = state
            .pty
            .as_ref()
            .filter(|pty| pty.is_running())
            .and_then(PtySession::pid);
        self.reap_recorded_sessions(current_pid).await;

        if let Some(pty) = state.pty.as_ref()
            && pty.is_running()
            && state.key.as_ref() == Some(&key)
        {
            return Ok(());
        }

        self.cleanup_locked(state).await;

        let program = AbsolutePath::new(binary)
            .map_err(|e| CodexCLISessionError::LaunchFailed(e.to_string()))?;
        let args: Vec<OsString> = options.extra_args.iter().map(OsString::from).collect();
        let env =
            Self::launch_process_env(&options.environment, options.working_directory.as_deref());
        let mut command = Command::new(program, args)
            .with_env(env)
            .timeout(options.timeout);
        command.set_cwd(options.working_directory.clone());
        command.set_reap_marker(true);

        let size = PtySize::new(options.rows, options.cols);
        let pty = PtySession::spawn(call, command, size, &self.registry, &self.gate)
            .await
            .map_err(|e| match e {
                PtyError::Cancelled => CodexCLISessionError::Cancelled,
                other => CodexCLISessionError::LaunchFailed(other.to_string()),
            })?;

        let record = self.persist_record(&pty, binary).await;
        state.pty = Some(pty);
        state.key = Some(key);
        state.started_at = Some(Instant::now());
        state.record = record;
        Ok(())
    }

    async fn cleanup_locked(&self, state: &mut WarmSessionState) {
        if let Some(mut pty) = state.pty.take() {
            pty.close().await;
        }
        if let Some(record) = state.record.take() {
            self.remove_record(&record).await;
        }
        state.key = None;
        state.started_at = None;
    }

    async fn persist_record(&self, pty: &PtySession, binary: &Path) -> Option<SessionRecord> {
        let (Some(store), Some(identity), Some(lock)) = (
            self.record_store.clone(),
            self.identity_provider.clone(),
            self.launch_lock.clone(),
        ) else {
            return None;
        };
        let pid = pty.pid()?;
        let (executable_path, start_epoch) = identity.identity(pid).or_else(|| {
            pty.process_identity()
                .map(|(path, ticks)| (path.to_string_lossy().into_owned(), ticks as f64))
        })?;
        let owner = identity.current_identity();
        let record = SessionRecord {
            pid,
            requested_binary_path: binary.to_path_buf(),
            executable_path,
            start_epoch,
            process_group: pty.process_group(),
            owner_pid: Some(identity.current_pid()),
            owner_executable_path: owner.as_ref().map(|(path, _)| path.clone()),
            owner_start_epoch: owner.map(|(_, start)| start),
            marker: pty.marker(),
        };
        if lock.acquire().await.is_ok() {
            let _held = HeldLaunchLock(lock);
            let _ = store.save(&record);
            return Some(record);
        }
        None
    }

    async fn remove_record(&self, record: &SessionRecord) {
        let (Some(store), Some(lock)) = (self.record_store.clone(), self.launch_lock.clone())
        else {
            return;
        };
        if lock.acquire().await.is_ok() {
            let _held = HeldLaunchLock(lock);
            let _ = store.remove(record);
        }
    }

    async fn reap_recorded_sessions(&self, owned_pid: Option<u32>) {
        let (Some(store), Some(identity), Some(lock)) = (
            self.record_store.clone(),
            self.identity_provider.clone(),
            self.launch_lock.clone(),
        ) else {
            return;
        };
        if lock.acquire().await.is_err() {
            return;
        }
        let _held = HeldLaunchLock(lock);
        let records = store.load().unwrap_or_default();
        for record in records {
            if owned_pid == Some(record.pid) {
                continue;
            }
            let Some((live_path, live_start)) = identity.identity(record.pid) else {
                let _ = store.remove(&record);
                continue;
            };
            if live_path != record.executable_path
                || (live_start - record.start_epoch).abs() >= 0.001
            {
                let _ = store.remove(&record);
                continue;
            }
            let protected = match (
                record.owner_pid,
                record.owner_executable_path.as_deref(),
                record.owner_start_epoch,
            ) {
                (Some(owner_pid), Some(owner_path), Some(owner_start))
                    if owner_pid != identity.current_pid() =>
                {
                    identity.identity(owner_pid).is_some_and(|(path, start)| {
                        path == owner_path && (start - owner_start).abs() < 0.001
                    })
                }
                _ => false,
            };
            if protected {
                continue;
            }
            identity.terminate_record(record.clone()).await;
            let _ = store.remove(&record);
        }
    }
}

async fn run_codex_status_loop(
    pty: &mut PtySession,
    cancel: &CancellationToken,
    timeout: Duration,
    settle_after_status: Duration,
    output_cap: usize,
    fail_on_early_exit: bool,
) -> Result<String, CodexCLISessionError> {
    let script = b"/status";
    let cursor_query = b"\x1b[6n";
    let mut status_scan_buffer =
        StreamScanBuffer::new(cursor_query.len().max(CodexStatusMarkers::LONGEST_STATUS));
    let mut update_scan_buffer = StreamScanBuffer::new(CodexStatusMarkers::LONGEST_UPDATE_PROMPT);
    let mut buffer = BoundedOutputBuffer::new(output_cap);

    let deadline = Instant::now() + timeout;
    let mut next_cursor_check_at = Instant::now();

    let mut skipped_codex_update = false;
    let mut sent_script = false;
    let mut update_skip_attempts = 0usize;
    let mut last_enter = Instant::now();
    let mut script_sent_at: Option<Instant> = None;
    let mut resend_status_retries = 0usize;
    let mut enter_retries = 0usize;
    let mut saw_codex_status = false;
    let mut saw_codex_update_prompt = false;
    let mut closed = false;
    let mut chunk = [0u8; 8192];

    while Instant::now() < deadline {
        if cancel.is_cancelled() {
            return Err(CodexCLISessionError::Cancelled);
        }

        let mut new_data = Vec::new();
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(60));
        match pty.read_once(&mut chunk, wait).await {
            Ok(DrainClassification::Data(mut n)) => loop {
                new_data.extend_from_slice(&chunk[..n]);
                match pty.read_once(&mut chunk, Duration::from_millis(5)).await {
                    Ok(DrainClassification::Data(next_n)) if next_n > 0 => {
                        n = next_n;
                    }
                    Ok(DrainClassification::Closed) => {
                        closed = true;
                        break;
                    }
                    _ => break,
                }
            },
            Ok(DrainClassification::WouldBlock) => {}
            Ok(DrainClassification::Closed) => {
                closed = true;
            }
            Err(PtyError::Cancelled) => return Err(CodexCLISessionError::Cancelled),
            Err(_) => {
                closed = true;
            }
        }

        if !new_data.is_empty() && !buffer.append(&new_data) {
            return Err(CodexCLISessionError::OutputTooLarge);
        }

        let scan_data = status_scan_buffer.append(&new_data);
        if Instant::now() >= next_cursor_check_at
            && !scan_data.is_empty()
            && scan_data
                .windows(cursor_query.len())
                .any(|w| w == cursor_query)
        {
            let _ = pty.write(b"\x1b[1;1R").await;
            next_cursor_check_at = Instant::now() + Duration::from_secs(1);
        }
        if !scan_data.is_empty()
            && !saw_codex_status
            && CodexStatusMarkers::STATUS
                .iter()
                .any(|needle| scan_data.windows(needle.len()).any(|w| w == *needle))
        {
            saw_codex_status = true;
        }

        if !skipped_codex_update && !saw_codex_update_prompt && !new_data.is_empty() {
            let lower_data = StreamScanBuffer::lowercased_ascii(&new_data);
            let lower_scan = update_scan_buffer.append(&lower_data);
            if CodexStatusMarkers::UPDATE_PROMPT
                .iter()
                .any(|needle| lower_scan.windows(needle.len()).any(|w| w == *needle))
            {
                saw_codex_update_prompt = true;
            }
        }

        if !skipped_codex_update && saw_codex_update_prompt {
            let _ = pty.write(b"\x1b[B").await;
            tokio::time::sleep(Duration::from_millis(120)).await;
            let _ = pty.write(b"\r").await;
            tokio::time::sleep(Duration::from_millis(150)).await;
            let _ = pty.write(b"\r").await;
            let _ = pty.write(script).await;
            let _ = pty.write(b"\r").await;
            update_skip_attempts += 1;
            if update_skip_attempts >= 1 {
                skipped_codex_update = true;
                sent_script = false;
                script_sent_at = None;
                buffer = BoundedOutputBuffer::new(output_cap);
                status_scan_buffer.reset();
                update_scan_buffer.reset();
                saw_codex_status = false;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }

        if !sent_script && (!saw_codex_update_prompt || skipped_codex_update) {
            let _ = pty.write(script).await;
            let _ = pty.write(b"\r").await;
            sent_script = true;
            let now = Instant::now();
            script_sent_at = Some(now);
            last_enter = now;
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }

        if sent_script && !saw_codex_status {
            if last_enter.elapsed() >= Duration::from_millis(1200) && enter_retries < 6 {
                let _ = pty.write(b"\r").await;
                enter_retries += 1;
                last_enter = Instant::now();
                tokio::time::sleep(Duration::from_millis(120)).await;
                continue;
            }
            if script_sent_at.is_some_and(|sent| sent.elapsed() >= Duration::from_secs(3))
                && resend_status_retries < 2
            {
                let _ = pty.write(script).await;
                let _ = pty.write(b"\r").await;
                resend_status_retries += 1;
                buffer = BoundedOutputBuffer::new(output_cap);
                status_scan_buffer.reset();
                update_scan_buffer.reset();
                saw_codex_status = false;
                let now = Instant::now();
                script_sent_at = Some(now);
                last_enter = now;
                tokio::time::sleep(Duration::from_millis(220)).await;
                continue;
            }
        }

        if saw_codex_status {
            break;
        }

        if pty.has_exited() || closed {
            if let Ok(drained) = pty.drain_output().await
                && !drained.is_empty()
            {
                if !buffer.append(&drained) {
                    return Err(CodexCLISessionError::OutputTooLarge);
                }
                let scan_data = status_scan_buffer.append(&drained);
                if CodexStatusMarkers::STATUS
                    .iter()
                    .any(|needle| scan_data.windows(needle.len()).any(|w| w == *needle))
                {
                    saw_codex_status = true;
                    break;
                }
            }
            if fail_on_early_exit && buffer.as_slice().is_empty() {
                return Err(CodexCLISessionError::ProcessExited);
            }
            break;
        }

        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    if saw_codex_status && !settle_after_status.is_zero() {
        let settle_deadline = Instant::now() + settle_after_status;
        while Instant::now() < settle_deadline {
            if cancel.is_cancelled() {
                return Err(CodexCLISessionError::Cancelled);
            }
            let wait = settle_deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(50));
            match pty.read_once(&mut chunk, wait).await {
                Ok(DrainClassification::Data(n)) if n > 0 => {
                    if !buffer.append(&chunk[..n]) {
                        return Err(CodexCLISessionError::OutputTooLarge);
                    }
                    let scan_data = status_scan_buffer.append(&chunk[..n]);
                    if Instant::now() >= next_cursor_check_at
                        && !scan_data.is_empty()
                        && scan_data
                            .windows(cursor_query.len())
                            .any(|w| w == cursor_query)
                    {
                        let _ = pty.write(b"\x1b[1;1R").await;
                        next_cursor_check_at = Instant::now() + Duration::from_secs(1);
                    }
                }
                Ok(DrainClassification::Closed) => break,
                _ => {
                    if pty.has_exited() {
                        break;
                    }
                }
            }
        }
    }

    let bytes = buffer.into_bytes();
    if bytes.is_empty() {
        return Err(CodexCLISessionError::TimedOut);
    }
    let text = String::from_utf8(bytes.to_vec()).map_err(|_| CodexCLISessionError::TimedOut)?;
    if text.is_empty() {
        return Err(CodexCLISessionError::TimedOut);
    }
    Ok(text)
}

/// Runs `codex` inside a PTY, sends `/status`, captures text, and parses credits/limits.
#[derive(Clone)]
pub struct CodexStatusProbe {
    pub codex_binary: String,
    pub resolved_executable: Option<AbsolutePath>,
    pub timeout: Duration,
    pub parse_retry_timeout: Duration,
    pub keep_cli_sessions_alive: bool,
    pub environment: BTreeMap<String, String>,
    pub home_dir: Option<PathBuf>,
    pub runtime_dir: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
    pub session: CodexCLISession,
    pub process_registry: Arc<ProcessRegistry>,
    pub core_launch_gate: Arc<LaunchGate>,
    pub cli_launch_gate: Arc<CodexCLILaunchGate>,
    pub initial_delay: Duration,
    pub settle_after_status: Duration,
    pub output_cap: usize,
}

impl fmt::Debug for CodexStatusProbe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexStatusProbe")
            .field("codex_binary", &self.codex_binary)
            .field("timeout", &self.timeout)
            .field("keep_cli_sessions_alive", &self.keep_cli_sessions_alive)
            .field(
                "environment",
                &format!(
                    "ProcessEnvironment({} entries; redacted)",
                    self.environment.len()
                ),
            )
            .finish()
    }
}

impl Default for CodexStatusProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexStatusProbe {
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(8);
    pub const PARSE_RETRY_TIMEOUT: Duration = Duration::from_secs(4);

    #[must_use]
    pub fn new() -> Self {
        let process_registry = Arc::new(ProcessRegistry::new());
        let core_launch_gate = Arc::new(LaunchGate::default());
        Self {
            codex_binary: "codex".to_owned(),
            resolved_executable: None,
            timeout: Self::DEFAULT_TIMEOUT,
            parse_retry_timeout: Self::PARSE_RETRY_TIMEOUT,
            keep_cli_sessions_alive: false,
            environment: BTreeMap::new(),
            home_dir: None,
            runtime_dir: None,
            state_dir: None,
            session: CodexCLISession::live(
                Arc::clone(&process_registry),
                Arc::clone(&core_launch_gate),
            ),
            process_registry,
            core_launch_gate,
            cli_launch_gate: Arc::new(CodexCLILaunchGate::new()),
            initial_delay: Duration::from_millis(400),
            settle_after_status: Duration::from_millis(250),
            output_cap: DEFAULT_OUTPUT_CAP,
        }
    }

    #[must_use]
    pub fn with_environment(mut self, environment: BTreeMap<String, String>) -> Self {
        self.environment = environment;
        self
    }

    #[must_use]
    pub fn with_home_dir(mut self, home_dir: Option<PathBuf>) -> Self {
        self.home_dir = home_dir;
        self
    }

    #[must_use]
    pub fn with_home_directory(mut self, home_dir: Option<PathBuf>) -> Self {
        self.home_dir = home_dir;
        self
    }

    #[must_use]
    pub fn with_runtime_directory(mut self, runtime_directory: impl AsRef<Path>) -> Self {
        self.runtime_dir = Some(runtime_directory.as_ref().to_path_buf());
        self
    }

    #[must_use]
    pub fn with_state_directory(mut self, state_directory: impl AsRef<Path>) -> Self {
        let dir = state_directory.as_ref().to_path_buf();
        self.state_dir = Some(dir.clone());
        self.session = self.session.with_state_directory(Ok(dir));
        self
    }

    #[must_use]
    pub fn with_codex_binary(mut self, binary: impl Into<String>) -> Self {
        self.codex_binary = binary.into();
        self
    }

    #[must_use]
    pub fn with_executable(mut self, executable: AbsolutePath) -> Self {
        self.resolved_executable = Some(executable);
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_parse_retry_timeout(mut self, retry_timeout: Duration) -> Self {
        self.parse_retry_timeout = retry_timeout;
        self
    }

    #[must_use]
    pub fn with_keep_cli_sessions_alive(mut self, keep_alive: bool) -> Self {
        self.keep_cli_sessions_alive = keep_alive;
        self
    }

    #[must_use]
    pub fn with_session(mut self, session: CodexCLISession) -> Self {
        self.process_registry = Arc::clone(session.process_registry());
        self.core_launch_gate = Arc::clone(session.launch_gate());
        self.session = session;
        self
    }

    #[must_use]
    pub fn with_cli_launch_gate(mut self, gate: Arc<CodexCLILaunchGate>) -> Self {
        self.cli_launch_gate = gate;
        self
    }

    #[must_use]
    pub fn with_launch_gate(mut self, gate: Arc<CodexCLILaunchGate>) -> Self {
        self.cli_launch_gate = gate;
        self
    }

    #[must_use]
    pub fn with_process_registry(mut self, registry: Arc<ProcessRegistry>) -> Self {
        self.session = self.session.with_process_registry(Arc::clone(&registry));
        self.process_registry = registry;
        self
    }

    #[must_use]
    pub fn with_core_launch_gate(mut self, gate: Arc<LaunchGate>) -> Self {
        self.session = self.session.with_launch_gate(Arc::clone(&gate));
        self.core_launch_gate = gate;
        self
    }

    #[must_use]
    pub fn with_process_broker(
        mut self,
        registry: Arc<ProcessRegistry>,
        gate: Arc<LaunchGate>,
    ) -> Self {
        self.session = self
            .session
            .with_process_broker(Arc::clone(&registry), Arc::clone(&gate));
        self.process_registry = registry;
        self.core_launch_gate = gate;
        self
    }

    #[must_use]
    pub fn with_timing(mut self, initial_delay: Duration, settle_after_status: Duration) -> Self {
        self.initial_delay = initial_delay;
        self.settle_after_status = settle_after_status;
        self
    }

    #[must_use]
    pub fn with_output_cap(mut self, output_cap: usize) -> Self {
        self.output_cap = output_cap;
        self
    }

    pub fn resolve_binary(&self) -> Option<AbsolutePath> {
        if let Some(exec) = &self.resolved_executable {
            return Some(exec.clone());
        }
        let proc_env =
            ProcessEnv::from_iter(self.environment.iter().map(|(k, v)| (k.clone(), v.clone())));
        let home = self
            .home_dir
            .clone()
            .or_else(|| self.environment.get("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/nonexistent"));
        let inputs = ResolverInputs::new(&proc_env, &home, &RealResolverFs);
        resolve_codex_for_rpc(&inputs, &self.codex_binary, || None)
            .ok()
            .map(|r| r.executable)
    }

    pub async fn fetch(
        &self,
        now: Timestamp,
    ) -> Result<CodexStatusSnapshot, CodexStatusProbeError> {
        let call = BrokerCall {
            interaction: FetchInteraction::UserInitiated,
            cancel: CancellationToken::new(),
            request_id: "codex.pty.status".to_owned(),
        };
        self.fetch_with_call(now, call).await
    }

    pub async fn fetch_with_call(
        &self,
        now: Timestamp,
        call: BrokerCall,
    ) -> Result<CodexStatusSnapshot, CodexStatusProbeError> {
        let Some(resolved) = self.resolve_binary() else {
            return Err(CodexStatusProbeError::CodexNotInstalled);
        };
        let binary_str = resolved.as_path().to_string_lossy().to_string();
        if let Some(message) =
            self.cli_launch_gate
                .background_skip_message(&binary_str, now, call.interaction)
        {
            return Err(CodexStatusProbeError::LaunchBlocked(message));
        }

        match self
            .run_and_parse(call.clone(), &resolved, 60, 200, self.timeout, now)
            .await
        {
            Ok(snapshot) => Ok(snapshot),
            Err(CodexStatusProbeError::ParseFailed(_)) => {
                self.run_and_parse(call, &resolved, 70, 220, self.parse_retry_timeout, now)
                    .await
            }
            Err(err) => Err(err),
        }
    }

    async fn run_and_parse(
        &self,
        call: BrokerCall,
        binary: &AbsolutePath,
        rows: u16,
        cols: u16,
        timeout: Duration,
        now: Timestamp,
    ) -> Result<CodexStatusSnapshot, CodexStatusProbeError> {
        let state_home = match (&self.runtime_dir, &self.state_dir) {
            (rt, Some(state_dir)) => {
                CodexStatusProbeIsolation::resolve_support_directory(rt.as_deref(), Some(state_dir))
            }
            (Some(rt), None) => CodexStatusProbeIsolation::support_directory_with_home(
                &self.environment,
                self.home_dir.as_deref(),
                Some(rt.as_path()),
            ),
            (None, None) => {
                if self.home_dir.is_some()
                    || self.environment.contains_key("HOME")
                    || self.environment.contains_key("XDG_RUNTIME_DIR")
                    || self.environment.contains_key("XDG_STATE_HOME")
                {
                    CodexStatusProbeIsolation::support_directory_with_home(
                        &self.environment,
                        self.home_dir.as_deref(),
                        None,
                    )
                } else {
                    CodexStatusProbeIsolation::support_directory(&self.environment)
                }
            }
        }
        .map_err(|e| CodexStatusProbeError::ParseFailed(e.to_string()))?;
        let extra_args = CodexStatusProbeIsolation::codex_arguments(&state_home);
        let working_directory = CodexStatusProbeIsolation::working_directory(&self.environment);
        let binary_str = binary.as_path().to_string_lossy().to_string();

        let text = if self.keep_cli_sessions_alive {
            let mut options = CodexCLISessionCaptureOptions::new(
                timeout,
                rows,
                cols,
                self.environment.clone(),
                extra_args,
                working_directory,
            );
            options.initial_delay = self.initial_delay;
            options.settle_after_status = self.settle_after_status;
            options.output_cap = self.output_cap;

            let session = self.session.clone().with_process_broker(
                Arc::clone(&self.process_registry),
                Arc::clone(&self.core_launch_gate),
            );
            match session
                .capture_status_with_call(call, binary.as_path(), options)
                .await
            {
                Ok(text) => text,
                Err(
                    CodexCLISessionError::ProcessExited
                    | CodexCLISessionError::TimedOut
                    | CodexCLISessionError::Cancelled,
                ) => return Err(CodexStatusProbeError::TimedOut),
                Err(CodexCLISessionError::OutputTooLarge) => {
                    return Err(CodexStatusProbeError::ParseFailed(
                        "output too large".to_owned(),
                    ));
                }
                Err(CodexCLISessionError::LaunchFailed(message)) => {
                    if let Some(throttled) =
                        self.cli_launch_gate
                            .record_launch_failure(&binary_str, &message, now)
                    {
                        return Err(CodexStatusProbeError::LaunchBlocked(throttled));
                    }
                    return Err(CodexStatusProbeError::CodexNotInstalled);
                }
            }
        } else {
            match self
                .run_one_shot(
                    call,
                    binary,
                    rows,
                    cols,
                    timeout,
                    &extra_args,
                    working_directory,
                )
                .await
            {
                Ok(text) => text,
                Err(
                    CodexCLISessionError::ProcessExited
                    | CodexCLISessionError::TimedOut
                    | CodexCLISessionError::Cancelled,
                ) => return Err(CodexStatusProbeError::TimedOut),
                Err(CodexCLISessionError::OutputTooLarge) => {
                    return Err(CodexStatusProbeError::ParseFailed(
                        "output too large".to_owned(),
                    ));
                }
                Err(CodexCLISessionError::LaunchFailed(message)) => {
                    if let Some(throttled) =
                        self.cli_launch_gate
                            .record_launch_failure(&binary_str, &message, now)
                    {
                        return Err(CodexStatusProbeError::LaunchBlocked(throttled));
                    }
                    return Err(CodexStatusProbeError::CodexNotInstalled);
                }
            }
        };

        Self::parse(&text, now)
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_one_shot(
        &self,
        call: BrokerCall,
        binary: &AbsolutePath,
        rows: u16,
        cols: u16,
        timeout: Duration,
        extra_args: &[String],
        working_directory: Option<PathBuf>,
    ) -> Result<String, CodexCLISessionError> {
        let args: Vec<OsString> = extra_args.iter().map(OsString::from).collect();
        let env =
            CodexCLISession::launch_process_env(&self.environment, working_directory.as_deref());
        let mut command = Command::new(binary.clone(), args)
            .with_env(env)
            .timeout(timeout);
        command.set_cwd(working_directory);
        command.set_reap_marker(true);

        let mut pty = PtySession::spawn(
            call.clone(),
            command,
            PtySize::new(rows, cols),
            &self.process_registry,
            &self.core_launch_gate,
        )
        .await
        .map_err(|e| match e {
            PtyError::Cancelled => CodexCLISessionError::Cancelled,
            other => CodexCLISessionError::LaunchFailed(other.to_string()),
        })?;

        if !self.initial_delay.is_zero() {
            tokio::select! {
                biased;
                () = call.cancel.cancelled() => {
                    pty.close().await;
                    return Err(CodexCLISessionError::Cancelled);
                }
                () = tokio::time::sleep(self.initial_delay) => {}
            }
        }

        let result = run_codex_status_loop(
            &mut pty,
            &call.cancel,
            timeout,
            self.settle_after_status,
            self.output_cap,
            false,
        )
        .await;

        if matches!(result, Err(CodexCLISessionError::OutputTooLarge)) {
            pty.terminate().await;
        } else {
            pty.close().await;
        }
        result
    }

    pub fn parse(text: &str, now: Timestamp) -> Result<CodexStatusSnapshot, CodexStatusProbeError> {
        Self::parse_with_time_zone(text, now, &TimeZone::system())
    }

    pub fn parse_with_time_zone(
        text: &str,
        now: Timestamp,
        tz: &TimeZone,
    ) -> Result<CodexStatusSnapshot, CodexStatusProbeError> {
        let clean = TextParsing::strip_ansi_codes(text);
        if clean.is_empty() {
            return Err(CodexStatusProbeError::TimedOut);
        }
        if clean
            .to_ascii_lowercase()
            .contains("data not available yet")
        {
            return Err(CodexStatusProbeError::ParseFailed(
                "data not available yet".to_owned(),
            ));
        }
        if Self::contains_update_prompt(&clean) {
            return Err(CodexStatusProbeError::UpdateRequired(
                "Run `bun install -g @openai/codex` to continue (update prompt blocking /status)."
                    .to_owned(),
            ));
        }

        let credits = TextParsing::first_credits_number(&clean);
        let five_line = TextParsing::first_line_starting_with("5h limit", &clean)
            .or_else(|| TextParsing::first_line_starting_with("5-hour limit", &clean));
        let week_line = TextParsing::first_line_starting_with("Weekly limit", &clean);
        let five_pct = five_line.as_deref().and_then(TextParsing::percent_left);
        let week_pct = week_line.as_deref().and_then(TextParsing::percent_left);
        let five_reset = five_line.as_deref().and_then(TextParsing::reset_string);
        let week_reset = week_line.as_deref().and_then(TextParsing::reset_string);
        let monthly_limit = Self::parse_monthly_credit_limit(&clean, now, tz);

        if credits.is_none() && five_pct.is_none() && week_pct.is_none() && monthly_limit.is_none()
        {
            let prefix: String = clean.chars().take(400).collect();
            return Err(CodexStatusProbeError::ParseFailed(prefix));
        }

        Ok(CodexStatusSnapshot {
            credits,
            five_hour_percent_left: five_pct,
            weekly_percent_left: week_pct,
            five_hour_resets_at: Self::parse_reset_date(five_reset.as_deref(), now, tz),
            weekly_resets_at: Self::parse_reset_date(week_reset.as_deref(), now, tz),
            five_hour_reset_description: five_reset,
            weekly_reset_description: week_reset,
            codex_credit_limit: monthly_limit,
            raw_text: clean,
        })
    }

    #[must_use]
    pub fn parse_monthly_credit_limit(
        text: &str,
        now: Timestamp,
        tz: &TimeZone,
    ) -> Option<CodexCreditLimitSnapshot> {
        let monthly_line = TextParsing::first_line_starting_with("Monthly credit limit", text)?;
        let (used, limit) = parse_monthly_used_and_limit(text)?;
        if limit <= 0.0 {
            return None;
        }
        let remaining_percent = TextParsing::percent_left(&monthly_line)
            .map(|p| p as f64)
            .unwrap_or_else(|| (100.0 - (used / limit * 100.0)).clamp(0.0, 100.0));
        let reset_text = TextParsing::reset_string(&monthly_line);
        Some(CodexCreditLimitSnapshot::new(
            CodexCreditLimitSnapshot::DEFAULT_TITLE,
            used,
            limit,
            remaining_percent,
            Self::parse_reset_date(reset_text.as_deref(), now, tz),
            now,
        ))
    }

    #[must_use]
    pub fn parse_reset_date(
        text: Option<&str>,
        now: Timestamp,
        tz: &TimeZone,
    ) -> Option<Timestamp> {
        let raw = text?.trim();
        if raw.is_empty() {
            return None;
        }
        let trimmed_parens = raw.trim_matches(['(', ')']);
        let collapsed = trimmed_parens
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if collapsed.is_empty() {
            return None;
        }

        let now_zoned = now.to_zoned(tz.clone());
        let now_year = now_zoned.year();

        let parts: Vec<&str> = collapsed.split(' ').collect();
        if parts.len() == 4 && parts[1] == "on" {
            let (hour, minute) = parse_hh_mm(parts[0])?;
            if let (Some(day), Some(month)) =
                (parse_day_1_2(parts[2]), parse_month_abbrev(parts[3]))
            {
                return resolve_date_with_year_bump(now_year, month, day, hour, minute, now, tz);
            }
            if let (Some(month), Some(day)) =
                (parse_month_abbrev(parts[2]), parse_day_1_2(parts[3]))
            {
                return resolve_date_with_year_bump(now_year, month, day, hour, minute, now, tz);
            }
            return None;
        }

        if parts.len() == 1 {
            let (hour, minute) = parse_hh_mm(parts[0])?;
            let anchored = now_zoned
                .date()
                .at(hour, minute, 0, 0)
                .to_zoned(tz.clone())
                .ok()?;
            if anchored.timestamp() >= now {
                return Some(anchored.timestamp());
            }
            let next_day = now_zoned
                .date()
                .tomorrow()
                .ok()?
                .at(hour, minute, 0, 0)
                .to_zoned(tz.clone())
                .ok()?;
            return Some(next_day.timestamp());
        }

        None
    }

    #[must_use]
    pub fn contains_update_prompt(text: &str) -> bool {
        let lower = text.to_ascii_lowercase();
        lower.contains("update available") && lower.contains("codex")
    }
}

fn parse_monthly_used_and_limit(text: &str) -> Option<(f64, f64)> {
    let lower = text.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(pos) = lower[search_from..].find("credits used") {
        let cu_pos = search_from + pos;
        let before_cu = &text[..cu_pos];
        let trimmed_before_cu = before_cu.trim_end();
        if trimmed_before_cu.len() < before_cu.len() {
            let limit_end = trimmed_before_cu.len();
            let bytes = text.as_bytes();
            let mut after_of_pos = limit_end;
            while after_of_pos > 0 {
                let b = bytes[after_of_pos - 1];
                if b.is_ascii_digit() || matches!(b, b'.' | b',' | b' ') {
                    after_of_pos -= 1;
                } else {
                    break;
                }
            }
            let limit_span = &text[after_of_pos..limit_end];
            let limit_trimmed = limit_span.trim_start();
            if limit_trimmed.len() < limit_span.len()
                && limit_trimmed
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_digit)
                && after_of_pos >= 2
                && text[after_of_pos - 2..after_of_pos].eq_ignore_ascii_case("of")
                && let Some(limit) = TextParsing::parse_number(limit_trimmed)
            {
                let of_pos = after_of_pos - 2;
                let before_of = &text[..of_pos];
                let trimmed_before_of = before_of.trim_end();
                let mut used = 0.0;
                if trimmed_before_of.len() < before_of.len() {
                    let used_end = trimmed_before_of.len();
                    let mut used_start = used_end;
                    while used_start > 0 {
                        let b = bytes[used_start - 1];
                        if b.is_ascii_digit() || matches!(b, b'.' | b',' | b' ') {
                            used_start -= 1;
                        } else {
                            break;
                        }
                    }
                    let used_trimmed = text[used_start..used_end].trim_start();
                    if used_trimmed
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_digit)
                        && let Some(parsed_used) = TextParsing::parse_number(used_trimmed)
                    {
                        used = parsed_used;
                    }
                }
                return Some((used, limit));
            }
        }
        search_from = cu_pos + "credits used".len();
    }
    None
}

fn parse_hh_mm(s: &str) -> Option<(i8, i8)> {
    let (h_str, m_str) = s.split_once(':')?;
    if !(1..=2).contains(&h_str.len()) || !h_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if m_str.len() != 2 || !m_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hour = h_str.parse::<i8>().ok()?;
    let minute = m_str.parse::<i8>().ok()?;
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return None;
    }
    Some((hour, minute))
}

fn parse_day_1_2(s: &str) -> Option<i8> {
    if !(1..=2).contains(&s.len()) || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<i8>().ok()
}

fn parse_month_abbrev(month: &str) -> Option<i8> {
    match month.to_ascii_lowercase().as_str() {
        "jan" => Some(1),
        "feb" => Some(2),
        "mar" => Some(3),
        "apr" => Some(4),
        "may" => Some(5),
        "jun" => Some(6),
        "jul" => Some(7),
        "aug" => Some(8),
        "sep" => Some(9),
        "oct" => Some(10),
        "nov" => Some(11),
        "dec" => Some(12),
        _ => None,
    }
}

fn resolve_date_with_year_bump(
    year: i16,
    month: i8,
    day: i8,
    hour: i8,
    minute: i8,
    now: Timestamp,
    tz: &TimeZone,
) -> Option<Timestamp> {
    if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
        return None;
    }
    if let Ok(date) = Date::new(year, month, day) {
        let dt = DateTime::from_parts(date, jiff::civil::Time::new(hour, minute, 0, 0).ok()?);
        let zoned = dt.to_zoned(tz.clone()).ok()?;
        if zoned.timestamp() >= now {
            return Some(zoned.timestamp());
        }
    }
    let next_date = Date::new(year.checked_add(1)?, month, day).ok()?;
    let next_dt = DateTime::from_parts(next_date, jiff::civil::Time::new(hour, minute, 0, 0).ok()?);
    let next_zoned = next_dt.to_zoned(tz.clone()).ok()?;
    Some(next_zoned.timestamp())
}
