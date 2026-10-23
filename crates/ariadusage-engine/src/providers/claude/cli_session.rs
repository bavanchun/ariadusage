// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeCLISession.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeStatusProbe.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Interactive PTY session (`ClaudeCLISession`), status probe (`ClaudeStatusProbe`),
//! workspace trust dialog handling, dedicated probe directory management, and
//! direct CLI `/usage` fallback for the Claude `cli` strategy.

use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_protocol::secret::SecretString;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::account_profile::ClaudeAccountProfile;
use super::cli_gates::{ClaudeCLIRateLimitGate, ClaudeProbeSessionArtifactCleaner};
use super::cli_parser::{
    ClaudeCLIAccountIdentity, ClaudeStatusProbeError, ClaudeStatusProbeParser, ClaudeStatusSnapshot,
};
use super::cli_screen::ClaudeCLIScreen;
use super::config_paths::normalize_lexical;
use super::credentials::{CLAUDE_OAUTH_SCOPES_ENV_KEY, CLAUDE_OAUTH_TOKEN_ENV_KEY};
use crate::brokers::call::BrokerCall;
use crate::brokers::exec_resolver::{
    RealResolverFs, ResolverFs, ResolverInputs, Tool, resolve as resolve_executable,
};
use crate::brokers::oauth::{BoxFuture, DelegatedRefreshError, DelegatedRefresher};
use crate::brokers::process::{
    AbsolutePath, BoundedOutputBuffer, Command, DEFAULT_OUTPUT_CAP, DrainClassification,
    ProcessEnv, ProcessError, ProcessRegistry, PtyError, PtySession, PtySize, StdinSpec,
    StreamScanBuffer, run,
};

/// Errors produced by [`ClaudeCLISession`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaudeCLISessionError {
    #[error("Failed to launch Claude CLI session: {0}")]
    LaunchFailed(String),
    #[error("Claude CLI PTY I/O failed: {0}")]
    IoFailed(String),
    #[error("Claude CLI session timed out.")]
    TimedOut,
    #[error("Claude CLI session exited.")]
    ProcessExited,
    #[error("Claude CLI session produced more output than Ariadusage can safely process.")]
    OutputTooLarge,
    #[error("Claude CLI session was cancelled.")]
    Cancelled,
}

/// Returns the default dedicated probe working directory under `base_dir`
/// (`<base_dir>/ariadusage/ClaudeProbe`).
#[must_use]
pub fn default_dedicated_probe_working_directory(base_dir: &Path) -> PathBuf {
    base_dir.join("ariadusage").join("ClaudeProbe")
}

fn resolve_symlinks_allow_missing(path: &Path) -> PathBuf {
    let normalized = normalize_lexical(path);
    if let Ok(canon) = std::fs::canonicalize(&normalized) {
        return canon;
    }

    let mut suffix = Vec::new();
    let mut current = normalized.as_path();
    while let Some(name) = current.file_name() {
        if let Ok(meta) = std::fs::symlink_metadata(current)
            && meta.file_type().is_symlink()
            && let Ok(target) = std::fs::read_link(current)
        {
            let resolved_target = if target.is_absolute() {
                target
            } else {
                current
                    .parent()
                    .unwrap_or_else(|| Path::new("/"))
                    .join(target)
            };
            let mut base = resolve_symlinks_allow_missing(&resolved_target);
            for part in suffix.into_iter().rev() {
                base.push(part);
            }
            return base;
        }
        suffix.push(name.to_os_string());
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
        if let Ok(canon) = std::fs::canonicalize(current) {
            let mut result = canon;
            for part in suffix.into_iter().rev() {
                result.push(part);
            }
            return result;
        }
    }
    normalized
}

/// Checks whether `directory` is the unredirected `expected_dedicated_dir`.
///
/// Mirrors `ClaudeStatusProbe.isDedicatedProbeWorkingDirectory`: rejects paths that do not
/// match `expected_dedicated_dir` as well as paths where `expected_dedicated_dir` or its
/// parent directory is a symlink redirecting elsewhere.
#[must_use]
pub fn is_dedicated_probe_working_directory(
    directory: &Path,
    expected_dedicated_dir: &Path,
) -> bool {
    let expected = normalize_lexical(expected_dedicated_dir);
    let dir_norm = normalize_lexical(directory);
    if dir_norm != expected {
        return false;
    }

    if std::fs::symlink_metadata(&expected).is_ok_and(|m| m.file_type().is_symlink()) {
        return false;
    }
    let Some(parent) = expected.parent() else {
        return false;
    };
    if std::fs::symlink_metadata(parent).is_ok_and(|m| m.file_type().is_symlink()) {
        return false;
    }
    let Some(grandparent) = parent.parent() else {
        return false;
    };
    let (Some(parent_name), Some(expected_name)) = (parent.file_name(), expected.file_name())
    else {
        return false;
    };

    let unredirected = resolve_symlinks_allow_missing(grandparent)
        .join(parent_name)
        .join(expected_name);
    resolve_symlinks_allow_missing(&expected) == unredirected
}

/// Returns `expected_dedicated_dir` if it passes [`is_dedicated_probe_working_directory`]
/// and can be created on disk, or falls back to [`std::env::temp_dir`].
#[must_use]
pub fn probe_working_directory(expected_dedicated_dir: &Path) -> PathBuf {
    if !is_dedicated_probe_working_directory(expected_dedicated_dir, expected_dedicated_dir) {
        return std::env::temp_dir();
    }
    if std::fs::create_dir_all(expected_dedicated_dir).is_err() {
        return std::env::temp_dir();
    }
    expected_dedicated_dir.to_path_buf()
}

/// Prepares the dedicated probe working directory (writing `.claude/settings.local.json`)
/// or returns the temporary directory fallback when the dedicated directory is redirected
/// or unavailable.
#[must_use]
pub fn prepared_probe_working_directory(expected_dedicated_dir: &Path) -> PathBuf {
    let directory = probe_working_directory(expected_dedicated_dir);
    if !is_dedicated_probe_working_directory(&directory, expected_dedicated_dir) {
        return directory;
    }
    let _ = prepare_probe_working_directory(&directory);
    directory
}

