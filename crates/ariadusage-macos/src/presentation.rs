// Ported from CodexBar Sources/CodexBar/IconRemainingResolver.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Turns protocol snapshots into what the status icon shows.

use ariadusage_protocol::metric::{Metric, MetricState};
use ariadusage_protocol::snapshot::{EngineSnapshot, ProviderSnapshot};
use ariadusage_protocol::usage::{RateWindow, StatusIndicator};

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
            .map(|s| status_level(&s.indicator))
            .unwrap_or_default(),
        reserves_missing_secondary_lane: reserves_missing_secondary_lane(provider.id.as_str()),
    }
}

/// The menu line describing the engine connection while there is no
/// snapshot to show a card for.
pub fn connection_status(snapshot: Option<&EngineSnapshot>, connected: bool) -> Option<String> {
    if !connected {
        Some("Waiting for the AriadUsage engine…".to_string())
    } else if snapshot.is_none() {
        Some("Refreshing…".to_string())
    } else {
        None
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
