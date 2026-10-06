//! Projection of core domain models into `ariadusage-protocol` wire snapshot types.
//!
//! Enforces honesty invariants (no synthetic placeholders, unknown usage, loading states,
//! or errors without kept data ever render as real numeric values), the identity silo,
//! safe static error messages (decision 12), and window-duration clamping.

use ariadusage_protocol::{
    Confidence, Cost, Identity, Metric, MetricSource, MetricState, NamedWindow, Pace, PaceStage,
    ProviderError, ProviderErrorCategory, ProviderErrorKind, ProviderId, ProviderSnapshot,
    ProviderWindows, RateWindow, SourceKind,
};

use crate::model::UsageSnapshot;
use crate::pace::UsagePace;

/// Refresh state of a provider fetch operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshState {
    Fresh,
    Loading,
    Stale,
    Error,
}

/// Structured error parts ready for projection into `ProviderError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedError {
    pub kind: ProviderErrorKind,
    pub category: ProviderErrorCategory,
    pub retry_after_seconds: Option<u64>,
}

impl ProjectedError {
    pub fn new(kind: ProviderErrorKind, category: ProviderErrorCategory) -> Self {
        Self {
            kind,
            category,
            retry_after_seconds: None,
        }
    }

    pub fn with_retry(mut self, seconds: u64) -> Self {
        self.retry_after_seconds = Some(seconds);
        self
    }
}

/// Input parameters for projecting a core `UsageSnapshot` into a protocol `ProviderSnapshot`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionInput {
    pub provider: ProviderId,
    pub display_name: String,
    pub enabled: bool,
    pub source_mode: String,
    pub refresh: RefreshState,
    pub kept_snapshot: bool,
    pub as_of: jiff::Timestamp,
    pub source_kind: SourceKind,
    pub source_label: String,
    pub error: Option<ProjectedError>,
    pub pace: Option<UsagePace>,
}

/// Returns the safe static message for a provider error category (decision 12).
pub fn category_safe_message(category: ProviderErrorCategory) -> &'static str {
    category.safe_description()
}

