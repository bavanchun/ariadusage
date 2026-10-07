// Ported from CodexBar Sources/CodexBarCore/Config/CodexBarConfigStore.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Errors encountered while resolving the configuration file path.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum PathError {
    #[error("configuration path override in ARIADUSAGE_CONFIG is not absolute: {0}")]
    NonAbsoluteOverride(PathBuf),

    #[error("no home directory available to resolve configuration path")]
    NoHomeDirectory,

    #[error("home directory is not absolute: {0}")]
    NonAbsoluteHome(PathBuf),
}

/// Expands a leading `~` or `~/` to the provided home directory if available.
fn expand_tilde(raw: &str, home: Option<&Path>) -> PathBuf {
    if raw == "~" {
        if let Some(h) = home {
            return h.to_path_buf();
        }
        return PathBuf::from(raw);
    }

    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(h) = home {
            return h.join(rest);
        }
        return PathBuf::from(raw);
    }

    #[cfg(windows)]
    if let Some(rest) = raw.strip_prefix(r"~\") {
        if let Some(h) = home {
            return h.join(rest);
        }
        return PathBuf::from(raw);
    }

    PathBuf::from(raw)
}

/// Resolves the configuration file path from an injected environment lookup and home path.
///
/// Priority:
/// 1. `ARIADUSAGE_CONFIG` (trimmed, tilde-expanded, must be absolute, otherwise an error).
/// 2. `XDG_CONFIG_HOME` (trimmed, tilde-expanded, must be absolute, relative values ignored)
///    joined with `ariadusage/config.json`.
/// 3. Fallback: `$HOME/.config/ariadusage/config.json`.
pub fn resolve_config_path(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Result<PathBuf, PathError> {
    if let Some(s) = env("ARIADUSAGE_CONFIG").as_deref().and_then(|v| v.to_str()) {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            let expanded = expand_tilde(trimmed, home);
            if expanded.is_absolute() {
                return Ok(expanded);
            }
            return Err(PathError::NonAbsoluteOverride(expanded));
        }
    }

    if let Some(s) = env("XDG_CONFIG_HOME").as_deref().and_then(|v| v.to_str()) {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            let expanded = expand_tilde(trimmed, home);
            if expanded.is_absolute() {
                return Ok(expanded.join("ariadusage").join("config.json"));
            }
        }
    }

    let home = home.ok_or(PathError::NoHomeDirectory)?;
    if !home.is_absolute() {
        return Err(PathError::NonAbsoluteHome(home.to_path_buf()));
    }

    Ok(home.join(".config").join("ariadusage").join("config.json"))
}

/// Resolves the configuration path using the real process environment and home directory.
pub fn config_path() -> Result<PathBuf, PathError> {
    resolve_config_path(
        |var| std::env::var_os(var),
        etcetera::home_dir().as_deref().ok(),
    )
}

/// Resolves the XDG state directory from an injected environment lookup and home path.
///
/// Priority:
/// 1. `XDG_STATE_HOME` (trimmed, tilde-expanded, must be absolute, relative values ignored).
/// 2. Fallback: `$HOME/.local/state`.
pub fn resolve_state_dir(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Result<PathBuf, PathError> {
    if let Some(s) = env("XDG_STATE_HOME").as_deref().and_then(|v| v.to_str()) {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            let expanded = expand_tilde(trimmed, home);
            if expanded.is_absolute() {
                return Ok(expanded);
            }
        }
    }

    let home = home.ok_or(PathError::NoHomeDirectory)?;
    if !home.is_absolute() {
        return Err(PathError::NonAbsoluteHome(home.to_path_buf()));
    }

    Ok(home.join(".local").join("state"))
}

/// Resolves the state directory using the real process environment and home directory.
pub fn state_dir() -> Result<PathBuf, PathError> {
    resolve_state_dir(
        |var| std::env::var_os(var),
        etcetera::home_dir().as_deref().ok(),
    )
}

