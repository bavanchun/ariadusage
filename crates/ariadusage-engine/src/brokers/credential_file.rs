// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexOAuth/CodexOAuthCredentials.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthCredentials.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthKeychainAccessGate.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthKeychainPromptMode.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthCredentials+Hashing.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/KeychainAccessGate.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/KeychainNoUIQuery.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::fmt::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ariadusage_core::digest::sha256_hex;
use ariadusage_core::pipeline::FetchInteraction;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::call::BrokerCall;
use super::secret_store::service::os_status;
use crate::state_store::BrokerStateStore;

/// Maximum permitted credential file or Keychain item size (1 MiB).
pub const MAX_CREDENTIAL_FILE_SIZE: u64 = 1024 * 1024;

/// macOS Keychain generic-password service name owned by Claude Code CLI.
pub const CLAUDE_CODE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// Cooldown after a user cancels or denies a foreign Keychain prompt (6 hours, matching
/// upstream `ClaudeOAuthKeychainAccessGate.cooldownInterval`).
pub const KEYCHAIN_DENIAL_COOLDOWN_SECS: i64 = 6 * 60 * 60;

/// Length of the truncated SHA-256 hex digest used in `KeychainItemFingerprint`.
pub const KEYCHAIN_FINGERPRINT_PREFIX_LEN: usize = 12;

/// A declared path to a credential file.
///
/// Outside crates cannot construct a `CredentialDecl` directly:
/// ```compile_fail
/// use ariadusage_engine::brokers::credential_file::CredentialDecl;
/// let _ = CredentialDecl::new("/path/to/cred");
/// ```
#[derive(Clone)]
pub struct CredentialDecl {
    path: PathBuf,
}

impl CredentialDecl {
    #[allow(dead_code)]
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the filesystem path for this declared credential file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Test hook allowing integration tests to construct a declared path.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn for_test(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl fmt::Debug for CredentialDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialDecl")
            .field("path_len", &self.path.as_os_str().len())
            .finish()
    }
}

/// Stat-based fingerprint of a credential file capturing filesystem device, inode,
/// modification time in nanoseconds, and file size.
///
/// Plaintext path is skipped during serialization to keep persisted broker state
/// free of user path text and secret tokens.
#[derive(Clone, Eq, Serialize, Deserialize)]
pub struct StatFingerprint {
    #[serde(skip)]
    pub path: PathBuf,
    pub dev: u64,
    pub ino: u64,
    pub mtime_ns: i128,
    pub size: u64,
}

impl PartialEq for StatFingerprint {
    fn eq(&self, other: &Self) -> bool {
        self.dev == other.dev
            && self.ino == other.ino
            && self.mtime_ns == other.mtime_ns
            && self.size == other.size
            && (self.path.as_os_str().is_empty()
                || other.path.as_os_str().is_empty()
                || self.path == other.path)
    }
}

impl std::hash::Hash for StatFingerprint {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.dev.hash(state);
        self.ino.hash(state);
        self.mtime_ns.hash(state);
        self.size.hash(state);
    }
}

impl fmt::Debug for StatFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatFingerprint")
            .field("path_len", &self.path.as_os_str().len())
            .field("dev", &self.dev)
            .field("ino", &self.ino)
            .field("mtime_ns", &self.mtime_ns)
            .field("size", &self.size)
            .finish()
    }
}

/// Successful read of a declared credential file, returning zeroized bytes and stat fingerprint.
pub struct CredentialRead {
    pub bytes: Zeroizing<Vec<u8>>,
    pub stat: StatFingerprint,
}

impl fmt::Debug for CredentialRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialRead")
            .field("bytes_len", &self.bytes.len())
            .field("stat", &self.stat)
            .finish()
    }
}

/// Errors encountered while reading or validating a declared credential file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CredentialFileError {
    #[error("credential file not found")]
    NotFound,

    #[error("credential file is untrusted")]
    Untrusted,

    #[error("credential file is unreadable")]
    Unreadable,

    #[error("credential file is too large")]
    TooLarge,

    #[error("credential file is malformed")]
    Malformed,

    #[error("operation cancelled")]
    Cancelled,
}