/// Returns the isolated probe working directory, failing with [`ClaudeCLISessionError::IoFailed`]
/// if `expected_dedicated_dir` had to fall back to the shared temporary directory.
pub fn isolated_probe_working_directory(
    expected_dedicated_dir: &Path,
) -> Result<PathBuf, ClaudeCLISessionError> {
    let directory = prepared_probe_working_directory(expected_dedicated_dir);
    if !is_dedicated_probe_working_directory(&directory, expected_dedicated_dir) {
        return Err(ClaudeCLISessionError::IoFailed(
            "Cannot prepare Claude's isolated probe directory.".to_owned(),
        ));
    }
    Ok(directory)
}

/// Writes `.claude/settings.local.json` inside `directory` with
/// `"disableDeepLinkRegistration": "disable"`, preserving any existing valid JSON object keys.
pub fn prepare_probe_working_directory(directory: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let claude_dir = directory.join(".claude");
    std::fs::create_dir_all(&claude_dir)?;

    let settings_path = claude_dir.join("settings.local.json");
    let mut settings = read_settings_object(&settings_path).unwrap_or_default();
    settings.insert(
        "disableDeepLinkRegistration".to_owned(),
        serde_json::Value::String("disable".to_owned()),
    );

    let data = serde_json::to_vec_pretty(&serde_json::Value::Object(settings))
        .map_err(std::io::Error::other)?;
    let tmp_path = claude_dir.join(".settings.local.json.tmp");
    std::fs::write(&tmp_path, &data)?;
    std::fs::rename(&tmp_path, &settings_path)?;
    Ok(())
}

fn read_settings_object(path: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() {
        return Some(serde_json::Map::new());
    }
    match serde_json::from_slice::<serde_json::Value>(&bytes).ok()? {
        serde_json::Value::Object(map) => Some(map),
        _ => None,
    }
}

fn is_valid_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (idx, &b) in bytes.iter().enumerate() {
        if matches!(idx, 8 | 13 | 18 | 23) {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

fn generate_uuid_v4() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let pid = u64::from(std::process::id());
    let mut a = now_nanos ^ pid.rotate_left(17) ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    if let Ok(mut urandom) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let mut buf = [0u8; 16];
        if urandom.read_exact(&mut buf).is_ok() {
            buf[6] = (buf[6] & 0x0f) | 0x40;
            buf[8] = (buf[8] & 0x3f) | 0x80;
            return format!(
                "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
                buf[0],
                buf[1],
                buf[2],
                buf[3],
                buf[4],
                buf[5],
                buf[6],
                buf[7],
                buf[8],
                buf[9],
                buf[10],
                buf[11],
                buf[12],
                buf[13],
                buf[14],
                buf[15]
            );
        }
    }
    a ^= a >> 33;
    a = a.wrapping_mul(0xff51_afd7_ed55_8ccd);
    let b = seq.wrapping_mul(0xc4ce_b9fe_1a85_ec53).wrapping_add(pid);
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        a as u16 & 0x0fff,
        ((b >> 48) as u16 & 0x3fff) | 0x8000,
        b & 0xffff_ffff_ffff
    )
}

fn fallback_probe_session_id() -> &'static str {
    static FALLBACK: OnceLock<String> = OnceLock::new();
    FALLBACK.get_or_init(generate_uuid_v4)
}

/// Predicate over normalized visible screen text used to stop PTY capture early.
pub type ClaudeCLINormalizedStopPredicate = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Request parameters for [`ClaudeCLISession::capture`].
pub struct ClaudeCLICaptureRequest {
    pub subcommand: String,
    pub binary: PathBuf,
    pub account_scope: Option<String>,
    pub timeout: Duration,
    pub environment: BTreeMap<String, SecretString>,
    pub idle_timeout: Option<Duration>,
    pub stop_on_substrings: Vec<String>,
    pub stop_when_normalized: Option<ClaudeCLINormalizedStopPredicate>,
    pub settle_after_stop: Duration,
    pub send_enter_every: Option<Duration>,
}

