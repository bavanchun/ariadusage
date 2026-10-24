// Ported from CodexBar Sources/CodexBar/IconRemainingResolver.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuBarMetricWindowResolver.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/AutomaticQuotaSelection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UnifiedIconSource.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UnifiedIconProviderResolver.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+Animation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+HighestUsage.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/Providers/Codex/CodexConsumerProjection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeMonthlySpendWindow.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeScopedWeeklyLimitMapper.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Copilot/CopilotProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Cursor/CursorProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Doubao/DoubaoProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Gemini/GeminiProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Kimi/KimiProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
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
    primary_provider_for_unified_icon_with_overview(
        snapshot,
        selected,
        None,
        false,
        source,
        focused_provider,
        is_merged_menu_open,
        is_stacked,
        now,
    )
}

/// Picks the provider for the merged status item across all `UnifiedIconSource`
/// modes and explicit Overview subset / selection state
/// (`StatusItemController.primaryProviderForUnifiedIcon` and `defaultProviderForUnifiedIcon`,
/// `StatusItemController+Animation.swift:1152-1194`).
#[allow(clippy::too_many_arguments)]
pub fn primary_provider_for_unified_icon_with_overview<'a>(
    snapshot: &'a EngineSnapshot,
    selected: Option<&'a str>,
    overview_providers: Option<&[&'a str]>,
    last_selected_was_overview: bool,
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
    let overview_list: Vec<&'a ProviderSnapshot> = match overview_providers {
        Some(ov) => ov
            .iter()
            .filter_map(|&id| enabled_list.iter().copied().find(|p| p.id.as_str() == id))
            .collect(),
        None => enabled_list.clone(),
    };

    let fallback_id = if source == UnifiedIconSource::HighestUsage
        && let Some(p) = provider_with_highest_usage(&overview_list, now)
            .or_else(|| overview_list.first().copied())
    {
        p.id.as_str()
    } else if last_selected_was_overview && let Some(p) = overview_list.first().copied() {
        p.id.as_str()
    } else if let Some(sel) = UnifiedIconContext::current_selection_provider(selected, &enabled_ids)
    {
        sel
    } else {
        enabled_list
            .iter()
            .copied()
            .find(|p| provider_has_snapshot_data(p))
            .or_else(|| enabled_list.first().copied())
            .map(|p| p.id.as_str())?
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

/// Picks the candidate provider with the highest non-excluded usage, matching
/// `UsageStore.providerWithHighestUsage(candidateProviders:now:)` (`UsageStore+HighestUsage.swift:8-59`).
pub fn provider_with_highest_usage<'a>(
    candidates: &[&'a ProviderSnapshot],
    now: Option<Timestamp>,
) -> Option<&'a ProviderSnapshot> {
    provider_with_highest_usage_with_options(
        candidates,
        MenuBarMetricPreference::Automatic,
        false,
        now,
    )
}

