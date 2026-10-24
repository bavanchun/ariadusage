// Ported from CodexBar Sources/CodexBar/MenuDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+Menu.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuCardItems.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+UsageHistoryMenu.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+CostMenuCard.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+OverviewSpend.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/SettingsStore+MenuPreferences.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+TokenCost.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/Providers/Claude/ClaudeProviderImplementation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/Providers/Codex/CodexProviderImplementation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/Providers/Antigravity/AntigravityProviderImplementation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Pure-Rust menu structure for the status item dropdown.
//!
//! Given the current [`EngineSnapshot`], connection state and selected tab,
//! [`build`] produces the switcher segments, primary card content (Overview
//! rows or single-provider usage card sections), history submenus (`Plan Usage`,
//! `Cost`), contextual provider actions and meta section in CodexBar's order and
//! wording with AriadUsage's own identity.

use std::collections::{BTreeMap, BTreeSet};

use ariadusage_protocol::metric::{Metric, MetricState};
use ariadusage_protocol::snapshot::{EngineSnapshot, ProviderSnapshot};
use ariadusage_protocol::usage::{
    Cost, CostHistory, CostProvenance, Identity, PlanUtilizationSeries, ProviderStatus, RateWindow,
    StatusIndicator,
};
use jiff::Timestamp;

use crate::card::{self, CardModel, CardPreferences, PercentStyle};
use crate::card_layout::CardSectionMode;
use crate::chart::{CostHistoryChartModel, PlanUtilizationChartModel, QuotaBurndownChartModel};
use crate::format::{self, DateTexts};
use crate::menu_session::MenuCardRefreshMonitor;
use crate::presentation;
use crate::switcher::{self, SwitcherSegment, SwitcherSelection};

/// Default version string shown in `About AriadUsage (vX.Y.Z)`.
pub use crate::settings::APP_VERSION;

/// Metadata for a provider's contextual menu actions (`ProviderMetadata`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderMenuMetadata {
    pub supports_login_flow: bool,
    pub dashboard_url: Option<&'static str>,
    pub status_page_url: Option<&'static str>,
    pub status_link_url: Option<&'static str>,
}

/// Returns the contextual menu metadata for a first-party provider.
pub fn provider_metadata(provider_id: &str) -> ProviderMenuMetadata {
    match provider_id {
        "claude" => ProviderMenuMetadata {
            supports_login_flow: true,
            dashboard_url: Some("https://claude.ai/settings/usage"),
            status_page_url: Some("https://status.claude.com/"),
            status_link_url: None,
        },
        "codex" => ProviderMenuMetadata {
            supports_login_flow: true,
            dashboard_url: Some("https://chatgpt.com/codex/settings/usage"),
            status_page_url: Some("https://status.openai.com/"),
            status_link_url: None,
        },
        "antigravity" => ProviderMenuMetadata {
            supports_login_flow: true,
            dashboard_url: None,
            status_page_url: None,
            status_link_url: Some("https://www.google.com/appsstatus/dashboard/"),
        },
        _ => ProviderMenuMetadata {
            supports_login_flow: false,
            dashboard_url: None,
            status_page_url: None,
            status_link_url: None,
        },
    }
}

/// An executable menu action (`MenuDescriptor.MenuAction` + status controller actions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    ShareUsageSnapshot,
    BuyCredits { url: String },
    AddAccount { provider_id: String },
    SwitchAccount { provider_id: String },
    OpenDashboard { url: String },
    OpenStatusPage { url: String },
    Refresh,
    Settings,
    About,
    Quit,
}

impl MenuAction {
    /// SF Symbol name for the menu item (`MenuDescriptor.MenuActionSystemImage`).
    pub fn system_image(&self) -> Option<&'static str> {
        match self {
            Self::ShareUsageSnapshot => Some("square.and.arrow.up"),
            Self::BuyCredits { .. } => Some("plus.circle"),
            Self::AddAccount { .. } => Some("plus"),
            Self::SwitchAccount { .. } => Some("key"),
            Self::OpenDashboard { .. } => Some("chart.xyaxis.line"),
            Self::OpenStatusPage { .. } => Some("waveform.path.ecg"),
            Self::Refresh => Some("arrow.clockwise"),
            Self::Settings => Some("gearshape"),
            Self::About => Some("info.circle"),
            Self::Quit => Some("xmark.rectangle"),
        }
    }

    /// Command key equivalent (`StatusItemController.shortcut(for:)`).
    pub fn key_equivalent(&self) -> &'static str {
        match self {
            Self::Refresh => "r",
            Self::Settings => ",",
            Self::Quit => "q",
            _ => "",
        }
    }
}

/// An item inside a submenu (`StatusItemController.makeStatusComponentsSubmenu`,
/// `StatusItemController+UsageHistoryMenu.swift`, `StatusItemController+CostMenuCard.swift`).
#[derive(Debug, Clone, PartialEq)]
pub enum SubmenuEntry {
    SecondaryText(String),
    Separator,
    Action {
        title: String,
        action: MenuAction,
        enabled: bool,
    },
    QuotaBurndownChart(Box<QuotaBurndownChartModel>),
    PlanUtilizationChart(Box<PlanUtilizationChartModel>),
    CostHistoryChart(Box<CostHistoryChartModel>),
}

