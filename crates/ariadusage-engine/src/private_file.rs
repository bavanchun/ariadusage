// Ported from CodexBar Sources/CodexBarCore/CredentialFileWriter.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::path::Path;
use std::sync::Arc;

use crate::error::StoreError;

/// Hook callback type for testing stages of private file writing.
pub type HookFn = Arc<dyn Fn(&Path) -> Result<(), StoreError> + Send + Sync>;

/// Test hooks injected into private file writing.
#[derive(Clone, Default)]
pub struct WriteHooks {
    pub before_write: Option<HookFn>,
    pub before_publish: Option<HookFn>,
}

/// Atomically writes `bytes` to `path` with private permissions (0600) via a staging directory (0700).
#[cfg(target_os = "linux")]
pub fn write_private(path: &Path, bytes: &[u8], hooks: &WriteHooks) -> Result<(), StoreError> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

    let parent = path.parent().ok_or_else(|| {
        StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path has no parent directory",
        ))
    })?;

    let file_name = path.file_name().ok_or_else(|| {
        StoreError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path has no file name",
        ))
    })?;

    // Create parent directory with 0700 if missing.
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    builder.mode(0o700);
    builder.create(parent)?;

    // Create staging directory beside target with 0700 permissions.
    let staging_dir = tempfile::Builder::new()
        .prefix(".ariadusage-staged-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)?;
    std::fs::set_permissions(staging_dir.path(), std::fs::Permissions::from_mode(0o700))?;

    let staged_path = staging_dir.path().join(file_name);

    // Open staged file with O_CREAT | O_EXCL and 0600 mode.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged_path)?;

    // Explicitly fchmod the open descriptor to 0600 before the first byte.
    rustix::fs::fchmod(&file, rustix::fs::Mode::from_bits_retain(0o600))
        .map_err(std::io::Error::from)?;

    // Run before_write while the staged file is empty.
    if let Some(hook) = &hooks.before_write {
        hook(&staged_path)?;
    }

    // Write content, fsync, and close.
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    // Run before_publish after contents are synced and before rename.
    if let Some(hook) = &hooks.before_publish {
        hook(&staged_path)?;
    }

    // Atomic rename over target.
    std::fs::rename(&staged_path, path)?;

    // fsync the parent directory after rename.
    let parent_dir = std::fs::File::open(parent)?;
    parent_dir.sync_all()?;

    Ok(())
}

/// Atomically writes `bytes` to `path`. Unsupported outside Linux.
#[cfg(not(target_os = "linux"))]
pub fn write_private(_path: &Path, _bytes: &[u8], _hooks: &WriteHooks) -> Result<(), StoreError> {
    Err(StoreError::Unsupported(
        "private file writing is unsupported on this platform",
    ))
}

/// If `path` exists, is a regular file owned by the current user, and has group/other permissions,
/// tightens permissions to `0600`. Safely skips symlinks via `O_NOFOLLOW`.
#[cfg(target_os = "linux")]
pub fn repair_permissions(path: &Path) {
    let Ok(fd) = rustix::fs::open(
        path,
        rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    ) else {
        return;
    };

    let Ok(stat) = rustix::fs::fstat(&fd) else {
        return;
    };

    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile {
        return;
    }

    if stat.st_uid != rustix::process::geteuid().as_raw() {
        return;
    }

    if (stat.st_mode & 0o077) != 0 {
        let _ = rustix::fs::fchmod(&fd, rustix::fs::Mode::from_bits_retain(0o600));
    }
}

/// Permissions repair is a no-op outside Linux.
#[cfg(not(target_os = "linux"))]
pub fn repair_permissions(_path: &Path) {}
