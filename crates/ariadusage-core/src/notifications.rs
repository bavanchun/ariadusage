// Ported from CodexBar Sources/CodexBar/SessionQuotaNotifications.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+SessionQuotaTransition.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+QuotaWarnings.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/LimitResetNotifications.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+LimitResetBoundary.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+LimitResetIdentity.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+LimitResetCelebration.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/Providers/Codex/CodexLimitResetOwnerKey.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+CredentialNotifications.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/ProviderCredentialFailure.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/CodexResetCreditExpiryNotifier.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuCardView+CodexResetCredits.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use ariadusage_protocol::ProviderId;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{QuotaWarningWindow, QuotaWarningWindowConfig, QuotaWarnings};
use crate::model::{NamedRateWindow, RateWindow, UsageSnapshot};
use crate::providers::find_by_id;
use crate::refresh::SessionQuotaTransition;

/// Equivalence tolerance in seconds for reset boundaries (`2 * 60`).
pub const PLAN_UTILIZATION_RESET_EQUIVALENCE_TOLERANCE_SECONDS: i64 = 2 * 60;

const ANTIGRAVITY_QUOTA_SUMMARY_WINDOW_ID_PREFIX: &str = "antigravity-quota-summary-";
const LIMIT_RESET_THRESHOLD: f64 = 1.0;
const CLAUDE_WEEKLY_RECOVERY_OBSERVATION_COUNT: usize = 2;
const CODEX_WEEKLY_ADVANCED_BOUNDARY_RESET_THRESHOLD: f64 = 5.0;
const CODEX_WEEKLY_RESET_CANDIDATE_BASELINE_THRESHOLD: f64 = 25.0;
const CODEX_WEEKLY_RESET_CANDIDATE_CONFIRMATION_THRESHOLD: f64 = 5.0;
const CODEX_WEEKLY_RESET_CANDIDATE_MINIMUM_AGE_SECS: f64 = 60.0;
const CODEX_WEEKLY_RESET_CANDIDATE_MAXIMUM_AGE_SECS: f64 = 30.0 * 60.0;
const CODEX_WEEKLY_RESET_CANDIDATE_BOUNDARY_TOLERANCE_SECS: f64 = 1.0;

/// Sanitized notification payload ready to be delivered by an OS or IPC notifier backend.
///
/// Security invariant: `title` and `body` never contain a token, an email, or an account identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPost {
    pub id_prefix: String,
    pub identifier: Option<String>,
    pub title: String,
    pub body: String,
    pub sound_enabled: bool,
    pub on_screen_alert_enabled: bool,
}

impl NotificationPost {
    pub fn new(
        id_prefix: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            id_prefix: id_prefix.into(),
            identifier: None,
            title: title.into(),
            body: body.into(),
            sound_enabled: true,
            on_screen_alert_enabled: false,
        }
    }
}

/// User-facing notification preferences controlling the five notification families.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPreferences {
    #[serde(default = "default_true")]
    pub session_quota_notifications_enabled: bool,
    #[serde(default)]
    pub quota_warning_notifications_enabled: bool,
    #[serde(default = "default_true")]
    pub quota_warning_sound_enabled: bool,
    #[serde(default)]
    pub quota_warning_on_screen_alert_enabled: bool,
    #[serde(default = "default_quota_warning_thresholds")]
    pub quota_warning_thresholds: Vec<u32>,
    #[serde(default = "default_true")]
    pub quota_warning_session_enabled: bool,
    #[serde(default = "default_true")]
    pub quota_warning_weekly_enabled: bool,
    #[serde(default)]
    pub limit_reset_notifications_enabled: bool,
    #[serde(default = "default_true")]
    pub credential_expiry_notifications_enabled: bool,
    #[serde(default)]
    pub hide_personal_info: bool,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub provider_quota_warnings: HashMap<ProviderId, QuotaWarnings>,
}

fn default_true() -> bool {
    true
}

fn default_quota_warning_thresholds() -> Vec<u32> {
    QuotaWarnings::DEFAULTS.to_vec()
}

impl Default for NotificationPreferences {
    fn default() -> Self {
        Self {
            session_quota_notifications_enabled: true,
            quota_warning_notifications_enabled: false,
            quota_warning_sound_enabled: true,
            quota_warning_on_screen_alert_enabled: false,
            quota_warning_thresholds: QuotaWarnings::DEFAULTS.to_vec(),
            quota_warning_session_enabled: true,
            quota_warning_weekly_enabled: true,
            limit_reset_notifications_enabled: false,
            credential_expiry_notifications_enabled: true,
            hide_personal_info: false,
            provider_quota_warnings: HashMap::new(),
        }
    }
}

impl NotificationPreferences {
    pub fn set_quota_warning_window_enabled(&mut self, window: QuotaWarningWindow, enabled: bool) {
        match window {
            QuotaWarningWindow::Session => self.quota_warning_session_enabled = enabled,
            QuotaWarningWindow::Weekly => self.quota_warning_weekly_enabled = enabled,
        }
    }

    pub fn quota_warning_window_enabled(&self, window: QuotaWarningWindow) -> bool {
        match window {
            QuotaWarningWindow::Session => self.quota_warning_session_enabled,
            QuotaWarningWindow::Weekly => self.quota_warning_weekly_enabled,
        }
    }

    pub fn quota_warning_enabled(&self, provider: &ProviderId, window: QuotaWarningWindow) -> bool {
        let global = self.quota_warning_window_enabled(window);
        self.provider_quota_warnings
            .get(provider)
            .map_or(global, |qw| qw.is_enabled(window, global))
    }

    pub fn resolved_quota_warning_thresholds(
        &self,
        provider: &ProviderId,
        window: QuotaWarningWindow,
    ) -> Vec<u32> {
        let global = QuotaWarnings::sanitize_thresholds(&self.quota_warning_thresholds);
        self.provider_quota_warnings
            .get(provider)
            .map_or(global.clone(), |qw| qw.thresholds(window, &global))
    }

    pub fn set_provider_quota_warning_thresholds(
        &mut self,
        provider: ProviderId,
        window: QuotaWarningWindow,
        thresholds: Vec<u32>,
    ) {
        let entry = self.provider_quota_warnings.entry(provider).or_default();
        let config = QuotaWarningWindowConfig::new(Some(thresholds), None);
        match window {
            QuotaWarningWindow::Session => entry.session = Some(config),
            QuotaWarningWindow::Weekly => entry.weekly = Some(config),
        }
    }
}

/// Source of a session quota window used to detect mode changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionQuotaWindowSource {
    Primary,
    CopilotSecondaryFallback,
    AntigravityQuotaSummary,
    AntigravityLegacy,
}

impl SessionQuotaWindowSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::CopilotSecondaryFallback => "copilotSecondaryFallback",
            Self::AntigravityQuotaSummary => "antigravityQuotaSummary",
            Self::AntigravityLegacy => "antigravityLegacy",
        }
    }
}

/// Returns whether two reset boundary timestamps are within the equivalence tolerance (`2 * 60` seconds).
pub fn are_equivalent_plan_utilization_reset_boundaries(
    lhs: Option<Timestamp>,
    rhs: Option<Timestamp>,
) -> bool {
    let (Some(lhs), Some(rhs)) = (lhs, rhs) else {
        return false;
    };
    let diff_ms = (lhs.as_millisecond() - rhs.as_millisecond()).abs();
    diff_ms < PLAN_UTILIZATION_RESET_EQUIVALENCE_TOLERANCE_SECONDS * 1000
}

/// Returns whether `current` advances past `previous` outside the equivalence tolerance.
pub fn limit_reset_boundary_advanced(
    previous: Option<Timestamp>,
    current: Option<Timestamp>,
    requires_previous_boundary: bool,
) -> bool {
    let Some(previous) = previous else {
        return !requires_previous_boundary;
    };
    let Some(current) = current else {
        return false;
    };
    !are_equivalent_plan_utilization_reset_boundaries(Some(previous), Some(current))
        && current > previous
}

/// Tracked state for session quota depletion/restoration transitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionQuotaTransitionState {
    pub remaining: f64,
    pub source: SessionQuotaWindowSource,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub observed_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_discriminator: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub trusted_reset_boundary: Option<Timestamp>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub pending_codex_restore_observation_at: Option<Timestamp>,
}

impl SessionQuotaTransitionState {
    pub fn advancing_observation_watermark(&self, observed_at: Timestamp) -> Self {
        if observed_at <= self.observed_at {
            return self.clone();
        }
        Self {
            remaining: self.remaining,
            source: self.source,
            observed_at,
            account_discriminator: self.account_discriminator.clone(),
            trusted_reset_boundary: self.trusted_reset_boundary,
            pending_codex_restore_observation_at: self.pending_codex_restore_observation_at,
        }
    }
}

/// Watermark barrier requiring a fresh Codex baseline after invalidation.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionQuotaBaselineRequirement {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub observed_at_watermark: Option<Timestamp>,
}

impl CodexSessionQuotaBaselineRequirement {
    pub fn merging(&self, observed_at: Option<Timestamp>) -> Self {
        let Some(observed_at) = observed_at else {
            return self.clone();
        };
        let next = match self.observed_at_watermark {
            Some(watermark) => watermark.max(observed_at),
            None => observed_at,
        };
        Self {
            observed_at_watermark: Some(next),
        }
    }

    pub fn admits(&self, observed_at: Timestamp) -> bool {
        self.observed_at_watermark
            .is_none_or(|watermark| observed_at > watermark)
    }
}

/// Reducer outcome for a session quota sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionQuotaTransitionOutcome {
    None,
    Depleted,
    Restored,
    BaselineChanged,
    StaleCodexObservation,
    SuppressedCodexRestore,
    AwaitingCodexRestoreConfirmation,
}

impl SessionQuotaTransitionOutcome {
    pub fn transition(self) -> SessionQuotaTransition {
        match self {
            Self::Depleted => SessionQuotaTransition::Depleted,
            Self::Restored => SessionQuotaTransition::Restored,
            _ => SessionQuotaTransition::None,
        }
    }
}

/// Reducer evaluation result pairing the transition outcome with the next state.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionQuotaTransitionEvaluation {
    pub outcome: SessionQuotaTransitionOutcome,
    pub state: SessionQuotaTransitionState,
}

/// Input observation for `SessionQuotaTransitionReducer`.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionQuotaTransitionObservation {
    pub provider: ProviderId,
    pub remaining: f64,
    pub source: SessionQuotaWindowSource,
    pub reset_boundary: Option<Timestamp>,
    pub observed_at: Timestamp,
    pub evaluation_time: Timestamp,
    pub account_discriminator: Option<String>,
}

/// Pure logic for session quota depletion/restoration transitions and notification copy.
pub struct SessionQuotaNotificationLogic;

impl SessionQuotaNotificationLogic {
    pub const DEPLETED_THRESHOLD: f64 = 0.0001;

    pub fn is_depleted(remaining: Option<f64>) -> bool {
        remaining.is_some_and(|r| r <= Self::DEPLETED_THRESHOLD)
    }

    pub fn transition(
        previous_remaining: Option<f64>,
        current_remaining: Option<f64>,
    ) -> SessionQuotaTransition {
        let (Some(previous), Some(current)) = (previous_remaining, current_remaining) else {
            return SessionQuotaTransition::None;
        };
        let was_depleted = previous <= Self::DEPLETED_THRESHOLD;
        let is_depleted = current <= Self::DEPLETED_THRESHOLD;
        if !was_depleted && is_depleted {
            SessionQuotaTransition::Depleted
        } else if was_depleted && !is_depleted {
            SessionQuotaTransition::Restored
        } else {
            SessionQuotaTransition::None
        }
    }

    pub fn notification_copy(
        transition: SessionQuotaTransition,
        provider_name: &str,
    ) -> (String, String) {
        match transition {
            SessionQuotaTransition::None => (String::new(), String::new()),
            SessionQuotaTransition::Depleted => (
                format!("{provider_name} session depleted"),
                "0% left. Will notify when it's available again.".to_owned(),
            ),
            SessionQuotaTransition::Restored => (
                format!("{provider_name} session restored"),
                "Session quota is available again.".to_owned(),
            ),
        }
    }

    pub fn notification_id_prefix(
        provider: &ProviderId,
        transition: SessionQuotaTransition,
    ) -> String {
        let transition_str = match transition {
            SessionQuotaTransition::None => "none",
            SessionQuotaTransition::Depleted => "depleted",
            SessionQuotaTransition::Restored => "restored",
        };
        format!("session-{}-{transition_str}", provider.as_str())
    }
}

/// Pure reducer for session quota transitions, including Codex false-restore suppression.
pub struct SessionQuotaTransitionReducer;

