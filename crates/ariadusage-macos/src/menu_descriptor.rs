// Ported from CodexBar Sources/CodexBar/MenuDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+Menu.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+UsageHistoryMenu.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+CostMenuCard.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+OverviewSpend.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/Providers/Claude/ClaudeProviderImplementation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/Providers/Codex/CodexProviderImplementation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/Providers/Antigravity/AntigravityProviderImplementation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Pure-Rust menu structure for the status item dropdown.
//!
//! Given the current [`EngineSnapshot`], connection state and selected tab,
//! [`build`] produces the switcher segments, primary card content (Overview
//! rows or single-provider usage card), history submenus (`Plan Usage`, `Cost`),
//! contextual provider actions and meta section in CodexBar's order and wording
//! with AriadUsage's own identity.

use ariadusage_protocol::metric::{Metric, MetricState};
use ariadusage_protocol::snapshot::{EngineSnapshot, ProviderSnapshot};
use ariadusage_protocol::usage::{
    Cost, CostHistory, Identity, PlanUtilizationSeries, ProviderStatus, RateWindow, StatusIndicator,
};
use jiff::Timestamp;

use crate::card::{self, CardModel, CardPreferences, PercentStyle};
use crate::format::{self, DateTexts};
use crate::presentation;
use crate::switcher::{self, SwitcherSegment, SwitcherSelection};

/// Default version string shown in `About AriadUsage (vX.Y.Z)`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

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

/// An item inside a submenu (`StatusItemController.makeStatusComponentsSubmenu`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmenuEntry {
    SecondaryText(String),
    Separator,
    Action {
        title: String,
        action: MenuAction,
        enabled: bool,
    },
}

/// A row in the history, contextual actions or meta section (`MenuDescriptor.Entry`).
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// One provider row on the `Overview` tab (`OverviewMenuCardRowView`).
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewCard {
    pub provider_id: String,
    pub model: CardModel,
}

/// Primary card content above the history, action and meta sections.
#[derive(Debug, Clone, PartialEq)]
pub enum PrimaryContent {
    /// Shown while disconnected or when no providers are enabled.
    Disconnected { message: String },
    /// Shown when the `Overview` tab is active (`addOverviewRows`).
    Overview {
        share_action: Option<MenuEntry>,
        cards: Vec<OverviewCard>,
    },
    /// Shown when a single provider tab is active (`UsageMenuCardView`).
    Provider {
        provider_id: String,
        card: Box<CardModel>,
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
    let version = snapshot
        .map(|s| s.engine.version.as_str())
        .filter(|v| !v.is_empty())
        .unwrap_or(APP_VERSION);
    let meta = build_meta_section(connected, version);

    let Some(snapshot) = snapshot.filter(|_| connected) else {
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

    let switcher = switcher::segments(snapshot, prefs.show_used);
    match &selection {
        SwitcherSelection::Overview => {
            let share_action = has_overview_spend_data(&enabled).then(|| MenuEntry::Action {
                title: "Share Usage Snapshot…".to_string(),
                system_image: MenuAction::ShareUsageSnapshot.system_image(),
                key_equivalent: MenuAction::ShareUsageSnapshot.key_equivalent(),
                enabled: true,
                action: MenuAction::ShareUsageSnapshot,
            });
            let cards = enabled
                .iter()
                .map(|provider| OverviewCard {
                    provider_id: provider.id.as_str().to_string(),
                    model: card::provider_card(provider, prefs, now, dates),
                })
                .collect();
            MenuStructure {
                selection: Some(selection),
                switcher,
                primary: PrimaryContent::Overview {
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
            let card_model = card::provider_card(provider, prefs, now, dates);
            let buy_credits = buy_credits_entry(provider, prefs);
            let history = build_history_section(provider, prefs);
            let actions = build_actions_section(provider, now, dates);
            MenuStructure {
                selection: Some(selection),
                switcher,
                primary: PrimaryContent::Provider {
                    provider_id: provider.id.as_str().to_string(),
                    card: Box::new(card_model),
                    buy_credits,
                },
                history,
                actions,
                meta,
            }
        }
    }
}

/// Whether any enabled provider carries spend data for the Overview share action
/// (`StatusItemController+OverviewSpend.swift:148-163`).
fn has_overview_spend_data(enabled: &[&ProviderSnapshot]) -> bool {
    enabled.iter().any(|provider| {
        let has_history = provider
            .cost_history
            .as_ref()
            .and_then(displayable_value)
            .is_some_and(|h| {
                h.session_cost_usd.is_some()
                    || h.last_30_days_cost_usd.is_some()
                    || !h.daily.is_empty()
            });
        let has_cost = provider.cost.as_ref().and_then(displayable_value).is_some();
        has_history || has_cost
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
    let mut entries = Vec::new();
    if let Some(plan_usage) = plan_usage_submenu(provider, prefs) {
        entries.push(plan_usage);
    }
    if let Some(cost) = cost_submenu(provider) {
        entries.push(cost);
    }
    entries
}

fn plan_usage_submenu(provider: &ProviderSnapshot, prefs: &CardPreferences) -> Option<MenuEntry> {
    let series_list: &Vec<PlanUtilizationSeries> =
        provider.utilization.as_ref().and_then(displayable_value)?;
    let style = if prefs.show_used {
        PercentStyle::Used
    } else {
        PercentStyle::Left
    };
    let mut items = Vec::new();
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

fn cost_submenu(provider: &ProviderSnapshot) -> Option<MenuEntry> {
    let history: Option<&CostHistory> = provider.cost_history.as_ref().and_then(displayable_value);
    let cost: Option<&Cost> = provider.cost.as_ref().and_then(displayable_value);
    let mut items = Vec::new();

    if let Some(history) = history {
        let currency = history.currency_code.as_deref().unwrap_or("USD");
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
                    (None, Some(t)) => format!("{} tokens", format::token_count_string(t as i64)),
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
            let period = cost
                .period
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("This month");
            let used = format_currency(cost.used, &cost.currency_code);
            let lim = format_currency(limit, &cost.currency_code);
            cost_lines.push(SubmenuEntry::SecondaryText(format!(
                "Extra usage ({period}): {used} / {lim}"
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
    if currency_code.eq_ignore_ascii_case("USD") || currency_code.trim().is_empty() {
        format!("${amount:.2}")
    } else {
        format!("{} {amount:.2}", currency_code.trim())
    }
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