impl ClaudeCLICaptureRequest {
    #[must_use]
    pub fn new(
        subcommand: impl Into<String>,
        binary: impl Into<PathBuf>,
        timeout: Duration,
        environment: BTreeMap<String, SecretString>,
    ) -> Self {
        Self {
            subcommand: subcommand.into(),
            binary: binary.into(),
            account_scope: None,
            timeout,
            environment,
            idle_timeout: Some(Duration::from_secs(3)),
            stop_on_substrings: Vec::new(),
            stop_when_normalized: None,
            settle_after_stop: Duration::from_millis(250),
            send_enter_every: None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct SessionIdentity {
    binary_path: PathBuf,
    account_scope: Option<String>,
    environment_digest: String,
}

fn compute_environment_digest(env: &BTreeMap<String, SecretString>) -> String {
    let mut parts: Vec<&[u8]> = Vec::with_capacity(env.len() * 2);
    for (k, v) in env {
        parts.push(k.as_bytes());
        parts.push(v.expose_secret().as_bytes());
    }
    ariadusage_core::digest::sha256_hex("ariadusage:claude-cli-session-env:v1", &parts)
}

struct SessionState {
    pty: Option<PtySession>,
    session_identity: Option<SessionIdentity>,
    started_at: Option<Instant>,
    launched_in_probe_directory: bool,
}

impl SessionState {
    fn new() -> Self {
        Self {
            pty: None,
            session_identity: None,
            started_at: None,
            launched_in_probe_directory: false,
        }
    }

    async fn cleanup(&mut self) {
        if let Some(mut pty) = self.pty.take() {
            if !pty.has_exited() {
                let _ = pty.write(b"/exit\r").await;
            }
            pty.close().await;
        }
        self.session_identity = None;
        self.started_at = None;
        self.launched_in_probe_directory = false;
    }
}

/// Interactive PTY session manager for the Claude CLI.
pub struct ClaudeCLISession {
    state: Mutex<SessionState>,
    working_directory_override: Option<PathBuf>,
    home_dir: PathBuf,
    dedicated_probe_dir: PathBuf,
    registry: Arc<ProcessRegistry>,
    gate: Arc<LaunchGate>,
    startup_ready_delay: Duration,
}

impl ClaudeCLISession {
    pub const PROBE_SETTINGS_JSON: &'static str =
        "{\"remoteControlAtStartup\":false,\"disableAllHooks\":true}";

    #[must_use]
    pub fn new(
        dedicated_probe_dir: PathBuf,
        registry: Arc<ProcessRegistry>,
        gate: Arc<LaunchGate>,
    ) -> Self {
        let home_dir = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        Self {
            state: Mutex::new(SessionState::new()),
            working_directory_override: None,
            home_dir,
            dedicated_probe_dir,
            registry,
            gate,
            startup_ready_delay: Duration::from_secs(2),
        }
    }

    #[must_use]
    pub fn with_home_directory(mut self, home_dir: PathBuf) -> Self {
        self.home_dir = home_dir;
        self
    }

    #[must_use]
    pub fn with_working_directory(mut self, working_directory: PathBuf) -> Self {
        self.working_directory_override = Some(working_directory);
        self
    }

    #[must_use]
    pub fn with_startup_ready_delay(mut self, delay: Duration) -> Self {
        self.startup_ready_delay = delay;
        self
    }

    #[must_use]
    pub fn dedicated_probe_dir(&self) -> &Path {
        &self.dedicated_probe_dir
    }

    #[must_use]
    pub fn probe_settings_arguments() -> Vec<String> {
        vec![
            "--settings".to_owned(),
            Self::PROBE_SETTINGS_JSON.to_owned(),
        ]
    }

    #[must_use]
    pub fn launch_arguments(session_id: &str) -> Vec<String> {
        let mut args = vec![
            "--allowed-tools".to_owned(),
            String::new(),
            "--strict-mcp-config".to_owned(),
        ];
        args.extend(Self::probe_settings_arguments());
        args.push("--session-id".to_owned());
        args.push(session_id.to_ascii_lowercase());
        args
    }

    #[must_use]
    pub fn direct_usage_arguments() -> Vec<String> {
        let mut args = vec!["--strict-mcp-config".to_owned()];
        args.extend(Self::probe_settings_arguments());
        args.push("/usage".to_owned());
        args
    }

    #[must_use]
    pub fn load_or_create_probe_session_id(directory: &Path) -> String {
        let file_path = directory.join(".ariadusage-session-id");
        if let Ok(raw) = std::fs::read_to_string(&file_path) {
            let trimmed = raw.trim();
            if is_valid_uuid(trimmed) {
                return trimmed.to_ascii_lowercase();
            }
        }

        let session_id = generate_uuid_v4().to_ascii_lowercase();
        if std::fs::create_dir_all(directory).is_err() {
            return fallback_probe_session_id().to_owned();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700));
        }

        if std::fs::write(&file_path, &session_id).is_err() {
            return fallback_probe_session_id().to_owned();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o600));
        }
        session_id
    }

    /// Cleans `base_env` for passive Claude CLI execution:
    /// - Removes `ARIADUSAGE_CLAUDE_OAUTH_TOKEN`, `ARIADUSAGE_CLAUDE_OAUTH_SCOPES`, and `ANTHROPIC_*`
    /// - Sets `DISABLE_AUTOUPDATER=1`
    #[must_use]
    pub fn launch_environment(
        base_env: &BTreeMap<String, SecretString>,
    ) -> BTreeMap<String, SecretString> {
        let mut env = base_env.clone();
        for key in [CLAUDE_OAUTH_TOKEN_ENV_KEY, CLAUDE_OAUTH_SCOPES_ENV_KEY] {
            env.remove(key);
        }
        let anthropic_keys: Vec<String> = env
            .keys()
            .filter(|k| k.starts_with("ANTHROPIC_"))
            .cloned()
            .collect();
        for key in anthropic_keys {
            env.remove(&key);
        }
        env.insert(
            "DISABLE_AUTOUPDATER".to_owned(),
            SecretString::new("1".to_owned()),
        );
        env
    }

    /// Builds a [`ProcessEnv`] from [`Self::launch_environment`].
    #[must_use]
    pub fn launch_process_env(
        base_env: &BTreeMap<String, SecretString>,
        working_dir: Option<&Path>,
    ) -> ProcessEnv {
        let cleaned = Self::launch_environment(base_env);
        let mut env = ProcessEnv::from_allowlist(
            cleaned.iter().map(|(k, v)| (k.as_str(), v.expose_secret())),
        );
        for (k, v) in &cleaned {
            if matches!(
                k.as_str(),
                "CLAUDE_CONFIG_DIR"
                    | "CLAUDE_SECURESTORAGE_CONFIG_DIR"
                    | "CLAUDE_CLI_PATH"
                    | "DISABLE_AUTOUPDATER"
                    | "TZ"
            ) {
                env = env.with(k.as_str(), v.expose_secret());
            }
        }
        env = env.with("DISABLE_AUTOUPDATER", "1");
        if let Some(cwd) = working_dir {
            env = env.with("PWD", cwd.as_os_str());
        }
        env
    }

    fn normalized_needle(text: &str) -> String {
        text.chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect()
    }

    #[must_use]
    pub fn command_palette_prompt(subcommand: &str) -> Option<&'static str> {
        if subcommand.eq_ignore_ascii_case("/usage") {
            Some("Show plan")
        } else if subcommand.eq_ignore_ascii_case("/status") {
            Some("Show Claude Code")
        } else {
            None
        }
    }

    /// Returns the key sequence for the workspace trust dialog on `screen`, or `None` when
    /// it is not shown.
    ///
    /// Mirrors `ClaudeCLISession.workspaceTrustKeys(onScreen:acceptsTrust:)`.
    #[must_use]
    pub fn workspace_trust_keys(screen: &str, accepts_trust: bool) -> Option<&'static str> {
        let normalized = Self::normalized_needle(screen);
        let lines: Vec<&str> = screen.split('\n').collect();
        let option = "yes,itrustthisfolder";
        let Some(option_row) = lines
            .iter()
            .position(|line| Self::normalized_needle(line).contains(option))
        else {
            if normalized.contains("quicksafetycheck:") {
                return if accepts_trust { None } else { Some("\x1b") };
            }
            if normalized.contains("doyoutrustthefilesinthisfolder?") {
                return Some(if accepts_trust { "y\r" } else { "\x1b" });
            }
            if normalized.contains("readytocodehere?") {
                return Some(if accepts_trust { "\r" } else { "\x1b" });
            }
            return None;
        };
        if !accepts_trust {
            return Some("\x1b");
        }

        let is_blank = |row: usize| lines[row].chars().all(char::is_whitespace);
        let mut first_row = option_row;
        while first_row > 0 && !is_blank(first_row - 1) {
            first_row -= 1;
        }
        let mut last_row = option_row;
        while last_row + 1 < lines.len() && !is_blank(last_row + 1) {
            last_row += 1;
        }
        let marker_row = (first_row..=last_row).find(|&row| lines[row].contains('❯'))?;
        if marker_row == option_row {
            return Some("\r");
        }
        Some(if marker_row < option_row {
            "\x1b[B"
        } else {
            "\x1b[A"
        })
    }

    pub async fn reset(&self) {
        let mut state = self.state.lock().await;
        state.cleanup().await;
    }

    pub async fn capture(
        &self,
        call: BrokerCall,
        request: ClaudeCLICaptureRequest,
    ) -> Result<String, ClaudeCLISessionError> {
        if call.cancel.is_cancelled() {
            return Err(ClaudeCLISessionError::Cancelled);
        }
        let mut state = tokio::select! {
            biased;
            () = call.cancel.cancelled() => return Err(ClaudeCLISessionError::Cancelled),
            guard = self.state.lock() => guard,
        };
        if call.cancel.is_cancelled() {
            return Err(ClaudeCLISessionError::Cancelled);
        }
        self.capture_exclusive(&mut state, call, request).await
    }

    async fn ensure_started(
        &self,
        state: &mut SessionState,
        call: BrokerCall,
        request: &ClaudeCLICaptureRequest,
    ) -> Result<bool, ClaudeCLISessionError> {
        let cleaned_env = Self::launch_environment(&request.environment);
        let identity = SessionIdentity {
            binary_path: request.binary.clone(),
            account_scope: request.account_scope.clone(),
            environment_digest: compute_environment_digest(&cleaned_env),
        };

        if let Some(pty) = state.pty.as_ref()
            && !pty.has_exited()
            && state.session_identity.as_ref() == Some(&identity)
        {
            return Ok(true);
        }

        state.cleanup().await;
        let working_directory = match &self.working_directory_override {
            Some(dir) => dir.clone(),
            None => isolated_probe_working_directory(&self.dedicated_probe_dir)?,
        };

        let home_dir = cleaned_env
            .get("HOME")
            .map(|h| PathBuf::from(h.expose_secret()))
            .unwrap_or_else(|| self.home_dir.clone());
        let _ = ClaudeProbeSessionArtifactCleaner::cleanup_probe_session_artifacts(
            &working_directory,
            &self.dedicated_probe_dir,
            &cleaned_env,
            &working_directory,
            &home_dir,
        );

        let session_id = Self::load_or_create_probe_session_id(&working_directory);
        let claude_args: Vec<std::ffi::OsString> = Self::launch_arguments(&session_id)
            .into_iter()
            .map(Into::into)
            .collect();
        let program = AbsolutePath::new(&request.binary)
            .map_err(|e| ClaudeCLISessionError::LaunchFailed(e.to_string()))?;
        let env = Self::launch_process_env(&request.environment, Some(&working_directory));
        let mut command = Command::new(program, claude_args)
            .with_env(env)
            .timeout(request.timeout);
        command.set_cwd(Some(working_directory.clone()));
        command.set_reap_marker(true);

        let size = PtySize::new(
            ClaudeCLIScreen::ROWS as u16,
            ClaudeCLIScreen::COLUMNS as u16,
        );
        let pty = PtySession::spawn(call, command, size, &self.registry, &self.gate)
            .await
            .map_err(|e| ClaudeCLISessionError::LaunchFailed(e.to_string()))?;

        state.pty = Some(pty);
        state.session_identity = Some(identity);
        state.started_at = Some(Instant::now());
        state.launched_in_probe_directory =
            is_dedicated_probe_working_directory(&working_directory, &self.dedicated_probe_dir);
        Ok(false)
    }

    async fn capture_exclusive(
        &self,
        state: &mut SessionState,
        call: BrokerCall,
        request: ClaudeCLICaptureRequest,
    ) -> Result<String, ClaudeCLISessionError> {
        let reused = self.ensure_started(state, call.clone(), &request).await?;
        if reused {
            if let Some(pty) = state.pty.as_mut() {
                pty.write(b"\x1b")
                    .await
                    .map_err(|e| ClaudeCLISessionError::IoFailed(e.to_string()))?;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }

        let mut ready_at = state
            .started_at
            .unwrap_or_else(Instant::now)
            .checked_add(self.startup_ready_delay)
            .unwrap_or_else(Instant::now);
        let mut command_sent = false;
        let trimmed = request.subcommand.trim().to_owned();
        let stop_needles: Vec<String> = request
            .stop_on_substrings
            .iter()
            .map(|s| Self::normalized_needle(s))
            .collect();
        let mut send_needles = vec![Self::normalized_needle("Press Enter to continue")];
        if let Some(palette_prompt) = Self::command_palette_prompt(&trimmed) {
            send_needles.push(Self::normalized_needle(palette_prompt));
        }
        let mut triggered_sends: HashSet<String> = HashSet::new();
        let cursor_query = b"\x1b[6n";
        let mut scan_buffer = StreamScanBuffer::new(cursor_query.len());
        let mut buffer = BoundedOutputBuffer::new(DEFAULT_OUTPUT_CAP);

        let now = Instant::now();
        let mut deadline = now.max(ready_at) + request.timeout;
        let mut last_output_at = Instant::now();
        let mut last_enter_at = Instant::now();
        let mut unchecked_frame = false;
        let mut trust_pending = false;
        let mut trust_keys_left: usize = 4;
        let mut stopped_early = false;
        let mut closed = false;
        let mut chunk = [0u8; 8192];

        while Instant::now() < deadline {
            if call.cancel.is_cancelled() {
                state.cleanup().await;
                return Err(ClaudeCLISessionError::Cancelled);
            }

            let wait = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(60));
            let pty = state
                .pty
                .as_mut()
                .ok_or(ClaudeCLISessionError::ProcessExited)?;

            match pty.read_once(&mut chunk, wait).await {
                Ok(DrainClassification::Data(mut n)) => loop {
                    if !buffer.append(&chunk[..n]) {
                        state.cleanup().await;
                        return Err(ClaudeCLISessionError::OutputTooLarge);
                    }
                    last_output_at = Instant::now();
                    unchecked_frame = true;
                    if scan_buffer
                        .append(&chunk[..n])
                        .windows(cursor_query.len())
                        .any(|w| w == cursor_query)
                    {
                        let _ = pty.write(b"\x1b[1;1R").await;
                    }
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
                Err(PtyError::Cancelled) => {
                    state.cleanup().await;
                    return Err(ClaudeCLISessionError::Cancelled);
                }
                Err(e) => {
                    state.cleanup().await;
                    return Err(ClaudeCLISessionError::IoFailed(e.to_string()));
                }
            }

            let process_exited = pty.has_exited() || closed;

            if unchecked_frame
                && (last_output_at.elapsed() >= Duration::from_millis(200) || process_exited)
                && let Ok(text) = std::str::from_utf8(buffer.as_slice())
            {
                unchecked_frame = false;
                let screen = ClaudeCLIScreen::render(text);
                let normalized = Self::normalized_needle(&screen);
                let trust_keys =
                    Self::workspace_trust_keys(&screen, state.launched_in_probe_directory);
                trust_pending = trust_keys.is_some()
                    || normalized.contains("quicksafetycheck:")
                    || normalized.contains("doyoutrustthefilesinthisfolder?")
                    || normalized.contains("readytocodehere?");
                if trust_pending {
                    if !process_exited
                        && trust_keys_left > 0
                        && let Some(keys) = trust_keys
                    {
                        pty.write(keys.as_bytes())
                            .await
                            .map_err(|e| ClaudeCLISessionError::IoFailed(e.to_string()))?;
                        trust_keys_left = if keys == "\x1b" {
                            0
                        } else {
                            trust_keys_left - 1
                        };
                        ready_at = Instant::now() + self.startup_ready_delay;
                    }
                } else {
                    if command_sent
                        && (stop_needles.iter().any(|n| normalized.contains(n))
                            || request
                                .stop_when_normalized
                                .as_ref()
                                .is_some_and(|pred| pred(&normalized)))
                    {
                        stopped_early = true;
                        break;
                    }
                    for needle in &send_needles {
                        if !triggered_sends.contains(needle)
                            && !process_exited
                            && normalized.contains(needle)
                        {
                            let _ = pty.write(b"\r").await;
                            triggered_sends.insert(needle.clone());
                            if !command_sent {
                                ready_at = Instant::now() + self.startup_ready_delay;
                            }
                        }
                    }
                }
            }

            if !command_sent
                && !trust_pending
                && !unchecked_frame
                && Instant::now() >= ready_at
                && !process_exited
            {
                buffer = BoundedOutputBuffer::new(DEFAULT_OUTPUT_CAP);
                if !trimmed.is_empty() {
                    let mut cmd_bytes = trimmed.as_bytes().to_vec();
                    cmd_bytes.push(b'\r');
                    pty.write(&cmd_bytes)
                        .await
                        .map_err(|e| ClaudeCLISessionError::IoFailed(e.to_string()))?;
                }
                command_sent = true;
                last_output_at = Instant::now();
                last_enter_at = Instant::now();
                deadline = Instant::now() + request.timeout;
            }

            if command_sent && !trust_pending && !unchecked_frame {
                if let Some(idle_timeout) = request.idle_timeout
                    && !buffer.as_slice().is_empty()
                    && last_output_at.elapsed() >= idle_timeout
                {
                    stopped_early = true;
                    break;
                }
                if let Some(every) = request.send_enter_every
                    && last_enter_at.elapsed() >= every
                    && !process_exited
                {
                    let _ = pty.write(b"\r").await;
                    last_enter_at = Instant::now();
                }
            }

            if process_exited {
                return Err(ClaudeCLISessionError::ProcessExited);
            }
        }

        if stopped_early && !request.settle_after_stop.is_zero() {
            let settle_deadline = deadline.min(Instant::now() + request.settle_after_stop);
            while Instant::now() < settle_deadline {
                if call.cancel.is_cancelled() {
                    state.cleanup().await;
                    return Err(ClaudeCLISessionError::Cancelled);
                }
                let wait = settle_deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(50));
                let Some(pty) = state.pty.as_mut() else {
                    break;
                };
                match pty.read_once(&mut chunk, wait).await {
                    Ok(DrainClassification::Data(n)) => {
                        if !buffer.append(&chunk[..n]) {
                            state.cleanup().await;
                            return Err(ClaudeCLISessionError::OutputTooLarge);
                        }
                    }
                    Ok(DrainClassification::WouldBlock) => {}
                    Ok(DrainClassification::Closed) => break,
                    Err(_) => break,
                }
            }
        }

        if !command_sent || trust_pending || buffer.as_slice().is_empty() {
            return Err(ClaudeCLISessionError::TimedOut);
        }
        String::from_utf8(buffer.into_bytes().to_vec()).map_err(|_| ClaudeCLISessionError::TimedOut)
    }
}

