// Ported from CodexBar Tests/CodexBarTests/MenuDescriptorAntigravityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/MenuDescriptorCodexManagedFallbackTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/MenuDescriptorUpdateAndVersionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatusItemControllerMenuTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#![cfg(unix)]

mod common;

use ariadusage_macos::card::CardPreferences;
use ariadusage_macos::menu_descriptor::{
    self, MenuAction, MenuEntry, PrimaryContent, SubmenuEntry,
};
use ariadusage_macos::switcher::SwitcherSelection;
use ariadusage_protocol::fixture::generate_scenario_snapshot;
use ariadusage_protocol::metric::{Confidence, Metric};
use ariadusage_protocol::usage::{ProviderStatus, StatusIndicator};
use common::FixedDates;
use jiff::{SignedDuration, Timestamp};

#[test]
fn disconnected_menu_shows_connection_status_and_meta_section() {
    let now = Timestamp::from_second(1_770_000_000).unwrap();
    let structure = menu_descriptor::build(
        None,
        false,
        None,
        &CardPreferences::default(),
        now,
        &FixedDates::utc(),
    );
    assert!(structure.selection.is_none());
    assert!(structure.switcher.is_empty());
    assert_eq!(
        structure.primary,
        PrimaryContent::Disconnected {
            message: "Waiting for the AriadUsage engine…".to_string()
        }
    );
    assert!(structure.history.is_empty());
    assert!(structure.actions.is_empty());
    assert_eq!(
        structure.meta,
        vec![
            MenuEntry::Action {
                title: "Refresh".to_string(),
                system_image: Some("arrow.clockwise"),
                key_equivalent: "r",
                enabled: false,
                action: MenuAction::Refresh,
            },
            MenuEntry::Action {
                title: "Settings...".to_string(),
                system_image: Some("gearshape"),
                key_equivalent: ",",
                enabled: true,
                action: MenuAction::Settings,
            },
            MenuEntry::Action {
                title: format!("About AriadUsage (v{})", menu_descriptor::APP_VERSION),
                system_image: Some("info.circle"),
                key_equivalent: "",
                enabled: true,
                action: MenuAction::About,
            },
            MenuEntry::Action {
                title: "Quit".to_string(),
                system_image: Some("xmark.rectangle"),
                key_equivalent: "q",
                enabled: true,
                action: MenuAction::Quit,
            },
        ]
    );
}

#[test]
fn overview_tab_lists_share_action_and_enabled_provider_cards() {
    let snap = generate_scenario_snapshot(0);
    let now = snap.generated_at;
    let structure = menu_descriptor::build(
        Some(&snap),
        true,
        Some(&SwitcherSelection::Overview),
        &CardPreferences::default(),
        now,
        &FixedDates::utc(),
    );
    assert_eq!(structure.selection, Some(SwitcherSelection::Overview));
    assert_eq!(structure.switcher.len(), 4);
    assert!(structure.history.is_empty());
    assert!(structure.actions.is_empty());

    let PrimaryContent::Overview {
        spend_summary,
        share_action,
        cards,
    } = &structure.primary
    else {
        panic!("expected Overview primary content");
    };
    let summary = spend_summary
        .as_ref()
        .expect("overview spend summary present");
    assert_eq!(summary.day_range_text, "30d");
    assert!(summary.primary_spend_text.starts_with("~$"));
    assert_eq!(
        summary.provider_coverage_text,
        "2 of 3 subscriptions have spend"
    );
    assert!(
        summary
            .token_text
            .as_deref()
            .is_some_and(|t| t.ends_with(" tokens"))
    );
    assert_eq!(summary.history_coverage_text, "Coverage: 30 / 30");
    assert_eq!(summary.provenance_text, "Metered and list-price");
    assert_eq!(
        summary.pricing_coverage_text,
        "Priced 3,578 · Unpriced 0 · Unmetered 0 · Estimated 0"
    );
    assert_eq!(
        *share_action,
        Some(MenuEntry::Action {
            title: "Share Usage Snapshot…".to_string(),
            system_image: Some("square.and.arrow.up"),
            key_equivalent: "",
            enabled: true,
            action: MenuAction::ShareUsageSnapshot,
        })
    );
    let ids: Vec<&str> = cards.iter().map(|c| c.provider_id.as_str()).collect();
    assert_eq!(ids, ["claude", "codex", "antigravity"]);
    assert_eq!(cards[0].submenu_title.as_deref(), Some("Cost"));
    assert!(cards[0].model.shows_submenu_indicator);
    assert!(matches!(
        cards[0].submenu.as_slice(),
        [SubmenuEntry::CostHistoryChart(_)]
    ));
    assert_eq!(cards[1].submenu_title.as_deref(), Some("Cost"));
    assert!(cards[1].model.shows_submenu_indicator);
    assert_eq!(cards[2].submenu_title, None);
    assert!(!cards[2].model.shows_submenu_indicator);
}