/// Computes the standard SHA-256 hexadecimal content fingerprint of the given byte buffer.
pub fn content_fingerprint(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Computes the 12-character hexadecimal SHA-256 prefix used by Keychain item fingerprints.
pub fn sha256_prefix(bytes: &[u8]) -> String {
    let full = content_fingerprint(bytes);
    full[..KEYCHAIN_FINGERPRINT_PREFIX_LEN].to_owned()
}

/// A declared read-only foreign Keychain item (such as Claude Code's `"Claude Code-credentials"`).
///
/// Outside crates cannot construct a `KeychainCredentialDecl` with an arbitrary service directly:
/// ```compile_fail
/// use ariadusage_engine::brokers::credential_file::KeychainCredentialDecl;
/// let _ = KeychainCredentialDecl::new("arbitrary-service", None);
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct KeychainCredentialDecl {
    service: String,
    account: Option<String>,
}

impl KeychainCredentialDecl {
    #[allow(dead_code)]
    pub(crate) fn new(service: impl Into<String>, account: Option<String>) -> Self {
        Self {
            service: service.into(),
            account,
        }
    }

    /// Declares the Claude Code OAuth Keychain item (`service = "Claude Code-credentials"`).
    pub fn claude_code() -> Self {
        Self {
            service: CLAUDE_CODE_KEYCHAIN_SERVICE.to_owned(),
            account: None,
        }
    }

    /// Declares the Claude Code OAuth Keychain item pinned to a specific account attribute.
    pub fn claude_code_with_account(account: impl Into<String>) -> Self {
        Self {
            service: CLAUDE_CODE_KEYCHAIN_SERVICE.to_owned(),
            account: Some(account.into()),
        }
    }

    /// Returns the declared Keychain service name (`kSecAttrService`).
    pub fn service(&self) -> &str {
        &self.service
    }

    /// Returns the optional Keychain account filter (`kSecAttrAccount`).
    pub fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    /// Test hook allowing integration tests to construct a declared Keychain item.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn for_test(service: impl Into<String>, account: Option<String>) -> Self {
        Self {
            service: service.into(),
            account,
        }
    }
}

impl fmt::Debug for KeychainCredentialDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeychainCredentialDecl")
            .field("service", &self.service)
            .field("has_account", &self.account.is_some())
            .finish()
    }
}

/// Prompt policy for reading another tool's Keychain item on macOS.
///
/// Following `ARCHITECTURE.md` §9.3, background work never triggers a system prompt:
/// `OnlyOnUserAction` (the default) allows interactive Keychain reads only on
/// `FetchInteraction::UserInitiated` calls while keeping background reads strictly non-interactive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeychainPromptMode {
    /// Never access the foreign Keychain item.
    Never,
    /// Only allow interactive Keychain prompts on explicit user actions (default).
    #[default]
    OnlyOnUserAction,
}

/// Non-secret metadata fingerprint for a foreign Keychain item (ported from
/// `ClaudeKeychainFingerprint` in `ClaudeOAuthCredentials.swift`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeychainItemFingerprint {
    pub modified_at: Option<i64>,
    pub created_at: Option<i64>,
    pub persistent_ref_hash: Option<String>,
}

/// Successful read of a declared foreign Keychain item.
pub struct KeychainCredentialRead {
    pub bytes: Zeroizing<Vec<u8>>,
    pub fingerprint: KeychainItemFingerprint,
}

impl fmt::Debug for KeychainCredentialRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeychainCredentialRead")
            .field("bytes_len", &self.bytes.len())
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

/// Errors encountered while reading a declared foreign Keychain item.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeychainCredentialError {
    #[error("keychain credential item not found")]
    NotFound,

    #[error("keychain credential item requires user interaction")]
    InteractionRequired,

    #[error("keychain credential prompt was dismissed or denied")]
    Dismissed,

    #[error("keychain credential prompt is suppressed by denial cooldown")]
    CooldownActive,

    #[error("keychain credential access is disabled")]
    Disabled,

    #[error("keychain credential item is too large")]
    TooLarge,

    #[error("keychain credential item is unreadable")]
    Unreadable,

    #[error("operation cancelled")]
    Cancelled,
}

impl KeychainCredentialError {
    /// Converts a Keychain credential error into the shared `CredentialFileError` vocabulary.
    pub fn into_credential_file_error(self) -> CredentialFileError {
        match self {
            Self::NotFound => CredentialFileError::NotFound,
            Self::TooLarge => CredentialFileError::TooLarge,
            Self::Cancelled => CredentialFileError::Cancelled,
            Self::InteractionRequired
            | Self::Dismissed
            | Self::CooldownActive
            | Self::Disabled
            | Self::Unreadable => CredentialFileError::Unreadable,
        }
    }
}

impl From<KeychainCredentialError> for CredentialFileError {
    fn from(value: KeychainCredentialError) -> Self {
        value.into_credential_file_error()
    }
}

#[derive(Default)]
struct KeychainGateState {
    disabled: bool,
    denied_until_by_key: BTreeMap<String, i64>,
    prompt_attempt_generation: u64,
}

/// Gate tracking denial cooldowns (6 hours) and prompt attempt generations for foreign Keychain
/// items (ported from `ClaudeOAuthKeychainAccessGate` and `KeychainAccessGate`).
pub struct KeychainAccessGate {
    state: Mutex<KeychainGateState>,
    store: Option<Arc<BrokerStateStore>>,
}

