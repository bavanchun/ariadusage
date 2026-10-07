// Ported from CodexBar Sources/CodexBarCore/ProviderSessionStoreFile.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use ariadusage_core::gates::delegated_cooldown::CooldownState;
use serde::{Deserialize, Serialize};

use crate::brokers::credential_file::StatFingerprint;
use crate::error::StoreError;
#[cfg(target_os = "linux")]
use crate::private_file::{WriteHooks, repair_permissions, write_private};
#[cfg(target_os = "linux")]
use crate::trust::{TrustPolicy, check_file_trust, check_parent_trust};

/// Warnings recorded during broker state store operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateWarning {
    /// File permissions were looser than 0600 and were tightened to 0600.
    PermissionsRepaired,
    /// State file content was malformed/corrupted and was reset to empty state.
    CorruptStateReset,
    /// State file has an unknown schema version and was reset to empty state.
    UnknownVersionReset(u32),
}

/// Persistent broker state tracking digests and timestamps only.
///
/// Contains no raw tokens, passwords, or plaintext user paths.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrokerState {
    pub schema_version: u32,
    #[serde(default)]
    pub last_seen: BTreeMap<String, StatFingerprint>,
    #[serde(default)]
    pub quarantine: BTreeMap<String, StatFingerprint>,
    #[serde(default)]
    pub delegated_cooldowns: BTreeMap<String, CooldownState>,
}

impl BrokerState {
    /// Current supported schema version.
    pub const CURRENT_SCHEMA_VERSION: u32 = 1;

    /// Creates an empty broker state with the current schema version.
    pub fn empty() -> Self {
        Self {
            schema_version: Self::CURRENT_SCHEMA_VERSION,
            last_seen: BTreeMap::new(),
            quarantine: BTreeMap::new(),
            delegated_cooldowns: BTreeMap::new(),
        }
    }
}

/// Thread-safe and process-safe persistent store for broker state.
pub struct BrokerStateStore {
    path: PathBuf,
    warning_count: AtomicUsize,
    warnings: Mutex<Vec<StateWarning>>,
    #[cfg(target_os = "linux")]
    hooks: WriteHooks,
}

