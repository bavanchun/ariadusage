// Ported from CodexBar Sources/CodexBarCore/Config/CodexBarConfigStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Maximum byte length of a Unix domain socket `sun_path` buffer on macOS (104 bytes including NUL).
/// Valid socket paths must be strictly shorter than `SUN_PATH_MAX_BYTES` (`< 104` bytes).
pub const SUN_PATH_MAX_BYTES: usize = 104;

/// Relative path of the engine IPC socket inside the trusted runtime directory.
pub const ENGINE_SOCKET_REL_PATH: &str = "ariadusage/engine.sock";

/// Errors encountered while resolving the configuration or runtime socket path.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum PathError {
    #[error("configuration path override in ARIADUSAGE_CONFIG is not absolute: {0}")]
    NonAbsoluteOverride(PathBuf),

    #[error("no home directory available to resolve configuration path")]
    NoHomeDirectory,

    #[error("home directory is not absolute: {0}")]
    NonAbsoluteHome(PathBuf),

    #[error("trusted runtime directory is unavailable or failed ownership/permission checks")]
    UntrustedRuntimeDirectory,

    #[error(
        "engine socket path is {len} bytes, which is not strictly less than the {max}-byte sun_path limit: {path}"
    )]
    SocketPathTooLong {
        path: PathBuf,
        len: usize,
        max: usize,
    },
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
///
/// On Linux, returns `XDG_RUNTIME_DIR` if set and absolute.
/// On macOS, returns `XDG_RUNTIME_DIR` if set and absolute, otherwise falls back to
/// `confstr(_CS_DARWIN_USER_TEMP_DIR)` (canonicalized through `/private/var`).
pub fn runtime_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        resolve_macos_runtime_dir_with(
            |var| std::env::var_os(var),
            etcetera::home_dir().as_deref().ok(),
            darwin_user_temp_dir,
        )
        .map(|p| canonicalize_macos_var_prefix(&p))
    }
    #[cfg(not(target_os = "macos"))]
    {
        resolve_runtime_dir(
            |var| std::env::var_os(var),
            etcetera::home_dir().as_deref().ok(),
        )
    }
}

/// Queries `confstr(_CS_DARWIN_USER_TEMP_DIR)` on macOS via `/usr/bin/getconf DARWIN_USER_TEMP_DIR`.
///
/// Using `/usr/bin/getconf` by absolute path keeps `ariadusage-engine` free of `unsafe` while
/// querying the kernel's per-user Darwin temporary directory directly (ignoring `TMPDIR`).
#[cfg(target_os = "macos")]
pub fn darwin_user_temp_dir() -> Option<PathBuf> {
    use std::sync::OnceLock;
    static DARWIN_TEMP_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DARWIN_TEMP_DIR
        .get_or_init(|| {
            let output = std::process::Command::new("/usr/bin/getconf")
                .arg("DARWIN_USER_TEMP_DIR")
                .env_clear()
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let raw = std::str::from_utf8(&output.stdout).ok()?.trim();
            let trimmed = raw.trim_end_matches('/');
            if trimmed.is_empty() || trimmed == "undefined" {
                return None;
            }
            let path = PathBuf::from(trimmed);
            if path.is_absolute() { Some(path) } else { None }
        })
        .clone()
}

/// Canonicalizes a leading `/var` prefix to `/private/var` on macOS, where `/var` is a
/// system symlink to `private/var`.
pub fn canonicalize_macos_var_prefix(path: &Path) -> PathBuf {
    if path == Path::new("/var") {
        return PathBuf::from("/private/var");
    }
    if let Ok(rest) = path.strip_prefix("/var") {
        return Path::new("/private/var").join(rest);
    }
    path.to_path_buf()
}

/// Resolves the macOS runtime directory from an injected environment lookup, home path, and
/// `confstr(_CS_DARWIN_USER_TEMP_DIR)` provider.
///
/// Priority:
/// 1. `XDG_RUNTIME_DIR` (trimmed, tilde-expanded, must be absolute, relative values ignored).
/// 2. `darwin_temp_dir()` (`confstr(_CS_DARWIN_USER_TEMP_DIR)`, must be absolute).
pub fn resolve_macos_runtime_dir_with(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
    darwin_temp_dir: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(xdg) = resolve_runtime_dir(env, home) {
        return Some(xdg);
    }
    let dir = darwin_temp_dir()?;
    let trimmed = dir.to_str().map(|s| s.trim_end_matches('/'));
    let normalized = match trimmed {
        Some(s) if !s.is_empty() => PathBuf::from(s),
        _ => dir,
    };
    if normalized.is_absolute() {
        Some(normalized)
    } else {
        None
    }
}