/// Type alias for an injected fetch override used in unit/integration tests.
pub type ClaudeStatusProbeFetchOverride = Arc<
    dyn Fn(
            PathBuf,
            Duration,
            bool,
        ) -> Pin<
            Box<dyn Future<Output = Result<ClaudeStatusSnapshot, ClaudeStatusProbeError>> + Send>,
        > + Send
        + Sync,
>;

/// Runs `claude` inside a PTY via [`ClaudeCLISession`], sends `/usage` and `/status`,
/// and parses the rendered text panels.
pub struct ClaudeStatusProbe {
    pub claude_binary: String,
    pub timeout: Duration,
    pub keep_cli_sessions_alive: bool,
    pub environment: BTreeMap<String, SecretString>,
    pub working_directory: PathBuf,
    pub home_dir: PathBuf,
    pub session: Arc<ClaudeCLISession>,
    pub fetch_override: Option<ClaudeStatusProbeFetchOverride>,
    pub direct_fetch_override: Option<ClaudeStatusProbeFetchOverride>,
}

impl ClaudeStatusProbe {
    pub const ACCOUNT_SCOPED_SESSION_REUSE_ENABLED: bool = true;

    #[must_use]
    pub fn new(
        environment: BTreeMap<String, SecretString>,
        home_dir: PathBuf,
        session: Arc<ClaudeCLISession>,
    ) -> Self {
        let working_directory = home_dir.clone();
        Self {
            claude_binary: "claude".to_owned(),
            timeout: Duration::from_secs(20),
            keep_cli_sessions_alive: false,
            environment,
            working_directory,
            home_dir,
            session,
            fetch_override: None,
            direct_fetch_override: None,
        }
    }

