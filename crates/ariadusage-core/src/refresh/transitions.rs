// Ported from CodexBar Sources/CodexBarCore/Hooks/HookTransitionDetector.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Hooks/HookEvent.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Hooks/HookRule.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Hooks/HookRateLimiter.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Hooks/HookEvent+UsageUpdated.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIHooksCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIHooksWatchCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/SessionQuotaNotifications.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+SessionQuotaTransition.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+QuotaWarnings.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+Hooks.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesHooksPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

use ariadusage_protocol::{ProviderId, StatusIndicator as ProviderStatusIndicator};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::config::{Config, QuotaWarningWindow, QuotaWarnings};
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
    pub const ALL: [Self; 7] = [
        Self::QuotaLow,
        Self::QuotaReached,
        Self::QuotaReset,
        Self::UsageUpdated,
        Self::ProviderUnavailable,
        Self::ProviderRecovered,
        Self::RefreshFailed,
    ];

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

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "quota_low" => Some(Self::QuotaLow),
            "quota_reached" => Some(Self::QuotaReached),
            "quota_reset" => Some(Self::QuotaReset),
            "usage_updated" => Some(Self::UsageUpdated),
            "provider_unavailable" => Some(Self::ProviderUnavailable),
            "provider_recovered" => Some(Self::ProviderRecovered),
            "refresh_failed" => Some(Self::RefreshFailed),
            _ => None,
        }
    }

    pub fn is_rate_limited(&self) -> bool {
        matches!(
            self,
            Self::UsageUpdated | Self::ProviderUnavailable | Self::RefreshFailed
        )
    }
}

/// Errors encountered when encoding a [`HookEvent`] JSON stdin payload.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HookPayloadError {
    #[error("hook payload exceeds the {limit}-byte limit ({size} bytes)")]
    PayloadTooLarge { size: usize, limit: usize },
    #[error("failed to encode hook payload: {0}")]
    EncodingFailed(String),
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
    /// Maximum JSON payload size written to a hook's stdin (`HookRunner.maxPayloadBytes`).
    pub const MAX_PAYLOAD_BYTES: usize = 4096;

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

    /// Encodes the event as key-sorted JSON bytes for the hook command's stdin.
    pub fn json_payload(&self) -> Result<Vec<u8>, HookPayloadError> {
        let val = serde_json::to_value(self)
            .map_err(|e| HookPayloadError::EncodingFailed(e.to_string()))?;
        let bytes = serde_json::to_vec(&val)
            .map_err(|e| HookPayloadError::EncodingFailed(e.to_string()))?;
        if bytes.len() > Self::MAX_PAYLOAD_BYTES {
            return Err(HookPayloadError::PayloadTooLarge {
                size: bytes.len(),
                limit: Self::MAX_PAYLOAD_BYTES,
            });
        }
        Ok(bytes)
    }

    pub fn environment_variables(&self) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        env.insert(
            "ARIADUSAGE_EVENT".to_string(),
            self.event.as_str().to_string(),
        );
        env.insert("ARIADUSAGE_PROVIDER".to_string(), self.provider.clone());
        env.insert(
            "ARIADUSAGE_TIMESTAMP".to_string(),
            self.timestamp.to_string(),
        );
        if let Some(account) = &self.account {
            env.insert("ARIADUSAGE_ACCOUNT".to_string(), account.clone());
        }
        if let Some(window) = &self.window {
            env.insert("ARIADUSAGE_WINDOW".to_string(), window.clone());
        }
        if let Some(usage_percent) = self.usage_percent {
            env.insert(
                "ARIADUSAGE_USAGE_PERCENT".to_string(),
                format_number(usage_percent),
            );
        }
        if let Some(window_minutes) = self.window_minutes {
            env.insert(
                "ARIADUSAGE_WINDOW_MINUTES".to_string(),
                window_minutes.to_string(),
            );
        }
        if let Some(used) = self.used {
            env.insert("ARIADUSAGE_USED".to_string(), format_number(used));
        }
        if let Some(limit) = self.limit {
            env.insert("ARIADUSAGE_LIMIT".to_string(), format_number(limit));
        }
        if let Some(reset_at) = self.reset_at {
            env.insert("ARIADUSAGE_RESET_AT".to_string(), reset_at.to_string());
        }
        if let Some(secondary_usage_percent) = self.secondary_usage_percent {
            env.insert(
                "ARIADUSAGE_SECONDARY_USAGE_PERCENT".to_string(),
                format_number(secondary_usage_percent),
            );
        }
        if let Some(secondary_window_minutes) = self.secondary_window_minutes {
            env.insert(
                "ARIADUSAGE_SECONDARY_WINDOW_MINUTES".to_string(),
                secondary_window_minutes.to_string(),
            );
        }
        if let Some(secondary_reset_at) = self.secondary_reset_at {
            env.insert(
                "ARIADUSAGE_SECONDARY_RESET_AT".to_string(),
                secondary_reset_at.to_string(),
            );
        }
        if let Some(status) = &self.status {
            env.insert("ARIADUSAGE_STATUS".to_string(), status.clone());
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
    #[serde(alias = "command")]
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

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
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

    pub fn with_arguments(mut self, arguments: Vec<String>) -> Self {
        self.arguments = arguments;
        self
    }

    pub fn with_timeout_seconds(mut self, timeout_seconds: f64) -> Self {
        self.timeout_seconds = timeout_seconds;
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
    #[serde(default, alias = "rules")]
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

/// In-memory storm suppression for repeated hook events (`HookRateLimiter.swift`).
///
/// Fires a given `(event, provider, account, window)` tuple at most once per `window_seconds`.
#[derive(Debug, Clone)]
pub struct HookRateLimiter {
    pub window_seconds: f64,
    last_fired: HashMap<String, Timestamp>,
}

impl Default for HookRateLimiter {
    fn default() -> Self {
        Self::new(Self::DEFAULT_WINDOW_SECONDS)
    }
}

impl HookRateLimiter {
    pub const DEFAULT_WINDOW_SECONDS: f64 = 600.0;

    pub fn new(window_seconds: f64) -> Self {
        Self {
            window_seconds,
            last_fired: HashMap::new(),
        }
    }

    pub fn allow(
        &mut self,
        event: &HookEvent,
        account_discriminator: Option<&str>,
        now: Timestamp,
    ) -> bool {
        if self.window_seconds <= 0.0 {
            return true;
        }
        let key = Self::key_for(event, account_discriminator);
        if let Some(previous) = self.last_fired.get(&key)
            && now.duration_since(*previous).as_secs_f64() < self.window_seconds
        {
            return false;
        }
        self.last_fired.insert(key, now);
        true
    }

    pub fn reset(&mut self) {
        self.last_fired.clear();
    }

    pub fn key_for(event: &HookEvent, account_discriminator: Option<&str>) -> String {
        [
            event.event.as_str(),
            event.provider.as_str(),
            account_discriminator
                .or(event.account.as_deref())
                .unwrap_or(""),
            event.window.as_deref().unwrap_or(""),
        ]
        .join("\x1f")
    }
}

/// Pure validation helpers for the hook rules editor (`PreferencesHooksPane.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookEditorValidation;

impl HookEditorValidation {
    pub const fn can_add_rule(count: usize) -> bool {
        count < HooksConfig::MAXIMUM_RULE_COUNT
    }

    pub const fn can_add_argument(count: usize) -> bool {
        count < HookRule::MAXIMUM_ARGUMENT_COUNT
    }

    pub fn threshold_fraction(percent: Option<f64>) -> Option<f64> {
        percent.map(|p| p.clamp(1.0, 100.0) / 100.0)
    }
}

/// Structured result of running a single hook rule during `hooks test` (`CLIHooksCommand.swift`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookTestResult {
    #[serde(rename = "ruleID")]
    pub rule_id: String,
    pub executable: String,
    pub event: String,
    pub provider: String,
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Builds a representative sample [`HookEvent`] for `hooks test` (`CLIHooksCommand.swift`).
pub fn sample_hook_event(
    event_type: HookEventType,
    provider: impl Into<String>,
    now: Timestamp,
) -> HookEvent {
    let provider = provider.into();
    if event_type == HookEventType::UsageUpdated {
        let primary_reset =
            Timestamp::from_second(now.as_second().saturating_add(3600)).unwrap_or(now);
        let secondary_reset =
            Timestamp::from_second(now.as_second().saturating_add(4 * 86400)).unwrap_or(now);
        let primary = RateWindow::new(50.0, Some(300), Some(primary_reset), None, None, false).ok();
        let secondary =
            RateWindow::new(40.0, Some(10080), Some(secondary_reset), None, None, false).ok();
        let snapshot = UsageSnapshot::new(
            primary,
            secondary,
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
            now,
            None,
            ariadusage_protocol::Confidence::Unknown,
        )
        .expect("valid sample usage snapshot");
        return HookEvent::usage_updated(provider, &snapshot, None, now);
    }

    let (usage_percent, status) = match event_type {
        HookEventType::QuotaLow | HookEventType::QuotaReached => (Some(1.0), None),
        HookEventType::QuotaReset => (Some(0.0), None),
        HookEventType::UsageUpdated => (None, None),
        HookEventType::ProviderUnavailable => (None, Some("major".to_string())),
        HookEventType::ProviderRecovered => (None, Some("none".to_string())),
        HookEventType::RefreshFailed => (None, Some("error".to_string())),
    };
    let window = matches!(
        event_type,
        HookEventType::QuotaReached | HookEventType::QuotaLow | HookEventType::QuotaReset
    )
    .then(|| "session".to_string());

    HookEvent {
        event: event_type,
        provider,
        account: None,
        window,
        usage_percent,
        window_minutes: None,
        used: None,
        limit: None,
        reset_at: None,
        secondary_usage_percent: None,
        secondary_window_minutes: None,
        secondary_reset_at: None,
        status,
        timestamp: now,
    }
}

/// Default poll period (in seconds) for `hooks watch`.
pub const HOOKS_WATCH_DEFAULT_INTERVAL: f64 = 300.0;
/// Minimum allowed `--interval` (in seconds) for `hooks watch`.
pub const HOOKS_WATCH_MINIMUM_INTERVAL: f64 = 60.0;
/// Default web timeout (in seconds) for `hooks watch`.
pub const HOOKS_WATCH_DEFAULT_WEB_TIMEOUT: f64 = 60.0;
/// Poll-period tick (in nanoseconds) used by [`sleep_interruptibly`].
pub const HOOKS_WATCH_SLEEP_TICK_NANOSECONDS: u64 = 200_000_000;

/// Thread-safe stop flag flipped by a termination signal handler (`CLIHooksWatchCommand.swift`).
#[derive(Debug, Default)]
pub struct HooksWatchStopSignal {
    requested: AtomicBool,
}

impl HooksWatchStopSignal {
    pub const fn new() -> Self {
        Self {
            requested: AtomicBool::new(false),
        }
    }

    pub fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
    }

    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }
}

