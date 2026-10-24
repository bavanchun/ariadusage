// Ported from CodexBar Sources/CodexBarCore/Hooks/HookTransitionDetector.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Hooks/HookEvent.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Hooks/HookRule.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Hooks/HookEvent+UsageUpdated.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/SessionQuotaNotifications.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+SessionQuotaTransition.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+QuotaWarnings.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, HashMap, HashSet};

use ariadusage_protocol::StatusIndicator as ProviderStatusIndicator;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::config::QuotaWarningWindow;
use crate::model::{RateWindow, UsageSnapshot};
use crate::providers;

/// Quota or provider state changes that can trigger external hook commands or notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEventType {
    #[serde(rename = "quota_low")]
    QuotaLow,
    #[serde(rename = "quota_reached")]
    QuotaReached,
    #[serde(rename = "quota_reset")]
    QuotaReset,
    #[serde(rename = "usage_updated")]
    UsageUpdated,
    #[serde(rename = "provider_unavailable")]
    ProviderUnavailable,
    #[serde(rename = "provider_recovered")]
    ProviderRecovered,
    #[serde(rename = "refresh_failed")]
    RefreshFailed,
}

impl HookEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::QuotaLow => "quota_low",
            Self::QuotaReached => "quota_reached",
            Self::QuotaReset => "quota_reset",
            Self::UsageUpdated => "usage_updated",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::ProviderRecovered => "provider_recovered",
            Self::RefreshFailed => "refresh_failed",
        }
    }

    pub fn is_rate_limited(&self) -> bool {
        matches!(
            self,
            Self::UsageUpdated | Self::ProviderUnavailable | Self::RefreshFailed
        )
    }
}

/// Non-secret metadata for a single quota or provider transition event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookEvent {
    pub event: HookEventType,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_usage_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_window_minutes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_reset_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub timestamp: Timestamp,
}

impl HookEvent {
    pub fn usage_updated(
        provider: impl Into<String>,
        snapshot: &UsageSnapshot,
        account: Option<String>,
        timestamp: Timestamp,
    ) -> Self {
        let primary = snapshot
            .primary
            .as_ref()
            .filter(|w| !w.is_synthetic_placeholder);
        let secondary = snapshot
            .secondary
            .as_ref()
            .filter(|w| !w.is_synthetic_placeholder);
        Self {
            event: HookEventType::UsageUpdated,
            provider: provider.into(),
            account,
            window: None,
            usage_percent: primary.map(|w| w.used_percent / 100.0),
            window_minutes: primary.and_then(|w| w.window_minutes),
            used: None,
            limit: None,
            reset_at: primary.and_then(|w| w.resets_at),
            secondary_usage_percent: secondary.map(|w| w.used_percent / 100.0),
            secondary_window_minutes: secondary.and_then(|w| w.window_minutes),
            secondary_reset_at: secondary.and_then(|w| w.resets_at),
            status: None,
            timestamp,
        }
    }

    pub fn environment_variables(&self) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        env.insert(
            "CODEXBAR_EVENT".to_string(),
            self.event.as_str().to_string(),
        );
        env.insert("CODEXBAR_PROVIDER".to_string(), self.provider.clone());
        env.insert("CODEXBAR_TIMESTAMP".to_string(), self.timestamp.to_string());
        if let Some(account) = &self.account {
            env.insert("CODEXBAR_ACCOUNT".to_string(), account.clone());
        }
        if let Some(window) = &self.window {
            env.insert("CODEXBAR_WINDOW".to_string(), window.clone());
        }
        if let Some(usage_percent) = self.usage_percent {
            env.insert(
                "CODEXBAR_USAGE_PERCENT".to_string(),
                format_number(usage_percent),
            );
        }
        if let Some(window_minutes) = self.window_minutes {
            env.insert(
                "CODEXBAR_WINDOW_MINUTES".to_string(),
                window_minutes.to_string(),
            );
        }
        if let Some(used) = self.used {
            env.insert("CODEXBAR_USED".to_string(), format_number(used));
        }
        if let Some(limit) = self.limit {
            env.insert("CODEXBAR_LIMIT".to_string(), format_number(limit));
        }
        if let Some(reset_at) = self.reset_at {
            env.insert("CODEXBAR_RESET_AT".to_string(), reset_at.to_string());
        }
        if let Some(secondary_usage_percent) = self.secondary_usage_percent {
            env.insert(
                "CODEXBAR_SECONDARY_USAGE_PERCENT".to_string(),
                format_number(secondary_usage_percent),
            );
        }
        if let Some(secondary_window_minutes) = self.secondary_window_minutes {
            env.insert(
                "CODEXBAR_SECONDARY_WINDOW_MINUTES".to_string(),
                secondary_window_minutes.to_string(),
            );
        }
        if let Some(secondary_reset_at) = self.secondary_reset_at {
            env.insert(
                "CODEXBAR_SECONDARY_RESET_AT".to_string(),
                secondary_reset_at.to_string(),
            );
        }
        if let Some(status) = &self.status {
            env.insert("CODEXBAR_STATUS".to_string(), status.clone());
        }
        env
    }
}