#[test]
fn overview_without_cost_history_omits_spend_summary_and_share_action() {
    let mut snap = generate_scenario_snapshot(0);
    for provider in &mut snap.providers {
        provider.cost_history = None;
    }
    let now = snap.generated_at;
    let structure = menu_descriptor::build(
        Some(&snap),
        true,
        Some(&SwitcherSelection::Overview),
        &CardPreferences::default(),
        now,
        &FixedDates::utc(),
    );
    let PrimaryContent::Overview {
        spend_summary,
        share_action,
        cards,
    } = &structure.primary
    else {
        panic!("expected Overview primary content");
    };
    assert!(spend_summary.is_none());
    assert!(share_action.is_none());
    assert_eq!(cards.len(), 3);
}

#[test]
fn claude_tab_builds_history_submenus_switch_account_dashboard_and_status_page() {
    let snap = generate_scenario_snapshot(0);
    let now = "2026-10-06T12:53:00Z".parse::<Timestamp>().unwrap();
    let structure = menu_descriptor::build(
        Some(&snap),
        true,
        Some(&SwitcherSelection::Provider("claude".to_string())),
        &CardPreferences::default(),
        now,
        &FixedDates::utc(),
    );
    assert_eq!(
        structure.selection,
        Some(SwitcherSelection::Provider("claude".to_string()))
    );
    let PrimaryContent::Provider {
        sections,
        cost_submenu,
        buy_credits,
        ..
    } = &structure.primary
    else {
        panic!("expected Provider primary content");
    };
    assert!(buy_credits.is_none());
    assert_eq!(sections.len(), 2);
    assert!(sections[0].has_cost_submenu);
    assert_eq!(cost_submenu.len(), 1);
    assert!(matches!(
        &cost_submenu[0],
        SubmenuEntry::CostHistoryChart(_)
    ));

    assert_eq!(structure.history.len(), 2);
    let MenuEntry::Submenu {
        title: plan_title,
        items: plan_items,
        ..
    } = &structure.history[0]
    else {
        panic!("expected Plan Usage submenu");
    };
    assert_eq!(plan_title, "Plan Usage");
    assert_eq!(plan_items.len(), 3);
    assert!(matches!(
        &plan_items[0],
        SubmenuEntry::QuotaBurndownChart(_)
    ));
    assert_eq!(plan_items[1], SubmenuEntry::Separator);
    assert!(matches!(
        &plan_items[2],
        SubmenuEntry::PlanUtilizationChart(_)
    ));
    let MenuEntry::Submenu {
        title: cost_title,
        items: cost_items,
        ..
    } = &structure.history[1]
    else {
        panic!("expected Cost submenu");
    };
    assert_eq!(cost_title, "Cost");
    assert_eq!(cost_items.len(), 1);
    assert!(matches!(&cost_items[0], SubmenuEntry::CostHistoryChart(_)));

    assert_eq!(
        structure.actions,
        vec![
            MenuEntry::Action {
                title: "Switch Account...".to_string(),
                system_image: Some("key"),
                key_equivalent: "",
                enabled: true,
                action: MenuAction::SwitchAccount {
                    provider_id: "claude".to_string()
                },
            },
            MenuEntry::Action {
                title: "Usage Dashboard".to_string(),
                system_image: Some("chart.xyaxis.line"),
                key_equivalent: "",
                enabled: true,
                action: MenuAction::OpenDashboard {
                    url: "https://claude.ai/settings/usage".to_string()
                },
            },
            MenuEntry::Submenu {
                title: "Status Page".to_string(),
                system_image: Some("waveform.path.ecg"),
                items: vec![
                    SubmenuEntry::SecondaryText("Elevated latency on Claude API".to_string()),
                    SubmenuEntry::SecondaryText("Claude API — Degraded Performance".to_string()),
                    SubmenuEntry::SecondaryText(
                        "  Messages API — Degraded Performance".to_string()
                    ),
                    SubmenuEntry::SecondaryText("  Message Batches — Operational".to_string()),
                    SubmenuEntry::SecondaryText("claude.ai — Operational".to_string()),
                    SubmenuEntry::SecondaryText("Console — Operational".to_string()),
                    SubmenuEntry::Separator,
                    SubmenuEntry::Action {
                        title: "Open Status Page".to_string(),
                        action: MenuAction::OpenStatusPage {
                            url: "https://status.claude.com/".to_string()
                        },
                        enabled: true,
                    },
                ],
            },
            MenuEntry::SecondaryText(
                "Elevated latency on Claude API — Updated 5 min ago".to_string()
            ),
        ]
    );
}

