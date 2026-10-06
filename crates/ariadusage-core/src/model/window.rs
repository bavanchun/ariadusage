// Ported from CodexBar Sources/CodexBarCore/UsageFetcher.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::ModelError;

/// A rate window indicating quota consumption and reset timing.
#[derive(Debug, Clone, PartialEq)]
pub struct RateWindow {
    /// Provider usage value, intentionally not normalized globally.
    pub used_percent: f64,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<jiff::Timestamp>,
    /// Optional textual reset description (used by Claude CLI UI scrape).
    pub reset_description: Option<String>,
    /// Optional percent restored on the next regeneration tick for providers with rolling recovery.
    pub next_regen_percent: Option<f64>,
    /// Whether this window was synthesized to stand in for a quota lane the provider did not actually
    /// report, rather than being a real zero-usage window.
    pub is_synthetic_placeholder: bool,
}

impl RateWindow {
    pub fn new(
        used_percent: f64,
        window_minutes: Option<i64>,
        resets_at: Option<jiff::Timestamp>,
        reset_description: Option<String>,
        next_regen_percent: Option<f64>,
        is_synthetic_placeholder: bool,
    ) -> Result<Self, ModelError> {
        if !used_percent.is_finite() {
            return Err(ModelError::NonFiniteNumber(
                "used_percent must be finite".to_string(),
            ));
        }
        if let Some(p) = next_regen_percent
            && !p.is_finite()
        {
            return Err(ModelError::NonFiniteNumber(
                "next_regen_percent must be finite".to_string(),
            ));
        }
        Ok(Self {
            used_percent,
            window_minutes,
            resets_at,
            reset_description,
            next_regen_percent,
            is_synthetic_placeholder,
        })
    }

    /// A synthetic placeholder has no measured quota value, even when its stored percent is zero.
    pub fn measured(&self) -> Option<&Self> {
        if self.is_synthetic_placeholder {
            None
        } else {
            Some(self)
        }
    }

    pub fn remaining_percent(&self) -> f64 {
        (100.0 - self.used_percent).max(0.0)
    }

    pub fn backfilling_reset_time(
        &self,
        cached: Option<&RateWindow>,
        now: jiff::Timestamp,
    ) -> RateWindow {
        if self.resets_at.is_some() {
            return self.clone();
        }
        let Some(cached) = cached else {
            return self.clone();
        };
        let Some(cached_reset) = cached.resets_at else {
            return self.clone();
        };
        if cached_reset <= now {
            return self.clone();
        }

        let window_minutes = if self.window_minutes.is_some_and(|m| m > 0) {
            self.window_minutes
        } else {
            cached.window_minutes
        };

        RateWindow {
            used_percent: self.used_percent,
            window_minutes,
            resets_at: Some(cached_reset),
            reset_description: self
                .reset_description
                .clone()
                .or_else(|| cached.reset_description.clone()),
            next_regen_percent: self.next_regen_percent,
            is_synthetic_placeholder: self.is_synthetic_placeholder,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateWindowRaw {
    used_percent: f64,
    #[serde(default)]
    window_minutes: Option<i64>,
    #[serde(default, with = "ariadusage_protocol::time::rfc3339_opt")]
    resets_at: Option<jiff::Timestamp>,
    #[serde(default)]
    reset_description: Option<String>,
    #[serde(default)]
    next_regen_percent: Option<f64>,
    #[serde(default)]
    is_synthetic_placeholder: bool,
}

impl<'de> Deserialize<'de> for RateWindow {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RateWindowRaw::deserialize(deserializer)?;
        Self::new(
            raw.used_percent,
            raw.window_minutes,
            raw.resets_at,
            raw.reset_description,
            raw.next_regen_percent,
            raw.is_synthetic_placeholder,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RateWindowSerialize<'a> {
    used_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    window_minutes: Option<i64>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    resets_at: Option<jiff::Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reset_description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_regen_percent: Option<f64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    is_synthetic_placeholder: bool,
}

impl Serialize for RateWindow {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let s = RateWindowSerialize {
            used_percent: self.used_percent,
            window_minutes: self.window_minutes,
            resets_at: self.resets_at,
            reset_description: self.reset_description.as_deref(),
            next_regen_percent: self.next_regen_percent,
            is_synthetic_placeholder: self.is_synthetic_placeholder,
        };
        s.serialize(serializer)
    }
}

fn default_true() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// A named rate window for additional provider-specific quota lanes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedRateWindow {
    pub id: String,
    pub title: String,
    pub window: RateWindow,
    /// Whether `window.used_percent` reflects known quota usage.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub usage_known: bool,
}

impl NamedRateWindow {
    pub fn new(id: impl Into<String>, title: impl Into<String>, window: RateWindow) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            window,
            usage_known: true,
        }
    }

    pub fn with_usage_known(mut self, usage_known: bool) -> Self {
        self.usage_known = usage_known;
        self
    }
}