impl KeychainAccessGate {
    /// Creates a new gate with an explicit `disabled` flag.
    pub fn new(disabled: bool) -> Self {
        Self {
            state: Mutex::new(KeychainGateState {
                disabled,
                denied_until_by_key: BTreeMap::new(),
                prompt_attempt_generation: 0,
            }),
            store: None,
        }
    }

    /// Creates a new gate honoring `ARIADUSAGE_DISABLE_KEYRING=1`.
    pub fn from_env() -> Self {
        let disabled = std::env::var("ARIADUSAGE_DISABLE_KEYRING").ok().as_deref() == Some("1");
        Self::new(disabled)
    }

    /// Attaches a `BrokerStateStore` so denial cooldowns persist across process restarts.
    pub fn with_state_store(mut self, store: Arc<BrokerStateStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Returns `true` when Keychain access is disabled for this gate.
    pub fn is_disabled(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .disabled
    }

    /// Updates the `disabled` flag on this gate.
    pub fn set_disabled(&self, disabled: bool) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .disabled = disabled;
    }

    /// Returns `true` if a user-initiated Keychain prompt for `service` is currently permitted at
    /// `now_seconds` (not disabled and not inside the 6-hour denial cooldown).
    pub fn should_allow_prompt(&self, service: &str, now_seconds: i64) -> bool {
        let key = cooldown_key(service);
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.disabled {
            return false;
        }

        if let Some(&until) = guard.denied_until_by_key.get(&key) {
            if until > now_seconds {
                return false;
            }
            guard.denied_until_by_key.remove(&key);
        }

        if let Some(store) = &self.store
            && let Ok(persisted) = store.load()
            && let Some(&until) = persisted.browser_cooldowns.get(&key)
        {
            if until > now_seconds {
                guard.denied_until_by_key.insert(key, until);
                return false;
            }
            let _ = store.update(|state| {
                state.browser_cooldowns.remove(&key);
            });
        }

        true
    }

    /// Records a Keychain prompt denial or cancellation for `service`, blocking further prompts
    /// until `now_seconds + KEYCHAIN_DENIAL_COOLDOWN_SECS` (6 hours).
    pub fn record_denied(&self, service: &str, now_seconds: i64) {
        let key = cooldown_key(service);
        let until = now_seconds.saturating_add(KEYCHAIN_DENIAL_COOLDOWN_SECS);
        {
            let mut guard = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.denied_until_by_key.insert(key.clone(), until);
        }
        if let Some(store) = &self.store {
            let _ = store.update(|state| {
                state.browser_cooldowns.insert(key, until);
            });
        }
    }

    /// Clears an active denial cooldown for `service` (used by explicit user-initiated repair).
    /// Returns `true` if a non-expired cooldown was cleared.
    pub fn clear_denied(&self, service: &str, now_seconds: i64) -> bool {
        let key = cooldown_key(service);
        let mut had_active = false;
        {
            let mut guard = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(until) = guard.denied_until_by_key.remove(&key)
                && until > now_seconds
            {
                had_active = true;
            }
        }
        if let Some(store) = &self.store {
            let _ = store.update(|state| {
                if let Some(until) = state.browser_cooldowns.remove(&key)
                    && until > now_seconds
                {
                    had_active = true;
                }
            });
        }
        had_active
    }

    /// Returns the current interactive prompt attempt generation counter.
    pub fn prompt_attempt_generation(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .prompt_attempt_generation
    }

    /// Advances and returns the interactive prompt attempt generation counter after a prompt
    /// attempt completes.
    pub fn record_prompt_attempt_completed(&self) -> u64 {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.prompt_attempt_generation = guard.prompt_attempt_generation.saturating_add(1);
        guard.prompt_attempt_generation
    }
}

impl fmt::Debug for KeychainAccessGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let guard = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f.debug_struct("KeychainAccessGate")
            .field("disabled", &guard.disabled)
            .field("cooldown_count", &guard.denied_until_by_key.len())
            .field(
                "prompt_attempt_generation",
                &guard.prompt_attempt_generation,
            )
            .finish()
    }
}

fn cooldown_key(service: &str) -> String {
    sha256_hex("keychain_cooldown", &[service.as_bytes()])
}

