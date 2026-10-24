// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityOfflineStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use super::oauth_store::EnvValue;

/// Offline Antigravity CLI store (tokscale lesson): counts local SQLite conversations
/// at `~/.gemini/antigravity-cli/conversations/*.db` without requiring a running
/// language server or OAuth. Used as a last-resort fallback when live quota
/// probes and OAuth both fail.
pub struct AntigravityOfflineStore;

impl AntigravityOfflineStore {
    /// Resolve the base Gemini home directory. Mirrors tokscale's `GEMINI_CLI_HOME`
    /// override: if the env var is set and non-empty, use it; otherwise `~/.gemini`.
    pub fn gemini_home_directory<V: EnvValue>(home: &Path, env: &BTreeMap<String, V>) -> PathBuf {
        if let Some(override_val) = env
            .get("GEMINI_CLI_HOME")
            .map(EnvValue::as_env_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return PathBuf::from(override_val);
        }
        home.join(".gemini")
    }

    /// Explicit Gemini homes only; no process inspection or recursive profile discovery.
    pub fn gemini_home_directories<V: EnvValue>(
        home: &Path,
        env: &BTreeMap<String, V>,
        additional_profile_homes: &[String],
    ) -> Vec<PathBuf> {
        let primary = Self::gemini_home_directory(home, env);
        if additional_profile_homes.is_empty() {
            return vec![primary];
        }
        let additional = additional_profile_homes.iter().filter_map(|raw| {
            let path = raw.trim();
            if path.contains('\0') {
                return None;
            }
            if path == "~" {
                return Some(home.to_path_buf());
            }
            if let Some(rest) = path.strip_prefix("~/") {
                return Some(home.join(rest));
            }
            if !path.starts_with('/') {
                return None;
            }
            Some(PathBuf::from(path))
        });

        let mut seen = BTreeSet::new();
        std::iter::once(primary)
            .chain(additional)
            .filter(|candidate| seen.insert(resolve_symlinks_and_standardize(candidate)))
            .collect()
    }

    pub fn conversations_directory<V: EnvValue>(home: &Path, env: &BTreeMap<String, V>) -> PathBuf {
        Self::gemini_home_directory(home, env)
            .join("antigravity-cli")
            .join("conversations")
    }

    pub fn app_data_directory<V: EnvValue>(home: &Path, env: &BTreeMap<String, V>) -> PathBuf {
        Self::gemini_home_directory(home, env).join("antigravity")
    }

    /// Tokscale cache alternative: `~/.config/tokscale/antigravity-cache/sessions`
    pub fn tokscale_cache_directory(home: &Path) -> PathBuf {
        home.join(".config")
            .join("tokscale")
            .join("antigravity-cache")
            .join("sessions")
    }

    /// Count offline conversations (`.db` files). Cheap, no SQLite open.
    pub fn count_conversations<V: EnvValue>(home: &Path, env: &BTreeMap<String, V>) -> usize {
        Self::count_conversations_with_profiles(home, env, &[])
    }

    pub fn count_conversations_with_profiles<V: EnvValue>(
        home: &Path,
        env: &BTreeMap<String, V>,
        additional_profile_homes: &[String],
    ) -> usize {
        let mut total_db = 0usize;
        for gemini_home in Self::gemini_home_directories(home, env, additional_profile_homes) {
            let primary = gemini_home.join("antigravity-cli").join("conversations");
            let app_data = gemini_home.join("antigravity");
            let app_data_conversations = app_data.join("conversations");
            total_db += Self::count_db_files(&primary)
                + Self::count_db_files(&app_data)
                + Self::count_db_files(&app_data_conversations);
        }
        if total_db > 0 {
            return total_db;
        }
        let cache = Self::tokscale_cache_directory(home);
        Self::count_jsonl_files(&cache)
    }

    pub fn has_offline_data<V: EnvValue>(home: &Path, env: &BTreeMap<String, V>) -> bool {
        Self::count_conversations(home, env) > 0
    }

    pub fn has_offline_data_with_profiles<V: EnvValue>(
        home: &Path,
        env: &BTreeMap<String, V>,
        additional_profile_homes: &[String],
    ) -> bool {
        Self::count_conversations_with_profiles(home, env, additional_profile_homes) > 0
    }

    fn count_db_files(directory: &Path) -> usize {
        Self::count_files_with_extension(directory, "db")
    }

    fn count_jsonl_files(directory: &Path) -> usize {
        Self::count_files_with_extension(directory, "jsonl")
    }

    fn count_files_with_extension(directory: &Path, expected_ext: &str) -> usize {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return 0;
        };
        entries
            .filter_map(Result::ok)
            .filter(|entry| {
                let path = entry.path();
                let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                    return false;
                };
                if file_name.starts_with('.') {
                    return false;
                }
                if !entry.file_type().is_ok_and(|ft| ft.is_file()) {
                    return false;
                }
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(expected_ext))
            })
            .count()
    }
}

pub(crate) fn standardize_lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

pub(crate) fn resolve_symlinks_and_standardize(path: &Path) -> PathBuf {
    let lexical = standardize_lexical_path(path);
    if let Ok(canonical) = std::fs::canonicalize(&lexical) {
        return canonical;
    }
    let mut existing = lexical.as_path();
    let mut tail = Vec::new();
    while let Some(parent) = existing.parent() {
        if let Some(name) = existing.file_name() {
            tail.push(name.to_os_string());
        }
        if let Ok(canonical_parent) = std::fs::canonicalize(parent) {
            let mut rebuilt = canonical_parent;
            for part in tail.into_iter().rev() {
                rebuilt.push(part);
            }
            return standardize_lexical_path(&rebuilt);
        }
        existing = parent;
    }
    lexical
}
