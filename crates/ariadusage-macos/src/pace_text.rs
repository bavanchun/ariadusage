// Ported from CodexBar Sources/CodexBar/UsagePaceText.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Pace labels under a usage bar: `7% in deficit · Runs out in 3d`.

use ariadusage_protocol::usage::{PaceStage, SessionEquivalentForecast};

use crate::format;

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
pub fn session_equivalent_detail(forecast: &SessionEquivalentForecast) -> SessionEquivalentDetail {
    let (displayed, unit) =
        if forecast.estimated_sessions.is_finite() && forecast.estimated_sessions > 0.0 {
            let bounded = forecast.estimated_sessions.min(1_000_000.0);
            let rounded = (bounded * 10.0).round() / 10.0;
            let text = if (rounded.fract()).abs() < 1e-9 {
                format::grouped_u64(rounded as u64)
            } else {
                format!("{rounded:.1}")
            };
            let unit = if rounded <= 1.0 {
                "session quota"
            } else {
                "session quotas"
            };
            (text, unit)
        } else {
            ("0".to_string(), "session quotas")
        };
    let left = format!("Est. {displayed} {unit} left");
    let right = match forecast.windows_until_reset {
        1 => "1 window until reset".to_string(),
        n => format!("{n} windows until reset"),
    };
    SessionEquivalentDetail { left, right }
}