/// A row in the history, contextual actions or meta section (`MenuDescriptor.Entry`).
#[derive(Debug, Clone, PartialEq)]
pub enum MenuEntry {
    SecondaryText(String),
    Action {
        title: String,
        system_image: Option<&'static str>,
        key_equivalent: &'static str,
        enabled: bool,
        action: MenuAction,
    },
    Submenu {
        title: String,
        system_image: Option<&'static str>,
        items: Vec<SubmenuEntry>,
    },
}

/// Summary card model shown at the top of the `Overview` tab (`OverviewSpendSummary`,
/// `StatusItemController+OverviewSpend.swift:6-14`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverviewSpendSummary {
    pub day_range_text: String,
    pub primary_spend_text: String,
    pub provider_coverage_text: String,
    pub token_text: Option<String>,
    pub history_coverage_text: String,
    pub provenance_text: String,
    pub pricing_coverage_text: String,
}

/// One card section `NSMenuItem` produced by `addMenuCardSections`
/// (`StatusItemController+Menu.swift:1290-1400`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardSectionEntry {
    pub mode: CardSectionMode,
    pub has_cost_submenu: bool,
}

/// One provider row on the `Overview` tab (`OverviewMenuCardRowView`).
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewCard {
    pub provider_id: String,
    pub model: CardModel,
    pub submenu_title: Option<String>,
    pub submenu: Vec<SubmenuEntry>,
}

/// Primary card content above the history, action and meta sections.
#[derive(Debug, Clone, PartialEq)]
pub enum PrimaryContent {
    /// Shown while disconnected or when no providers are enabled.
    Disconnected { message: String },
    /// Shown when the `Overview` tab is active (`addOverviewRows`).
    Overview {
        spend_summary: Option<OverviewSpendSummary>,
        share_action: Option<MenuEntry>,
        cards: Vec<OverviewCard>,
    },
    /// Shown when a single provider tab is active (`UsageMenuCardView` / `addMenuCardSections`).
    Provider {
        provider_id: String,
        card: Box<CardModel>,
        sections: Vec<CardSectionEntry>,
        cost_submenu: Vec<SubmenuEntry>,
        buy_credits: Option<MenuEntry>,
    },
}

/// Complete menu structure for the status item dropdown.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuStructure {
    pub selection: Option<SwitcherSelection>,
    pub switcher: Vec<SwitcherSegment>,
    pub primary: PrimaryContent,
    pub history: Vec<MenuEntry>,
    pub actions: Vec<MenuEntry>,
    pub meta: Vec<MenuEntry>,
}

/// Builds the full menu structure for `snapshot` and `current_selection`.
pub fn build(
    snapshot: Option<&EngineSnapshot>,
    connected: bool,
    current_selection: Option<&SwitcherSelection>,
    prefs: &CardPreferences,
    now: Timestamp,
    dates: &dyn DateTexts,
) -> MenuStructure {
    build_with_refresh(
        snapshot,
        connected,
        current_selection,
        prefs,
        now,
        dates,
        None,
    )
}

/// Builds the full menu structure for `snapshot` and `current_selection`,
/// applying `refresh_monitor`'s frozen card models, loading subtitles and
/// per-tab `"Refresh"` action availability (`StatusItemController+Menu.swift`).
pub fn build_with_refresh(
    snapshot: Option<&EngineSnapshot>,
    connected: bool,
    current_selection: Option<&SwitcherSelection>,
    prefs: &CardPreferences,
    now: Timestamp,
    dates: &dyn DateTexts,
    refresh_monitor: Option<&MenuCardRefreshMonitor>,
) -> MenuStructure {
    let version = snapshot
        .map(|s| s.engine.version.as_str())
        .filter(|v| !v.is_empty())
        .unwrap_or(APP_VERSION);

    let Some(snapshot) = snapshot.filter(|_| connected) else {
        let meta = build_meta_section(false, version);
        let message = presentation::connection_status(snapshot, connected)
            .unwrap_or_else(|| "Connecting to engine…".to_string());
        return MenuStructure {
            selection: None,
            switcher: Vec::new(),
            primary: PrimaryContent::Disconnected { message },
            history: Vec::new(),
            actions: Vec::new(),
            meta,
        };
    };

    let enabled: Vec<&ProviderSnapshot> = snapshot.providers.iter().filter(|p| p.enabled).collect();
    let Some(selection) = switcher::resolve_selection(snapshot, current_selection) else {
        let meta = build_meta_section(connected, version);
        let message = presentation::connection_status(Some(snapshot), connected)
            .unwrap_or_else(|| "No providers enabled".to_string());
        return MenuStructure {
            selection: None,
            switcher: Vec::new(),
            primary: PrimaryContent::Disconnected { message },
            history: Vec::new(),
            actions: Vec::new(),
            meta,
        };
    };

    let refresh_in_flight = refresh_monitor
        .is_some_and(|m| m.is_refresh_action_in_flight(Some(&selection), Some(snapshot)));
    let meta = build_meta_section(connected && !refresh_in_flight, version);
    let switcher = switcher::segments(snapshot, prefs.show_used);
    match &selection {
        SwitcherSelection::Overview => {
            let spend_summary = overview_spend_summary(&enabled);
            let share_action = spend_summary.is_some().then(|| MenuEntry::Action {
                title: "Share Usage Snapshot…".to_string(),
                system_image: MenuAction::ShareUsageSnapshot.system_image(),
                key_equivalent: MenuAction::ShareUsageSnapshot.key_equivalent(),
                enabled: true,
                action: MenuAction::ShareUsageSnapshot,
            });
            let cards = enabled
                .iter()
                .map(|provider| {
                    let candidate = card::provider_card(provider, prefs, now, dates);
                    let mut model = match refresh_monitor {
                        Some(monitor) => {
                            let is_refreshing = monitor
                                .is_provider_refreshing(provider.id.as_str(), Some(snapshot));
                            monitor.effective_card_model(
                                provider.id.as_str(),
                                candidate,
                                is_refreshing,
                            )
                        }
                        None => candidate,
                    };
                    let (submenu_title, submenu) = overview_row_submenu(provider, now, dates);
                    model.shows_submenu_indicator = !submenu.is_empty();
                    OverviewCard {
                        provider_id: provider.id.as_str().to_string(),
                        model,
                        submenu_title,
                        submenu,
                    }
                })
                .collect();
            MenuStructure {
                selection: Some(selection),
                switcher,
                primary: PrimaryContent::Overview {
                    spend_summary,
                    share_action,
                    cards,
                },
                history: Vec::new(),
                // Overview tab suppresses provider-specific contextual actions
                // (`includeContextualActions: !isOverviewSelected`).
                actions: Vec::new(),
                meta,
            }
        }
        SwitcherSelection::Provider(id) => {
            let provider = enabled
                .iter()
                .copied()
                .find(|p| p.id.as_str() == id.as_str())
                .unwrap_or(enabled[0]);
            let candidate = card::provider_card(provider, prefs, now, dates);
            let card_model = match refresh_monitor {
                Some(monitor) => {
                    let is_refreshing =
                        monitor.is_provider_refreshing(provider.id.as_str(), Some(snapshot));
                    monitor.effective_card_model(provider.id.as_str(), candidate, is_refreshing)
                }
                None => candidate,
            };
            let sections = provider_card_sections(provider, &card_model);
            let cost_submenu = cost_chart_submenu_items(provider, dates);
            let buy_credits = buy_credits_entry(provider, prefs);
            let history = build_history_section_at(provider, prefs, now, dates);
            let actions = build_actions_section(provider, now, dates);
            MenuStructure {
                selection: Some(selection),
                switcher,
                primary: PrimaryContent::Provider {
                    provider_id: provider.id.as_str().to_string(),
                    card: Box::new(card_model),
                    sections,
                    cost_submenu,
                    buy_credits,
                },
                history,
                actions,
                meta,
            }
        }
    }
}

