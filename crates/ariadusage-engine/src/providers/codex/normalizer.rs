// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexRateWindowNormalizer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexAdditionalRateLimitMapper.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashSet;

use ariadusage_core::model::window::{NamedRateWindow, RateWindow};
use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowKind {
    Session,
    Weekly,
    Unknown,
}

pub struct CodexRateWindowNormalizer;

impl CodexRateWindowNormalizer {
    pub const SESSION_MINUTES: i64 = 300;
    pub const WEEKLY_MINUTES: i64 = 10_080;

    pub fn normalize(
        primary: Option<RateWindow>,
        secondary: Option<RateWindow>,
    ) -> (Option<RateWindow>, Option<RateWindow>) {
        match (primary, secondary) {
            (None, None) => (None, None),
            (Some(window), None) | (None, Some(window)) => match Self::classify(&window) {
                WindowKind::Session | WindowKind::Unknown => (Some(window), None),
                WindowKind::Weekly => (None, Some(window)),
            },
            (Some(first), Some(second)) => {
                let first_kind = Self::classify(&first);
                let second_kind = Self::classify(&second);
                match (first_kind, second_kind) {
                    (WindowKind::Weekly, WindowKind::Session)
                    | (WindowKind::Weekly, WindowKind::Unknown) => (Some(second), Some(first)),
                    _ => (Some(first), Some(second)),
                }
            }
        }
    }

