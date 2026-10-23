// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeCLIUsageSpawnThrottle.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeCLIRateLimitGate.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeCLIAuthStatusProbe.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeProbeSessionArtifactCleaner.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Gates, throttles, auth-status probe, background availability markers, and
//! session artifact cleanup for the Claude `cli` strategy.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::pipeline::{FetchInteraction, FetchResult};
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;

use super::account_profile::ClaudeAccountProfile;
use super::cli_session::{ClaudeCLISession, is_dedicated_probe_working_directory};
use super::config_paths::{ClaudeConfigPaths, normalize_lexical};
use crate::brokers::call::BrokerCall;
use crate::brokers::process::{AbsolutePath, Command, ProcessRegistry, run};

/// Prompt policy mode for background opaque-child execution and auto CLI fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClaudeKeychainPromptMode {
    /// Never allow unattended background prompts.
    Never,
    /// Only allow prompts on explicit user action (default).
    #[default]
    OnlyOnUserAction,
    /// Explicitly allow background opaque child execution.
    Always,
}

/// Key for [`ClaudeCLIUsageSpawnThrottle`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClaudeCLIUsageSpawnThrottleKey {
    pub binary: String,
    pub account_scope: String,
    pub use_web_extras: bool,
    pub include_prepaid_balance: bool,
}

#[derive(Debug, Clone)]
struct ThrottleEntry {
    result: FetchResult,
    recorded_at: Timestamp,
}

/// Background CLI usage spawn throttle (15-minute minimum interval per account scope).
#[derive(Debug, Default)]
pub struct ClaudeCLIUsageSpawnThrottle {
    entries: Mutex<HashMap<ClaudeCLIUsageSpawnThrottleKey, ThrottleEntry>>,
}

impl ClaudeCLIUsageSpawnThrottle {
    pub const MINIMUM_INTERVAL: Duration = Duration::from_secs(15 * 60);

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn key(
        binary: &str,
        env: &BTreeMap<String, SecretString>,
        working_dir: &Path,
        home_dir: &Path,
        use_web_extras: bool,
        include_prepaid_balance: bool,
        call: &BrokerCall,
    ) -> Option<ClaudeCLIUsageSpawnThrottleKey> {
        let account_scope =
            ClaudeAccountProfile::identified_session_scope(env, working_dir, home_dir, call)?;
        Some(ClaudeCLIUsageSpawnThrottleKey {
            binary: binary.to_owned(),
            account_scope,
            use_web_extras,
            include_prepaid_balance,
        })
    }

    #[must_use]
    pub fn cached_result(
        &self,
        key: &ClaudeCLIUsageSpawnThrottleKey,
        now: Timestamp,
    ) -> Option<FetchResult> {
        let mut guard = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = guard.get(key)?;
        let elapsed = now.duration_since(entry.recorded_at);
        if elapsed >= jiff::SignedDuration::ZERO
            && elapsed < jiff::SignedDuration::from_secs(15 * 60)
            && !Self::any_window_has_reset(&entry.result.usage, now)
        {
            return Some(entry.result.clone());
        }
        guard.remove(key);
        None
    }

    #[must_use]
    pub fn any_window_has_reset(
        usage: &ariadusage_core::model::UsageSnapshot,
        now: Timestamp,
    ) -> bool {
        let primary_windows = [
            usage.primary.as_ref(),
            usage.secondary.as_ref(),
            usage.tertiary.as_ref(),
        ]
        .into_iter()
        .flatten();
        let extra_windows = usage
            .extra_rate_windows
            .iter()
            .flatten()
            .map(|named| &named.window);
        primary_windows
            .chain(extra_windows)
            .any(|window| window.resets_at.is_some_and(|resets_at| resets_at <= now))
    }

    pub fn record(&self, result: FetchResult, key: ClaudeCLIUsageSpawnThrottleKey, now: Timestamp) {
        let mut guard = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.retain(|_, entry| {
            let elapsed = now.duration_since(entry.recorded_at);
            elapsed < jiff::SignedDuration::from_secs(15 * 60)
        });
        guard.insert(
            key,
            ThrottleEntry {
                result,
                recorded_at: now,
            },
        );
    }

    pub fn reset(&self) {
        let mut guard = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.clear();
    }
}

/// Process-wide rate-limit cooldown gate for the Claude CLI `/usage` endpoint (5 minutes).
#[derive(Debug, Default)]
pub struct ClaudeCLIRateLimitGate {
    cooldown_until: Mutex<Option<Timestamp>>,
}

