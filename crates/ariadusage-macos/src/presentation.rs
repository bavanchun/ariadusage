// Ported from CodexBar Sources/CodexBar/IconRemainingResolver.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuBarMetricWindowResolver.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/AutomaticQuotaSelection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UnifiedIconSource.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UnifiedIconProviderResolver.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeMonthlySpendWindow.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeScopedWeeklyLimitMapper.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Copilot/CopilotProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Cursor/CursorProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Doubao/DoubaoProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Gemini/GeminiProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/LiteLLM/LiteLLMProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Mistral/MistralProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Perplexity/PerplexityProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Warp/WarpProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/UsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Turns protocol snapshots into what the status icon and menu-bar metrics show:
//! per-provider `iconWindows` resolvers, `IconRemainingResolver`,
//! `MenuBarMetricWindowResolver`, `AutomaticQuotaSelection`, and
//! `UnifiedIconContext`.

use std::collections::{HashMap, HashSet};

use ariadusage_protocol::metric::{Metric, MetricState};
use ariadusage_protocol::snapshot::{EngineSnapshot, ProviderSnapshot};
use ariadusage_protocol::usage::{NamedWindow, RateWindow, StatusIndicator};
use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::icon::{IconInput, IconStyle, QuotaLayoutPolicy, StatusLevel};

/// Smallest non-zero percent used in `show_used` mode for Warp's unused bonus
/// lane so `IconRenderer`'s `<= 0` check does not collapse it into the
/// missing-secondary layout (`IconRemainingResolver.visibleZeroPercent`).
pub const VISIBLE_ZERO_PERCENT: f64 = 0.1;

const ANTIGRAVITY_QUOTA_SUMMARY_PREFIX: &str = "antigravity-quota-summary-";
const ANTIGRAVITY_COMPACT_FALLBACK_PREFIX: &str = "antigravity-compact-fallback-";
const CLAUDE_SCOPED_WEEKLY_PREFIX: &str = "claude-weekly-scoped-";
const CURSOR_GROK_BOT_EXTRA_ID: &str = "cursor-grok-bot";
const MISTRAL_MONTHLY_PLAN_EXTRA_ID: &str = "mistral-monthly-plan";
const DOUBAO_AGENT_SESSION_EXTRA_ID: &str = "doubao-agent-session";
const DOUBAO_AGENT_WEEKLY_EXTRA_ID: &str = "doubao-agent-weekly";
const MONTHLY_SENTINEL_MINUTES: u32 = 30 * 24 * 60;

/// Menu-bar metric window selection preference (`MenuBarMetricPreference`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuBarMetricPreference {
    #[default]
    Automatic,
    Primary,
    Secondary,
    Tertiary,
    PrimaryAndSecondary,
    Average,
    ExtraUsage,
    MonthlyPlan,
}

/// Source for the unified (merged) status item icon (`UnifiedIconSource`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnifiedIconSource {
    #[default]
    CurrentSelection,
    HighestUsage,
    FrontmostApp,
}

impl UnifiedIconSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::CurrentSelection => "Selected provider",
            Self::HighestUsage => "Most constrained",
            Self::FrontmostApp => "Frontmost app",
        }
    }
}

/// Pure resolver context for the merged status item's active provider (`UnifiedIconContext`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnifiedIconContext<'a> {
    pub source: UnifiedIconSource,
    pub focused_provider: Option<&'a str>,
    pub is_merged_menu_open: bool,
    pub is_stacked: bool,
}

impl<'a> UnifiedIconContext<'a> {
    /// Resolves the provider ID for the merged icon (`UnifiedIconContext.resolve`).
    pub fn resolve(
        &self,
        fallback: &'a str,
        merge_icons: bool,
        enabled_providers: &HashSet<&'a str>,
    ) -> &'a str {
        if merge_icons
            && !self.is_stacked
            && !self.is_merged_menu_open
            && self.source == UnifiedIconSource::FrontmostApp
            && let Some(focused) = self.focused_provider
            && enabled_providers.contains(focused)
        {
            return focused;
        }
        fallback
    }

    /// Returns `selected` if it is currently enabled (`UnifiedIconContext.currentSelectionProvider`).
    pub fn current_selection_provider(
        selected: Option<&'a str>,
        enabled: &HashSet<&'a str>,
    ) -> Option<&'a str> {
        selected.filter(|id| enabled.contains(id))
    }
}

/// Display preferences that change what the icon meters show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IconPreferences {
    /// Fill meters with used instead of remaining percentages.
    pub show_used: bool,
    /// Suppress critter decorations and draw plain rounded meter bars (`MenuBarIconStyle.bars`).
    pub hide_critters: bool,
    /// Selected menu-bar metric preference.
    pub metric_preference: MenuBarMetricPreference,
    /// Optional reference timestamp for reset-expiry checks.
    pub now: Option<Timestamp>,
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

