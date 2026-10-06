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