/// Candidate metadata and payload used when selecting among multiple generic password items for a
/// service.
#[derive(Clone)]
pub struct KeychainCandidateRecord {
    pub account: String,
    pub modified_at: Option<i64>,
    pub created_at: Option<i64>,
    pub persistent_ref_hash: Option<String>,
    pub data: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for KeychainCandidateRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeychainCandidateRecord")
            .field("account_len", &self.account.len())
            .field("modified_at", &self.modified_at)
            .field("created_at", &self.created_at)
            .field("persistent_ref_hash", &self.persistent_ref_hash)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// Sorts Keychain candidates newest-first by `modified_at ?? created_at ?? i64::MIN` (ported from
/// `ClaudeOAuthCredentials.swift` candidate ordering).
pub fn sort_keychain_candidates(candidates: &mut [KeychainCandidateRecord]) {
    candidates.sort_by(|left, right| {
        let left_ts = left.modified_at.or(left.created_at).unwrap_or(i64::MIN);
        let right_ts = right.modified_at.or(right.created_at).unwrap_or(i64::MIN);
        right_ts.cmp(&left_ts)
    });
}

/// Low-level reader seam for foreign Keychain items.
///
/// Implementations return macOS `OSStatus` codes (`os_status::ERR_SEC_*`) on failure so tests on
/// any platform can exercise the full prompt-mode and denial-cooldown state machine without
/// touching the user's login Keychain.
pub trait KeychainCredentialReader: Send + Sync {
    fn read_item(
        &self,
        decl: &KeychainCredentialDecl,
        allow_prompt: bool,
    ) -> Result<KeychainCredentialRead, i32>;

    fn probe_fingerprint(
        &self,
        decl: &KeychainCredentialDecl,
    ) -> Result<Option<KeychainItemFingerprint>, i32>;
}

impl<T: KeychainCredentialReader + ?Sized> KeychainCredentialReader for Arc<T> {
    fn read_item(
        &self,
        decl: &KeychainCredentialDecl,
        allow_prompt: bool,
    ) -> Result<KeychainCredentialRead, i32> {
        (**self).read_item(decl, allow_prompt)
    }

    fn probe_fingerprint(
        &self,
        decl: &KeychainCredentialDecl,
    ) -> Result<Option<KeychainItemFingerprint>, i32> {
        (**self).probe_fingerprint(decl)
    }
}

/// Read-only Keychain credential source for foreign tools' items (such as Claude Code's OAuth
/// item `"Claude Code-credentials"`).
///
/// By construction this type exposes only read and fingerprint-probe operations; AriadUsage never
/// writes to or deletes another tool's Keychain item (`ARCHITECTURE.md` §9.2).
pub struct KeychainCredentialSource {
    gate: Arc<KeychainAccessGate>,
    prompt_mode: KeychainPromptMode,
    reader: Arc<dyn KeychainCredentialReader>,
}

impl KeychainCredentialSource {
    /// Creates a new read-only Keychain credential source governed by `gate`.
    pub fn new(gate: Arc<KeychainAccessGate>) -> Self {
        Self {
            gate,
            prompt_mode: KeychainPromptMode::OnlyOnUserAction,
            reader: default_keychain_reader(),
        }
    }

    /// Sets the prompt policy for foreign Keychain reads.
    pub fn with_prompt_mode(mut self, prompt_mode: KeychainPromptMode) -> Self {
        self.prompt_mode = prompt_mode;
        self
    }

    /// Targets an isolated file-based `SecKeychain` (for macOS tests so the real login Keychain is
    /// never touched).
    #[cfg(target_os = "macos")]
    pub fn with_keychain(
        mut self,
        keychain: security_framework::os::macos::keychain::SecKeychain,
    ) -> Self {
        self.reader = Arc::new(MacosKeychainCredentialReader {
            keychain: Some(keychain),
        });
        self
    }

    /// Injects a custom `KeychainCredentialReader` implementation (for deterministic tests).
    pub fn with_reader(mut self, reader: impl KeychainCredentialReader + 'static) -> Self {
        self.reader = Arc::new(reader);
        self
    }

    /// Returns a reference to the underlying `KeychainAccessGate`.
    pub fn gate(&self) -> &Arc<KeychainAccessGate> {
        &self.gate
    }

    /// Returns the active `KeychainPromptMode`.
    pub fn prompt_mode(&self) -> KeychainPromptMode {
        self.prompt_mode
    }

    /// Reads the declared Keychain item using the current wall-clock timestamp for cooldown checks.
    pub fn read(
        &self,
        decl: &KeychainCredentialDecl,
        call: &BrokerCall,
    ) -> Result<KeychainCredentialRead, KeychainCredentialError> {
        self.read_at(decl, call, Timestamp::now().as_second())
    }

