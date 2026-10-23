// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeStatusProbe.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeCLISpendCapParser.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::Duration;

use ariadusage_core::model::{
    NamedRateWindow, ProviderCostSnapshot, ProviderIdentitySnapshot, RateWindow, UsageSnapshot,
};
use ariadusage_core::pipeline::{ClassifiedError, TransportClass};
use ariadusage_protocol::usage::ProviderErrorKind;
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::Timestamp;
use jiff::civil::{Date, DateTime};
use jiff::tz::{AmbiguousOffset, TimeZone};
use serde_json::Value;

use super::cli_gates::ClaudeCLIRateLimitGate;
use super::cli_screen::{ClaudeCLIScreen, ClaudeCLISpendCapParser, split_newline_components};
use super::identity::ClaudeAccountIdentity;
use super::plan::ClaudePlan;
use super::scoped_weekly::ClaudeScopedWeeklyLimitMapper;

pub const SESSION_WINDOW_MINUTES: u32 = 5 * 60;
pub const WEEKLY_WINDOW_MINUTES: u32 = 7 * 24 * 60;

pub const SUBSCRIPTION_QUOTA_UNAVAILABLE_DESCRIPTION: &str = "Claude CLI /usage returned a subscription notice without session quota data. Local cost and token history remain available.";

const USAGE_INSIGHTS_MARKER: &str = "What's contributing to your limits usage?";

/// Account identity fields parsed from interactive Claude CLI `/usage` or `/status` output.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct ClaudeCLIAccountIdentity {
    pub account_email: Option<String>,
    pub account_organization: Option<String>,
    pub login_method: Option<String>,
}

impl fmt::Debug for ClaudeCLIAccountIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaudeCLIAccountIdentity")
            .field("has_account_email", &self.account_email.is_some())
            .field(
                "has_account_organization",
                &self.account_organization.is_some(),
            )
            .field("login_method", &self.login_method)
            .finish()
    }
}

impl ClaudeCLIAccountIdentity {
    #[must_use]
    pub fn to_account_identity(&self) -> ClaudeAccountIdentity {
        ClaudeAccountIdentity {
            account_uuid: None,
            email: self.account_email.clone(),
            organization_uuid: None,
            organization_name: self.account_organization.clone(),
        }
    }
}

/// Parsed status and quota snapshot from the interactive Claude CLI (`/usage` and `/status`).
#[derive(Clone, PartialEq)]
pub struct ClaudeStatusSnapshot {
    pub session_percent_left: Option<i32>,
    pub weekly_percent_left: Option<i32>,
    pub opus_percent_left: Option<i32>,
    pub extra_rate_windows: Vec<NamedRateWindow>,
    pub provider_cost: Option<ProviderCostSnapshot>,
    pub account_email: Option<String>,
    pub account_organization: Option<String>,
    pub login_method: Option<String>,
    pub primary_reset_description: Option<String>,
    pub secondary_reset_description: Option<String>,
    pub opus_reset_description: Option<String>,
    pub raw_text: String,
}

impl fmt::Debug for ClaudeStatusSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaudeStatusSnapshot")
            .field("session_percent_left", &self.session_percent_left)
            .field("weekly_percent_left", &self.weekly_percent_left)
            .field("opus_percent_left", &self.opus_percent_left)
            .field("extra_rate_windows", &self.extra_rate_windows)
            .field("provider_cost", &self.provider_cost)
            .field("has_account_email", &self.account_email.is_some())
            .field(
                "has_account_organization",
                &self.account_organization.is_some(),
            )
            .field("login_method", &self.login_method)
            .field("primary_reset_description", &self.primary_reset_description)
            .field(
                "secondary_reset_description",
                &self.secondary_reset_description,
            )
            .field("opus_reset_description", &self.opus_reset_description)
            .field("raw_text_len", &self.raw_text.len())
            .finish()
    }
}

impl ClaudeStatusSnapshot {
    /// Converts this CLI status snapshot into a core [`UsageSnapshot`] with [`Confidence::PercentOnly`].
    pub fn to_usage_snapshot(&self, now: Timestamp) -> Result<UsageSnapshot, ClassifiedError> {
        self.to_usage_snapshot_with_tz(now, &TimeZone::system())
    }

    pub fn to_usage_snapshot_with_tz(
        &self,
        now: Timestamp,
        default_tz: &TimeZone,
    ) -> Result<UsageSnapshot, ClassifiedError> {
        let Some(session_pct_left) = self.session_percent_left else {
            return Err(ClassifiedError::new(
                ProviderErrorKind::ParseFailure,
                "Could not parse Claude usage: missing session data",
            ));
        };

        let make_window = |pct_left: Option<i32>,
                           reset: Option<&str>,
                           window_minutes: u32|
         -> Option<RateWindow> {
            let left = pct_left?;
            let used = (100.0 - f64::from(left)).clamp(0.0, 100.0);
            let reset_clean = reset.map(str::trim).filter(|s| !s.is_empty());
            let resets_at = ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
                reset_clean,
                now,
                Some(Duration::from_secs(u64::from(window_minutes) * 60)),
                default_tz,
            );
            RateWindow::new(
                used,
                Some(i64::from(window_minutes)),
                resets_at,
                reset_clean.map(str::to_owned),
                None,
                false,
            )
            .ok()
        };

        let primary = make_window(
            Some(session_pct_left),
            self.primary_reset_description.as_deref(),
            SESSION_WINDOW_MINUTES,
        );
        let secondary = make_window(
            self.weekly_percent_left,
            self.secondary_reset_description.as_deref(),
            WEEKLY_WINDOW_MINUTES,
        );
        let tertiary = make_window(
            self.opus_percent_left,
            self.opus_reset_description.as_deref(),
            WEEKLY_WINDOW_MINUTES,
        );

        let provider_cost = self.provider_cost.clone().or_else(|| {
            ClaudeCLISpendCapParser::parse(
                &ClaudeCLIScreen::render_with_options(&self.raw_text, true),
                now,
            )
        });

        let identity = ProviderIdentitySnapshot {
            provider_id: ProviderId::new("claude").ok(),
            account_email: self.account_email.clone(),
            account_organization: self.account_organization.clone(),
            login_method: self.login_method.clone(),
            account_id: None,
        };

        UsageSnapshot::new(
            primary,
            secondary,
            tertiary,
            (!self.extra_rate_windows.is_empty()).then(|| self.extra_rate_windows.clone()),
            provider_cost,
            Vec::new(),
            None,
            None,
            now,
            Some(identity),
            Confidence::PercentOnly,
        )
        .map_err(|_| {
            ClassifiedError::new(
                ProviderErrorKind::ParseFailure,
                "Claude CLI snapshot validation failed",
            )
        })
    }
}

/// Errors produced when probing or parsing the interactive Claude CLI.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaudeStatusProbeError {
    #[error("Claude CLI is not installed or not on PATH.")]
    ClaudeNotInstalled,
    #[error("{0}")]
    AuthenticationFailed(String),
    #[error("Could not parse Claude usage: {0}")]
    ParseFailed(String),
    #[error("Claude usage probe timed out.")]
    TimedOut,
    #[error("Claude usage probe cancelled.")]
    Cancelled,
}

