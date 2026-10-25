// Ported from CodexBar Sources/CodexBar/UsagePaceText.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Pace labels under a usage bar: `7% in deficit · Runs out in 3d`.

use ariadusage_protocol::usage::{PaceStage, RateWindow, SessionEquivalentForecast};
use jiff::Timestamp;

use crate::format::{self, DateTexts};

/// Minimum expected usage percentage before session/weekly pace is shown (`UsagePaceText.swift:160`).
pub const MINIMUM_EXPECTED_PERCENT: f64 = 3.0;

/// The pace facts the labels are built from (`UsagePace`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaceFacts {
    pub stage: PaceStage,
    /// Actual minus expected used percentage.
    pub delta_percent: f64,
    pub expected_used_percent: f64,
    pub will_last_to_reset: bool,
    pub eta_seconds: Option<f64>,
    /// Probability of running out before the reset, 0–1.
    pub run_out_probability: Option<f64>,
    /// How much faster usage could run and still last to the reset.
    pub speed_multiplier_to_reset: Option<f64>,
}

/// Which window the pace describes; it changes the run-out wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaceContext {
    Session,
    Weekly,
}

/// The two halves of a pace line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaceDetail {
    pub left: String,
    pub right: Option<String>,
}

impl PaceDetail {
    /// `Pace: <left> · <right>`, or `Pace: <left>` without a right half.
    pub fn summary(&self) -> String {
        match &self.right {
            Some(right) => format!("Pace: {} · {right}", self.left),
            None => format!("Pace: {}", self.left),
        }
    }
}

/// Builds the pace labels. `shows_headroom_hint` is the provider's pace
/// capability (Codex only at the baseline).
pub fn detail(pace: &PaceFacts, shows_headroom_hint: bool, context: PaceContext) -> PaceDetail {
    PaceDetail {
        left: left_label(pace),
        right: right_label(pace, shows_headroom_hint, context),
    }
}

fn left_label(pace: &PaceFacts) -> String {
    let delta = pace.delta_percent.abs().round() as i64;
    if delta == 0 {
        return "On pace".to_string();
    }
    match pace.stage {
        PaceStage::SlightlyAhead | PaceStage::Ahead | PaceStage::FarAhead => {
            format!("{delta}% in deficit")
        }
        PaceStage::SlightlyBehind | PaceStage::Behind | PaceStage::FarBehind => {
            format!("{delta}% in reserve")
        }
        PaceStage::OnTrack | PaceStage::Unknown => "On pace".to_string(),
    }
}

fn right_label(
    pace: &PaceFacts,
    shows_headroom_hint: bool,
    context: PaceContext,
) -> Option<String> {
    let eta_label = if pace.will_last_to_reset {
        Some(lasts_label(pace, shows_headroom_hint))
    } else {
        pace.eta_seconds.map(|seconds| {
            let eta = duration_text(seconds);
            match (context, eta.as_str()) {
                (PaceContext::Session, "now") => "Projected empty now".to_string(),
                (PaceContext::Session, _) => format!("Projected empty in {eta}"),
                (PaceContext::Weekly, "now") => "Runs out now".to_string(),
                (PaceContext::Weekly, _) => format!("Runs out in {eta}"),
            }
        })
    };
    let Some(probability) = pace.run_out_probability else {
        return eta_label;
    };
    let risk = format!("({}% risk)", rounded_risk_percent(probability));
    Some(match eta_label {
        Some(eta) => format!("{eta} {risk}"),
        None => risk,
    })
}

fn lasts_label(pace: &PaceFacts, shows_headroom_hint: bool) -> String {
    let headroom = shows_headroom_hint
        && pace.delta_percent < -15.0
        && pace.speed_multiplier_to_reset.is_some_and(|m| m >= 1.5);
    if headroom {
        "Lasts until reset · 1.5× headroom".to_string()
    } else {
        "Lasts until reset".to_string()
    }
}

fn duration_text(seconds: f64) -> String {
    let countdown = format::reset_countdown_description(seconds);
    match countdown.strip_prefix("in ") {
        Some(rest) => rest.to_string(),
        None => countdown,
    }
}

/// Risk rounded to the nearest 5 %.
fn rounded_risk_percent(probability: f64) -> i64 {
    let percent = probability.clamp(0.0, 1.0) * 100.0;
    ((percent / 5.0).round() * 5.0) as i64
}

/// The two halves of a session-equivalent forecast line
/// (`UsagePaceText.SessionEquivalentDetail`, `UsagePaceText.swift:12-16,39-74`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEquivalentDetail {
    pub left: String,
    pub right: String,
}

impl SessionEquivalentDetail {
    /// `<left> · <right>`.
    pub fn summary(&self) -> String {
        format!("{} · {}", self.left, self.right)
    }
}

/// Builds the session-equivalent forecast labels
/// (`UsagePaceText.sessionEquivalentDetail`, `UsagePaceText.swift:39-74`).
pub fn session_equivalent_detail(
    forecast: &SessionEquivalentForecast,
    dates: &dyn DateTexts,
) -> SessionEquivalentDetail {
    let (displayed, unit) =
        if forecast.estimated_sessions.is_finite() && forecast.estimated_sessions > 0.0 {
            let bounded = forecast.estimated_sessions.min(1_000_000.0);
            let rounded = (bounded * 10.0).round() / 10.0;
            let text = dates.format_one_decimal(rounded);
            let unit = if rounded > 0.0 && rounded <= 1.0 {
                "session quota"
            } else {
                "session quotas"
            };
            (text, unit)
        } else {
            ("0".to_string(), "session quotas")
        };
    let left = format!("Est. {displayed} {unit} left");
    let right = format!("{} windows until reset", forecast.windows_until_reset);
    SessionEquivalentDetail { left, right }
}