    /// Reads the declared Keychain item at `now_seconds`.
    ///
    /// - Background calls (`FetchInteraction::Background`) never trigger a Keychain prompt: they
    ///   execute with `allow_prompt = false` (`kSecUseAuthenticationUISkip` and
    ///   `SecKeychainSetUserInteractionAllowed(false)`), and if the item requires interaction they
    ///   fail with `KeychainCredentialError::InteractionRequired` without recording a denial
    ///   cooldown.
    /// - User-initiated calls (`FetchInteraction::UserInitiated`) check the 6-hour denial cooldown
    ///   first, execute with `allow_prompt = true`, advance `prompt_attempt_generation`, and record
    ///   a 6-hour denial cooldown if the prompt is canceled or denied.
    pub fn read_at(
        &self,
        decl: &KeychainCredentialDecl,
        call: &BrokerCall,
        now_seconds: i64,
    ) -> Result<KeychainCredentialRead, KeychainCredentialError> {
        if call.cancel.is_cancelled() {
            return Err(KeychainCredentialError::Cancelled);
        }
        if self.gate.is_disabled() {
            return Err(KeychainCredentialError::Disabled);
        }
        if self.prompt_mode == KeychainPromptMode::Never {
            return Err(KeychainCredentialError::InteractionRequired);
        }

        match call.interaction {
            FetchInteraction::Background => match self.reader.read_item(decl, false) {
                Ok(read) => validate_keychain_read(read),
                Err(os_status::ERR_SEC_ITEM_NOT_FOUND) => Err(KeychainCredentialError::NotFound),
                Err(
                    os_status::ERR_SEC_INTERACTION_NOT_ALLOWED
                    | os_status::ERR_SEC_AUTH_FAILED
                    | os_status::ERR_SEC_NO_ACCESS_FOR_ITEM
                    | os_status::ERR_SEC_USER_CANCELED,
                ) => Err(KeychainCredentialError::InteractionRequired),
                Err(_) => Err(KeychainCredentialError::Unreadable),
            },
            FetchInteraction::UserInitiated => {
                if !self.gate.should_allow_prompt(decl.service(), now_seconds) {
                    return Err(KeychainCredentialError::CooldownActive);
                }

                let outcome = self.reader.read_item(decl, true);
                let _ = self.gate.record_prompt_attempt_completed();

                match outcome {
                    Ok(read) => validate_keychain_read(read),
                    Err(os_status::ERR_SEC_ITEM_NOT_FOUND) => {
                        Err(KeychainCredentialError::NotFound)
                    }
                    Err(
                        os_status::ERR_SEC_USER_CANCELED
                        | os_status::ERR_SEC_AUTH_FAILED
                        | os_status::ERR_SEC_NO_ACCESS_FOR_ITEM
                        | os_status::ERR_SEC_INTERACTION_NOT_ALLOWED,
                    ) => {
                        self.gate.record_denied(decl.service(), now_seconds);
                        Err(KeychainCredentialError::Dismissed)
                    }
                    Err(_) => Err(KeychainCredentialError::Unreadable),
                }
            }
        }
    }

    /// Probes the item's non-secret attribute fingerprint without requesting secret data or
    /// allowing any UI prompt.
    pub fn probe_fingerprint(
        &self,
        decl: &KeychainCredentialDecl,
        call: &BrokerCall,
    ) -> Result<Option<KeychainItemFingerprint>, KeychainCredentialError> {
        if call.cancel.is_cancelled() {
            return Err(KeychainCredentialError::Cancelled);
        }
        if self.gate.is_disabled() {
            return Err(KeychainCredentialError::Disabled);
        }
        if self.prompt_mode == KeychainPromptMode::Never {
            return Ok(None);
        }

        match self.reader.probe_fingerprint(decl) {
            Ok(fp) => Ok(fp),
            Err(os_status::ERR_SEC_ITEM_NOT_FOUND) => Ok(None),
            Err(
                os_status::ERR_SEC_INTERACTION_NOT_ALLOWED
                | os_status::ERR_SEC_AUTH_FAILED
                | os_status::ERR_SEC_NO_ACCESS_FOR_ITEM
                | os_status::ERR_SEC_USER_CANCELED,
            ) => Err(KeychainCredentialError::InteractionRequired),
            Err(_) => Err(KeychainCredentialError::Unreadable),
        }
    }
}

impl fmt::Debug for KeychainCredentialSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeychainCredentialSource")
            .field("gate", &self.gate)
            .field("prompt_mode", &self.prompt_mode)
            .finish()
    }
}

fn validate_keychain_read(
    read: KeychainCredentialRead,
) -> Result<KeychainCredentialRead, KeychainCredentialError> {
    if (read.bytes.len() as u64) > MAX_CREDENTIAL_FILE_SIZE {
        return Err(KeychainCredentialError::TooLarge);
    }
    Ok(read)
}

#[cfg(target_os = "macos")]
fn default_keychain_reader() -> Arc<dyn KeychainCredentialReader> {
    Arc::new(MacosKeychainCredentialReader { keychain: None })
}

#[cfg(not(target_os = "macos"))]
fn default_keychain_reader() -> Arc<dyn KeychainCredentialReader> {
    Arc::new(UnsupportedKeychainCredentialReader)
}