impl ClaudeCLIRateLimitGate {
    pub const COOLDOWN_DURATION: Duration = Duration::from_secs(5 * 60);
    pub const MESSAGE: &'static str =
        "Claude CLI usage endpoint is rate limited right now. Please try again later.";

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn blocked_until(
        &self,
        interaction: FetchInteraction,
        now: Timestamp,
    ) -> Option<Timestamp> {
        if interaction == FetchInteraction::UserInitiated {
            return None;
        }
        self.current_blocked_until(now)
    }

    #[must_use]
    pub fn current_blocked_until(&self, now: Timestamp) -> Option<Timestamp> {
        let mut guard = self
            .cooldown_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let until = (*guard)?;
        if until > now {
            return Some(until);
        }
        *guard = None;
        None
    }

    pub fn record_rate_limit(&self, now: Timestamp) {
        let mut guard = self
            .cooldown_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = now
            .checked_add(jiff::SignedDuration::from_secs(5 * 60))
            .ok();
    }

    pub fn record_success(&self) {
        self.reset();
    }

    pub fn reset(&self) {
        let mut guard = self
            .cooldown_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = None;
    }

    #[must_use]
    pub fn is_rate_limit_error(message: &str) -> bool {
        Self::is_rate_limit_message(message, true)
    }

    #[must_use]
    pub fn is_rate_limit_message(message: &str, allow_raw_rate_limit_token: bool) -> bool {
        let lower = message.to_ascii_lowercase();
        lower.contains(&Self::MESSAGE.to_ascii_lowercase())
            || (allow_raw_rate_limit_token && lower.contains("rate_limit_error"))
            || (lower.contains("claude cli")
                && lower.contains("usage")
                && lower.contains("rate limited"))
    }
}

/// Authentication status returned by `claude auth status --json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeCLIAuthenticationStatus {
    LoggedIn,
    LoggedOut,
    Unavailable,
}

/// Output captured from a non-interactive Claude CLI subprocess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCLISubprocessResult {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Non-interactive `claude auth status --json` probe.
pub struct ClaudeCLIAuthStatusProbe;

impl ClaudeCLIAuthStatusProbe {
    pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

    #[must_use]
    pub fn probe_arguments() -> Vec<String> {
        vec!["auth".to_owned(), "status".to_owned(), "--json".to_owned()]
    }

    #[must_use]
    pub fn parse_logged_in(stdout: &[u8]) -> bool {
        Self::parse_status(stdout) == Some(ClaudeCLIAuthenticationStatus::LoggedIn)
    }

    #[must_use]
    pub fn parse_status(stdout: &[u8]) -> Option<ClaudeCLIAuthenticationStatus> {
        let value: serde_json::Value = serde_json::from_slice(stdout).ok()?;
        let logged_in = value.get("loggedIn")?.as_bool()?;
        Some(if logged_in {
            ClaudeCLIAuthenticationStatus::LoggedIn
        } else {
            ClaudeCLIAuthenticationStatus::LoggedOut
        })
    }

    #[must_use]
    pub fn evaluate_subprocess_result(
        result: Option<&ClaudeCLISubprocessResult>,
    ) -> ClaudeCLIAuthenticationStatus {
        let Some(result) = result else {
            return ClaudeCLIAuthenticationStatus::Unavailable;
        };
        Self::parse_status(&result.stdout).unwrap_or(ClaudeCLIAuthenticationStatus::Unavailable)
    }

    pub async fn authentication_status(
        call: BrokerCall,
        registry: &ProcessRegistry,
        gate: &LaunchGate,
        binary: &Path,
        base_env: &BTreeMap<String, SecretString>,
        working_directory: &Path,
        timeout: Duration,
    ) -> ClaudeCLIAuthenticationStatus {
        let Ok(program) = AbsolutePath::new(binary) else {
            return ClaudeCLIAuthenticationStatus::Unavailable;
        };
        let env = ClaudeCLISession::launch_process_env(base_env, Some(working_directory));
        let args = Self::probe_arguments()
            .into_iter()
            .map(Into::into)
            .collect();
        let mut command = Command::new(program, args).with_env(env).timeout(timeout);
        command.set_cwd(Some(working_directory.to_path_buf()));
        command.set_reap_marker(true);
        let run_result =
            run(call, command, registry, gate)
                .await
                .ok()
                .map(|out| ClaudeCLISubprocessResult {
                    exit_code: out.status.code(),
                    stdout: out.stdout.to_vec(),
                    stderr: out.stderr.to_vec(),
                });
        Self::evaluate_subprocess_result(run_result.as_ref())
    }