    #[must_use]
    pub fn with_working_directory(mut self, working_directory: PathBuf) -> Self {
        self.working_directory = working_directory;
        self
    }

    #[must_use]
    pub fn with_binary(mut self, binary: impl Into<String>) -> Self {
        self.claude_binary = binary.into();
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_keep_cli_sessions_alive(mut self, keep_alive: bool) -> Self {
        self.keep_cli_sessions_alive = keep_alive;
        self
    }

    #[must_use]
    pub fn with_fetch_override(mut self, fetch_override: ClaudeStatusProbeFetchOverride) -> Self {
        self.fetch_override = Some(fetch_override);
        self
    }

    #[must_use]
    pub fn with_direct_fetch_override(
        mut self,
        direct_fetch_override: ClaudeStatusProbeFetchOverride,
    ) -> Self {
        self.direct_fetch_override = Some(direct_fetch_override);
        self
    }

    #[must_use]
    pub fn should_keep_cli_session_alive(requested: bool) -> bool {
        requested && Self::ACCOUNT_SCOPED_SESSION_REUSE_ENABLED
    }

    /// Resolves the `claude` binary path using [`ExecutableResolver`] (or direct path if
    /// `binary_name` contains `'/'`).
    #[must_use]
    pub fn resolve_binary_path(
        binary_name: &str,
        environment: &BTreeMap<String, SecretString>,
        home_dir: &Path,
    ) -> Option<PathBuf> {
        if binary_name.contains('/') {
            let path = PathBuf::from(binary_name);
            return RealResolverFs.is_executable(&path).then_some(path);
        }
        let process_env = ProcessEnv::from_allowlist(
            environment
                .iter()
                .map(|(k, v)| (k.as_str(), v.expose_secret())),
        );
        let process_env = if let Some(override_val) = environment.get("CLAUDE_CLI_PATH") {
            process_env.with("CLAUDE_CLI_PATH", override_val.expose_secret())
        } else {
            process_env
        };
        let inputs = ResolverInputs::new(&process_env, home_dir, &RealResolverFs);
        resolve_executable(Tool::Claude, &inputs)
            .ok()
            .map(AbsolutePath::into_path_buf)
    }

    pub async fn fetch(
        &self,
        call: BrokerCall,
    ) -> Result<ClaudeStatusSnapshot, ClaudeStatusProbeError> {
        if call.cancel.is_cancelled() {
            return Err(ClaudeStatusProbeError::Cancelled);
        }
        let timeout = self.timeout;
        let keep_alive = Self::should_keep_cli_session_alive(self.keep_cli_sessions_alive);
        if let Some(ref fetch_override) = self.fetch_override {
            return (fetch_override)(PathBuf::from(&self.claude_binary), timeout, keep_alive).await;
        }
        let resolved =
            Self::resolve_binary_path(&self.claude_binary, &self.environment, &self.home_dir)
                .ok_or(ClaudeStatusProbeError::ClaudeNotInstalled)?;

        let account_scope = ClaudeAccountProfile::session_scope(
            &self.environment,
            &self.working_directory,
            &self.home_dir,
            &generate_uuid_v4(),
            &call,
        );
        let result = self
            .fetch_interactive(call, &resolved, &account_scope, timeout)
            .await;

        if !keep_alive {
            self.reset_transient_session_and_cleanup().await;
        }
        result
    }

    async fn fetch_interactive(
        &self,
        call: BrokerCall,
        resolved: &Path,
        account_scope: &str,
        timeout: Duration,
    ) -> Result<ClaudeStatusSnapshot, ClaudeStatusProbeError> {
        let mut usage = self
            .capture_subcommand(call.clone(), "/usage", resolved, account_scope, timeout)
            .await?;
        if !ClaudeStatusProbeParser::usage_output_looks_relevant(&usage) {
            usage = self
                .capture_subcommand(
                    call.clone(),
                    "/usage",
                    resolved,
                    account_scope,
                    timeout.max(Duration::from_secs(14)),
                )
                .await?;
        }
        ClaudeStatusProbeParser::validate_usage_before_status_probe(&usage)?;
        let status = self
            .capture_subcommand(
                call,
                "/status",
                resolved,
                account_scope,
                timeout.min(Duration::from_secs(12)),
            )
            .await
            .ok();
        ClaudeStatusProbeParser::parse(&usage, status.as_deref())
    }

    async fn reset_transient_session_and_cleanup(&self) {
        self.session.reset().await;
        let probe_dir = self.session.dedicated_probe_dir();
        let _ = ClaudeProbeSessionArtifactCleaner::cleanup_probe_session_artifacts(
            probe_dir,
            probe_dir,
            &self.environment,
            probe_dir,
            &self.home_dir,
        );
    }

    async fn capture_subcommand(
        &self,
        call: BrokerCall,
        subcommand: &str,
        binary: &Path,
        account_scope: &str,
        timeout: Duration,
    ) -> Result<String, ClaudeStatusProbeError> {
        let is_usage = subcommand == "/usage";
        let stop_on_substrings = if is_usage {
            vec!["Failed to load usage data".to_owned()]
        } else {
            Vec::new()
        };
        let idle_timeout = if is_usage {
            None
        } else {
            Some(Duration::from_secs(3))
        };
        let send_enter_every = if is_usage {
            Some(Duration::from_millis(800))
        } else {
            None
        };
        let stop_when_normalized: Option<ClaudeCLINormalizedStopPredicate> = if is_usage {
            Some(Arc::new(|normalized: &str| {
                ClaudeStatusProbeParser::usage_capture_has_session_value(normalized)
                    || ClaudeStatusProbeParser::usage_capture_has_subscription_notice(normalized)
            }))
        } else {
            None
        };
        let settle_after_stop = if is_usage {
            Duration::from_secs(2)
        } else {
            Duration::from_millis(250)
        };

        let request = ClaudeCLICaptureRequest {
            subcommand: subcommand.to_owned(),
            binary: binary.to_path_buf(),
            account_scope: Some(account_scope.to_owned()),
            timeout,
            environment: self.environment.clone(),
            idle_timeout,
            stop_on_substrings,
            stop_when_normalized,
            settle_after_stop,
            send_enter_every,
        };

        match self.session.capture(call, request).await {
            Ok(text) => Ok(text),
            Err(ClaudeCLISessionError::ProcessExited | ClaudeCLISessionError::TimedOut) => {
                self.session.reset().await;
                Err(ClaudeStatusProbeError::TimedOut)
            }
            Err(ClaudeCLISessionError::LaunchFailed(_)) => {
                Err(ClaudeStatusProbeError::ClaudeNotInstalled)
            }
            Err(ClaudeCLISessionError::Cancelled) => {
                self.session.reset().await;
                Err(ClaudeStatusProbeError::Cancelled)
            }
            Err(other) => {
                self.session.reset().await;
                Err(ClaudeStatusProbeError::ParseFailed(other.to_string()))
            }
        }
    }

    pub async fn fetch_identity(
        &self,
        call: BrokerCall,
        timeout: Duration,
    ) -> Result<ClaudeCLIAccountIdentity, ClaudeStatusProbeError> {
        let resolved =
            Self::resolve_binary_path(&self.claude_binary, &self.environment, &self.home_dir)
                .or_else(|| {
                    self.fetch_override
                        .is_some()
                        .then(|| PathBuf::from(&self.claude_binary))
                })
                .ok_or(ClaudeStatusProbeError::ClaudeNotInstalled)?;
        let account_scope = ClaudeAccountProfile::session_scope(
            &self.environment,
            &self.working_directory,
            &self.home_dir,
            &generate_uuid_v4(),
            &call,
        );
        let status_text = self
            .capture_subcommand(call, "/status", &resolved, &account_scope, timeout)
            .await?;
        Ok(ClaudeStatusProbeParser::parse_identity(
            None,
            Some(&status_text),
        ))
    }

    pub async fn touch_oauth_auth_path(
        &self,
        call: BrokerCall,
        timeout: Duration,
    ) -> Result<(), ClaudeStatusProbeError> {
        let resolved =
            Self::resolve_binary_path(&self.claude_binary, &self.environment, &self.home_dir)
                .or_else(|| {
                    self.fetch_override
                        .is_some()
                        .then(|| PathBuf::from(&self.claude_binary))
                })
                .ok_or(ClaudeStatusProbeError::ClaudeNotInstalled)?;
        let account_scope = ClaudeAccountProfile::session_scope(
            &self.environment,
            &self.working_directory,
            &self.home_dir,
            &generate_uuid_v4(),
            &call,
        );
        let request = ClaudeCLICaptureRequest {
            subcommand: "/status".to_owned(),
            binary: resolved,
            account_scope: Some(account_scope),
            timeout,
            environment: self.environment.clone(),
            idle_timeout: None,
            stop_on_substrings: Vec::new(),
            stop_when_normalized: None,
            settle_after_stop: Duration::from_millis(800),
            send_enter_every: Some(Duration::from_millis(800)),
        };
        let res = self.session.capture(call, request).await;
        self.session.reset().await;
        match res {
            Ok(_) => Ok(()),
            Err(ClaudeCLISessionError::Cancelled) => Err(ClaudeStatusProbeError::Cancelled),
            Err(ClaudeCLISessionError::TimedOut | ClaudeCLISessionError::ProcessExited) => {
                Err(ClaudeStatusProbeError::TimedOut)
            }
            Err(ClaudeCLISessionError::LaunchFailed(_)) => {
                Err(ClaudeStatusProbeError::ClaudeNotInstalled)
            }
            Err(other) => Err(ClaudeStatusProbeError::ParseFailed(other.to_string())),
        }
    }
}

/// [`DelegatedRefresher`] implementation backed by [`ClaudeStatusProbe::touch_oauth_auth_path`].
pub struct ClaudeCLIDelegatedRefresher {
    probe: ClaudeStatusProbe,
}

impl ClaudeCLIDelegatedRefresher {
    #[must_use]
    pub fn new(probe: ClaudeStatusProbe) -> Self {
        Self { probe }
    }
}

impl DelegatedRefresher for ClaudeCLIDelegatedRefresher {
    fn is_available(&self) -> bool {
        ClaudeStatusProbe::resolve_binary_path(
            &self.probe.claude_binary,
            &self.probe.environment,
            &self.probe.home_dir,
        )
        .is_some()
    }