#[cfg(not(target_os = "macos"))]
struct UnsupportedKeychainCredentialReader;

#[cfg(not(target_os = "macos"))]
impl KeychainCredentialReader for UnsupportedKeychainCredentialReader {
    fn read_item(
        &self,
        _decl: &KeychainCredentialDecl,
        _allow_prompt: bool,
    ) -> Result<KeychainCredentialRead, i32> {
        Err(os_status::ERR_SEC_UNIMPLEMENTED)
    }

    fn probe_fingerprint(
        &self,
        _decl: &KeychainCredentialDecl,
    ) -> Result<Option<KeychainItemFingerprint>, i32> {
        Err(os_status::ERR_SEC_UNIMPLEMENTED)
    }
}

#[cfg(target_os = "macos")]
struct MacosKeychainCredentialReader {
    keychain: Option<security_framework::os::macos::keychain::SecKeychain>,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug)]
struct DiscoveredAttrCandidate {
    account: String,
    modified_at: Option<i64>,
    created_at: Option<i64>,
    attr_digest: String,
}

#[cfg(target_os = "macos")]
impl MacosKeychainCredentialReader {
    fn discover_candidates(
        &self,
        decl: &KeychainCredentialDecl,
        skip_authenticated: bool,
    ) -> Result<Vec<DiscoveredAttrCandidate>, i32> {
        use security_framework::item::{ItemClass, ItemSearchOptions, Limit, SearchResult};

        let mut search = ItemSearchOptions::new();
        search
            .class(ItemClass::generic_password())
            .service(decl.service())
            .limit(Limit::All)
            .load_attributes(true)
            .skip_authenticated_items(skip_authenticated);
        if let Some(account) = decl.account() {
            search.account(account);
        }
        if let Some(kc) = &self.keychain {
            search.keychains(std::slice::from_ref(kc));
        }

        let results = search.search().map_err(|e| e.code())?;
        let mut candidates = Vec::with_capacity(results.len());
        for result in results {
            if let SearchResult::Dict(_) = &result
                && let Some(dict) = result.simplify_dict()
            {
                let account = dict.get("acct").cloned().unwrap_or_default();
                let mdat_raw = dict.get("mdat").cloned().unwrap_or_default();
                let cdat_raw = dict.get("cdat").cloned().unwrap_or_default();
                let modified_at = parse_cfdate_description(&mdat_raw);
                let created_at = parse_cfdate_description(&cdat_raw);
                let digest_input = format!("{}:{account}:{mdat_raw}:{cdat_raw}", decl.service());
                let attr_digest = sha256_prefix(digest_input.as_bytes());
                candidates.push(DiscoveredAttrCandidate {
                    account,
                    modified_at,
                    created_at,
                    attr_digest,
                });
            }
        }

        candidates.sort_by(|left, right| {
            let left_ts = left.modified_at.or(left.created_at).unwrap_or(i64::MIN);
            let right_ts = right.modified_at.or(right.created_at).unwrap_or(i64::MIN);
            right_ts.cmp(&left_ts)
        });
        Ok(candidates)
    }
}

#[cfg(target_os = "macos")]
impl KeychainCredentialReader for MacosKeychainCredentialReader {
    fn read_item(
        &self,
        decl: &KeychainCredentialDecl,
        allow_prompt: bool,
    ) -> Result<KeychainCredentialRead, i32> {
        use security_framework::os::macos::passwords::find_generic_password;

        super::secret_store::service::with_macos_keychain_interaction(allow_prompt, || {
            let candidates = match self.discover_candidates(decl, !allow_prompt) {
                Ok(list) if !list.is_empty() => list,
                Ok(_) | Err(os_status::ERR_SEC_ITEM_NOT_FOUND) => {
                    if !allow_prompt
                        && let Ok(unfiltered) = self.discover_candidates(decl, false)
                        && !unfiltered.is_empty()
                    {
                        return Err(os_status::ERR_SEC_INTERACTION_NOT_ALLOWED);
                    }
                    return Err(os_status::ERR_SEC_ITEM_NOT_FOUND);
                }
                Err(code) => return Err(code),
            };

            let keychains = self.keychain.as_ref().map(std::slice::from_ref);
            let mut last_err = os_status::ERR_SEC_ITEM_NOT_FOUND;

            for candidate in candidates {
                match find_generic_password(keychains, decl.service(), &candidate.account) {
                    Ok((password, _item)) => {
                        let bytes = Zeroizing::new(password.as_ref().to_vec());
                        let persistent_ref_hash = Some(sha256_prefix(&bytes));
                        return Ok(KeychainCredentialRead {
                            bytes,
                            fingerprint: KeychainItemFingerprint {
                                modified_at: candidate.modified_at,
                                created_at: candidate.created_at,
                                persistent_ref_hash,
                            },
                        });
                    }
                    Err(err) => {
                        last_err = err.code();
                        if matches!(
                            last_err,
                            os_status::ERR_SEC_USER_CANCELED
                                | os_status::ERR_SEC_AUTH_FAILED
                                | os_status::ERR_SEC_NO_ACCESS_FOR_ITEM
                                | os_status::ERR_SEC_INTERACTION_NOT_ALLOWED
                        ) {
                            return Err(last_err);
                        }
                    }
                }
            }

            Err(last_err)
        })
    }