#[test]
fn claude_without_account_shows_sign_in_with_claude_code() {
    let mut snap = generate_scenario_snapshot(0);
    let claude = &mut snap.providers[0];
    claude.windows.primary = None;
    claude.windows.secondary = None;
    claude.windows.tertiary = None;
    claude.identity = None;
    let (title, action) = menu_descriptor::login_menu_action(claude);
    assert_eq!(title, "Sign in with Claude Code...");
    assert_eq!(
        action,
        MenuAction::SwitchAccount {
            provider_id: "claude".to_string()
        }
    );
}

#[test]
fn codex_tab_includes_buy_credits_history_submenus_and_add_account() {
    let snap = generate_scenario_snapshot(0);
    let now = snap.generated_at;
    let structure = menu_descriptor::build(
        Some(&snap),
        true,
        Some(&SwitcherSelection::Provider("codex".to_string())),
        &CardPreferences::default(),
        now,
        &FixedDates::utc(),
    );
    let PrimaryContent::Provider { buy_credits, .. } = &structure.primary else {
        panic!("expected Provider primary content");
    };
    assert_eq!(
        *buy_credits,
        Some(MenuEntry::Action {
            title: "Buy Credits...".to_string(),
            system_image: Some("plus.circle"),
            key_equivalent: "",
            enabled: true,
            action: MenuAction::BuyCredits {
                url: "https://chatgpt.com/codex/settings/usage".to_string(),
            },
        })
    );
    assert_eq!(structure.history.len(), 2);
    assert_eq!(
        structure.actions,
        vec![
            MenuEntry::Action {
                title: "Add Account...".to_string(),
                system_image: Some("plus"),
                key_equivalent: "",
                enabled: true,
                action: MenuAction::AddAccount {
                    provider_id: "codex".to_string()
                },
            },
            MenuEntry::Action {
                title: "Usage Dashboard".to_string(),
                system_image: Some("chart.xyaxis.line"),
                key_equivalent: "",
                enabled: true,
                action: MenuAction::OpenDashboard {
                    url: "https://chatgpt.com/codex/settings/usage".to_string()
                },
            },
            MenuEntry::Submenu {
                title: "Status Page".to_string(),
                system_image: Some("waveform.path.ecg"),
                items: vec![
                    SubmenuEntry::SecondaryText("Codex Cloud Agent — Operational".to_string()),
                    SubmenuEntry::SecondaryText(
                        "  Container Sandbox — Degraded Performance".to_string()
                    ),
                    SubmenuEntry::SecondaryText("Responses API — Operational".to_string()),
                    SubmenuEntry::Separator,
                    SubmenuEntry::Action {
                        title: "Open Status Page".to_string(),
                        action: MenuAction::OpenStatusPage {
                            url: "https://status.openai.com/".to_string()
                        },
                        enabled: true,
                    },
                ],
            },
        ]
    );
}

#[test]
fn antigravity_tab_hides_missing_history_and_uses_plain_status_page_link() {
    let snap = generate_scenario_snapshot(0);
    let now = snap.generated_at;
    let structure = menu_descriptor::build(
        Some(&snap),
        true,
        Some(&SwitcherSelection::Provider("antigravity".to_string())),
        &CardPreferences::default(),
        now,
        &FixedDates::utc(),
    );
    assert!(structure.history.is_empty());
    assert_eq!(
        structure.actions,
        vec![
            MenuEntry::Action {
                title: "Add Account...".to_string(),
                system_image: Some("key"),
                key_equivalent: "",
                enabled: true,
                action: MenuAction::SwitchAccount {
                    provider_id: "antigravity".to_string()
                },
            },
            MenuEntry::Action {
                title: "Status Page".to_string(),
                system_image: Some("waveform.path.ecg"),
                key_equivalent: "",
                enabled: true,
                action: MenuAction::OpenStatusPage {
                    url: "https://www.google.com/appsstatus/dashboard/".to_string()
                },
            },
        ]
    );
}