    fn touch(&self, timeout: Duration) -> BoxFuture<'_, Result<(), DelegatedRefreshError>> {
        Box::pin(async move {
            let call = BrokerCall {
                interaction: FetchInteraction::UserInitiated,
                cancel: CancellationToken::new(),
                request_id: "claude-delegated-refresh".to_owned(),
            };
            match self.probe.touch_oauth_auth_path(call, timeout).await {
                Ok(()) => Ok(()),
                Err(ClaudeStatusProbeError::ClaudeNotInstalled) => {
                    Err(DelegatedRefreshError::CliUnavailable)
                }
                Err(ClaudeStatusProbeError::TimedOut) => Err(DelegatedRefreshError::Timeout),
                Err(other) => Err(DelegatedRefreshError::Failed(other.to_string())),
            }
        })
    }
}

/// Executes the Claude CLI usage flow (`loadViaCLIWithRetry`, `loadViaCLI`, `loadViaPTY`,
/// and `loadViaDirectCLI` fallback) from `ClaudeUsageFetcher.swift`.
pub struct ClaudeCLIUsageLoader {
    pub probe: ClaudeStatusProbe,
    pub rate_limit_gate: Arc<ClaudeCLIRateLimitGate>,
    pub registry: Arc<ProcessRegistry>,
    pub launch_gate: Arc<LaunchGate>,
}

