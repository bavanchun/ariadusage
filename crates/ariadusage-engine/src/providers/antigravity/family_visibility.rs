// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityQuotaFamilyVisibility.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};

use ariadusage_core::model::{NamedRateWindow, UsageSnapshot};

use super::status_probe::AntigravityStatusSnapshot;

pub struct AntigravityQuotaFamilyVisibility;

impl AntigravityQuotaFamilyVisibility {
    pub fn idle_window_ids(snapshot: &UsageSnapshot) -> BTreeSet<String> {
        idle_window_ids(snapshot)
    }

    pub fn known_family_for_window(window: &NamedRateWindow) -> Option<KnownFamily> {
        known_family_for_window(window)
    }

    pub fn known_family(window_id: &str, title: &str) -> Option<KnownFamily> {
        known_family(window_id, title)
    }
}

/// Top-level Antigravity quota family (`gemini` or `claude-gpt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KnownFamily {
    Gemini,
    ClaudeGpt,
}

impl KnownFamily {
    pub const ALL: [Self; 2] = [Self::Gemini, Self::ClaudeGpt];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gemini => "gemini",
            Self::ClaudeGpt => "claude-gpt",
        }
    }
}

const BUCKET_TITLE_SUFFIXES: [&str; 2] = [" 5-hour", " weekly"];

/// Window IDs that display surfaces should drop. Empty when every family is untouched, so a card
/// or widget right after a reset still renders its lanes instead of an empty list.
pub fn idle_window_ids(snapshot: &UsageSnapshot) -> BTreeSet<String> {
    let windows: Vec<&NamedRateWindow> = snapshot
        .extra_rate_windows
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|w| AntigravityStatusSnapshot::is_quota_summary_window_id(&w.id))
        .collect();
    if windows.is_empty() {
        return BTreeSet::new();
    }

    let mut families: BTreeMap<String, Vec<&NamedRateWindow>> = BTreeMap::new();
    for window in windows {
        families.entry(family_key(window)).or_default().push(window);
    }

    let idle_families: Vec<Vec<&NamedRateWindow>> = families
        .iter()
        .filter_map(|(_, lanes)| {
            if lanes
                .iter()
                .all(|w| w.usage_known && w.window.used_percent <= 0.0)
            {
                Some(lanes.clone())
            } else {
                None
            }
        })
        .collect();

    if idle_families.len() >= families.len() {
        return BTreeSet::new();
    }

    idle_families
        .into_iter()
        .flatten()
        .map(|w| w.id.clone())
        .collect()
}

pub fn known_family_for_window(window: &NamedRateWindow) -> Option<KnownFamily> {
    known_family(&window.id, &window.title)
}

pub fn known_family(window_id: &str, title: &str) -> Option<KnownFamily> {
    if !AntigravityStatusSnapshot::is_quota_summary_window_id(window_id) {
        return None;
    }
    let id = window_id.to_lowercase();
    if id.contains("gemini") {
        return Some(KnownFamily::Gemini);
    }
    if id.contains("3p") || id.contains("third-party") {
        return Some(KnownFamily::ClaudeGpt);
    }
    let title = title.to_lowercase();
    if title.contains("gemini") {
        return Some(KnownFamily::Gemini);
    }
    if title.contains("claude") || title.contains("gpt") {
        return Some(KnownFamily::ClaudeGpt);
    }
    None
}

fn family_key(named_window: &NamedRateWindow) -> String {
    if let Some(family) = known_family(&named_window.id, &named_window.title) {
        return family.as_str().to_owned();
    }
    let title = named_window.title.to_lowercase();
    for suffix in BUCKET_TITLE_SUFFIXES {
        if let Some(prefix) = title.strip_suffix(suffix) {
            let stripped = prefix.trim();
            if !stripped.is_empty() {
                return stripped.to_owned();
            }
        }
    }
    title
}