    fn probe_fingerprint(
        &self,
        decl: &KeychainCredentialDecl,
    ) -> Result<Option<KeychainItemFingerprint>, i32> {
        super::secret_store::service::with_macos_keychain_interaction(false, || {
            match self.discover_candidates(decl, true) {
                Ok(candidates) => {
                    Ok(candidates
                        .into_iter()
                        .next()
                        .map(|c| KeychainItemFingerprint {
                            modified_at: c.modified_at,
                            created_at: c.created_at,
                            persistent_ref_hash: Some(c.attr_digest),
                        }))
                }
                Err(os_status::ERR_SEC_ITEM_NOT_FOUND) => {
                    if let Ok(unfiltered) = self.discover_candidates(decl, false)
                        && !unfiltered.is_empty()
                    {
                        return Err(os_status::ERR_SEC_INTERACTION_NOT_ALLOWED);
                    }
                    Ok(None)
                }
                Err(code) => Err(code),
            }
        })
    }
}

/// Extracts a Unix timestamp (seconds) from a `CFCopyDescription(CFDate)` string when present
/// (e.g. `"2026-10-24 04:00:00 +0000"`).
#[cfg(target_os = "macos")]
fn parse_cfdate_description(description: &str) -> Option<i64> {
    for token in description.split_whitespace() {
        if token.len() == 10
            && token.as_bytes().get(4) == Some(&b'-')
            && token.as_bytes().get(7) == Some(&b'-')
        {
            let rest = description.get(description.find(token)?..)?;
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 2 {
                let iso_candidate = format!("{}T{}Z", parts[0], parts[1]);
                if let Ok(ts) = iso_candidate.parse::<Timestamp>() {
                    return Some(ts.as_second());
                }
            }
        }
    }
    None
}

/// Reads the declared credential file safely.
///
/// Follows symlinks, opens with `O_RDONLY|O_NONBLOCK|O_NOCTTY|O_CLOEXEC`, checks that the target
/// is a regular file owned by the current EUID, and enforces a size limit of 1 MiB.
pub fn read(
    decl: &CredentialDecl,
    call: &BrokerCall,
) -> Result<CredentialRead, CredentialFileError> {
    read_with_expected_uid(decl, call, None)
}

/// Reads the declared credential file safely with an injected expected UID (for test isolation).
pub fn read_with_expected_uid(
    decl: &CredentialDecl,
    call: &BrokerCall,
    expected_uid: Option<u32>,
) -> Result<CredentialRead, CredentialFileError> {
    if call.cancel.is_cancelled() {
        return Err(CredentialFileError::Cancelled);
    }

    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        let fd = match rustix::fs::open(
            decl.path(),
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::NOCTTY
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT | rustix::io::Errno::NOTDIR) => {
                return Err(CredentialFileError::NotFound);
            }
            Err(_) => return Err(CredentialFileError::Unreadable),
        };

        let stat = match rustix::fs::fstat(&fd) {
            Ok(stat) => stat,
            Err(_) => return Err(CredentialFileError::Unreadable),
        };

        // Must be a regular file (e.g. FIFOs return immediately on nonblocking open and are rejected here).
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile {
            return Err(CredentialFileError::Unreadable);
        }

        // Owner check: must match current EUID or injected expected_uid.
        let required_uid = expected_uid.unwrap_or_else(|| rustix::process::geteuid().as_raw());
        if stat.st_uid != required_uid {
            return Err(CredentialFileError::Untrusted);
        }

        // Size check: must not exceed 1 MiB.
        if (stat.st_size as u64) > MAX_CREDENTIAL_FILE_SIZE {
            return Err(CredentialFileError::TooLarge);
        }

        let mtime_ns = (stat.st_mtime as i128) * 1_000_000_000 + (stat.st_mtime_nsec as i128);
        let stat_fp = StatFingerprint {
            path: decl.path().to_path_buf(),
            dev: stat.st_dev,
            ino: stat.st_ino,
            mtime_ns,
            size: stat.st_size as u64,
        };

        let mut file = std::fs::File::from(fd);
        let mut buffer = Vec::new();
        let mut handle = (&mut file).take(MAX_CREDENTIAL_FILE_SIZE + 1);
        if handle.read_to_end(&mut buffer).is_err() {
            return Err(CredentialFileError::Unreadable);
        }

        if buffer.len() as u64 > MAX_CREDENTIAL_FILE_SIZE {
            return Err(CredentialFileError::TooLarge);
        }

        Ok(CredentialRead {
            bytes: Zeroizing::new(buffer),
            stat: stat_fp,
        })
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (decl, expected_uid);
        Err(CredentialFileError::Unreadable)
    }
}