impl ClaudeCLIUsageLoader {
    pub const CLI_AUTO_PROBE_TIMEOUT: Duration = Duration::from_secs(12);
    pub const CLI_PROBE_TIMEOUT: Duration = Duration::from_secs(24);
    pub const CLI_RETRY_PROBE_TIMEOUT: Duration = Duration::from_secs(60);

    #[must_use]
    pub fn direct_cli_usage_timeout(pty_timeout: Duration) -> Duration {
        let secs = (pty_timeout.as_secs_f64() / 3.0).clamp(6.0, 8.0);
        Duration::from_secs_f64(secs)
    }

    pub async fn load_with_retry(
        &self,
        call: BrokerCall,
        initial_timeout: Duration,
        now: jiff::Timestamp,
    ) -> Result<ClaudeStatusSnapshot, ClaudeStatusProbeError> {
        match self.load_once(call.clone(), initial_timeout, now).await {
            Ok(snap) => Ok(snap),
            Err(err) => {
                if call.cancel.is_cancelled() || !err.is_retryable() {
                    return Err(err);
                }
                self.load_once(call, Self::CLI_RETRY_PROBE_TIMEOUT, now)
                    .await
            }
        }
    }

    pub async fn load_once(
        &self,
        call: BrokerCall,
        timeout: Duration,
        now: jiff::Timestamp,
    ) -> Result<ClaudeStatusSnapshot, ClaudeStatusProbeError> {
        if call.cancel.is_cancelled() {
            return Err(ClaudeStatusProbeError::Cancelled);
        }
        if self
            .rate_limit_gate
            .blocked_until(call.interaction, now)
            .is_some()
        {
            return Err(ClaudeStatusProbeError::ParseFailed(
                ClaudeCLIRateLimitGate::MESSAGE.to_owned(),
            ));
        }

        let snapshot = match self.load_via_pty(call.clone(), timeout).await {
            Ok(snap) => snap,
            Err(pty_err) => {
                if call.cancel.is_cancelled()
                    || matches!(pty_err, ClaudeStatusProbeError::Cancelled)
                {
                    return Err(ClaudeStatusProbeError::Cancelled);
                }
                if pty_err.is_rate_limit_error()
                    || ClaudeCLIRateLimitGate::is_rate_limit_error(&pty_err.to_string())
                {
                    self.rate_limit_gate.record_rate_limit(now);
                    return Err(pty_err);
                }
                if !pty_err.should_try_direct_cli_usage() {
                    return Err(pty_err);
                }
                let direct_timeout = Self::direct_cli_usage_timeout(timeout);
                match self.load_via_direct_cli(call.clone(), direct_timeout).await {
                    Ok(snap) => snap,
                    Err(direct_err) => {
                        if call.cancel.is_cancelled()
                            || matches!(direct_err, ClaudeStatusProbeError::Cancelled)
                        {
                            return Err(ClaudeStatusProbeError::Cancelled);
                        }
                        if direct_err.is_rate_limit_error()
                            || ClaudeCLIRateLimitGate::is_rate_limit_error(&direct_err.to_string())
                        {
                            self.rate_limit_gate.record_rate_limit(now);
                            return Err(direct_err);
                        }
                        if ClaudeStatusProbeParser::is_subscription_quota_unavailable_description(
                            Some(&direct_err.to_string()),
                        ) {
                            return Err(direct_err);
                        }
                        return Err(pty_err);
                    }
                }
            }
        };

        self.rate_limit_gate.record_success();
        Ok(snapshot)
    }

