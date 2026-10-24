// Ported from CodexBar Tests/CodexBarTests/PreferencesSelectionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/PreferencesNavigationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/SettingsSidebarAccessibilityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/SettingsWindowBehaviorTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#[cfg(unix)]
use std::sync::mpsc;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use ariadusage_macos::engine_client::{EngineClient, EngineEvent};
use ariadusage_macos::geometry::Size;
use ariadusage_macos::settings::{
    ABOUT_LICENSE_FOOTER, APP_VERSION, DockWindowDescriptor, FormButtonAction, FormControl,
    MISSING_ENGINE_INTENTS, ProviderStatusDotColor, SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH,
    SIDEBAR_MIN_WIDTH, SettingsPane, SettingsSelection, SettingsState, SidebarChipStyle,
    SidebarFilterState, SidebarModel, SidebarProviderInput, WINDOW_AUTOSAVE_NAME, WINDOW_HEIGHT,
    WINDOW_IDENTIFIER, WINDOW_WIDTH, agent_sessions_dependents_available, clamp_sidebar_width,
    combined_icon_dependents_available, icon_and_percent_dependents_available,
    inactive_display_contrast_available, pane_model, provider_accessibility_label,
    refresh_interval_label, secret_set_command_args, secret_set_terminal_command,
    should_promote_for_presented_window, should_use_regular_activation_policy,
    surprise_me_available, workday_tick_appearance_available,
};
use ariadusage_macos::settings_layout::{SettingsHitTarget, hit_test, layout_settings_window};
#[cfg(unix)]
use ariadusage_protocol::fixture::{FixtureConfig, start_fixture_server};
use ariadusage_protocol::ids::SettingId;
use ariadusage_protocol::settings::{
    ChoiceOption, DescriptorKind, NumberConfig, SettingDescriptor, SettingsPage, SettingsScope,
    SettingsSection,
};
use ariadusage_protocol::usage::StatusIndicator;

fn sample_app_settings() -> SettingsPage {
    SettingsPage::new(
        SettingsScope::App,
        vec![SettingsSection::new(
            "general",
            "General",
            vec![
                SettingDescriptor::new(
                    SettingId::new("refreshInterval").unwrap(),
                    "Refresh interval",
                    DescriptorKind::Number {
                        value: 900.0,
                        config: NumberConfig::default(),
                    },
                ),
                SettingDescriptor::new(
                    SettingId::new("menuBar.iconStyle").unwrap(),
                    "Menu bar style",
                    DescriptorKind::Choice {
                        selected: "bars".to_string(),
                        options: vec![
                            ChoiceOption::new("bars", "Bars"),
                            ChoiceOption::new("critters", "Critters"),
                        ],
                    },
                ),
                SettingDescriptor::new(
                    SettingId::new("menuBar.showUsed").unwrap(),
                    "Usage bars fill",
                    DescriptorKind::Toggle { value: false },
                ),
                SettingDescriptor::new(
                    SettingId::new("menuBar.mergeIcons").unwrap(),
                    "Merge icons",
                    DescriptorKind::Toggle { value: true },
                ),
                SettingDescriptor::new(
                    SettingId::new("menu.showOptionalCreditsAndExtraUsage").unwrap(),
                    "Show optional credits, spending & billing details",
                    DescriptorKind::Toggle { value: true },
                ),
                SettingDescriptor::new(
                    SettingId::new("providers.claude.apiKey").unwrap(),
                    "Claude API Key",
                    DescriptorKind::Secret {
                        is_set: true,
                        source: Some("keychain".to_string()),
                    },
                ),
            ],
        )],
    )
}

#[test]
fn settings_pane_persistence_tokens_round_trip_and_map_legacy_display() {
    for pane in SettingsPane::standard_app_panes(true) {
        let token = pane.persistence_token();
        assert_eq!(SettingsPane::from_persistence_token(&token), Some(pane));
    }
    assert_eq!(
        SettingsPane::from_persistence_token("display"),
        Some(SettingsPane::MenuBar)
    );
    assert_eq!(
        SettingsPane::from_persistence_token("provider:codex"),
        Some(SettingsPane::Provider("codex".to_string()))
    );
    assert_eq!(SettingsPane::from_persistence_token("provider:   "), None);
    assert_eq!(SettingsPane::from_persistence_token("unknown"), None);
}