/// Returns true for providers whose descriptor sets `tokenCost.supportsTokenCost = true`
/// (`SettingsStore.isCostUsageEffectivelyEnabled`, `SettingsStore+MenuPreferences.swift:309-313`).
pub fn provider_supports_token_cost(provider_id: &str) -> bool {
    matches!(
        provider_id,
        "antigravity"
            | "bedrock"
            | "claude"
            | "codex"
            | "cursor"
            | "grok"
            | "mistral"
            | "muse"
            | "openai"
            | "opencodego"
            | "openrouter"
            | "pi"
            | "vertexai"
            | "xai"
    )
}

/// Returns true for providers whose token cost is projected from the live provider
/// snapshot rather than the local JSONL scanner (`UsageStore.tokenCostRequiresProviderSnapshot`,
/// `UsageStore+TokenCost.swift:747-756`).
pub fn token_cost_requires_provider_snapshot(provider_id: &str) -> bool {
    matches!(
        provider_id,
        "grok" | "mistral" | "openai" | "opencodego" | "openrouter" | "xai"
    )
}

/// Splits a provider's card into separate `NSMenuItem` sections when
/// `hasOpenAIWebMenuItems || requiresSectionedMenuForProviderDerivedCost` holds
/// (`openAIWebContext`, `addMenuCardSections`, `requiresSectionedMenuForProviderDerivedCost`,
/// `StatusItemController+Menu.swift:472-493,748-749,1290-1400,1521-1523,1584-1587`).
pub fn provider_card_sections(
    provider: &ProviderSnapshot,
    card: &CardModel,
) -> Vec<CardSectionEntry> {
    let id = provider.id.as_str();
    let has_cost_chart = provider
        .cost_history
        .as_ref()
        .and_then(displayable_value)
        .is_some_and(|h| !h.daily.is_empty());
    let has_cost_history = provider_supports_token_cost(id) && has_cost_chart;
    let has_openai_web_items = has_cost_history;
    let requires_sectioned_cost = token_cost_requires_provider_snapshot(id) && has_cost_chart;
    if !has_openai_web_items && !requires_sectioned_cost {
        return vec![CardSectionEntry {
            mode: CardSectionMode::Full,
            has_cost_submenu: false,
        }];
    }
    let has_credits = card.credits.is_some();
    let usage_bottom = if has_credits { 4.0 } else { 6.0 };
    let has_usage_cost_submenu = provider_supports_token_cost(id)
        && has_cost_chart
        && (matches!(id, "openai" | "mistral") || card.inline_usage_dashboard.is_some());
    let mut sections = vec![CardSectionEntry {
        mode: CardSectionMode::HeaderAndUsage {
            bottom_padding: usage_bottom,
        },
        has_cost_submenu: has_usage_cost_submenu,
    }];
    if has_credits {
        sections.push(CardSectionEntry {
            mode: CardSectionMode::CreditsOnly {
                top_padding: 6.0,
                bottom_padding: 4.0,
            },
            has_cost_submenu: false,
        });
    }
    if card.provider_cost.is_some() {
        sections.push(CardSectionEntry {
            mode: CardSectionMode::ExtraUsageOnly {
                top_padding: 6.0,
                bottom_padding: usage_bottom,
            },
            has_cost_submenu: false,
        });
    }
    sections
}