fn format_number(value: f64) -> String {
    if value == value.round() && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

/// A user-configured hook rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookRule {
    #[serde(default = "default_rule_id")]
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub event: HookEventType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    pub executable: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: f64,
}

fn default_rule_id() -> String {
    "hook-rule".to_string()
}

const fn default_true() -> bool {
    true
}

const fn default_timeout_seconds() -> f64 {
    HookRule::DEFAULT_TIMEOUT_SECONDS
}

impl HookRule {
    pub const MINIMUM_TIMEOUT_SECONDS: f64 = 0.1;
    pub const MAXIMUM_TIMEOUT_SECONDS: f64 = 300.0;
    pub const DEFAULT_TIMEOUT_SECONDS: f64 = 10.0;
    pub const MAXIMUM_ID_BYTES: usize = 128;
    pub const MAXIMUM_ARGUMENT_COUNT: usize = 32;
    pub const MAXIMUM_STRING_BYTES: usize = 4096;
    pub const MAXIMUM_COMMAND_BYTES: usize = 32 * 1024;

    pub fn new(event: HookEventType, executable: impl Into<String>) -> Self {
        Self {
            id: default_rule_id(),
            enabled: true,
            event,
            provider: None,
            threshold: None,
            executable: executable.into(),
            arguments: Vec::new(),
            timeout_seconds: Self::DEFAULT_TIMEOUT_SECONDS,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = Some(provider.into());
        self
    }

    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.threshold = Some(threshold);
        self
    }

    pub fn matches(&self, event: &HookEvent) -> bool {
        if !self.enabled || self.event != event.event {
            return false;
        }
        if !self.has_valid_executable_path()
            || !self.has_valid_timeout()
            || !self.has_valid_command_shape()
            || !self.has_known_provider()
            || !self.has_valid_threshold()
        {
            return false;
        }
        if let Some(provider) = &self.provider
            && provider != &event.provider
        {
            return false;
        }
        if self.event == HookEventType::QuotaLow
            && let Some(threshold) = self.threshold
        {
            let Some(usage) = event.usage_percent else {
                return false;
            };
            if usage < threshold {
                return false;
            }
        }
        true
    }

    pub fn has_valid_executable_path(&self) -> bool {
        !self.executable.is_empty()
            && self.executable.len() <= Self::MAXIMUM_STRING_BYTES
            && self.executable.starts_with('/')
    }

    pub fn has_valid_timeout(&self) -> bool {
        self.timeout_seconds.is_finite()
            && (Self::MINIMUM_TIMEOUT_SECONDS..=Self::MAXIMUM_TIMEOUT_SECONDS)
                .contains(&self.timeout_seconds)
    }

    pub fn has_known_provider(&self) -> bool {
        match &self.provider {
            None => true,
            Some(provider) => providers::find_by_id_str(provider).is_some(),
        }
    }

    pub fn has_valid_threshold(&self) -> bool {
        match self.threshold {
            None => true,
            Some(threshold) => threshold.is_finite() && threshold > 0.0 && threshold <= 1.0,
        }
    }

    pub fn has_valid_command_shape(&self) -> bool {
        if self.id.is_empty() || self.id.len() > Self::MAXIMUM_ID_BYTES {
            return false;
        }
        if self.arguments.len() > Self::MAXIMUM_ARGUMENT_COUNT {
            return false;
        }
        if !self
            .arguments
            .iter()
            .all(|arg| arg.len() <= Self::MAXIMUM_STRING_BYTES)
        {
            return false;
        }
        let total_bytes: usize =
            self.executable.len() + self.arguments.iter().map(String::len).sum::<usize>();
        total_bytes <= Self::MAXIMUM_COMMAND_BYTES
    }
}

/// Top-level `hooks` section of the configuration.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub events: Vec<HookRule>,
}

impl HooksConfig {
    pub const MAXIMUM_RULE_COUNT: usize = 32;

    pub fn new(enabled: bool, events: Vec<HookRule>) -> Self {
        Self { enabled, events }
    }

    pub fn matching_rules(&self, event: &HookEvent) -> Vec<HookRule> {
        if !self.enabled || self.events.len() > Self::MAXIMUM_RULE_COUNT {
            return Vec::new();
        }
        self.events
            .iter()
            .filter(|rule| rule.matches(event))
            .cloned()
            .collect()
    }
}