/// Sleeps up to `interval` seconds in bounded 200ms ticks while checking `stop` before each tick.
pub async fn sleep_interruptibly<F, Fut>(
    interval: f64,
    stop: &HooksWatchStopSignal,
    mut sleep_fn: F,
) where
    F: FnMut(u64) -> Fut,
    Fut: Future<Output = ()>,
{
    let mut remaining_nanoseconds = (interval.max(0.0) * 1_000_000_000.0).round() as u64;
    while remaining_nanoseconds > 0 && !stop.is_requested() {
        let sleep_nanoseconds = remaining_nanoseconds.min(HOOKS_WATCH_SLEEP_TICK_NANOSECONDS);
        sleep_fn(sleep_nanoseconds).await;
        remaining_nanoseconds -= sleep_nanoseconds;
    }
}

/// Argument validation error for `hooks watch` CLI decoding.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct HooksWatchArgumentError {
    pub message: String,
}

impl HooksWatchArgumentError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Rejects `--once` for `hooks watch` because transition baselines must survive between polls.
pub fn validate_hooks_watch_flags(once: bool) -> Result<(), HooksWatchArgumentError> {
    if once {
        return Err(HooksWatchArgumentError::new(
            "Unknown flag --once: hooks watch requires a continuous loop so transition state survives between polls.",
        ));
    }
    Ok(())
}

/// Validates and decodes `--interval` without reading the config file.
pub fn decode_hooks_watch_interval(raw: Option<&str>) -> Result<f64, HooksWatchArgumentError> {
    let Some(raw) = raw else {
        return Ok(HOOKS_WATCH_DEFAULT_INTERVAL);
    };
    let Ok(parsed) = raw.parse::<f64>() else {
        return Err(HooksWatchArgumentError::new(format!(
            "Invalid --interval value: {raw}"
        )));
    };
    if !parsed.is_finite() {
        return Err(HooksWatchArgumentError::new(format!(
            "Invalid --interval value: {raw}"
        )));
    }
    if parsed < HOOKS_WATCH_MINIMUM_INTERVAL {
        return Err(HooksWatchArgumentError::new(format!(
            "--interval must be at least {} seconds.",
            HOOKS_WATCH_MINIMUM_INTERVAL as i64
        )));
    }
    Ok(parsed)
}

/// Decodes `--web-timeout` for `hooks watch`, falling back to [`HOOKS_WATCH_DEFAULT_WEB_TIMEOUT`]
/// (`60.0` seconds) when omitted, non-numeric, non-finite, or `<= 0.0`.
pub fn decode_hooks_watch_web_timeout(raw: Option<&str>) -> f64 {
    raw.and_then(|s| s.parse::<f64>().ok())
        .filter(|&seconds| seconds.is_finite() && seconds > 0.0)
        .unwrap_or(HOOKS_WATCH_DEFAULT_WEB_TIMEOUT)
}

/// Validates explicit `--provider` names (`all`, `both`, or concrete provider names) without
/// reading the config file.
pub fn decode_hooks_watch_provider_names(
    raw: &[&str],
) -> Result<Option<Vec<ProviderId>>, HooksWatchArgumentError> {
    if raw.is_empty() {
        return Ok(None);
    }
    let mut selected = Vec::new();
    for &name in raw {
        let trimmed = name.trim().to_ascii_lowercase();
        let expanded: Vec<ProviderId> = match trimmed.as_str() {
            "all" => providers::first_party_order()
                .iter()
                .map(|d| d.id.clone())
                .collect(),
            "both" => vec![
                ProviderId::new("codex").expect("valid id"),
                ProviderId::new("claude").expect("valid id"),
            ],
            other => {
                let Some(desc) = providers::find_by_id_str(other) else {
                    return Err(HooksWatchArgumentError::new(format!(
                        "Unknown provider: {name}"
                    )));
                };
                vec![desc.id.clone()]
            }
        };
        for pid in expanded {
            if !selected.contains(&pid) {
                selected.push(pid);
            }
        }
    }
    Ok(Some(selected))
}

/// Resolves the providers to poll for `hooks watch`, defaulting to enabled providers in `config`.
pub fn hooks_watch_providers(
    explicit: Option<Vec<ProviderId>>,
    config: &Config,
) -> Result<Vec<ProviderId>, HooksWatchArgumentError> {
    if let Some(explicit) = explicit {
        return Ok(explicit);
    }
    let enabled: Vec<ProviderId> = config
        .enabled_providers()
        .into_iter()
        .filter_map(|id| providers::find_by_id_str(&id).map(|d| d.id.clone()))
        .collect();
    if enabled.is_empty() {
        return Err(HooksWatchArgumentError::new("No providers are enabled."));
    }
    Ok(enabled)
}

/// Builds the quota lane observations for a `hooks watch` poll (`CLIHooksWatchCommand.swift`).
pub fn hooks_watch_lanes(
    provider: &ProviderId,
    usage: &UsageSnapshot,
    config: &Config,
    account_discriminator: Option<&str>,
    hides_personal_info: bool,
) -> Vec<HookQuotaLaneObservation> {
    let account = usage
        .account_email(provider)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let warnings = config
        .provider_config(provider.as_str())
        .and_then(|c| c.quota_warnings.as_ref());
    let lanes = [
        (QuotaWarningWindow::Session, usage.primary.as_ref()),
        (QuotaWarningWindow::Weekly, usage.secondary.as_ref()),
    ];

    lanes
        .into_iter()
        .filter_map(|(window, rate_window)| {
            let rate_window = rate_window.filter(|w| !w.is_synthetic_placeholder)?;
            let thresholds = warnings.map_or_else(
                || QuotaWarnings::DEFAULTS.to_vec(),
                |w| w.thresholds(window, &QuotaWarnings::DEFAULTS),
            );
            let fallback_thresholds = thresholds
                .into_iter()
                .map(|t| (100.0 - f64::from(t)) / 100.0)
                .collect();
            let discriminator = account_discriminator
                .map(ToOwned::to_owned)
                .or_else(|| account.clone());
            let display_name = if hides_personal_info {
                None
            } else {
                account.clone()
            };
            Some(
                HookQuotaLaneObservation::new(
                    HookQuotaLaneKey::new(provider.as_str(), window, discriminator, None),
                    window.display_name(),
                    Some(rate_window.clone()),
                    fallback_thresholds,
                )
                .with_account_display_name(display_name),
            )
        })
        .collect()
}