fn cost_chart_submenu_items(
    provider: &ProviderSnapshot,
    dates: &dyn DateTexts,
) -> Vec<SubmenuEntry> {
    let currency = provider
        .cost_history
        .as_ref()
        .and_then(displayable_value)
        .and_then(|h| h.currency_code.as_deref())
        .unwrap_or("USD");
    let Some(chart) = CostHistoryChartModel::from_provider_with_dates(provider, currency, dates)
    else {
        return Vec::new();
    };
    vec![SubmenuEntry::CostHistoryChart(Box::new(chart))]
}

/// Builds the submenu attached to an `Overview` card row (`makeOverviewSubmenu`,
/// `StatusItemController+OverviewSubmenus.swift:5-35`): prefers the daily cost
/// history chart submenu when available, otherwise falls back to the plan usage
/// history chart submenu when utilization history exists.
fn overview_row_submenu(
    provider: &ProviderSnapshot,
    now: Timestamp,
    dates: &dyn DateTexts,
) -> (Option<String>, Vec<SubmenuEntry>) {
    let cost_items = cost_chart_submenu_items(provider, dates);
    if !cost_items.is_empty() {
        return (Some("Cost".to_string()), cost_items);
    }
    let plan_items = plan_usage_chart_submenu_items(provider, now, dates);
    if !plan_items.is_empty() {
        return (Some("Plan Usage".to_string()), plan_items);
    }
    (None, Vec::new())
}

/// Builds the `OverviewSpendSummary` across enabled providers from their
/// 30-day `cost_history` snapshots (`StatusItemController+OverviewSpend.swift:61-147`).
/// Returns `None` when no enabled provider has displayable cost history, which
/// also suppresses `Share Usage Snapshot…` (`StatusItemController+Menu.swift:562-579`).
pub fn overview_spend_summary(enabled: &[&ProviderSnapshot]) -> Option<OverviewSpendSummary> {
    if enabled.is_empty() {
        return None;
    }
    let total_provider_count = enabled.len();
    let histories: Vec<&CostHistory> = enabled
        .iter()
        .filter_map(|p| p.cost_history.as_ref().and_then(displayable_value))
        .filter(|h| {
            h.last_30_days_cost_usd.is_some()
                || h.last_30_days_tokens.is_some()
                || h.session_cost_usd.is_some()
                || h.session_tokens.is_some()
                || !h.daily.is_empty()
        })
        .collect();
    if histories.is_empty() {
        return None;
    }

    let requested_days: u32 = 30;
    let included_provider_count = histories.len();

    #[derive(Default)]
    struct GroupAgg {
        provider_count: usize,
        priced_provider_count: usize,
        total_cost: Option<f64>,
        has_partial_cost: bool,
        total_tokens: Option<u64>,
        covered_days: Option<u32>,
    }

    let mut groups: BTreeMap<String, GroupAgg> = BTreeMap::new();
    let mut saw_vendor_metered = false;
    let mut saw_estimate = false;
    let mut priced_requests: u64 = 0;
    let mut unpriced_requests: u64 = 0;

    for history in &histories {
        let code = history
            .currency_code
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("USD")
            .to_ascii_uppercase();
        let group = groups.entry(code).or_default();
        group.provider_count += 1;

        let prov_cost = history.last_30_days_cost_usd.or_else(|| {
            let mut sum = 0.0;
            let mut any = false;
            for d in &history.daily {
                if let Some(c) = d.cost_usd.filter(|c| c.is_finite() && *c >= 0.0) {
                    sum += c;
                    any = true;
                }
            }
            any.then_some(sum)
        });

        let prov_tokens = history.last_30_days_tokens.or_else(|| {
            let mut sum = 0_u64;
            let mut any = false;
            for d in &history.daily {
                if let Some(t) = d.total_tokens {
                    sum = sum.saturating_add(t);
                    any = true;
                }
            }
            any.then_some(sum)
        });

        let incomplete_reqs = history
            .comparison_windows
            .iter()
            .find(|w| w.days == requested_days)
            .and_then(|w| w.incomplete_request_count)
            .unwrap_or(0);
        let has_unpriced_daily = history
            .daily
            .iter()
            .any(|d| d.total_tokens.unwrap_or(0) > 0 && d.cost_usd.is_none());

        if let Some(c) = prov_cost.filter(|c| c.is_finite() && *c >= 0.0) {
            group.priced_provider_count += 1;
            group.total_cost = Some(group.total_cost.unwrap_or(0.0) + c);
            if history.history_scan_is_partial || incomplete_reqs > 0 || has_unpriced_daily {
                group.has_partial_cost = true;
            }
            match history.cost_provenance.unwrap_or(CostProvenance::Unknown) {
                CostProvenance::VendorMetered => saw_vendor_metered = true,
                CostProvenance::ListPriceEstimate | CostProvenance::Unknown => saw_estimate = true,
                CostProvenance::Mixed => {
                    saw_vendor_metered = true;
                    saw_estimate = true;
                }
            }
            if history.metered_cost_usd.is_some()
                && requested_days >= history.history_days.unwrap_or(requested_days)
            {
                saw_vendor_metered = true;
            }
        } else {
            group.has_partial_cost = true;
        }

        if let Some(t) = prov_tokens {
            group.total_tokens = Some(group.total_tokens.unwrap_or(0).saturating_add(t));
        }

        let distinct_days = history
            .daily
            .iter()
            .map(|d| d.date.trim())
            .filter(|d| !d.is_empty())
            .collect::<BTreeSet<_>>()
            .len() as u32;
        let prov_covered = if distinct_days > 0 {
            distinct_days.min(requested_days)
        } else if history.history_coverage_is_established {
            history.history_days.unwrap_or(0).min(requested_days)
        } else {
            0
        };
        group.covered_days = Some(match group.covered_days {
            Some(prev) => prev.min(prov_covered),
            None => prov_covered,
        });

        if !history.daily.is_empty() {
            for entry in &history.daily {
                let req = entry.total_requests.unwrap_or(0);
                if entry.cost_usd.is_some() || entry.total_tokens == Some(0) {
                    priced_requests = priced_requests.saturating_add(req);
                } else {
                    unpriced_requests = unpriced_requests.saturating_add(req);
                }
            }
        } else if let Some(req) = history.last_30_days_requests {
            if prov_cost.is_some() {
                priced_requests = priced_requests.saturating_add(req);
            } else {
                unpriced_requests = unpriced_requests.saturating_add(req);
            }
        }
        unpriced_requests = unpriced_requests.saturating_add(incomplete_reqs);
    }

    let priced_provider_count: usize = groups.values().map(|g| g.priced_provider_count).sum();
    let has_unpriced_providers = included_provider_count < total_provider_count
        || groups.values().any(|g| g.total_cost.is_none());

    let priced_groups: Vec<(&String, &GroupAgg)> = groups
        .iter()
        .filter(|(_, g)| g.total_cost.is_some())
        .collect();
    let primary_spend_text = if priced_groups.is_empty() {
        "Spend unavailable".to_string()
    } else {
        priced_groups
            .into_iter()
            .map(|(code, g)| {
                let cost = g.total_cost.unwrap_or(0.0);
                let formatted = format::currency_string(cost, code);
                let is_partial = g.has_partial_cost
                    || g.priced_provider_count < g.provider_count
                    || has_unpriced_providers;
                if is_partial {
                    format!("~{formatted}")
                } else {
                    formatted
                }
            })
            .collect::<Vec<_>>()
            .join(" + ")
    };

    let total_tokens: u64 = groups.values().filter_map(|g| g.total_tokens).sum();
    let has_tokens = groups.values().any(|g| g.total_tokens.is_some());
    let token_text =
        has_tokens.then(|| format!("{} tokens", format::share_stats_compact_count(total_tokens)));

    let covered_days = groups
        .values()
        .filter_map(|g| g.covered_days)
        .max()
        .unwrap_or(0);
    let history_coverage_text = format!("Coverage: {covered_days} / {requested_days}");

    let provenance_text = match (saw_vendor_metered, saw_estimate) {
        (true, true) => "Metered and list-price".to_string(),
        (true, false) => "Vendor-metered".to_string(),
        (false, true) => "List-price estimate".to_string(),
        (false, false) => "Provenance unavailable".to_string(),
    };

    let pricing_coverage_text = format!(
        "Priced {} · Unpriced {} · Unmetered 0 · Estimated 0",
        format::grouped_u64(priced_requests),
        format::grouped_u64(unpriced_requests),
    );

    Some(OverviewSpendSummary {
        day_range_text: format!("{requested_days}d"),
        primary_spend_text,
        provider_coverage_text: format!(
            "{priced_provider_count} of {total_provider_count} subscriptions have spend"
        ),
        token_text,
        history_coverage_text,
        provenance_text,
        pricing_coverage_text,
    })
}