    pub async fn is_logged_in(
        call: BrokerCall,
        registry: &ProcessRegistry,
        gate: &LaunchGate,
        binary: &Path,
        base_env: &BTreeMap<String, SecretString>,
        working_directory: &Path,
        timeout: Duration,
    ) -> bool {
        Self::authentication_status(
            call,
            registry,
            gate,
            binary,
            base_env,
            working_directory,
            timeout,
        )
        .await
            == ClaudeCLIAuthenticationStatus::LoggedIn
    }
}

/// Marker identifying a binary + account scope whose CLI availability was established by user action.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClaudeCLIBackgroundMarker {
    pub binary: String,
    pub account_scope: String,
}

#[derive(Debug, Default)]
struct AvailabilityInner {
    markers: HashSet<ClaudeCLIBackgroundMarker>,
    revoked_markers: HashSet<ClaudeCLIBackgroundMarker>,
}

/// Store tracking background availability markers and revocations for the Claude CLI.
#[derive(Debug, Default)]
pub struct ClaudeCLIBackgroundAvailabilityStore {
    inner: Mutex<AvailabilityInner>,
}

impl ClaudeCLIBackgroundAvailabilityStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn capture_marker(
        binary: &str,
        env: &BTreeMap<String, SecretString>,
        working_dir: &Path,
        home_dir: &Path,
        call: &BrokerCall,
    ) -> Option<ClaudeCLIBackgroundMarker> {
        let account_scope =
            ClaudeAccountProfile::identified_session_scope(env, working_dir, home_dir, call)?;
        Some(ClaudeCLIBackgroundMarker {
            binary: binary.to_owned(),
            account_scope,
        })
    }

    #[must_use]
    pub fn is_established(&self, marker: Option<&ClaudeCLIBackgroundMarker>) -> bool {
        let Some(marker) = marker else {
            return false;
        };
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.markers.contains(marker)
    }

    #[must_use]
    pub fn is_established_for(
        &self,
        binary: &str,
        env: &BTreeMap<String, SecretString>,
        working_dir: &Path,
        home_dir: &Path,
        call: &BrokerCall,
    ) -> bool {
        let marker = Self::capture_marker(binary, env, working_dir, home_dir, call);
        self.is_established(marker.as_ref())
    }

    pub fn establish(&self, marker: ClaudeCLIBackgroundMarker) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.revoked_markers.remove(&marker);
        guard.markers.insert(marker);
    }

    pub fn establish_for(
        &self,
        binary: &str,
        env: &BTreeMap<String, SecretString>,
        working_dir: &Path,
        home_dir: &Path,
        call: &BrokerCall,
    ) {
        if let Some(marker) = Self::capture_marker(binary, env, working_dir, home_dir, call) {
            self.establish(marker);
        }
    }

    pub fn revoke(&self, marker: &ClaudeCLIBackgroundMarker) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.markers.remove(marker);
        guard.revoked_markers.insert(marker.clone());
    }

    #[must_use]
    pub fn is_revoked(&self, marker: &ClaudeCLIBackgroundMarker) -> bool {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.revoked_markers.contains(marker)
    }

    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn allows_opaque_child_execution(
        &self,
        binary: &str,
        env: &BTreeMap<String, SecretString>,
        working_dir: &Path,
        home_dir: &Path,
        interaction: FetchInteraction,
        keychain_disabled: bool,
        prompt_mode: ClaudeKeychainPromptMode,
        call: &BrokerCall,
    ) -> bool {
        if interaction != FetchInteraction::Background {
            return true;
        }
        if !self.is_established_for(binary, env, working_dir, home_dir, call) {
            return false;
        }
        keychain_disabled || prompt_mode == ClaudeKeychainPromptMode::Always
    }

    #[allow(clippy::too_many_arguments)]
    pub fn allows_background_auto_usage_fetch<F>(
        &self,
        binary: &str,
        env: &BTreeMap<String, SecretString>,
        working_dir: &Path,
        home_dir: &Path,
        interaction: FetchInteraction,
        keychain_disabled: bool,
        prompt_mode: ClaudeKeychainPromptMode,
        call: &BrokerCall,
        oauth_credentials_confirmed_absent: F,
    ) -> bool
    where
        F: FnOnce() -> bool,
    {
        if interaction != FetchInteraction::Background {
            return true;
        }
        if !keychain_disabled {
            if self.allows_opaque_child_execution(
                binary,
                env,
                working_dir,
                home_dir,
                interaction,
                keychain_disabled,
                prompt_mode,
                call,
            ) {
                return true;
            }
            if self.is_established_for(binary, env, working_dir, home_dir, call) {
                return false;
            }
            let Some(marker) = Self::capture_marker(binary, env, working_dir, home_dir, call)
            else {
                return false;
            };
            if self.is_revoked(&marker) {
                return false;
            }
            if prompt_mode != ClaudeKeychainPromptMode::Always {
                return false;
            }
            return oauth_credentials_confirmed_absent();
        }
        let Some(marker) = Self::capture_marker(binary, env, working_dir, home_dir, call) else {
            return false;
        };
        !self.is_revoked(&marker)
    }
}