/// Returns the `quota_low` rules whose watched threshold was crossed upward between
/// `previous_usage` and `current_usage` (both `0.0..=1.0` usage fractions).
pub fn crossed_quota_low_rules(
    rules: &[HookRule],
    previous_usage: f64,
    current_usage: f64,
    fallback_thresholds: &[f64],
) -> Vec<HookRule> {
    rules
        .iter()
        .filter(|rule| {
            let single;
            let watched: &[f64] = match rule.threshold {
                Some(ref t) => {
                    single = [*t];
                    &single
                }
                None => fallback_thresholds,
            };
            watched
                .iter()
                .any(|&threshold| previous_usage < threshold && current_usage >= threshold)
        })
        .cloned()
        .collect()
}

/// Identifies one tracked quota lane (`provider` + `window` + optional account / window ID).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HookQuotaLaneKey {
    pub provider: String,
    pub window: QuotaWarningWindow,
    pub account_discriminator: Option<String>,
    pub window_id: Option<String>,
}

impl HookQuotaLaneKey {
    pub fn new(
        provider: impl Into<String>,
        window: QuotaWarningWindow,
        account_discriminator: Option<String>,
        window_id: Option<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            window,
            account_discriminator,
            window_id,
        }
    }
}

/// Observation for a single quota lane during a poll.
#[derive(Debug, Clone, PartialEq)]
pub struct HookQuotaLaneObservation {
    pub key: HookQuotaLaneKey,
    pub label: String,
    pub rate_window: Option<RateWindow>,
    pub fallback_thresholds: Vec<f64>,
}

impl HookQuotaLaneObservation {
    pub fn new(
        key: HookQuotaLaneKey,
        label: impl Into<String>,
        rate_window: Option<RateWindow>,
        fallback_thresholds: Vec<f64>,
    ) -> Self {
        Self {
            key,
            label: label.into(),
            rate_window,
            fallback_thresholds,
        }
    }
}

/// Normalized provider status indicator for hook transition detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HookProviderStatus {
    None,
    Minor,
    Major,
    Critical,
    Maintenance,
    #[default]
    Unknown,
}

impl From<ProviderStatusIndicator> for HookProviderStatus {
    fn from(indicator: ProviderStatusIndicator) -> Self {
        match indicator {
            ProviderStatusIndicator::None => Self::None,
            ProviderStatusIndicator::Minor => Self::Minor,
            ProviderStatusIndicator::Major => Self::Major,
            ProviderStatusIndicator::Critical => Self::Critical,
            ProviderStatusIndicator::Maintenance => Self::Maintenance,
            ProviderStatusIndicator::Unknown => Self::Unknown,
        }
    }
}

/// Observation of one provider at the end of a refresh pass.
#[derive(Debug, Clone, PartialEq)]
pub struct HookProviderObservation {
    pub provider: String,
    pub account: Option<String>,
    pub lanes: Vec<HookQuotaLaneObservation>,
    pub secondary: Option<RateWindow>,
    pub status: HookProviderStatus,
    pub refresh_failure_status: Option<String>,
}

impl HookProviderObservation {
    pub fn new(
        provider: impl Into<String>,
        account: Option<String>,
        lanes: Vec<HookQuotaLaneObservation>,
        secondary: Option<RateWindow>,
        status: HookProviderStatus,
        refresh_failure_status: Option<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            account,
            lanes,
            secondary,
            status,
            refresh_failure_status,
        }
    }
}

/// A transition event paired with the specific rules that should execute (when filtered).
#[derive(Debug, Clone, PartialEq)]
pub struct HookDispatch {
    pub event: HookEvent,
    pub rules: Option<Vec<HookRule>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct WindowObservation {
    usage: f64,
    reset_at: Option<Timestamp>,
}

/// Pure edge-triggered transition detector for hooks.
///
/// The first observation for a lane or provider status only establishes a baseline;
/// subsequent observations emit transitions only when an edge is crossed.
#[derive(Debug, Clone, Default)]
pub struct HookTransitionDetector {
    window_observation: HashMap<HookQuotaLaneKey, WindowObservation>,
    provider_status_had_issue: HashMap<String, bool>,
    last_config_revision: Option<u64>,
}

impl HookTransitionDetector {
    /// Usage drop (as a `0.0..=1.0` fraction) that counts as a quota reset when the provider
    /// does not report a `resets_at` timestamp.
    pub const RESET_DROP_THRESHOLD: f64 = 0.5;
    /// Tolerance (in seconds) when comparing `resets_at` timestamps across polls.
    pub const RESET_TIMESTAMP_TOLERANCE_SECS: f64 = 1.0;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset_if_configuration_changed(&mut self, revision: u64) {
        if self.last_config_revision != Some(revision) {
            self.last_config_revision = Some(revision);
            self.window_observation.clear();
            self.provider_status_had_issue.clear();
        }
    }