impl SessionQuotaTransitionReducer {
    pub fn evaluate(
        previous: Option<&SessionQuotaTransitionState>,
        observation: &SessionQuotaTransitionObservation,
        notifications_enabled: bool,
        force_baseline: bool,
    ) -> SessionQuotaTransitionEvaluation {
        if force_baseline {
            return SessionQuotaTransitionEvaluation {
                outcome: SessionQuotaTransitionOutcome::BaselineChanged,
                state: Self::baseline_state(observation),
            };
        }
        let Some(previous) = previous else {
            // Note: in CodexBar, when previous is nil and forceBaseline is false, startup depletion
            // is recorded as `.depleted` by the reducer if `notificationsEnabled && isDepleted`,
            // while hook/edge-trigger policy requires a previous sample for edge transitions unless
            // startup depletion is explicitly evaluated by `SessionQuotaTransitionReducer`.
            let outcome = if notifications_enabled
                && SessionQuotaNotificationLogic::is_depleted(Some(observation.remaining))
            {
                SessionQuotaTransitionOutcome::Depleted
            } else {
                SessionQuotaTransitionOutcome::None
            };
            return SessionQuotaTransitionEvaluation {
                outcome,
                state: Self::baseline_state(observation),
            };
        };

        let owner_changed = previous.account_discriminator != observation.account_discriminator;
        if previous.source != observation.source || owner_changed {
            return SessionQuotaTransitionEvaluation {
                outcome: SessionQuotaTransitionOutcome::BaselineChanged,
                state: Self::baseline_state(observation),
            };
        }

        let is_codex = observation.provider.as_str() == "codex";
        if is_codex && observation.observed_at <= previous.observed_at {
            return SessionQuotaTransitionEvaluation {
                outcome: SessionQuotaTransitionOutcome::StaleCodexObservation,
                state: previous.clone(),
            };
        }

        if !notifications_enabled {
            return SessionQuotaTransitionEvaluation {
                outcome: SessionQuotaTransitionOutcome::None,
                state: Self::updated_state(previous, observation, false),
            };
        }

        let transition = SessionQuotaNotificationLogic::transition(
            Some(previous.remaining),
            Some(observation.remaining),
        );
        if transition != SessionQuotaTransition::Restored || !is_codex {
            let outcome = match transition {
                SessionQuotaTransition::None => SessionQuotaTransitionOutcome::None,
                SessionQuotaTransition::Depleted => SessionQuotaTransitionOutcome::Depleted,
                SessionQuotaTransition::Restored => SessionQuotaTransitionOutcome::Restored,
            };
            let preserve_depleted_boundary = is_codex
                && previous.trusted_reset_boundary.is_some()
                && SessionQuotaNotificationLogic::is_depleted(Some(previous.remaining))
                && SessionQuotaNotificationLogic::is_depleted(Some(observation.remaining));
            let preserve_codex_boundary = preserve_depleted_boundary
                || (is_codex
                    && previous.trusted_reset_boundary.is_some_and(|boundary| {
                        observation.evaluation_time < boundary || observation.observed_at < boundary
                    }));
            return SessionQuotaTransitionEvaluation {
                outcome,
                state: Self::updated_state(previous, observation, preserve_codex_boundary),
            };
        }

        if let Some(trusted_reset_boundary) = previous.trusted_reset_boundary {
            if observation.evaluation_time < trusted_reset_boundary
                || observation.observed_at < trusted_reset_boundary
            {
                return SessionQuotaTransitionEvaluation {
                    outcome: SessionQuotaTransitionOutcome::SuppressedCodexRestore,
                    state: Self::preserved_depleted_state(previous, observation, None),
                };
            }

            if let Some(reset_boundary) = Self::valid_reset_boundary(
                observation.reset_boundary,
                observation.observed_at,
                observation.evaluation_time,
            ) && !are_equivalent_plan_utilization_reset_boundaries(
                Some(trusted_reset_boundary),
                Some(reset_boundary),
            ) && reset_boundary > trusted_reset_boundary
            {
                return SessionQuotaTransitionEvaluation {
                    outcome: SessionQuotaTransitionOutcome::Restored,
                    state: Self::updated_state(previous, observation, false),
                };
            }
        }

        if let Some(pending) = previous.pending_codex_restore_observation_at
            && observation.observed_at > pending
        {
            return SessionQuotaTransitionEvaluation {
                outcome: SessionQuotaTransitionOutcome::Restored,
                state: Self::updated_state(previous, observation, false),
            };
        }

        SessionQuotaTransitionEvaluation {
            outcome: SessionQuotaTransitionOutcome::AwaitingCodexRestoreConfirmation,
            state: Self::preserved_depleted_state(
                previous,
                observation,
                Some(observation.observed_at),
            ),
        }
    }

    fn baseline_state(
        observation: &SessionQuotaTransitionObservation,
    ) -> SessionQuotaTransitionState {
        let is_codex = observation.provider.as_str() == "codex";
        SessionQuotaTransitionState {
            remaining: observation.remaining,
            source: observation.source,
            observed_at: observation.observed_at,
            account_discriminator: observation.account_discriminator.clone(),
            trusted_reset_boundary: if is_codex {
                Self::valid_reset_boundary(
                    observation.reset_boundary,
                    observation.observed_at,
                    observation.evaluation_time,
                )
            } else {
                None
            },
            pending_codex_restore_observation_at: None,
        }
    }

    fn updated_state(
        previous: &SessionQuotaTransitionState,
        observation: &SessionQuotaTransitionObservation,
        preserve_codex_reset_boundary: bool,
    ) -> SessionQuotaTransitionState {
        let is_codex = observation.provider.as_str() == "codex";
        let trusted_reset_boundary = if !is_codex {
            None
        } else if preserve_codex_reset_boundary {
            previous.trusted_reset_boundary
        } else {
            Self::monotonic_reset_boundary(
                previous.trusted_reset_boundary,
                Self::valid_reset_boundary(
                    observation.reset_boundary,
                    observation.observed_at,
                    observation.evaluation_time,
                ),
            )
        };
        SessionQuotaTransitionState {
            remaining: observation.remaining,
            source: observation.source,
            observed_at: observation.observed_at,
            account_discriminator: observation.account_discriminator.clone(),
            trusted_reset_boundary,
            pending_codex_restore_observation_at: None,
        }
    }

    fn preserved_depleted_state(
        previous: &SessionQuotaTransitionState,
        observation: &SessionQuotaTransitionObservation,
        pending_restore_observation_at: Option<Timestamp>,
    ) -> SessionQuotaTransitionState {
        SessionQuotaTransitionState {
            remaining: previous.remaining,
            source: observation.source,
            observed_at: observation.observed_at,
            account_discriminator: observation.account_discriminator.clone(),
            trusted_reset_boundary: previous.trusted_reset_boundary,
            pending_codex_restore_observation_at: pending_restore_observation_at,
        }
    }

    fn monotonic_reset_boundary(
        previous: Option<Timestamp>,
        current: Option<Timestamp>,
    ) -> Option<Timestamp> {
        let Some(previous) = previous else {
            return current;
        };
        if limit_reset_boundary_advanced(Some(previous), current, false) {
            current
        } else {
            Some(previous)
        }
    }

    fn valid_reset_boundary(
        candidate: Option<Timestamp>,
        observed_at: Timestamp,
        evaluation_time: Timestamp,
    ) -> Option<Timestamp> {
        candidate.filter(|&c| c > observed_at && c > evaluation_time)
    }
}

/// Returns whether a rate window qualifies as a session window (`<= 6 hours` or unspecified).
pub fn is_session_window(window: &RateWindow) -> bool {
    window
        .window_minutes
        .is_none_or(|minutes| minutes <= 6 * 60)
}

/// Returns whether the snapshot carries Antigravity quota-summary extra rate windows.
pub fn has_antigravity_quota_summary_windows(snapshot: &UsageSnapshot) -> bool {
    snapshot.extra_rate_windows.as_ref().is_some_and(|windows| {
        windows
            .iter()
            .any(|w| w.id.starts_with(ANTIGRAVITY_QUOTA_SUMMARY_WINDOW_ID_PREFIX))
    })
}

/// Selects the highest-utilization Antigravity window for the requested duration (`window_minutes`).
pub fn antigravity_window(snapshot: &UsageSnapshot, window_minutes: i64) -> Option<RateWindow> {
    let windows: Vec<RateWindow> = if has_antigravity_quota_summary_windows(snapshot) {
        snapshot
            .extra_rate_windows
            .as_ref()
            .map(|extra| {
                extra
                    .iter()
                    .filter(|w| {
                        w.usage_known
                            && w.id.starts_with(ANTIGRAVITY_QUOTA_SUMMARY_WINDOW_ID_PREFIX)
                            && w.window.window_minutes == Some(window_minutes)
                    })
                    .map(|w| w.window.clone())
                    .collect()
            })
            .unwrap_or_default()
    } else {
        [
            snapshot.primary.as_ref(),
            snapshot.secondary.as_ref(),
            snapshot.tertiary.as_ref(),
        ]
        .into_iter()
        .flatten()
        .filter(|w| {
            w.window_minutes == Some(window_minutes)
                || (window_minutes == 5 * 60 && w.window_minutes.is_none())
        })
        .cloned()
        .collect()
    };
    windows
        .into_iter()
        .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
}

/// Selects the session quota window and its source for a provider snapshot.
pub fn session_quota_window(
    provider: &ProviderId,
    snapshot: &UsageSnapshot,
) -> Option<(RateWindow, SessionQuotaWindowSource)> {
    let id = provider.as_str();
    if id == "mimo" || id == "qoder" {
        return None;
    }
    if id == "antigravity" {
        let window = antigravity_window(snapshot, 5 * 60)?;
        let source = if has_antigravity_quota_summary_windows(snapshot) {
            SessionQuotaWindowSource::AntigravityQuotaSummary
        } else {
            SessionQuotaWindowSource::AntigravityLegacy
        };
        return Some((window, source));
    }
    if let Some(primary) = snapshot.primary.as_ref()
        && is_session_window(primary)
    {
        return Some((primary.clone(), SessionQuotaWindowSource::Primary));
    }
    if id == "copilot"
        && let Some(secondary) = snapshot.secondary.as_ref()
    {
        return Some((
            secondary.clone(),
            SessionQuotaWindowSource::CopilotSecondaryFallback,
        ));
    }
    None
}

/// Codex identity classification for owner-scoped notification keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexIdentity {
    ProviderAccount { id: String },
    EmailOnly { normalized_email: String },
    Unresolved,
}

/// Codex active credential source for owner-scoped notification keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexActiveSource {
    LiveSystem,
    ManagedAccount { id: String },
    ProfileHome { path: String },
}

/// Account-scoped refresh guard used to derive Codex session and limit-reset owner keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexAccountScopedRefreshGuard {
    pub source: CodexActiveSource,
    pub identity: CodexIdentity,
    pub account_key: Option<String>,
    pub auth_fingerprint: Option<String>,
}

impl CodexAccountScopedRefreshGuard {
    pub fn new(
        source: CodexActiveSource,
        identity: CodexIdentity,
        account_key: Option<String>,
        auth_fingerprint: Option<String>,
    ) -> Self {
        Self {
            source,
            identity,
            account_key,
            auth_fingerprint,
        }
    }
}