/// Stores last-seen stat fingerprints per profile digest to detect credential changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastSeen {
    entries: BTreeMap<String, StatFingerprint>,
}

impl LastSeen {
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks if the file changed since last seen for this profile.
    pub fn has_changed(&self, profile_digest: &str, current: &StatFingerprint) -> bool {
        match self.entries.get(profile_digest) {
            Some(stored) => stored != current,
            None => true,
        }
    }

    /// Records the current fingerprint for this profile.
    pub fn record(&mut self, profile_digest: &str, fingerprint: StatFingerprint) {
        self.entries.insert(profile_digest.to_owned(), fingerprint);
    }

    /// Gets the recorded fingerprint for this profile, if any.
    pub fn get(&self, profile_digest: &str) -> Option<&StatFingerprint> {
        self.entries.get(profile_digest)
    }

    /// Returns a reference to all entries.
    pub fn entries(&self) -> &BTreeMap<String, StatFingerprint> {
        &self.entries
    }
}

/// Manages quarantine for profiles whose credential file was proven invalid.
/// Quarantine holds only while the current fingerprint matches the quarantined one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quarantine {
    entries: BTreeMap<String, StatFingerprint>,
}

impl Quarantine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks if this profile's file is currently quarantined.
    /// If the current fingerprint matches stored -> returns `true`.
    /// If the fingerprint changed -> clears quarantine automatically and returns `false`.
    pub fn is_quarantined(&mut self, profile_digest: &str, current: &StatFingerprint) -> bool {
        if let Some(stored) = self.entries.get(profile_digest) {
            if stored == current {
                return true;
            }
            self.entries.remove(profile_digest);
        }
        false
    }

    /// Places a profile under quarantine with its current fingerprint.
    pub fn quarantine(&mut self, profile_digest: &str, fingerprint: StatFingerprint) {
        self.entries.insert(profile_digest.to_owned(), fingerprint);
    }

    /// Manually clears quarantine for a profile.
    pub fn clear(&mut self, profile_digest: &str) {
        self.entries.remove(profile_digest);
    }

    /// Returns a reference to all quarantined entries.
    pub fn entries(&self) -> &BTreeMap<String, StatFingerprint> {
        &self.entries
    }
}

/// Re-reads a declared credential file up to 3 times, 50 ms apart, while the publication
/// predicate `accept` returns false. Cancellation is checked before each attempt.
pub async fn read_with_publication_retry<Accept>(
    decl: &CredentialDecl,
    call: &BrokerCall,
    accept: Accept,
) -> Result<CredentialRead, CredentialFileError>
where
    Accept: Fn(&CredentialRead) -> bool,
{
    read_with_publication_retry_impl(call, || read(decl, call), accept).await
}

/// Generic bounded publication retry helper.
///
/// At most 3 reads, 50 ms apart. If `call` is cancelled, returns `Cancelled` (with 0 reads
/// if cancelled before the first read). Returns the final error if attempts are exhausted.
pub async fn read_with_publication_retry_impl<T, F, Accept>(
    call: &BrokerCall,
    mut read_fn: F,
    accept: Accept,
) -> Result<T, CredentialFileError>
where
    F: FnMut() -> Result<T, CredentialFileError>,
    Accept: Fn(&T) -> bool,
{
    const MAX_ATTEMPTS: usize = 3;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);

    let mut last_err = CredentialFileError::NotFound;

    for attempt in 0..MAX_ATTEMPTS {
        if call.cancel.is_cancelled() {
            return Err(CredentialFileError::Cancelled);
        }

        if attempt > 0 {
            tokio::select! {
                _ = call.cancel.cancelled() => {
                    return Err(CredentialFileError::Cancelled);
                }
                _ = tokio::time::sleep(RETRY_DELAY) => {}
            }
            if call.cancel.is_cancelled() {
                return Err(CredentialFileError::Cancelled);
            }
        }

        match read_fn() {
            Ok(val) => {
                if accept(&val) {
                    return Ok(val);
                } else {
                    last_err = CredentialFileError::Malformed;
                }
            }
            Err(e) => {
                last_err = e;
            }
        }
    }

    Err(last_err)
}
