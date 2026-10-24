// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityQuotaFamilyVisibility.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeSet;

use ariadusage_core::model::{NamedRateWindow, UsageSnapshot};

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

/// Top-level Antigravity quota family (`gemini` or `claudeGPT`).
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
            Self::ClaudeGpt => "claudeGPT",
        }
    }
}

/// Returns the set of `NamedRateWindow.id` values belonging to quota families where all known
/// windows in that family are completely idle (`used_percent <= 0.0001 && resets_at.is_none()`).
pub fn idle_window_ids(snapshot: &UsageSnapshot) -> BTreeSet<String> {
    let windows: Vec<&NamedRateWindow> = snapshot
        .extra_rate_windows
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|w| w.usage_known)
        .collect();
    if windows.is_empty() {
        return BTreeSet::new();
    }

    let mut idle = BTreeSet::new();
    for family in KnownFamily::ALL {
        let family_windows: Vec<&&NamedRateWindow> = windows
            .iter()
            .filter(|w| known_family_for_window(w) == Some(family))
            .collect();
        if family_windows.is_empty() {
            continue;
        }
        if family_windows.iter().all(|w| is_idle(w)) {
            for w in family_windows {
                idle.insert(w.id.clone());
            }
        }
    }
    idle
}

pub fn known_family_for_window(window: &NamedRateWindow) -> Option<KnownFamily> {
    known_family(&window.id, &window.title)
}

pub fn known_family(window_id: &str, title: &str) -> Option<KnownFamily> {
    let normalized_id = window_id.trim().to_lowercase().replace('_', "-");
    let normalized_title = title.trim().to_lowercase();

    if normalized_id.starts_with("antigravity-gemini-")
        || normalized_id.starts_with("antigravity-quota-summary-gemini-")
        || normalized_title.starts_with("gemini")
    {
        return Some(KnownFamily::Gemini);
    }

    if normalized_id.starts_with("antigravity-claude")
        || normalized_id.starts_with("antigravity-gpt")
        || normalized_id.starts_with("antigravity-quota-summary-3p-")
        || normalized_id.starts_with("antigravity-quota-summary-third-party-")
        || normalized_id.starts_with("antigravity-quota-summary-claude")
        || normalized_id.starts_with("antigravity-quota-summary-gpt")
        || normalized_title.starts_with("claude")
        || normalized_title.starts_with("gpt")
    {
        return Some(KnownFamily::ClaudeGpt);
    }

    None
}

fn is_idle(window: &NamedRateWindow) -> bool {
    window.window.used_percent <= 0.0001 && window.window.resets_at.is_none()
}