fn normalize_workspace_account_id(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn normalize_email(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn normalize_fingerprint(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn normalize_home_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn sha256_hex(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Opaque SHA-256 owner key for Codex limit-reset notifications (`codex-limit-reset-owner:v2`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CodexLimitResetOwnerKey {
    pub raw_value: String,
}

impl CodexLimitResetOwnerKey {
    pub fn new(identity: &CodexIdentity, account_email: Option<&str>) -> Option<Self> {
        let CodexIdentity::ProviderAccount { id } = identity else {
            return None;
        };
        let normalized_id = normalize_workspace_account_id(Some(id))?;
        let normalized_email = normalize_email(account_email)?;
        let input = format!("codex-limit-reset-owner:v2\0{normalized_id}\0{normalized_email}");
        Some(Self {
            raw_value: sha256_hex(&input),
        })
    }

    pub fn from_refresh_guard(guard: &CodexAccountScopedRefreshGuard) -> Option<Self> {
        Self::new(&guard.identity, guard.account_key.as_deref())
    }
}

/// Opaque SHA-256 owner key for Codex session quota notifications (`codex-session-quota-owner:v1`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CodexSessionQuotaOwnerKey {
    pub raw_value: String,
}

impl CodexSessionQuotaOwnerKey {
    pub fn from_refresh_guard(refresh_guard: &CodexAccountScopedRefreshGuard) -> Option<Self> {
        let input = match &refresh_guard.identity {
            CodexIdentity::ProviderAccount { id } => {
                let normalized_id = normalize_workspace_account_id(Some(id))?;
                let normalized_email = normalize_email(refresh_guard.account_key.as_deref())?;
                format!(
                    "codex-session-quota-owner:v1\0provider\0{normalized_id}\0{normalized_email}"
                )
            }
            CodexIdentity::EmailOnly { normalized_email } => {
                let email = normalize_email(Some(normalized_email))?;
                if let Some(account_key) = normalize_email(refresh_guard.account_key.as_deref())
                    && account_key != email
                {
                    return None;
                }
                let source_key = Self::source_key(&refresh_guard.source)?;
                let fingerprint = normalize_fingerprint(refresh_guard.auth_fingerprint.as_deref())?;
                format!("codex-session-quota-owner:v1\0email\0{source_key}\0{email}\0{fingerprint}")
            }
            CodexIdentity::Unresolved => return None,
        };
        Some(Self {
            raw_value: sha256_hex(&input),
        })
    }

    fn source_key(source: &CodexActiveSource) -> Option<String> {
        match source {
            CodexActiveSource::LiveSystem => Some("live-system".to_owned()),
            CodexActiveSource::ManagedAccount { id } => {
                let trimmed = id.trim().to_ascii_lowercase();
                (!trimmed.is_empty()).then(|| format!("managed:{trimmed}"))
            }
            CodexActiveSource::ProfileHome { path } => {
                normalize_home_path(path).map(|p| format!("profile:{p}"))
            }
        }
    }
}

/// Event emitted when a provider's remaining quota crosses a configured warning threshold.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaWarningEvent {
    pub window: QuotaWarningWindow,
    pub threshold: u32,
    pub current_remaining: f64,
    pub window_id: Option<String>,
    pub window_display_label: Option<String>,
}

impl QuotaWarningEvent {
    pub fn new(
        window: QuotaWarningWindow,
        threshold: u32,
        current_remaining: f64,
        window_id: Option<String>,
        window_display_label: Option<String>,
    ) -> Self {
        Self {
            window,
            threshold,
            current_remaining,
            window_id,
            window_display_label,
        }
    }
}

/// Pure logic for quota threshold warnings and notification copy.
pub struct QuotaWarningNotificationLogic;

impl QuotaWarningNotificationLogic {
    pub fn active_thresholds(raw: &[u32]) -> Vec<u32> {
        QuotaWarnings::sanitize_thresholds(raw)
            .into_iter()
            .filter(|&t| t > 0)
            .collect()
    }

    pub fn notification_id_prefix(provider: &ProviderId, event: &QuotaWarningEvent) -> String {
        let window_segment = event
            .window_id
            .as_deref()
            .map(|id| format!("-{id}"))
            .unwrap_or_default();
        format!(
            "quota-warning-{}-{}{}-{}",
            provider.as_str(),
            event.window.display_name(),
            window_segment,
            event.threshold
        )
    }

    /// Formats the title and body for a quota warning notification.
    ///
    /// Security invariant: never includes an email or account identifier in notification text.
    pub fn notification_copy(
        provider_name: &str,
        window: QuotaWarningWindow,
        threshold: u32,
        current_remaining: f64,
        window_display_label: Option<&str>,
    ) -> (String, String) {
        let window_label = window_display_label.unwrap_or_else(|| window.display_name());
        let remaining_text = Self::percent_text(current_remaining);
        let title = format!("{provider_name} {window_label} quota low");
        let body = format!(
            "{remaining_text} left. Reached your {threshold}% {window_label} warning threshold."
        );
        (title, body)
    }

    pub fn crossed_threshold(
        previous_remaining: Option<f64>,
        current_remaining: f64,
        thresholds: &[u32],
        already_fired: &BTreeSet<u32>,
    ) -> Option<u32> {
        let sanitized = Self::active_thresholds(thresholds);
        let eligible: Vec<u32> = sanitized
            .into_iter()
            .filter(|&threshold| {
                current_remaining <= f64::from(threshold) && !already_fired.contains(&threshold)
            })
            .collect();
        if eligible.is_empty() {
            return None;
        }
        if let Some(previous_remaining) = previous_remaining {
            return eligible
                .into_iter()
                .filter(|&threshold| previous_remaining > f64::from(threshold))
                .min();
        }
        eligible.into_iter().min()
    }

    pub fn fired_thresholds_after_warning(threshold: u32, thresholds: &[u32]) -> BTreeSet<u32> {
        Self::active_thresholds(thresholds)
            .into_iter()
            .filter(|&t| t >= threshold)
            .collect()
    }

    pub fn thresholds_to_clear(
        current_remaining: f64,
        already_fired: &BTreeSet<u32>,
    ) -> BTreeSet<u32> {
        already_fired
            .iter()
            .copied()
            .filter(|&threshold| current_remaining > f64::from(threshold))
            .collect()
    }

    fn percent_text(value: f64) -> String {
        format!("{}%", value.clamp(0.0, 100.0).round() as i64)
    }
}

/// Key scoping quota warning threshold state by provider, window, account, and extra window ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWarningStateKey {
    pub provider: ProviderId,
    pub window: QuotaWarningWindow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_discriminator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<String>,
}

/// Tracked threshold state for a single quota warning key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWarningState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_remaining: Option<f64>,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub observed_at: Timestamp,
    #[serde(default)]
    pub fired_thresholds: BTreeSet<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SessionQuotaWindowSource>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub resets_at: Option<Timestamp>,
    #[serde(default)]
    pub shared_with_unresolved_account: bool,
}

impl Default for QuotaWarningState {
    fn default() -> Self {
        Self {
            last_remaining: None,
            observed_at: Timestamp::MIN,
            fired_thresholds: BTreeSet::new(),
            source: None,
            resets_at: None,
            shared_with_unresolved_account: false,
        }
    }
}

/// Observation of the active Claude OAuth/CLI account during a refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeOAuthActiveAccountObservation {
    Stable { identity: Option<String> },
    Changed,
}

/// Pure logic for limit-reset notifications.
pub struct LimitResetNotificationLogic;

impl LimitResetNotificationLogic {
    /// Formats the title and body for a limit-reset notification.
    ///
    /// Security invariant: never includes an email or account identifier in notification text.
    pub fn notification_copy(provider_name: &str, window: QuotaWarningWindow) -> (String, String) {
        let window_label = window.display_name();
        let title = format!("{provider_name} {window_label} limit reset");
        let body = "Fresh quota is available.".to_owned();
        (title, body)
    }

    pub fn notification_id_prefix(provider: &ProviderId, window: QuotaWarningWindow) -> String {
        format!(
            "limit-reset-{}-{}",
            provider.as_str(),
            window.display_name()
        )
    }
}

/// Receipt remembering that the current usage cycle's reset or restore notice was already sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitResetNotificationReceipt {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub reset_boundary: Option<Timestamp>,
}

/// Arbitrated notice outcome between a session-restored banner and a limit-reset banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitResetNotice {
    Restored,
    Reset,
}

/// Persisted detector state for limit-reset detection and notification deduplication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitResetDetectorState {
    pub was_above_threshold: bool,
    #[serde(default)]
    pub was_above_codex_weekly_candidate_threshold: bool,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    pub last_observed_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_raw_value: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub reset_boundary: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_above_threshold_count: Option<usize>,
    #[serde(default)]
    pub pending_low_confirmation: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub pending_low_observed_at: Option<Timestamp>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub last_posted_reset_boundary: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_receipt: Option<LimitResetNotificationReceipt>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub last_notified_reset_boundary: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_raw_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_weekly_evidence_version: Option<u32>,
}

impl LimitResetDetectorState {
    pub const CURRENT_CODEX_WEEKLY_EVIDENCE_VERSION: u32 = 2;

    pub fn new(
        was_above_threshold: bool,
        last_observed_at: Timestamp,
        source_raw_value: Option<String>,
    ) -> Self {
        Self {
            was_above_threshold,
            was_above_codex_weekly_candidate_threshold: false,
            last_observed_at,
            source_raw_value,
            reset_boundary: None,
            recovery_above_threshold_count: None,
            pending_low_confirmation: false,
            pending_low_observed_at: None,
            last_posted_reset_boundary: None,
            notification_receipt: None,
            last_notified_reset_boundary: None,
            plan_raw_value: None,
            codex_weekly_evidence_version: Some(Self::CURRENT_CODEX_WEEKLY_EVIDENCE_VERSION),
        }
    }
}

/// Input sample for limit-reset detection.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitResetObservation {
    pub used_percent: f64,
    pub observed_at: Timestamp,
    pub reset_boundary: Option<Timestamp>,
    pub source: Option<SessionQuotaWindowSource>,
}

/// Arbitrates whether a limit-reset or session-restored notice should be emitted for `state`.
pub fn prepare_limit_reset_notification(
    state: &mut LimitResetDetectorState,
    previous_state: Option<&LimitResetDetectorState>,
    observation: &LimitResetObservation,
    reset_confirmed: bool,
    restored: bool,
    session_quota_notifications_enabled: bool,
    limit_reset_notifications_enabled: bool,
) -> Option<LimitResetNotice> {
    let new_usage_cycle =
        state.was_above_threshold && previous_state.is_some_and(|prev| !prev.was_above_threshold);
    let restored_already_covered = state.notification_receipt.as_ref().is_some_and(|receipt| {
        !limit_reset_boundary_advanced(receipt.reset_boundary, observation.reset_boundary, true)
    });
    let restored_notice_enabled =
        restored && session_quota_notifications_enabled && !restored_already_covered;
    if new_usage_cycle
        || SessionQuotaNotificationLogic::is_depleted(Some(100.0 - observation.used_percent))
    {
        state.notification_receipt = None;
    }
    if let Some(boundary) = observation.reset_boundary {
        let advanced = limit_reset_boundary_advanced(
            state.last_notified_reset_boundary,
            Some(boundary),
            false,
        );
        if let Some(receipt) = state.notification_receipt.as_ref()
            && receipt.reset_boundary.is_none()
        {
            if advanced {
                state.last_notified_reset_boundary = Some(boundary);
            }
            state.notification_receipt = Some(LimitResetNotificationReceipt {
                reset_boundary: Some(boundary),
            });
            return None;
        }
        if !advanced && !(restored_notice_enabled && state.notification_receipt.is_none()) {
            if reset_confirmed || restored {
                state.notification_receipt = Some(LimitResetNotificationReceipt {
                    reset_boundary: Some(boundary),
                });
            }
            return None;
        }
    } else if state.notification_receipt.is_some() {
        return None;
    }

    let notice = if restored_notice_enabled {
        LimitResetNotice::Restored
    } else if reset_confirmed && limit_reset_notifications_enabled {
        LimitResetNotice::Reset
    } else {
        return None;
    };

    state.notification_receipt = Some(LimitResetNotificationReceipt {
        reset_boundary: observation.reset_boundary,
    });
    if let Some(boundary) = observation.reset_boundary {
        state.last_notified_reset_boundary = Some(
            state
                .last_notified_reset_boundary
                .map_or(boundary, |prev| prev.max(boundary)),
        );
    }
    Some(notice)
}

#[derive(Debug, Clone)]
struct CodexWeeklyResetDecision {
    should_start_candidate: bool,
    retaining_candidate: bool,
    expired_candidate: bool,
    should_post: bool,
}

impl CodexWeeklyResetDecision {
    fn inactive() -> Self {
        Self {
            should_start_candidate: false,
            retaining_candidate: false,
            expired_candidate: false,
            should_post: false,
        }
    }
}

fn codex_weekly_reset_candidate_boundary_matches(
    lhs: Option<Timestamp>,
    rhs: Option<Timestamp>,
) -> bool {
    let (Some(lhs), Some(rhs)) = (lhs, rhs) else {
        return false;
    };
    let diff_secs = ((lhs.as_millisecond() - rhs.as_millisecond()).abs() as f64) / 1000.0;
    diff_secs < CODEX_WEEKLY_RESET_CANDIDATE_BOUNDARY_TOLERANCE_SECS
}

fn limit_reset_plan_identifier(snapshot: &UsageSnapshot) -> Option<String> {
    let codex_id = ProviderId::new("codex").expect("valid provider id");
    snapshot
        .identity_for(&codex_id)
        .or(snapshot.identity.as_ref())
        .and_then(|id| id.login_method.as_deref())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}

fn limit_reset_boundary_allows_post(
    provider: &ProviderId,
    window: QuotaWarningWindow,
    previous_state: Option<&LimitResetDetectorState>,
    observation: &LimitResetObservation,
) -> bool {
    if window == QuotaWarningWindow::Session {
        return limit_reset_boundary_advanced(
            previous_state.and_then(|s| s.reset_boundary),
            observation.reset_boundary,
            false,
        );
    }
    if provider.as_str() == "codex" && window == QuotaWarningWindow::Weekly {
        return limit_reset_boundary_advanced(
            previous_state.and_then(|s| s.reset_boundary),
            observation.reset_boundary,
            true,
        );
    }
    true
}