/// Picks the provider for the merged status item across all `UnifiedIconSource`
/// modes (`StatusItemController.primaryProviderForUnifiedIcon`).
pub fn primary_provider_for_unified_icon<'a>(
    snapshot: &'a EngineSnapshot,
    selected: Option<&'a str>,
    source: UnifiedIconSource,
    focused_provider: Option<&'a str>,
    is_merged_menu_open: bool,
    is_stacked: bool,
    now: Option<Timestamp>,
) -> Option<&'a ProviderSnapshot> {
    let enabled_list: Vec<&'a ProviderSnapshot> =
        snapshot.providers.iter().filter(|p| p.enabled).collect();
    if enabled_list.is_empty() {
        return None;
    }
    let enabled_ids: HashSet<&'a str> = enabled_list.iter().map(|p| p.id.as_str()).collect();

    let fallback_id = if source == UnifiedIconSource::HighestUsage {
        provider_with_highest_usage(&enabled_list, now)
            .or_else(|| enabled_list.first().copied())
            .map(|p| p.id.as_str())?
    } else if let Some(sel) = UnifiedIconContext::current_selection_provider(selected, &enabled_ids)
    {
        sel
    } else {
        enabled_list.first().map(|p| p.id.as_str())?
    };

    let ctx = UnifiedIconContext {
        source,
        focused_provider,
        is_merged_menu_open,
        is_stacked,
    };
    let resolved_id = ctx.resolve(fallback_id, true, &enabled_ids);
    enabled_list
        .into_iter()
        .find(|p| p.id.as_str() == resolved_id)
}

fn provider_with_highest_usage<'a>(
    candidates: &[&'a ProviderSnapshot],
    now: Option<Timestamp>,
) -> Option<&'a ProviderSnapshot> {
    let mut best: Option<(&'a ProviderSnapshot, Selection)> = None;
    for provider in candidates {
        if provider.id.as_str() == "antigravity"
            && has_antigravity_quota_summary(provider)
            && !antigravity_quota_summary_families_are_all_blocked(provider)
        {
            // When Antigravity has quota summary rows and at least one family is still usable,
            // do not let a single exhausted family claim 100% blocked status over all families.
            if let Some(w) = resolve_menu_bar_metric_window(
                MenuBarMetricPreference::Automatic,
                provider,
                false,
                false,
                now,
            ) && let Some(sel) = make_selection(
                Candidate {
                    window: w,
                    slot: None,
                },
                provider.id.as_str(),
                now,
            ) && best.as_ref().is_none_or(|(_, b)| precedes(b, &sel))
            {
                best = Some((*provider, sel));
            }
            continue;
        }
        if let Some(w) = resolve_menu_bar_metric_window(
            MenuBarMetricPreference::Automatic,
            provider,
            false,
            true,
            now,
        ) && let Some(sel) = make_selection(
            Candidate {
                window: w,
                slot: None,
            },
            provider.id.as_str(),
            now,
        ) && best.as_ref().is_none_or(|(_, b)| precedes(b, &sel))
        {
            best = Some((*provider, sel));
        }
    }
    best.map(|(p, _)| p)
}

/// Resolves the icon input for one provider.
pub fn icon_input(provider: &ProviderSnapshot, prefs: IconPreferences) -> IconInput {
    icon_input_with_override(provider, prefs, None)
}

/// Resolves the icon input for one provider with an optional secondary window
/// override ID (used by Copilot budget overrides).
pub fn icon_input_with_override(
    provider: &ProviderSnapshot,
    prefs: IconPreferences,
    secondary_override_window_id: Option<&str>,
) -> IconInput {
    let id = provider.id.as_str();
    let style = IconStyle::from_provider_id(id);
    let (primary, secondary) = if prefs.metric_preference == MenuBarMetricPreference::MonthlyPlan {
        let primary = resolve_menu_bar_metric_window(
            MenuBarMetricPreference::MonthlyPlan,
            provider,
            false,
            false,
            prefs.now,
        )
        .map(|w| meter_percent(&w, prefs.show_used));
        (primary, None)
    } else if id == "mistral" {
        (None, None)
    } else {
        resolved_icon_percents_for_style(
            provider,
            style,
            prefs.show_used,
            secondary_override_window_id,
            prefs.now,
        )
    };

    let policy =
        quota_layout_policy_for_provider(provider, secondary_override_window_id, prefs.now);

    IconInput {
        primary,
        secondary,
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
        reserves_missing_secondary_lane: policy.reserves_missing_secondary_lane,
        treats_exhausted_secondary_as_missing: policy.treats_exhausted_secondary_as_missing,
        style,
        hide_critters: prefs.hide_critters,
        blink: 0.0,
        wiggle: 0.0,
        tilt: 0.0,
    }
}

/// Resolves the `QuotaLayoutPolicy` for `provider` and its snapshot
/// (`IconRenderer.QuotaLayoutPolicy.provider(_:snapshot:)`).
pub fn quota_layout_policy_for_provider(
    provider: &ProviderSnapshot,
    secondary_override_window_id: Option<&str>,
    now: Option<Timestamp>,
) -> QuotaLayoutPolicy {
    let id = provider.id.as_str();
    let mut reserves_missing_secondary_lane = id == "claude";
    if reserves_missing_secondary_lane {
        let raw_primary = provider
            .windows
            .primary
            .as_ref()
            .and_then(displayable_value);
        let raw_secondary = provider
            .windows
            .secondary
            .as_ref()
            .and_then(displayable_value);
        let (resolved_primary, resolved_secondary) =
            resolved_icon_windows(provider, secondary_override_window_id, now);
        if raw_primary.is_none()
            && raw_secondary.is_none()
            && resolved_primary.is_some()
            && resolved_secondary.is_none()
        {
            reserves_missing_secondary_lane = false;
        }
    }
    QuotaLayoutPolicy {
        reserves_missing_secondary_lane,
        treats_exhausted_secondary_as_missing: id == "warp",
    }
}