impl ClaudeStatusProbeError {
    /// Matches `ClaudeUsageFetcher.isRetryableCLIProbeError`.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::AuthenticationFailed(_) | Self::ClaudeNotInstalled | Self::Cancelled => false,
            Self::TimedOut => true,
            Self::ParseFailed(message) => {
                let lower = message.to_ascii_lowercase();
                lower.contains("still loading usage")
            }
        }
    }

    /// Matches `ClaudeUsageFetcher.shouldTryDirectCLIUsage`.
    pub fn should_try_direct_cli_usage(&self) -> bool {
        if let Self::ParseFailed(message) = self
            && message
                .to_ascii_lowercase()
                .contains("could not load usage data")
        {
            return true;
        }
        self.is_retryable()
    }

    /// Matches `ClaudeCLIRateLimitGate.isRateLimitError`.
    pub fn is_rate_limit_error(&self) -> bool {
        match self {
            Self::ParseFailed(message) => {
                ClaudeCLIRateLimitGate::is_rate_limit_message(message, true)
            }
            Self::AuthenticationFailed(_)
            | Self::ClaudeNotInstalled
            | Self::TimedOut
            | Self::Cancelled => {
                ClaudeCLIRateLimitGate::is_rate_limit_message(&self.to_string(), false)
            }
        }
    }

    pub fn into_classified_error(self) -> ClassifiedError {
        let msg = self.to_string();
        match self {
            Self::ClaudeNotInstalled => {
                ClassifiedError::new(ProviderErrorKind::ProviderUnavailable, msg)
            }
            Self::AuthenticationFailed(_) => {
                ClassifiedError::new(ProviderErrorKind::AuthenticationExpired, msg)
            }
            Self::TimedOut => ClassifiedError::new(ProviderErrorKind::NetworkFailure, msg)
                .with_transport(TransportClass::Timeout),
            Self::Cancelled => ClassifiedError::new(ProviderErrorKind::NetworkFailure, msg)
                .with_transport(TransportClass::Cancelled),
            Self::ParseFailed(_) => {
                if self.is_rate_limit_error() {
                    ClassifiedError::new(ProviderErrorKind::RateLimited, msg)
                } else {
                    ClassifiedError::new(ProviderErrorKind::ParseFailure, msg)
                }
            }
        }
    }
}

struct LabelSearchContext {
    lines: Vec<String>,
    normalized_lines: Vec<String>,
}

impl LabelSearchContext {
    fn new(text: &str) -> Self {
        let lines: Vec<String> = split_newline_components(text)
            .into_iter()
            .map(str::to_owned)
            .collect();
        let normalized_lines = lines
            .iter()
            .map(|l| ClaudeStatusProbeParser::normalized_for_label_search(l))
            .collect();
        Self {
            lines,
            normalized_lines,
        }
    }
}

#[derive(Debug, Clone)]
struct ScopedWeeklyUsage {
    model_name: String,
    percent_left: i32,
    reset_description: Option<String>,
}

/// Parser for Claude CLI `/usage` and `/status` captures and reset timestamps.
pub struct ClaudeStatusProbeParser;

impl ClaudeStatusProbeParser {
    pub fn parse(
        text: &str,
        status_text: Option<&str>,
    ) -> Result<ClaudeStatusSnapshot, ClaudeStatusProbeError> {
        Self::parse_at(text, status_text, Timestamp::now(), &TimeZone::system())
    }

    pub fn parse_at(
        text: &str,
        status_text: Option<&str>,
        now: Timestamp,
        default_tz: &TimeZone,
    ) -> Result<ClaudeStatusSnapshot, ClaudeStatusProbeError> {
        let clean = Self::clean_capture(text);
        let status_clean = status_text.map(Self::clean_capture);
        if clean.is_empty() {
            return Err(ClaudeStatusProbeError::TimedOut);
        }

        if let Some(usage_error) = Self::extract_usage_error(&clean) {
            return Err(Self::usage_probe_error(usage_error));
        }

        let latest_usage_panel = Self::trim_to_latest_usage_panel(&clean);
        let usage_panel_text = latest_usage_panel.as_deref().unwrap_or(&clean);
        if Self::is_usage_still_loading(usage_panel_text) {
            return Err(ClaudeStatusProbeError::ParseFailed(
                "Claude CLI /usage is still loading usage data.".to_owned(),
            ));
        }

        let label_context = LabelSearchContext::new(usage_panel_text);

        let mut session_pct = Self::extract_percent("Current session", &label_context);
        let weekly_pct = Self::extract_percent("Current week (all models)", &label_context);
        let opus_labels = [
            "Current week (Opus)",
            "Current week (Sonnet only)",
            "Current week (Sonnet)",
        ];
        let opus_pct = Self::extract_percent_multi(&opus_labels, &label_context);

        let weekly_models: HashSet<String> = label_context
            .lines
            .iter()
            .filter_map(|line| Self::weekly_model_name(line))
            .map(|m| Self::normalized_for_label_search(&m))
            .collect();
        let has_all_models_weekly_label = weekly_models
            .iter()
            .any(|m| Self::is_all_models_weekly_model(m));
        let opus_models: HashSet<String> = opus_labels
            .iter()
            .filter_map(|label| Self::weekly_model_name(label))
            .map(|m| Self::normalized_for_label_search(&m))
            .collect();
        let has_opus_label = !weekly_models.is_disjoint(&opus_models);

        if session_pct.is_none() {
            let ordered = Self::all_percents(usage_panel_text);
            if let Some(&first) = ordered.first() {
                session_pct = Some(first);
            }
        }

        let identity = Self::extract_identity(&clean, status_clean.as_deref());

        let Some(session_pct) = session_pct else {
            return Err(ClaudeStatusProbeError::ParseFailed(
                "Missing Current session.".to_owned(),
            ));
        };

        let session_reset = Self::extract_reset("Current session", &label_context);
        let weekly_reset = if has_all_models_weekly_label {
            Self::extract_reset("Current week (all models)", &label_context)
        } else {
            None
        };
        let opus_reset = if has_opus_label {
            Self::extract_reset_multi(&opus_labels, &label_context)
        } else {
            None
        };

        let scoped_weekly_usages = Self::extract_scoped_weekly_usages(&label_context);
        let extra_rate_windows = Self::extra_rate_windows(
            &scoped_weekly_usages,
            weekly_reset.as_deref(),
            now,
            default_tz,
        );
        let provider_cost = ClaudeCLISpendCapParser::parse(
            &ClaudeCLIScreen::render_with_options(
                &(format!("{text}{}", status_text.unwrap_or(""))),
                true,
            ),
            now,
        );

        Ok(ClaudeStatusSnapshot {
            session_percent_left: Some(session_pct),
            weekly_percent_left: weekly_pct,
            opus_percent_left: opus_pct,
            extra_rate_windows,
            provider_cost,
            account_email: identity.account_email,
            account_organization: identity.account_organization,
            login_method: identity.login_method,
            primary_reset_description: session_reset,
            secondary_reset_description: weekly_reset,
            opus_reset_description: opus_reset,
            raw_text: format!("{text}{}", status_text.unwrap_or("")),
        })
    }

    pub fn parse_identity(
        usage_text: Option<&str>,
        status_text: Option<&str>,
    ) -> ClaudeCLIAccountIdentity {
        let usage_clean = usage_text.map(Self::clean_capture).unwrap_or_default();
        let status_clean = status_text.map(Self::clean_capture);
        Self::extract_identity(&usage_clean, status_clean.as_deref())
    }

    pub fn clean_capture(text: &str) -> String {
        let rendered = ClaudeCLIScreen::render_with_options(text, true);
        let Some((_start, end)) = find_ascii_case_insensitive(&rendered, USAGE_INSIGHTS_MARKER)
        else {
            return rendered;
        };
        rendered[..end].to_owned()
    }

