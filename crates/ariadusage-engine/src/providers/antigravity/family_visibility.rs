// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityQuotaFamilyVisibility.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeSet;

use super::status_probe::{AntigravityModelQuota, AntigravityStatusSnapshot};

/// Known Antigravity model family buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KnownFamily {
    Gemini,
    ClaudeGpt,
}

impl KnownFamily {
    pub const ALL: [KnownFamily; 2] = [KnownFamily::Gemini, KnownFamily::ClaudeGpt];

    pub const fn window_id(self) -> &'static str {
        match self {
            Self::Gemini => "antigravity-gemini",
            Self::ClaudeGpt => "antigravity-claude-gpt",
        }
    }
}

/// Classifies a model quota into a known Antigravity quota family (`Gemini` or `ClaudeGpt`),
/// excluding tab autocomplete, image models, and Gemini Lite variants.
pub fn known_family(quota: &AntigravityModelQuota) -> Option<KnownFamily> {
    for raw_candidate in [&quota.model_id, &quota.label] {
        let candidate = AntigravityStatusSnapshot::canonical_model_id(raw_candidate);
        let lower = candidate.trim().to_lowercase();
        if lower.is_empty() {
            continue;
        }
        if lower.contains("tab") {
            return None;
        }
        if contains_token("image", &lower) {
            return None;
        }

        if lower.contains("claude") || lower.contains("gpt") {
            return Some(KnownFamily::ClaudeGpt);
        }
        if lower.contains("gemini") {
            if lower.contains("lite") {
                return None;
            }
            return Some(KnownFamily::Gemini);
        }
    }
    None
}

/// Returns the window IDs (`antigravity-gemini`, `antigravity-claude-gpt`) for families
/// whose member models all have `remaining_fraction >= 0.999`.
pub fn idle_window_ids(quotas: &[AntigravityModelQuota]) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    for family in KnownFamily::ALL {
        let family_quotas: Vec<&AntigravityModelQuota> = quotas
            .iter()
            .filter(|q| known_family(q) == Some(family))
            .collect();
        if family_quotas.is_empty() {
            continue;
        }
        let has_consumed_member = family_quotas.iter().any(|quota| {
            let Some(remaining_fraction) = quota.remaining_fraction else {
                return true;
            };
            remaining_fraction < 0.999
        });
        if !has_consumed_member {
            result.insert(family.window_id().to_owned());
        }
    }
    result
}

pub(crate) fn contains_token(token: &str, text: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric())
        .any(|part| part == token)
}
