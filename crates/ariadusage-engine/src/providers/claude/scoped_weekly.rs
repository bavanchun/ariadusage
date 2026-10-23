// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeScopedWeeklyLimitMapper.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeWeb/ClaudeWebExtraRateWindowParser.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashSet;

use ariadusage_core::model::{NamedRateWindow, RateWindow};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One entry in the Claude OAuth `limits` array.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuthScopedLimitEntry {
    pub group: Option<String>,
    pub kind: Option<String>,
    #[serde(rename = "model_id")]
    pub model_id: Option<String>,
    #[serde(rename = "model_name")]
    pub model_name: Option<String>,
    pub percent: Option<f64>,
    #[serde(rename = "resets_at")]
    pub resets_at: Option<String>,
}

impl OAuthScopedLimitEntry {
    /// Decodes a single entry lossily so wrong-typed optional string fields become `None`
    /// rather than rejecting the entry.
    pub fn from_json_value(value: &Value) -> Option<Self> {
        let obj = value.as_object()?;
        let scope_model = obj
            .get("scope")
            .and_then(Value::as_object)
            .and_then(|scope| scope.get("model"))
            .and_then(Value::as_object);
        let group = optional_string_field(obj.get("group"));
        let kind = optional_string_field(obj.get("kind"));
        let model_id = scope_model
            .and_then(|model| optional_string_field(model.get("id")))
            .or_else(|| optional_string_field(obj.get("model_id")));
        let model_name = scope_model
            .and_then(|model| optional_string_field(model.get("display_name")))
            .or_else(|| optional_string_field(obj.get("model_name")));
        let percent = obj.get("percent").and_then(Value::as_f64);
        let resets_at = optional_string_field(obj.get("resets_at"));
        Some(Self {
            group,
            kind,
            model_id,
            model_name,
            percent,
            resets_at,
        })
    }

    /// Decodes a JSON `limits` array lossily, skipping non-object or malformed entries.
    pub fn parse_lossy_array(value: &Value) -> Vec<Self> {
        let Some(items) = value.as_array() else {
            return Vec::new();
        };
        items.iter().filter_map(Self::from_json_value).collect()
    }
}

fn optional_string_field(value: Option<&Value>) -> Option<String> {
    value?.as_str().map(str::to_owned)
}

/// Maps Claude's model-scoped weekly limit entries (`limits`) into `NamedRateWindow` records.
pub struct ClaudeScopedWeeklyLimitMapper;

impl ClaudeScopedWeeklyLimitMapper {
    pub const WEEKLY_WINDOW_MINUTES: i64 = 10080;

    /// Converts scoped weekly limit entries into deduplicated `NamedRateWindow` items.
    pub fn named_windows(entries: &[OAuthScopedLimitEntry]) -> Vec<NamedRateWindow> {
        Self::named_windows_with_reset_description(entries, None::<fn(Timestamp) -> String>)
    }

    /// Converts scoped weekly limit entries into deduplicated `NamedRateWindow` items with an optional reset formatter.
    pub fn named_windows_with_reset_description<F>(
        entries: &[OAuthScopedLimitEntry],
        reset_description: Option<F>,
    ) -> Vec<NamedRateWindow>
    where
        F: Fn(Timestamp) -> String,
    {
        let mut seen_ids = HashSet::new();
        let mut windows = Vec::new();

        for entry in entries {
            let Some(group) = Self::normalized_token(entry.group.as_deref()) else {
                continue;
            };
            if group != "weekly" {
                continue;
            }
            let Some(kind) = Self::normalized_token(entry.kind.as_deref()) else {
                continue;
            };
            if kind != "weekly_scoped" {
                continue;
            }
            let Some(percent) = entry.percent else {
                continue;
            };
            if !percent.is_finite() {
                continue;
            }
            let Some(model_name) = Self::non_empty(entry.model_name.as_deref()) else {
                continue;
            };
            if Self::is_all_models_scope(entry.model_id.as_deref(), model_name) {
                continue;
            }

            let identity = Self::non_empty(entry.model_id.as_deref()).unwrap_or(model_name);
            let slug = Self::slug(identity);
            if slug.is_empty() {
                continue;
            }
            let id = format!("claude-weekly-scoped-{slug}");
            if !seen_ids.insert(id.clone()) {
                continue;
            }

            let resets_at = entry.resets_at.as_deref().and_then(parse_iso8601_timestamp);
            let reset_desc = resets_at.and_then(|ts| reset_description.as_ref().map(|f| f(ts)));
            let Ok(window) = RateWindow::new(
                percent,
                Some(Self::WEEKLY_WINDOW_MINUTES),
                resets_at,
                reset_desc,
                None,
                false,
            ) else {
                continue;
            };
            windows.push(NamedRateWindow::new(
                id,
                format!("{model_name} only"),
                window,
            ));
        }

        windows
    }

    /// Maps a single CLI scoped weekly model window (`Current week (<Model>)`) into a `NamedRateWindow`.
    pub fn extra_rate_window(
        model_name: &str,
        used_percent: f64,
        resets_at: Option<Timestamp>,
        reset_description: Option<String>,
    ) -> Option<NamedRateWindow> {
        let clean_name = Self::non_empty(Some(model_name))?;
        if Self::is_all_models_scope(None, clean_name) {
            return None;
        }
        let slug = Self::slug(clean_name);
        if slug.is_empty() {
            return None;
        }
        let id = format!("claude-weekly-scoped-{slug}");
        let window = RateWindow::new(
            used_percent,
            Some(Self::WEEKLY_WINDOW_MINUTES),
            resets_at,
            reset_description,
            None,
            false,
        )
        .ok()?;
        Some(NamedRateWindow::new(
            id,
            format!("{clean_name} only"),
            window,
        ))
    }

    fn non_empty(raw: Option<&str>) -> Option<&str> {
        let trimmed = raw?.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }

    fn normalized_token(raw: Option<&str>) -> Option<String> {
        Some(Self::non_empty(raw)?.to_lowercase())
    }

    fn is_all_models_scope(model_id: Option<&str>, model_name: &str) -> bool {
        if Self::slug(model_name) == "all-models" {
            return true;
        }
        let Some(model_id_slug) = model_id.map(Self::slug) else {
            return false;
        };
        model_id_slug == "all-models" || model_id_slug.ends_with("-all-models")
    }

    pub(crate) fn slug(raw: &str) -> String {
        let lower = raw.trim().to_lowercase();
        let mut scalars = String::with_capacity(lower.len());
        let mut previous_was_separator = false;

        for ch in lower.chars() {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
                scalars.push(ch);
                previous_was_separator = false;
            } else if !previous_was_separator && !scalars.is_empty() {
                scalars.push('-');
                previous_was_separator = true;
            }
        }

        if scalars.ends_with('-') {
            scalars.pop();
        }
        scalars
    }
}

pub(crate) fn parse_iso8601_timestamp(raw: &str) -> Option<Timestamp> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.parse::<Timestamp>().ok()
}