#[test]
fn standard_app_panes_match_codexbar_sidebar_order() {
    let panes = SettingsPane::standard_app_panes(false);
    assert_eq!(
        panes,
        vec![
            SettingsPane::General,
            SettingsPane::ICloudSync,
            SettingsPane::UsageSpend,
            SettingsPane::Notifications,
            SettingsPane::MenuBar,
            SettingsPane::Menu,
            SettingsPane::Advanced,
            SettingsPane::Hooks,
            SettingsPane::Plugins,
            SettingsPane::About,
        ]
    );
    assert_eq!(
        SettingsPane::About.chip_style(),
        Some(SidebarChipStyle::AppIcon)
    );
    let with_debug = SettingsPane::standard_app_panes(true);
    assert_eq!(with_debug.len(), 11);
    assert_eq!(with_debug.last(), Some(&SettingsPane::Debug));
}

#[test]
fn settings_selection_navigates_back_and_forward_and_prunes_history() {
    let mut sel = SettingsSelection::new(SettingsPane::General);
    assert!(!sel.can_go_back());
    assert!(!sel.can_go_forward());

    sel.select(SettingsPane::MenuBar);
    sel.select(SettingsPane::Menu);
    sel.select(SettingsPane::About);
    assert_eq!(sel.pane(), &SettingsPane::About);
    assert_eq!(sel.about_root_requests(), 1);

    // Selecting About again bumps `about_root_requests` without pushing history.
    sel.select(SettingsPane::About);
    assert_eq!(sel.about_root_requests(), 2);
    assert_eq!(sel.back_stack().len(), 3);

    assert!(sel.go_back());
    assert_eq!(sel.pane(), &SettingsPane::Menu);
    assert!(sel.can_go_forward());

    assert!(sel.go_back());
    assert_eq!(sel.pane(), &SettingsPane::MenuBar);

    assert!(sel.go_forward());
    assert_eq!(sel.pane(), &SettingsPane::Menu);

    // Selecting a new pane clears the forward stack.
    sel.select(SettingsPane::Debug);
    assert!(!sel.can_go_forward());
    assert_eq!(sel.pane(), &SettingsPane::Debug);

    // Turning off debug menu resets selection to General and strips Debug from history.
    sel.ensure_valid_selection(false);
    assert_eq!(sel.pane(), &SettingsPane::General);
    assert!(!sel.back_stack().contains(&SettingsPane::Debug));
}

#[test]
fn remove_from_history_collapses_adjacent_duplicates_and_trailing_current() {
    let mut sel = SettingsSelection::new(SettingsPane::General);
    sel.select(SettingsPane::Provider("codex".to_string()));
    sel.select(SettingsPane::MenuBar);
    sel.select(SettingsPane::Provider("codex".to_string()));
    sel.select(SettingsPane::General);

    sel.remove_from_history(|p| *p == SettingsPane::Provider("codex".to_string()));
    assert_eq!(
        sel.back_stack(),
        &[SettingsPane::General, SettingsPane::MenuBar]
    );
}

#[test]
fn sidebar_width_clamps_to_200_and_380() {
    assert_eq!(clamp_sidebar_width(120.0), SIDEBAR_MIN_WIDTH);
    assert_eq!(clamp_sidebar_width(260.0), SIDEBAR_DEFAULT_WIDTH);
    assert_eq!(clamp_sidebar_width(500.0), SIDEBAR_MAX_WIDTH);
}