    async fn load_via_pty(
        &self,
        call: BrokerCall,
        timeout: Duration,
    ) -> Result<ClaudeStatusSnapshot, ClaudeStatusProbeError> {
        let mut probe = ClaudeStatusProbe {
            claude_binary: self.probe.claude_binary.clone(),
            timeout,
            keep_cli_sessions_alive: self.probe.keep_cli_sessions_alive,
            environment: self.probe.environment.clone(),
            working_directory: self.probe.working_directory.clone(),
            home_dir: self.probe.home_dir.clone(),
            session: Arc::clone(&self.probe.session),
            fetch_override: self.probe.fetch_override.clone(),
            direct_fetch_override: self.probe.direct_fetch_override.clone(),
        };
        probe.timeout = timeout;
        probe.fetch(call).await
    }

    pub async fn load_via_direct_cli(
        &self,
        call: BrokerCall,
        timeout: Duration,
    ) -> Result<ClaudeStatusSnapshot, ClaudeStatusProbeError> {
        if call.cancel.is_cancelled() {
            return Err(ClaudeStatusProbeError::Cancelled);
        }
        if let Some(ref direct_override) = self.probe.direct_fetch_override {
            return (direct_override)(PathBuf::from(&self.probe.claude_binary), timeout, false)
                .await;
        }
        if self.probe.fetch_override.is_some() {
            // When the PTY probe is stubbed via `fetch_override` and no `direct_fetch_override`
            // is supplied, never fall through to resolving or spawning a real `claude` binary.
            return Err(ClaudeStatusProbeError::ClaudeNotInstalled);
        }
        self.fetch_direct_usage_snapshot(call, timeout).await
    }

    pub async fn fetch_direct_usage_snapshot(
        &self,
        call: BrokerCall,
        timeout: Duration,
    ) -> Result<ClaudeStatusSnapshot, ClaudeStatusProbeError> {
        if call.cancel.is_cancelled() {
            return Err(ClaudeStatusProbeError::Cancelled);
        }
        let resolved = ClaudeStatusProbe::resolve_binary_path(
            &self.probe.claude_binary,
            &self.probe.environment,
            &self.probe.home_dir,
        )
        .ok_or(ClaudeStatusProbeError::ClaudeNotInstalled)?;

        let dedicated_dir = self.probe.session.dedicated_probe_dir();
        let working_directory = isolated_probe_working_directory(dedicated_dir)
            .map_err(|e| ClaudeStatusProbeError::ParseFailed(e.to_string()))?;

        let cleaned_env = ClaudeCLISession::launch_environment(&self.probe.environment);
        let process_env =
            ClaudeCLISession::launch_process_env(&self.probe.environment, Some(&working_directory));
        let program =
            AbsolutePath::new(&resolved).map_err(|_| ClaudeStatusProbeError::ClaudeNotInstalled)?;
        let args = ClaudeCLISession::direct_usage_arguments()
            .into_iter()
            .map(Into::into)
            .collect();
        let mut command = Command::new(program, args)
            .with_env(process_env)
            .timeout(timeout);
        command.set_cwd(Some(working_directory.clone()));
        command.set_stdin(StdinSpec::Null);
        command.set_reap_marker(true);

        let cancel = call.cancel.clone();
        let run_res = run(call, command, &self.registry, &self.launch_gate).await;

        let _ = ClaudeProbeSessionArtifactCleaner::cleanup_probe_session_artifacts(
            &working_directory,
            dedicated_dir,
            &cleaned_env,
            &working_directory,
            &self.probe.home_dir,
        );

        match run_res {
            Ok(output) => {
                if cancel.is_cancelled() {
                    return Err(ClaudeStatusProbeError::Cancelled);
                }
                let stdout_str = String::from_utf8_lossy(&output.stdout);
                ClaudeStatusProbeParser::parse(&stdout_str, None)
            }
            Err(ProcessError::Cancelled) => Err(ClaudeStatusProbeError::Cancelled),
            Err(ProcessError::TimedOut) => {
                if cancel.is_cancelled() {
                    Err(ClaudeStatusProbeError::Cancelled)
                } else {
                    Err(ClaudeStatusProbeError::TimedOut)
                }
            }
            Err(ProcessError::LaunchFailed) => Err(ClaudeStatusProbeError::ClaudeNotInstalled),
            Err(other) => Err(ClaudeStatusProbeError::ParseFailed(other.to_string())),
        }
    }
}