/// Resolves the primary and secondary `RateWindow`s that drive the status icon
/// (`IconRemainingResolver.resolvedWindows`).
pub fn resolved_icon_windows(
    provider: &ProviderSnapshot,
    secondary_override_window_id: Option<&str>,
    now: Option<Timestamp>,
) -> (Option<RateWindow>, Option<RateWindow>) {
    match provider.id.as_str() {
        "codex" => codex_icon_windows(provider, now),
        "claude" => claude_icon_windows(provider),
        "antigravity" => antigravity_icon_windows(provider),
        "copilot" => copilot_icon_windows(provider, secondary_override_window_id),
        "doubao" => doubao_icon_windows(provider),
        "perplexity" => perplexity_icon_windows(provider),
        _ => (
            provider
                .windows
                .primary
                .as_ref()
                .and_then(displayable_value)
                .cloned(),
            provider
                .windows
                .secondary
                .as_ref()
                .and_then(displayable_value)
                .cloned(),
        ),
    }
}

/// Resolves the primary and secondary fill percentages for the status icon
/// (`IconRemainingResolver.resolvedPercents`).
pub fn resolved_icon_percents(
    provider: &ProviderSnapshot,
    show_used: bool,
    secondary_override_window_id: Option<&str>,
    now: Option<Timestamp>,
) -> (Option<f64>, Option<f64>) {
    let style = IconStyle::from_provider_id(provider.id.as_str());
    resolved_icon_percents_for_style(
        provider,
        style,
        show_used,
        secondary_override_window_id,
        now,
    )
}