    pub fn reset_provider(&mut self, provider: &str) {
        self.window_observation
            .retain(|key, _| key.provider != provider);
        self.provider_status_had_issue.remove(provider);
    }

    pub fn evaluate(
        &mut self,
        observation: &HookProviderObservation,
        config: &HooksConfig,
        now: Timestamp,
    ) -> Vec<HookDispatch> {
        if !config.enabled {
            return Vec::new();
        }

        let mut dispatches = Vec::new();

        if let Some(failure_status) = &observation.refresh_failure_status {
            dispatches.push(HookDispatch {
                event: HookEvent {
                    event: HookEventType::RefreshFailed,
                    provider: observation.provider.clone(),
                    account: observation.account.clone(),
                    window: None,
                    usage_percent: None,
                    window_minutes: None,
                    used: None,
                    limit: None,
                    reset_at: None,
                    secondary_usage_percent: None,
                    secondary_window_minutes: None,
                    secondary_reset_at: None,
                    status: Some(failure_status.clone()),
                    timestamp: now,
                },
                rules: None,
            });
            dispatches.extend(self.evaluate_status(observation, now));
            return dispatches;
        }

        let observed_keys: HashSet<HookQuotaLaneKey> = observation
            .lanes
            .iter()
            .map(|lane| lane.key.clone())
            .collect();
        self.window_observation
            .retain(|key, _| key.provider != observation.provider || observed_keys.contains(key));

        for lane in &observation.lanes {
            let Some(rate_window) = &lane.rate_window else {
                self.window_observation.remove(&lane.key);
                continue;
            };
            if rate_window.is_synthetic_placeholder || !rate_window.used_percent.is_finite() {
                self.window_observation.remove(&lane.key);
                continue;
            }

            let current_usage = (rate_window.used_percent / 100.0).clamp(0.0, 1.0);
            let current = WindowObservation {
                usage: current_usage,
                reset_at: rate_window.resets_at,
            };

            let Some(previous) = self.window_observation.insert(lane.key.clone(), current) else {
                // First observation establishes the baseline without firing.
                continue;
            };

            let did_reset = Self::window_did_reset(previous, current);
            if did_reset {
                dispatches.push(HookDispatch {
                    event: self.quota_event(
                        HookEventType::QuotaReset,
                        observation,
                        lane,
                        rate_window,
                        current_usage,
                        now,
                    ),
                    rules: None,
                });
            }

            if lane.key.window == QuotaWarningWindow::Session
                && !did_reset
                && previous.usage < 1.0
                && current_usage >= 1.0
            {
                dispatches.push(HookDispatch {
                    event: self.quota_event(
                        HookEventType::QuotaReached,
                        observation,
                        lane,
                        rate_window,
                        current_usage,
                        now,
                    ),
                    rules: None,
                });
            }

            let candidate = self.quota_event(
                HookEventType::QuotaLow,
                observation,
                lane,
                rate_window,
                current_usage,
                now,
            );
            let matching_rules = config.matching_rules(&candidate);
            let crossed_rules = crossed_quota_low_rules(
                &matching_rules,
                previous.usage,
                current_usage,
                &lane.fallback_thresholds,
            );
            if !crossed_rules.is_empty() {
                dispatches.push(HookDispatch {
                    event: candidate,
                    rules: Some(crossed_rules),
                });
            }
        }

        dispatches.extend(self.evaluate_status(observation, now));
        dispatches
    }

    fn evaluate_status(
        &mut self,
        observation: &HookProviderObservation,
        now: Timestamp,
    ) -> Vec<HookDispatch> {
        let has_issue = match observation.status {
            HookProviderStatus::None => Some(false),
            HookProviderStatus::Minor
            | HookProviderStatus::Major
            | HookProviderStatus::Critical => Some(true),
            HookProviderStatus::Maintenance | HookProviderStatus::Unknown => None,
        };
        let Some(has_issue) = has_issue else {
            return Vec::new();
        };

        let previous = self
            .provider_status_had_issue
            .insert(observation.provider.clone(), has_issue);
        let Some(previous) = previous else {
            // First definite status sample establishes the baseline without firing.
            return Vec::new();
        };
        if previous == has_issue {
            return Vec::new();
        }

        let (event_type, status_label) = if has_issue {
            let label = match observation.status {
                HookProviderStatus::Minor => "minor",
                HookProviderStatus::Major => "major",
                HookProviderStatus::Critical => "critical",
                _ => "unavailable",
            };
            (HookEventType::ProviderUnavailable, label)
        } else {
            (HookEventType::ProviderRecovered, "operational")
        };

        vec![HookDispatch {
            event: HookEvent {
                event: event_type,
                provider: observation.provider.clone(),
                account: observation.account.clone(),
                window: None,
                usage_percent: None,
                window_minutes: None,
                used: None,
                limit: None,
                reset_at: None,
                secondary_usage_percent: None,
                secondary_window_minutes: None,
                secondary_reset_at: None,
                status: Some(status_label.to_string()),
                timestamp: now,
            },
            rules: None,
        }]
    }