    fn classify(window: &RateWindow) -> WindowKind {
        match window.window_minutes {
            Some(Self::SESSION_MINUTES) => WindowKind::Session,
            Some(Self::WEEKLY_MINUTES) => WindowKind::Weekly,
            _ => WindowKind::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowSnapshot {
    pub used_percent: i64,
    pub reset_at: i64,
    pub limit_window_seconds: i64,
}

impl WindowSnapshot {
    pub fn to_rate_window(&self) -> Option<RateWindow> {
        let resets_at = if self.reset_at > 0 {
            Timestamp::from_second(self.reset_at).ok()
        } else {
            None
        };
        let window_minutes = if self.limit_window_seconds > 0 {
            Some(self.limit_window_seconds / 60)
        } else {
            None
        };
        RateWindow::new(
            self.used_percent as f64,
            window_minutes,
            resets_at,
            None,
            None,
            false,
        )
        .ok()
    }

    pub fn to_rate_window_unchecked(&self) -> Option<RateWindow> {
        let resets_at = Timestamp::from_second(self.reset_at).ok();
        let window_minutes = Some(self.limit_window_seconds / 60);
        RateWindow::new(
            self.used_percent as f64,
            window_minutes,
            resets_at,
            None,
            None,
            false,
        )
        .ok()
    }
}

pub(crate) fn deserialize_optional_window<'de, D>(
    deserializer: D,
) -> Result<(Option<WindowSnapshot>, bool), D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(val) = raw else {
        return Ok((None, false));
    };
    if val.is_null() {
        return Ok((None, false));
    }
    match serde_json::from_value::<WindowSnapshot>(val) {
        Ok(window) => Ok((Some(window), false)),
        Err(_) => Ok((None, true)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct AdditionalRateLimitDetails {
    pub allowed: Option<bool>,
    pub limit_reached: Option<bool>,
    pub primary_window: Option<WindowSnapshot>,
    pub secondary_window: Option<WindowSnapshot>,
    #[serde(skip)]
    pub primary_window_decode_failed: bool,
    #[serde(skip)]
    pub secondary_window_decode_failed: bool,
}

impl AdditionalRateLimitDetails {
    pub fn has_window_decode_failure(&self) -> bool {
        self.primary_window_decode_failed || self.secondary_window_decode_failed
    }
}

impl<'de> Deserialize<'de> for AdditionalRateLimitDetails {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawDetails {
            #[serde(default)]
            allowed: Option<serde_json::Value>,
            #[serde(default)]
            limit_reached: Option<serde_json::Value>,
            #[serde(default, deserialize_with = "deserialize_optional_window")]
            primary_window: (Option<WindowSnapshot>, bool),
            #[serde(default, deserialize_with = "deserialize_optional_window")]
            secondary_window: (Option<WindowSnapshot>, bool),
        }

        let raw = RawDetails::deserialize(deserializer)?;
        Ok(Self {
            allowed: raw.allowed.and_then(|v| v.as_bool()),
            limit_reached: raw.limit_reached.and_then(|v| v.as_bool()),
            primary_window: raw.primary_window.0,
            secondary_window: raw.secondary_window.0,
            primary_window_decode_failed: raw.primary_window.1,
            secondary_window_decode_failed: raw.secondary_window.1,
        })
    }
}

fn deserialize_optional_additional_details<'de, D>(
    deserializer: D,
) -> Result<(Option<AdditionalRateLimitDetails>, bool), D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(val) = raw else {
        return Ok((None, false));
    };
    if val.is_null() {
        return Ok((None, false));
    }
    match serde_json::from_value::<AdditionalRateLimitDetails>(val) {
        Ok(details) => Ok((Some(details), false)),
        Err(_) => Ok((None, true)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct AdditionalRateLimit {
    pub limit_name: Option<String>,
    pub metered_feature: Option<String>,
    pub rate_limit: Option<AdditionalRateLimitDetails>,
    #[serde(skip)]
    pub rate_limit_decode_failed: bool,
}

impl AdditionalRateLimit {
    pub fn has_window_decode_failure(&self) -> bool {
        self.rate_limit_decode_failed
            || self
                .rate_limit
                .as_ref()
                .is_some_and(AdditionalRateLimitDetails::has_window_decode_failure)
    }
}

impl<'de> Deserialize<'de> for AdditionalRateLimit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawEntry {
            #[serde(default)]
            limit_name: Option<serde_json::Value>,
            #[serde(default)]
            metered_feature: Option<serde_json::Value>,
            #[serde(default, deserialize_with = "deserialize_optional_additional_details")]
            rate_limit: (Option<AdditionalRateLimitDetails>, bool),
        }

        let raw = RawEntry::deserialize(deserializer)?;
        Ok(Self {
            limit_name: raw
                .limit_name
                .and_then(|v| v.as_str().map(ToOwned::to_owned)),
            metered_feature: raw
                .metered_feature
                .and_then(|v| v.as_str().map(ToOwned::to_owned)),
            rate_limit: raw.rate_limit.0,
            rate_limit_decode_failed: raw.rate_limit.1,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SparkWindowKind {
    FiveHour,
    Weekly,
}

impl SparkWindowKind {
    fn id(self) -> &'static str {
        match self {
            Self::FiveHour => CodexAdditionalRateLimitMapper::SPARK_PRIMARY_WINDOW_ID,
            Self::Weekly => CodexAdditionalRateLimitMapper::SPARK_WEEKLY_WINDOW_ID,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::FiveHour => CodexAdditionalRateLimitMapper::SPARK_WINDOW_TITLE,
            Self::Weekly => CodexAdditionalRateLimitMapper::SPARK_WEEKLY_WINDOW_TITLE,
        }
    }
}

pub struct CodexAdditionalRateLimitMapper;

impl CodexAdditionalRateLimitMapper {
    pub const SPARK_PRIMARY_WINDOW_ID: &'static str = "codex-spark";
    pub const SPARK_WEEKLY_WINDOW_ID: &'static str = "codex-spark-weekly";
    pub const SPARK_WINDOW_TITLE: &'static str = "Codex Spark 5-hour";
    pub const SPARK_WEEKLY_WINDOW_TITLE: &'static str = "Codex Spark Weekly";

    pub fn extra_rate_windows(
        entries: Option<&[AdditionalRateLimit]>,
        _now: Timestamp,
    ) -> Vec<NamedRateWindow> {
        let Some(entries) = entries else {
            return Vec::new();
        };
        if entries.is_empty() {
            return Vec::new();
        }
        let mut used_ids = HashSet::new();
        let mut results = Vec::new();
        for entry in entries {
            results.extend(Self::named_windows(entry, &mut used_ids));
        }
        results
    }

    fn named_windows(
        entry: &AdditionalRateLimit,
        used_ids: &mut HashSet<String>,
    ) -> Vec<NamedRateWindow> {
        if Self::is_spark(entry) {
            return Self::spark_windows(entry, used_ids);
        }

        let Some(details) = &entry.rate_limit else {
            return Vec::new();
        };
        let Some(snapshot) = details
            .primary_window
            .as_ref()
            .or(details.secondary_window.as_ref())
        else {
            return Vec::new();
        };
        let Some(id) = Self::window_id(entry) else {
            return Vec::new();
        };
        if !used_ids.insert(id.clone()) {
            return Vec::new();
        }
        let Some(window) = snapshot.to_rate_window() else {
            return Vec::new();
        };
        vec![NamedRateWindow::new(id, Self::window_title(entry), window)]
    }

    fn spark_windows(
        entry: &AdditionalRateLimit,
        used_ids: &mut HashSet<String>,
    ) -> Vec<NamedRateWindow> {
        let Some(details) = &entry.rate_limit else {
            return Vec::new();
        };
        let candidates = [
            (details.primary_window.as_ref(), SparkWindowKind::FiveHour),
            (details.secondary_window.as_ref(), SparkWindowKind::Weekly),
        ];
        let mut out = Vec::new();
        for (snapshot_opt, fallback_kind) in candidates {
            let Some(snapshot) = snapshot_opt else {
                continue;
            };
            let kind = Self::spark_window_kind(snapshot, fallback_kind);
            if !used_ids.insert(kind.id().to_owned()) {
                continue;
            }
            if let Some(window) = snapshot.to_rate_window() {
                out.push(NamedRateWindow::new(kind.id(), kind.title(), window));
            }
        }
        out
    }

    fn spark_window_kind(snapshot: &WindowSnapshot, fallback: SparkWindowKind) -> SparkWindowKind {
        let minutes = if snapshot.limit_window_seconds > 0 {
            snapshot.limit_window_seconds / 60
        } else {
            0
        };
        if minutes > 0 && minutes <= 6 * 60 {
            return SparkWindowKind::FiveHour;
        }
        if minutes >= 6 * 24 * 60 {
            return SparkWindowKind::Weekly;
        }
        fallback
    }

    fn is_spark(entry: &AdditionalRateLimit) -> bool {
        [
            entry.limit_name.as_deref(),
            entry.metered_feature.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|val| val.to_lowercase().contains("spark"))
    }

    fn window_id(entry: &AdditionalRateLimit) -> Option<String> {
        let source = Self::first_non_empty(&[
            entry.metered_feature.as_deref(),
            entry.limit_name.as_deref(),
        ])?;
        let slug = Self::slug(source);
        if slug.is_empty() {
            None
        } else {
            Some(format!("codex-{slug}"))
        }
    }

    fn window_title(entry: &AdditionalRateLimit) -> String {
        Self::first_non_empty(&[
            entry.limit_name.as_deref(),
            entry.metered_feature.as_deref(),
        ])
        .unwrap_or("Codex extra limit")
        .to_owned()
    }

    fn first_non_empty<'a>(values: &[Option<&'a str>]) -> Option<&'a str> {
        for value in values.iter().flatten() {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
        None
    }

    fn slug(value: &str) -> String {
        let mut result = String::new();
        let mut last_was_dash = false;
        for ch in value.to_lowercase().chars() {
            if ch.is_alphanumeric() {
                result.push(ch);
                last_was_dash = false;
            } else if !last_was_dash {
                result.push('-');
                last_was_dash = true;
            }
        }
        result.trim_matches('-').to_owned()
    }
}
