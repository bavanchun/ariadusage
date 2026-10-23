// Ported from CodexBar Tests/CodexBarTests/MenuDescriptorTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

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
        share_action,
        cards,
    } = &structure.primary
    else {
        panic!("expected Overview primary content");
    };
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
    let PrimaryContent::Provider { buy_credits, .. } = &structure.primary else {
        panic!("expected Provider primary content");
    };
    assert!(buy_credits.is_none());

    assert_eq!(structure.history.len(), 2);
    assert_eq!(
        structure.history[0],
        MenuEntry::Submenu {
            title: "Plan Usage".to_string(),
            system_image: None,
            items: vec![
                SubmenuEntry::SecondaryText("Session: 58% left".to_string()),
                SubmenuEntry::SecondaryText("Weekly: 36% left".to_string()),
                SubmenuEntry::SecondaryText("Sonnet: 82% left".to_string()),
            ],
        }
    );
    assert!(matches!(
        &structure.history[1],
        MenuEntry::Submenu { title, .. } if title == "Cost"
    ));

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
