// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderDiagnosticExport.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::sync::LazyLock;

use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::usage::ProviderErrorCategory;
use ariadusage_protocol::{Confidence, DetailRow, DetailSection, ProviderId};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};

use crate::model::{RateWindow, UsageSnapshot};
use crate::pipeline::error::{ClassifiedError, FetchError};
use crate::pipeline::outcome::{AttemptOutcome, FetchAttempt, FetchOutcome};
use crate::providers::{ProviderDescriptor, SourceMode};

/// Returns the short diagnostic label for a metric source kind.
pub fn kind_label(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Cli => "cli",
        SourceKind::Web => "web",
        SourceKind::Oauth => "oauth",
        SourceKind::Api => "api",
        SourceKind::LocalProbe => "local",
        SourceKind::Dashboard => "web",
        SourceKind::Unknown => "unknown",
    }
}

/// Returns the short diagnostic label for a provider source mode.
pub fn source_mode_label(mode: SourceMode) -> &'static str {
    match mode {
        SourceMode::Auto => "auto",
        SourceMode::Web => "web",
        SourceMode::Cli => "cli",
        SourceMode::Oauth => "oauth",
        SourceMode::Api => "api",
    }
}

/// Returns the fixed safe description for a diagnostic error category.
pub fn category_safe_description(category: ProviderErrorCategory) -> &'static str {
    match category {
        ProviderErrorCategory::Network => "Network error - check your connection",
        ProviderErrorCategory::Auth => "Authentication or setup issue - check provider credentials",
        ProviderErrorCategory::Api => "API error - service returned an unexpected response",
        ProviderErrorCategory::Parse => "Parse error - unexpected response format",
        ProviderErrorCategory::Configuration => {
            "Configuration issue - check provider source and settings"
        }
        ProviderErrorCategory::Unknown => "An unexpected error occurred",
    }
}

/// Returns the fixed safe description for a diagnostic error category label string.
pub fn category_label_safe_description(category: &str) -> &'static str {
    match category {
        "network" => "Network error - check your connection",
        "auth" => "Authentication or setup issue - check provider credentials",
        "api" => "API error - service returned an unexpected response",
        "parse" => "Parse error - unexpected response format",
        "configuration" => "Configuration issue - check provider source and settings",
        _ => "An unexpected error occurred",
    }
}

fn is_subscription_quota_unavailable_description(desc_lower: &str) -> bool {
    desc_lower.contains("subscription notice without session quota data")
}

/// Classifies an arbitrary error description into a safe category label without exposing raw text.
pub fn error_category_label(description: Option<&str>) -> &'static str {
    let Some(raw) = description else {
        return "unknown";
    };
    let desc = raw.to_ascii_lowercase();
    if desc.contains("endpoint override") || is_subscription_quota_unavailable_description(&desc) {
        return "configuration";
    }
    if desc.contains("network") || desc.contains("timeout") || desc.contains("connection") {
        return "network";
    }
    if desc.contains("auth")
        || desc.contains("credential")
        || desc.contains("token")
        || desc.contains("cookie")
        || desc.contains("api key")
        || desc.contains("key not configured")
        || desc.contains("missing key")
    {
        return "auth";
    }
    if desc.contains("source") || desc.contains("not supported") || desc.contains("unavailable") {
        return "configuration";
    }
    if desc.contains("api") || desc.contains("http") || desc.contains("404") || desc.contains("403")
    {
        return "api";
    }
    if desc.contains("parse") || desc.contains("format") || desc.contains("decode") {
        return "parse";
    }
    "unknown"
}

/// Current platform label for diagnostic exports.
pub struct DiagnosticPlatform;

impl DiagnosticPlatform {
    pub fn current() -> &'static str {
        match std::env::consts::OS {
            "macos" => "macOS",
            "linux" => "Linux",
            _ => "unknown",
        }
    }

    pub fn current_string() -> String {
        Self::current().to_string()
    }
}

/// Diagnostic export representation of a provider failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticError {
    pub category: String,
    pub safe_description: String,
}