/// Resolves the primary and secondary fill percentages using an explicit `IconStyle`
/// (`IconRemainingResolver.resolvedPercents(snapshot:style:showUsed:secondaryOverrideWindowID:now:)`).
pub fn resolved_icon_percents_for_style(
    provider: &ProviderSnapshot,
    style: IconStyle,
    show_used: bool,
    secondary_override_window_id: Option<&str>,
    now: Option<Timestamp>,
) -> (Option<f64>, Option<f64>) {
    let (primary, secondary) = resolved_icon_windows(provider, secondary_override_window_id, now);
    if show_used {
        let primary_pct = primary.as_ref().map(|w| w.used_percent.clamp(0.0, 100.0));
        let treats_exhausted_as_missing = style
            .quota_layout_policy()
            .treats_exhausted_secondary_as_missing
            || provider.id.as_str() == "warp";
        let secondary_pct = if treats_exhausted_as_missing {
            secondary.as_ref().map(|w| {
                if remaining_percent(w) > 0.0 {
                    w.used_percent.clamp(0.0, 100.0).max(VISIBLE_ZERO_PERCENT)
                } else {
                    0.0
                }
            })
        } else {
            secondary.as_ref().map(|w| w.used_percent.clamp(0.0, 100.0))
        };
        (primary_pct, secondary_pct)
    } else {
        (
            primary.as_ref().map(remaining_percent),
            secondary.as_ref().map(remaining_percent),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexLaneKind {
    Session,
    Weekly,
    Monthly,
}

fn classify_codex_lane(window: &RateWindow, fallback: CodexLaneKind) -> CodexLaneKind {
    match window.window_minutes {
        Some(300) => CodexLaneKind::Session,
        Some(10080) => CodexLaneKind::Weekly,
        Some(43200) => CodexLaneKind::Monthly,
        _ => fallback,
    }
}

fn codex_icon_windows(
    provider: &ProviderSnapshot,
    now: Option<Timestamp>,
) -> (Option<RateWindow>, Option<RateWindow>) {
    let raw_primary = provider
        .windows
        .primary
        .as_ref()
        .and_then(displayable_value)
        .cloned();
    let raw_secondary = provider
        .windows
        .secondary
        .as_ref()
        .and_then(displayable_value)
        .cloned();

    let mut lanes: Vec<(CodexLaneKind, RateWindow)> = Vec::with_capacity(2);
    if let Some(w) = raw_primary {
        let kind = classify_codex_lane(&w, CodexLaneKind::Session);
        lanes.push((kind, w));
    }
    if let Some(w) = raw_secondary {
        let kind = classify_codex_lane(&w, CodexLaneKind::Weekly);
        if !lanes.iter().any(|(k, _)| *k == kind) {
            lanes.push((kind, w));
        }
    }

    let is_expired =
        |w: &RateWindow| -> bool { now.is_some_and(|n| w.resets_at.is_some_and(|r| r <= n)) };

    let weekly_exhausted_active = lanes
        .iter()
        .any(|(k, w)| *k == CodexLaneKind::Weekly && remaining_percent(w) <= 0.0 && !is_expired(w));

    let mut visible: Vec<RateWindow> = Vec::with_capacity(2);
    for (kind, mut window) in lanes {
        if remaining_percent(&window) <= 0.0 && is_expired(&window) {
            continue;
        }
        if kind == CodexLaneKind::Session && weekly_exhausted_active {
            window.used_percent = 100.0;
        }
        visible.push(window);
    }

    (visible.first().cloned(), visible.get(1).cloned())
}

fn claude_icon_windows(provider: &ProviderSnapshot) -> (Option<RateWindow>, Option<RateWindow>) {
    let raw_primary = provider
        .windows
        .primary
        .as_ref()
        .and_then(displayable_value)
        .cloned();
    let raw_secondary = provider
        .windows
        .secondary
        .as_ref()
        .and_then(displayable_value)
        .cloned();
    let raw_tertiary = provider
        .windows
        .tertiary
        .as_ref()
        .and_then(displayable_value);

    if raw_primary.is_none()
        && raw_secondary.is_none()
        && raw_tertiary.is_none()
        && let Some(fallback) =
            claude_scoped_weekly_window(provider).or_else(|| claude_monthly_spend_window(provider))
    {
        return (Some(fallback), None);
    }
    (raw_primary, raw_secondary)
}

/// Resolves the most constrained `claude-weekly-scoped-*` extra window
/// (`UsageSnapshot.claudeScopedWeeklyWindow`, `ClaudeScopedWeeklyLimitMapper.swift:84-89`).
pub fn claude_scoped_weekly_window(provider: &ProviderSnapshot) -> Option<RateWindow> {
    provider
        .windows
        .extra
        .iter()
        .filter(|w| w.id.starts_with(CLAUDE_SCOPED_WEEKLY_PREFIX))
        .filter_map(|w| displayable_value(&w.window))
        .max_by(|a, b| {
            a.used_percent
                .partial_cmp(&b.used_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned()
}

/// Synthesizes the monthly spend `RateWindow` for Claude accounts with a positive
/// spend cap (`UsageSnapshot.claudeMonthlySpendWindow`, `ClaudeMonthlySpendWindow.swift:5-18`).
pub fn claude_monthly_spend_window(provider: &ProviderSnapshot) -> Option<RateWindow> {
    let login_method = provider
        .identity
        .as_ref()
        .and_then(displayable_value)
        .and_then(|id| id.login_method.as_deref())
        .map(str::trim);
    if login_method.is_some_and(|m| m.eq_ignore_ascii_case("Admin API")) {
        return None;
    }
    let cost = provider.cost.as_ref().and_then(displayable_value)?;
    let limit = cost.limit?;
    if !limit.is_finite() || limit <= 0.0 || !cost.used.is_finite() || cost.used < 0.0 {
        return None;
    }
    let used_percent = (cost.used / limit * 100.0).clamp(0.0, 100.0);
    let resets_at = cost
        .resets_at
        .or_else(|| cost.balance_updated_at.and_then(end_of_utc_month));
    let mut w = RateWindow::new(used_percent);
    w.window_minutes = Some(MONTHLY_SENTINEL_MINUTES);
    w.resets_at = resets_at;
    Some(w)
}

/// Returns the UTC start of the next calendar month (`ClaudeMonthlySpendWindow.endOfUTCMonth`).
pub fn end_of_utc_month(ts: Timestamp) -> Option<Timestamp> {
    let zdt = ts.to_zoned(TimeZone::UTC);
    let year = zdt.year();
    let month = zdt.month();
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next_date = jiff::civil::date(next_year, next_month, 1).at(0, 0, 0, 0);
    next_date
        .to_zoned(TimeZone::UTC)
        .ok()
        .map(|z| z.timestamp())
}

/// Computes the exact number of minutes in the calendar month ending at `ending_at` in UTC
/// (`ProviderPaceCapability.inferredMonthlyWindowMinutes(endingAt:)`).
pub fn inferred_monthly_window_minutes(ending_at: Timestamp) -> Option<u32> {
    let zdt = ending_at.to_zoned(TimeZone::UTC);
    let year = zdt.year();
    let month = zdt.month();
    let (prev_year, prev_month) = if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    };
    let prev_days = days_in_month(prev_year, prev_month);
    let day = zdt.day().min(prev_days);
    let start_civil = jiff::civil::date(prev_year, prev_month, day).at(
        zdt.hour(),
        zdt.minute(),
        zdt.second(),
        zdt.subsec_nanosecond(),
    );
    let start_ts = start_civil.to_zoned(TimeZone::UTC).ok()?.timestamp();
    let mins = (ending_at.as_second() - start_ts.as_second()) / 60;
    u32::try_from(mins).ok().filter(|&m| m > 0)
}

/// Resolves a `RateWindow` for Claude pace calculations, replacing the 43200-minute
/// monthly sentinel with the actual UTC calendar month duration (`resolvedResetWindowForPace`).
pub fn resolved_claude_reset_window_for_pace(window: &RateWindow) -> RateWindow {
    if window.window_minutes == Some(MONTHLY_SENTINEL_MINUTES)
        && let Some(resets_at) = window.resets_at
        && let Some(minutes) = inferred_monthly_window_minutes(resets_at)
    {
        let mut updated = window.clone();
        updated.window_minutes = Some(minutes);
        return updated;
    }
    window.clone()
}

fn days_in_month(year: i16, month: i8) -> i8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
            if is_leap { 29 } else { 28 }
        }
        _ => 30,
    }
}

fn antigravity_icon_windows(
    provider: &ProviderSnapshot,
) -> (Option<RateWindow>, Option<RateWindow>) {
    let windows: Vec<(&str, &RateWindow)> = provider
        .windows
        .extra
        .iter()
        .filter(|w| w.id.starts_with(ANTIGRAVITY_QUOTA_SUMMARY_PREFIX))
        .filter_map(|w| displayable_value(&w.window).map(|rw| (w.id.as_str(), rw)))
        .collect();
    if windows.is_empty() {
        return (None, None);
    }
    (
        most_constrained_antigravity(&windows, 300),
        most_constrained_antigravity(&windows, 10080),
    )
}

fn most_constrained_antigravity(
    windows: &[(&str, &RateWindow)],
    minutes: u32,
) -> Option<RateWindow> {
    windows
        .iter()
        .filter(|(_, w)| w.window_minutes == Some(minutes))
        .max_by(|(lhs_id, lhs_w), (rhs_id, rhs_w)| {
            match lhs_w
                .used_percent
                .partial_cmp(&rhs_w.used_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
            {
                std::cmp::Ordering::Equal => rhs_id.cmp(lhs_id),
                ord => ord,
            }
        })
        .map(|(_, w)| (*w).clone())
}

fn copilot_icon_windows(
    provider: &ProviderSnapshot,
    secondary_override_window_id: Option<&str>,
) -> (Option<RateWindow>, Option<RateWindow>) {
    let primary = provider
        .windows
        .primary
        .as_ref()
        .and_then(displayable_value)
        .cloned();
    let secondary = secondary_override_window_id
        .and_then(|id| extra_window_by_id(provider, id))
        .or_else(|| {
            provider
                .windows
                .secondary
                .as_ref()
                .and_then(displayable_value)
                .cloned()
        });
    (primary, secondary)
}

fn doubao_icon_windows(provider: &ProviderSnapshot) -> (Option<RateWindow>, Option<RateWindow>) {
    let primary = provider
        .windows
        .primary
        .as_ref()
        .and_then(displayable_value)
        .cloned()
        .or_else(|| extra_window_by_id(provider, DOUBAO_AGENT_SESSION_EXTRA_ID));
    let secondary = provider
        .windows
        .secondary
        .as_ref()
        .and_then(displayable_value)
        .cloned()
        .or_else(|| extra_window_by_id(provider, DOUBAO_AGENT_WEEKLY_EXTRA_ID));
    (primary, secondary)
}

fn perplexity_icon_windows(
    provider: &ProviderSnapshot,
) -> (Option<RateWindow>, Option<RateWindow>) {
    let primary = provider
        .windows
        .primary
        .as_ref()
        .and_then(displayable_value)
        .cloned();
    let secondary = provider
        .windows
        .secondary
        .as_ref()
        .and_then(displayable_value)
        .cloned();
    let tertiary = provider
        .windows
        .tertiary
        .as_ref()
        .and_then(displayable_value)
        .cloned();

    let fallback_candidates: Vec<RateWindow> =
        [tertiary, secondary].into_iter().flatten().collect();
    let mut ordered_fallbacks: Vec<RateWindow> = fallback_candidates
        .iter()
        .filter(|w| remaining_percent(w) > 0.0)
        .cloned()
        .collect();
    ordered_fallbacks.extend(
        fallback_candidates
            .into_iter()
            .filter(|w| remaining_percent(w) <= 0.0),
    );

    let ordered: Vec<RateWindow> = if primary.as_ref().is_some_and(|w| remaining_percent(w) > 0.0)
        || ordered_fallbacks.is_empty()
    {
        primary.into_iter().chain(ordered_fallbacks).collect()
    } else {
        ordered_fallbacks.into_iter().chain(primary).collect()
    };
    (ordered.first().cloned(), ordered.get(1).cloned())
}

fn extra_window_by_id(provider: &ProviderSnapshot, id: &str) -> Option<RateWindow> {
    provider
        .windows
        .extra
        .iter()
        .find(|w| w.id == id)
        .and_then(|w| displayable_value(&w.window))
        .cloned()
}

/// Resolves the single `RateWindow` for a menu-bar metric preference
/// (`MenuBarMetricWindowResolver.rateWindow`).
pub fn resolve_menu_bar_metric_window(
    preference: MenuBarMetricPreference,
    provider: &ProviderSnapshot,
    supports_average: bool,
    antigravity_prioritize_exhausted_quotas: bool,
    now: Option<Timestamp>,
) -> Option<RateWindow> {
    let id = provider.id.as_str();
    let primary = provider
        .windows
        .primary
        .as_ref()
        .and_then(displayable_value)
        .cloned();
    let secondary = provider
        .windows
        .secondary
        .as_ref()
        .and_then(displayable_value)
        .cloned();
    let tertiary = provider
        .windows
        .tertiary
        .as_ref()
        .and_then(displayable_value)
        .cloned();

    // 1. Provider-specific `menuBarWindow` overrides
    match id {
        "claude" => {
            if preference == MenuBarMetricPreference::ExtraUsage {
                return claude_monthly_spend_window(provider);
            }
            if preference == MenuBarMetricPreference::Automatic {
                let quotas: Vec<RateWindow> = [primary.clone(), secondary.clone()]
                    .into_iter()
                    .flatten()
                    .collect();
                if !quotas.is_empty() {
                    return select_automatic_candidates(
                        &candidates_from_windows(&quotas, provider),
                        id,
                        now,
                    );
                }
            }
            if matches!(
                preference,
                MenuBarMetricPreference::Automatic | MenuBarMetricPreference::PrimaryAndSecondary
            ) && primary.is_none()
                && secondary.is_none()
                && tertiary.is_none()
            {
                return claude_scoped_weekly_window(provider)
                    .or_else(|| claude_monthly_spend_window(provider));
            }
        }
        "antigravity" => match preference {
            MenuBarMetricPreference::Primary
            | MenuBarMetricPreference::Secondary
            | MenuBarMetricPreference::Tertiary => {
                let order = requested_lane_order(id, preference);
                return window_following_order(provider, &order).or_else(|| {
                    most_constrained_extra_window(provider, ANTIGRAVITY_COMPACT_FALLBACK_PREFIX)
                });
            }
            MenuBarMetricPreference::Average if !supports_average => {
                return primary.or(secondary).or(tertiary);
            }
            MenuBarMetricPreference::Automatic => {
                if antigravity_prioritize_exhausted_quotas
                    && let Some(ranked) = antigravity_quota_summary_ranking_window(provider, now)
                {
                    return Some(ranked);
                }
                return most_constrained_extra_window(provider, ANTIGRAVITY_QUOTA_SUMMARY_PREFIX)
                    .or_else(|| most_constrained_of(&[primary, secondary, tertiary]))
                    .or_else(|| {
                        most_constrained_extra_window(provider, ANTIGRAVITY_COMPACT_FALLBACK_PREFIX)
                    });
            }
            _ => {}
        },
        "cursor" if preference == MenuBarMetricPreference::Automatic => {
            let grok_bot = extra_window_by_id(provider, CURSOR_GROK_BOT_EXTRA_ID);
            let subquotas: Vec<RateWindow> = [secondary, tertiary, grok_bot]
                .into_iter()
                .flatten()
                .collect();
            let usable_subquotas: Vec<RateWindow> = subquotas
                .iter()
                .filter(|w| remaining_percent(w) > 0.0)
                .cloned()
                .collect();
            if let Some(total) = &primary
                && remaining_percent(total) <= 0.0
            {
                return Some(total.clone());
            }
            if !subquotas.is_empty() && usable_subquotas.is_empty() {
                return subquotas.into_iter().max_by(|a, b| {
                    a.used_percent
                        .partial_cmp(&b.used_percent)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            let candidates: Vec<RateWindow> = primary.into_iter().chain(usable_subquotas).collect();
            return select_automatic_candidates(
                &candidates_from_windows(&candidates, provider),
                id,
                now,
            );
        }
        "gemini" if preference == MenuBarMetricPreference::Automatic => {
            return exhausted_of(&[primary.clone(), secondary.clone(), tertiary.clone()])
                .or(primary)
                .or(secondary)
                .or(tertiary);
        }
        "litellm" if preference == MenuBarMetricPreference::Automatic => {
            return exhausted_of(&[primary.clone(), secondary.clone()])
                .or(secondary)
                .or(primary);
        }
        "mistral" => match preference {
            MenuBarMetricPreference::Automatic => return None,
            MenuBarMetricPreference::MonthlyPlan => {
                return extra_window_by_id(provider, MISTRAL_MONTHLY_PLAN_EXTRA_ID);
            }
            _ => {}
        },
        _ => {}
    }

    // 2. Standard fallback resolution (`MenuBarMetricWindowResolver.swift:35-57`)
    match preference {
        MenuBarMetricPreference::MonthlyPlan => None,
        MenuBarMetricPreference::ExtraUsage => spend_limit_window(provider),
        MenuBarMetricPreference::Primary
        | MenuBarMetricPreference::Secondary
        | MenuBarMetricPreference::Tertiary => {
            let order = requested_lane_order(id, preference);
            window_following_order(provider, &order)
        }
        MenuBarMetricPreference::PrimaryAndSecondary => most_constrained_of(&[primary, secondary]),
        MenuBarMetricPreference::Average => {
            if supports_average && let (Some(p), Some(s)) = (&primary, &secondary) {
                Some(RateWindow::new((p.used_percent + s.used_percent) / 2.0))
            } else {
                primary.or(secondary).or(tertiary)
            }
        }
        MenuBarMetricPreference::Automatic => {
            let candidates = candidates_from_snapshot(provider);
            select_automatic_candidates(&candidates, id, now)
        }
    }
}

/// Picks the binding supported quota-summary lane for the Antigravity exhausted-first opt-in
/// (`MenuBarMetricWindowResolver.antigravityQuotaSummaryRankingWindow`).
pub fn antigravity_quota_summary_ranking_window(
    provider: &ProviderSnapshot,
    now: Option<Timestamp>,
) -> Option<RateWindow> {
    provider
        .windows
        .extra
        .iter()
        .filter(|row| row.id.starts_with(ANTIGRAVITY_QUOTA_SUMMARY_PREFIX))
        .filter_map(|row| displayable_value(&row.window).map(|w| (row.id.as_str(), w)))
        .filter(|(_, w)| {
            w.used_percent.is_finite() && matches!(w.window_minutes, Some(300 | 10080))
        })
        .max_by(|(lhs_id, lhs_w), (rhs_id, rhs_w)| {
            match lhs_w
                .used_percent
                .partial_cmp(&rhs_w.used_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
            {
                std::cmp::Ordering::Equal => {}
                ord => return ord,
            }
            let future_reset = |w: &RateWindow| -> Option<Timestamp> {
                match (w.resets_at, now) {
                    (Some(r), Some(n)) if r > n => Some(r),
                    _ => None,
                }
            };
            let lhs_reset = future_reset(lhs_w);
            let rhs_reset = future_reset(rhs_w);
            match (lhs_reset, rhs_reset) {
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(l), Some(r)) if l != r => r.cmp(&l),
                _ => lhs_id.cmp(rhs_id),
            }
        })
        .map(|(_, w)| w.clone())
}

/// True only when every fully understood Antigravity quota family has an exhausted binding lane
/// (`MenuBarMetricWindowResolver.antigravityQuotaSummaryFamiliesAreAllBlocked`).
pub fn antigravity_quota_summary_families_are_all_blocked(provider: &ProviderSnapshot) -> bool {
    let summary_metrics: Vec<&NamedWindow> = provider
        .windows
        .extra
        .iter()
        .filter(|w| w.id.starts_with(ANTIGRAVITY_QUOTA_SUMMARY_PREFIX))
        .collect();
    if summary_metrics.is_empty() {
        return false;
    }

    let mut family_blocked: HashMap<String, bool> = HashMap::new();
    for named in summary_metrics {
        let Some(rw) = displayable_value(&named.window) else {
            return false;
        };
        if !rw.used_percent.is_finite() || !matches!(rw.window_minutes, Some(300 | 10080)) {
            return false;
        }
        let Some(family) = antigravity_quota_family(&named.id, rw.window_minutes) else {
            return false;
        };
        let entry = family_blocked.entry(family).or_insert(false);
        if rw.used_percent >= 100.0 {
            *entry = true;
        }
    }
    !family_blocked.is_empty() && family_blocked.values().all(|&blocked| blocked)
}

fn has_antigravity_quota_summary(provider: &ProviderSnapshot) -> bool {
    provider
        .windows
        .extra
        .iter()
        .any(|w| w.id.starts_with(ANTIGRAVITY_QUOTA_SUMMARY_PREFIX))
}

fn antigravity_quota_family(id: &str, window_minutes: Option<u32>) -> Option<String> {
    let suffix = id.strip_prefix(ANTIGRAVITY_QUOTA_SUMMARY_PREFIX)?;
    let mut normalized = suffix.to_ascii_lowercase().replace('_', "-");
    if let Some(stripped) = normalized.strip_suffix(" limit") {
        normalized = stripped.to_string();
    }
    let cadence_suffixes: &[&str] = match window_minutes {
        Some(300) => &["-session", "-5h", "-5-hour", "-five hour", "-five-hour"],
        Some(10080) => &["-weekly"],
        _ => return None,
    };
    let matched_suffix = cadence_suffixes
        .iter()
        .find(|&&s| normalized.ends_with(s))?;
    let family = normalized[..normalized.len() - matched_suffix.len()].trim();
    if family.is_empty()
        || family.starts_with('-')
        || family.ends_with('-')
        || !family
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return None;
    }
    Some(family.to_string())
}

fn spend_limit_window(provider: &ProviderSnapshot) -> Option<RateWindow> {
    if provider.id.as_str() == "claude" {
        return claude_monthly_spend_window(provider);
    }
    let cost = provider.cost.as_ref().and_then(displayable_value)?;
    let limit = cost.limit?;
    if !limit.is_finite() || limit <= 0.0 || !cost.used.is_finite() || cost.used < 0.0 {
        return None;
    }
    let mut w = RateWindow::new((cost.used / limit * 100.0).clamp(0.0, 100.0));
    w.resets_at = cost.resets_at;
    w.reset_description = cost.period.clone();
    Some(w)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsageLane {
    Primary,
    Secondary,
    Tertiary,
}

fn requested_lane_order(provider_id: &str, preference: MenuBarMetricPreference) -> Vec<UsageLane> {
    match preference {
        MenuBarMetricPreference::Primary => {
            vec![
                UsageLane::Primary,
                UsageLane::Secondary,
                UsageLane::Tertiary,
            ]
        }
        MenuBarMetricPreference::Secondary => {
            vec![
                UsageLane::Secondary,
                UsageLane::Primary,
                UsageLane::Tertiary,
            ]
        }
        MenuBarMetricPreference::Tertiary => {
            if matches!(provider_id, "antigravity" | "cursor") {
                vec![
                    UsageLane::Tertiary,
                    UsageLane::Secondary,
                    UsageLane::Primary,
                ]
            } else {
                vec![
                    UsageLane::Primary,
                    UsageLane::Secondary,
                    UsageLane::Tertiary,
                ]
            }
        }
        _ => Vec::new(),
    }
}

fn window_following_order(provider: &ProviderSnapshot, order: &[UsageLane]) -> Option<RateWindow> {
    for lane in order {
        let metric = match lane {
            UsageLane::Primary => &provider.windows.primary,
            UsageLane::Secondary => &provider.windows.secondary,
            UsageLane::Tertiary => &provider.windows.tertiary,
        };
        if let Some(w) = metric.as_ref().and_then(displayable_value) {
            return Some(w.clone());
        }
    }
    None
}

fn most_constrained_extra_window(provider: &ProviderSnapshot, prefix: &str) -> Option<RateWindow> {
    let windows: Vec<&RateWindow> = provider
        .windows
        .extra
        .iter()
        .filter(|w| w.id.starts_with(prefix))
        .filter_map(|w| displayable_value(&w.window))
        .collect();
    let usable: Vec<&RateWindow> = windows
        .iter()
        .copied()
        .filter(|w| w.used_percent < 100.0)
        .collect();
    let pool = if usable.is_empty() { windows } else { usable };
    pool.into_iter()
        .max_by(|a, b| {
            a.used_percent
                .partial_cmp(&b.used_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned()
}

fn most_constrained_of(windows: &[Option<RateWindow>]) -> Option<RateWindow> {
    let present: Vec<&RateWindow> = windows.iter().flatten().collect();
    if let Some(exhausted) = present.iter().find(|w| w.used_percent >= 100.0) {
        return Some((*exhausted).clone());
    }
    present
        .into_iter()
        .max_by(|a, b| {
            a.used_percent
                .partial_cmp(&b.used_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned()
}

fn exhausted_of(windows: &[Option<RateWindow>]) -> Option<RateWindow> {
    windows
        .iter()
        .flatten()
        .find(|w| w.used_percent >= 100.0)
        .cloned()
}

#[derive(Debug, Clone)]
struct Candidate {
    window: RateWindow,
    slot: Option<UsageLane>,
}

#[derive(Debug, Clone)]
struct Selection {
    window: RateWindow,
    eta_seconds: Option<f64>,
}

fn candidates_from_snapshot(provider: &ProviderSnapshot) -> Vec<Candidate> {
    [
        provider
            .windows
            .primary
            .as_ref()
            .and_then(displayable_value)
            .map(|w| Candidate {
                window: w.clone(),
                slot: Some(UsageLane::Primary),
            }),
        provider
            .windows
            .secondary
            .as_ref()
            .and_then(displayable_value)
            .map(|w| Candidate {
                window: w.clone(),
                slot: Some(UsageLane::Secondary),
            }),
        provider
            .windows
            .tertiary
            .as_ref()
            .and_then(displayable_value)
            .map(|w| Candidate {
                window: w.clone(),
                slot: Some(UsageLane::Tertiary),
            }),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn candidates_from_windows(windows: &[RateWindow], provider: &ProviderSnapshot) -> Vec<Candidate> {
    let snap_candidates = candidates_from_snapshot(provider);
    windows
        .iter()
        .map(|w| {
            let matches: Vec<UsageLane> = snap_candidates
                .iter()
                .filter(|c| &c.window == w)
                .filter_map(|c| c.slot)
                .collect();
            let slot = (matches.len() == 1).then(|| matches[0]);
            Candidate {
                window: w.clone(),
                slot,
            }
        })
        .collect()
}

fn select_automatic_candidates(
    candidates: &[Candidate],
    provider_id: &str,
    now: Option<Timestamp>,
) -> Option<RateWindow> {
    let mut best: Option<Selection> = None;
    for candidate in candidates {
        let Some(sel) = make_selection(candidate.clone(), provider_id, now) else {
            continue;
        };
        if best.as_ref().is_none_or(|b| precedes(b, &sel)) {
            best = Some(sel);
        }
    }
    best.map(|s| s.window)
}

fn make_selection(
    candidate: Candidate,
    provider_id: &str,
    now: Option<Timestamp>,
) -> Option<Selection> {
    let w = &candidate.window;
    if !w.used_percent.is_finite() || w.used_percent < 0.0 {
        return None;
    }
    let eta_seconds = now.and_then(|now_ts| {
        let effective_window = if provider_id == "claude" {
            resolved_claude_reset_window_for_pace(w)
        } else {
            w.clone()
        };
        let duration_mins = effective_window.window_minutes?;
        let resets_at = effective_window.resets_at?;
        let duration_secs = f64::from(duration_mins) * 60.0;
        let time_until_reset = (resets_at.as_second() - now_ts.as_second()) as f64;
        if time_until_reset <= 0.0 || duration_secs <= 0.0 || time_until_reset > duration_secs {
            return None;
        }
        let elapsed = (duration_secs - time_until_reset).clamp(0.0, duration_secs);
        let expected_used_percent = (elapsed / duration_secs * 100.0).clamp(0.0, 100.0);
        if expected_used_percent < 3.0 || elapsed <= 0.0 {
            return None;
        }
        let actual = effective_window.used_percent.clamp(0.0, 100.0);
        if actual <= 0.0 || actual >= 100.0 {
            return None;
        }
        let rate = actual / elapsed;
        let eta = (100.0 - actual) / rate;
        (eta.is_finite() && eta > 0.0 && eta < time_until_reset).then_some(eta)
    });
    Some(Selection {
        window: candidate.window,
        eta_seconds,
    })
}

fn precedes(lhs: &Selection, rhs: &Selection) -> bool {
    let lhs_exhausted = lhs.window.used_percent >= 100.0;
    let rhs_exhausted = rhs.window.used_percent >= 100.0;
    if lhs_exhausted != rhs_exhausted {
        return !lhs_exhausted;
    }
    if lhs.eta_seconds.is_some() != rhs.eta_seconds.is_some() {
        return lhs.eta_seconds.is_none();
    }
    if let (Some(l_eta), Some(r_eta)) = (lhs.eta_seconds, rhs.eta_seconds)
        && (l_eta - r_eta).abs() > f64::EPSILON
    {
        return l_eta > r_eta;
    }
    lhs.window.used_percent < rhs.window.used_percent
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

fn remaining_percent(window: &RateWindow) -> f64 {
    (100.0 - window.used_percent).clamp(0.0, 100.0)
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
