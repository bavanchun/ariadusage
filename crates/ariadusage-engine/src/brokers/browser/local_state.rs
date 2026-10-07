use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use super::error::BrowserError;
use super::sqlite_copy::read_owned_file;

#[derive(Deserialize)]
struct LocalState {
    profile: LocalProfiles,
}

#[derive(Deserialize)]
struct LocalProfiles {
    #[serde(rename = "info_cache")]
    info_cache: BTreeMap<String, ProfileInfo>,
}

#[derive(Deserialize)]
struct ProfileInfo {
    name: Option<String>,
}

pub(super) fn profile_name(root: &Path, profile_id: &str) -> Option<String> {
    let bytes = read_owned_file(&root.join("Local State"), 1024 * 1024).ok()?;
    let state: LocalState = serde_json::from_slice(&bytes)
        .map_err(|_| BrowserError::Malformed)
        .ok()?;
    state
        .profile
        .info_cache
        .get(profile_id)
        .and_then(|info| info.name.as_deref())
        .map(|name| safe_label(name, "Browser profile"))
}

pub(super) fn safe_label(value: &str, fallback: &str) -> String {
    let label = value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(64)
        .collect::<String>()
        .trim()
        .to_owned();
    if label.is_empty() || label.contains('@') {
        fallback.to_owned()
    } else {
        label
    }
}
