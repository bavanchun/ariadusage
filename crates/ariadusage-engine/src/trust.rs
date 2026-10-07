// Ported from CodexBar Sources/CodexBarCore/Config/CodexBarConfigStore.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::path::Path;

use crate::error::StoreError;

/// Trust policy governing permitted filesystem permissions on parent directories and files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustPolicy {
    /// Disallows group- or world-writable bits (`mode & 0o022 != 0`).
    pub forbid_group_other_write: bool,
    /// Disallows any group or other bits (`mode & 0o077 != 0`).
    pub forbid_group_other_any: bool,
}

impl TrustPolicy {
    /// Policy for configuration files: allows group/other read (e.g. 0644, 0755), forbids group/other write.
    pub const CONFIG: Self = Self {
        forbid_group_other_write: true,
        forbid_group_other_any: false,
    };

    /// Policy for private files (e.g. broker-state.json, secrets.json): forbids any group or other permissions (0600, 0700).
    pub const PRIVATE: Self = Self {
        forbid_group_other_write: true,
        forbid_group_other_any: true,
    };
}

/// Checks that `parent` directory is owned by the current EUID and adheres to `policy`.
#[cfg(unix)]
pub fn check_parent_trust(parent: &Path, policy: TrustPolicy) -> Result<(), StoreError> {
    let stat = rustix::fs::stat(parent).map_err(std::io::Error::from)?;
    let euid = rustix::process::geteuid().as_raw();
    if stat.st_uid != euid {
        return Err(StoreError::UntrustedDirectory(format!(
            "parent directory {:?} is owned by UID {}, expected {}",
            parent, stat.st_uid, euid
        )));
    }
    if policy.forbid_group_other_any && (stat.st_mode & 0o077) != 0 {
        return Err(StoreError::UntrustedDirectory(format!(
            "parent directory {:?} has mode {:04o}, must not have group or other permissions (required mode 0700)",
            parent,
            stat.st_mode & 0o777
        )));
    } else if policy.forbid_group_other_write && (stat.st_mode & 0o022) != 0 {
        return Err(StoreError::UntrustedDirectory(format!(
            "parent directory {:?} has mode {:04o}, must not be group- or world-writable (required mode 0700 or at most 0755)",
            parent,
            stat.st_mode & 0o777
        )));
    }
    Ok(())
}

/// Checks that `path` is a regular file owned by the current EUID, is not a symlink, and adheres to `policy`.
/// Returns `Ok(Some(fd))` if the file exists and is trusted, or `Ok(None)` if it does not exist.
#[cfg(unix)]
pub fn check_file_trust(
    path: &Path,
    policy: TrustPolicy,
) -> Result<Option<rustix::fd::OwnedFd>, StoreError> {
    let fd = match rustix::fs::open(
        path,
        rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(rustix::io::Errno::LOOP) => {
            return Err(StoreError::UntrustedFile(format!(
                "config file {:?} is a symlink",
                path
            )));
        }
        Err(e) => return Err(StoreError::Io(e.into())),
    };

    let stat = rustix::fs::fstat(&fd).map_err(std::io::Error::from)?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile {
        return Err(StoreError::UntrustedFile(format!(
            "config file {:?} is not a regular file",
            path
        )));
    }
    let euid = rustix::process::geteuid().as_raw();
    if stat.st_uid != euid {
        return Err(StoreError::UntrustedFile(format!(
            "config file {:?} is owned by UID {}, expected {}",
            path, stat.st_uid, euid
        )));
    }
    if policy.forbid_group_other_any && (stat.st_mode & 0o077) != 0 {
        return Err(StoreError::UntrustedFile(format!(
            "config file {:?} has group or other permissions (mode {:04o})",
            path,
            stat.st_mode & 0o777
        )));
    } else if policy.forbid_group_other_write && (stat.st_mode & 0o022) != 0 {
        return Err(StoreError::UntrustedFile(format!(
            "config file {:?} is group- or world-writable (mode {:04o})",
            path,
            stat.st_mode & 0o777
        )));
    }

    Ok(Some(fd))
}

/// On non-Unix platforms, parent directory trust is a no-op.
#[cfg(not(unix))]
pub fn check_parent_trust(_parent: &Path, _policy: TrustPolicy) -> Result<(), StoreError> {
    Ok(())
}