impl DiagnosticError {
    pub fn new(category: impl Into<String>, safe_description: impl Into<String>) -> Self {
        Self {
            category: category.into(),
            safe_description: safe_description.into(),
        }
    }

    pub fn from_error(error: &FetchError, auth_configured: bool) -> Self {
        match error {
            FetchError::NoAvailableStrategy(_) => {
                let cat = if auth_configured {
                    ProviderErrorCategory::Configuration
                } else {
                    ProviderErrorCategory::Auth
                };
                Self::from_category(cat)
            }
            FetchError::Cancelled => Self::from_category(ProviderErrorCategory::Unknown),
            FetchError::Classified(classified) => {
                Self::from_classified(classified, auth_configured)
            }
        }
    }

    pub fn from_classified(classified: &ClassifiedError, _auth_configured: bool) -> Self {
        let lower = classified.message.to_ascii_lowercase();
        if lower.contains("endpoint override")
            || is_subscription_quota_unavailable_description(&lower)
        {
            return Self::from_category(ProviderErrorCategory::Configuration);
        }
        Self::from_category(classified.category())
    }

    pub fn from_description(description: &str, _auth_configured: bool) -> Self {
        let cat_str = error_category_label(Some(description));
        Self {
            category: cat_str.to_string(),
            safe_description: category_label_safe_description(cat_str).to_string(),
        }
    }

    fn from_category(cat: ProviderErrorCategory) -> Self {
        let cat_str = match cat {
            ProviderErrorCategory::Auth => "auth",
            ProviderErrorCategory::Api => "api",
            ProviderErrorCategory::Parse => "parse",
            ProviderErrorCategory::Network => "network",
            ProviderErrorCategory::Configuration => "configuration",
            ProviderErrorCategory::Unknown => "unknown",
        };
        Self {
            category: cat_str.to_string(),
            safe_description: category_safe_description(cat).to_string(),
        }
    }
}

/// Diagnostic export representation of an individual strategy attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticFetchAttempt {
    #[serde(rename = "strategyID", skip_serializing_if = "Option::is_none")]
    pub strategy_id: Option<String>,
    pub kind: String,
    pub outcome: String,
    #[serde(rename = "wasAvailable")]
    pub was_available: bool,
    #[serde(rename = "errorCategory", skip_serializing_if = "Option::is_none")]
    pub error_category: Option<String>,
}

#[derive(Deserialize)]
struct DiagnosticFetchAttemptRaw {
    #[serde(rename = "strategyID", default)]
    strategy_id: Option<String>,
    kind: String,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(rename = "wasAvailable")]
    was_available: bool,
    #[serde(rename = "errorCategory", default)]
    error_category: Option<String>,
}

impl<'de> Deserialize<'de> for DiagnosticFetchAttempt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = DiagnosticFetchAttemptRaw::deserialize(deserializer)?;
        Ok(Self::new(
            raw.strategy_id,
            raw.kind,
            raw.outcome,
            raw.was_available,
            raw.error_category,
        ))
    }
}

impl DiagnosticFetchAttempt {
    pub fn new(
        strategy_id: Option<String>,
        kind: impl Into<String>,
        outcome: Option<String>,
        was_available: bool,
        error_category: Option<String>,
    ) -> Self {
        let resolved_outcome = outcome
            .unwrap_or_else(|| Self::derived_outcome(was_available, error_category.as_deref()));
        Self {
            strategy_id,
            kind: kind.into(),
            outcome: resolved_outcome,
            was_available,
            error_category,
        }
    }

    pub fn from_attempt(attempt: &FetchAttempt) -> Self {
        let (outcome, error_category) = match attempt.outcome() {
            AttemptOutcome::Succeeded => ("succeeded".to_string(), None),
            AttemptOutcome::Skipped => ("skipped".to_string(), None),
            AttemptOutcome::Failed => {
                let cat = attempt
                    .failure
                    .as_ref()
                    .map(|f| match f.category {
                        ProviderErrorCategory::Auth => "auth",
                        ProviderErrorCategory::Api => "api",
                        ProviderErrorCategory::Parse => "parse",
                        ProviderErrorCategory::Network => "network",
                        ProviderErrorCategory::Configuration => "configuration",
                        ProviderErrorCategory::Unknown => "unknown",
                    })
                    .unwrap_or("unknown");
                ("failed".to_string(), Some(cat.to_string()))
            }
        };

        let strategy_id = if attempt.strategy_id.is_empty() {
            None
        } else {
            Some(attempt.strategy_id.clone())
        };

        Self {
            strategy_id,
            kind: kind_label(attempt.kind).to_string(),
            was_available: attempt.was_available,
            outcome,
            error_category,
        }
    }