/// Evaluates a single limit-reset detector transition for `(provider, window)`.
pub fn evaluate_limit_reset_detector_transition(
    provider: &ProviderId,
    snapshot: &UsageSnapshot,
    window: QuotaWarningWindow,
    observation: &LimitResetObservation,
    previous_state: Option<&LimitResetDetectorState>,
    requires_low_confirmation: bool,
) -> (LimitResetDetectorState, bool) {
    let current_used = observation.used_percent;
    let current_observed_at = observation.observed_at;
    let was_above_threshold = current_used > LIMIT_RESET_THRESHOLD;
    let was_above_codex_weekly_candidate_threshold =
        current_used > CODEX_WEEKLY_RESET_CANDIDATE_BASELINE_THRESHOLD;
    let is_claude_weekly = provider.as_str() == "claude" && window == QuotaWarningWindow::Weekly;
    let is_codex_weekly = provider.as_str() == "codex" && window == QuotaWarningWindow::Weekly;
    let claude_weekly_recovery_pending = is_claude_weekly
        && previous_state.is_some_and(|s| s.recovery_above_threshold_count.is_some());
    let plan_raw_value = if is_codex_weekly {
        limit_reset_plan_identifier(snapshot)
    } else {
        None
    };
    let plan_changed = is_codex_weekly
        && previous_state.is_some_and(|s| s.plan_raw_value.is_some())
        && previous_state.and_then(|s| s.plan_raw_value.as_ref()) != plan_raw_value.as_ref();
    let plan_matches_previous = is_codex_weekly
        && plan_raw_value.is_some()
        && previous_state.and_then(|s| s.plan_raw_value.as_ref()) == plan_raw_value.as_ref();
    let source_raw_value = observation.source.map(|s| s.as_str().to_owned());
    let source_changed = window == QuotaWarningWindow::Session
        && previous_state.is_some_and(|s| s.source_raw_value.is_some())
        && previous_state.and_then(|s| s.source_raw_value.as_ref()) != source_raw_value.as_ref();
    let reset_boundary_allows_post =
        limit_reset_boundary_allows_post(provider, window, previous_state, observation);
    let crossed_below_threshold = !source_changed
        && previous_state.is_some_and(|s| s.was_above_threshold)
        && !was_above_threshold;
    let confirming_low_sample = !source_changed
        && previous_state.is_some_and(|s| s.pending_low_confirmation)
        && !was_above_threshold;

    let codex_decision = if is_codex_weekly {
        let boundary_matches_previous = codex_weekly_reset_candidate_boundary_matches(
            previous_state.and_then(|s| s.reset_boundary),
            observation.reset_boundary,
        );
        let boundary_already_posted = codex_weekly_reset_candidate_boundary_matches(
            previous_state.and_then(|s| s.last_posted_reset_boundary),
            observation.reset_boundary,
        );
        let pending_candidate_age = previous_state
            .and_then(|s| s.pending_low_observed_at)
            .map(|t| (current_observed_at.as_millisecond() - t.as_millisecond()) as f64 / 1000.0);
        let pending_candidate_compatible = previous_state
            .is_some_and(|s| s.pending_low_confirmation)
            && plan_matches_previous
            && boundary_matches_previous
            && !boundary_already_posted
            && current_used <= CODEX_WEEKLY_RESET_CANDIDATE_CONFIRMATION_THRESHOLD
            && observation
                .reset_boundary
                .is_some_and(|b| current_observed_at < b)
            && pending_candidate_age.is_some_and(|age| {
                (0.0..=CODEX_WEEKLY_RESET_CANDIDATE_MAXIMUM_AGE_SECS).contains(&age)
            });
        let confirming_candidate = pending_candidate_compatible
            && pending_candidate_age
                .is_some_and(|age| age >= CODEX_WEEKLY_RESET_CANDIDATE_MINIMUM_AGE_SECS);
        let retaining_candidate = pending_candidate_compatible && !confirming_candidate;
        let expired_candidate = previous_state.is_some_and(|s| s.pending_low_confirmation)
            && !confirming_candidate
            && !retaining_candidate
            && !reset_boundary_allows_post;
        let crossed_into_candidate_reset_range = !source_changed
            && previous_state.is_some_and(|s| s.was_above_codex_weekly_candidate_threshold)
            && current_used <= CODEX_WEEKLY_RESET_CANDIDATE_CONFIRMATION_THRESHOLD;
        let should_start_candidate = !previous_state.is_some_and(|s| s.pending_low_confirmation)
            && plan_matches_previous
            && crossed_into_candidate_reset_range
            && !reset_boundary_allows_post
            && boundary_matches_previous
            && !boundary_already_posted
            && observation
                .reset_boundary
                .is_some_and(|b| current_observed_at < b);
        let crossed_into_advanced_boundary_reset_range = !source_changed
            && previous_state.is_some_and(|s| s.was_above_threshold)
            && current_used <= CODEX_WEEKLY_ADVANCED_BOUNDARY_RESET_THRESHOLD;
        let should_post = !plan_changed
            && ((crossed_into_advanced_boundary_reset_range && reset_boundary_allows_post)
                || confirming_candidate);
        CodexWeeklyResetDecision {
            should_start_candidate,
            retaining_candidate,
            expired_candidate,
            should_post,
        }
    } else {
        CodexWeeklyResetDecision::inactive()
    };

    let should_post = if requires_low_confirmation {
        confirming_low_sample && !claude_weekly_recovery_pending
    } else if is_codex_weekly {
        codex_decision.should_post
    } else {
        crossed_below_threshold && reset_boundary_allows_post && !claude_weekly_recovery_pending
    };

    let suppressed_guarded_crossing = crossed_below_threshold && !reset_boundary_allows_post;
    let should_await_low_confirmation = requires_low_confirmation
        && crossed_below_threshold
        && !confirming_low_sample
        && reset_boundary_allows_post
        && !claude_weekly_recovery_pending;
    let should_preserve_boundary = !source_changed
        && !reset_boundary_allows_post
        && (window == QuotaWarningWindow::Session
            || previous_state.and_then(|s| s.reset_boundary).is_some());
    let should_preserve_baseline =
        suppressed_guarded_crossing && !plan_changed && !codex_decision.expired_candidate;
    let previous_recovery_count = previous_state
        .and_then(|s| s.recovery_above_threshold_count)
        .unwrap_or(0);
    let next_recovery_count = if claude_weekly_recovery_pending {
        if was_above_threshold {
            previous_recovery_count + 1
        } else {
            0
        }
    } else {
        0
    };
    let claude_weekly_recovery_confirmed = claude_weekly_recovery_pending
        && next_recovery_count >= CLAUDE_WEEKLY_RECOVERY_OBSERVATION_COUNT;
    let next_was_above_threshold = if should_post {
        false
    } else if claude_weekly_recovery_pending {
        claude_weekly_recovery_confirmed
    } else if codex_decision.expired_candidate || plan_changed {
        if is_codex_weekly {
            current_used > CODEX_WEEKLY_RESET_CANDIDATE_BASELINE_THRESHOLD
        } else {
            was_above_threshold
        }
    } else if should_preserve_baseline || should_await_low_confirmation {
        true
    } else {
        was_above_threshold
    };
    let next_was_above_codex_weekly_candidate_threshold = if is_codex_weekly {
        if should_post {
            false
        } else if should_preserve_baseline {
            previous_state.is_some_and(|s| s.was_above_codex_weekly_candidate_threshold)
        } else {
            was_above_codex_weekly_candidate_threshold
        }
    } else {
        false
    };
    let persisted_recovery_count = if should_post {
        Some(0)
    } else if claude_weekly_recovery_pending && !claude_weekly_recovery_confirmed {
        Some(next_recovery_count)
    } else {
        None
    };
    let pending_low_confirmation = should_await_low_confirmation
        || codex_decision.should_start_candidate
        || codex_decision.retaining_candidate;
    let pending_low_observed_at = if codex_decision.should_start_candidate {
        Some(current_observed_at)
    } else if codex_decision.retaining_candidate {
        previous_state.and_then(|s| s.pending_low_observed_at)
    } else {
        None
    };
    let last_posted_reset_boundary = if plan_changed {
        None
    } else if should_post {
        observation.reset_boundary
    } else {
        previous_state.and_then(|s| s.last_posted_reset_boundary)
    };

    let state = LimitResetDetectorState {
        was_above_threshold: next_was_above_threshold,
        was_above_codex_weekly_candidate_threshold: next_was_above_codex_weekly_candidate_threshold,
        last_observed_at: current_observed_at,
        source_raw_value,
        reset_boundary: if should_preserve_boundary {
            previous_state.and_then(|s| s.reset_boundary)
        } else {
            observation.reset_boundary
        },
        recovery_above_threshold_count: persisted_recovery_count,
        pending_low_confirmation,
        pending_low_observed_at,
        last_posted_reset_boundary,
        notification_receipt: previous_state.and_then(|s| s.notification_receipt.clone()),
        last_notified_reset_boundary: previous_state.and_then(|s| s.last_notified_reset_boundary),
        plan_raw_value,
        codex_weekly_evidence_version: Some(
            LimitResetDetectorState::CURRENT_CODEX_WEEKLY_EVIDENCE_VERSION,
        ),
    };
    (state, should_post)
}

/// Classification of provider fetch errors for credential-expiry notification gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFetchErrorKind {
    AuthenticationExpired,
    MissingCredential,
    RateLimited,
    Network,
    PermissionDenied,
    ParseFailed,
    Other,
}

/// Structured fetch failure examined by `ProviderCredentialFailure`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderCredentialError {
    Classified {
        kind: ProviderFetchErrorKind,
        message: String,
    },
    ClaudeOAuthRefreshFailed(String),
    ClaudeLegacyOAuthFailed(String),
    Unauthorized,
    TokenExpired,
    TokenRevoked,
    TokenReused,
    NativeRefreshRequired,
    ReadOnlySource,
    NotLoggedIn,
    SessionExpired,
    NoSessionCookie,
    KeychainAccessRevoked,
    Other(String),
}

/// Pure classifier determining whether a provider error represents an authentication failure.
///
/// Never infers authentication failure from an arbitrary message containing `"token"` or `"login"`.
pub struct ProviderCredentialFailure;

impl ProviderCredentialFailure {
    pub fn is_authentication_failure(error: &ProviderCredentialError) -> bool {
        match error {
            ProviderCredentialError::Classified { kind, .. } => matches!(
                kind,
                ProviderFetchErrorKind::AuthenticationExpired
                    | ProviderFetchErrorKind::MissingCredential
            ),
            ProviderCredentialError::Unauthorized
            | ProviderCredentialError::TokenExpired
            | ProviderCredentialError::TokenRevoked
            | ProviderCredentialError::TokenReused
            | ProviderCredentialError::NativeRefreshRequired
            | ProviderCredentialError::ReadOnlySource
            | ProviderCredentialError::NotLoggedIn
            | ProviderCredentialError::SessionExpired
            | ProviderCredentialError::NoSessionCookie => true,
            ProviderCredentialError::ClaudeOAuthRefreshFailed(message) => {
                Self::is_rejected_refresh(message)
            }
            ProviderCredentialError::ClaudeLegacyOAuthFailed(message) => (message
                .starts_with("Claude OAuth token refresh failed:")
                && Self::is_rejected_refresh(message))
                || message == "Claude OAuth request unauthorized."
                || message == "Claude OAuth credentials not found."
                || message == "Claude OAuth refresh token is missing."
                || message.starts_with("Claude OAuth token expired")
                || message.starts_with(
                    "Claude OAuth token is still unavailable after delegated Claude CLI refresh.",
                ),
            ProviderCredentialError::KeychainAccessRevoked | ProviderCredentialError::Other(_) => {
                false
            }
        }
    }

    fn is_rejected_refresh(message: &str) -> bool {
        let normalized = message.to_ascii_lowercase();
        normalized.contains("invalid_grant") || normalized.contains("invalid refresh token")
    }
}

/// Key scoping credential-expiry notification episodes by provider and account.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialNotificationKey {
    pub provider: ProviderId,
    pub account: String,
}

impl CredentialNotificationKey {
    pub fn new(provider: ProviderId, account: impl Into<String>) -> Self {
        Self {
            provider,
            account: account.into(),
        }
    }
}

/// Outcome of a provider fetch passed to `CredentialNotificationTracker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialFetchOutcome {
    Success {
        source_label: String,
        is_local_probe: bool,
        diagnostic: Option<String>,
    },
    Failure(ProviderCredentialError),
}

impl CredentialFetchOutcome {
    pub fn fresh_success(source_label: impl Into<String>) -> Self {
        Self::Success {
            source_label: source_label.into(),
            is_local_probe: false,
            diagnostic: None,
        }
    }
}

/// Action emitted by `CredentialNotificationTracker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialNotificationAction {
    None,
    Post(NotificationPost),
    Remove { identifier: String },
}

/// Pure logic for credential expiry notification copy and identifiers.
pub struct CredentialNotificationLogic;

