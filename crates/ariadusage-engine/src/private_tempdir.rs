use std::ops::Deref;
use std::path::{Path, PathBuf};

use crate::brokers::call::BrokerCall;

/// Errors that can occur when creating or accessing a private temporary directory.
#[derive(Debug, thiserror::Error)]
pub enum TempDirError {
    #[error("trusted runtime directory is unavailable or untrusted")]
    Unsupported,

    #[error("i/o error creating private temporary directory: {0}")]
    Io(#[from] std::io::Error),
}

/// A private temporary directory rooted under the trusted runtime directory.
/// Mode 0700 is enforced and all contents are removed on drop, including during panic unwind.
pub struct PrivateTempDir {
    #[allow(dead_code)]
    inner: Option<tempfile::TempDir>,
    #[allow(dead_code)]
    path: PathBuf,
}

impl PrivateTempDir {
    /// Returns the filesystem path to the private temporary directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Deref for PrivateTempDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.path()
    }
}

impl AsRef<Path> for PrivateTempDir {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl std::fmt::Debug for PrivateTempDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateTempDir")
            .field("path_len", &self.path.as_os_str().len())
            .finish()
    }
}

/// Creates a new private temporary directory under the trusted runtime directory.
/// Returns `Err(TempDirError::Unsupported)` if the runtime directory is unset or untrusted.
pub fn create(_call: &BrokerCall) -> Result<PrivateTempDir, TempDirError> {
    #[cfg(target_os = "linux")]
    {
        let runtime_dir = crate::paths::trusted_runtime_dir().ok_or(TempDirError::Unsupported)?;
        create_in(&runtime_dir)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(TempDirError::Unsupported)
    }
}

/// Creates a new private temporary directory under the specified parent directory with mode 0700.
pub fn create_in(parent: &Path) -> Result<PrivateTempDir, TempDirError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let tempdir = tempfile::Builder::new()
            .prefix("ariadusage-tmp-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(parent)?;
        let path = tempdir.path().to_path_buf();
        Ok(PrivateTempDir {
            inner: Some(tempdir),
            path,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        Err(TempDirError::Unsupported)
    }
}

/// Sweeps any leftover `ariadusage-tmp-*` directories from previous crashed runs under the trusted runtime directory.
pub fn sweep_stale() {
    #[cfg(target_os = "linux")]
    if let Some(runtime_dir) = crate::paths::trusted_runtime_dir() {
        sweep_stale_in(&runtime_dir);
    }
}

/// Sweeps any leftover `ariadusage-tmp-*` directories from previous crashed runs under `runtime_dir`.
pub fn sweep_stale_in(runtime_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(runtime_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_target_dir = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.starts_with("ariadusage-tmp-"))
            && entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        if is_target_dir {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}