/// Project a core `UsageSnapshot` into a protocol `ProviderSnapshot`.
pub fn provider_snapshot(snapshot: &UsageSnapshot, input: &ProjectionInput) -> ProviderSnapshot {
    let source = MetricSource::new(input.source_kind, input.source_label.clone());
    let as_of = Some(input.as_of);

    let data_state = match input.refresh {
        RefreshState::Fresh => MetricState::Value,
        RefreshState::Stale => MetricState::Stale,
        RefreshState::Loading => {
            if input.kept_snapshot {
                MetricState::Stale
            } else {
                MetricState::Loading
            }
        }
        RefreshState::Error => {
            if input.kept_snapshot {
                MetricState::Stale
            } else {
                MetricState::Error
            }
        }
    };

    let primary = project_window_metric(
        snapshot.primary.as_ref(),
        data_state,
        as_of,
        &source,
        snapshot.data_confidence,
    );
    let secondary = project_window_metric(
        snapshot.secondary.as_ref(),
        data_state,
        as_of,
        &source,
        snapshot.data_confidence,
    );
    let tertiary = project_window_metric(
        snapshot.tertiary.as_ref(),
        data_state,
        as_of,
        &source,
        snapshot.data_confidence,
    );

    let extra = if let Some(ref extras) = snapshot.extra_rate_windows {
        extras
            .iter()
            .map(|extra| {
                let state = if !extra.usage_known || extra.window.is_synthetic_placeholder {
                    MetricState::Unknown
                } else {
                    data_state
                };
                let win_val = if matches!(state, MetricState::Value | MetricState::Stale) {
                    Some(project_rate_window(&extra.window))
                } else {
                    None
                };
                let metric = Metric::new(
                    state,
                    win_val,
                    as_of,
                    Some(source.clone()),
                    snapshot.data_confidence,
                    None,
                )
                .expect("metric honesty invariant holds");

                NamedWindow {
                    id: extra.id.clone(),
                    title: extra.title.clone(),
                    window: metric,
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    let windows = ProviderWindows {
        primary,
        secondary,
        tertiary,
        extra,
    };

    let cost = snapshot.provider_cost.as_ref().map(|c| {
        let limit = if c.limit <= 0.0 || c.limit.is_nan() {
            None
        } else {
            Some(c.limit)
        };
        let cost_val = if matches!(data_state, MetricState::Value | MetricState::Stale) {
            Some(Cost {
                used: c.used,
                limit,
                currency_code: c.currency_code.clone(),
                period: c.period.clone(),
                resets_at: c.resets_at,
                balance: c.balance,
            })
        } else {
            None
        };
        Metric::new(
            data_state,
            cost_val,
            as_of,
            Some(source.clone()),
            snapshot.data_confidence,
            None,
        )
        .expect("metric honesty invariant holds")
    });

    let identity = snapshot.identity_for(&input.provider).map(|id| {
        let id_val = if matches!(data_state, MetricState::Value | MetricState::Stale) {
            Some(Identity {
                provider_id: input.provider.clone(),
                account_email: id.account_email.clone(),
                account_organization: id.account_organization.clone(),
                login_method: id.login_method.clone(),
                account_id: id.account_id.clone(),
                plan: id.login_method.clone(),
            })
        } else {
            None
        };
        Metric::new(
            data_state,
            id_val,
            as_of,
            Some(source.clone()),
            snapshot.data_confidence,
            None,
        )
        .expect("metric honesty invariant holds")
    });

    let pace = input.pace.as_ref().map(|p| {
        let pace_val = if matches!(data_state, MetricState::Value | MetricState::Stale) {
            Some(project_pace(p, &input.provider, input.as_of))
        } else {
            None
        };
        Metric::new(
            data_state,
            pace_val,
            as_of,
            Some(source.clone()),
            snapshot.data_confidence,
            None,
        )
        .expect("metric honesty invariant holds")
    });

    let last_error = input.error.as_ref().map(|e| ProviderError {
        kind: e.kind,
        category: e.category,
        message: category_safe_message(e.category).to_string(),
        retry_after_seconds: e.retry_after_seconds,
    });

    ProviderSnapshot {
        id: input.provider.clone(),
        display_name: input.display_name.clone(),
        enabled: input.enabled,
        source_mode: input.source_mode.clone(),
        windows,
        credits: None,
        cost,
        identity,
        status: None,
        pace,
        details: snapshot.details.clone(),
        accounts: Vec::new(),
        last_error,
        updated_at: Some(snapshot.updated_at),
    }
}

fn project_window_metric(
    window: Option<&crate::model::RateWindow>,
    data_state: MetricState,
    as_of: Option<jiff::Timestamp>,
    source: &MetricSource,
    confidence: Confidence,
) -> Option<Metric<RateWindow>> {
    let win = window?;
    let state = if win.is_synthetic_placeholder {
        MetricState::Unknown
    } else {
        data_state
    };
    let value = if matches!(state, MetricState::Value | MetricState::Stale) {
        Some(project_rate_window(win))
    } else {
        None
    };
    Some(
        Metric::new(state, value, as_of, Some(source.clone()), confidence, None)
            .expect("metric honesty invariant holds"),
    )
}

fn project_rate_window(win: &crate::model::RateWindow) -> RateWindow {
    let window_minutes = match win.window_minutes {
        Some(m) if m > 0 && m <= u32::MAX as i64 => Some(m as u32),
        _ => None,
    };
    RateWindow {
        used_percent: win.used_percent,
        window_minutes,
        resets_at: win.resets_at,
        reset_description: win.reset_description.clone(),
        next_regen_percent: win.next_regen_percent,
    }
}

/// Project a core `UsagePace` into a `protocol::Pace` structure with formatted summary text.
pub fn project_pace(pace: &UsagePace, provider: &ProviderId, now: jiff::Timestamp) -> Pace {
    let descriptor = crate::providers::find_by_id(provider);
    let shows_headroom_hint = descriptor
        .map(|d| d.pace_capability.shows_headroom_hint)
        .unwrap_or(false);

    let summary = format_pace_summary(pace, shows_headroom_hint, now);

    Pace {
        stage: pace.stage,
        delta_percent: pace.delta_percent.round(),
        expected_used_percent: pace.expected_used_percent.round(),
        will_last_to_reset: pace.will_last_to_reset,
        eta_seconds: pace.eta_seconds.map(|s| s.max(0.0).round() as u64),
        summary,
    }
}

/// Format the human-readable summary string for pace.
pub fn format_pace_summary(
    pace: &UsagePace,
    shows_headroom_hint: bool,
    _now: jiff::Timestamp,
) -> String {
    let expected = pace.expected_used_percent.round() as i64;
    let mut parts: Vec<String> = Vec::new();

    parts.push(pace_left_label(pace));
    parts.push(format!("Expected {expected}% used"));

    if let Some(right) = pace_right_label(pace, shows_headroom_hint) {
        parts.push(right);
    }

    parts.join(" | ")
}

fn pace_left_label(pace: &UsagePace) -> String {
    let delta = pace.delta_percent.abs().round() as i64;
    if delta == 0 {
        return "On pace".to_string();
    }
    match pace.stage {
        PaceStage::OnTrack => "On pace".to_string(),
        PaceStage::SlightlyAhead | PaceStage::Ahead | PaceStage::FarAhead => {
            format!("{delta}% in deficit")
        }
        PaceStage::SlightlyBehind | PaceStage::Behind | PaceStage::FarBehind => {
            format!("{delta}% in reserve")
        }
        PaceStage::Unknown => "On pace".to_string(),
    }
}

fn pace_right_label(pace: &UsagePace, shows_headroom_hint: bool) -> Option<String> {
    if pace.will_last_to_reset {
        if shows_headroom_hint
            && pace.delta_percent < -15.0
            && pace.speed_multiplier_to_reset.is_some_and(|m| m >= 1.5)
        {
            Some("Lasts until reset | 1.5× headroom".to_string())
        } else {
            Some("Lasts until reset".to_string())
        }
    } else if let Some(eta_seconds) = pace.eta_seconds {
        let dur = format_countdown_duration(eta_seconds);
        if dur == "now" {
            Some("Runs out now".to_string())
        } else {
            Some(format!("Runs out in {dur}"))
        }
    } else {
        None
    }
}

fn format_countdown_duration(seconds: f64) -> String {
    let total_minutes = if seconds < 1.0 {
        0
    } else {
        (seconds / 60.0).ceil() as u64
    };

    if total_minutes == 0 {
        return "now".to_string();
    }

    let days = total_minutes / (24 * 60);
    let hours = (total_minutes / 60) % 24;
    let minutes = total_minutes % 60;

    if days > 0 {
        if hours > 0 {
            format!("{days}d {hours}h")
        } else if minutes > 0 {
            format!("{days}d {minutes}m")
        } else {
            format!("{days}d")
        }
    } else if hours > 0 {
        if minutes > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{hours}h")
        }
    } else {
        format!("{total_minutes}m")
    }
}