impl CredentialNotificationLogic {
    pub fn notification_copy(provider_name: &str) -> (String, String) {
        (
            format!("{provider_name} needs sign-in"),
            "Open AriadUsage to review the account error and sign in again.".to_owned(),
        )
    }

    pub fn notification_id_prefix(provider: &ProviderId) -> String {
        format!("credential-expired-{}", provider.as_str())
    }

    pub fn notification_identifier(episode_id: u64) -> String {
        format!("ariadusage-credential-{episode_id}")
    }
}

/// Async-style delivery guard mirroring `AppNotifications.deliverIfCurrent`.
pub fn deliver_if_current<A, S, R, C>(
    mut authorize: A,
    mut submit: S,
    mut remove: R,
    mut is_current: C,
) where
    A: FnMut() -> bool,
    S: FnMut(),
    R: FnMut(),
    C: FnMut() -> bool,
{
    if !is_current() {
        return;
    }
    if !authorize() {
        return;
    }
    if !is_current() {
        return;
    }
    submit();
    if !is_current() {
        remove();
    }
}

/// State machine tracking credential-expiry notification episodes and Claude scope bindings.
#[derive(Debug, Clone, Default)]
pub struct CredentialNotificationTracker {
    pub episodes: HashMap<CredentialNotificationKey, u64>,
    pub claude_scopes: HashMap<String, String>,
    pub stopped: bool,
    next_episode_id: u64,
}

impl CredentialNotificationTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn claude_credential_notification_scope(
        &mut self,
        identity: Option<&str>,
        fingerprint: Option<&str>,
    ) -> String {
        let account = identity
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|id| format!("claude-account:{id}"));
        let credential = fingerprint
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|fp| format!("claude-credential:{fp}"));
        let prior = credential
            .as_ref()
            .and_then(|c| self.claude_scopes.get(c).cloned());
        let scope = if let Some(account) = account {
            let resolved = if let Some(established) = self.claude_scopes.get(&account) {
                established.clone()
            } else if let Some(prior_scope) = prior
                && !self
                    .claude_scopes
                    .iter()
                    .any(|(k, v)| k.starts_with("claude-account:") && v == &prior_scope)
            {
                prior_scope
            } else {
                account.clone()
            };
            self.claude_scopes.insert(account, resolved.clone());
            resolved
        } else {
            prior
                .or_else(|| credential.clone())
                .unwrap_or_else(|| "default".to_owned())
        };
        if let Some(credential) = credential {
            self.claude_scopes.insert(credential, scope.clone());
        }
        scope
    }

    pub fn handle_outcome(
        &mut self,
        provider: &ProviderId,
        account: &str,
        outcome: &CredentialFetchOutcome,
        credential_expiry_notifications_enabled: bool,
    ) -> CredentialNotificationAction {
        if self.stopped {
            return CredentialNotificationAction::None;
        }
        let key = CredentialNotificationKey::new(provider.clone(), account);
        match outcome {
            CredentialFetchOutcome::Success {
                source_label,
                is_local_probe,
                diagnostic,
            } => {
                if diagnostic.is_some()
                    || *is_local_probe
                    || source_label.to_ascii_lowercase().contains("cache")
                {
                    return CredentialNotificationAction::None;
                }
                if let Some(episode_id) = self.episodes.remove(&key) {
                    return CredentialNotificationAction::Remove {
                        identifier: CredentialNotificationLogic::notification_identifier(
                            episode_id,
                        ),
                    };
                }
                CredentialNotificationAction::None
            }
            CredentialFetchOutcome::Failure(error) => {
                if !credential_expiry_notifications_enabled
                    || !ProviderCredentialFailure::is_authentication_failure(error)
                    || self.episodes.contains_key(&key)
                {
                    return CredentialNotificationAction::None;
                }
                self.next_episode_id += 1;
                let episode_id = self.next_episode_id;
                self.episodes.insert(key, episode_id);
                let provider_name =
                    find_by_id(provider).map_or_else(|| provider.as_str(), |d| d.display_name);
                let (title, body) = CredentialNotificationLogic::notification_copy(provider_name);
                CredentialNotificationAction::Post(NotificationPost {
                    id_prefix: CredentialNotificationLogic::notification_id_prefix(provider),
                    identifier: Some(CredentialNotificationLogic::notification_identifier(
                        episode_id,
                    )),
                    title,
                    body,
                    sound_enabled: true,
                    on_screen_alert_enabled: false,
                })
            }
        }
    }

    pub fn complete_delivery(
        &mut self,
        provider: &ProviderId,
        account: &str,
        identifier: &str,
        delivered: bool,
    ) {
        if delivered {
            return;
        }
        let key = CredentialNotificationKey::new(provider.clone(), account);
        if let Some(&episode_id) = self.episodes.get(&key)
            && CredentialNotificationLogic::notification_identifier(episode_id) == identifier
        {
            self.episodes.remove(&key);
        }
    }

    pub fn retire_provider(&mut self, provider: Option<&ProviderId>) -> Vec<String> {
        let keys_to_remove: Vec<CredentialNotificationKey> = self
            .episodes
            .keys()
            .filter(|k| provider.is_none_or(|p| &k.provider == p))
            .cloned()
            .collect();
        let mut removed = Vec::with_capacity(keys_to_remove.len());
        for key in keys_to_remove {
            if let Some(episode_id) = self.episodes.remove(&key) {
                removed.push(CredentialNotificationLogic::notification_identifier(
                    episode_id,
                ));
            }
        }
        removed
    }

    pub fn retire_disabled_providers(
        &mut self,
        enabled_providers: &HashSet<ProviderId>,
    ) -> Vec<String> {
        let disabled: Vec<ProviderId> = self
            .episodes
            .keys()
            .map(|k| k.provider.clone())
            .filter(|p| !enabled_providers.contains(p))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let mut removed = Vec::new();
        for provider in disabled {
            removed.extend(self.retire_provider(Some(&provider)));
        }
        removed
    }
}

/// Summary item for a Codex rate-limit reset credit used by `CodexResetCreditExpiryNotifier`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexResetCreditItem {
    pub id: String,
    pub available: bool,
    pub expires_at: Option<Timestamp>,
}

impl CodexResetCreditItem {
    pub fn available(id: impl Into<String>, expires_at: Option<Timestamp>) -> Self {
        Self {
            id: id.into(),
            available: true,
            expires_at,
        }
    }
}

/// Notifier that emits a single bounded summary per unique expiring Codex reset-credit inventory.
#[derive(Debug, Clone, Default)]
pub struct CodexResetCreditExpiryNotifier {
    pub remembered_fingerprints: Vec<String>,
}

impl CodexResetCreditExpiryNotifier {
    pub const EXPIRY_WINDOW_SECS: i64 = 3 * 24 * 60 * 60;
    pub const NOTIFICATION_PREFIX: &'static str = "codex-reset-credit-expiry";
    pub const SUMMARY_FINGERPRINTS_KEY: &'static str = "codexResetCreditExpirySummaryFingerprints";
    pub const MAXIMUM_REMEMBERED_SUMMARIES: usize = 64;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_remembered_fingerprints(fingerprints: Vec<String>) -> Self {
        Self {
            remembered_fingerprints: fingerprints,
        }
    }

    pub fn post_expiring_credits_if_needed(
        &mut self,
        credits: &[CodexResetCreditItem],
        now: Timestamp,
    ) -> Option<NotificationPost> {
        let mut available: Vec<&CodexResetCreditItem> = credits
            .iter()
            .filter(|c| c.available && c.expires_at.is_none_or(|exp| exp > now))
            .collect();
        available.sort_by(|lhs, rhs| match (lhs.expires_at, rhs.expires_at) {
            (Some(l), Some(r)) if l != r => l.cmp(&r),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            _ => lhs.id.cmp(&rhs.id),
        });

        let expiring: Vec<&CodexResetCreditItem> = available
            .into_iter()
            .filter(|c| {
                c.expires_at.is_some_and(|exp| {
                    let secs = (exp.as_millisecond() - now.as_millisecond()) as f64 / 1000.0;
                    secs <= Self::EXPIRY_WINDOW_SECS as f64
                })
            })
            .collect();
        if expiring.is_empty() {
            return None;
        }

        let fingerprint = Self::summary_fingerprint(&expiring);
        if self.remembered_fingerprints.contains(&fingerprint) {
            return None;
        }
        self.remembered_fingerprints.push(fingerprint);
        if self.remembered_fingerprints.len() > Self::MAXIMUM_REMEMBERED_SUMMARIES {
            let overflow = self.remembered_fingerprints.len() - Self::MAXIMUM_REMEMBERED_SUMMARIES;
            self.remembered_fingerprints.drain(0..overflow);
        }

        let lines: Vec<String> = expiring
            .iter()
            .enumerate()
            .map(|(index, credit)| {
                let expiry_text = match credit.expires_at {
                    Some(exp) => {
                        let secs = (exp.as_millisecond() - now.as_millisecond()) as f64 / 1000.0;
                        let countdown = reset_countdown_description(secs);
                        format!("Expires {countdown}")
                    }
                    None => "No expiry".to_owned(),
                };
                format!("{}. {expiry_text}", index + 1)
            })
            .collect();

        Some(NotificationPost {
            id_prefix: Self::NOTIFICATION_PREFIX.to_owned(),
            identifier: None,
            title: "Limit Reset Credits".to_owned(),
            body: lines.join("\n"),
            sound_enabled: true,
            on_screen_alert_enabled: false,
        })
    }

    pub fn summary_fingerprint(credits: &[&CodexResetCreditItem]) -> String {
        let material = credits
            .iter()
            .map(|credit| {
                let secs = credit.expires_at.map_or(0_i64, |t| t.as_second());
                format!("{}\x1f{secs}", credit.id)
            })
            .collect::<Vec<_>>()
            .join("\x1e");
        sha256_hex(&material)
    }
}

fn reset_countdown_minutes(seconds: f64) -> Option<i64> {
    let minutes = (seconds / 60.0).ceil();
    if !minutes.is_finite() || minutes.abs() >= 9.0e18 {
        return None;
    }
    Some(if seconds < 1.0 {
        0
    } else {
        (minutes as i64).max(1)
    })
}

fn reset_countdown_description(seconds: f64) -> String {
    let Some(total_minutes) = reset_countdown_minutes(seconds) else {
        return "Unknown".to_owned();
    };
    if total_minutes == 0 {
        return "now".to_owned();
    }
    let days = total_minutes / (24 * 60);
    let hours = (total_minutes / 60) % 24;
    let minutes = total_minutes % 60;
    if days > 0 {
        if hours > 0 {
            return format!("in {days}d {hours}h");
        }
        if minutes > 0 {
            return format!("in {days}d {minutes}m");
        }
        return format!("in {days}d");
    }
    if hours > 0 {
        if minutes > 0 {
            return format!("in {hours}h {minutes}m");
        }
        return format!("in {hours}h");
    }
    format!("in {total_minutes}m")
}

/// JSON-serializable snapshot of edge-triggered notification state persisted in
/// `$XDG_STATE_HOME/ariadusage/notifications-state.json` (mode `0600`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPersistedState {
    #[serde(default)]
    pub session_quota_states: BTreeMap<ProviderId, SessionQuotaTransitionState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_session_quota_baseline_requirement: Option<CodexSessionQuotaBaselineRequirement>,
    #[serde(default)]
    pub quota_warning_states: Vec<(QuotaWarningStateKey, QuotaWarningState)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_claude_quota_warning_account: Option<String>,
    #[serde(default)]
    pub claude_oauth_account_uuid_map: HashMap<String, String>,
    #[serde(default)]
    pub session_limit_reset_states: HashMap<String, LimitResetDetectorState>,
    #[serde(default)]
    pub weekly_limit_reset_states: HashMap<String, LimitResetDetectorState>,
    #[serde(default)]
    pub codex_reset_credit_fingerprints: Vec<String>,
}

/// Stateful notification policy engine coordinating session quota, quota warning, limit reset,
/// credential expiry, and Codex reset-credit expiry notifications across providers and accounts.
#[derive(Debug, Clone, Default)]
pub struct NotificationPolicyEngine {
    pub preferences: NotificationPreferences,
    pub session_quota_states: HashMap<ProviderId, SessionQuotaTransitionState>,
    pub codex_session_quota_baseline_requirement: Option<CodexSessionQuotaBaselineRequirement>,
    pub quota_warning_states: HashMap<QuotaWarningStateKey, QuotaWarningState>,
    pub last_claude_quota_warning_account: Option<String>,
    pub claude_oauth_account_uuid_map: HashMap<String, String>,
    pub session_limit_reset_states: HashMap<String, LimitResetDetectorState>,
    pub weekly_limit_reset_states: HashMap<String, LimitResetDetectorState>,
    pub credential_tracker: CredentialNotificationTracker,
    pub codex_reset_credit_notifier: CodexResetCreditExpiryNotifier,
}