#[test]
fn sidebar_model_filters_sorts_and_computes_accessibility_labels() {
    let providers = vec![
        SidebarProviderInput {
            id: "codex".to_string(),
            display_name: "Codex".to_string(),
            enabled: true,
            refreshing: false,
            status_indicator: Some(StatusIndicator::None),
        },
        SidebarProviderInput {
            id: "antigravity".to_string(),
            display_name: "Antigravity".to_string(),
            enabled: true,
            refreshing: false,
            status_indicator: Some(StatusIndicator::Minor),
        },
        SidebarProviderInput {
            id: "cursor".to_string(),
            display_name: "Cursor".to_string(),
            enabled: false,
            refreshing: false,
            status_indicator: Some(StatusIndicator::None),
        },
    ];

    let mut filter = SidebarFilterState::default();
    let model = SidebarModel::build(&filter, &providers, true);
    assert_eq!(model.enabled_provider_count, 2);
    assert_eq!(model.providers_header_count_text, "2 on");
    assert_eq!(model.providers.len(), 3);
    assert_eq!(
        model.providers[0].status_dot,
        Some(ProviderStatusDotColor::Green)
    );
    assert_eq!(
        model.providers[1].status_dot,
        Some(ProviderStatusDotColor::Yellow)
    );
    assert_eq!(model.providers[2].status_dot, None);
    assert_eq!(
        model.providers[0].accessibility_label,
        "Codex — Provider service status: Operational"
    );
    assert_eq!(model.providers[2].accessibility_label, "Cursor — Disabled");
    assert_eq!(
        provider_accessibility_label("Codex", true, false, Some(StatusIndicator::None)),
        "Codex"
    );

    // Alphabetical sort puts enabled providers first in A-Z order.
    filter.sorted_alphabetically = true;
    let sorted = SidebarModel::build(&filter, &providers, true);
    assert_eq!(sorted.providers[0].id, "antigravity");
    assert_eq!(sorted.providers[1].id, "codex");
    assert_eq!(sorted.providers[2].id, "cursor");

    // Enabled-only hides disabled providers.
    filter.enabled_only = true;
    let enabled_only = SidebarModel::build(&filter, &providers, true);
    assert_eq!(enabled_only.providers.len(), 2);

    // Search query matching nothing shows empty filter state.
    filter.search_text = "nonexistent".to_string();
    let empty = SidebarModel::build(&filter, &providers, true);
    assert!(empty.shows_empty_filter_state);
    assert!(!filter.can_reorder_providers());

    filter.clear_filters();
    assert!(filter.search_text.is_empty());
    assert!(!filter.enabled_only);
}

#[test]
fn dock_activation_policy_promotes_for_visible_or_miniaturized_settings_window() {
    let status_bar_win = DockWindowDescriptor {
        identifier: None,
        frame_autosave_name: String::new(),
        title: String::new(),
        width: 24.0,
        height: 22.0,
        is_visible: true,
        is_miniaturized: false,
        can_become_key: false,
        is_known_settings_window: false,
        is_status_bar_window: true,
    };
    assert!(!should_use_regular_activation_policy(&[status_bar_win]));

    let visible_settings = DockWindowDescriptor {
        identifier: Some(WINDOW_IDENTIFIER.to_string()),
        frame_autosave_name: WINDOW_AUTOSAVE_NAME.to_string(),
        title: "Settings".to_string(),
        width: WINDOW_WIDTH,
        height: WINDOW_HEIGHT,
        is_visible: true,
        is_miniaturized: false,
        can_become_key: true,
        is_known_settings_window: true,
        is_status_bar_window: false,
    };
    assert!(should_promote_for_presented_window(&visible_settings));
    assert!(should_use_regular_activation_policy(std::slice::from_ref(
        &visible_settings
    )));

    let miniaturized_settings = DockWindowDescriptor {
        is_visible: false,
        is_miniaturized: true,
        ..visible_settings.clone()
    };
    assert!(should_use_regular_activation_policy(&[
        miniaturized_settings
    ]));

    let closed_settings = DockWindowDescriptor {
        is_visible: false,
        is_miniaturized: false,
        ..visible_settings
    };
    assert!(!should_use_regular_activation_policy(&[closed_settings]));
}

#[test]
fn settings_state_applies_fixture_descriptors_and_never_stores_secret_values() {
    let page = sample_app_settings();
    let mut state = SettingsState::default();
    state.apply_page(&page);

    assert!(state.has_descriptor("refreshInterval"));
    assert!(state.has_descriptor("menuBar.iconStyle"));
    assert!(state.has_descriptor("menuBar.showUsed"));
    assert!(state.has_descriptor("menuBar.mergeIcons"));
    assert!(state.has_descriptor("menu.showOptionalCreditsAndExtraUsage"));
    assert_eq!(state.refresh_interval_label(), "15 min");
    assert_eq!(refresh_interval_label(0.0), "Manual");
    assert_eq!(refresh_interval_label(300.0), "5 min");

    // Secret descriptor only exposes `is_set` and `source` metadata.
    let claude_secret = state
        .secret_statuses
        .get("providers.claude.apiKey")
        .expect("secret status metadata");
    assert!(claude_secret.is_set);
    assert_eq!(claude_secret.source.as_deref(), Some("keychain"));

    // Optimistic local update works for backed settings.
    state.set_local("menuBar.showUsed", serde_json::Value::Bool(true));
    assert!(state.menu_bar_show_used);
}