    pub fn from_description(
        strategy_id: Option<&str>,
        kind: SourceKind,
        was_available: bool,
        error_description: Option<&str>,
    ) -> Self {
        let error_category = error_description.map(|d| error_category_label(Some(d)).to_string());
        let outcome = if !was_available {
            "skipped".to_string()
        } else if error_category.is_none() {
            "succeeded".to_string()
        } else {
            "failed".to_string()
        };
        Self {
            strategy_id: strategy_id.map(str::to_string),
            kind: kind_label(kind).to_string(),
            outcome,
            was_available,
            error_category,
        }
    }

    pub fn derived_outcome(was_available: bool, error_category: Option<&str>) -> String {
        if !was_available {
            "skipped".to_string()
        } else if error_category.is_none() {
            "succeeded".to_string()
        } else {
            "failed".to_string()
        }
    }
}

/// Summary of configured authentication modes for a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticAuthSummary {
    pub configured: bool,
    pub modes: Vec<String>,
}

impl DiagnosticAuthSummary {
    pub fn new(configured: bool, modes: Vec<String>) -> Self {
        Self { configured, modes }
    }

    pub fn resolved_with_outcome(&self, outcome: &FetchOutcome) -> Self {
        let mut resolved_modes = self.modes.clone();
        let is_success = outcome.result.is_ok();
        if is_success {
            for attempt in &outcome.attempts {
                if attempt.was_available {
                    let mode = kind_label(attempt.kind).to_string();
                    if !resolved_modes.contains(&mode) {
                        resolved_modes.push(mode);
                    }
                }
            }
        }
        let configured = self.configured || is_success;
        Self {
            configured,
            modes: resolved_modes,
        }
    }
}

fn default_true() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// Diagnostic representation of a rate window without raw reset descriptions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRateWindow {
    pub label: String,
    pub used_percent: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<i64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub resets_at: Option<jiff::Timestamp>,
    pub has_reset_description: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_regen_percent: Option<f64>,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub usage_known: bool,
}

impl DiagnosticRateWindow {
    pub fn from_window(label: impl Into<String>, window: &RateWindow, usage_known: bool) -> Self {
        Self {
            label: label.into(),
            used_percent: window.used_percent,
            window_minutes: window.window_minutes,
            resets_at: window.resets_at,
            has_reset_description: window
                .reset_description
                .as_ref()
                .is_some_and(|s| !s.is_empty()),
            next_regen_percent: window.next_regen_percent,
            usage_known,
        }
    }
}

fn default_unknown_confidence() -> String {
    "unknown".to_string()
}

fn confidence_label(c: Confidence) -> &'static str {
    match c {
        Confidence::Exact => "exact",
        Confidence::Estimated => "estimated",
        Confidence::PercentOnly => "percentOnly",
        Confidence::Unknown => "unknown",
    }
}

/// Diagnostic summary of a provider usage snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticUsageSummary {
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub updated_at: jiff::Timestamp,
    #[serde(default = "default_unknown_confidence")]
    pub data_confidence: String,
    pub windows: Vec<DiagnosticRateWindow>,
    pub extra_window_count: usize,
    pub provider_cost_present: bool,
    pub provider_specific_data: Vec<String>,
    #[serde(default)]
    pub detail_sections: Vec<DetailSection>,
}