    fn window_did_reset(previous: WindowObservation, current: WindowObservation) -> bool {
        if let (Some(prev_reset), Some(curr_reset)) = (previous.reset_at, current.reset_at) {
            return curr_reset.duration_since(prev_reset).as_secs_f64()
                > Self::RESET_TIMESTAMP_TOLERANCE_SECS;
        }
        (previous.usage - current.usage) >= Self::RESET_DROP_THRESHOLD
    }

    fn quota_event(
        &self,
        event: HookEventType,
        observation: &HookProviderObservation,
        lane: &HookQuotaLaneObservation,
        rate_window: &RateWindow,
        usage_percent: f64,
        now: Timestamp,
    ) -> HookEvent {
        let secondary = observation
            .secondary
            .as_ref()
            .filter(|w| !w.is_synthetic_placeholder);
        HookEvent {
            event,
            provider: observation.provider.clone(),
            account: observation.account.clone(),
            window: Some(lane.label.clone()),
            usage_percent: Some(usage_percent),
            window_minutes: rate_window.window_minutes,
            used: None,
            limit: None,
            reset_at: rate_window.resets_at,
            secondary_usage_percent: secondary.map(|w| w.used_percent / 100.0),
            secondary_window_minutes: secondary.and_then(|w| w.window_minutes),
            secondary_reset_at: secondary.and_then(|w| w.resets_at),
            status: None,
            timestamp: now,
        }
    }
}

/// Session quota depletion/restoration edge transitions (`SessionQuotaNotifications.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionQuotaTransition {
    None,
    Depleted,
    Restored,
}

/// Pure logic for session quota depletion/restoration transitions.
#[derive(Debug, Clone, Default)]
pub struct SessionQuotaTransitionDetector {
    last_known_remaining: HashMap<String, f64>,
}

impl SessionQuotaTransitionDetector {
    pub const DEPLETED_THRESHOLD: f64 = 0.0001;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_depleted(remaining: f64) -> bool {
        remaining <= Self::DEPLETED_THRESHOLD
    }

    pub fn transition(
        previous_remaining: Option<f64>,
        current_remaining: Option<f64>,
    ) -> SessionQuotaTransition {
        let Some(current) = current_remaining else {
            return SessionQuotaTransition::None;
        };
        let Some(previous) = previous_remaining else {
            return SessionQuotaTransition::None;
        };
        let was_depleted = Self::is_depleted(previous);
        let is_depleted = Self::is_depleted(current);
        if !was_depleted && is_depleted {
            SessionQuotaTransition::Depleted
        } else if was_depleted && !is_depleted {
            SessionQuotaTransition::Restored
        } else {
            SessionQuotaTransition::None
        }
    }

    /// Evaluates a provider's current remaining session percentage (`0.0..=100.0`).
    /// The first sample records the baseline and returns `SessionQuotaTransition::None`.
    pub fn evaluate(
        &mut self,
        provider_key: &str,
        current_remaining: Option<f64>,
    ) -> SessionQuotaTransition {
        let Some(current) = current_remaining else {
            self.last_known_remaining.remove(provider_key);
            return SessionQuotaTransition::None;
        };
        let previous = self
            .last_known_remaining
            .insert(provider_key.to_string(), current);
        Self::transition(previous, Some(current))
    }