#[test]
fn pane_models_expose_ariadusage_identity_and_keep_unbacked_controls_disabled() {
    let page = sample_app_settings();
    let mut state = SettingsState::default();
    state.apply_page(&page);

    let general = pane_model(&SettingsPane::General, &state, None);
    assert_eq!(general.sections.len(), 4);
    assert_eq!(
        general
            .general_footer
            .as_ref()
            .map(|f| f.quit_title.as_str()),
        Some("Quit AriadUsage")
    );

    let menu_bar = pane_model(&SettingsPane::MenuBar, &state, None);
    assert_eq!(menu_bar.sections.len(), 4);

    let menu = pane_model(&SettingsPane::Menu, &state, None);
    assert_eq!(menu.sections.len(), 5);

    let about = pane_model(&SettingsPane::About, &state, None);
    let hero = about.about_hero.expect("About hero");
    assert_eq!(hero.app_name, "AriadUsage");
    assert_eq!(hero.version_text, format!("Version {APP_VERSION}"));
    assert!(hero.homepage_url.contains("bavanchun/ariadusage"));
    assert!(!hero.homepage_url.contains("steipete"));
    assert_eq!(about.centered_footer.as_deref(), Some(ABOUT_LICENSE_FOOTER));
    assert_eq!(ABOUT_LICENSE_FOOTER, "© 2026 VChun. MIT License.");
    assert!(!MISSING_ENGINE_INTENTS.is_empty());

    // Low 3: About -> Updates StatusNote points to GitHub Releases for macOS app and keeps CLI line for CLI only.
    let updates_note = &about.sections[0].rows[2];
    match &updates_note.control {
        FormControl::StatusNote { lines, command } => {
            assert!(lines[0].contains("GitHub Releases"));
            assert!(lines[0].contains("CLI only"));
            assert!(
                command
                    .as_deref()
                    .is_some_and(|c| c.contains("ariadusage-cli"))
            );
        }
        other => panic!("expected StatusNote on About -> Updates row 2, got {other:?}"),
    }

    // Low 2: Dependent option enablement rules match upstream PreferencesMenuBarPane / PreferencesMenuPane.
    assert!(!inactive_display_contrast_available("bars"));
    assert!(inactive_display_contrast_available("iconAndPercent"));
    assert!(!icon_and_percent_dependents_available("bars"));
    assert!(icon_and_percent_dependents_available("iconAndPercent"));
    assert!(surprise_me_available("bars"));
    assert!(!surprise_me_available("iconAndPercent"));
    assert!(combined_icon_dependents_available(true));
    assert!(!combined_icon_dependents_available(false));
    assert!(!workday_tick_appearance_available(None));
    assert!(workday_tick_appearance_available(Some(5)));
    assert!(!agent_sessions_dependents_available(false));
    assert!(agent_sessions_dependents_available(true));

    // With default style = "bars" and merge_icons = true:
    assert!(!menu_bar.sections[0].rows[1].row_enabled); // Improve visibility on inactive displays
    assert!(!menu_bar.sections[0].rows[2].row_enabled); // Color Pace Indicator
    assert!(!menu_bar.sections[0].rows[3].row_enabled); // Pace sign
    assert!(!menu_bar.sections[1].rows[0].row_enabled); // Layout editor
    assert!(menu_bar.sections[2].rows[1].row_enabled); // Combined icon style
    assert!(menu_bar.sections[3].rows[0].row_enabled); // Surprise me

    assert!(!menu.sections[0].rows[4].row_enabled); // Workday tick appearance
    assert!(menu.sections[1].rows[3].row_enabled); // Overview layout
    assert!(!menu.sections[4].rows[2].row_enabled); // Session labels
    assert!(!menu.sections[4].rows[3].row_enabled); // Remote sessions
    assert!(!menu.sections[4].rows[4].row_enabled); // Hide unreachable hosts
    assert!(!menu.sections[4].rows[5].row_enabled); // Additional SSH hosts

    // When merge_icons is false and style is "iconAndPercent":
    let mut icon_pct_state = state.clone();
    icon_pct_state.menu_bar_icon_style = "iconAndPercent".to_string();
    icon_pct_state.menu_bar_merge_icons = false;
    let mb_icon_pct = pane_model(&SettingsPane::MenuBar, &icon_pct_state, None);
    assert!(mb_icon_pct.sections[0].rows[1].row_enabled);
    assert!(mb_icon_pct.sections[0].rows[2].row_enabled);
    assert!(mb_icon_pct.sections[0].rows[3].row_enabled);
    assert!(mb_icon_pct.sections[1].rows[0].row_enabled);
    assert!(!mb_icon_pct.sections[2].rows[1].row_enabled);
    assert!(!mb_icon_pct.sections[3].rows[0].row_enabled);

    let menu_unmerged = pane_model(&SettingsPane::Menu, &icon_pct_state, None);
    assert!(!menu_unmerged.sections[1].rows[3].row_enabled); // Overview layout dimmed when !merge_icons

    // Only Debug remains staged as empty; all 10 standard app panes and Provider detail panes are populated.
    assert!(SettingsPane::Debug.is_staged_empty_pane());
    assert!(
        pane_model(&SettingsPane::Debug, &state, None)
            .sections
            .is_empty()
    );
    for populated in [
        SettingsPane::ICloudSync,
        SettingsPane::UsageSpend,
        SettingsPane::Notifications,
        SettingsPane::Advanced,
        SettingsPane::Hooks,
        SettingsPane::Plugins,
        SettingsPane::Provider("codex".to_string()),
        SettingsPane::Provider("claude".to_string()),
        SettingsPane::Provider("cursor".to_string()),
        SettingsPane::Provider("antigravity".to_string()),
    ] {
        assert!(!populated.is_staged_empty_pane());
        let model = pane_model(&populated, &state, None);
        assert!(
            !model.sections.is_empty(),
            "expected populated sections for {populated:?}"
        );
    }
}

