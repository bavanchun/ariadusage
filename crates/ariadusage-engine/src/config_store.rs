// Ported from CodexBar Sources/CodexBarCore/Config/CodexBarConfigStore.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::path::{Path, PathBuf};

use ariadusage_core::config::Config;

use crate::error::StoreError;
use crate::private_file::WriteHooks;

/// Result of an opportunistic configuration update via `try_update`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateResult {
    Updated,
    Skipped,
}

/// Persistent configuration store handling path resolution, trust checks, file locking,
/// atomic private writes, and codec normalization.
pub struct ConfigStore {
    path: PathBuf,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    hooks: WriteHooks,
}

impl ConfigStore {
    /// Creates a new store for the given configuration path.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            hooks: WriteHooks::default(),
        }
    }

    /// Creates a new store with custom write hooks for testing.
    #[cfg(feature = "test-hooks")]
    pub fn with_write_hooks(path: PathBuf, hooks: WriteHooks) -> Self {
        Self { path, hooks }
    }

    /// Returns the configuration file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads and decodes configuration if present and non-blank.
    /// Returns `Ok(None)` if absent or blank. Fails closed without rewriting on decode error.
    pub fn load(&self) -> Result<Option<Config>, StoreError> {
        #[cfg(target_os = "linux")]
        {
            if let Some(parent) = self.path.parent()
                && parent.exists()
            {
                crate::trust::check_parent_trust(parent, crate::trust::TrustPolicy::CONFIG)?;
            }

            let Some(fd) =
                crate::trust::check_file_trust(&self.path, crate::trust::TrustPolicy::CONFIG)?
            else {
                return Ok(None);
            };

            use std::io::Read;
            let mut file = std::fs::File::from(fd);
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;

            match ariadusage_core::config::decode(&bytes)? {
                Some(cfg) => Ok(Some(ariadusage_core::config::normalize(cfg))),
                None => Ok(None),
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            if !self.path.exists() {
                return Ok(None);
            }
            let bytes = std::fs::read(&self.path)?;
            match ariadusage_core::config::decode(&bytes)? {
                Some(cfg) => Ok(Some(ariadusage_core::config::normalize(cfg))),
                None => Ok(None),
            }
        }
    }

    /// Returns the current normalized configuration if present, or the default normalized
    /// configuration if absent or blank, **without writing anything to disk**.
    pub fn load_effective(&self) -> Result<Config, StoreError> {
        if let Some(cfg) = self.load()? {
            Ok(cfg)
        } else {
            Ok(default_config())
        }
    }

    /// Loads the configuration, or writes and returns the default configuration if absent or blank.
    pub fn load_or_default(&self) -> Result<Config, StoreError> {
        #[cfg(not(target_os = "linux"))]
        {
            Err(StoreError::Unsupported(
                "configuration write is unsupported on this platform",
            ))
        }

        #[cfg(target_os = "linux")]
        {
            self.with_write_lock(true, || {
                if let Some(cfg) = self.load()? {
                    Ok(cfg)
                } else {
                    let default_cfg = default_config();
                    self.save_under_lock(&default_cfg)?;
                    Ok(default_cfg)
                }
            })
        }
    }

    /// Normalizes and saves `config` to disk under the write lock.
    pub fn save(&self, config: &Config) -> Result<(), StoreError> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = config;
            Err(StoreError::Unsupported(
                "configuration write is unsupported on this platform",
            ))
        }

        #[cfg(target_os = "linux")]
        {
            self.with_write_lock(true, || self.save_under_lock(config))
        }
    }

    /// Acquires the write lock, loads current effective config, executes `f`, and saves the result.
    pub fn update(&self, f: impl FnOnce(&mut Config)) -> Result<(), StoreError> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = f;
            Err(StoreError::Unsupported(
                "configuration write is unsupported on this platform",
            ))
        }

        #[cfg(target_os = "linux")]
        {
            self.with_write_lock(true, || {
                let mut cfg = self.load_effective()?;
                f(&mut cfg);
                self.save_under_lock(&cfg)
            })
        }
    }

    /// Opportunistically acquires the write lock without blocking.
    /// If contention occurs, returns `Ok(UpdateResult::Skipped)`.
    pub fn try_update(
        &self,
        f: impl FnOnce(&mut Config) -> bool,
    ) -> Result<UpdateResult, StoreError> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = f;
            Err(StoreError::Unsupported(
                "configuration write is unsupported on this platform",
            ))
        }

        #[cfg(target_os = "linux")]
        {
            let res = self.with_write_lock(false, || {
                if let Some(mut cfg) = self.load()?
                    && f(&mut cfg)
                {
                    self.save_under_lock(&cfg)?;
                    return Ok(UpdateResult::Updated);
                }
                Ok(UpdateResult::Skipped)
            });

            match res {
                Ok(r) => Ok(r),
                Err(StoreError::LockContention) => Ok(UpdateResult::Skipped),
                Err(e) => Err(e),
            }
        }
    }

    /// Deletes the configuration file under the write lock if it exists.
    /// No-op if absent. The lock file is never unlinked.
    pub fn delete_if_present(&self) -> Result<(), StoreError> {
        if !self.path.exists() {
            return Ok(());
        }

        #[cfg(not(target_os = "linux"))]
        {
            Err(StoreError::Unsupported(
                "configuration delete is unsupported on this platform",
            ))
        }

        #[cfg(target_os = "linux")]
        {
            self.with_write_lock(true, || {
                if self.path.exists() {
                    let _ = crate::trust::check_file_trust(
                        &self.path,
                        crate::trust::TrustPolicy::CONFIG,
                    )?;
                    std::fs::remove_file(&self.path)?;
                }
                Ok(())
            })
        }
    }

    #[cfg(target_os = "linux")]
    fn save_under_lock(&self, config: &Config) -> Result<(), StoreError> {
        if self.path.exists() {
            let _ = crate::trust::check_file_trust(&self.path, crate::trust::TrustPolicy::CONFIG)?;
        }
        let normalized = ariadusage_core::config::normalize(config.clone());
        let mut bytes = ariadusage_core::config::encode(&normalized);
        bytes.push(b'\n');
        crate::private_file::write_private(&self.path, &bytes, &self.hooks)
    }

    #[cfg(target_os = "linux")]
    fn with_write_lock<R>(
        &self,
        wait: bool,
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

        crate::trust::check_parent_trust(parent, crate::trust::TrustPolicy::CONFIG)?;

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
        if wait {
            lock_file.lock()?;
        } else {
            match lock_file.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(StoreError::LockContention);
                }
                Err(e) => return Err(StoreError::Io(e.into())),
            }
        }

        body()
    }
}

fn default_config() -> Config {
    ariadusage_core::config::normalize(Config::new(Config::CURRENT_VERSION, Vec::new()))
}