/// Builds the `Buy Credits...` item on Codex when optional credits are enabled
/// and the provider exposes spendable credits (`StatusItemController+Menu.swift:483-484,1450-1462`).
pub fn buy_credits_entry(
    provider: &ProviderSnapshot,
    prefs: &CardPreferences,
) -> Option<MenuEntry> {
    if provider.id.as_str() != "codex" || !prefs.show_optional_credits_and_extra_usage {
        return None;
    }
    let credits = provider.credits.as_ref().and_then(displayable_value)?;
    if credits.credits_available == Some(false) {
        return None;
    }
    let url = provider_metadata(provider.id.as_str())
        .dashboard_url
        .unwrap_or("https://chatgpt.com/codex/settings/usage")
        .to_string();
    let action = MenuAction::BuyCredits { url };
    Some(MenuEntry::Action {
        title: "Buy Credits...".to_string(),
        system_image: action.system_image(),
        key_equivalent: action.key_equivalent(),
        enabled: true,
        action,
    })
}

/// Builds the usage & cost history submenus (`Plan Usage` and `Cost`) when the
/// snapshot carries utilization or cost data (`StatusItemController+UsageHistoryMenu.swift`,
/// `StatusItemController+CostMenuCard.swift`).
pub fn build_history_section(
    provider: &ProviderSnapshot,
    prefs: &CardPreferences,
) -> Vec<MenuEntry> {
    struct UtcFallbackDates;
    impl DateTexts for UtcFallbackDates {
        fn time_zone(&self) -> jiff::tz::TimeZone {
            jiff::tz::TimeZone::UTC
        }
        fn hour_minute(&self, date: Timestamp) -> String {
            let z = date.to_zoned(jiff::tz::TimeZone::UTC);
            z.strftime("%-I:%M %p").to_string()
        }
        fn month_day_hour_minute(&self, date: Timestamp) -> String {
            let z = date.to_zoned(jiff::tz::TimeZone::UTC);
            z.strftime("%b %-d at %-I:%M %p").to_string()
        }
        fn relative_abbreviated(&self, date: Timestamp, now: Timestamp) -> String {
            let secs = now.duration_since(date).as_secs();
            format!("{} min ago", secs / 60)
        }
    }
    let now = provider.updated_at.unwrap_or(Timestamp::UNIX_EPOCH);
    build_history_section_at(provider, prefs, now, &UtcFallbackDates)
}