#[test]
fn non_operational_provider_status_adds_status_line_and_submenu_description() {
    let mut snap = generate_scenario_snapshot(0);
    let now = snap.generated_at;
    let updated = Some(now - SignedDuration::from_mins(5));
    snap.providers[2].status = Some(Metric::value(
        ProviderStatus {
            indicator: StatusIndicator::Minor,
            description: None,
            updated_at: updated,
            page_url: None,
            components: Vec::new(),
        },
        updated,
        None,
        Confidence::Exact,
    ));
    let actions =
        menu_descriptor::build_actions_section(&snap.providers[2], now, &FixedDates::utc());
    assert_eq!(
        actions.last(),
        Some(&MenuEntry::SecondaryText(
            "Partial outage — Updated 5 min ago".to_string()
        ))
    );
    assert!(actions.contains(&MenuEntry::Submenu {
        title: "Status Page".to_string(),
        system_image: Some("waveform.path.ecg"),
        items: vec![
            SubmenuEntry::SecondaryText("Partial outage".to_string()),
            SubmenuEntry::Separator,
            SubmenuEntry::Action {
                title: "Open Status Page".to_string(),
                action: MenuAction::OpenStatusPage {
                    url: "https://www.google.com/appsstatus/dashboard/".to_string(),
                },
                enabled: true,
            },
        ],
    }));
}

#[test]
fn antigravity_with_utilization_attaches_plan_usage_without_burndown_and_codex_without_history_still_splits_credits()
 {
    use ariadusage_protocol::usage::{PlanUtilizationEntry, PlanUtilizationSeries};

    let mut snap = generate_scenario_snapshot(0);
    let now = snap.generated_at;
    snap.providers[2].utilization = Some(Metric::value(
        vec![
            PlanUtilizationSeries {
                name: "antigravityGemini".to_string(),
                title: Some("Gemini Models".to_string()),
                window_minutes: 1440,
                entries: vec![PlanUtilizationEntry {
                    captured_at: now - SignedDuration::from_hours(2),
                    used_percent: 25.0,
                    resets_at: Some(now + SignedDuration::from_hours(22)),
                }],
            },
            PlanUtilizationSeries {
                name: "antigravityClaudeGpt".to_string(),
                title: Some("Claude and GPT".to_string()),
                window_minutes: 1440,
                entries: vec![PlanUtilizationEntry {
                    captured_at: now - SignedDuration::from_hours(2),
                    used_percent: 40.0,
                    resets_at: Some(now + SignedDuration::from_hours(22)),
                }],
            },
        ],
        Some(now),
        None,
        Confidence::Exact,
    ));

    // Overview row for Antigravity falls back to "Plan Usage" chart submenu without burndown.
    let ov_structure = menu_descriptor::build(
        Some(&snap),
        true,
        Some(&SwitcherSelection::Overview),
        &CardPreferences::default(),
        now,
        &FixedDates::utc(),
    );
    let PrimaryContent::Overview { cards, .. } = &ov_structure.primary else {
        panic!("expected Overview");
    };
    assert_eq!(cards[2].submenu_title.as_deref(), Some("Plan Usage"));
    assert!(cards[2].model.shows_submenu_indicator);
    assert!(matches!(
        cards[2].submenu.as_slice(),
        [SubmenuEntry::PlanUtilizationChart(_)]
    ));

    // Antigravity tab builds Plan Usage history submenu with only PlanUtilizationChart.
    let anti_structure = menu_descriptor::build(
        Some(&snap),
        true,
        Some(&SwitcherSelection::Provider("antigravity".to_string())),
        &CardPreferences::default(),
        now,
        &FixedDates::utc(),
    );
    assert_eq!(anti_structure.history.len(), 1);
    let MenuEntry::Submenu { title, items, .. } = &anti_structure.history[0] else {
        panic!("expected Plan Usage submenu");
    };
    assert_eq!(title, "Plan Usage");
    assert!(matches!(
        items.as_slice(),
        [SubmenuEntry::PlanUtilizationChart(_)]
    ));

    // Codex with credits and extra usage but no cost history still splits into
    // HeaderAndUsage + CreditsOnly + ExtraUsageOnly, without attaching a Cost
    // submenu to HeaderAndUsage.
    snap.providers[1].cost_history = None;
    let codex_structure = menu_descriptor::build(
        Some(&snap),
        true,
        Some(&SwitcherSelection::Provider("codex".to_string())),
        &CardPreferences::default(),
        now,
        &FixedDates::utc(),
    );
    let PrimaryContent::Provider {
        card,
        sections,
        cost_submenu,
        ..
    } = &codex_structure.primary
    else {
        panic!("expected Provider");
    };
    assert!(!card.shows_submenu_indicator);
    assert_eq!(sections.len(), 3);
    assert!(!sections[0].has_cost_submenu);
    assert!(cost_submenu.is_empty());
}