impl BrokerStateStore {
    /// Creates a new `BrokerStateStore` targeting `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            warning_count: AtomicUsize::new(0),
            warnings: Mutex::new(Vec::new()),
            #[cfg(target_os = "linux")]
            hooks: WriteHooks::default(),
        }
    }

    /// Resolves the default broker state path (`$XDG_STATE_HOME/ariadusage/broker-state.json`).
    pub fn from_default_path() -> Result<Self, crate::paths::PathError> {
        let state_dir = crate::paths::state_dir()?;
        let path = state_dir.join("ariadusage").join("broker-state.json");
        Ok(Self::new(path))
    }

    /// Path to the broker state file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the total count of warnings recorded across all operations.
    pub fn warning_count(&self) -> usize {
        self.warning_count.load(Ordering::SeqCst)
    }

    /// Returns a copy of all recorded warnings.
    pub fn warnings(&self) -> Vec<StateWarning> {
        self.warnings.lock().unwrap().clone()
    }

    #[cfg(target_os = "linux")]
    fn record_warning(&self, warning: StateWarning) {
        self.warning_count.fetch_add(1, Ordering::SeqCst);
        self.warnings.lock().unwrap().push(warning);
    }

    /// Loads the current `BrokerState`.
    ///
    /// If the file does not exist, returns `BrokerState::empty()`.
    /// If permissions are looser than 0600 on Unix, repairs them and records a warning.
    /// If the file is corrupt or has an unknown schema version, resets to empty and records a warning.
    pub fn load(&self) -> Result<BrokerState, StoreError> {
        #[cfg(target_os = "linux")]
        {
            if let Some(parent) = self.path.parent()
                && parent.exists()
            {
                check_parent_trust(parent, TrustPolicy::CONFIG)?;
            }

            if !self.path.exists() {
                return Ok(BrokerState::empty());
            }

            // Inspect permissions: if looser than 0600, repair and record counted warning.
            if let Ok(meta) = std::fs::symlink_metadata(&self.path) {
                use std::os::unix::fs::PermissionsExt;
                if (meta.permissions().mode() & 0o077) != 0 {
                    repair_permissions(&self.path);
                    self.record_warning(StateWarning::PermissionsRepaired);
                }
            }

            let Some(mut file) = check_file_trust(&self.path, TrustPolicy::PRIVATE)? else {
                return Ok(BrokerState::empty());
            };

            use std::io::Read;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;

            if bytes.is_empty() {
                return Ok(BrokerState::empty());
            }

            let state: BrokerState = match serde_json::from_slice(&bytes) {
                Ok(s) => s,
                Err(_) => {
                    self.record_warning(StateWarning::CorruptStateReset);
                    return Ok(BrokerState::empty());
                }
            };

            if state.schema_version != BrokerState::CURRENT_SCHEMA_VERSION {
                self.record_warning(StateWarning::UnknownVersionReset(state.schema_version));
                return Ok(BrokerState::empty());
            }

            Ok(state)
        }

        #[cfg(not(target_os = "linux"))]
        {
            Err(StoreError::Unsupported(
                "broker state is unsupported on this platform",
            ))
        }
    }

    /// Saves `state` to disk atomically with 0600 permissions under a file lock.
    pub fn save(&self, state: &BrokerState) -> Result<(), StoreError> {
        #[cfg(target_os = "linux")]
        {
            self.with_write_lock(|| {
                let mut bytes = serde_json::to_vec_pretty(state).map_err(|e| {
                    StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                })?;
                bytes.push(b'\n');
                write_private(&self.path, &bytes, &self.hooks)
            })
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = state;
            Err(StoreError::Unsupported(
                "broker state is unsupported on this platform",
            ))
        }
    }

    /// Acquires the file lock, loads current state, executes `f`, and saves under the lock.
    pub fn update<R>(&self, f: impl FnOnce(&mut BrokerState) -> R) -> Result<R, StoreError> {
        #[cfg(target_os = "linux")]
        {
            self.with_write_lock(|| {
                let mut state = self.load()?;
                let result = f(&mut state);
                let mut bytes = serde_json::to_vec_pretty(&state).map_err(|e| {
                    StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
                })?;
                bytes.push(b'\n');
                write_private(&self.path, &bytes, &self.hooks)?;
                Ok(result)
            })
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = f;
            Err(StoreError::Unsupported(
                "broker state is unsupported on this platform",
            ))
        }
    }

    #[cfg(target_os = "linux")]
    fn with_write_lock<R>(
        &self,
        body: impl FnOnce() -> Result<R, StoreError>,
    ) -> Result<R, StoreError> {
        use std::os::unix::fs::DirBuilderExt;

        let parent = self.path.parent().ok_or_else(|| {
            StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path has no parent directory",
            ))
        })?;

        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        builder.mode(0o700);
        builder.create(parent)?;

        check_parent_trust(parent, TrustPolicy::CONFIG)?;

        let lock_name = format!(
            "{}.lock",
            self.path.file_name().unwrap_or_default().to_string_lossy()
        );
        let lock_path = parent.join(lock_name);

        let fd = match rustix::fs::open(
            &lock_path,
            rustix::fs::OFlags::CREATE
                | rustix::fs::OFlags::RDWR
                | rustix::fs::OFlags::CLOEXEC
                | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::from_bits_retain(0o600),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::LOOP) => {
                return Err(StoreError::LockRejected(format!(
                    "lock file {:?} is a symlink",
                    lock_path
                )));
            }
            Err(e) => return Err(StoreError::Io(e.into())),
        };

        let stat = rustix::fs::fstat(&fd).map_err(std::io::Error::from)?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile {
            return Err(StoreError::LockRejected(format!(
                "lock file {:?} is not a regular file",
                lock_path
            )));
        }
        let euid = rustix::process::geteuid().as_raw();
        if stat.st_uid != euid {
            return Err(StoreError::LockRejected(format!(
                "lock file {:?} is owned by UID {}, expected {}",
                lock_path, stat.st_uid, euid
            )));
        }

        let lock_file = std::fs::File::from(fd);
        lock_file.lock()?;

        body()
    }
}