/// Builds the usage & cost history submenus using explicit `now` and `dates`.
pub fn build_history_section_at(
    provider: &ProviderSnapshot,
    prefs: &CardPreferences,
    now: Timestamp,
    dates: &dyn DateTexts,
) -> Vec<MenuEntry> {
    let mut entries = Vec::new();
    if let Some(plan_usage) = plan_usage_submenu(provider, prefs, now, dates) {
        entries.push(plan_usage);
    }
    if let Some(cost) = cost_submenu(provider, dates) {
        entries.push(cost);
    }
    entries
}

/// `supportsQuotaBurndownSubmenu` (`StatusItemController+UsageHistoryMenu.swift:53-55`).
fn supports_quota_burndown_submenu(provider_id: &str) -> bool {
    matches!(provider_id, "codex" | "claude" | "kiro")
}

fn plan_usage_chart_submenu_items(
    provider: &ProviderSnapshot,
    now: Timestamp,
    dates: &dyn DateTexts,
) -> Vec<SubmenuEntry> {
    if provider
        .utilization
        .as_ref()
        .and_then(displayable_value)
        .is_none()
    {
        return Vec::new();
    }
    let burndown = supports_quota_burndown_submenu(provider.id.as_str())
        .then(|| QuotaBurndownChartModel::from_provider(provider, now, dates))
        .flatten();
    let util = PlanUtilizationChartModel::from_provider(provider, dates);
    let mut items = Vec::new();
    if let Some(b) = burndown {
        items.push(SubmenuEntry::QuotaBurndownChart(Box::new(b)));
    }
    if let Some(u) = util {
        if !items.is_empty() {
            items.push(SubmenuEntry::Separator);
        }
        items.push(SubmenuEntry::PlanUtilizationChart(Box::new(u)));
    }
    items
}

fn plan_usage_submenu(
    provider: &ProviderSnapshot,
    prefs: &CardPreferences,
    now: Timestamp,
    dates: &dyn DateTexts,
) -> Option<MenuEntry> {
    let series_list: &Vec<PlanUtilizationSeries> =
        provider.utilization.as_ref().and_then(displayable_value)?;
    let mut items = plan_usage_chart_submenu_items(provider, now, dates);
    if items.is_empty() {
        let style = if prefs.show_used {
            PercentStyle::Used
        } else {
            PercentStyle::Left
        };
        for series in series_list {
            let Some(latest) = series.entries.last() else {
                continue;
            };
            let title = series
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| default_series_title(&series.name));
            let pct = if prefs.show_used {
                latest.used_percent.clamp(0.0, 100.0)
            } else {
                (100.0 - latest.used_percent).clamp(0.0, 100.0)
            };
            let pct_text = format::percent_text(pct, style.label_suffix());
            items.push(SubmenuEntry::SecondaryText(format!("{title}: {pct_text}")));
        }
    }
    if items.is_empty() {
        return None;
    }
    Some(MenuEntry::Submenu {
        title: "Plan Usage".to_string(),
        system_image: None,
        items,
    })
}

fn default_series_title(name: &str) -> &str {
    match name {
        "session" => "Session",
        "weekly" => "Weekly",
        "monthly" => "Monthly",
        "opus" => "Sonnet",
        "antigravityGemini" => "Gemini Models",
        "antigravityClaudeGpt" => "Claude and GPT",
        other => other,
    }
}

