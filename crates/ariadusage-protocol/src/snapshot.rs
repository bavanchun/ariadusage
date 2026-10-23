//! Engine and provider usage snapshots.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::ids::ProviderId;
use crate::metric::Metric;
use crate::usage::{
    Cost, Credits, DetailSection, Identity, NamedWindow, Pace, ProviderError, RateWindow,
    StatusIndicator,
};

/// Information about the running engine daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    pub version: String,
    pub refreshing: bool,
}

/// Positional and named rate windows for a provider or account.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderWindows {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<Metric<RateWindow>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary: Option<Metric<RateWindow>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tertiary: Option<Metric<RateWindow>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<NamedWindow>,
}

/// Snapshot of an individual account within a multi-account provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccountSnapshot {
    pub id: String,
    pub label: String,
    pub active: bool,
    #[serde(default)]
    pub windows: ProviderWindows,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<Metric<Credits>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<Metric<Cost>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<Metric<Identity>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Metric<StatusIndicator>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<Metric<Pace>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<ProviderError>,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub updated_at: Option<jiff::Timestamp>,
}

/// Validation error for [`ProviderPresentation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPresentationError(pub String);

impl fmt::Display for ProviderPresentationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProviderPresentationError {}

/// Engine-owned labels and accent color that frontends use to title a
/// provider's usage windows and tint its progress bars.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPresentation {
    /// Compact provider name for tight layouts (e.g. "Anti").
    pub short_display_name: String,
    /// Title of the primary window (e.g. "Session").
    pub session_label: String,
    /// Title of the secondary window (e.g. "Weekly").
    pub weekly_label: String,
    /// Title of the tertiary window, when the provider has one (e.g. "Sonnet").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tertiary_label: Option<String>,
    /// Provider accent color as uppercase `#RRGGBB` hex.
    #[schemars(regex(pattern = r"^#[0-9A-F]{6}$"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent_color: Option<String>,
}

impl ProviderPresentation {
    /// Create a presentation block, rejecting an accent color that is not
    /// uppercase `#RRGGBB` hex.
    pub fn new(
        short_display_name: impl Into<String>,
        session_label: impl Into<String>,
        weekly_label: impl Into<String>,
        tertiary_label: Option<String>,
        accent_color: Option<String>,
    ) -> Result<Self, ProviderPresentationError> {
        if let Some(color) = &accent_color {
            Self::validate_accent_color(color)?;
        }
        Ok(Self {
            short_display_name: short_display_name.into(),
            session_label: session_label.into(),
            weekly_label: weekly_label.into(),
            tertiary_label,
            accent_color,
        })
    }

    /// Format an RGB triple as an uppercase `#RRGGBB` accent color.
    #[must_use]
    pub fn accent_color_from_rgb(red: u8, green: u8, blue: u8) -> String {
        format!("#{red:02X}{green:02X}{blue:02X}")
    }

    /// Check that `color` is exactly `#` followed by six uppercase hex digits.
    pub fn validate_accent_color(color: &str) -> Result<(), ProviderPresentationError> {
        let valid = color.len() == 7
            && color.starts_with('#')
            && color[1..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b));
        if valid {
            Ok(())
        } else {
            Err(ProviderPresentationError(
                "presentation.accentColor must be uppercase #RRGGBB hex".to_string(),
            ))
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderPresentationRaw {
    short_display_name: String,
    session_label: String,
    weekly_label: String,
    #[serde(default)]
    tertiary_label: Option<String>,
    #[serde(default)]
    accent_color: Option<String>,
}

impl<'de> Deserialize<'de> for ProviderPresentation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = ProviderPresentationRaw::deserialize(deserializer)?;
        Self::new(
            raw.short_display_name,
            raw.session_label,
            raw.weekly_label,
            raw.tertiary_label,
            raw.accent_color,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Snapshot of a single AI provider's usage and state.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSnapshot {
    pub id: ProviderId,
    pub display_name: String,
    pub enabled: bool,
    pub source_mode: String,
    /// Window titles and accent color for known providers; absent for
    /// providers the engine has no descriptor for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<ProviderPresentation>,
    #[serde(default)]
    pub windows: ProviderWindows,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<Metric<Credits>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<Metric<Cost>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<Metric<Identity>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<Metric<StatusIndicator>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<Metric<Pace>>,
    #[schemars(length(max = 8))]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<DetailSection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<AccountSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<ProviderError>,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub updated_at: Option<jiff::Timestamp>,
}

impl ProviderSnapshot {
    pub fn new(
        id: ProviderId,
        display_name: impl Into<String>,
        enabled: bool,
        source_mode: impl Into<String>,
    ) -> Self {
        Self {
            id,
            display_name: display_name.into(),
            enabled,
            source_mode: source_mode.into(),
            presentation: None,
            windows: ProviderWindows::default(),
            credits: None,
            cost: None,
            identity: None,
            status: None,
            pace: None,
            details: Vec::new(),
            accounts: Vec::new(),
            last_error: None,
            updated_at: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderSnapshotRaw {
    id: ProviderId,
    display_name: String,
    enabled: bool,
    source_mode: String,
    #[serde(default)]
    presentation: Option<ProviderPresentation>,
    #[serde(default)]
    windows: ProviderWindows,
    #[serde(default)]
    credits: Option<Metric<Credits>>,
    #[serde(default)]
    cost: Option<Metric<Cost>>,
    #[serde(default)]
    identity: Option<Metric<Identity>>,
    #[serde(default)]
    status: Option<Metric<StatusIndicator>>,
    #[serde(default)]
    pace: Option<Metric<Pace>>,
    #[serde(default)]
    details: Vec<DetailSection>,
    #[serde(default)]
    accounts: Vec<AccountSnapshot>,
    #[serde(default)]
    last_error: Option<ProviderError>,
    #[serde(default, with = "crate::time::rfc3339_opt")]
    updated_at: Option<jiff::Timestamp>,
}

impl<'de> Deserialize<'de> for ProviderSnapshot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = ProviderSnapshotRaw::deserialize(deserializer)?;
        DetailSection::validate_sections(&raw.details).map_err(serde::de::Error::custom)?;
        Ok(Self {
            id: raw.id,
            display_name: raw.display_name,
            enabled: raw.enabled,
            source_mode: raw.source_mode,
            presentation: raw.presentation,
            windows: raw.windows,
            credits: raw.credits,
            cost: raw.cost,
            identity: raw.identity,
            status: raw.status,
            pace: raw.pace,
            details: raw.details,
            accounts: raw.accounts,
            last_error: raw.last_error,
            updated_at: raw.updated_at,
        })
    }
}

/// Complete Snapshot v1 emitted by the AriadUsage engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EngineSnapshot {
    /// Schema version, fixed to 1 for Snapshot v1.
    pub schema_version: u32,
    /// Timestamp when this snapshot was assembled.
    #[schemars(with = "String")]
    #[serde(with = "crate::time::rfc3339")]
    pub generated_at: jiff::Timestamp,
    /// Seconds after which clients should consider this snapshot stale.
    pub stale_after_seconds: u32,
    /// Engine daemon metadata.
    pub engine: EngineInfo,
    /// Snapshots for each configured provider.
    pub providers: Vec<ProviderSnapshot>,
}

impl EngineSnapshot {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn new(
        generated_at: jiff::Timestamp,
        stale_after_seconds: u32,
        engine: EngineInfo,
        providers: Vec<ProviderSnapshot>,
    ) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            generated_at,
            stale_after_seconds,
            engine,
            providers,
        }
    }
}
