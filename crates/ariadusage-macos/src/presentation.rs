// Ported from CodexBar Sources/CodexBar/IconRemainingResolver.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Turns protocol snapshots into what the status icon shows.

use ariadusage_protocol::metric::{Metric, MetricState};
use ariadusage_protocol::snapshot::{EngineSnapshot, ProviderSnapshot};
use ariadusage_protocol::usage::{RateWindow, StatusIndicator};

use crate::format;
use crate::icon::{IconInput, StatusLevel};

/// Display preferences that change what the icon meters show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IconPreferences {
    /// Fill meters with used instead of remaining percentages.
    pub show_used: bool,
}

/// Picks the provider the merged status item represents: the selected one
/// when it is still enabled, otherwise the first enabled provider.
pub fn icon_provider<'a>(
    snapshot: &'a EngineSnapshot,
    selected: Option<&str>,
) -> Option<&'a ProviderSnapshot> {
    let enabled = || snapshot.providers.iter().filter(|p| p.enabled);
    selected
        .and_then(|id| enabled().find(|p| p.id.as_str() == id))
        .or_else(|| enabled().next())
}

/// Resolves the icon input for one provider.
pub fn icon_input(provider: &ProviderSnapshot, prefs: IconPreferences) -> IconInput {
    let percent = |window: &Option<Metric<RateWindow>>| {
        window
            .as_ref()
            .and_then(displayable_value)
            .map(|w| meter_percent(w, prefs.show_used))
    };
    IconInput {
        primary: percent(&provider.windows.primary),
        secondary: percent(&provider.windows.secondary),
        credits_remaining: provider
            .credits
            .as_ref()
            .and_then(displayable_value)
            .map(|c| c.remaining),
        stale: provider.last_error.is_some() || has_stale_window(provider),
        status: provider
            .status
            .as_ref()
            .and_then(displayable_value)
            .map(status_level)
            .unwrap_or_default(),
        reserves_missing_secondary_lane: reserves_missing_secondary_lane(provider.id.as_str()),
    }
}

/// One informational line at the top of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderLine {
    pub provider_id: String,
    pub text: String,
}

/// Lines describing each enabled provider's primary window, or the engine
/// connection state when there is nothing to show yet.
pub fn provider_lines(
    snapshot: Option<&EngineSnapshot>,
    connected: bool,
    prefs: IconPreferences,
) -> Vec<ProviderLine> {
    let Some(snapshot) = snapshot.filter(|_| connected) else {
        return vec![ProviderLine {
            provider_id: String::new(),
            text: if connected {
                "Refreshing…".to_string()
            } else {
                "Waiting for the AriadUsage engine…".to_string()
            },
        }];
    };
    snapshot
        .providers
        .iter()
        .filter(|p| p.enabled)
        .map(|p| ProviderLine {
            provider_id: p.id.as_str().to_string(),
            text: format!("{}: {}", p.display_name, primary_summary(p, prefs)),
        })
        .collect()
}

fn primary_summary(provider: &ProviderSnapshot, prefs: IconPreferences) -> String {
    if let Some(error) = &provider.last_error {
        return error.message.clone();
    }
    match &provider.windows.primary {
        Some(metric) => match (metric.state, displayable_value(metric)) {
            (_, Some(window)) => {
                let used = window.used_percent.clamp(0.0, 100.0);
                format::usage_line(100.0 - used, used, prefs.show_used)
            }
            (MetricState::Loading, None) => "Refreshing…".to_string(),
            _ => "Not fetched yet".to_string(),
        },
        None => "Not fetched yet".to_string(),
    }
}

/// A metric's value when its state allows showing it; loading, unknown and
/// failed metrics never render as a number.
fn displayable_value<T>(metric: &Metric<T>) -> Option<&T> {
    match metric.state {
        MetricState::Value | MetricState::Stale => metric.value.as_ref(),
        _ => None,
    }
}

fn meter_percent(window: &RateWindow, show_used: bool) -> f64 {
    let used = window.used_percent.clamp(0.0, 100.0);
    if show_used { used } else { 100.0 - used }
}

fn has_stale_window(provider: &ProviderSnapshot) -> bool {
    [&provider.windows.primary, &provider.windows.secondary]
        .into_iter()
        .flatten()
        .any(|m| m.state == MetricState::Stale)
}

fn status_level(indicator: &StatusIndicator) -> StatusLevel {
    match indicator {
        StatusIndicator::None => StatusLevel::None,
        StatusIndicator::Minor => StatusLevel::Minor,
        StatusIndicator::Maintenance => StatusLevel::Maintenance,
        StatusIndicator::Major => StatusLevel::Major,
        StatusIndicator::Critical => StatusLevel::Critical,
        StatusIndicator::Unknown => StatusLevel::Unknown,
    }
}

/// Claude keeps an empty weekly lane when its weekly window is missing.
fn reserves_missing_secondary_lane(provider_id: &str) -> bool {
    provider_id == "claude"
}