/// Removes synthetic `.jsonl` session transcripts written by Claude Code under
/// `~/.claude/projects/<sanitized-probe-dir>/` when probing from Ariadusage's dedicated probe directory.
pub struct ClaudeProbeSessionArtifactCleaner;

impl ClaudeProbeSessionArtifactCleaner {
    const MAX_SANITIZED_LENGTH: usize = 200;

    #[must_use]
    pub fn cleanup_probe_session_artifacts(
        probe_directory: &Path,
        dedicated_probe_root: &Path,
        env: &BTreeMap<String, SecretString>,
        working_dir: &Path,
        home_dir: &Path,
    ) -> Vec<PathBuf> {
        if !is_dedicated_probe_working_directory(probe_directory, dedicated_probe_root) {
            return Vec::new();
        }

        let config_directory =
            ClaudeConfigPaths::config_root_from_secret_env(env, working_dir, home_dir);
        let project_directory = Self::project_directory_path(probe_directory, &config_directory);
        let Ok(read_dir) = std::fs::read_dir(&project_directory) else {
            return Vec::new();
        };

        let mut removed = Vec::new();
        for entry in read_dir.flatten() {
            let path = entry.path();
            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            if is_dir {
                continue;
            }
            let is_jsonl = path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"));
            if !is_jsonl {
                continue;
            }
            if std::fs::remove_file(&path).is_ok() {
                removed.push(path);
            }
        }
        removed.sort();

        if std::fs::read_dir(&project_directory)
            .ok()
            .is_some_and(|mut it| it.next().is_none())
        {
            let _ = std::fs::remove_dir(&project_directory);
        }

        removed
    }

    #[must_use]
    pub fn project_directory_path(probe_directory: &Path, config_directory: &Path) -> PathBuf {
        let resolved = std::fs::canonicalize(probe_directory)
            .unwrap_or_else(|_| normalize_lexical(probe_directory));
        let mut path_str = resolved.to_string_lossy().into_owned();
        if path_str.len() > 1 && path_str.ends_with('/') {
            path_str.pop();
        }
        config_directory
            .join("projects")
            .join(Self::sanitized_project_directory_name(&path_str))
    }

    #[must_use]
    pub fn sanitized_project_directory_name(path: &str) -> String {
        let mut sanitized = String::with_capacity(path.len());
        for unit in path.encode_utf16() {
            if matches!(unit, 0x30..=0x39 | 0x41..=0x5A | 0x61..=0x7A) {
                sanitized.push(char::from(unit as u8));
            } else {
                sanitized.push('-');
            }
        }
        if sanitized.len() <= Self::MAX_SANITIZED_LENGTH {
            return sanitized;
        }
        let prefix = &sanitized[..Self::MAX_SANITIZED_LENGTH];
        format!("{prefix}-{}", Self::js_base36_hash(path))
    }

    #[must_use]
    pub fn js_base36_hash(text: &str) -> String {
        let mut hash: i32 = 0;
        for unit in text.encode_utf16() {
            hash = hash
                .wrapping_shl(5)
                .wrapping_sub(hash)
                .wrapping_add(i32::from(unit));
        }
        Self::format_radix_36(hash.unsigned_abs())
    }

    fn format_radix_36(mut value: u32) -> String {
        if value == 0 {
            return "0".to_owned();
        }
        const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut buf = Vec::with_capacity(8);
        while value > 0 {
            buf.push(DIGITS[(value % 36) as usize]);
            value /= 36;
        }
        buf.reverse();
        String::from_utf8(buf).unwrap_or_default()
    }
}