/// Coarse failure classification for `refresh_failed` hooks (`UsageStore.refreshFailureHookStatus`).
///
/// Never forwards raw error strings or HTTP bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTransportErrorKind {
    Cancelled,
    TimedOut,
    Offline,
    NetworkError,
    AuthRequired,
    Other,
}

pub const fn hook_refresh_failure_status(kind: HookTransportErrorKind) -> &'static str {
    match kind {
        HookTransportErrorKind::Cancelled => "cancelled",
        HookTransportErrorKind::TimedOut => "timeout",
        HookTransportErrorKind::Offline => "offline",
        HookTransportErrorKind::NetworkError => "network_error",
        HookTransportErrorKind::AuthRequired => "auth_required",
        HookTransportErrorKind::Other => "error",
    }
}

/// Account display label for a hook payload, redacted when `hide_personal_info` is enabled.
pub fn hook_account_display_name(
    provider: &ProviderId,
    snapshot: &UsageSnapshot,
    hide_personal_info: bool,
) -> Option<String> {
    if hide_personal_info {
        return None;
    }
    snapshot
        .account_email(provider)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

/// Private identity used only to keep rate-limit buckets account-scoped (`UsageStore.hookAccountDiscriminator`).
/// Never serialized into hook payloads or environment variables.
pub fn hook_account_discriminator(
    provider: &ProviderId,
    snapshot: &UsageSnapshot,
) -> Option<String> {
    let identity = snapshot.identity_for(provider)?;
    if let Some(account_id) = identity
        .account_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(format!("provider-account:{account_id}"));
    }
    let email = identity
        .account_email
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_ascii_lowercase();
    Some(format!("email:{email}"))
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
    pub account_display_name: Option<String>,
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
            account_display_name: None,
        }
    }

    pub fn with_account_display_name(mut self, account_display_name: Option<String>) -> Self {
        self.account_display_name = account_display_name;
        self
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

impl HookProviderStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minor => "minor",
            Self::Major => "major",
            Self::Critical => "critical",
            Self::Maintenance => "maintenance",
            Self::Unknown => "unknown",
        }
    }
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
    pub successful_usage: Option<UsageSnapshot>,
    pub account_discriminator: Option<String>,
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
            successful_usage: None,
            account_discriminator: None,
        }
    }

    pub fn with_successful_usage(mut self, usage: Option<UsageSnapshot>) -> Self {
        self.successful_usage = usage;
        self
    }

    pub fn with_account_discriminator(mut self, discriminator: Option<String>) -> Self {
        self.account_discriminator = discriminator;
        self
    }
}

/// A transition event paired with the specific rules that should execute (when filtered)
/// and the private rate-limit account discriminator.
#[derive(Debug, Clone, PartialEq)]
pub struct HookDispatch {
    pub event: HookEvent,
    pub rules: Option<Vec<HookRule>>,
    pub account_discriminator: Option<String>,
}

impl HookDispatch {
    pub fn new(
        event: HookEvent,
        rules: Option<Vec<HookRule>>,
        account_discriminator: Option<String>,
    ) -> Self {
        Self {
            event,
            rules,
            account_discriminator,
        }
    }
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
#[derive(Debug, Clone)]
pub struct HookTransitionDetector {
    window_observation: HashMap<HookQuotaLaneKey, WindowObservation>,
    provider_status_had_issue: HashMap<String, bool>,
    last_config_revision: Option<u64>,
    reached_threshold: f64,
    reset_drop_threshold: f64,
}

impl Default for HookTransitionDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl HookTransitionDetector {
    /// Default usage fraction at or above which a session lane counts as depleted.
    pub const DEFAULT_REACHED_THRESHOLD: f64 = 1.0;
    /// Default usage drop (as a `0.0..=1.0` fraction) that counts as a quota reset when the
    /// provider does not report a `resets_at` timestamp (`HookTransitionDetector.swift`).
    pub const RESET_DROP_THRESHOLD: f64 = 0.2;
    /// Tolerance (in seconds) when comparing `resets_at` timestamps across polls.
    pub const RESET_TIMESTAMP_TOLERANCE_SECS: f64 = 0.0;

    pub fn new() -> Self {
        Self::with_thresholds(Self::DEFAULT_REACHED_THRESHOLD, Self::RESET_DROP_THRESHOLD)
    }

    pub fn with_thresholds(reached_threshold: f64, reset_drop_threshold: f64) -> Self {
        Self {
            window_observation: HashMap::new(),
            provider_status_had_issue: HashMap::new(),
            last_config_revision: None,
            reached_threshold,
            reset_drop_threshold,
        }
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

    pub fn clear_quota_low_lanes(&mut self, provider: &str) {
        self.window_observation
            .retain(|key, _| key.provider != provider);
    }

    pub fn prune_extra_lanes(
        &mut self,
        provider: &str,
        account_discriminator: Option<&str>,
        keeping_extra_window_ids: &HashSet<String>,
    ) {
        self.window_observation.retain(|key, _| {
            if key.provider != provider
                || key.account_discriminator.as_deref() != account_discriminator
            {
                return true;
            }
            let Some(window_id) = &key.window_id else {
                return true;
            };
            keeping_extra_window_ids.contains(window_id)
        });
    }

    pub fn evaluate(
        &mut self,
        observation: &HookProviderObservation,
        config: &HooksConfig,
        now: Timestamp,
    ) -> Vec<HookDispatch> {
        if !config.enabled || config.events.len() > HooksConfig::MAXIMUM_RULE_COUNT {
            return Vec::new();
        }

        if let Some(failure_status) = &observation.refresh_failure_status {
            return vec![HookDispatch {
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
                account_discriminator: observation.account_discriminator.clone(),
            }];
        }

        let mut dispatches = self.evaluate_status(observation, now);

        if let Some(usage) = &observation.successful_usage {
            let usage_event = HookEvent::usage_updated(
                observation.provider.clone(),
                usage,
                observation.account.clone(),
                now,
            );
            if config.events.iter().any(|rule| rule.matches(&usage_event)) {
                dispatches.push(HookDispatch {
                    event: usage_event,
                    rules: None,
                    account_discriminator: observation.account_discriminator.clone(),
                });
            }
        }

        let observed_keys: HashSet<HookQuotaLaneKey> = observation
            .lanes
            .iter()
            .map(|lane| lane.key.clone())
            .collect();

        for lane in &observation.lanes {
            let Some(rate_window) = &lane.rate_window else {
                self.window_observation.remove(&lane.key);
                continue;
            };
            if rate_window.is_synthetic_placeholder || !rate_window.used_percent.is_finite() {
                self.window_observation.remove(&lane.key);
                continue;
            }

            let current_usage = rate_window.used_percent / 100.0;
            let current = WindowObservation {
                usage: current_usage,
                reset_at: rate_window.resets_at,
            };

            let Some(previous) = self.window_observation.insert(lane.key.clone(), current) else {
                // First observation establishes the baseline without firing.
                continue;
            };

            let did_reset = self.window_did_reset(previous, current);
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
                    account_discriminator: None,
                });
                continue;
            }