#[test]
fn remaining_app_panes_and_provider_detail_respect_secret_and_header_contracts() {
    let page = sample_app_settings();
    let mut state = SettingsState::default();
    state.apply_page(&page);

    // Also apply a provider settings page for Codex with enabled, sourceMode, and apiKey secret.
    let codex_page = SettingsPage::new(
        SettingsScope::Provider(ariadusage_protocol::ids::ProviderId::new("codex").unwrap()),
        vec![SettingsSection::new(
            "codex",
            "Codex",
            vec![
                SettingDescriptor::new(
                    SettingId::new("providers.codex.enabled").unwrap(),
                    "Enabled",
                    DescriptorKind::Toggle { value: true },
                ),
                SettingDescriptor::new(
                    SettingId::new("providers.codex.sourceMode").unwrap(),
                    "Source mode",
                    DescriptorKind::Choice {
                        selected: "auto".to_string(),
                        options: vec![
                            ChoiceOption::new("auto", "Auto"),
                            ChoiceOption::new("web", "Web"),
                            ChoiceOption::new("cli", "CLI"),
                            ChoiceOption::new("oauth", "OAuth"),
                            ChoiceOption::new("api", "API"),
                        ],
                    },
                ),
                SettingDescriptor::new(
                    SettingId::new("providers.codex.apiKey").unwrap(),
                    "API key",
                    DescriptorKind::Secret {
                        is_set: false,
                        source: None,
                    },
                ),
            ],
        )],
    );
    state.apply_page(&codex_page);

    // Header title contract: UsageSpend and Provider(_) hide the detail header title.
    assert!(!SettingsPane::UsageSpend.shows_detail_header_title());
    assert!(!SettingsPane::Provider("codex".to_string()).shows_detail_header_title());
    assert!(SettingsPane::ICloudSync.shows_detail_header_title());
    assert!(SettingsPane::Notifications.shows_detail_header_title());
    assert!(SettingsPane::Advanced.shows_detail_header_title());
    assert!(SettingsPane::Hooks.shows_detail_header_title());
    assert!(SettingsPane::Plugins.shows_detail_header_title());

    // Secret terminal command helper contract:
    let claude_api_key_id = SettingId::new("providers.claude.apiKey").unwrap();
    assert_eq!(
        secret_set_command_args(&claude_api_key_id),
        [
            "secret".to_string(),
            "set".to_string(),
            "providers.claude.apiKey".to_string(),
        ]
    );
    assert_eq!(
        secret_set_terminal_command(&claude_api_key_id),
        "ariadusage secret set providers.claude.apiKey"
    );

    // Claude provider detail shows "Set (keychain)" for `providers.claude.apiKey` (never the secret value)
    // and offers "Set in Terminal" button action.
    let claude = pane_model(&SettingsPane::Provider("claude".to_string()), &state, None);
    assert!(!claude.shows_header_title);
    let claude_creds = claude
        .sections
        .iter()
        .find(|s| s.title.as_deref() == Some("Credentials & Accounts"))
        .expect("Claude Credentials & Accounts section");
    match &claude_creds.rows[0].control {
        FormControl::SecretStatus {
            is_set,
            source,
            setting_id,
        } => {
            assert!(*is_set);
            assert_eq!(source.as_deref(), Some("keychain"));
            assert_eq!(setting_id.as_str(), "providers.claude.apiKey");
        }
        other => panic!("expected SecretStatus on Claude API key row, got {other:?}"),
    }

    // Codex provider detail has enabled ProviderHeader, sourceMode BorderedChoice, and HitTest support.
    let codex_pane = SettingsPane::Provider("codex".to_string());
    let codex_model = pane_model(&codex_pane, &state, None);
    let selection = SettingsSelection::new(codex_pane);
    let filter = SidebarFilterState::default();
    let sidebar_model = SidebarModel::from_snapshot(&filter, None, true);
    // Sidebar includes Cursor as disabled alongside Codex, Claude, Antigravity.
    assert_eq!(sidebar_model.providers.len(), 4);
    assert_eq!(sidebar_model.enabled_provider_count, 3);
    assert_eq!(
        sidebar_model
            .providers
            .iter()
            .find(|p| p.id == "cursor")
            .unwrap()
            .accessibility_label,
        "Cursor — Disabled"
    );

    let codex_layout = layout_settings_window(
        Size::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        SIDEBAR_DEFAULT_WIDTH,
        0.0,
        &selection,
        &filter,
        &sidebar_model,
        &codex_model,
    );
    let header_row = &codex_layout.sections[0].rows[0];
    match &header_row.control {
        ariadusage_macos::settings_layout::FormControlLayout::ProviderHeader {
            refresh_button,
            toggle_rect,
            ..
        } => {
            assert_eq!(
                hit_test(
                    &codex_layout,
                    (refresh_button.rect.mid_x(), refresh_button.rect.mid_y())
                ),
                Some(SettingsHitTarget::ButtonAction(
                    FormButtonAction::RefreshProvider(Some("codex".to_string()))
                ))
            );
            assert_eq!(
                hit_test(&codex_layout, (toggle_rect.mid_x(), toggle_rect.mid_y())),
                Some(SettingsHitTarget::ToggleSetting {
                    setting_id: SettingId::new("providers.codex.enabled").unwrap(),
                    new_value: false,
                })
            );
        }
        other => panic!("expected ProviderHeader on row 0, got {other:?}"),
    }

    // Unsupported Cursor provider detail has disabled controls.
    let cursor_model = pane_model(&SettingsPane::Provider("cursor".to_string()), &state, None);
    match &cursor_model.sections[0].rows[0].control {
        FormControl::ProviderHeader {
            enabled,
            enable_toggle_supported,
            enable_setting_id,
            ..
        } => {
            assert!(!*enabled);
            assert!(!*enable_toggle_supported);
            assert!(enable_setting_id.is_none());
        }
        other => panic!("expected ProviderHeader on Cursor row 0, got {other:?}"),
    }
}