impl DiagnosticUsageSummary {
    pub fn from_snapshot(snapshot: &UsageSnapshot) -> Self {
        let mut windows = Vec::new();
        if let Some(primary) = &snapshot.primary {
            windows.push(DiagnosticRateWindow::from_window("primary", primary, true));
        }
        if let Some(secondary) = &snapshot.secondary {
            windows.push(DiagnosticRateWindow::from_window(
                "secondary",
                secondary,
                true,
            ));
        }
        if let Some(tertiary) = &snapshot.tertiary {
            windows.push(DiagnosticRateWindow::from_window(
                "tertiary", tertiary, true,
            ));
        }
        for extra in snapshot.extra_rate_windows.as_deref().unwrap_or(&[]) {
            windows.push(DiagnosticRateWindow::from_window(
                &extra.title,
                &extra.window,
                extra.usage_known,
            ));
        }

        let detail_sections = snapshot.details.iter().map(redact_detail_section).collect();

        Self {
            updated_at: snapshot.updated_at,
            data_confidence: confidence_label(snapshot.data_confidence).to_string(),
            windows,
            extra_window_count: snapshot.extra_rate_windows.as_ref().map_or(0, Vec::len),
            provider_cost_present: snapshot.provider_cost.is_some(),
            provider_specific_data: Vec::new(),
            detail_sections,
        }
    }
}

/// Diagnostic summary of provider settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSettingsSummary {
    pub source_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_region: Option<String>,
}

impl DiagnosticSettingsSummary {
    pub fn new(source_mode: SourceMode, api_region: Option<String>) -> Self {
        Self {
            source_mode: source_mode_label(source_mode).to_string(),
            api_region,
        }
    }
}

fn default_schema_version() -> String {
    "1.0".to_string()
}

/// Safe single-provider diagnostic export envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticExport {
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub timestamp: jiff::Timestamp,
    #[serde(default = "DiagnosticPlatform::current_string")]
    pub platform: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    pub provider: String,
    pub display_name: String,
    pub source: String,
    pub source_mode: String,
    pub auth: DiagnosticAuthSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<DiagnosticUsageSummary>,
    pub fetch_attempts: Vec<DiagnosticFetchAttempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<DiagnosticError>,
    pub settings: DiagnosticSettingsSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl DiagnosticExport {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        timestamp: jiff::Timestamp,
        provider: impl Into<String>,
        display_name: impl Into<String>,
        source: impl Into<String>,
        source_mode: impl Into<String>,
        auth: DiagnosticAuthSummary,
        usage: Option<DiagnosticUsageSummary>,
        fetch_attempts: Vec<DiagnosticFetchAttempt>,
        error: Option<DiagnosticError>,
        settings: DiagnosticSettingsSummary,
    ) -> Self {
        Self {
            schema_version: default_schema_version(),
            timestamp,
            platform: DiagnosticPlatform::current_string(),
            app_version: None,
            provider: provider.into(),
            display_name: display_name.into(),
            source: source.into(),
            source_mode: source_mode.into(),
            auth,
            usage,
            fetch_attempts,
            error,
            settings,
            details: None,
        }
    }

    pub fn with_app_version(mut self, app_version: Option<String>) -> Self {
        self.app_version = app_version;
        self
    }
}

/// Safe multi-provider batch diagnostic export envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticBatchExport {
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub timestamp: jiff::Timestamp,
    pub diagnostics: Vec<DiagnosticExport>,
}

impl DiagnosticBatchExport {
    pub fn new(timestamp: jiff::Timestamp, diagnostics: Vec<DiagnosticExport>) -> Self {
        Self {
            schema_version: default_schema_version(),
            timestamp,
            diagnostics,
        }
    }
}

/// Input parameters for building a `DiagnosticExport`.
pub struct DiagnosticExportBuilderInput<'a> {
    pub provider: &'a ProviderId,
    pub descriptor: &'a ProviderDescriptor,
    pub outcome: &'a FetchOutcome,
    pub source_mode: SourceMode,
    pub api_region: Option<String>,
    pub auth: DiagnosticAuthSummary,
    pub app_version: Option<String>,
    pub timestamp: Option<jiff::Timestamp>,
}

/// Builder for constructing safe `DiagnosticExport` values from fetch outcomes.
pub struct DiagnosticExportBuilder;