fn cost_submenu(provider: &ProviderSnapshot, dates: &dyn DateTexts) -> Option<MenuEntry> {
    let history: Option<&CostHistory> = provider.cost_history.as_ref().and_then(displayable_value);
    let cost: Option<&Cost> = provider.cost.as_ref().and_then(displayable_value);
    let mut items = Vec::new();

    if let Some(history) = history {
        let currency = history.currency_code.as_deref().unwrap_or("USD");
        if let Some(chart) =
            CostHistoryChartModel::from_provider_with_dates(provider, currency, dates)
        {
            return Some(MenuEntry::Submenu {
                title: "Cost".to_string(),
                system_image: None,
                items: vec![SubmenuEntry::CostHistoryChart(Box::new(chart))],
            });
        }
        if history.session_cost_usd.is_some() || history.session_tokens.is_some() {
            let cost_str = history
                .session_cost_usd
                .map(|c| format_currency(c, currency))
                .unwrap_or_else(|| "—".to_string());
            let line = match history.session_tokens {
                Some(tokens) => format!(
                    "Today: {cost_str} · {} tokens",
                    format::token_count_string(tokens as i64)
                ),
                None => format!("Today: {cost_str}"),
            };
            items.push(SubmenuEntry::SecondaryText(line));
        }

        let month_tokens = history.last_30_days_tokens.or_else(|| {
            let sum: u64 = history.daily.iter().filter_map(|d| d.total_tokens).sum();
            (sum > 0).then_some(sum)
        });
        if history.last_30_days_cost_usd.is_some() || month_tokens.is_some() {
            let window_label = history
                .history_label
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| match history.history_days {
                    Some(1) => "Today".to_string(),
                    Some(days) => format!("Last {days} days"),
                    None => "Last 30 days".to_string(),
                });
            let cost_str = history
                .last_30_days_cost_usd
                .map(|c| format_currency(c, currency))
                .unwrap_or_else(|| "—".to_string());
            let line = match month_tokens {
                Some(tokens) => format!(
                    "{window_label}: {cost_str} · {} tokens",
                    format::token_count_string(tokens as i64)
                ),
                None => format!("{window_label}: {cost_str}"),
            };
            items.push(SubmenuEntry::SecondaryText(line));
        }

        if let Some(top) = &history.top_model {
            let model_name = top.model_name.trim();
            if !model_name.is_empty() {
                let detail = match (top.cost_usd, top.total_tokens) {
                    (Some(c), Some(t)) => format!(
                        "{} · {} tokens",
                        format_currency(c, currency),
                        format::token_count_string(t as i64)
                    ),
                    (Some(c), None) => format_currency(c, currency),
                    (None, Some(t)) => {
                        format!("{} tokens", format::token_count_string(t as i64))
                    }
                    (None, None) => String::new(),
                };
                let line = if detail.is_empty() {
                    format!("Top model: {model_name}")
                } else {
                    format!("Top model: {model_name} ({detail})")
                };
                items.push(SubmenuEntry::SecondaryText(line));
            }
        }

        for hint in &history.hint_lines {
            let trimmed = hint.trim();
            if !trimmed.is_empty() {
                items.push(SubmenuEntry::SecondaryText(trimmed.to_string()));
            }
        }
    }

    if let Some(cost) = cost {
        let mut cost_lines = Vec::new();
        if let Some(limit) = cost.limit.filter(|&l| l > 0.0) {
            let used = format_currency(cost.used, &cost.currency_code);
            let lim = format_currency(limit, &cost.currency_code);
            cost_lines.push(SubmenuEntry::SecondaryText(format!(
                "Extra usage: {used} / {lim}"
            )));
        }
        if let Some(balance) = cost.balance
            && cost.balance_is_unavailable != Some(true)
        {
            let bal = format_currency(balance, &cost.currency_code);
            cost_lines.push(SubmenuEntry::SecondaryText(format!("Balance: {bal}")));
        }
        if !cost_lines.is_empty() {
            if !items.is_empty() {
                items.push(SubmenuEntry::Separator);
            }
            items.extend(cost_lines);
        }
    }

    if items.is_empty() {
        return None;
    }
    Some(MenuEntry::Submenu {
        title: "Cost".to_string(),
        system_image: None,
        items,
    })
}

fn format_currency(amount: f64, currency_code: &str) -> String {
    format::currency_string(amount, currency_code)
}

/// Builds the contextual actions section for `provider` (`MenuDescriptor.actionsSection`).
pub fn build_actions_section(
    provider: &ProviderSnapshot,
    now: Timestamp,
    dates: &dyn DateTexts,
) -> Vec<MenuEntry> {
    let meta = provider_metadata(provider.id.as_str());
    let mut entries = Vec::new();

    if meta.supports_login_flow {
        let (title, action) = login_menu_action(provider);
        entries.push(MenuEntry::Action {
            title,
            system_image: action.system_image(),
            key_equivalent: action.key_equivalent(),
            enabled: true,
            action,
        });
    }

    if let Some(url) = meta.dashboard_url {
        let action = MenuAction::OpenDashboard {
            url: url.to_string(),
        };
        entries.push(MenuEntry::Action {
            title: "Usage Dashboard".to_string(),
            system_image: action.system_image(),
            key_equivalent: action.key_equivalent(),
            enabled: true,
            action,
        });
    }

    let has_non_none_indicator =
        active_status(provider).is_some_and(|(s, _)| s.indicator != StatusIndicator::None);
    let status_url = meta
        .status_page_url
        .or(meta.status_link_url)
        .map(str::to_string)
        .or_else(|| {
            active_status(provider)
                .and_then(|(s, _)| s.page_url.as_deref())
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string)
        });

    if let Some(url) = status_url {
        if meta.status_page_url.is_some() || has_non_none_indicator {
            let mut items = status_submenu_items(provider);
            if !items.is_empty() {
                items.push(SubmenuEntry::Separator);
            }
            items.push(SubmenuEntry::Action {
                title: "Open Status Page".to_string(),
                action: MenuAction::OpenStatusPage { url },
                enabled: true,
            });
            entries.push(MenuEntry::Submenu {
                title: "Status Page".to_string(),
                system_image: Some("waveform.path.ecg"),
                items,
            });
        } else {
            let action = MenuAction::OpenStatusPage { url };
            entries.push(MenuEntry::Action {
                title: "Status Page".to_string(),
                system_image: action.system_image(),
                key_equivalent: action.key_equivalent(),
                enabled: true,
                action,
            });
        }
    }

    if let Some(line) = status_line(provider, now, dates) {
        entries.push(MenuEntry::SecondaryText(line));
    }

    entries
}