/// Returns whether the provider's pace capability shows the `1.5× headroom` hint
/// (`UsagePaceText.swift:120-127`, Codex only at the baseline).
pub fn shows_headroom_hint(provider_id: &str) -> bool {
    provider_id == "codex"
}

/// Returns whether `window` supports session-style pace for `provider_id`
/// (`ProviderPaceCapability.supportsSessionPace`, `UsagePaceText.swift:155-157`).
pub fn supports_session_pace(provider_id: &str, window: &RateWindow) -> bool {
    match provider_id {
        "claude" => true,
        // Codex suppresses weekly (10080) and monthly (43200) windows on the session lane,
        // while keeping 5h (300) and fallback/unknown durations (`CodexProviderDescriptor.swift:56-63`).
        "codex" => !matches!(window.window_minutes, Some(10_080 | 43_200)),
        // Antigravity requires an explicit 5h (300-minute) window (`AntigravityProviderDescriptor.swift:71`).
        "antigravity" => window.window_minutes == Some(300),
        _ => false,
    }
}

/// Computes session pace facts for `window` when supported by `provider_id`
/// (`UsagePaceText.sessionPace`, `UsagePaceText.swift:155-162`).
pub fn session_pace(provider_id: &str, window: &RateWindow, now: Timestamp) -> Option<PaceFacts> {
    if !supports_session_pace(provider_id, window) {
        return None;
    }
    if (100.0 - window.used_percent).max(0.0) <= 0.0 {
        return None;
    }
    let resets_at = window.resets_at?;
    let minutes = window.window_minutes.unwrap_or(300);
    if minutes == 0 {
        return None;
    }
    let duration = f64::from(minutes) * 60.0;
    let time_until_reset = now.duration_until(resets_at).as_secs_f64();
    if time_until_reset <= 0.0 || time_until_reset > duration {
        return None;
    }
    let elapsed = (duration - time_until_reset).clamp(0.0, duration);
    let expected = ((elapsed / duration) * 100.0).clamp(0.0, 100.0);
    if elapsed == 0.0 && window.used_percent > 0.0 {
        return None;
    }
    if expected < MINIMUM_EXPECTED_PERCENT {
        return None;
    }
    let actual = window.used_percent.clamp(0.0, 100.0);
    let delta = actual - expected;
    let stage = stage_for_delta(delta);

    let mut eta_seconds = None;
    let mut will_last_to_reset = false;
    let mut speed_multiplier_to_reset = None;
    if elapsed > 0.0 && actual > 0.0 {
        let rate = actual / elapsed;
        if rate > 0.0 {
            let remaining = (100.0 - actual).max(0.0);
            let candidate = remaining / rate;
            if candidate >= time_until_reset {
                will_last_to_reset = true;
                if time_until_reset > 0.0 {
                    let required_rate = remaining / time_until_reset;
                    let multiplier = required_rate / rate;
                    if multiplier.is_finite() && multiplier > 0.0 {
                        speed_multiplier_to_reset = Some(multiplier);
                    }
                }
            } else {
                eta_seconds = Some(candidate);
            }
        }
    } else if elapsed > 0.0 && actual == 0.0 {
        will_last_to_reset = true;
    }

    Some(PaceFacts {
        stage,
        delta_percent: delta,
        expected_used_percent: expected,
        will_last_to_reset,
        eta_seconds,
        run_out_probability: None,
        speed_multiplier_to_reset,
    })
}

/// Builds the session pace detail for `window` (`UsagePaceText.sessionDetail`, `UsagePaceText.swift:164-171`).
pub fn session_detail(
    provider_id: &str,
    window: &RateWindow,
    now: Timestamp,
) -> Option<PaceDetail> {
    let pace = session_pace(provider_id, window, now)?;
    Some(detail(
        &pace,
        shows_headroom_hint(provider_id),
        PaceContext::Session,
    ))
}

/// Builds the single-line session pace summary (`UsagePaceText.sessionSummary`, `UsagePaceText.swift:173-179`).
pub fn session_summary(provider_id: &str, window: &RateWindow, now: Timestamp) -> Option<String> {
    Some(session_detail(provider_id, window, now)?.summary())
}

fn stage_for_delta(delta: f64) -> PaceStage {
    let abs_delta = delta.abs();
    if abs_delta <= 2.0 {
        PaceStage::OnTrack
    } else if abs_delta <= 6.0 {
        if delta >= 0.0 {
            PaceStage::SlightlyAhead
        } else {
            PaceStage::SlightlyBehind
        }
    } else if abs_delta <= 12.0 {
        if delta >= 0.0 {
            PaceStage::Ahead
        } else {
            PaceStage::Behind
        }
    } else if delta >= 0.0 {
        PaceStage::FarAhead
    } else {
        PaceStage::FarBehind
    }
}