impl NotificationPolicyEngine {
    pub fn new(preferences: NotificationPreferences) -> Self {
        Self {
            preferences,
            ..Self::default()
        }
    }

    pub fn export_persisted_state(&self) -> NotificationPersistedState {
        let mut quota_warning_states: Vec<(QuotaWarningStateKey, QuotaWarningState)> = self
            .quota_warning_states
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        quota_warning_states.sort_by(|(a, _), (b, _)| {
            (
                a.provider.as_str(),
                a.window.display_name(),
                a.account_discriminator.as_deref(),
                a.window_id.as_deref(),
            )
                .cmp(&(
                    b.provider.as_str(),
                    b.window.display_name(),
                    b.account_discriminator.as_deref(),
                    b.window_id.as_deref(),
                ))
        });
        NotificationPersistedState {
            session_quota_states: self
                .session_quota_states
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            codex_session_quota_baseline_requirement: self
                .codex_session_quota_baseline_requirement
                .clone(),
            quota_warning_states,
            last_claude_quota_warning_account: self.last_claude_quota_warning_account.clone(),
            claude_oauth_account_uuid_map: self.claude_oauth_account_uuid_map.clone(),
            session_limit_reset_states: self.session_limit_reset_states.clone(),
            weekly_limit_reset_states: self.weekly_limit_reset_states.clone(),
            codex_reset_credit_fingerprints: self
                .codex_reset_credit_notifier
                .remembered_fingerprints
                .clone(),
        }
    }

    pub fn restore_persisted_state(&mut self, persisted: NotificationPersistedState) {
        self.session_quota_states = persisted.session_quota_states.into_iter().collect();
        self.codex_session_quota_baseline_requirement =
            persisted.codex_session_quota_baseline_requirement;
        self.quota_warning_states = persisted.quota_warning_states.into_iter().collect();
        self.last_claude_quota_warning_account = persisted.last_claude_quota_warning_account;
        self.claude_oauth_account_uuid_map = persisted.claude_oauth_account_uuid_map;
        self.session_limit_reset_states = persisted.session_limit_reset_states;
        self.weekly_limit_reset_states = persisted.weekly_limit_reset_states;
        self.codex_reset_credit_notifier.remembered_fingerprints =
            persisted.codex_reset_credit_fingerprints;
    }

    pub fn codex_session_quota_baseline_required(&self) -> bool {
        self.codex_session_quota_baseline_requirement.is_some()
    }

    pub fn clear_session_quota_transition_state(&mut self, provider: &ProviderId) {
        let removed = self.session_quota_states.remove(provider);
        if provider.as_str() == "codex"
            && let Some(removed_state) = removed
            && SessionQuotaNotificationLogic::is_depleted(Some(removed_state.remaining))
        {
            self.update_codex_session_quota_baseline_requirement(Some(removed_state.observed_at));
        }
    }

    pub fn require_fresh_codex_session_quota_baseline(&mut self, observed_at: Option<Timestamp>) {
        let codex_id = ProviderId::new("codex").expect("valid provider id");
        let removed = self.session_quota_states.remove(&codex_id);
        self.update_codex_session_quota_baseline_requirement(removed.map(|s| s.observed_at));
        self.update_codex_session_quota_baseline_requirement(observed_at);
    }

    pub fn clear_disabled_provider_state(&mut self, enabled_providers: &HashSet<ProviderId>) {
        let disabled: Vec<ProviderId> = self
            .session_quota_states
            .keys()
            .filter(|p| !enabled_providers.contains(*p))
            .cloned()
            .collect();
        for provider in disabled {
            self.clear_session_quota_transition_state(&provider);
        }
    }

    pub fn reset_provider_for_account_switch(&mut self, provider: &ProviderId) {
        self.clear_session_quota_transition_state(provider);
        self.quota_warning_states
            .retain(|k, _| &k.provider != provider);
        if provider.as_str() == "claude" {
            self.last_claude_quota_warning_account = None;
        }
        let prefix = format!("{}:", provider.as_str());
        self.session_limit_reset_states
            .retain(|k, _| !k.starts_with(&prefix));
        self.weekly_limit_reset_states
            .retain(|k, _| !k.starts_with(&prefix));
        let _ = self.credential_tracker.retire_provider(Some(provider));
    }

    fn update_codex_session_quota_baseline_requirement(&mut self, observed_at: Option<Timestamp>) {
        let requirement = self
            .codex_session_quota_baseline_requirement
            .clone()
            .unwrap_or_default();
        self.codex_session_quota_baseline_requirement = Some(requirement.merging(observed_at));
    }

    pub fn persist_claude_oauth_account_uuid_map(&mut self, map: HashMap<String, String>) {
        self.claude_oauth_account_uuid_map = map;
    }

    pub fn warning_claude_account_discriminators(
        &mut self,
        strategy_kind: &str,
        observation: &ClaudeOAuthActiveAccountObservation,
        oauth_history_owner_identifier: Option<&str>,
    ) -> (Option<String>, Option<String>) {
        if strategy_kind != "oauth" && strategy_kind != "cli" {
            return (None, None);
        }
        let identity = match observation {
            ClaudeOAuthActiveAccountObservation::Stable { identity } => {
                identity.as_deref().map(str::trim).filter(|s| !s.is_empty())
            }
            ClaudeOAuthActiveAccountObservation::Changed => None,
        };
        let unknown_account = "claude-account:unknown";
        let mut account = identity.map(|id| format!("claude-account:{id}"));
        let mut source = account
            .clone()
            .unwrap_or_else(|| unknown_account.to_owned());
        if strategy_kind == "oauth"
            && let Some(owner) = oauth_history_owner_identifier
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
        {
            let owner_key = format!("claude-oauth-owner:{owner}");
            source = account.clone().unwrap_or_else(|| owner_key.clone());
            if let Some(bound_identity) = self.claude_oauth_account_uuid_map.get(&owner).cloned() {
                let bound_account = format!("claude-account:{bound_identity}");
                if account.is_some() && account.as_deref() != Some(bound_account.as_str()) {
                    return (None, None);
                }
                self.reconcile_claude_quota_warning_owner(&owner_key, &bound_account);
                account = Some(bound_account);
            }
        }
        if let Some(ref acc) = account
            && acc != unknown_account
        {
            self.reconcile_claude_quota_warning_owner(unknown_account, acc);
            self.last_claude_quota_warning_account = Some(acc.clone());
        }
        let quota = account.unwrap_or_else(|| source.clone());
        (Some(quota), Some(source))
    }

    fn reconcile_claude_quota_warning_owner(&mut self, owner: &str, account: &str) {
        let matching_keys: Vec<QuotaWarningStateKey> = self
            .quota_warning_states
            .keys()
            .filter(|k| {
                k.provider.as_str() == "claude" && k.account_discriminator.as_deref() == Some(owner)
            })
            .cloned()
            .collect();
        for key in matching_keys {
            let Some(prior) = self.quota_warning_states.remove(&key) else {
                continue;
            };
            let account_key = QuotaWarningStateKey {
                provider: key.provider.clone(),
                window: key.window,
                account_discriminator: Some(account.to_owned()),
                window_id: key.window_id.clone(),
            };
            let existing_observed_at = self
                .quota_warning_states
                .get(&account_key)
                .map_or(Timestamp::MIN, |s| s.observed_at);
            if prior.observed_at >= existing_observed_at {
                self.quota_warning_states.insert(account_key.clone(), prior);
            }
            if owner == "claude-account:unknown"
                && let Some(entry) = self.quota_warning_states.get_mut(&account_key)
            {
                entry.shared_with_unresolved_account = true;
            }
        }
    }