    pub fn reset(&mut self, provider_key: &str) {
        self.last_known_remaining.remove(provider_key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROVIDER: &str = "codex";

    fn sample_now() -> Timestamp {
        Timestamp::from_second(1_778_371_200).expect("valid timestamp")
    }

    fn lane_key(
        window: QuotaWarningWindow,
        account: Option<&str>,
        window_id: Option<&str>,
    ) -> HookQuotaLaneKey {
        HookQuotaLaneKey::new(
            PROVIDER,
            window,
            account.map(ToOwned::to_owned),
            window_id.map(ToOwned::to_owned),
        )
    }

    fn rate_window(
        used_percent: f64,
        resets_at: Option<Timestamp>,
        is_synthetic_placeholder: bool,
    ) -> RateWindow {
        RateWindow::new(
            used_percent,
            Some(300),
            resets_at,
            None,
            None,
            is_synthetic_placeholder,
        )
        .expect("valid rate window")
    }

    fn lane(
        used_percent: Option<f64>,
        key: HookQuotaLaneKey,
        resets_at: Option<Timestamp>,
        thresholds: Vec<f64>,
        is_synthetic_placeholder: bool,
    ) -> HookQuotaLaneObservation {
        let label = match key.window {
            QuotaWarningWindow::Session => "Session",
            QuotaWarningWindow::Weekly => "Weekly",
        };
        HookQuotaLaneObservation::new(
            key,
            label,
            used_percent.map(|pct| rate_window(pct, resets_at, is_synthetic_placeholder)),
            thresholds,
        )
    }

    fn default_lane(used_percent: Option<f64>) -> HookQuotaLaneObservation {
        lane(
            used_percent,
            lane_key(QuotaWarningWindow::Session, None, None),
            None,
            vec![0.8],
            false,
        )
    }

    fn observation(
        lanes: Vec<HookQuotaLaneObservation>,
        status: HookProviderStatus,
        refresh_failure_status: Option<&str>,
    ) -> HookProviderObservation {
        HookProviderObservation::new(
            PROVIDER,
            None,
            lanes,
            None,
            status,
            refresh_failure_status.map(ToOwned::to_owned),
        )
    }

    fn default_config(enabled: bool, rules: Option<Vec<HookRule>>) -> HooksConfig {
        HooksConfig::new(
            enabled,
            rules.unwrap_or_else(|| {
                vec![
                    HookRule::new(HookEventType::QuotaLow, "/bin/true"),
                    HookRule::new(HookEventType::QuotaReached, "/bin/true"),
                    HookRule::new(HookEventType::QuotaReset, "/bin/true"),
                    HookRule::new(HookEventType::ProviderUnavailable, "/bin/true"),
                    HookRule::new(HookEventType::ProviderRecovered, "/bin/true"),
                    HookRule::new(HookEventType::RefreshFailed, "/bin/true"),
                ]
            }),
        )
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:79 first sample establishes baseline without firing
    #[test]
    fn first_sample_establishes_baseline_without_firing() {
        let mut detector = HookTransitionDetector::new();
        let dispatches = detector.evaluate(
            &observation(
                vec![default_lane(Some(95.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &default_config(true, None),
            sample_now(),
        );
        assert!(dispatches.is_empty());
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:88 quota low fires once on upward crossing
    #[test]
    fn quota_low_fires_once_on_upward_crossing() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();

        let _ = detector.evaluate(
            &observation(
                vec![default_lane(Some(50.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );

        let crossing = detector.evaluate(
            &observation(
                vec![default_lane(Some(85.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert_eq!(
            crossing.iter().map(|d| d.event.event).collect::<Vec<_>>(),
            vec![HookEventType::QuotaLow]
        );

        let persisting = detector.evaluate(
            &observation(
                vec![default_lane(Some(90.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert!(persisting.is_empty());
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:106 quota low dispatches only the rule whose threshold crossed
    #[test]
    fn quota_low_dispatches_only_the_rule_whose_threshold_crossed() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(
            true,
            Some(vec![
                HookRule::new(HookEventType::QuotaLow, "/bin/true")
                    .with_id("low")
                    .with_threshold(0.5),
                HookRule::new(HookEventType::QuotaLow, "/bin/true")
                    .with_id("high")
                    .with_threshold(0.8),
            ]),
        );
        let now = sample_now();

        let _ = detector.evaluate(
            &observation(
                vec![default_lane(Some(60.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );

        let dispatches = detector.evaluate(
            &observation(
                vec![default_lane(Some(85.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert_eq!(dispatches.len(), 1);
        let rule_ids: Vec<&str> = dispatches[0]
            .rules
            .as_ref()
            .expect("matched rules")
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        assert_eq!(rule_ids, vec!["high"]);
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:126 quota reached fires on upward edge only and not while saturated
    #[test]
    fn quota_reached_fires_on_upward_edge_only_and_not_while_saturated() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();

        let _ = detector.evaluate(
            &observation(
                vec![default_lane(Some(90.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );

        let reached = detector.evaluate(
            &observation(
                vec![default_lane(Some(100.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert!(
            reached
                .iter()
                .any(|d| d.event.event == HookEventType::QuotaReached)
        );

        let still_full = detector.evaluate(
            &observation(
                vec![default_lane(Some(100.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert!(
            !still_full
                .iter()
                .any(|d| d.event.event == HookEventType::QuotaReached)
        );
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:143 quota reached never fires for the weekly lane
    #[test]
    fn quota_reached_never_fires_for_weekly_lane() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();
        let weekly = lane_key(QuotaWarningWindow::Weekly, None, None);

        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(90.0), weekly.clone(), None, vec![0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );

        let dispatches = detector.evaluate(
            &observation(
                vec![lane(Some(100.0), weekly, None, vec![0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert!(
            !dispatches
                .iter()
                .any(|d| d.event.event == HookEventType::QuotaReached)
        );
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:162 quota reset fires when reset boundary advances
    #[test]
    fn quota_reset_fires_when_reset_boundary_advances() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();
        let first = Timestamp::from_second(1_000_000).expect("valid timestamp");
        let second = Timestamp::from_second(1_018_000).expect("valid timestamp");
        let key = lane_key(QuotaWarningWindow::Session, None, None);

        let _ = detector.evaluate(
            &observation(
                vec![lane(
                    Some(100.0),
                    key.clone(),
                    Some(first),
                    vec![0.8],
                    false,
                )],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![lane(Some(0.0), key, Some(second), vec![0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert_eq!(
            dispatches.iter().map(|d| d.event.event).collect::<Vec<_>>(),
            vec![HookEventType::QuotaReset]
        );
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:179 quota reset fires on usage drop without reset boundary
    #[test]
    fn quota_reset_fires_on_usage_drop_without_reset_boundary() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();

        let _ = detector.evaluate(
            &observation(
                vec![default_lane(Some(95.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![default_lane(Some(10.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert_eq!(
            dispatches.iter().map(|d| d.event.event).collect::<Vec<_>>(),
            vec![HookEventType::QuotaReset]
        );
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:191 reset suppresses depletion edge in same poll
    #[test]
    fn reset_suppresses_depletion_edge_in_same_poll() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();

        let _ = detector.evaluate(
            &observation(
                vec![default_lane(Some(95.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![default_lane(Some(5.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert!(
            !dispatches
                .iter()
                .any(|d| d.event.event == HookEventType::QuotaReached)
        );
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:205 provider status fires outage and recovery edges
    #[test]
    fn provider_status_fires_outage_and_recovery_edges() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();

        let _ = detector.evaluate(
            &observation(vec![], HookProviderStatus::None, None),
            &cfg,
            now,
        );

        let outage = detector.evaluate(
            &observation(vec![], HookProviderStatus::Major, None),
            &cfg,
            now,
        );
        assert_eq!(
            outage.iter().map(|d| d.event.event).collect::<Vec<_>>(),
            vec![HookEventType::ProviderUnavailable]
        );

        let persisting = detector.evaluate(
            &observation(vec![], HookProviderStatus::Critical, None),
            &cfg,
            now,
        );
        assert!(persisting.is_empty());

        let recovered = detector.evaluate(
            &observation(vec![], HookProviderStatus::None, None),
            &cfg,
            now,
        );
        assert_eq!(
            recovered.iter().map(|d| d.event.event).collect::<Vec<_>>(),
            vec![HookEventType::ProviderRecovered]
        );
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:223 unknown and maintenance never flip status state
    #[test]
    fn unknown_and_maintenance_never_flip_status_state() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();

        let _ = detector.evaluate(
            &observation(vec![], HookProviderStatus::None, None),
            &cfg,
            now,
        );

        assert!(
            detector
                .evaluate(
                    &observation(vec![], HookProviderStatus::Unknown, None),
                    &cfg,
                    now
                )
                .is_empty()
        );
        assert!(
            detector
                .evaluate(
                    &observation(vec![], HookProviderStatus::Maintenance, None),
                    &cfg,
                    now
                )
                .is_empty()
        );

        let outage = detector.evaluate(
            &observation(vec![], HookProviderStatus::Major, None),
            &cfg,
            now,
        );
        assert_eq!(
            outage.iter().map(|d| d.event.event).collect::<Vec<_>>(),
            vec![HookEventType::ProviderUnavailable]
        );
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:241 first definite status does not fire
    #[test]
    fn first_definite_status_does_not_fire() {
        let mut detector = HookTransitionDetector::new();
        let dispatches = detector.evaluate(
            &observation(vec![], HookProviderStatus::Critical, None),
            &default_config(true, None),
            sample_now(),
        );
        assert!(dispatches.is_empty());
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:252 refresh failure emits coarse status only
    #[test]
    fn refresh_failure_emits_coarse_status_only() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();

        let failed = detector.evaluate(
            &observation(vec![], HookProviderStatus::Unknown, Some("timeout")),
            &cfg,
            now,
        );
        assert_eq!(
            failed.iter().map(|d| d.event.event).collect::<Vec<_>>(),
            vec![HookEventType::RefreshFailed]
        );
        assert_eq!(failed[0].event.status.as_deref(), Some("timeout"));
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:262 refresh failure does not disturb quota baselines
    #[test]
    fn refresh_failure_does_not_disturb_quota_baselines() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();

        let _ = detector.evaluate(
            &observation(
                vec![default_lane(Some(50.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );

        let _ = detector.evaluate(
            &observation(vec![], HookProviderStatus::Unknown, Some("offline")),
            &cfg,
            now,
        );

        let crossing = detector.evaluate(
            &observation(
                vec![default_lane(Some(85.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert!(
            crossing
                .iter()
                .any(|d| d.event.event == HookEventType::QuotaLow)
        );
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:280 disabled hooks produce no events
    #[test]
    fn disabled_hooks_produce_no_events() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(false, None);
        let now = sample_now();

        let _ = detector.evaluate(
            &observation(
                vec![default_lane(Some(50.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![default_lane(Some(95.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert!(dispatches.is_empty());
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:291 configuration change clears baselines
    #[test]
    fn configuration_change_clears_baselines() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();

        detector.reset_if_configuration_changed(1);
        let _ = detector.evaluate(
            &observation(
                vec![default_lane(Some(50.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );

        detector.reset_if_configuration_changed(2);
        let after_config_change = detector.evaluate(
            &observation(
                vec![default_lane(Some(95.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert!(after_config_change.is_empty());
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:306 synthetic placeholder lane never fires
    #[test]
    fn synthetic_placeholder_lane_never_fires() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();

        let _ = detector.evaluate(
            &observation(
                vec![default_lane(Some(50.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );

        let key = lane_key(QuotaWarningWindow::Session, None, None);
        let dispatches = detector.evaluate(
            &observation(
                vec![lane(Some(100.0), key, None, vec![0.8], true)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert!(dispatches.is_empty());
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:320 disappearing lane resets baseline
    #[test]
    fn disappearing_lane_resets_baseline() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();

        let _ = detector.evaluate(
            &observation(
                vec![default_lane(Some(50.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );

        // Disappearing lane resets baseline.
        let _ = detector.evaluate(
            &observation(vec![], HookProviderStatus::Unknown, None),
            &cfg,
            now,
        );
        let after_reappear = detector.evaluate(
            &observation(
                vec![default_lane(Some(95.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert!(after_reappear.is_empty());
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:335 accounts on same provider track independently
    #[test]
    fn accounts_on_same_provider_track_independently() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();

        let first = lane_key(QuotaWarningWindow::Session, Some("a@example.com"), None);
        let second = lane_key(QuotaWarningWindow::Session, Some("b@example.com"), None);

        let _ = detector.evaluate(
            &observation(
                vec![
                    lane(Some(50.0), first.clone(), None, vec![0.8], false),
                    lane(Some(50.0), second.clone(), None, vec![0.8], false),
                ],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );

        let dispatches = detector.evaluate(
            &observation(
                vec![
                    lane(Some(85.0), first, None, vec![0.8], false),
                    lane(Some(55.0), second, None, vec![0.8], false),
                ],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].event.event, HookEventType::QuotaLow);
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:361 quota low respects explicit rule threshold over fallback
    #[test]
    fn quota_low_respects_explicit_rule_threshold_over_fallback() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(
            true,
            Some(vec![
                HookRule::new(HookEventType::QuotaLow, "/bin/true").with_threshold(0.9),
            ]),
        );
        let now = sample_now();
        let key = lane_key(QuotaWarningWindow::Session, None, None);

        let _ = detector.evaluate(
            &observation(
                vec![lane(Some(50.0), key.clone(), None, vec![0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );

        let below = detector.evaluate(
            &observation(
                vec![lane(Some(85.0), key.clone(), None, vec![0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert!(below.is_empty());

        let above = detector.evaluate(
            &observation(
                vec![lane(Some(95.0), key, None, vec![0.8], false)],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert_eq!(
            above.iter().map(|d| d.event.event).collect::<Vec<_>>(),
            vec![HookEventType::QuotaLow]
        );
    }

    // upstream: TestsLinux/HookTransitionDetectorLinuxTests.swift:383 quota low ignores rules scoped to another provider
    #[test]
    fn quota_low_ignores_rules_scoped_to_another_provider() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(
            true,
            Some(vec![
                HookRule::new(HookEventType::QuotaLow, "/bin/true").with_provider("claude"),
            ]),
        );
        let now = sample_now();

        let _ = detector.evaluate(
            &observation(
                vec![default_lane(Some(50.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        let dispatches = detector.evaluate(
            &observation(
                vec![default_lane(Some(95.0))],
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert!(dispatches.is_empty());
    }

    #[test]
    fn session_quota_transition_requires_baseline_before_firing() {
        let mut detector = SessionQuotaTransitionDetector::new();
        assert_eq!(
            detector.evaluate("claude", Some(0.0)),
            SessionQuotaTransition::None
        );
        assert_eq!(
            detector.evaluate("claude", Some(50.0)),
            SessionQuotaTransition::Restored
        );
        assert_eq!(
            detector.evaluate("claude", Some(0.0)),
            SessionQuotaTransition::Depleted
        );
    }
}