/// Resolves the XDG data directory from an injected environment lookup and home path.
///
/// Priority:
/// 1. `XDG_DATA_HOME` (trimmed, tilde-expanded, must be absolute, relative values ignored).
/// 2. Fallback: `$HOME/.local/share`.
pub fn resolve_data_dir(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Result<PathBuf, PathError> {
    if let Some(s) = env("XDG_DATA_HOME").as_deref().and_then(|v| v.to_str()) {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            let expanded = expand_tilde(trimmed, home);
            if expanded.is_absolute() {
                return Ok(expanded);
            }
        }
    }

    let home = home.ok_or(PathError::NoHomeDirectory)?;
    if !home.is_absolute() {
        return Err(PathError::NonAbsoluteHome(home.to_path_buf()));
    }

    Ok(home.join(".local").join("share"))
}

/// Resolves the data directory using the real process environment and home directory.
pub fn data_dir() -> Result<PathBuf, PathError> {
    resolve_data_dir(
        |var| std::env::var_os(var),
        etcetera::home_dir().as_deref().ok(),
    )
}

/// Resolves the XDG runtime directory from an injected environment lookup and home path.
///
/// Priority:
/// 1. `XDG_RUNTIME_DIR` (trimmed, tilde-expanded, must be absolute, relative values ignored).
/// 2. Unset / relative -> `None` (the runtime directory has no default).
pub fn resolve_runtime_dir(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    let s = env("XDG_RUNTIME_DIR")?;
    let s_str = s.to_str()?;
    let trimmed = s_str.trim();
    if trimmed.is_empty() {
        return None;
    }
    let expanded = expand_tilde(trimmed, home);
    if expanded.is_absolute() {
        Some(expanded)
    } else {
        None
    }
}

/// Resolves the runtime directory using the real process environment and home directory.
pub fn runtime_dir() -> Option<PathBuf> {
    resolve_runtime_dir(
        |var| std::env::var_os(var),
        etcetera::home_dir().as_deref().ok(),
    )
}

/// Linux tmpfs magic constant (0x01021994).
#[cfg(target_os = "linux")]
pub const TMPFS_MAGIC: u64 = 0x0102_1994;

/// Validates that `dir` is trusted: exists, is a directory, owned by euid, mode 0700,
/// and resides on a tmpfs filesystem according to `statfs_is_tmpfs`.
#[cfg(target_os = "linux")]
pub fn resolve_trusted_runtime_dir_with(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
    statfs_is_tmpfs: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let dir = resolve_runtime_dir(env, home)?;

    // Directory must exist and be accessible.
    let stat = rustix::fs::stat(&dir).ok()?;

    // Must be a directory.
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory {
        return None;
    }

    // Must be owned by current EUID.
    let euid = rustix::process::geteuid().as_raw();
    if stat.st_uid != euid {
        return None;
    }

    // Must have exact mode 0700 (owner rwx, no group, no other).
    if (stat.st_mode & 0o777) != 0o700 {
        return None;
    }

    // Must be on a tmpfs filesystem.
    if !statfs_is_tmpfs(&dir) {
        return None;
    }

    Some(dir)
}

#[cfg(not(target_os = "linux"))]
pub fn resolve_trusted_runtime_dir_with(
    _env: impl Fn(&str) -> Option<OsString>,
    _home: Option<&Path>,
    _statfs_is_tmpfs: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    None
}

/// Resolves the trusted runtime directory using process environment, EUID, and kernel statfs.
/// Returns `None` if unset, untrusted, or not running on Linux.
#[cfg(target_os = "linux")]
pub fn trusted_runtime_dir() -> Option<PathBuf> {
    resolve_trusted_runtime_dir_with(
        |var| std::env::var_os(var),
        etcetera::home_dir().as_deref().ok(),
        |path| {
            rustix::fs::statfs(path)
                .map(|s| (s.f_type as u64) == TMPFS_MAGIC)
                .unwrap_or(false)
        },
    )
}

#[cfg(not(target_os = "linux"))]
pub fn trusted_runtime_dir() -> Option<PathBuf> {
    None
}