/// Validates that `dir` is a trusted macOS runtime directory:
/// - Canonicalizes a leading `/var` prefix to `/private/var`.
/// - Verifies with `lstat` that every path prefix from `/` down to `dir` is a real directory
///   (rejecting any symlink escape at leaf or intermediate components).
/// - Verifies `dir` is owned by `expected_euid` and has exact permissions `0700`.
#[cfg(unix)]
pub fn resolve_macos_trusted_runtime_dir_with(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
    darwin_temp_dir: impl FnOnce() -> Option<PathBuf>,
    expected_euid: u32,
) -> Option<PathBuf> {
    let raw_dir = resolve_macos_runtime_dir_with(env, home, darwin_temp_dir)?;
    let dir = canonicalize_macos_var_prefix(&raw_dir);
    if !dir.is_absolute() {
        return None;
    }

    // Walk every ancestor prefix from `/` down to `dir` using `lstat` to reject any symlink.
    let mut current = PathBuf::new();
    for component in dir.components() {
        match component {
            std::path::Component::RootDir => {
                current.push(Path::new("/"));
            }
            std::path::Component::Normal(part) => {
                current.push(part);
            }
            // Reject CurDir (`.`), ParentDir (`..`), or Prefix components in runtime dir paths.
            _ => return None,
        }
        let stat = rustix::fs::lstat(&current).ok()?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory {
            return None;
        }
    }

    let stat = rustix::fs::lstat(&dir).ok()?;
    if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::Directory {
        return None;
    }
    if stat.st_uid != expected_euid {
        return None;
    }
    if (stat.st_mode & 0o777) != 0o700 {
        return None;
    }

    Some(dir)
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

#[cfg(target_os = "macos")]
pub fn resolve_trusted_runtime_dir_with(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
    _statfs_is_tmpfs: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    resolve_macos_trusted_runtime_dir_with(
        env,
        home,
        darwin_user_temp_dir,
        rustix::process::geteuid().as_raw(),
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn resolve_trusted_runtime_dir_with(
    _env: impl Fn(&str) -> Option<OsString>,
    _home: Option<&Path>,
    _statfs_is_tmpfs: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    None
}

/// Resolves the trusted runtime directory using process environment and OS checks.
///
/// - On Linux: requires `XDG_RUNTIME_DIR` on `tmpfs`, owned by EUID, mode `0700`.
/// - On macOS: uses `XDG_RUNTIME_DIR` when set, otherwise `confstr(_CS_DARWIN_USER_TEMP_DIR)`
///   (canonicalized through `/private/var`), rejecting symlinks and requiring EUID ownership and
///   mode `0700`.
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

#[cfg(target_os = "macos")]
pub fn trusted_runtime_dir() -> Option<PathBuf> {
    resolve_macos_trusted_runtime_dir_with(
        |var| std::env::var_os(var),
        etcetera::home_dir().as_deref().ok(),
        darwin_user_temp_dir,
        rustix::process::geteuid().as_raw(),
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn trusted_runtime_dir() -> Option<PathBuf> {
    None
}

/// Validates that `socket_path` is strictly shorter than `SUN_PATH_MAX_BYTES` (`< 104` bytes).
pub fn validate_socket_path_len(socket_path: &Path) -> Result<(), PathError> {
    #[cfg(unix)]
    let len = {
        use std::os::unix::ffi::OsStrExt;
        socket_path.as_os_str().as_bytes().len()
    };
    #[cfg(not(unix))]
    let len = socket_path.as_os_str().len();

    if len >= SUN_PATH_MAX_BYTES {
        return Err(PathError::SocketPathTooLong {
            path: socket_path.to_path_buf(),
            len,
            max: SUN_PATH_MAX_BYTES,
        });
    }
    Ok(())
}

/// Resolves `<runtime_dir>/ariadusage/engine.sock` and enforces the `< 104`-byte `sun_path` limit.
pub fn resolve_socket_path_from_runtime_dir(runtime_dir: &Path) -> Result<PathBuf, PathError> {
    let sock = runtime_dir.join(ENGINE_SOCKET_REL_PATH);
    validate_socket_path_len(&sock)?;
    Ok(sock)
}

/// Resolves the engine Unix domain socket path inside the trusted runtime directory and enforces
/// the `< 104`-byte `sun_path` limit.
pub fn socket_path() -> Result<PathBuf, PathError> {
    let runtime_dir = trusted_runtime_dir().ok_or(PathError::UntrustedRuntimeDirectory)?;
    resolve_socket_path_from_runtime_dir(&runtime_dir)
}