impl DiagnosticExportBuilder {
    pub fn build(input: DiagnosticExportBuilderInput<'_>) -> DiagnosticExport {
        let resolved_auth = input.auth.resolved_with_outcome(input.outcome);
        let (source, usage, error) = match &input.outcome.result {
            Ok(res) => (
                res.source_label.clone(),
                Some(DiagnosticUsageSummary::from_snapshot(&res.usage)),
                None,
            ),
            Err(err) => (
                "failed".to_string(),
                None,
                Some(DiagnosticError::from_error(err, resolved_auth.configured)),
            ),
        };
        let settings = DiagnosticSettingsSummary::new(input.source_mode, input.api_region);
        let fetch_attempts = input
            .outcome
            .attempts
            .iter()
            .map(DiagnosticFetchAttempt::from_attempt)
            .collect();

        DiagnosticExport {
            schema_version: default_schema_version(),
            timestamp: input.timestamp.unwrap_or_else(jiff::Timestamp::now),
            platform: DiagnosticPlatform::current_string(),
            app_version: input.app_version,
            provider: input.provider.as_str().to_string(),
            display_name: input.descriptor.display_name.to_string(),
            source,
            source_mode: source_mode_label(input.source_mode).to_string(),
            auth: resolved_auth,
            usage,
            fetch_attempts,
            error,
            settings,
            details: None,
        }
    }
}

static EMAIL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}").expect("valid email regex")
});
static COOKIE_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(cookie\s*:\s*)([^\r\n"']+)"#).expect("valid cookie header regex")
});
static AUTH_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(authorization\s*:\s*)([^\r\n"']+)"#).expect("valid auth header regex")
});
static BEARER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bbearer\s+[a-z0-9._\-]+=*\b").expect("valid bearer regex"));
static TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:sk-ant-[^\s"'`;,)>\]]+|sk-cp-[^\s"'`;,)>\]]+|sk-api-[^\s"'`;,)>\]]+|sk-[A-Za-z0-9._\-]{6,}|ya29\.[A-Za-z0-9._\-]+|1//[A-Za-z0-9._\-]+|ghp_[A-Za-z0-9_]+|github_pat_[A-Za-z0-9_]+|eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+)"#,
    )
    .expect("valid token regex")
});
static COOKIE_KV_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b(sessionKey|session_id|__Secure-next-auth\.session-token|_cfuvid|cf_clearance|HERTZ-SESSION)=([^\s"';,)>\]]+)"#,
    )
    .expect("valid cookie kv regex")
});
static ACCOUNT_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:acct_[A-Za-z0-9_\-]+|org-[A-Za-z0-9_\-]+|user-[A-Za-z0-9_\-]+)\b")
        .expect("valid account id regex")
});

/// Redacts sensitive tokens, cookies, emails, auth headers, and account identifiers to `[redacted]`.
pub fn redact_diagnostic_text(text: &str) -> String {
    let mut output = EMAIL_RE.replace_all(text, "[redacted]").into_owned();
    output = TOKEN_RE.replace_all(&output, "[redacted]").into_owned();
    output = BEARER_RE
        .replace_all(&output, "Bearer [redacted]")
        .into_owned();
    output = COOKIE_KV_RE
        .replace_all(&output, "$1=[redacted]")
        .into_owned();
    output = COOKIE_HEADER_RE
        .replace_all(&output, "$1[redacted]")
        .into_owned();
    output = AUTH_HEADER_RE
        .replace_all(&output, "$1[redacted]")
        .into_owned();
    output = ACCOUNT_ID_RE
        .replace_all(&output, "[redacted]")
        .into_owned();
    output
}

fn redact_detail_section(section: &DetailSection) -> DetailSection {
    let rows = section
        .rows
        .iter()
        .map(|row| DetailRow {
            id: row.id.clone(),
            label: redact_diagnostic_text(&row.label),
            value: redact_diagnostic_text(&row.value),
            secondary_value: row.secondary_value.as_deref().map(redact_diagnostic_text),
            progress: row.progress.clone(),
            severity: row.severity,
            usage_value: row.usage_value,
        })
        .collect();
    DetailSection {
        title: section.title.as_deref().map(redact_diagnostic_text),
        rows,
        chart: section.chart.clone(),
    }
}