            let candidate = self.quota_event(
                HookEventType::QuotaLow,
                observation,
                lane,
                rate_window,
                current_usage,
                now,
            );
            let matching_rules: Vec<HookRule> = config
                .events
                .iter()
                .filter(|rule| {
                    rule.enabled
                        && rule.event == HookEventType::QuotaLow
                        && rule
                            .provider
                            .as_ref()
                            .is_none_or(|p| p == &observation.provider)
                })
                .cloned()
                .collect();
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
                    account_discriminator: None,
                });
            }

            if lane.key.window == QuotaWarningWindow::Session
                && previous.usage < self.reached_threshold
                && current_usage >= self.reached_threshold
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
                    account_discriminator: None,
                });
            }
        }

        self.window_observation
            .retain(|key, _| key.provider != observation.provider || observed_keys.contains(key));

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

        let event_type = if has_issue {
            HookEventType::ProviderUnavailable
        } else {
            HookEventType::ProviderRecovered
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
                status: Some(observation.status.as_str().to_string()),
                timestamp: now,
            },
            rules: None,
            account_discriminator: None,
        }]
    }

    fn window_did_reset(&self, previous: WindowObservation, current: WindowObservation) -> bool {
        if let (Some(prev_reset), Some(curr_reset)) = (previous.reset_at, current.reset_at) {
            return curr_reset.duration_since(prev_reset).as_secs_f64()
                > Self::RESET_TIMESTAMP_TOLERANCE_SECS;
        }
        (previous.usage - current.usage) >= self.reset_drop_threshold
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
            account: lane
                .account_display_name
                .clone()
                .or_else(|| observation.account.clone()),
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
    use crate::config::{ProviderConfig, ProviderEntry, decode, encode};
    use crate::model::ProviderIdentitySnapshot;

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

    fn make_event(
        event: HookEventType,
        provider: &str,
        usage_percent: Option<f64>,
        account: Option<&str>,
        window: Option<&str>,
    ) -> HookEvent {
        HookEvent {
            event,
            provider: provider.to_string(),
            account: account.map(ToOwned::to_owned),
            window: window.map(ToOwned::to_owned),
            usage_percent,
            window_minutes: None,
            used: None,
            limit: None,
            reset_at: Some(Timestamp::from_second(1_700_000_000).expect("ts")),
            secondary_usage_percent: None,
            secondary_window_minutes: None,
            secondary_reset_at: None,
            status: None,
            timestamp: Timestamp::from_second(1_700_000_100).expect("ts"),
        }
    }

    // -------------------------------------------------------------------------
    // TestsLinux/HooksTests.swift (pure logic)
    // -------------------------------------------------------------------------

    // upstream: TestsLinux/HooksTests.swift:25 rule matches on event and provider
    #[test]
    fn rule_matches_on_event_and_provider() {
        let rule = HookRule::new(HookEventType::QuotaReached, "/bin/echo").with_provider("codex");
        assert!(rule.matches(&make_event(
            HookEventType::QuotaReached,
            "codex",
            Some(0.95),
            None,
            Some("session")
        )));
        assert!(!rule.matches(&make_event(
            HookEventType::QuotaReached,
            "claude",
            Some(0.95),
            None,
            Some("session")
        )));
        assert!(!rule.matches(&make_event(
            HookEventType::QuotaLow,
            "codex",
            Some(0.95),
            None,
            Some("session")
        )));
    }

    // upstream: TestsLinux/HooksTests.swift:33 nil provider matches any provider
    #[test]
    fn nil_provider_matches_any_provider() {
        let rule = HookRule::new(HookEventType::QuotaReached, "/bin/echo");
        assert!(rule.matches(&make_event(
            HookEventType::QuotaReached,
            "codex",
            Some(0.95),
            None,
            Some("session")
        )));
        assert!(rule.matches(&make_event(
            HookEventType::QuotaReached,
            "claude",
            Some(0.95),
            None,
            Some("session")
        )));
    }

    // upstream: TestsLinux/HooksTests.swift:40 quotaLow threshold gates on usage percent
    #[test]
    fn quota_low_threshold_gates_on_usage_percent() {
        let rule = HookRule::new(HookEventType::QuotaLow, "/bin/echo").with_threshold(0.90);
        assert!(rule.matches(&make_event(
            HookEventType::QuotaLow,
            "codex",
            Some(0.92),
            None,
            Some("session")
        )));
        assert!(rule.matches(&make_event(
            HookEventType::QuotaLow,
            "codex",
            Some(0.90),
            None,
            Some("session")
        )));
        assert!(!rule.matches(&make_event(
            HookEventType::QuotaLow,
            "codex",
            Some(0.80),
            None,
            Some("session")
        )));
        assert!(!rule.matches(&make_event(
            HookEventType::QuotaLow,
            "codex",
            None,
            None,
            Some("session")
        )));
    }

    // upstream: TestsLinux/HooksTests.swift:49 disabled rule and relative path never match
    #[test]
    fn disabled_rule_and_relative_path_never_match() {
        let disabled = HookRule::new(HookEventType::QuotaReached, "/bin/echo").with_enabled(false);
        let ev = make_event(
            HookEventType::QuotaReached,
            "codex",
            Some(0.95),
            None,
            Some("session"),
        );
        assert!(!disabled.matches(&ev));

        let relative = HookRule::new(HookEventType::QuotaReached, "my-command");
        assert!(!relative.matches(&ev));
    }

    // upstream: TestsLinux/HooksTests.swift:58 disabled config yields no matching rules
    #[test]
    fn disabled_config_yields_no_matching_rules() {
        let rule = HookRule::new(HookEventType::QuotaReached, "/bin/echo");
        let enabled = HooksConfig::new(true, vec![rule.clone()]);
        let disabled = HooksConfig::new(false, vec![rule]);
        let ev = make_event(
            HookEventType::QuotaReached,
            "codex",
            Some(0.95),
            None,
            Some("session"),
        );
        assert_eq!(enabled.matching_rules(&ev).len(), 1);
        assert!(disabled.matching_rules(&ev).is_empty());
    }

    // upstream: TestsLinux/HooksTests.swift:69 quotaLow rule fires only when its own threshold is crossed upward
    #[test]
    fn quota_low_rule_fires_only_when_its_own_threshold_is_crossed_upward() {
        let rule = HookRule::new(HookEventType::QuotaLow, "/bin/echo").with_threshold(0.90);
        let fallback = [0.50, 0.80];

        assert!(
            !crossed_quota_low_rules(std::slice::from_ref(&rule), 0.85, 0.95, &fallback).is_empty()
        );
        assert!(
            crossed_quota_low_rules(std::slice::from_ref(&rule), 0.92, 0.97, &fallback).is_empty()
        );
        assert!(crossed_quota_low_rules(&[rule], 0.40, 0.85, &fallback).is_empty());
    }

    // upstream: TestsLinux/HooksTests.swift:87 thresholdless quotaLow rule falls back to notification thresholds
    #[test]
    fn thresholdless_quota_low_rule_falls_back_to_notification_thresholds() {
        let rule = HookRule::new(HookEventType::QuotaLow, "/bin/echo");
        let fallback = [0.50, 0.80];
        assert!(
            !crossed_quota_low_rules(std::slice::from_ref(&rule), 0.40, 0.55, &fallback).is_empty()
        );
        assert!(crossed_quota_low_rules(&[rule], 0.55, 0.60, &fallback).is_empty());
    }

    // upstream: TestsLinux/HooksTests.swift:97 only the crossed rule is selected among several
    #[test]
    fn only_the_crossed_rule_is_selected_among_several() {
        let low = HookRule::new(HookEventType::QuotaLow, "/bin/echo")
            .with_id("low")
            .with_threshold(0.50);
        let high = HookRule::new(HookEventType::QuotaLow, "/bin/echo")
            .with_id("high")
            .with_threshold(0.90);
        let crossed = crossed_quota_low_rules(&[low, high], 0.85, 0.95, &[]);
        assert_eq!(
            crossed.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["high"]
        );
    }

    // upstream: TestsLinux/HooksTests.swift:109 environment variables include set fields and omit nil
    #[test]
    fn environment_variables_include_set_fields_and_omit_nil() {
        let env = make_event(
            HookEventType::QuotaLow,
            "codex",
            Some(0.5),
            None,
            Some("weekly"),
        )
        .environment_variables();
        assert_eq!(
            env.get("ARIADUSAGE_EVENT").map(String::as_str),
            Some("quota_low")
        );
        assert_eq!(
            env.get("ARIADUSAGE_PROVIDER").map(String::as_str),
            Some("codex")
        );
        assert_eq!(
            env.get("ARIADUSAGE_WINDOW").map(String::as_str),
            Some("weekly")
        );
        assert_eq!(
            env.get("ARIADUSAGE_USAGE_PERCENT").map(String::as_str),
            Some("0.5")
        );
        assert_eq!(
            env.get("ARIADUSAGE_RESET_AT").map(String::as_str),
            Some("2023-11-14T22:13:20Z")
        );
        assert!(env.contains_key("ARIADUSAGE_TIMESTAMP"));
        assert!(!env.contains_key("ARIADUSAGE_ACCOUNT"));
        assert!(!env.contains_key("ARIADUSAGE_STATUS"));
    }

    // upstream: TestsLinux/HooksTests.swift:123 json payload round-trips
    #[test]
    fn json_payload_round_trips() {
        let original = make_event(
            HookEventType::QuotaReached,
            "claude",
            Some(0.42),
            None,
            Some("session"),
        );
        let data = original.json_payload().expect("json payload");
        let decoded: HookEvent = serde_json::from_slice(&data).expect("decode hook event");
        assert_eq!(decoded.event, HookEventType::QuotaReached);
        assert_eq!(decoded.provider, "claude");
        assert_eq!(decoded.usage_percent, Some(0.42));
        assert_eq!(decoded.window.as_deref(), Some("session"));
    }

    // upstream: TestsLinux/HooksTests.swift:138 rate limiter suppresses same key within window
    #[test]
    fn rate_limiter_suppresses_same_key_within_window() {
        let mut limiter = HookRateLimiter::new(600.0);
        let base = Timestamp::from_second(1_000_000).expect("ts");
        let ev = make_event(
            HookEventType::QuotaReached,
            "codex",
            Some(0.95),
            None,
            Some("session"),
        );
        assert!(limiter.allow(&ev, None, base));
        assert!(!limiter.allow(&ev, None, Timestamp::from_second(1_000_300).expect("ts")));
        assert!(limiter.allow(&ev, None, Timestamp::from_second(1_000_601).expect("ts")));
    }

    // upstream: TestsLinux/HooksTests.swift:147 rate limiter treats distinct keys independently
    #[test]
    fn rate_limiter_treats_distinct_keys_independently() {
        let mut limiter = HookRateLimiter::new(600.0);
        let now = Timestamp::from_second(1_000_000).expect("ts");
        let codex = make_event(
            HookEventType::QuotaReached,
            "codex",
            Some(0.95),
            None,
            Some("session"),
        );
        let claude = make_event(
            HookEventType::QuotaReached,
            "claude",
            Some(0.95),
            None,
            Some("session"),
        );
        assert!(limiter.allow(&codex, None, now));
        assert!(limiter.allow(&claude, None, now));
    }

    // upstream: TestsLinux/HooksTests.swift:157 only storm-prone events are rate limited
    #[test]
    fn only_storm_prone_events_are_rate_limited() {
        assert!(HookEventType::RefreshFailed.is_rate_limited());
        assert!(HookEventType::ProviderUnavailable.is_rate_limited());
        assert!(HookEventType::UsageUpdated.is_rate_limited());
        assert!(!HookEventType::QuotaLow.is_rate_limited());
        assert!(!HookEventType::QuotaReached.is_rate_limited());
        assert!(!HookEventType::QuotaReset.is_rate_limited());
        assert!(!HookEventType::ProviderRecovered.is_rate_limited());
    }

    // -------------------------------------------------------------------------
    // TestsLinux/HookTransitionDetectorLinuxTests.swift
    // -------------------------------------------------------------------------

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

    // -------------------------------------------------------------------------
    // TestsLinux/UsageUpdatedHookMappingTests.swift (pure logic)
    // -------------------------------------------------------------------------

    // upstream: TestsLinux/UsageUpdatedHookMappingTests.swift:19 shared mapping preserves genuine zero and omits synthetic windows
    #[test]
    fn shared_mapping_preserves_genuine_zero_and_omits_synthetic_windows() {
        let now = Timestamp::from_second(1_800_000_000).expect("ts");
        let snapshot = UsageSnapshot {
            primary: Some(
                RateWindow::new(0.0, Some(300), Some(now), None, None, false).expect("rw"),
            ),
            secondary: Some(
                RateWindow::new(40.0, Some(10080), Some(now), None, None, false).expect("rw"),
            ),
            tertiary: None,
            extra_rate_windows: None,
            provider_cost: None,
            details: Vec::new(),
            subscription_expires_at: None,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            subscription_expires_at_is_date_only: false,
            updated_at: now,
            identity: None,
            data_confidence: ariadusage_protocol::Confidence::Unknown,
        };
        let zero = HookEvent::usage_updated("codex", &snapshot, None, now);
        assert_eq!(zero.usage_percent, Some(0.0));
        assert_eq!(zero.secondary_usage_percent, Some(0.4));
        assert_eq!(zero.window_minutes, Some(300));

        let placeholder = UsageSnapshot {
            primary: Some(RateWindow::new(0.0, None, None, None, None, true).expect("rw")),
            secondary: None,
            tertiary: None,
            extra_rate_windows: None,
            provider_cost: None,
            details: Vec::new(),
            subscription_expires_at: None,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            subscription_expires_at_is_date_only: false,
            updated_at: now,
            identity: None,
            data_confidence: ariadusage_protocol::Confidence::Unknown,
        };
        let event = HookEvent::usage_updated("codex", &placeholder, None, now);
        let payload: serde_json::Value =
            serde_json::from_slice(&event.json_payload().expect("payload")).expect("json");
        assert_eq!(event.usage_percent, None);
        assert!(payload.get("usagePercent").is_none());
        assert!(payload.get("windowMinutes").is_none());
        assert!(
            !event
                .environment_variables()
                .contains_key("ARIADUSAGE_SECONDARY_USAGE_PERCENT")
        );
    }

    // upstream: TestsLinux/UsageUpdatedHookMappingTests.swift:42 watch offers successful first and unchanged polls with private routing state
    #[test]
    fn watch_offers_successful_first_and_unchanged_polls_with_private_routing_state() {
        let now = Timestamp::from_second(1_800_000_000).expect("ts");
        let snapshot = UsageSnapshot {
            primary: Some(
                RateWindow::new(0.0, Some(300), Some(now), None, None, false).expect("rw"),
            ),
            secondary: Some(
                RateWindow::new(40.0, Some(10080), Some(now), None, None, false).expect("rw"),
            ),
            tertiary: None,
            extra_rate_windows: None,
            provider_cost: None,
            details: Vec::new(),
            subscription_expires_at: None,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            subscription_expires_at_is_date_only: false,
            updated_at: now,
            identity: None,
            data_confidence: ariadusage_protocol::Confidence::Unknown,
        };
        let mut detector = HookTransitionDetector::new();
        let obs = HookProviderObservation::new(
            "codex",
            None,
            Vec::new(),
            None,
            HookProviderStatus::Unknown,
            None,
        )
        .with_successful_usage(Some(snapshot.clone()))
        .with_account_discriminator(Some("private-owner".to_string()));
        let cfg = HooksConfig::new(
            true,
            vec![HookRule::new(HookEventType::UsageUpdated, "/usr/bin/true")],
        );
        for instant in [now, Timestamp::from_second(1_800_000_060).expect("ts")] {
            let dispatches = detector.evaluate(&obs, &cfg, instant);
            assert_eq!(dispatches.len(), 1);
            let dispatch = &dispatches[0];
            assert_eq!(
                dispatch.event,
                HookEvent::usage_updated("codex", &snapshot, None, instant)
            );
            assert_eq!(
                dispatch.account_discriminator.as_deref(),
                Some("private-owner")
            );
            let payload =
                String::from_utf8(dispatch.event.json_payload().expect("payload")).expect("utf8");
            assert!(!payload.contains("private-owner"));
            assert!(
                !dispatch
                    .event
                    .environment_variables()
                    .contains_key("ARIADUSAGE_ACCOUNT")
            );
        }
    }

    // upstream: TestsLinux/UsageUpdatedHookMappingTests.swift:63 failed and status only observations cannot emit usage updates
    #[test]
    fn failed_and_status_only_observations_cannot_emit_usage_updates() {
        let now = Timestamp::from_second(1_800_000_000).expect("ts");
        let snapshot = UsageSnapshot {
            primary: Some(
                RateWindow::new(0.0, Some(300), Some(now), None, None, false).expect("rw"),
            ),
            secondary: None,
            tertiary: None,
            extra_rate_windows: None,
            provider_cost: None,
            details: Vec::new(),
            subscription_expires_at: None,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            subscription_expires_at_is_date_only: false,
            updated_at: now,
            identity: None,
            data_confidence: ariadusage_protocol::Confidence::Unknown,
        };
        let mut detector = HookTransitionDetector::new();
        let cfg = HooksConfig::new(
            true,
            vec![HookRule::new(HookEventType::UsageUpdated, "/usr/bin/true")],
        );
        let failure = HookProviderObservation::new(
            "codex",
            None,
            Vec::new(),
            None,
            HookProviderStatus::Unknown,
            Some("offline".to_string()),
        )
        .with_successful_usage(Some(snapshot));
        assert_eq!(
            detector
                .evaluate(&failure, &cfg, now)
                .into_iter()
                .map(|d| d.event.event)
                .collect::<Vec<_>>(),
            vec![HookEventType::RefreshFailed]
        );

        let status_only = HookProviderObservation::new(
            "codex",
            None,
            Vec::new(),
            None,
            HookProviderStatus::None,
            None,
        );
        assert!(detector.evaluate(&status_only, &cfg, now).is_empty());
    }

    // upstream: TestsLinux/UsageUpdatedHookMappingTests.swift:75 private account throttles open at exactly 600 seconds and reset on restart
    #[test]
    fn private_account_throttles_open_at_exactly_600_seconds_and_reset_on_restart() {
        let now = Timestamp::from_second(1_800_000_000).expect("ts");
        let snapshot = UsageSnapshot {
            primary: Some(
                RateWindow::new(0.0, Some(300), Some(now), None, None, false).expect("rw"),
            ),
            secondary: None,
            tertiary: None,
            extra_rate_windows: None,
            provider_cost: None,
            details: Vec::new(),
            subscription_expires_at: None,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            subscription_expires_at_is_date_only: false,
            updated_at: now,
            identity: None,
            data_confidence: ariadusage_protocol::Confidence::Unknown,
        };
        let event = HookEvent::usage_updated("codex", &snapshot, None, now);
        let mut limiter = HookRateLimiter::default();
        assert!(limiter.allow(&event, Some("A"), now));
        assert!(!limiter.allow(
            &event,
            Some("A"),
            Timestamp::from_second(1_800_000_599).expect("ts")
        ));
        assert!(limiter.allow(
            &event,
            Some("B"),
            Timestamp::from_second(1_800_000_599).expect("ts")
        ));
        assert!(limiter.allow(
            &event,
            Some("A"),
            Timestamp::from_second(1_800_000_600).expect("ts")
        ));
        assert!(HookRateLimiter::default().allow(&event, Some("A"), now));
    }

    // -------------------------------------------------------------------------
    // TestsLinux/CLIHooksWatchSleepLinuxTests.swift
    // -------------------------------------------------------------------------

    // upstream: TestsLinux/CLIHooksWatchSleepLinuxTests.swift:7 already requested stop skips every sleep tick
    #[tokio::test]
    async fn already_requested_stop_skips_every_sleep_tick() {
        let stop = HooksWatchStopSignal::new();
        stop.request();

        sleep_interruptibly(30.0, &stop, |_| async {
            panic!("An already requested stop must not sleep");
        })
        .await;
    }

    // upstream: TestsLinux/CLIHooksWatchSleepLinuxTests.swift:17 stop requested during a tick prevents the next sleep
    #[tokio::test]
    async fn stop_requested_during_a_tick_prevents_the_next_sleep() {
        let stop = HooksWatchStopSignal::new();
        let mut ticks = Vec::new();
        sleep_interruptibly(10.0, &stop, |ns| {
            ticks.push(ns);
            stop.request();
            async {}
        })
        .await;

        assert_eq!(ticks, vec![200_000_000]);
    }

    // upstream: TestsLinux/CLIHooksWatchSleepLinuxTests.swift:31 unsignaled sleep requests the full interval in bounded ticks
    #[tokio::test]
    async fn unsignaled_sleep_requests_the_full_interval_in_bounded_ticks() {
        for interval in [0.0, -1.0, 0.05, 0.4, 0.45] {
            let stop = HooksWatchStopSignal::new();
            let mut ticks = Vec::new();
            sleep_interruptibly(interval, &stop, |ns| {
                ticks.push(ns);
                async {}
            })
            .await;

            let expected = (interval.max(0.0) * 1_000_000_000.0).round() as u64;
            assert_eq!(ticks.iter().copied().sum::<u64>(), expected);
            assert!(ticks.iter().all(|&t| t > 0 && t <= 200_000_000));
            assert_eq!(ticks.len(), expected.div_ceil(200_000_000) as usize);
        }
    }

    // -------------------------------------------------------------------------
    // TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift
    // -------------------------------------------------------------------------

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:23 once mode is rejected because watch state must survive between polls
    #[test]
    fn once_mode_is_rejected_because_watch_state_must_survive_between_polls() {
        let err = validate_hooks_watch_flags(true).expect_err("expected --once to be rejected");
        assert!(err.message.contains("--once"));
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:35 interval below the floor is rejected without reading config
    #[test]
    fn interval_below_the_floor_is_rejected_without_reading_config() {
        let err = decode_hooks_watch_interval(Some("5")).expect_err("expected failure");
        assert!(err.message.contains("at least 60"));
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:45 non numeric interval is rejected without reading config
    #[test]
    fn non_numeric_interval_is_rejected_without_reading_config() {
        let err = decode_hooks_watch_interval(Some("abc")).expect_err("expected failure");
        assert!(err.message.contains("Invalid --interval"));
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:55 interval defaults when absent
    #[test]
    fn interval_defaults_when_absent() {
        assert_eq!(
            decode_hooks_watch_interval(None).expect("default interval"),
            HOOKS_WATCH_DEFAULT_INTERVAL
        );
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:65 interval at the floor is accepted
    #[test]
    fn interval_at_the_floor_is_accepted() {
        assert_eq!(
            decode_hooks_watch_interval(Some("60")).expect("floor accepted"),
            60.0
        );
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:77 unknown provider name is rejected without reading config
    #[test]
    fn unknown_provider_name_is_rejected_without_reading_config() {
        let err =
            decode_hooks_watch_provider_names(&["nosuchprovider"]).expect_err("expected failure");
        assert!(err.message.contains("Unknown provider"));
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:88 absent provider option defers to config
    #[test]
    fn absent_provider_option_defers_to_config() {
        assert_eq!(decode_hooks_watch_provider_names(&[]).expect("ok"), None);
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:99 explicit providers resolve without consulting config
    #[test]
    fn explicit_providers_resolve_without_consulting_config() {
        let resolved = decode_hooks_watch_provider_names(&["codex", "codex"]).expect("ok");
        assert_eq!(resolved, Some(vec![ProviderId::new("codex").expect("id")]));
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:113 explicit providers win over the enabled set
    #[test]
    fn explicit_providers_win_over_the_enabled_set() {
        let config = Config::new(Config::CURRENT_VERSION, Vec::new());
        let codex = ProviderId::new("codex").expect("id");
        let resolved =
            hooks_watch_providers(Some(vec![codex.clone()]), &config).expect("explicit wins");
        assert_eq!(resolved, vec![codex]);
    }

    // upstream: TestsLinux/CLIHooksWatchArgumentPrecedenceLinuxTests.swift:124 empty enabled set reports no providers
    #[test]
    fn empty_enabled_set_reports_no_providers() {
        let mut claude = ProviderConfig::new(ProviderId::new("claude").expect("id"));
        claude.enabled = Some(false);
        let config = Config::new(Config::CURRENT_VERSION, vec![ProviderEntry::Typed(claude)]);
        let err = hooks_watch_providers(None, &config).expect_err("expected empty failure");
        assert!(err.message.contains("No providers are enabled"));
    }

    // -------------------------------------------------------------------------
    // Tests/CodexBarTests/CLIHooksTests.swift
    // -------------------------------------------------------------------------

    // upstream: Tests/CodexBarTests/CLIHooksTests.swift:8 watch provider option accepts all both and deduplicates concrete names
    #[test]
    fn watch_provider_option_accepts_all_both_and_deduplicates_concrete_names() {
        let all = decode_hooks_watch_provider_names(&["all"])
            .expect("all")
            .expect("some");
        let expected_all: Vec<ProviderId> = providers::first_party_order()
            .iter()
            .map(|d| d.id.clone())
            .collect();
        assert_eq!(all, expected_all);

        let both = decode_hooks_watch_provider_names(&["both", "codex", "codex"])
            .expect("both")
            .expect("some");
        assert_eq!(
            both,
            vec![
                ProviderId::new("codex").expect("id"),
                ProviderId::new("claude").expect("id"),
            ]
        );

        assert!(decode_hooks_watch_provider_names(&["bogus"]).is_err());
    }

    // upstream: Tests/CodexBarTests/CLIHooksTests.swift:27 watch privacy keeps account routing private and skips synthetic lanes
    #[test]
    fn watch_privacy_keeps_account_routing_private_and_skips_synthetic_lanes() {
        let now = sample_now();
        let codex = ProviderId::new("codex").expect("id");
        let usage = UsageSnapshot {
            primary: Some(RateWindow::new(0.0, Some(300), None, None, None, false).expect("rw")),
            secondary: Some(RateWindow::new(0.0, None, None, None, None, true).expect("rw")),
            tertiary: None,
            extra_rate_windows: None,
            provider_cost: None,
            details: Vec::new(),
            subscription_expires_at: None,
            subscription_renews_at: None,
            subscription_renews_at_is_date_only: false,
            subscription_expires_at_is_date_only: false,
            updated_at: now,
            identity: Some(ProviderIdentitySnapshot {
                provider_id: Some(codex.clone()),
                account_id: None,
                account_email: Some("fixture@example.invalid".to_string()),
                account_organization: None,
                login_method: None,
            }),
            data_confidence: ariadusage_protocol::Confidence::Unknown,
        };
        let config = Config::new(Config::CURRENT_VERSION, Vec::new());
        let lanes = hooks_watch_lanes(&codex, &usage, &config, Some("private-owner"), true);
        assert_eq!(lanes.len(), 1);
        assert_eq!(lanes[0].account_display_name, None);
        assert_eq!(
            lanes[0].key.account_discriminator.as_deref(),
            Some("private-owner")
        );
        assert_eq!(
            lanes[0].rate_window.as_ref().map(|w| w.used_percent),
            Some(0.0)
        );
    }

    // upstream: Tests/CodexBarTests/CLIHooksTests.swift:55 sample quota-low event matches maximum threshold
    #[test]
    fn sample_quota_low_event_matches_maximum_threshold() {
        let event = sample_hook_event(HookEventType::QuotaLow, "codex", sample_now());
        let rule = HookRule::new(HookEventType::QuotaLow, "/bin/echo").with_threshold(1.0);
        assert_eq!(event.usage_percent, Some(1.0));
        assert!(rule.matches(&event));
    }

    // upstream: Tests/CodexBarTests/CLIHooksTests.swift:64 sample refresh failure uses production status
    #[test]
    fn sample_refresh_failure_uses_production_status() {
        let event = sample_hook_event(HookEventType::RefreshFailed, "codex", sample_now());
        assert_eq!(event.status.as_deref(), Some("error"));
    }

    // upstream: Tests/CodexBarTests/CLIHooksTests.swift:71 sample usage update represents session and weekly windows
    #[test]
    fn sample_usage_update_represents_session_and_weekly_windows() {
        let event = sample_hook_event(HookEventType::UsageUpdated, "codex", sample_now());
        assert_eq!(event.usage_percent, Some(0.5));
        assert_eq!(event.window_minutes, Some(300));
        assert!(event.reset_at.is_some());
        assert_eq!(event.secondary_usage_percent, Some(0.4));
        assert_eq!(event.secondary_window_minutes, Some(10080));
        assert!(event.secondary_reset_at.is_some());
    }

    // upstream: Tests/CodexBarTests/CLIHooksTests.swift:83 usage updated event carries primary and secondary quota windows
    #[test]
    fn usage_updated_event_carries_primary_and_secondary_quota_windows() {
        let primary_reset: Timestamp = "2026-09-08T12:00:00Z".parse().expect("ts");
        let secondary_reset: Timestamp = "2026-09-10T12:00:00Z".parse().expect("ts");
        let event = HookEvent {
            event: HookEventType::UsageUpdated,
            provider: "codex".to_string(),
            account: None,
            window: Some("Session".to_string()),
            usage_percent: Some(0.2),
            window_minutes: None,
            used: None,
            limit: None,
            reset_at: Some(primary_reset),
            secondary_usage_percent: Some(0.6),
            secondary_window_minutes: None,
            secondary_reset_at: Some(secondary_reset),
            status: None,
            timestamp: primary_reset,
        };
        let data = event.json_payload().expect("payload");
        let decoded: HookEvent = serde_json::from_slice(&data).expect("decode");
        assert_eq!(decoded.event, HookEventType::UsageUpdated);
        assert_eq!(decoded.usage_percent, Some(0.2));
        assert_eq!(decoded.secondary_usage_percent, Some(0.6));
        assert_eq!(decoded.secondary_reset_at, Some(secondary_reset));
    }

    // upstream: Tests/CodexBarTests/CLIHooksTests.swift:108 hook test JSON result is structured
    #[test]
    fn hook_test_json_result_is_structured() {
        let result = HookTestResult {
            rule_id: "fixture".to_string(),
            executable: "/bin/echo".to_string(),
            event: "quota_reached".to_string(),
            provider: "codex".to_string(),
            success: true,
            stdout: Some("ok".to_string()),
            error: None,
        };
        let encoded = serde_json::to_string(&vec![result.clone()]).expect("encode");
        let decoded: Vec<HookTestResult> = serde_json::from_str(&encoded).expect("decode");
        assert_eq!(decoded, vec![result]);
    }

    // -------------------------------------------------------------------------
    // Tests/CodexBarTests/HookEditorValidationTests.swift
    // -------------------------------------------------------------------------

    // upstream: Tests/CodexBarTests/HookEditorValidationTests.swift:6 rule creation stops at runtime limit
    #[test]
    fn rule_creation_stops_at_runtime_limit() {
        assert!(HookEditorValidation::can_add_rule(
            HooksConfig::MAXIMUM_RULE_COUNT - 1
        ));
        assert!(!HookEditorValidation::can_add_rule(
            HooksConfig::MAXIMUM_RULE_COUNT
        ));
    }

    // upstream: Tests/CodexBarTests/HookEditorValidationTests.swift:12 argument creation stops at runtime limit
    #[test]
    fn argument_creation_stops_at_runtime_limit() {
        assert!(HookEditorValidation::can_add_argument(
            HookRule::MAXIMUM_ARGUMENT_COUNT - 1
        ));
        assert!(!HookEditorValidation::can_add_argument(
            HookRule::MAXIMUM_ARGUMENT_COUNT
        ));
    }

    // upstream: Tests/CodexBarTests/HookEditorValidationTests.swift:18 quota threshold stays in runtime valid range
    #[test]
    fn quota_threshold_stays_in_runtime_valid_range() {
        assert_eq!(HookEditorValidation::threshold_fraction(None), None);
        assert_eq!(
            HookEditorValidation::threshold_fraction(Some(0.0)),
            Some(0.01)
        );
        assert_eq!(
            HookEditorValidation::threshold_fraction(Some(-5.0)),
            Some(0.01)
        );
        assert_eq!(
            HookEditorValidation::threshold_fraction(Some(50.0)),
            Some(0.5)
        );
        assert_eq!(
            HookEditorValidation::threshold_fraction(Some(120.0)),
            Some(1.0)
        );
    }

    // -------------------------------------------------------------------------
    // Tests/CodexBarTests/CodexBarConfigHooksTests.swift
    // -------------------------------------------------------------------------

    // upstream: Tests/CodexBarTests/CodexBarConfigHooksTests.swift:6 hooks survive config round trip
    #[test]
    fn hooks_survive_config_round_trip() {
        let hooks = HooksConfig::new(
            true,
            vec![
                HookRule::new(HookEventType::QuotaLow, "/usr/bin/true")
                    .with_id("quota-low")
                    .with_provider("codex")
                    .with_threshold(0.9)
                    .with_timeout_seconds(30.0),
            ],
        );
        let mut codex = ProviderConfig::new(ProviderId::new("codex").expect("id"));
        codex.enabled = Some(true);
        let config = Config::new(Config::CURRENT_VERSION, vec![ProviderEntry::Typed(codex)])
            .with_hooks_config(Some(hooks.clone()));

        let data = encode(&config);
        let decoded = decode(&data).expect("decode").expect("some");
        assert_eq!(decoded.hooks_config(), Some(hooks));
    }

    // upstream: Tests/CodexBarTests/CodexBarConfigHooksTests.swift:33 sharing is opt in and legacy configs omit the flag on round trip
    #[test]
    fn sharing_is_opt_in_and_legacy_configs_omit_the_flag_on_round_trip() {
        let legacy_json = br#"{"version":1,"providers":[{"id":"codex","enabled":true}]}"#;
        let mut config = decode(legacy_json).expect("decode").expect("some");
        assert!(!config.share_usage_with_cli);
        assert!(config.providers.first().is_some_and(|p| p.is_enabled()));
        for enabled in [false, true, false] {
            config.share_usage_with_cli = enabled;
            let data = encode(&config);
            let text = String::from_utf8(data.clone()).expect("utf8");
            assert_eq!(text.contains("shareUsageWithCLI"), enabled);
            assert_eq!(
                decode(&data)
                    .expect("decode")
                    .expect("some")
                    .share_usage_with_cli,
                enabled
            );
        }
    }

    // -------------------------------------------------------------------------
    // Tests/CodexBarTests/RefreshFailureHookStatusTests.swift
    // -------------------------------------------------------------------------

    // upstream: Tests/CodexBarTests/RefreshFailureHookStatusTests.swift:6 maps URL errors to coarse categories
    #[test]
    fn maps_url_errors_to_coarse_categories() {
        assert_eq!(
            hook_refresh_failure_status(HookTransportErrorKind::TimedOut),
            "timeout"
        );
        assert_eq!(
            hook_refresh_failure_status(HookTransportErrorKind::Offline),
            "offline"
        );
        assert_eq!(
            hook_refresh_failure_status(HookTransportErrorKind::Cancelled),
            "cancelled"
        );
    }

    // upstream: Tests/CodexBarTests/RefreshFailureHookStatusTests.swift:20 never forwards the raw error description
    #[test]
    fn never_forwards_the_raw_error_description() {
        let status = hook_refresh_failure_status(HookTransportErrorKind::Other);
        assert_eq!(status, "error");
        assert!(!status.contains("secret-token"));
        assert!(!status.contains("500"));
    }

    // -------------------------------------------------------------------------
    // Tests/CodexBarTests/QuotaLowHookAccountScopingTests.swift
    // -------------------------------------------------------------------------

    // upstream: Tests/CodexBarTests/QuotaLowHookAccountScopingTests.swift:8 quota_low crossing history is scoped per account
    #[test]
    fn quota_low_crossing_history_is_scoped_per_account() {
        let account_a = HookQuotaLaneKey::new(
            "claude",
            QuotaWarningWindow::Session,
            Some("a@example.com".to_string()),
            None,
        );
        let account_b = HookQuotaLaneKey::new(
            "claude",
            QuotaWarningWindow::Session,
            Some("b@example.com".to_string()),
            None,
        );
        assert_ne!(account_a, account_b);

        let mut usage = HashMap::new();
        usage.insert(account_a.clone(), 0.40_f64);
        usage.insert(account_b.clone(), 0.95_f64);
        assert_eq!(usage.get(&account_a), Some(&0.40));
        assert_eq!(usage.get(&account_b), Some(&0.95));
    }

    // upstream: Tests/CodexBarTests/QuotaLowHookAccountScopingTests.swift:26 distinct windows and lanes stay independent for one account
    #[test]
    fn distinct_windows_and_lanes_stay_independent_for_one_account() {
        let session = HookQuotaLaneKey::new(
            "claude",
            QuotaWarningWindow::Session,
            Some("a@example.com".to_string()),
            None,
        );
        let weekly = HookQuotaLaneKey::new(
            "claude",
            QuotaWarningWindow::Weekly,
            Some("a@example.com".to_string()),
            None,
        );
        let scoped = HookQuotaLaneKey::new(
            "claude",
            QuotaWarningWindow::Weekly,
            Some("a@example.com".to_string()),
            Some("claude-weekly-scoped-fable".to_string()),
        );
        let set: HashSet<HookQuotaLaneKey> = [session, weekly, scoped].into_iter().collect();
        assert_eq!(set.len(), 3);
    }

    // upstream: Tests/CodexBarTests/QuotaLowHookAccountScopingTests.swift:40 inactive hooks discard quota-low baselines
    #[test]
    fn inactive_hooks_discard_quota_low_baselines() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();
        let claude = HookQuotaLaneKey::new(
            "claude",
            QuotaWarningWindow::Session,
            Some("account".to_string()),
            None,
        );
        let codex = HookQuotaLaneKey::new(
            "codex",
            QuotaWarningWindow::Session,
            Some("account".to_string()),
            None,
        );
        let _ = detector.evaluate(
            &HookProviderObservation::new(
                "claude",
                None,
                vec![lane(Some(40.0), claude, None, vec![0.8], false)],
                None,
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        let _ = detector.evaluate(
            &HookProviderObservation::new(
                "codex",
                None,
                vec![lane(Some(50.0), codex.clone(), None, vec![0.8], false)],
                None,
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );

        detector.clear_quota_low_lanes("claude");

        let codex_crossing = detector.evaluate(
            &HookProviderObservation::new(
                "codex",
                None,
                vec![lane(Some(85.0), codex, None, vec![0.8], false)],
                None,
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert_eq!(codex_crossing.len(), 1);
    }

    // upstream: Tests/CodexBarTests/QuotaLowHookAccountScopingTests.swift:55 configuration revision discards quota-low baselines
    #[test]
    fn configuration_revision_discards_quota_low_baselines() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();
        let key = HookQuotaLaneKey::new(
            "claude",
            QuotaWarningWindow::Session,
            Some("account".to_string()),
            None,
        );
        detector.reset_if_configuration_changed(1);
        let _ = detector.evaluate(
            &HookProviderObservation::new(
                "claude",
                None,
                vec![lane(Some(40.0), key.clone(), None, vec![0.8], false)],
                None,
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );

        detector.reset_if_configuration_changed(2);
        let after = detector.evaluate(
            &HookProviderObservation::new(
                "claude",
                None,
                vec![lane(Some(90.0), key, None, vec![0.8], false)],
                None,
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        assert!(after.is_empty());
    }

    // upstream: Tests/CodexBarTests/QuotaLowHookAccountScopingTests.swift:70 vanished extra lanes discard quota-low baselines
    #[test]
    fn vanished_extra_lanes_discard_quota_low_baselines() {
        let mut detector = HookTransitionDetector::new();
        let cfg = default_config(true, None);
        let now = sample_now();
        let retained = HookQuotaLaneKey::new(
            "claude",
            QuotaWarningWindow::Weekly,
            Some("account".to_string()),
            Some("retained".to_string()),
        );
        let vanished = HookQuotaLaneKey::new(
            "claude",
            QuotaWarningWindow::Weekly,
            Some("account".to_string()),
            Some("vanished".to_string()),
        );
        let _ = detector.evaluate(
            &HookProviderObservation::new(
                "claude",
                None,
                vec![
                    lane(Some(40.0), retained.clone(), None, vec![0.8], false),
                    lane(Some(50.0), vanished.clone(), None, vec![0.8], false),
                ],
                None,
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );

        let keeping: HashSet<String> = ["retained".to_string()].into_iter().collect();
        detector.prune_extra_lanes("claude", Some("account"), &keeping);

        let dispatches = detector.evaluate(
            &HookProviderObservation::new(
                "claude",
                None,
                vec![
                    lane(Some(85.0), retained, None, vec![0.8], false),
                    lane(Some(85.0), vanished, None, vec![0.8], false),
                ],
                None,
                HookProviderStatus::Unknown,
                None,
            ),
            &cfg,
            now,
        );
        // Only `retained` had a prior baseline, so only `retained` crosses.
        assert_eq!(dispatches.len(), 1);
    }
}