/// Builds the meta section at the bottom of the menu (`MenuDescriptor.metaSection`).
pub fn build_meta_section(connected: bool, version_text: &str) -> Vec<MenuEntry> {
    let about_title = if version_text.trim().is_empty() {
        "About AriadUsage".to_string()
    } else {
        format!("About AriadUsage (v{})", version_text.trim())
    };
    vec![
        MenuEntry::Action {
            title: "Refresh".to_string(),
            system_image: MenuAction::Refresh.system_image(),
            key_equivalent: MenuAction::Refresh.key_equivalent(),
            enabled: connected,
            action: MenuAction::Refresh,
        },
        MenuEntry::Action {
            title: "Settings...".to_string(),
            system_image: MenuAction::Settings.system_image(),
            key_equivalent: MenuAction::Settings.key_equivalent(),
            enabled: true,
            action: MenuAction::Settings,
        },
        MenuEntry::Action {
            title: about_title,
            system_image: MenuAction::About.system_image(),
            key_equivalent: MenuAction::About.key_equivalent(),
            enabled: true,
            action: MenuAction::About,
        },
        MenuEntry::Action {
            title: "Quit".to_string(),
            system_image: MenuAction::Quit.system_image(),
            key_equivalent: MenuAction::Quit.key_equivalent(),
            enabled: true,
            action: MenuAction::Quit,
        },
    ]
}

/// Resolves the account login/switch action for `provider`
/// (`MenuDescriptor.actionsSection` + `ProviderImplementation.loginMenuAction`).
pub fn login_menu_action(provider: &ProviderSnapshot) -> (String, MenuAction) {
    let provider_id = provider.id.as_str().to_string();
    match provider.id.as_str() {
        "codex" => (
            "Add Account...".to_string(),
            MenuAction::AddAccount { provider_id },
        ),
        "antigravity" => (
            "Add Account...".to_string(),
            MenuAction::SwitchAccount { provider_id },
        ),
        "claude" => {
            let label = if has_account(provider) {
                "Switch Account..."
            } else {
                "Sign in with Claude Code..."
            };
            (label.to_string(), MenuAction::SwitchAccount { provider_id })
        }
        _ => {
            let label = if has_account(provider) {
                "Switch Account..."
            } else {
                "Add Account..."
            };
            (label.to_string(), MenuAction::SwitchAccount { provider_id })
        }
    }
}

/// Whether `provider` currently has a signed-in account (`MenuDescriptor.hasAccount`).
pub fn has_account(provider: &ProviderSnapshot) -> bool {
    let identity: Option<&Identity> = provider.identity.as_ref().and_then(displayable_value);
    if identity
        .and_then(|id| id.account_email.as_deref())
        .is_some_and(|email| !email.trim().is_empty())
    {
        return true;
    }
    let has_windows = has_rate_limit_windows(provider);
    if provider.id.as_str() == "claude" {
        if identity.is_some() && has_windows {
            return true;
        }
        if has_windows && provider.last_error.is_none() && provider.source_mode == "cli" {
            return true;
        }
    }
    false
}

fn has_rate_limit_windows(provider: &ProviderSnapshot) -> bool {
    window_present(provider.windows.primary.as_ref())
        || window_present(provider.windows.secondary.as_ref())
        || window_present(provider.windows.tertiary.as_ref())
}

fn window_present(metric: Option<&Metric<RateWindow>>) -> bool {
    metric.is_some_and(|m| displayable_value(m).is_some())
}

fn displayable_value<T>(metric: &Metric<T>) -> Option<&T> {
    match metric.state {
        MetricState::Value | MetricState::Stale => metric.value.as_ref(),
        _ => None,
    }
}

fn active_status(provider: &ProviderSnapshot) -> Option<(&ProviderStatus, Option<Timestamp>)> {
    let metric = provider.status.as_ref()?;
    let status = displayable_value(metric)?;
    let updated_at = status.updated_at.or(metric.as_of);
    Some((status, updated_at))
}

fn status_submenu_items(provider: &ProviderSnapshot) -> Vec<SubmenuEntry> {
    let Some((status, _)) = active_status(provider) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    if status.indicator != StatusIndicator::None {
        let desc = status
            .description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| indicator_label(status.indicator));
        items.push(SubmenuEntry::SecondaryText(desc.to_string()));
    }
    for comp in &status.components {
        items.push(SubmenuEntry::SecondaryText(format!(
            "{} — {}",
            comp.name.trim(),
            comp.status.trim()
        )));
        for child in &comp.children {
            items.push(SubmenuEntry::SecondaryText(format!(
                "  {} — {}",
                child.name.trim(),
                child.status.trim()
            )));
        }
    }
    items
}

/// Formats `<status> — <freshness>` when `status.indicator != None` (`MenuDescriptor.statusLine`).
pub fn status_line(
    provider: &ProviderSnapshot,
    now: Timestamp,
    dates: &dyn DateTexts,
) -> Option<String> {
    let (status, updated_at) = active_status(provider)?;
    if status.indicator == StatusIndicator::None {
        return None;
    }
    let label = status
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| indicator_label(status.indicator));
    if let Some(updated_at) = updated_at {
        let freshness = format::updated_string(updated_at, now, dates);
        Some(format!("{label} — {freshness}"))
    } else {
        Some(label.to_string())
    }
}

fn indicator_label(indicator: StatusIndicator) -> &'static str {
    match indicator {
        StatusIndicator::None => "Operational",
        StatusIndicator::Minor => "Partial outage",
        StatusIndicator::Major => "Major outage",
        StatusIndicator::Critical => "Critical outage",
        StatusIndicator::Maintenance => "Maintenance",
        StatusIndicator::Unknown => "Status unknown",
    }
}