#[test]
fn layout_and_hit_testing_cover_sidebar_navigation_and_controls() {
    let page = sample_app_settings();
    let mut state = SettingsState::default();
    state.apply_page(&page);

    let mut selection = SettingsSelection::new(SettingsPane::General);
    selection.select(SettingsPane::Menu);
    let filter = SidebarFilterState::default();
    let sidebar_model = SidebarModel::from_snapshot(&filter, None, true);
    let p_model = pane_model(selection.pane(), &state, None);

    let layout = layout_settings_window(
        Size::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        SIDEBAR_DEFAULT_WIDTH,
        0.0,
        &selection,
        &filter,
        &sidebar_model,
        &p_model,
    );

    // Resize handle at sidebar boundary
    assert_eq!(
        hit_test(&layout, (SIDEBAR_DEFAULT_WIDTH, 200.0)),
        Some(SettingsHitTarget::SidebarResizeHandle)
    );

    // Clicking Back button in detail header
    let back_rect = layout.detail_header.back_button_rect;
    assert_eq!(
        hit_test(&layout, (back_rect.mid_x(), back_rect.mid_y())),
        Some(SettingsHitTarget::NavBack)
    );

    // Clicking About row in sidebar
    let about_row = layout
        .sidebar
        .app_rows
        .iter()
        .find(|r| r.pane == SettingsPane::About)
        .unwrap();
    assert_eq!(
        hit_test(
            &layout,
            (about_row.row_rect.mid_x(), about_row.row_rect.mid_y())
        ),
        Some(SettingsHitTarget::SidebarPane(SettingsPane::About))
    );

    // Enabled Choice popup ("Usage bars fill" in Menu -> Usage row 0)
    let first_row = &layout.sections[0].rows[0];
    if let ariadusage_macos::settings_layout::FormControlLayout::Choice { rect, .. } =
        &first_row.control
    {
        match hit_test(&layout, (rect.mid_x(), rect.mid_y())) {
            Some(SettingsHitTarget::ChoicePopup { setting_id, .. }) => {
                assert_eq!(setting_id.as_str(), "menuBar.showUsed");
            }
            other => panic!("expected ChoicePopup, got {other:?}"),
        }
    } else {
        panic!("expected Choice control on first row of Menu pane");
    }

    // Disabled Toggle ("Show quota warning markers" in Menu -> Usage row 1) does not fire hit target
    let second_row = &layout.sections[0].rows[1];
    if let ariadusage_macos::settings_layout::FormControlLayout::Toggle { rect, .. } =
        &second_row.control
    {
        assert_eq!(hit_test(&layout, (rect.mid_x(), rect.mid_y())), None);
    }

    // General pane Quit button hit target
    let general_selection = SettingsSelection::new(SettingsPane::General);
    let general_model = pane_model(&SettingsPane::General, &state, None);
    let general_layout = layout_settings_window(
        Size::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        SIDEBAR_DEFAULT_WIDTH,
        9999.0,
        &general_selection,
        &filter,
        &sidebar_model,
        &general_model,
    );
    let quit_rect = general_layout
        .general_footer
        .as_ref()
        .unwrap()
        .quit_button
        .rect;
    assert_eq!(
        hit_test(&general_layout, (quit_rect.mid_x(), quit_rect.mid_y())),
        Some(SettingsHitTarget::ButtonAction(FormButtonAction::QuitApp))
    );

    // About pane external link hit target
    let about_selection = SettingsSelection::new(SettingsPane::About);
    let about_model = pane_model(&SettingsPane::About, &state, None);
    let about_layout = layout_settings_window(
        Size::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        SIDEBAR_DEFAULT_WIDTH,
        0.0,
        &about_selection,
        &filter,
        &sidebar_model,
        &about_model,
    );
    let github_row = &about_layout.sections[1].rows[0];
    assert!(matches!(
        &github_row.control,
        ariadusage_macos::settings_layout::FormControlLayout::ExternalLink { .. }
    ));
    assert_eq!(
        hit_test(
            &about_layout,
            (github_row.rect.mid_x(), github_row.rect.mid_y())
        ),
        Some(SettingsHitTarget::ExternalUrl(
            "https://github.com/bavanchun/ariadusage".to_string()
        ))
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn engine_client_fetches_and_updates_settings_over_ipc() {
    let dir = tempfile::tempdir().unwrap();
    let socket_path = dir.path().join("engine.sock");
    let mut server = start_fixture_server(FixtureConfig {
        socket_path: socket_path.clone(),
        ..FixtureConfig::default()
    })
    .await
    .unwrap();

    let (tx, rx) = mpsc::channel();
    let client = EngineClient::spawn(socket_path, move |event| {
        let _ = tx.send(event);
    })
    .unwrap();

    // Wait for initial SettingsPage from connect
    let rx = tokio::task::spawn_blocking(move || {
        loop {
            if let EngineEvent::Settings(page) = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("initial settings page")
            {
                let mut state = SettingsState::default();
                state.apply_page(&page);
                assert!(!state.menu_bar_show_used);
                break;
            }
        }
        rx
    })
    .await
    .unwrap();

    // Mutate `menuBar.showUsed` to `true` over IPC
    client.set_setting(
        SettingId::new("menuBar.showUsed").unwrap(),
        serde_json::Value::Bool(true),
    );

    let updated = tokio::task::spawn_blocking(move || {
        loop {
            if let EngineEvent::Settings(page) = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("updated settings page")
            {
                let mut state = SettingsState::default();
                state.apply_page(&page);
                if state.menu_bar_show_used {
                    return true;
                }
            }
        }
    })
    .await
    .unwrap();

    assert!(updated);
    server.stop().await;
}