/// Picks the candidate provider with the highest non-excluded usage with explicit
/// metric preference and Antigravity exhausted-quota priority options
/// (`UsageStore.providerWithHighestUsage`, `UsageStore+HighestUsage.swift:8-84`).
pub fn provider_with_highest_usage_with_options<'a>(
    candidates: &[&'a ProviderSnapshot],
    preference: MenuBarMetricPreference,
    antigravity_prioritize_exhausted_quotas: bool,
    now: Option<Timestamp>,
) -> Option<&'a ProviderSnapshot> {
    let mut best: Option<(&'a ProviderSnapshot, Selection)> = None;
    for &provider in candidates {
        if !provider.enabled || !provider_has_snapshot_data(provider) {
            continue;
        }
        let id = provider.id.as_str();
        let automatic = preference == MenuBarMetricPreference::Automatic;
        let selection = if id == "codex" && automatic {
            codex_automatic_menu_bar_selection(provider, now)
        } else {
            let window = if id == "antigravity"
                && automatic
                && !antigravity_prioritize_exhausted_quotas
                && has_antigravity_quota_summary(provider)
            {
                most_constrained_antigravity_quota_summary_window(provider)
            } else {
                resolve_menu_bar_metric_window(
                    preference,
                    provider,
                    false,
                    antigravity_prioritize_exhausted_quotas,
                    now,
                )
            };
            window.and_then(|w| {
                let slot = if automatic {
                    candidates_from_windows(std::slice::from_ref(&w), provider)
                        .into_iter()
                        .next()
                        .and_then(|c| c.slot)
                } else {
                    None
                };
                make_selection(Candidate { window: w, slot }, id, now)
            })
        };
        let Some(selection) = selection else {
            continue;
        };
        if should_exclude_from_highest_usage(
            provider,
            preference,
            antigravity_prioritize_exhausted_quotas,
            selection.window.used_percent,
            now,
        ) {
            continue;
        }
        let priority = if automatic {
            selection
        } else {
            Selection {
                window: selection.window,
                eta_seconds: None,
            }
        };
        if best.as_ref().is_none_or(|(_, b)| precedes(b, &priority)) {
            best = Some((provider, priority));
        }
    }
    best.map(|(p, _)| p)
}

/// Returns whether `provider` should be excluded from highest-usage unified icon selection
/// (`UsageStore.shouldExcludeFromHighestUsage`, `UsageStore+HighestUsage.swift:86-161`).
pub fn should_exclude_from_highest_usage(
    provider: &ProviderSnapshot,
    effective_preference: MenuBarMetricPreference,
    antigravity_prioritize_exhausted_quotas: bool,
    metric_percent: f64,
    now: Option<Timestamp>,
) -> bool {
    if metric_percent < 100.0 {
        return false;
    }
    let id = provider.id.as_str();
    if matches!(id, "codex" | "claude")
        && effective_preference == MenuBarMetricPreference::PrimaryAndSecondary
    {
        if id == "codex" && codex_has_binding_weekly_cap(provider, now) {
            return true;
        }
        if id == "claude" && claude_spend_limit_window(provider).is_some() {
            return true;
        }
        let percents: Vec<f64> = [
            measured_window(provider.windows.primary.as_ref()),
            measured_window(provider.windows.secondary.as_ref()),
        ]
        .into_iter()
        .flatten()
        .map(|w| w.used_percent)
        .collect();
        if percents.is_empty() {
            return true;
        }
        return percents.iter().all(|&p| p >= 100.0);
    }
    if id == "antigravity" && effective_preference == MenuBarMetricPreference::Automatic {
        if antigravity_prioritize_exhausted_quotas {
            return antigravity_quota_summary_families_are_all_blocked(provider);
        }
        let windows = antigravity_rendered_quota_summary_windows(provider);
        if windows.is_empty() {
            return true;
        }
        return windows.iter().all(|w| w.used_percent >= 100.0);
    }
    if id == "copilot"
        && effective_preference == MenuBarMetricPreference::Automatic
        && let (Some(primary), Some(secondary)) = (
            measured_window(provider.windows.primary.as_ref()),
            measured_window(provider.windows.secondary.as_ref()),
        )
    {
        return primary.used_percent >= 100.0 && secondary.used_percent >= 100.0;
    }
    if id == "cursor" && effective_preference == MenuBarMetricPreference::Automatic {
        let percents: Vec<f64> = [
            measured_window(provider.windows.primary.as_ref()),
            measured_window(provider.windows.secondary.as_ref()),
            measured_window(provider.windows.tertiary.as_ref()),
        ]
        .into_iter()
        .flatten()
        .map(|w| w.used_percent)
        .collect();
        if percents.is_empty() {
            return true;
        }
        return percents.iter().all(|&p| p >= 100.0);
    }
    if effective_preference == MenuBarMetricPreference::Automatic
        && automatic_selection_prioritizes_exhausted_window(id)
    {
        let percents: Vec<f64> = [
            measured_window(provider.windows.primary.as_ref()),
            measured_window(provider.windows.secondary.as_ref()),
            measured_window(provider.windows.tertiary.as_ref()),
        ]
        .into_iter()
        .flatten()
        .map(|w| w.used_percent)
        .collect();
        if percents.is_empty() {
            return true;
        }
        return percents.iter().all(|&p| p >= 100.0);
    }
    true
}