    pub fn validate_usage_before_status_probe(text: &str) -> Result<(), ClaudeStatusProbeError> {
        let clean = Self::clean_capture(text);
        if let Some(usage_error) = Self::extract_usage_error(&clean) {
            return Err(Self::usage_probe_error(usage_error));
        }

        let latest_usage_panel = Self::trim_to_latest_usage_panel(&clean);
        if Self::is_usage_still_loading(latest_usage_panel.as_deref().unwrap_or(&clean)) {
            return Err(ClaudeStatusProbeError::ParseFailed(
                "Claude CLI /usage is still loading usage data.".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn usage_output_looks_relevant(text: &str) -> bool {
        let normalized: String = Self::clean_capture(text)
            .chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect();
        normalized.contains("currentsession")
            || normalized.contains("currentweek")
            || normalized.contains("loadingusage")
            || normalized.contains("failedtoloadusagedata")
            || Self::usage_capture_has_subscription_notice(&normalized)
    }

    pub fn usage_capture_has_session_value(normalized_text: &str) -> bool {
        let insights_start = normalized_text
            .find("what'scontributingtoyourlimitsusage?")
            .unwrap_or(normalized_text.len());
        let quota_text = &normalized_text[..insights_start];
        let Some(pos) = quota_text.find("currentsession") else {
            return false;
        };
        let tail = &quota_text[(pos + "currentsession".len())..];
        find_percent_token(tail).is_some()
    }

    pub fn usage_capture_has_subscription_notice(normalized_text: &str) -> bool {
        normalized_text.contains("currentlyusingyoursubscription")
            && normalized_text.contains("claudecodeusage")
    }

    pub fn is_subscription_quota_unavailable_description(text: Option<&str>) -> bool {
        text.is_some_and(|t| {
            find_ascii_case_insensitive(t, "subscription notice without session quota data")
                .is_some()
        })
    }

    fn extract_percent_multi(labels: &[&str], context: &LabelSearchContext) -> Option<i32> {
        for label in labels {
            if let Some(value) = Self::extract_percent(label, context) {
                return Some(value);
            }
        }
        None
    }

    fn extract_percent(label_substring: &str, context: &LabelSearchContext) -> Option<i32> {
        Self::extract_percent_inner(label_substring, context, false)
            .or_else(|| Self::extract_percent_inner(label_substring, context, true))
    }

    fn extract_percent_inner(
        label_substring: &str,
        context: &LabelSearchContext,
        allow_fuzzy: bool,
    ) -> Option<i32> {
        let label = Self::normalized_for_label_search(label_substring);
        for (idx, line) in context.lines.iter().enumerate() {
            let normalized_line = &context.normalized_lines[idx];
            if !Self::matches_label(line, normalized_line, label_substring, &label, allow_fuzzy) {
                continue;
            }

            let end = (idx + 12).min(context.lines.len());
            for candidate in &context.lines[idx..end] {
                if Self::crosses_label_boundary(candidate, label_substring, &label, allow_fuzzy) {
                    break;
                }
                if let Some(pct) = Self::percent_from_line(candidate, false) {
                    return Some(pct);
                }
            }
        }
        None
    }

    fn extract_reset_multi(labels: &[&str], context: &LabelSearchContext) -> Option<String> {
        for label in labels {
            if let Some(value) = Self::extract_reset(label, context) {
                return Some(value);
            }
        }
        None
    }

    fn extract_reset(label_substring: &str, context: &LabelSearchContext) -> Option<String> {
        Self::extract_reset_inner(label_substring, context, false)
            .or_else(|| Self::extract_reset_inner(label_substring, context, true))
    }

    fn extract_reset_inner(
        label_substring: &str,
        context: &LabelSearchContext,
        allow_fuzzy: bool,
    ) -> Option<String> {
        let label = Self::normalized_for_label_search(label_substring);
        for (idx, line) in context.lines.iter().enumerate() {
            let normalized_line = &context.normalized_lines[idx];
            if !Self::matches_label(line, normalized_line, label_substring, &label, allow_fuzzy) {
                continue;
            }

            let end = (idx + 14).min(context.lines.len());
            for candidate in &context.lines[idx..end] {
                if Self::crosses_label_boundary(candidate, label_substring, &label, allow_fuzzy) {
                    break;
                }
                if let Some(reset) = Self::reset_from_line(candidate) {
                    return Some(reset);
                }
            }
        }
        None
    }

    fn extract_scoped_weekly_usages(context: &LabelSearchContext) -> Vec<ScopedWeeklyUsage> {
        let mut usage_index_by_model: HashMap<String, usize> = HashMap::new();
        let mut usages: Vec<ScopedWeeklyUsage> = Vec::new();
        for (index, line) in context.lines.iter().enumerate() {
            let Some(model_name) = Self::weekly_model_name(line) else {
                continue;
            };
            let normalized_model = Self::normalized_for_label_search(&model_name);
            if normalized_model.is_empty() || Self::is_all_models_weekly_model(&normalized_model) {
                continue;
            }

            let end = (index + 14).min(context.lines.len());
            let mut percent_left = None;
            let mut reset_description = None;
            let normalized_label = Self::normalized_for_label_search(line);
            for candidate in &context.lines[index..end] {
                if Self::crosses_label_boundary(candidate, line, &normalized_label, false) {
                    break;
                }
                if percent_left.is_none() {
                    percent_left = Self::percent_from_line(candidate, false);
                }
                if reset_description.is_none() {
                    reset_description = Self::reset_from_line(candidate);
                }
            }
            let Some(percent_left) = percent_left else {
                continue;
            };
            let usage = ScopedWeeklyUsage {
                model_name,
                percent_left,
                reset_description,
            };
            if let Some(&existing_idx) = usage_index_by_model.get(&normalized_model) {
                usages[existing_idx] = usage;
            } else {
                usage_index_by_model.insert(normalized_model, usages.len());
                usages.push(usage);
            }
        }
        usages
    }

    fn extra_rate_windows(
        usages: &[ScopedWeeklyUsage],
        fallback_reset_description: Option<&str>,
        now: Timestamp,
        default_tz: &TimeZone,
    ) -> Vec<NamedRateWindow> {
        usages
            .iter()
            .filter_map(|usage| {
                let normalized_model = Self::normalized_for_label_search(&usage.model_name);
                if matches!(normalized_model.as_str(), "opus" | "sonnet" | "sonnetonly") {
                    return None;
                }
                let used_percent = (100.0 - f64::from(usage.percent_left)).clamp(0.0, 100.0);
                let reset_description = usage
                    .reset_description
                    .as_deref()
                    .or(fallback_reset_description)
                    .map(str::to_owned);
                let resets_at = Self::parse_reset_date_with_default_tz(
                    reset_description.as_deref(),
                    now,
                    Some(Duration::from_secs(7 * 24 * 60 * 60)),
                    default_tz,
                );
                ClaudeScopedWeeklyLimitMapper::extra_rate_window(
                    &usage.model_name,
                    used_percent,
                    resets_at,
                    reset_description,
                )
            })
            .collect()
    }

    fn matches_label(
        line: &str,
        normalized_line: &str,
        label_substring: &str,
        normalized_label: &str,
        allow_fuzzy: bool,
    ) -> bool {
        let Some(expected_model) = Self::weekly_model_name(label_substring) else {
            return normalized_line.contains(normalized_label);
        };
        let Some(actual_model) = Self::weekly_model_name(line) else {
            return false;
        };
        let expected_normalized = Self::normalized_for_label_search(&expected_model);
        let actual_normalized = Self::normalized_for_label_search(&actual_model);
        if allow_fuzzy && Self::is_all_models_weekly_model(&expected_normalized) {
            return Self::is_all_models_weekly_model(&actual_normalized);
        }
        actual_normalized == expected_normalized
    }

    fn crosses_label_boundary(
        line: &str,
        label_substring: &str,
        normalized_label: &str,
        allow_fuzzy: bool,
    ) -> bool {
        let normalized_line = Self::normalized_for_label_search(line);
        if !normalized_line.starts_with("current") {
            return false;
        }
        let Some(expected_model) = Self::weekly_model_name(label_substring) else {
            return !normalized_line.contains(normalized_label);
        };
        let Some(actual_model) = Self::weekly_model_name(line) else {
            return true;
        };
        let expected_normalized = Self::normalized_for_label_search(&expected_model);
        let actual_normalized = Self::normalized_for_label_search(&actual_model);
        if allow_fuzzy && Self::is_all_models_weekly_model(&expected_normalized) {
            return !Self::is_all_models_weekly_model(&actual_normalized);
        }
        actual_normalized != expected_normalized
    }

    fn weekly_model_name(line: &str) -> Option<String> {
        let lower = line.to_ascii_lowercase();
        let mut search_from = 0;
        while let Some(rel) = lower[search_from..].find("current") {
            let start = search_from + rel;
            let after_current = line[(start + "current".len())..].trim_start();
            if after_current.len() >= 4
                && after_current.as_bytes()[..4].eq_ignore_ascii_case(b"week")
            {
                let after_week = after_current[4..].trim_start();
                if let Some(after_open) = after_week.strip_prefix('(')
                    && let Some(close_idx) = after_open.find(')')
                {
                    return Some(after_open[..close_idx].trim().to_owned());
                }
            }
            search_from = start + "current".len();
        }
        None
    }

    fn is_all_models_weekly_model(normalized_model: &str) -> bool {
        normalized_model == "allmodels" || Self::edit_distance(normalized_model, "allmodels") <= 2
    }

    fn edit_distance(lhs: &str, rhs: &str) -> usize {
        let a: Vec<char> = lhs.chars().collect();
        let b: Vec<char> = rhs.chars().collect();
        if a.is_empty() {
            return b.len();
        }
        if b.is_empty() {
            return a.len();
        }
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, &ca) in a.iter().enumerate() {
            let mut prev_diag = row[0];
            row[0] = i + 1;
            for (j, &cb) in b.iter().enumerate() {
                let deletion = row[j + 1] + 1;
                let insertion = row[j] + 1;
                let substitution = prev_diag + usize::from(ca != cb);
                prev_diag = row[j + 1];
                row[j + 1] = deletion.min(insertion).min(substitution);
            }
        }
        row[b.len()]
    }

    pub fn normalized_for_label_search(text: &str) -> String {
        text.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    }

    fn percent_from_line(line: &str, assume_remaining_when_unclear: bool) -> Option<i32> {
        if Self::is_likely_status_context_line(line) {
            return None;
        }
        let raw_num = find_percent_token(line)?;
        let raw_val = raw_num.parse::<f64>().unwrap_or(0.0);
        let clamped = raw_val.clamp(0.0, 100.0);
        let lower = line.to_ascii_lowercase();
        let used_keywords = ["used", "spent", "consumed"];
        let remaining_keywords = ["left", "remaining", "available"];
        if used_keywords.iter().any(|k| lower.contains(k)) {
            return Some((100.0 - clamped).clamp(0.0, 100.0).round() as i32);
        }
        if remaining_keywords.iter().any(|k| lower.contains(k)) {
            return Some(clamped.round() as i32);
        }
        assume_remaining_when_unclear.then_some(clamped.round() as i32)
    }

    fn is_likely_status_context_line(line: &str) -> bool {
        if !line.contains('|') {
            return false;
        }
        let lower = line.to_ascii_lowercase();
        ["opus", "sonnet", "haiku", "default"]
            .iter()
            .any(|t| lower.contains(t))
    }

    fn all_percents(text: &str) -> Vec<i32> {
        let normalized: String = text
            .chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect();
        let has_usage_windows =
            normalized.contains("currentsession") || normalized.contains("currentweek");
        let has_loading = normalized.contains("loadingusage");
        let has_usage_percent_keywords = normalized.contains("used")
            || normalized.contains("left")
            || normalized.contains("remaining")
            || normalized.contains("available");
        let loading_only = has_loading && !has_usage_windows;
        if !(has_usage_windows || has_loading) || loading_only || !has_usage_percent_keywords {
            return Vec::new();
        }
        split_newline_components(text)
            .into_iter()
            .filter_map(|line| Self::percent_from_line(line, false))
            .collect()
    }

    fn trim_to_latest_usage_panel(text: &str) -> Option<String> {
        let settings_start = rfind_ascii_case_insensitive(text, "Settings:")?;
        let tail = &text[settings_start..];
        find_ascii_case_insensitive(tail, "Usage")?;
        let lower = tail.to_ascii_lowercase();
        let has_percent = lower.contains('%');
        let has_usage_words = lower.contains("used")
            || lower.contains("left")
            || lower.contains("remaining")
            || lower.contains("available");
        let has_loading = lower.contains("loading usage");
        if (has_percent && has_usage_words) || has_loading {
            Some(tail.to_owned())
        } else {
            None
        }
    }

    fn is_usage_still_loading(text: &str) -> bool {
        let normalized: String = Self::clean_capture(text)
            .chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect();
        if !normalized.contains("loadingusage") {
            return false;
        }
        !Self::usage_capture_has_session_value(&normalized) && Self::all_percents(text).is_empty()
    }

    fn is_subscription_notice_only(text: &str) -> bool {
        let normalized: String = text
            .chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect();
        if !normalized.contains("currentlyusingyoursubscription")
            || !normalized.contains("claudecodeusage")
        {
            return false;
        }
        let has_quota_data = normalized.contains("currentsession")
            || normalized.contains("currentweek")
            || normalized.contains("%used")
            || normalized.contains("%left")
            || normalized.contains("%remaining")
            || normalized.contains("%available");
        !has_quota_data
    }

    fn reset_from_line(line: &str) -> Option<String> {
        let start = find_reset_keyword_start(line)?;
        let raw = line[start..].trim();
        Some(Self::clean_reset_line(raw))
    }

    fn clean_reset_line(raw: &str) -> String {
        let mut cleaned = raw.trim().trim_matches(|c| c == ' ' || c == ')').to_owned();
        cleaned = repair_garbled_at_in_reset(&cleaned);
        let open_count = cleaned.chars().filter(|&c| c == '(').count();
        let close_count = cleaned.chars().filter(|&c| c == ')').count();
        if open_count > close_count {
            cleaned.push(')');
        }
        cleaned
    }

    fn extract_identity(usage_text: &str, status_text: Option<&str>) -> ClaudeCLIAccountIdentity {
        let status = status_text.unwrap_or("");
        let email = extract_labeled_email(usage_text)
            .or_else(|| extract_labeled_email(status))
            .or_else(|| extract_labeled_token(usage_text, &["Account:", "Email:"]))
            .or_else(|| extract_labeled_token(status, &["Account:", "Email:"]))
            .or_else(|| extract_standalone_email(usage_text))
            .or_else(|| extract_standalone_email(status));

        let org_raw = extract_labeled_line(usage_text, &["Org:", "Organization:"])
            .or_else(|| extract_labeled_line(status, &["Org:", "Organization:"]));
        let org = org_raw.and_then(|org_text| {
            let trimmed = org_text.trim();
            if trimmed.is_empty() {
                return None;
            }
            if let Some(ref em) = email
                && trimmed
                    .to_ascii_lowercase()
                    .starts_with(&em.to_ascii_lowercase())
            {
                return None;
            }
            Some(trimmed.to_owned())
        });

        let login =
            Self::extract_login_method(status).or_else(|| Self::extract_login_method(usage_text));

        ClaudeCLIAccountIdentity {
            account_email: email,
            account_organization: org,
            login_method: login,
        }
    }

    fn extract_login_method(text: &str) -> Option<String> {
        if text.is_empty() {
            return None;
        }
        if let Some(explicit) = extract_login_method_label(text) {
            let cleaned = Self::clean_plan_name(&explicit);
            return ClaudePlan::cli_compatibility_login_method(Some(&cleaned));
        }
        let mut candidates = Vec::new();
        for raw_match in find_claude_plan_phrases(text) {
            let cleaned = Self::clean_plan_name(&raw_match);
            let candidate =
                ClaudePlan::cli_compatibility_login_method(Some(&cleaned)).unwrap_or(cleaned);
            candidates.push(candidate);
        }
        candidates.into_iter().find(|cand| {
            let lower = cand.to_ascii_lowercase();
            !lower.contains("code v") && !lower.contains("code version") && !lower.contains("code")
        })
    }

    /// Matches `UsageFormatter.cleanPlanName`.
    pub fn clean_plan_name(text: &str) -> String {
        let stripped = strip_ansi_codes(text);
        let without_codes = strip_leading_orphan_sgr(&stripped);
        let without_boilerplate = remove_boilerplate_words(without_codes);
        let mut cleaned = collapse_whitespace(&without_boilerplate);
        if cleaned.is_empty() {
            cleaned = stripped.trim().to_owned();
        }
        if cleaned.eq_ignore_ascii_case("oauth") {
            return "Ollama".to_owned();
        }
        let mut chars = cleaned.chars();
        if let Some(first) = chars.next()
            && first.is_lowercase()
        {
            let mut result: String = first.to_uppercase().collect();
            result.push_str(chars.as_str());
            return result;
        }
        cleaned
    }

    fn extract_usage_error(text: &str) -> Option<String> {
        if let Some(json_hint) = Self::extract_usage_error_json(text) {
            return Some(json_hint);
        }

        let lower = text.to_ascii_lowercase();
        let compact: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
        if lower.contains("do you trust the files in this folder?")
            && !lower.contains("current session")
        {
            let folder_hint = extract_folder_after_trust_prompt(text);
            if let Some(folder_hint) = folder_hint {
                return Some(format!(
                    "Claude CLI is waiting for a folder trust prompt ({folder_hint}). AriadUsage tries to auto-accept this, but if it keeps appearing run: `cd \"{folder_hint}\" && claude` and choose \u{201c}Yes, proceed\u{201d}, then retry."
                ));
            }
            return Some(
                "Claude CLI is waiting for a folder trust prompt. AriadUsage tries to auto-accept this, but if it keeps appearing open `claude` once, choose \u{201c}Yes, proceed\u{201d}, then retry."
                    .to_owned(),
            );
        }

        let failure_line = Self::usage_failure_line(text)
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        if failure_line.contains("token_expired") || failure_line.contains("token has expired") {
            return Some("Claude CLI token expired. Run `claude login` to refresh.".to_owned());
        }
        if failure_line.contains("authentication_error") {
            return Some("Claude CLI authentication error. Run `claude login`.".to_owned());
        }
        let compact_failure: String = failure_line
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if failure_line.contains("rate_limit_error")
            || failure_line.contains("rate limited")
            || compact_failure.contains("ratelimited")
        {
            return Some(
                "Claude CLI usage endpoint is rate limited right now. Please try again later."
                    .to_owned(),
            );
        }
        if Self::is_subscription_notice_only(text) {
            return if find_ascii_case_insensitive(text, USAGE_INSIGHTS_MARKER).is_some() {
                Some("Claude CLI /usage returned usage insights without quota data.".to_owned())
            } else {
                Some(SUBSCRIPTION_QUOTA_UNAVAILABLE_DESCRIPTION.to_owned())
            };
        }
        if lower.contains("failed to load usage data") || compact.contains("failedtoloadusagedata")
        {
            return Some(
                "Claude CLI could not load usage data. Open the CLI and retry `/usage`.".to_owned(),
            );
        }
        None
    }

    fn usage_failure_line(text: &str) -> Option<String> {
        let mut last_match = None;
        for line in split_newline_components(text) {
            if let Some(pos) = find_failed_to_load_usage_data(line) {
                last_match = Some(line[pos..].to_owned());
            }
        }
        last_match
    }

    fn extract_usage_error_json(text: &str) -> Option<String> {
        let start = find_failed_to_load_usage_data_end(text)?;
        let after = text[start..].trim_start();
        if !after.starts_with('{') {
            return None;
        }
        let close = after.rfind('}')?;
        let json_slice = &after[..=close];
        let compact_json = json_slice.replace(['\r', '\n'], "");
        let payload: Value = serde_json::from_str(if compact_json.is_empty() {
            json_slice
        } else {
            &compact_json
        })
        .ok()?;
        let error = payload.get("error")?.as_object()?;

        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let details = error.get("details").and_then(Value::as_object);
        let code = details
            .and_then(|d| d.get("error_code"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let error_type = error
            .get("type")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_ascii_lowercase());

        if error_type.as_deref() == Some("rate_limit_error") {
            return Some(
                "Claude CLI usage endpoint is rate limited right now. Please try again later."
                    .to_owned(),
            );
        }

        let mut parts = Vec::new();
        if let Some(m) = message {
            parts.push(m.to_owned());
        }
        if let Some(c) = code {
            parts.push(format!("({c})"));
        }
        if parts.is_empty() {
            return None;
        }
        let hint = parts.join(" ");
        if let Some(c) = code
            && c.to_ascii_lowercase().contains("token")
        {
            return Some(format!("{hint}. Run `claude login` to refresh."));
        }
        if matches!(
            error_type.as_deref(),
            Some("authentication_error" | "permission_error")
        ) {
            return Some(format!(
                "Claude CLI authentication error: {hint}. Run `claude login`."
            ));
        }
        Some(format!("Claude CLI error: {hint}"))
    }

    fn usage_probe_error(message: String) -> ClaudeStatusProbeError {
        let lower = message.to_ascii_lowercase();
        let auth_markers = [
            "authentication_error",
            "permission_error",
            "token_expired",
            "token expired",
            "token has expired",
            "oauth account information not found",
            "does not have access to claude code",
            "run /login",
            "run `claude login`",
            "run claude login",
            "api error: 401",
            "api error: 403",
            "forbidden",
            "invalid api key",
            "invalid_api_key",
            "invalid credential",
            "not authenticated",
            "not authorized",
            "token revoked",
            "unauthorized",
        ];
        if auth_markers.iter().any(|m| lower.contains(m)) {
            ClaudeStatusProbeError::AuthenticationFailed(message)
        } else {
            ClaudeStatusProbeError::ParseFailed(message)
        }
    }

    // MARK: - Reset date parsing

    pub fn parse_reset_date(text: Option<&str>, now: Timestamp) -> Option<Timestamp> {
        Self::parse_reset_date_with_window(text, now, None)
    }

    pub fn parse_reset_date_with_window(
        text: Option<&str>,
        now: Timestamp,
        expected_window: Option<Duration>,
    ) -> Option<Timestamp> {
        Self::parse_reset_date_with_default_tz(text, now, expected_window, &TimeZone::system())
    }

    pub fn parse_reset_date_with_default_tz(
        text: Option<&str>,
        now: Timestamp,
        expected_window: Option<Duration>,
        default_tz: &TimeZone,
    ) -> Option<Timestamp> {
        let (raw, explicit_tz) = Self::normalize_reset_input(text)?;
        let tz = explicit_tz.as_ref().unwrap_or(default_tz);

        if let Some((year, month, day, hour, minute)) = parse_explicit_year_datetime(&raw) {
            let candidates = Self::exact_local_occurrences(year, month, day, hour, minute, tz);
            return Self::resolve_occurrence(candidates, now, expected_window);
        }

        if let Some((month, day, hour, minute)) = parse_month_day_datetime_with_minutes(&raw) {
            let candidates = Self::yearly_occurrences(month, day, hour, minute, now, tz);
            return Self::resolve_occurrence(candidates, now, expected_window);
        }

        if let Some((month, day, hour)) = parse_month_day_datetime_hour_only(&raw) {
            let candidates = Self::yearly_occurrences(month, day, hour, 0, now, tz);
            return Self::resolve_occurrence(candidates, now, expected_window);
        }

        if let Some((hour, minute)) = parse_time_with_minutes(&raw) {
            let candidates = Self::daily_occurrences(hour, minute, now, tz);
            return Self::resolve_occurrence(candidates, now, expected_window);
        }

        if let Some(hour) = parse_time_hour_only(&raw) {
            let candidates = Self::daily_occurrences(hour, 0, now, tz);
            return Self::resolve_occurrence(candidates, now, expected_window);
        }

        None
    }

    fn resolve_occurrence(
        mut candidates: Vec<Timestamp>,
        now: Timestamp,
        expected_window: Option<Duration>,
    ) -> Option<Timestamp> {
        if candidates.is_empty() {
            return None;
        }
        candidates.sort_unstable();
        candidates.dedup();

        let Some(&future) = candidates.iter().find(|&&d| d >= now) else {
            return candidates.last().copied();
        };
        let Some(expected_window) = expected_window else {
            return Some(future);
        };

        let horizon_secs = expected_window.as_secs_f64().max(0.0);
        let past = candidates.iter().rev().find(|&&d| d < now).copied();
        let past_is_plausible =
            past.is_some_and(|p| now.duration_since(p).as_secs_f64() <= horizon_secs);
        let future_is_plausible = future.duration_since(now).as_secs_f64() <= horizon_secs;
        if past_is_plausible && !future_is_plausible {
            past
        } else {
            Some(future)
        }
    }

    fn yearly_occurrences(
        month: i8,
        day: i8,
        hour: i8,
        minute: i8,
        now: Timestamp,
        tz: &TimeZone,
    ) -> Vec<Timestamp> {
        let current_year = now.to_zoned(tz.clone()).year();
        let mut results = Vec::new();
        for year in (current_year - 8)..=(current_year + 8) {
            results.extend(Self::exact_local_occurrences(
                year, month, day, hour, minute, tz,
            ));
        }
        results
    }

    fn daily_occurrences(hour: i8, minute: i8, now: Timestamp, tz: &TimeZone) -> Vec<Timestamp> {
        let today = now.to_zoned(tz.clone()).date();
        let mut results = Vec::new();
        for offset in -1..=1 {
            let Some(date) = (if offset < 0 {
                today.yesterday().ok()
            } else if offset > 0 {
                today.tomorrow().ok()
            } else {
                Some(today)
            }) else {
                continue;
            };
            results.extend(Self::exact_local_occurrences(
                date.year(),
                date.month(),
                date.day(),
                hour,
                minute,
                tz,
            ));
        }
        results
    }

    fn exact_local_occurrences(
        year: i16,
        month: i8,
        day: i8,
        hour: i8,
        minute: i8,
        tz: &TimeZone,
    ) -> Vec<Timestamp> {
        let Ok(civil_dt) = DateTime::new(year, month, day, hour, minute, 0, 0) else {
            return Vec::new();
        };
        let ambiguous = tz.to_ambiguous_zoned(civil_dt);
        match ambiguous.offset() {
            AmbiguousOffset::Unambiguous { .. } => ambiguous
                .unambiguous()
                .map(|zdt| vec![zdt.timestamp()])
                .unwrap_or_default(),
            AmbiguousOffset::Fold { .. } => {
                let mut out = Vec::new();
                if let Ok(before) = ambiguous.clone().earlier() {
                    out.push(before.timestamp());
                }
                if let Ok(after) = ambiguous.later() {
                    let ts = after.timestamp();
                    if !out.contains(&ts) {
                        out.push(ts);
                    }
                }
                out
            }
            AmbiguousOffset::Gap { .. } => Vec::new(),
        }
    }

    fn normalize_reset_input(text: Option<&str>) -> Option<(String, Option<TimeZone>)> {
        let mut raw = text?.trim().to_owned();
        if raw.is_empty() {
            return None;
        }
        raw = strip_leading_resets_prefix(&raw);
        raw = replace_case_insensitive_at(&raw);
        raw = insert_space_month_digit(&raw);
        raw = insert_space_after_comma_digit(&raw);
        raw = replace_digit_at_digit(&raw);
        raw = replace_dot_minutes(&raw);

        let tz = Self::extract_timezone(&mut raw);
        raw = collapse_whitespace(&raw);
        if raw.is_empty() {
            None
        } else {
            Some((raw, tz))
        }
    }

    fn extract_timezone(text: &mut String) -> Option<TimeZone> {
        let open = text.find('(')?;
        let close_rel = text[open..].find(')')?;
        let close = open + close_rel;
        let tz_id = text[(open + 1)..close].trim().to_owned();
        text.replace_range(open..=close, "");
        *text = text.trim().to_owned();
        if tz_id.eq_ignore_ascii_case("UTC") || tz_id.eq_ignore_ascii_case("GMT") {
            Some(TimeZone::UTC)
        } else {
            TimeZone::get(&tz_id).ok()
        }
    }

    /// Parses Claude CLI JSON output (`ClaudeUsageFetcher.parse(json:now:)`).
    pub fn parse_json(json: &[u8], now: Timestamp) -> Option<UsageSnapshot> {
        Self::parse_json_with_tz(json, now, &TimeZone::system())
    }

    pub fn parse_json_with_tz(
        json: &[u8],
        now: Timestamp,
        default_tz: &TimeZone,
    ) -> Option<UsageSnapshot> {
        let output = std::str::from_utf8(json).ok()?;
        let obj_val: Value = serde_json::from_str(output).ok()?;
        let obj = obj_val.as_object()?;

        if let Some(ok) = obj.get("ok").and_then(Value::as_bool)
            && !ok
        {
            return None;
        }

        let first_window_dict = |keys: &[&str]| -> Option<&serde_json::Map<String, Value>> {
            keys.iter()
                .find_map(|&k| obj.get(k).and_then(Value::as_object))
        };

        let make_window = |dict: Option<&serde_json::Map<String, Value>>,
                           window_minutes: u32|
         -> Option<RateWindow> {
            let dict = dict?;
            let pct = dict.get("pct_used").and_then(Value::as_f64).unwrap_or(0.0);
            let reset_text = dict.get("resets").and_then(Value::as_str);
            let resets_at = Self::parse_reset_date_with_default_tz(
                reset_text,
                now,
                Some(Duration::from_secs(u64::from(window_minutes) * 60)),
                default_tz,
            );
            RateWindow::new(
                pct,
                Some(i64::from(window_minutes)),
                resets_at,
                reset_text.map(str::to_owned),
                None,
                false,
            )
            .ok()
        };

        let session = make_window(first_window_dict(&["session_5h"]), SESSION_WINDOW_MINUTES)?;
        let week_all = make_window(
            first_window_dict(&["week_all_models", "week_all"]),
            WEEKLY_WINDOW_MINUTES,
        );
        let opus_window = make_window(
            first_window_dict(&["week_sonnet", "week_sonnet_only", "week_opus"]),
            WEEKLY_WINDOW_MINUTES,
        );

        let email = obj
            .get("account_email")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let org = obj
            .get("account_org")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let login_method = obj
            .get("login_method")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_owned());

        let identity = ProviderIdentitySnapshot {
            provider_id: ProviderId::new("claude").ok(),
            account_email: email,
            account_organization: org,
            login_method,
            account_id: None,
        };

        UsageSnapshot::new(
            Some(session),
            week_all,
            opus_window,
            None,
            None,
            Vec::new(),
            None,
            None,
            now,
            Some(identity),
            Confidence::Unknown,
        )
        .ok()
    }
}

// MARK: - String & date helpers without regex

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let h_bytes = haystack.as_bytes();
    let n_bytes = needle.as_bytes();
    for i in 0..=(h_bytes.len() - n_bytes.len()) {
        if h_bytes[i..(i + n_bytes.len())].eq_ignore_ascii_case(n_bytes) {
            return Some((i, i + n_bytes.len()));
        }
    }
    None
}

fn rfind_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let h_bytes = haystack.as_bytes();
    let n_bytes = needle.as_bytes();
    for i in (0..=(h_bytes.len() - n_bytes.len())).rev() {
        if h_bytes[i..(i + n_bytes.len())].eq_ignore_ascii_case(n_bytes) {
            return Some(i);
        }
    }
    None
}

/// Finds the first `([0-9]{1,3}(?:\.[0-9]+)?)\p{Zs}*%` token and returns the number substring.
fn find_percent_token(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let mut digit_len = 0;
        while i + digit_len < bytes.len() && digit_len < 3 && bytes[i + digit_len].is_ascii_digit()
        {
            digit_len += 1;
        }
        let mut num_end = i + digit_len;
        if num_end < bytes.len() && bytes[num_end] == b'.' {
            let frac_start = num_end + 1;
            let mut frac_len = 0;
            while frac_start + frac_len < bytes.len()
                && bytes[frac_start + frac_len].is_ascii_digit()
            {
                frac_len += 1;
            }
            if frac_len > 0 {
                num_end = frac_start + frac_len;
            }
        }
        let after_num = &line[num_end..];
        let after_space =
            after_num.trim_start_matches(|c: char| c.is_whitespace() && c != '\n' && c != '\r');
        if after_space.starts_with('%') {
            return Some(&line[i..num_end]);
        }
        i += 1;
    }
    None
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Matches `(?i)\bresets?\b` or `\b(?:Reset|Resets)(?=[A-Z0-9])`.
fn find_reset_keyword_start(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    for i in 0..bytes.len() {
        if i > 0 && is_word_char(bytes[i - 1]) {
            continue;
        }
        let rem = &bytes[i..];
        let matched_len = if rem.len() >= 6 && rem[..6].eq_ignore_ascii_case(b"resets") {
            Some(6)
        } else if rem.len() >= 5 && rem[..5].eq_ignore_ascii_case(b"reset") {
            Some(5)
        } else {
            None
        };
        if let Some(len) = matched_len {
            let after_idx = i + len;
            if after_idx == bytes.len() || !is_word_char(bytes[after_idx]) {
                return Some(i);
            }
        }
    }
    for i in 0..bytes.len() {
        if i > 0 && is_word_char(bytes[i - 1]) {
            continue;
        }
        let rem = &bytes[i..];
        let matched_len = if rem.starts_with(b"Resets") {
            Some(6)
        } else if rem.starts_with(b"Reset") {
            Some(5)
        } else {
            None
        };
        if let Some(len) = matched_len {
            let after_idx = i + len;
            if after_idx < bytes.len()
                && (bytes[after_idx].is_ascii_uppercase() || bytes[after_idx].is_ascii_digit())
            {
                return Some(i);
            }
        }
    }
    None
}

/// Replaces `(?i)\b([A-Za-z]{3}\s+\d{1,2})\s+t\s+(\d)` with `$1 at $2`.
fn repair_garbled_at_in_reset(input: &str) -> String {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    if tokens.len() < 4 {
        return input.to_owned();
    }
    for i in 0..=(tokens.len() - 4) {
        let mon = tokens[i];
        let day = tokens[i + 1];
        let t_tok = tokens[i + 2];
        let next_tok = tokens[i + 3];
        if mon.len() == 3
            && mon.chars().all(|c| c.is_ascii_alphabetic())
            && (1..=2).contains(&day.len())
            && day.chars().all(|c| c.is_ascii_digit())
            && t_tok.eq_ignore_ascii_case("t")
            && next_tok.starts_with(|c: char| c.is_ascii_digit())
        {
            if let Some(pos) = input.find(" t ") {
                let mut out = String::with_capacity(input.len() + 1);
                out.push_str(&input[..pos]);
                out.push_str(" at ");
                out.push_str(&input[(pos + 3)..]);
                return out;
            }
            if let Some(pos) = input.find(" T ") {
                let mut out = String::with_capacity(input.len() + 1);
                out.push_str(&input[..pos]);
                out.push_str(" at ");
                out.push_str(&input[(pos + 3)..]);
                return out;
            }
        }
    }
    input.to_owned()
}

fn extract_labeled_email(text: &str) -> Option<String> {
    let lines = split_newline_components(text);
    for label in ["Account:", "Email:"] {
        for &line in &lines {
            if let Some((_, end)) = find_ascii_case_insensitive(line, label) {
                let rest = line[end..].trim_start();
                let token = rest.split_whitespace().next().unwrap_or("");
                if let Some(at_pos) = token.find('@')
                    && at_pos > 0
                    && at_pos + 1 < token.len()
                    && !token[(at_pos + 1)..].contains('@')
                {
                    return Some(token.to_owned());
                }
            }
        }
    }
    None
}

fn extract_labeled_token(text: &str, labels: &[&str]) -> Option<String> {
    let lines = split_newline_components(text);
    for &label in labels {
        for &line in &lines {
            if let Some((_, end)) = find_ascii_case_insensitive(line, label) {
                let rest = line[end..].trim_start();
                let token = rest.split_whitespace().next().unwrap_or("");
                if !token.is_empty() {
                    return Some(token.to_owned());
                }
            }
        }
    }
    None
}

fn extract_labeled_line(text: &str, labels: &[&str]) -> Option<String> {
    let lines = split_newline_components(text);
    for &label in labels {
        for &line in &lines {
            if let Some((_, end)) = find_ascii_case_insensitive(line, label) {
                let val = line[end..].trim();
                if !val.is_empty() {
                    return Some(val.to_owned());
                }
            }
        }
    }
    None
}

fn extract_standalone_email(text: &str) -> Option<String> {
    for token in text.split_whitespace() {
        let trimmed = token.trim_matches(|c: char| {
            !c.is_ascii_alphanumeric() && !matches!(c, '.' | '_' | '%' | '+' | '-' | '@')
        });
        let Some(at_pos) = trimmed.find('@') else {
            continue;
        };
        let local = &trimmed[..at_pos];
        let domain = &trimmed[(at_pos + 1)..];
        if local.is_empty()
            || !local
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-'))
        {
            continue;
        }
        let Some(dot_pos) = domain.rfind('.') else {
            continue;
        };
        let host = &domain[..dot_pos];
        let tld = &domain[(dot_pos + 1)..];
        if !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
            && tld.len() >= 2
            && tld.chars().all(|c| c.is_ascii_alphabetic())
        {
            return Some(trimmed.to_owned());
        }
    }
    None
}

fn extract_login_method_label(text: &str) -> Option<String> {
    for line in split_newline_components(text) {
        let lower = line.to_ascii_lowercase();
        if let Some(pos) = lower.find("login") {
            let after_login = line[(pos + "login".len())..].trim_start();
            if after_login.len() >= 7
                && after_login.as_bytes()[..7].eq_ignore_ascii_case(b"method:")
            {
                let value = after_login[7..].trim();
                if !value.is_empty() {
                    return Some(value.to_owned());
                }
            }
        }
    }
    None
}

/// Finds all matches of `(?i)(claude[ \t]+[a-z0-9][a-z0-9 \t._-]{0,24})` in `text`.
fn find_claude_plan_phrases(text: &str) -> Vec<String> {
    let mut results = Vec::new();
    for line in split_newline_components(text) {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i + 7 <= bytes.len() {
            if bytes[i..(i + 6)].eq_ignore_ascii_case(b"claude")
                && matches!(bytes[i + 6], b' ' | b'\t')
            {
                let mut j = i + 6;
                while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
                    j += 1;
                }
                if j < bytes.len() && bytes[j].is_ascii_alphanumeric() {
                    let tail_start = j + 1;
                    let mut tail_len = 0;
                    while tail_start + tail_len < bytes.len() && tail_len < 24 {
                        let b = bytes[tail_start + tail_len];
                        if b.is_ascii_alphanumeric()
                            || matches!(b, b' ' | b'\t' | b'.' | b'_' | b'-')
                        {
                            tail_len += 1;
                        } else {
                            break;
                        }
                    }
                    let end = tail_start + tail_len;
                    results.push(line[i..end].to_owned());
                    i = end;
                    continue;
                }
            }
            i += 1;
        }
    }
    results
}

