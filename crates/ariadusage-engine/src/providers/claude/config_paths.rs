// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeConfigPaths.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use ariadusage_protocol::secret::SecretString;

pub const CLAUDE_CONFIG_DIR_ENV_KEY: &str = "CLAUDE_CONFIG_DIR";
pub const CLAUDE_SECURE_STORAGE_CONFIG_DIR_ENV_KEY: &str = "CLAUDE_SECURESTORAGE_CONFIG_DIR";

/// Pure resolver for Claude Code configuration and credential paths.
pub struct ClaudeConfigPaths;

impl ClaudeConfigPaths {
    /// Resolves the Claude configuration root directory (`CLAUDE_CONFIG_DIR` or `<HOME>/.claude`).
    pub fn config_root(
        environment: &BTreeMap<String, String>,
        working_directory: &Path,
        home_directory: &Path,
    ) -> PathBuf {
        Self::config_root_with_lookup(
            |key| environment.get(key).map(String::as_str),
            working_directory,
            home_directory,
        )
    }

    /// Resolves the Claude configuration root directory from a `SecretString` environment map.
    pub fn config_root_from_secret_env(
        environment: &BTreeMap<String, SecretString>,
        working_directory: &Path,
        home_directory: &Path,
    ) -> PathBuf {
        Self::config_root_with_lookup(
            |key| environment.get(key).map(SecretString::expose_secret),
            working_directory,
            home_directory,
        )
    }

    /// Resolves the Claude account configuration file path (`.config.json` or `.claude.json`).
    pub fn account_config_path(
        environment: &BTreeMap<String, String>,
        working_directory: &Path,
        home_directory: &Path,
    ) -> PathBuf {
        Self::account_config_path_with_lookup(
            |key| environment.get(key).map(String::as_str),
            working_directory,
            home_directory,
        )
    }

    /// Resolves the Claude account configuration file path from a `SecretString` environment map.
    pub fn account_config_path_from_secret_env(
        environment: &BTreeMap<String, SecretString>,
        working_directory: &Path,
        home_directory: &Path,
    ) -> PathBuf {
        Self::account_config_path_with_lookup(
            |key| environment.get(key).map(SecretString::expose_secret),
            working_directory,
            home_directory,
        )
    }

    /// Resolves the Claude OAuth `.credentials.json` file path.
    pub fn credentials_path(
        environment: &BTreeMap<String, String>,
        working_directory: &Path,
        home_directory: &Path,
    ) -> PathBuf {
        Self::credentials_path_with_lookup(
            |key| environment.get(key).map(String::as_str),
            working_directory,
            home_directory,
        )
    }

    /// Resolves the Claude OAuth `.credentials.json` file path from a `SecretString` environment map.
    pub fn credentials_path_from_secret_env(
        environment: &BTreeMap<String, SecretString>,
        working_directory: &Path,
        home_directory: &Path,
    ) -> PathBuf {
        Self::credentials_path_with_lookup(
            |key| environment.get(key).map(SecretString::expose_secret),
            working_directory,
            home_directory,
        )
    }

    /// Resolves a raw directory path against `working_directory` when it does not start with `/`.
    ///
    /// Follows Claude Code's Node `path.resolve(...)` behavior: only a leading `/` is treated as
    /// absolute; relative paths (including literal `~/...`) are resolved against `working_directory`.
    pub fn resolved_directory_path(
        raw_value: Option<&str>,
        working_directory: &Path,
    ) -> Option<PathBuf> {
        let trimmed = raw_value?.trim();
        if trimmed.is_empty() {
            return None;
        }
        if trimmed.starts_with('/') {
            Some(normalize_lexical(Path::new(trimmed)))
        } else {
            Some(normalize_lexical(&working_directory.join(trimmed)))
        }
    }

    fn config_root_with_lookup<'a, F>(
        lookup: F,
        working_directory: &Path,
        home_directory: &Path,
    ) -> PathBuf
    where
        F: Fn(&str) -> Option<&'a str>,
    {
        if let Some(custom_root) =
            Self::resolved_directory_path(lookup(CLAUDE_CONFIG_DIR_ENV_KEY), working_directory)
        {
            return custom_root;
        }
        normalize_lexical(
            &Self::effective_home_with_lookup(&lookup, working_directory, home_directory)
                .join(".claude"),
        )
    }

    fn account_config_path_with_lookup<'a, F>(
        lookup: F,
        working_directory: &Path,
        home_directory: &Path,
    ) -> PathBuf
    where
        F: Fn(&str) -> Option<&'a str>,
    {
        let config_root = Self::config_root_with_lookup(&lookup, working_directory, home_directory);
        let preferred_path = normalize_lexical(&config_root.join(".config.json"));
        if preferred_path.exists() {
            return preferred_path;
        }
        if Self::resolved_directory_path(lookup(CLAUDE_CONFIG_DIR_ENV_KEY), working_directory)
            .is_some()
        {
            return normalize_lexical(&config_root.join(".claude.json"));
        }
        normalize_lexical(
            &Self::effective_home_with_lookup(&lookup, working_directory, home_directory)
                .join(".claude.json"),
        )
    }

    fn credentials_path_with_lookup<'a, F>(
        lookup: F,
        working_directory: &Path,
        home_directory: &Path,
    ) -> PathBuf
    where
        F: Fn(&str) -> Option<&'a str>,
    {
        if let Some(raw_secure_dir) = lookup(CLAUDE_SECURE_STORAGE_CONFIG_DIR_ENV_KEY) {
            if let Some(secure_dir) =
                Self::resolved_directory_path(Some(raw_secure_dir), working_directory)
            {
                return normalize_lexical(&secure_dir.join(".credentials.json"));
            }
            return normalize_lexical(
                &Self::effective_home_with_lookup(&lookup, working_directory, home_directory)
                    .join(".claude")
                    .join(".credentials.json"),
            );
        }
        normalize_lexical(
            &Self::config_root_with_lookup(&lookup, working_directory, home_directory)
                .join(".credentials.json"),
        )
    }

    fn effective_home_with_lookup<'a, F>(
        lookup: &F,
        working_directory: &Path,
        home_directory: &Path,
    ) -> PathBuf
    where
        F: Fn(&str) -> Option<&'a str>,
    {
        if let Some(resolved_home) =
            Self::resolved_directory_path(lookup("HOME"), working_directory)
        {
            return resolved_home;
        }
        normalize_lexical(home_directory)
    }
}

/// Lexically normalizes `.` and `..` components without touching the filesystem.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !normalized.has_root() {
                    normalized.push("..");
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}