    pub fn handle_quota_warning_transitions(
        &mut self,
        provider: &ProviderId,
        snapshot: &UsageSnapshot,
        account_discriminator: Option<&str>,
        requires_known_account: bool,
    ) -> (Vec<QuotaWarningEvent>, Vec<NotificationPost>) {
        if !self.preferences.quota_warning_notifications_enabled {
            return (Vec::new(), Vec::new());
        }
        if requires_known_account && account_discriminator.is_none() {
            return (Vec::new(), Vec::new());
        }

        let id = provider.as_str();
        let source = if id == "antigravity" {
            Some(if has_antigravity_quota_summary_windows(snapshot) {
                SessionQuotaWindowSource::AntigravityQuotaSummary
            } else {
                SessionQuotaWindowSource::AntigravityLegacy
            })
        } else {
            None
        };
        let (primary_window, secondary_window) = if id == "antigravity" {
            (
                antigravity_window(snapshot, 5 * 60),
                antigravity_window(snapshot, 7 * 24 * 60),
            )
        } else {
            let suppress = id == "mimo" || id == "qoder";
            if suppress {
                (None, None)
            } else {
                (snapshot.primary.clone(), snapshot.secondary.clone())
            }
        };
        let primary_display_label = (id == "amp").then(|| "Other usage".to_owned());
        let secondary_display_label = (id == "amp").then(|| "Orb usage".to_owned());
        let extra_windows: Vec<NamedRateWindow> = if id == "claude" {
            snapshot
                .extra_rate_windows
                .as_ref()
                .map(|extras| {
                    extras
                        .iter()
                        .filter(|w| is_claude_notifiable_extra_window(w))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        type QuotaWarningLane = (
            QuotaWarningWindow,
            Option<RateWindow>,
            Option<String>,
            Option<String>,
        );
        let mut lanes: Vec<QuotaWarningLane> = vec![
            (
                QuotaWarningWindow::Session,
                primary_window,
                None,
                primary_display_label,
            ),
            (
                QuotaWarningWindow::Weekly,
                secondary_window,
                None,
                secondary_display_label,
            ),
        ];
        for extra in &extra_windows {
            lanes.push((
                QuotaWarningWindow::Weekly,
                Some(extra.window.clone()),
                Some(extra.id.clone()),
                Some(extra.title.clone()),
            ));
        }

        let mut events = Vec::new();
        let mut posts = Vec::new();
        for (window, rate_window, window_id, label) in lanes {
            if let Some((event, post)) = self.handle_single_quota_warning_transition(
                provider,
                window,
                rate_window.as_ref(),
                source,
                account_discriminator,
                snapshot.updated_at,
                window_id,
                label,
            ) {
                events.push(event);
                posts.push(post);
            }
        }

        if !extra_windows.is_empty()
            && self
                .preferences
                .quota_warning_enabled(provider, QuotaWarningWindow::Weekly)
        {
            let active_ids: HashSet<&str> = extra_windows.iter().map(|w| w.id.as_str()).collect();
            self.quota_warning_states.retain(|key, _| {
                &key.provider != provider
                    || key.window != QuotaWarningWindow::Weekly
                    || key.account_discriminator.as_deref() != account_discriminator
                    || key
                        .window_id
                        .as_deref()
                        .is_none_or(|wid| active_ids.contains(wid))
            });
        }

        (events, posts)
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_single_quota_warning_transition(
        &mut self,
        provider: &ProviderId,
        window: QuotaWarningWindow,
        rate_window: Option<&RateWindow>,
        source: Option<SessionQuotaWindowSource>,
        account_discriminator: Option<&str>,
        observed_at: Timestamp,
        window_id: Option<String>,
        window_display_label: Option<String>,
    ) -> Option<(QuotaWarningEvent, NotificationPost)> {
        let mut key = QuotaWarningStateKey {
            provider: provider.clone(),
            window,
            account_discriminator: account_discriminator.map(str::to_owned),
            window_id: window_id.clone(),
        };
        if !self.preferences.quota_warning_enabled(provider, window) {
            self.quota_warning_states
                .retain(|k, _| &k.provider != provider || k.window != window);
            return None;
        }
        let Some(rate_window) = rate_window else {
            self.quota_warning_states.remove(&key);
            return None;
        };
        if provider.as_str() == "claude"
            && window == QuotaWarningWindow::Session
            && !is_session_window(rate_window)
        {
            return None;
        }
        if rate_window.is_synthetic_placeholder {
            return None;
        }

        let thresholds = self
            .preferences
            .resolved_quota_warning_thresholds(provider, window);
        let current_remaining = rate_window.remaining_percent();
        if provider.as_str() == "claude"
            && key.account_discriminator.as_deref() == Some("claude-account:unknown")
            && !self.quota_warning_states.contains_key(&key)
            && let Some(ref account) = self.last_claude_quota_warning_account
        {
            let account_key = QuotaWarningStateKey {
                provider: provider.clone(),
                window,
                account_discriminator: Some(account.clone()),
                window_id: window_id.clone(),
            };
            if let Some(prior) = self.quota_warning_states.get(&account_key).cloned()
                && prior.observed_at <= observed_at
            {
                if let Some(resets_at) = rate_window.resets_at
                    && Some(resets_at) == prior.resets_at
                    && let Some(remaining) = prior.last_remaining
                    && current_remaining <= remaining
                {
                    key = account_key;
                } else if prior.shared_with_unresolved_account {
                    self.quota_warning_states.insert(key.clone(), prior);
                }
            }
        }

        let previous_state = self.quota_warning_states.get(&key).cloned();
        if let Some(ref prev) = previous_state
            && prev.source != source
        {
            self.quota_warning_states.insert(
                key,
                QuotaWarningState {
                    last_remaining: Some(current_remaining),
                    observed_at,
                    fired_thresholds: BTreeSet::new(),
                    source,
                    resets_at: None,
                    shared_with_unresolved_account: false,
                },
            );
            return None;
        }

        let mut state = previous_state.unwrap_or(QuotaWarningState {
            source,
            ..QuotaWarningState::default()
        });
        let cleared = QuotaWarningNotificationLogic::thresholds_to_clear(
            current_remaining,
            &state.fired_thresholds,
        );
        for t in cleared {
            state.fired_thresholds.remove(&t);
        }

        let mut fired_result = None;
        if let Some(threshold) = QuotaWarningNotificationLogic::crossed_threshold(
            state.last_remaining,
            current_remaining,
            &thresholds,
            &state.fired_thresholds,
        ) {
            let newly_fired = QuotaWarningNotificationLogic::fired_thresholds_after_warning(
                threshold,
                &thresholds,
            );
            state.fired_thresholds.extend(newly_fired);
            let event = QuotaWarningEvent::new(
                window,
                threshold,
                current_remaining,
                window_id,
                window_display_label.clone(),
            );
            let provider_name =
                find_by_id(provider).map_or_else(|| provider.as_str(), |d| d.display_name);
            let (title, body) = QuotaWarningNotificationLogic::notification_copy(
                provider_name,
                window,
                threshold,
                current_remaining,
                window_display_label.as_deref(),
            );
            let id_prefix = QuotaWarningNotificationLogic::notification_id_prefix(provider, &event);
            let post = NotificationPost {
                id_prefix,
                identifier: None,
                title,
                body,
                sound_enabled: self.preferences.quota_warning_sound_enabled,
                on_screen_alert_enabled: self.preferences.quota_warning_on_screen_alert_enabled,
            };
            fired_result = Some((event, post));
        }

        state.observed_at = observed_at;
        state.resets_at = rate_window.resets_at;
        state.last_remaining = Some(current_remaining);
        self.quota_warning_states.insert(key, state);
        fired_result
    }

    /// Evaluates session quota transitions for `provider` and `snapshot`.
    ///
    /// Returns `(posted_notification, session_restored_pending_for_reset_arbitration, transition)`.
    pub fn handle_session_quota_transition(
        &mut self,
        provider: &ProviderId,
        snapshot: &UsageSnapshot,
        account_discriminator: Option<&str>,
        codex_owner_key: Option<&CodexSessionQuotaOwnerKey>,
        hooks_active: bool,
        now: Timestamp,
    ) -> (Option<NotificationPost>, bool, SessionQuotaTransition) {
        let notifications_enabled = self.preferences.session_quota_notifications_enabled;
        let detection_enabled = notifications_enabled || hooks_active;
        let is_codex = provider.as_str() == "codex";

        if is_codex && !detection_enabled {
            self.require_fresh_codex_session_quota_baseline(Some(snapshot.updated_at));
            return (None, false, SessionQuotaTransition::None);
        }
        if is_codex && codex_owner_key.is_none() {
            self.require_fresh_codex_session_quota_baseline(Some(snapshot.updated_at));
            return (None, false, SessionQuotaTransition::None);
        }
        let Some((session_window, current_source)) = session_quota_window(provider, snapshot)
        else {
            if is_codex {
                let codex_id = ProviderId::new("codex").expect("valid provider id");
                if let Some(previous) = self.session_quota_states.get(&codex_id).cloned() {
                    if previous.account_discriminator.as_deref()
                        != codex_owner_key.map(|k| k.raw_value.as_str())
                    {
                        self.require_fresh_codex_session_quota_baseline(Some(snapshot.updated_at));
                    } else {
                        self.session_quota_states.insert(
                            codex_id,
                            previous.advancing_observation_watermark(snapshot.updated_at),
                        );
                    }
                } else if self.codex_session_quota_baseline_requirement.is_some() {
                    self.require_fresh_codex_session_quota_baseline(Some(snapshot.updated_at));
                }
            } else {
                self.clear_session_quota_transition_state(provider);
            }
            return (None, false, SessionQuotaTransition::None);
        };

        if session_window.is_synthetic_placeholder {
            return (None, false, SessionQuotaTransition::None);
        }
        if is_codex
            && let Some(ref req) = self.codex_session_quota_baseline_requirement
            && !req.admits(snapshot.updated_at)
        {
            return (None, false, SessionQuotaTransition::None);
        }

        let previous_state = self.session_quota_states.get(provider).cloned();
        let force_baseline = is_codex && self.codex_session_quota_baseline_requirement.is_some();
        let notification_account = codex_owner_key
            .map(|k| k.raw_value.clone())
            .or_else(|| account_discriminator.map(str::to_owned))
            .or_else(|| plan_utilization_identity_account_key(provider, snapshot));
        let evaluation = SessionQuotaTransitionReducer::evaluate(
            previous_state.as_ref(),
            &SessionQuotaTransitionObservation {
                provider: provider.clone(),
                remaining: session_window.remaining_percent(),
                source: current_source,
                reset_boundary: session_window.resets_at,
                observed_at: snapshot.updated_at,
                evaluation_time: now,
                account_discriminator: notification_account,
            },
            detection_enabled,
            force_baseline,
        );
        self.session_quota_states
            .insert(provider.clone(), evaluation.state);
        if is_codex {
            self.codex_session_quota_baseline_requirement = None;
        }

        let transition = evaluation.outcome.transition();
        match transition {
            SessionQuotaTransition::None => (None, false, SessionQuotaTransition::None),
            SessionQuotaTransition::Restored => {
                if notifications_enabled && self.preferences.limit_reset_notifications_enabled {
                    return (None, true, SessionQuotaTransition::Restored);
                }
                let post = notifications_enabled.then(|| {
                    self.make_session_quota_post(provider, SessionQuotaTransition::Restored)
                });
                (post, false, SessionQuotaTransition::Restored)
            }
            SessionQuotaTransition::Depleted => {
                let post = notifications_enabled.then(|| {
                    self.make_session_quota_post(provider, SessionQuotaTransition::Depleted)
                });
                (post, false, SessionQuotaTransition::Depleted)
            }
        }
    }

    pub fn make_session_quota_post(
        &self,
        provider: &ProviderId,
        transition: SessionQuotaTransition,
    ) -> NotificationPost {
        let provider_name =
            find_by_id(provider).map_or_else(|| provider.as_str(), |d| d.display_name);
        let (title, body) =
            SessionQuotaNotificationLogic::notification_copy(transition, provider_name);
        NotificationPost {
            id_prefix: SessionQuotaNotificationLogic::notification_id_prefix(provider, transition),
            identifier: None,
            title,
            body,
            sound_enabled: true,
            on_screen_alert_enabled: false,
        }
    }

    pub fn make_limit_reset_post(
        &self,
        provider: &ProviderId,
        window: QuotaWarningWindow,
    ) -> NotificationPost {
        let provider_name =
            find_by_id(provider).map_or_else(|| provider.as_str(), |d| d.display_name);
        let (title, body) = LimitResetNotificationLogic::notification_copy(provider_name, window);
        NotificationPost {
            id_prefix: LimitResetNotificationLogic::notification_id_prefix(provider, window),
            identifier: None,
            title,
            body,
            sound_enabled: true,
            on_screen_alert_enabled: false,
        }
    }

    /// Evaluates limit-reset detection across session and weekly lanes for `provider` and `snapshot`.
    pub fn handle_limit_reset_transitions(
        &mut self,
        provider: &ProviderId,
        snapshot: &UsageSnapshot,
        account_key: Option<&str>,
        codex_limit_reset_owner_key: Option<&CodexLimitResetOwnerKey>,
        session_restored_pending: bool,
    ) -> Vec<NotificationPost> {
        let mut posts = Vec::new();
        let session_obs = session_quota_window(provider, snapshot)
            .filter(|(w, _)| !w.is_synthetic_placeholder)
            .map(|(w, source)| LimitResetObservation {
                used_percent: w.used_percent,
                observed_at: snapshot.updated_at,
                reset_boundary: w.resets_at,
                source: Some(source),
            });
        let weekly_obs = weekly_limit_reset_window(provider, snapshot)
            .filter(|w| !w.is_synthetic_placeholder)
            .map(|w| LimitResetObservation {
                used_percent: w.used_percent,
                observed_at: snapshot.updated_at,
                reset_boundary: w.resets_at,
                source: None,
            });

        if let Some(post) = self.evaluate_single_limit_reset_lane(
            provider,
            snapshot,
            QuotaWarningWindow::Session,
            session_obs.as_ref(),
            account_key,
            codex_limit_reset_owner_key,
            session_restored_pending,
        ) {
            posts.push(post);
        }
        if let Some(post) = self.evaluate_single_limit_reset_lane(
            provider,
            snapshot,
            QuotaWarningWindow::Weekly,
            weekly_obs.as_ref(),
            account_key,
            codex_limit_reset_owner_key,
            false,
        ) {
            posts.push(post);
        }
        posts
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate_single_limit_reset_lane(
        &mut self,
        provider: &ProviderId,
        snapshot: &UsageSnapshot,
        window: QuotaWarningWindow,
        observation: Option<&LimitResetObservation>,
        account_key: Option<&str>,
        codex_limit_reset_owner_key: Option<&CodexLimitResetOwnerKey>,
        restored_pending: bool,
    ) -> Option<NotificationPost> {
        let restored = window == QuotaWarningWindow::Session && restored_pending;
        let Some(observation) = observation else {
            if restored && self.preferences.session_quota_notifications_enabled {
                return Some(
                    self.make_session_quota_post(provider, SessionQuotaTransition::Restored),
                );
            }
            return None;
        };

        let Some(account_identifier) = limit_reset_account_identifier(
            provider,
            snapshot,
            account_key,
            codex_limit_reset_owner_key,
        ) else {
            if restored && self.preferences.session_quota_notifications_enabled {
                return Some(
                    self.make_session_quota_post(provider, SessionQuotaTransition::Restored),
                );
            }
            return None;
        };

        let detector_key = format!("{}:{}", provider.as_str(), account_identifier);
        let requires_low_confirmation =
            provider.as_str() == "claude" && account_identifier == provider.as_str();
        let states = match window {
            QuotaWarningWindow::Session => &mut self.session_limit_reset_states,
            QuotaWarningWindow::Weekly => &mut self.weekly_limit_reset_states,
        };
        if let Some(existing) = states.get(&detector_key)
            && observation.observed_at <= existing.last_observed_at
        {
            return None;
        }
        let previous_state = states.get(&detector_key).cloned();
        let (mut state, should_post) = evaluate_limit_reset_detector_transition(
            provider,
            snapshot,
            window,
            observation,
            previous_state.as_ref(),
            requires_low_confirmation,
        );
        let notice = prepare_limit_reset_notification(
            &mut state,
            previous_state.as_ref(),
            observation,
            should_post,
            restored,
            self.preferences.session_quota_notifications_enabled,
            self.preferences.limit_reset_notifications_enabled,
        );
        states.insert(detector_key, state);

        match notice? {
            LimitResetNotice::Restored => self
                .preferences
                .session_quota_notifications_enabled
                .then(|| self.make_session_quota_post(provider, SessionQuotaTransition::Restored)),
            LimitResetNotice::Reset => self
                .preferences
                .limit_reset_notifications_enabled
                .then(|| self.make_limit_reset_post(provider, window)),
        }
    }
}

fn is_claude_notifiable_extra_window(named: &NamedRateWindow) -> bool {
    named.usage_known
        && (named.id.starts_with("claude-weekly-scoped-") || named.id == "claude-routines")
}

fn plan_utilization_identity_account_key(
    provider: &ProviderId,
    snapshot: &UsageSnapshot,
) -> Option<String> {
    let identity = snapshot
        .identity_for(provider)
        .or(snapshot.identity.as_ref())?;
    identity
        .account_email
        .as_deref()
        .or(identity.account_organization.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("email:{}", s.to_ascii_lowercase()))
}

fn weekly_limit_reset_window(
    provider: &ProviderId,
    snapshot: &UsageSnapshot,
) -> Option<RateWindow> {
    if provider.as_str() == "antigravity" {
        return antigravity_window(snapshot, 7 * 24 * 60);
    }
    if provider.as_str() == "mimo" || provider.as_str() == "qoder" {
        return None;
    }
    snapshot.secondary.clone()
}

pub fn limit_reset_account_identifier(
    provider: &ProviderId,
    snapshot: &UsageSnapshot,
    account_key: Option<&str>,
    codex_limit_reset_owner_key: Option<&CodexLimitResetOwnerKey>,
) -> Option<String> {
    if provider.as_str() == "codex" {
        return codex_limit_reset_owner_key.map(|k| k.raw_value.clone());
    }
    let identity = snapshot
        .identity_for(provider)
        .or(snapshot.identity.as_ref());
    account_key
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            identity
                .and_then(|i| i.account_email.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .or_else(|| {
            identity
                .and_then(|i| i.account_organization.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .or_else(|| Some(provider.as_str().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // upstream: SessionQuotaNotificationLogicTests.swift:8 does nothing without previous value
    #[test]
    fn session_quota_does_nothing_without_previous_value() {
        let transition = SessionQuotaNotificationLogic::transition(None, Some(0.0));
        assert_eq!(transition, SessionQuotaTransition::None);
    }

    // upstream: SessionQuotaNotificationLogicTests.swift:14 detects depleted transition
    #[test]
    fn session_quota_detects_depleted_transition() {
        let transition = SessionQuotaNotificationLogic::transition(Some(12.0), Some(0.0));
        assert_eq!(transition, SessionQuotaTransition::Depleted);
    }

    // upstream: SessionQuotaNotificationLogicTests.swift:20 detects restored transition
    #[test]
    fn session_quota_detects_restored_transition() {
        let transition = SessionQuotaNotificationLogic::transition(Some(0.0), Some(5.0));
        assert_eq!(transition, SessionQuotaTransition::Restored);
    }

    // upstream: SessionQuotaNotificationLogicTests.swift:26 ignores non transitions
    #[test]
    fn session_quota_ignores_non_transitions() {
        assert_eq!(
            SessionQuotaNotificationLogic::transition(Some(0.0), Some(0.0)),
            SessionQuotaTransition::None
        );
        assert_eq!(
            SessionQuotaNotificationLogic::transition(Some(10.0), Some(10.0)),
            SessionQuotaTransition::None
        );
        assert_eq!(
            SessionQuotaNotificationLogic::transition(Some(10.0), Some(9.0)),
            SessionQuotaTransition::None
        );
    }

    // upstream: SessionQuotaNotificationLogicTests.swift:33 treats tiny positive remaining as depleted
    #[test]
    fn session_quota_treats_tiny_positive_remaining_as_depleted() {
        let transition = SessionQuotaNotificationLogic::transition(Some(0.0), Some(0.00001));
        assert_eq!(transition, SessionQuotaTransition::None);
    }

    #[test]
    fn session_quota_depleted_notification_copy_english() {
        let (title, body) = SessionQuotaNotificationLogic::notification_copy(
            SessionQuotaTransition::Depleted,
            "Codex",
        );
        assert_eq!(title, "Codex session depleted");
        assert_eq!(body, "0% left. Will notify when it's available again.");
    }

    #[test]
    fn session_quota_restored_notification_copy_english() {
        let (title, body) = SessionQuotaNotificationLogic::notification_copy(
            SessionQuotaTransition::Restored,
            "Codex",
        );
        assert_eq!(title, "Codex session restored");
        assert_eq!(body, "Session quota is available again.");
    }

    // upstream: QuotaWarningNotificationLogicTests.swift:7 quota warning copy includes current remaining and threshold
    #[test]
    fn quota_warning_copy_includes_current_remaining_and_threshold() {
        let (title, body) = QuotaWarningNotificationLogic::notification_copy(
            "Codex",
            QuotaWarningWindow::Session,
            20,
            12.4,
            None,
        );
        assert_eq!(title, "Codex session quota low");
        assert_eq!(
            body,
            "12% left. Reached your 20% session warning threshold."
        );
    }

    // upstream: QuotaWarningNotificationLogicTests.swift:21 quota warning copy clamps current remaining
    #[test]
    fn quota_warning_copy_clamps_current_remaining() {
        let (title, body) = QuotaWarningNotificationLogic::notification_copy(
            "Codex",
            QuotaWarningWindow::Weekly,
            50,
            -3.0,
            None,
        );
        assert_eq!(title, "Codex weekly quota low");
        assert_eq!(body, "0% left. Reached your 50% weekly warning threshold.");
    }

    #[test]
    fn quota_warning_copy_never_includes_account_identifier() {
        let (title, body) = QuotaWarningNotificationLogic::notification_copy(
            "Codex",
            QuotaWarningWindow::Session,
            50,
            45.0,
            None,
        );
        assert_eq!(title, "Codex session quota low");
        assert_eq!(
            body,
            "45% left. Reached your 50% session warning threshold."
        );
        assert!(!body.contains('@'));
    }

    // upstream: QuotaWarningNotificationLogicTests.swift:50 quota warning copy uses the extra-window display label when provided
    #[test]
    fn quota_warning_copy_uses_extra_window_display_label_when_provided() {
        let (title, body) = QuotaWarningNotificationLogic::notification_copy(
            "Claude",
            QuotaWarningWindow::Weekly,
            50,
            45.0,
            Some("Fable only"),
        );
        assert_eq!(title, "Claude Fable only quota low");
        assert_eq!(
            body,
            "45% left. Reached your 50% Fable only warning threshold."
        );
    }

    // upstream: QuotaWarningNotificationLogicTests.swift:65 extra-window notification identifiers are independent
    #[test]
    fn extra_window_notification_identifiers_are_independent() {
        let fable = QuotaWarningEvent::new(
            QuotaWarningWindow::Weekly,
            50,
            45.0,
            Some("claude-weekly-scoped-fable".to_owned()),
            None,
        );
        let routines = QuotaWarningEvent::new(
            QuotaWarningWindow::Weekly,
            50,
            45.0,
            Some("claude-routines".to_owned()),
            None,
        );
        let claude = ProviderId::new("claude").unwrap();
        let id_fable = QuotaWarningNotificationLogic::notification_id_prefix(&claude, &fable);
        let id_routines = QuotaWarningNotificationLogic::notification_id_prefix(&claude, &routines);
        assert_ne!(id_fable, id_routines);
        assert!(id_fable.contains("claude-weekly-scoped-fable"));
        assert!(id_routines.contains("claude-routines"));
    }

    // upstream: QuotaWarningNotificationLogicTests.swift:101 does nothing without crossing
    #[test]
    fn quota_warning_does_nothing_without_crossing() {
        let crossed = QuotaWarningNotificationLogic::crossed_threshold(
            Some(60.0),
            55.0,
            &[50, 20],
            &BTreeSet::new(),
        );
        assert_eq!(crossed, None);
    }

    // upstream: QuotaWarningNotificationLogicTests.swift:112 detects downward crossing
    #[test]
    fn quota_warning_detects_downward_crossing() {
        let crossed = QuotaWarningNotificationLogic::crossed_threshold(
            Some(55.0),
            45.0,
            &[50, 20],
            &BTreeSet::new(),
        );
        assert_eq!(crossed, Some(50));
    }

    // upstream: QuotaWarningNotificationLogicTests.swift:123 skips already fired thresholds
    #[test]
    fn quota_warning_skips_already_fired_thresholds() {
        let fired = BTreeSet::from([50]);
        let crossed =
            QuotaWarningNotificationLogic::crossed_threshold(Some(55.0), 45.0, &[50, 20], &fired);
        assert_eq!(crossed, None);
    }

    // upstream: QuotaWarningNotificationLogicTests.swift:134 chooses most severe threshold when crossing several at once
    #[test]
    fn quota_warning_chooses_most_severe_threshold_when_crossing_several_at_once() {
        let crossed = QuotaWarningNotificationLogic::crossed_threshold(
            Some(80.0),
            10.0,
            &[50, 20],
            &BTreeSet::new(),
        );
        assert_eq!(crossed, Some(20));
    }

    // upstream: QuotaWarningNotificationLogicTests.swift:145 startup below threshold warns once at most severe threshold
    #[test]
    fn quota_warning_startup_below_threshold_warns_once_at_most_severe_threshold() {
        let crossed = QuotaWarningNotificationLogic::crossed_threshold(
            None,
            10.0,
            &[50, 20],
            &BTreeSet::new(),
        );
        assert_eq!(crossed, Some(20));
    }

    // upstream: QuotaWarningNotificationLogicTests.swift:156 warning marks threshold and higher thresholds fired
    #[test]
    fn quota_warning_marks_threshold_and_higher_thresholds_fired() {
        let fired = QuotaWarningNotificationLogic::fired_thresholds_after_warning(20, &[50, 20]);
        assert_eq!(fired, BTreeSet::from([50, 20]));
    }

    // upstream: QuotaWarningNotificationLogicTests.swift:165 recovery clears only thresholds below current remaining
    #[test]
    fn quota_warning_recovery_clears_only_thresholds_below_current_remaining() {
        let already_fired = BTreeSet::from([50, 20]);
        let cleared = QuotaWarningNotificationLogic::thresholds_to_clear(30.0, &already_fired);
        assert_eq!(cleared, BTreeSet::from([20]));
    }

    // upstream: QuotaWarningNotificationLogicTests.swift:174 zero threshold does not post quota warning
    #[test]
    fn quota_warning_zero_threshold_does_not_post_quota_warning() {
        let fired = BTreeSet::from([10]);
        let crossed =
            QuotaWarningNotificationLogic::crossed_threshold(Some(10.0), 0.0, &[10, 0], &fired);
        assert_eq!(crossed, None);
        assert_eq!(
            QuotaWarningNotificationLogic::fired_thresholds_after_warning(10, &[10, 0]),
            BTreeSet::from([10])
        );
    }

    // upstream: CodexResetCreditExpiryNotifierTests.swift:9 posts one bounded summary without persisting or logging raw credit IDs
    #[test]
    fn codex_reset_credit_expiry_posts_one_bounded_summary_without_raw_credit_ids() {
        let now = Timestamp::from_second(1_781_726_400).unwrap();
        let raw_id = "private-provider-credit-id";
        let mut notifier = CodexResetCreditExpiryNotifier::new();
        let credits = vec![
            CodexResetCreditItem::available(
                sha256_hex(raw_id),
                Some(Timestamp::from_second(1_781_726_400 + 86_400).unwrap()),
            ),
            CodexResetCreditItem::available(sha256_hex("no-expiry-private-id"), None),
        ];

        let first = notifier.post_expiring_credits_if_needed(&credits, now);
        let second = notifier.post_expiring_credits_if_needed(&credits, now);

        assert!(second.is_none());
        let post = first.expect("expected expiring credit notification");
        assert_eq!(
            post.id_prefix,
            CodexResetCreditExpiryNotifier::NOTIFICATION_PREFIX
        );
        assert_eq!(post.title, "Limit Reset Credits");
        assert_eq!(post.body, "1. Expires in 1d");
        assert!(!post.id_prefix.contains(raw_id));
        assert!(!post.body.contains(raw_id));
        assert_eq!(notifier.remembered_fingerprints.len(), 1);
        assert_eq!(notifier.remembered_fingerprints[0].len(), 64);
        assert!(!notifier.remembered_fingerprints[0].contains(raw_id));
    }

    // upstream: CodexResetCreditExpiryNotifierTests.swift:45 switching account inventories does not repeat either notification
    #[test]
    fn codex_reset_credit_expiry_switching_account_inventories_does_not_repeat_either_notification()
    {
        let now = Timestamp::from_second(1_781_726_400).unwrap();
        let mut notifier = CodexResetCreditExpiryNotifier::new();
        let first_account = vec![CodexResetCreditItem::available(
            sha256_hex("first-account-credit"),
            Some(Timestamp::from_second(1_781_726_400 + 86_400).unwrap()),
        )];
        let second_account = vec![CodexResetCreditItem::available(
            sha256_hex("second-account-credit"),
            Some(Timestamp::from_second(1_781_726_400 + 172_800).unwrap()),
        )];

        assert!(
            notifier
                .post_expiring_credits_if_needed(&first_account, now)
                .is_some()
        );
        assert!(
            notifier
                .post_expiring_credits_if_needed(&second_account, now)
                .is_some()
        );
        assert!(
            notifier
                .post_expiring_credits_if_needed(&first_account, now)
                .is_none()
        );
        assert!(
            notifier
                .post_expiring_credits_if_needed(&second_account, now)
                .is_none()
        );
        assert_eq!(notifier.remembered_fingerprints.len(), 2);
    }

    // upstream: CodexResetCreditExpiryNotifierTests.swift:73 no-expiry inventory does not trigger an expiry notification
    #[test]
    fn codex_reset_credit_expiry_no_expiry_inventory_does_not_trigger_notification() {
        let now = Timestamp::from_second(1_781_726_400).unwrap();
        let mut notifier = CodexResetCreditExpiryNotifier::new();
        let credits = vec![CodexResetCreditItem::available(
            sha256_hex("no-expiry"),
            None,
        )];

        assert!(
            notifier
                .post_expiring_credits_if_needed(&credits, now)
                .is_none()
        );
        assert!(notifier.remembered_fingerprints.is_empty());
    }
}