fn strip_ansi_codes(text: &str) -> String {
    ClaudeCLIScreen::render_with_options(text, true)
}

fn strip_leading_orphan_sgr(mut s: &str) -> &str {
    loop {
        let trimmed = s.trim_start();
        let Some(after_bracket) = trimmed.strip_prefix('[') else {
            return s;
        };
        let bytes = after_bracket.as_bytes();
        let mut digits = 0;
        while digits < bytes.len() && digits < 3 && bytes[digits].is_ascii_digit() {
            digits += 1;
        }
        if digits == 0 || digits >= bytes.len() || bytes[digits] != b'm' {
            return s;
        }
        s = after_bracket[(digits + 1)..].trim_start();
    }
}

fn remove_boilerplate_words(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_alphanumeric() || chars[i] == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if !matches!(
                word.to_ascii_lowercase().as_str(),
                "claude" | "codex" | "account" | "plan"
            ) {
                result.push_str(&word);
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_folder_after_trust_prompt(text: &str) -> Option<String> {
    let (_, end) = find_ascii_case_insensitive(text, "Do you trust the files in this folder?")?;
    let after = &text[end..];
    let lines = split_newline_components(after);
    if lines.len() < 2 || !lines[0].trim().is_empty() {
        return None;
    }
    for &line in &lines[1..] {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    None
}

/// Finds the start index of `(?i)failed\s*to\s*load\s*usage\s*data` in `text`.
fn find_failed_to_load_usage_data(text: &str) -> Option<usize> {
    find_failed_to_load_usage_data_span(text, true).map(|(s, _)| s)
}

/// Finds the end index of `Failed\s*to\s*load\s*usage\s*data:` (case-sensitive) in `text`.
fn find_failed_to_load_usage_data_end(text: &str) -> Option<usize> {
    let (_, end) = find_failed_to_load_usage_data_span(text, false)?;
    let rem = text[end..].trim_start();
    let after_colon = rem.strip_prefix(':')?;
    Some(text.len() - after_colon.len())
}

fn find_failed_to_load_usage_data_span(
    text: &str,
    case_insensitive: bool,
) -> Option<(usize, usize)> {
    let words = if case_insensitive {
        ["failed", "to", "load", "usage", "data"]
    } else {
        ["Failed", "to", "load", "usage", "data"]
    };
    let bytes = text.as_bytes();
    for start in 0..bytes.len() {
        let mut cursor = start;
        let mut matched = true;
        for (idx, word) in words.iter().enumerate() {
            if idx > 0 {
                while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                    cursor += 1;
                }
            }
            if cursor + word.len() > bytes.len() {
                matched = false;
                break;
            }
            let slice = &bytes[cursor..(cursor + word.len())];
            let word_matches = if case_insensitive {
                slice.eq_ignore_ascii_case(word.as_bytes())
            } else {
                slice == word.as_bytes()
            };
            if !word_matches {
                matched = false;
                break;
            }
            cursor += word.len();
        }
        if matched {
            return Some((start, cursor));
        }
    }
    None
}

fn strip_leading_resets_prefix(raw: &str) -> String {
    let trimmed = raw.trim_start();
    let trimmed_bytes = trimmed.as_bytes();
    let after_word =
        if trimmed_bytes.len() >= 6 && trimmed_bytes[..6].eq_ignore_ascii_case(b"resets") {
            &trimmed[6..]
        } else if trimmed_bytes.len() >= 5 && trimmed_bytes[..5].eq_ignore_ascii_case(b"reset") {
            &trimmed[5..]
        } else {
            return raw.to_owned();
        };
    let after_colon = after_word.strip_prefix(':').unwrap_or(after_word);
    after_colon.trim_start().to_owned()
}

fn replace_case_insensitive_at(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 4 <= bytes.len() && bytes[i..(i + 4)].eq_ignore_ascii_case(b" at ") {
            out.push(' ');
            i += 4;
        } else {
            let ch = raw[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn insert_space_month_digit(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len() + 2);
    let mut i = 0;
    while i < chars.len() {
        let at_word_start =
            i == 0 || (!chars[i - 1].is_ascii_alphanumeric() && chars[i - 1] != '_');
        if at_word_start
            && i + 3 < chars.len()
            && chars[i].is_ascii_alphabetic()
            && chars[i + 1].is_ascii_alphabetic()
            && chars[i + 2].is_ascii_alphabetic()
            && chars[i + 3].is_ascii_digit()
        {
            out.push(chars[i]);
            out.push(chars[i + 1]);
            out.push(chars[i + 2]);
            out.push(' ');
            i += 3;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn insert_space_after_comma_digit(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len() + 2);
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ',' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
            out.push(',');
            out.push(' ');
            i += 1;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn replace_digit_at_digit(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < chars.len() {
        if i + 3 < chars.len()
            && chars[i].is_ascii_digit()
            && chars[i + 1].eq_ignore_ascii_case(&'a')
            && chars[i + 2].eq_ignore_ascii_case(&'t')
            && chars[i + 3].is_ascii_digit()
        {
            out.push(chars[i]);
            out.push(' ');
            i += 3;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn replace_dot_minutes(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < chars.len() {
        if i > 0
            && chars[i - 1].is_ascii_digit()
            && chars[i] == '.'
            && i + 2 < chars.len()
            && chars[i + 1].is_ascii_digit()
            && chars[i + 2].is_ascii_digit()
            && (i + 3 == chars.len()
                || (!chars[i + 3].is_ascii_alphanumeric() && chars[i + 3] != '_'))
        {
            out.push(':');
            out.push(chars[i + 1]);
            out.push(chars[i + 2]);
            i += 3;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn parse_month_abbrev(s: &str) -> Option<i8> {
    match s.to_ascii_lowercase().as_str() {
        "jan" => Some(1),
        "feb" => Some(2),
        "mar" => Some(3),
        "apr" => Some(4),
        "may" => Some(5),
        "jun" => Some(6),
        "jul" => Some(7),
        "aug" => Some(8),
        "sep" => Some(9),
        "oct" => Some(10),
        "nov" => Some(11),
        "dec" => Some(12),
        _ => None,
    }
}

fn parse_explicit_year_datetime(raw: &str) -> Option<(i16, i8, i8, i8, i8)> {
    let (mon_tok, rest) = raw.split_once(' ')?;
    let month = parse_month_abbrev(mon_tok)?;
    let (day_tok, rest) = rest.split_once(' ')?;
    let day_str = day_tok.strip_suffix(',').unwrap_or(day_tok);
    let day: i8 = day_str.parse().ok()?;
    let (year_tok, time_part) = rest.split_once(' ')?;
    let year_str = year_tok.strip_suffix(',').unwrap_or(year_tok);
    if year_str.len() != 4 || !year_str.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let year: i16 = year_str.parse().ok()?;
    Date::new(year, month, day).ok()?;

    if let Some((hour, minute)) = parse_time_with_minutes(time_part) {
        return Some((year, month, day, hour, minute));
    }
    if let Some(hour) = parse_time_hour_only(time_part) {
        return Some((year, month, day, hour, 0));
    }
    if time_part.len() == 2
        && time_part.chars().all(|c| c.is_ascii_digit())
        && let Ok(hour) = time_part.parse::<i8>()
        && (0..=23).contains(&hour)
    {
        return Some((year, month, day, hour, 0));
    }
    None
}

fn parse_month_day_datetime_with_minutes(raw: &str) -> Option<(i8, i8, i8, i8)> {
    let (mon_tok, rest) = raw.split_once(' ')?;
    let month = parse_month_abbrev(mon_tok)?;
    let (day_tok, time_part) = rest.split_once(' ')?;
    let day_str = day_tok.strip_suffix(',').unwrap_or(day_tok);
    let day: i8 = day_str.parse().ok()?;
    // Validate month/day against leap year 2000 (so Feb 29 parses and resolves across leap years).
    Date::new(2000, month, day).ok()?;
    let (hour, minute) = parse_time_with_minutes(time_part)?;
    Some((month, day, hour, minute))
}

fn parse_month_day_datetime_hour_only(raw: &str) -> Option<(i8, i8, i8)> {
    let (mon_tok, rest) = raw.split_once(' ')?;
    let month = parse_month_abbrev(mon_tok)?;
    let (day_tok, time_part) = rest.split_once(' ')?;
    let day_str = day_tok.strip_suffix(',').unwrap_or(day_tok);
    let day: i8 = day_str.parse().ok()?;
    Date::new(2000, month, day).ok()?;
    let hour = parse_time_hour_only(time_part)?;
    Some((month, day, hour))
}

fn parse_time_with_minutes(raw: &str) -> Option<(i8, i8)> {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    if let Some(without_meridiem) = lower
        .strip_suffix("am")
        .or_else(|| lower.strip_suffix("pm"))
    {
        let is_pm = lower.ends_with("pm");
        let hm = without_meridiem.trim();
        let (h_str, m_str) = hm.split_once(':')?;
        if m_str.len() != 2 || !m_str.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let h12: i8 = h_str.parse().ok()?;
        let minute: i8 = m_str.parse().ok()?;
        if !(1..=12).contains(&h12) || !(0..=59).contains(&minute) {
            return None;
        }
        let hour = match (h12, is_pm) {
            (12, false) => 0,
            (12, true) => 12,
            (h, false) => h,
            (h, true) => h + 12,
        };
        Some((hour, minute))
    } else {
        let (h_str, m_str) = trimmed.split_once(':')?;
        if !(1..=2).contains(&h_str.len())
            || m_str.len() != 2
            || !h_str.chars().all(|c| c.is_ascii_digit())
            || !m_str.chars().all(|c| c.is_ascii_digit())
        {
            return None;
        }
        let hour: i8 = h_str.parse().ok()?;
        let minute: i8 = m_str.parse().ok()?;
        if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
            return None;
        }
        Some((hour, minute))
    }
}

fn parse_time_hour_only(raw: &str) -> Option<i8> {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    let without_meridiem = lower
        .strip_suffix("am")
        .or_else(|| lower.strip_suffix("pm"))?;
    let is_pm = lower.ends_with("pm");
    let h_str = without_meridiem.trim();
    if h_str.is_empty() || !h_str.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let h12: i8 = h_str.parse().ok()?;
    if !(1..=12).contains(&h12) {
        return None;
    }
    Some(match (h12, is_pm) {
        (12, false) => 0,
        (12, true) => 12,
        (h, false) => h,
        (h, true) => h + 12,
    })
}