/// Whether automatic selection prioritizes an exhausted window over non-exhausted windows
/// (`MenuBarMetricWindowResolver.automaticSelectionPrioritizesExhaustedWindow`,
/// `MenuBarMetricWindowResolver.swift:60-63`).
pub fn automatic_selection_prioritizes_exhausted_window(provider_id: &str) -> bool {
    !matches!(
        provider_id,
        "antigravity"
            | "claude"
            | "codex"
            | "copilot"
            | "cursor"
            | "minimax"
            | "perplexity"
            | "warp"
            | "zai"
    )
}

/// Resolves the menu-bar icon credits fallback for `provider`
/// (`StatusItemController.menuBarCreditsRemainingForIcon` and `CodexConsumerProjection.menuBarFallback`,
/// `StatusItemController+Animation.swift:565-582`, `CodexConsumerProjection.swift:646-657`).
///
/// Only Codex projects credits into the menu-bar icon fallback, and only when `credits.remaining > 0`
/// and either all rate windows are absent or at least one lane is exhausted (`remaining_percent <= 0`)
/// with a reset timestamp still in the future (or absent).
pub fn menu_bar_credits_remaining_for_icon(
    provider: &ProviderSnapshot,
    now: Option<Timestamp>,
) -> Option<f64> {
    if provider.id.as_str() != "codex" {
        return None;
    }
    let credits = provider.credits.as_ref().and_then(displayable_value)?;
    if credits.balance_read_succeeded == Some(false) || credits.credits_available == Some(false) {
        return None;
    }
    if !credits.remaining.is_finite() || credits.remaining <= 0.0 {
        return None;
    }
    let eval_now = now.or(provider.updated_at);
    let lanes = codex_rate_windows_by_lane(provider);
    let has_no_rate_windows = lanes.is_empty();
    let has_exhausted_lane = lanes.values().any(|(w, _)| {
        remaining_percent(w) <= 0.0
            && w.resets_at
                .is_none_or(|reset_at| eval_now.is_none_or(|n| reset_at > n))
    });
    (has_exhausted_lane || has_no_rate_windows).then_some(credits.remaining)
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
    let credits_remaining = menu_bar_credits_remaining_for_icon(provider, prefs.now);

    IconInput {
        primary,
        secondary,
        credits_remaining,
        stale: provider.last_error.is_some(),
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

/// Resolves the status-item [`IconInput`] and target provider ID from the full
/// [`EngineSnapshot`], applying `UsageStore.iconStyle` (`IconStyle::Combined`
/// when `merge_icons` is true and more than one provider is enabled) while
/// preserving the resolved provider's own `QuotaLayoutPolicy` and window percents
/// (`StatusItemController+Animation.swift:353, 393-401, 498`).
pub fn status_item_icon_input<'a>(
    snapshot: Option<&'a EngineSnapshot>,
    selected_provider_id: Option<&str>,
    merge_icons: bool,
    prefs: IconPreferences,
) -> (Option<&'a ProviderSnapshot>, IconInput) {
    let Some(snapshot) = snapshot else {
        return (
            None,
            IconInput {
                stale: false,
                hide_critters: prefs.hide_critters,
                ..IconInput::default()
            },
        );
    };
    let provider = icon_provider(snapshot, selected_provider_id);
    let mut input = match provider {
        Some(p) => icon_input(p, prefs),
        None => IconInput {
            stale: false,
            hide_critters: prefs.hide_critters,
            ..IconInput::default()
        },
    };
    if merge_icons {
        input.style = store_icon_style(Some(snapshot));
    }
    (provider, input)
}

/// Resolves the `QuotaLayoutPolicy` for `provider` and its snapshot
/// (`IconRenderer.QuotaLayoutPolicy.provider(_:snapshot:)`, `IconRenderer.swift:10-21`).
pub fn quota_layout_policy_for_provider(
    provider: &ProviderSnapshot,
    secondary_override_window_id: Option<&str>,
    now: Option<Timestamp>,
) -> QuotaLayoutPolicy {
    let id = provider.id.as_str();
    let mut reserves_missing_secondary_lane = id == "claude";
    if reserves_missing_secondary_lane {
        let raw_primary = measured_window(provider.windows.primary.as_ref());
        let effective_now = now.or(provider.updated_at);
        let (resolved_primary, resolved_secondary) =
            resolved_icon_windows(provider, secondary_override_window_id, effective_now);
        let projected_single_window =
            raw_primary.is_none() && resolved_primary.is_some() && resolved_secondary.is_none();
        if projected_single_window {
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
            measured_window(provider.windows.primary.as_ref()).cloned(),
            measured_window(provider.windows.secondary.as_ref()).cloned(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

fn codex_rate_windows_by_lane(
    provider: &ProviderSnapshot,
) -> HashMap<CodexLaneKind, (RateWindow, Option<UsageLane>)> {
    let mut slotted: Vec<(CodexLaneKind, RateWindow, Option<UsageLane>)> = Vec::with_capacity(4);
    if let Some(w) = measured_window(provider.windows.primary.as_ref()).cloned() {
        let kind = classify_codex_lane(&w, CodexLaneKind::Session);
        slotted.push((kind, w, Some(UsageLane::Primary)));
    }
    if let Some(w) = measured_window(provider.windows.secondary.as_ref()).cloned() {
        let kind = classify_codex_lane(&w, CodexLaneKind::Weekly);
        slotted.push((kind, w, Some(UsageLane::Secondary)));
    }
    if let Some(w) = measured_window(provider.windows.tertiary.as_ref()).cloned() {
        let kind = classify_codex_lane(&w, CodexLaneKind::Monthly);
        slotted.push((kind, w, Some(UsageLane::Tertiary)));
    }
    for named in &provider.windows.extra {
        if let Some(w) = displayable_value(&named.window).cloned() {
            let kind = classify_codex_lane(&w, CodexLaneKind::Monthly);
            slotted.push((kind, w, None));
        }
    }

    let mut windows_by_lane: HashMap<CodexLaneKind, (RateWindow, Option<UsageLane>)> =
        HashMap::with_capacity(3);
    for (lane, window, slot) in slotted {
        windows_by_lane.insert(lane, (window, slot));
    }

    if windows_by_lane.is_empty()
        && let Some(monthly) = codex_monthly_credit_limit_window(provider)
    {
        windows_by_lane.insert(CodexLaneKind::Monthly, (monthly, None));
    }
    windows_by_lane
}

fn codex_monthly_credit_limit_window(provider: &ProviderSnapshot) -> Option<RateWindow> {
    if let Some(limit) = provider
        .credits
        .as_ref()
        .and_then(displayable_value)
        .and_then(|c| c.credit_limit.as_ref())
        && limit.total.is_finite()
        && limit.total > 0.0
        && limit.used.is_finite()
        && limit.used >= 0.0
    {
        let mut w = RateWindow::new((limit.used / limit.total) * 100.0);
        w.window_minutes = Some(MONTHLY_SENTINEL_MINUTES);
        w.resets_at = limit.resets_at;
        w.reset_description = limit.reset_description.clone();
        return Some(w);
    }
    let cost = provider.cost.as_ref().and_then(displayable_value)?;
    let limit = cost.limit?;
    if !cost.currency_code.eq_ignore_ascii_case("USD")
        || !limit.is_finite()
        || limit <= 0.0
        || !cost.used.is_finite()
        || cost.used < 0.0
    {
        return None;
    }
    let mut w = RateWindow::new((cost.used / limit) * 100.0);
    w.window_minutes = Some(MONTHLY_SENTINEL_MINUTES);
    w.resets_at = cost.resets_at;
    w.reset_description = cost.period.clone();
    Some(w)
}

/// Whether Codex's weekly lane is exhausted and its reset has not yet passed
/// (`CodexConsumerProjection.hasBindingWeeklyCap`, `CodexConsumerProjection.swift:417-423, 558-561`).
pub fn codex_has_binding_weekly_cap(provider: &ProviderSnapshot, now: Option<Timestamp>) -> bool {
    let eval_now = now.or(provider.updated_at);
    let lanes = codex_rate_windows_by_lane(provider);
    lanes.get(&CodexLaneKind::Weekly).is_some_and(|(w, _)| {
        remaining_percent(w) <= 0.0
            && w.resets_at
                .is_none_or(|reset_at| eval_now.is_none_or(|n| reset_at > n))
    })
}

fn codex_automatic_menu_bar_selection(
    provider: &ProviderSnapshot,
    now: Option<Timestamp>,
) -> Option<Selection> {
    let eval_now = now.or(provider.updated_at);
    let lanes = codex_rate_windows_by_lane(provider);
    let is_expired =
        |w: &RateWindow| -> bool { eval_now.is_some_and(|n| w.resets_at.is_some_and(|r| r <= n)) };
    let weekly_caps_session = lanes
        .get(&CodexLaneKind::Weekly)
        .is_some_and(|(w, _)| remaining_percent(w) <= 0.0 && !is_expired(w));
    let weekly_reset = lanes
        .get(&CodexLaneKind::Weekly)
        .and_then(|(w, _)| w.resets_at);

    let mut visible: Vec<(CodexLaneKind, Candidate)> = Vec::with_capacity(3);
    for kind in [
        CodexLaneKind::Session,
        CodexLaneKind::Weekly,
        CodexLaneKind::Monthly,
    ] {
        let Some((mut window, slot)) = lanes.get(&kind).cloned() else {
            continue;
        };
        if kind == CodexLaneKind::Session && weekly_caps_session {
            window.used_percent = window.used_percent.max(100.0);
            if let Some(wr) = weekly_reset {
                window.resets_at = Some(match window.resets_at {
                    Some(sr) => sr.max(wr),
                    None => wr,
                });
            }
        }
        if remaining_percent(&window) <= 0.0 && is_expired(&window) {
            continue;
        }
        visible.push((kind, Candidate { window, slot }));
    }

    let has_longer_than_weekly = visible
        .iter()
        .any(|(_, c)| c.window.window_minutes.is_some_and(|m| m > 10080));
    let eligible: Vec<Candidate> = if has_longer_than_weekly
        && let Some((_, weekly)) = visible.iter().find(|(k, _)| *k == CodexLaneKind::Weekly)
    {
        vec![weekly.clone()]
    } else {
        visible.into_iter().map(|(_, c)| c).collect()
    };

    let mut best: Option<Selection> = None;
    for candidate in eligible {
        let Some(sel) = make_selection(candidate, "codex", eval_now) else {
            continue;
        };
        if best.as_ref().is_none_or(|b| precedes(b, &sel)) {
            best = Some(sel);
        }
    }
    best
}

fn codex_icon_windows(
    provider: &ProviderSnapshot,
    now: Option<Timestamp>,
) -> (Option<RateWindow>, Option<RateWindow>) {
    let raw_primary = measured_window(provider.windows.primary.as_ref()).cloned();
    let raw_secondary = measured_window(provider.windows.secondary.as_ref()).cloned();

    let mut slotted: Vec<(CodexLaneKind, RateWindow)> = Vec::with_capacity(2);
    if let Some(w) = raw_primary {
        let kind = classify_codex_lane(&w, CodexLaneKind::Session);
        slotted.push((kind, w));
    }
    if let Some(w) = raw_secondary {
        let kind = classify_codex_lane(&w, CodexLaneKind::Weekly);
        slotted.push((kind, w));
    }

    // `CodexProviderDescriptor.windowsByLane` (`CodexProviderDescriptor.swift:227-237`):
    // duplicate lanes are last-wins.
    let mut windows_by_lane: HashMap<CodexLaneKind, RateWindow> = HashMap::with_capacity(2);
    for (lane, window) in &slotted {
        windows_by_lane.insert(*lane, window.clone());
    }

    let is_expired =
        |w: &RateWindow| -> bool { now.is_some_and(|n| w.resets_at.is_some_and(|r| r <= n)) };

    let weekly_caps_session = windows_by_lane
        .get(&CodexLaneKind::Weekly)
        .is_some_and(|w| remaining_percent(w) <= 0.0 && !is_expired(w));

    let mut seen: HashSet<CodexLaneKind> = HashSet::with_capacity(2);
    let mut visible: Vec<RateWindow> = Vec::with_capacity(2);
    for (kind, _) in slotted {
        if !seen.insert(kind) {
            continue;
        }
        let Some(mut window) = windows_by_lane.get(&kind).cloned() else {
            continue;
        };
        if kind == CodexLaneKind::Session && weekly_caps_session {
            window.used_percent = 100.0;
        }
        if remaining_percent(&window) <= 0.0 && is_expired(&window) {
            continue;
        }
        visible.push(window);
    }

    (visible.first().cloned(), visible.get(1).cloned())
}

fn claude_icon_windows(provider: &ProviderSnapshot) -> (Option<RateWindow>, Option<RateWindow>) {
    let raw_primary = measured_window(provider.windows.primary.as_ref()).cloned();
    let raw_secondary = measured_window(provider.windows.secondary.as_ref()).cloned();
    let raw_tertiary = measured_window(provider.windows.tertiary.as_ref());

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

/// Returns the Claude monthly spend-limit window when the account only exposes an
/// enterprise/extra-usage spend limit and has no real session/weekly/tertiary or
/// scoped-weekly quota lanes (`MenuBarMetricWindowResolver.claudeSpendLimitWindow`,
/// `MenuBarMetricWindowResolver.swift:170-176`).
pub fn claude_spend_limit_window(provider: &ProviderSnapshot) -> Option<RateWindow> {
    if measured_window(provider.windows.primary.as_ref()).is_some()
        || measured_window(provider.windows.secondary.as_ref()).is_some()
        || measured_window(provider.windows.tertiary.as_ref()).is_some()
        || claude_scoped_weekly_window(provider).is_some()
    {
        return None;
    }
    claude_monthly_spend_window(provider)
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

fn antigravity_rendered_quota_summary_windows(provider: &ProviderSnapshot) -> Vec<RateWindow> {
    let (primary, secondary) = antigravity_icon_windows(provider);
    [primary, secondary].into_iter().flatten().collect()
}

fn most_constrained_antigravity_quota_summary_window(
    provider: &ProviderSnapshot,
) -> Option<RateWindow> {
    let windows = antigravity_rendered_quota_summary_windows(provider);
    if windows.is_empty() {
        return None;
    }
    let usable: Vec<&RateWindow> = windows.iter().filter(|w| w.used_percent < 100.0).collect();
    let pool: Vec<&RateWindow> = if usable.is_empty() {
        windows.iter().collect()
    } else {
        usable
    };
    pool.into_iter()
        .max_by(|a, b| {
            a.used_percent
                .partial_cmp(&b.used_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned()
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

    // `CursorProviderDescriptor.menuBarMetrics.tertiaryRequiresWindow` falls back to `.automatic`
    // when `.tertiary` is requested but the API lane (`tertiary`) is absent.
    let preference = if id == "cursor"
        && preference == MenuBarMetricPreference::Tertiary
        && tertiary.is_none()
    {
        MenuBarMetricPreference::Automatic
    } else {
        preference
    };

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
        "kimi" if preference == MenuBarMetricPreference::Automatic => {
            let monthly = extra_window_by_id(provider, "kimi-monthly");
            return exhausted_of(&[monthly, primary.clone(), secondary.clone()])
                .or(secondary)
                .or(primary);
        }
        "litellm" if preference == MenuBarMetricPreference::Automatic => {
            return exhausted_of(&[primary.clone(), secondary.clone()])
                .or(secondary)
                .or(primary);
        }
        "perplexity" if preference == MenuBarMetricPreference::Automatic => {
            return perplexity_icon_windows(provider).0;
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
            if (supports_average || id == "gemini")
                && let (Some(p), Some(s)) = (&primary, &secondary)
            {
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
            if matches!(provider_id, "antigravity" | "cursor" | "perplexity") {
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

/// Resolves the store-level `IconStyle` used by the merged status item (`UsageStore.iconStyle`,
/// `UsageStore.swift:614-624`): `.combined` when multiple providers are enabled, the single enabled
/// provider's style when exactly one is enabled, or `.codex` when none are enabled.
pub fn store_icon_style(snapshot: Option<&EngineSnapshot>) -> IconStyle {
    let Some(snapshot) = snapshot else {
        return IconStyle::Codex;
    };
    let mut enabled = snapshot.providers.iter().filter(|p| p.enabled);
    let Some(first) = enabled.next() else {
        return IconStyle::Codex;
    };
    if enabled.next().is_some() {
        IconStyle::Combined
    } else {
        IconStyle::from_provider_id(first.id.as_str())
    }
}

/// Whether a provider has completed a snapshot fetch or has known-unavailable limits
/// (`UsageStore.hasSatisfiedUsageFetch`, `UsageStore.swift:709-712`).
pub fn provider_has_snapshot_data(provider: &ProviderSnapshot) -> bool {
    let metric_resolved = |m: Option<&Metric<RateWindow>>| {
        m.is_some_and(|metric| {
            matches!(
                metric.state,
                MetricState::Value | MetricState::Stale | MetricState::Unknown
            )
        })
    };
    provider.updated_at.is_some()
        || metric_resolved(provider.windows.primary.as_ref())
        || metric_resolved(provider.windows.secondary.as_ref())
        || metric_resolved(provider.windows.tertiary.as_ref())
        || provider.windows.extra.iter().any(|w| {
            matches!(
                w.window.state,
                MetricState::Value | MetricState::Stale | MetricState::Unknown
            )
        })
        || provider
            .credits
            .as_ref()
            .and_then(displayable_value)
            .is_some()
        || provider.cost.as_ref().and_then(displayable_value).is_some()
}

/// Whether `provider` needs the menu-bar loading animation
/// (`StatusItemController.shouldAnimate(provider:)`, `StatusItemController+Animation.swift:1230-1254`).
pub fn should_animate_provider(provider: &ProviderSnapshot, debug_force_animation: bool) -> bool {
    if !provider.enabled {
        return false;
    }
    if debug_force_animation {
        return true;
    }
    let has_satisfied_usage_fetch = provider_has_snapshot_data(provider);
    let is_stale = provider.last_error.is_some();
    !has_satisfied_usage_fetch && !is_stale
}

/// Whether any enabled provider in `snapshot` needs the menu-bar loading animation
/// (`StatusItemController.anyEnabledProviderNeedsLoadingAnimation()`,
/// `StatusItemController+Animation.swift:32-36`).
pub fn any_enabled_provider_needs_loading_animation(
    snapshot: &EngineSnapshot,
    debug_force_animation: bool,
) -> bool {
    snapshot
        .providers
        .iter()
        .filter(|p| p.enabled)
        .any(|p| should_animate_provider(p, debug_force_animation))
}

/// Whether the menu-bar status item should run the loading animation
/// (`StatusItemController.needsMenuBarIconAnimation()`, `StatusItemController+Animation.swift:13-25`).
/// When `merge_icons` is `true`, only the resolved unified icon provider is checked;
/// when `merge_icons` is `false`, any enabled provider needing loading animation qualifies.
pub fn needs_menu_bar_icon_animation(
    snapshot: &EngineSnapshot,
    selected_provider_id: Option<&str>,
    merge_icons: bool,
    debug_force_animation: bool,
) -> bool {
    if merge_icons {
        let Some(provider) = icon_provider(snapshot, selected_provider_id) else {
            return false;
        };
        should_animate_provider(provider, debug_force_animation)
    } else {
        any_enabled_provider_needs_loading_animation(snapshot, debug_force_animation)
    }
}

/// Computes the store-icon observation signature used to skip redundant icon updates when
/// non-visual snapshot or status metadata changes (`StatusItemController.storeIconObservationSignature`).
pub fn store_icon_observation_signature(
    snapshot: Option<&EngineSnapshot>,
    selected: Option<&str>,
    merge_icons: bool,
    prefs: IconPreferences,
) -> u64 {
    store_icon_observation_signature_with_override(snapshot, selected, merge_icons, prefs, None)
}

/// Computes the store-icon observation signature with an optional Copilot secondary window override ID
/// (`StatusItemController.storeIconObservationSignature`).
pub fn store_icon_observation_signature_with_override(
    snapshot: Option<&EngineSnapshot>,
    selected: Option<&str>,
    merge_icons: bool,
    prefs: IconPreferences,
    secondary_override_window_id: Option<&str>,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    merge_icons.hash(&mut hasher);
    prefs.show_used.hash(&mut hasher);
    prefs.hide_critters.hash(&mut hasher);
    (prefs.metric_preference as u8).hash(&mut hasher);
    store_icon_style(snapshot).hash(&mut hasher);

    let Some(snapshot) = snapshot else {
        false.hash(&mut hasher);
        return hasher.finish();
    };
    true.hash(&mut hasher);

    let hash_provider =
        |p: &ProviderSnapshot, h: &mut std::collections::hash_map::DefaultHasher| {
            p.id.as_str().hash(h);
            let input = icon_input_with_override(p, prefs, secondary_override_window_id);
            input.primary.map(|v| (v * 1000.0).round() as i64).hash(h);
            input.secondary.map(|v| (v * 1000.0).round() as i64).hash(h);
            input
                .credits_remaining
                .map(|v| (v * 100.0).round() as i64)
                .hash(h);
            input.stale.hash(h);
            (input.status as u8).hash(h);
            input.reserves_missing_secondary_lane.hash(h);
            input.treats_exhausted_secondary_as_missing.hash(h);
        };

    if merge_icons {
        if let Some(primary) = icon_provider(snapshot, selected) {
            hash_provider(primary, &mut hasher);
        }
    } else {
        for provider in snapshot.providers.iter().filter(|p| p.enabled) {
            hash_provider(provider, &mut hasher);
        }
    }
    hasher.finish()
}

/// Returns a measured `RateWindow` when the metric has a real value (`Value` or `Stale`).
/// In Snapshot v1 (`ariadusage-core::projection::project_window_metric`), a synthetic
/// placeholder window (`is_synthetic_placeholder == true`) is projected onto the wire as
/// `MetricState::Unknown` (`value: None`), so this helper matches upstream `RateWindow.measured`
/// (`ClaudeProviderDescriptor.swift:255, 280` and `IconRenderer.swift:16`).
pub fn measured_window(metric: Option<&Metric<RateWindow>>) -> Option<&RateWindow> {
    metric.and_then(displayable_value)
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
