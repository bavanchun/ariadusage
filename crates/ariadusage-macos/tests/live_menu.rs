// Ported from CodexBar Tests/CodexBarTests/MenuCardRefreshTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/MenuOpenRefreshPlanTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/MenuSessionCoordinatorTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatusMenuSwitcherRefreshTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatusMenuSwitcherTrackingTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatusMenuSwitcherWarmupTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Live-menu integration tests and ported `StatusMenuSwitcher*Tests`,
//! `MenuSessionCoordinatorTests`, `MenuOpenRefreshPlanTests`, and `MenuCardRefreshTests`
//! suites for `ariadusage-macos`.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    macos::main();
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use ariadusage_macos::appkit::{
        CardView, FoundationDateTexts, MenuController, png_bytes, set_loading,
        set_provider_loading, set_snapshot,
    };
    use ariadusage_macos::card::{self, CardPreferences, SubtitleStyle};
    use ariadusage_macos::engine_client::{EngineClient, EngineEvent};
    use ariadusage_macos::menu_descriptor::{
        self, MenuAction, MenuEntry, PrimaryContent, SubmenuEntry,
    };
    use ariadusage_macos::menu_session::{
        CachedMergedSwitcherMenuContent, ClosedPreparationPlan, ManualRefreshScope,
        MenuCardRefreshMonitor, MenuOpenRefreshInputs, MenuOpenRefreshPlan,
        MenuRebuildRequestRegistry, MenuRowShape, MenuSessionCoordinator,
        MergedSwitcherContentCache, RefreshScheduling, TrackingRunLoopGate,
    };
    use ariadusage_macos::switcher::SwitcherSelection;
    use ariadusage_protocol::fixture;
    use ariadusage_protocol::ipc::{ClientMessage, PROTOCOL_V1, ServerMessage};
    use ariadusage_protocol::metric::{Confidence, Metric, MetricState};
    use ariadusage_protocol::settings::SettingsPage;
    use ariadusage_protocol::snapshot::EngineSnapshot;
    use ariadusage_protocol::usage::{
        ProviderError, ProviderErrorCategory, ProviderErrorKind, ProviderStatus, RateWindow,
        StatusIndicator,
    };
    use jiff::Timestamp;
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSMenu, NSMenuItem,
    };
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSRunLoop, NSSize, NSString};

    type TestFn = fn(MainThreadMarker);

    const TESTS: &[(&str, TestFn)] = &[
        // Upstream StatusMenuSwitcherRefreshTests.swift (15 declarations)
        (
            "native_switcher_action_preserves_off_tab_switches_after_button_state_toggles",
            native_switcher_action_preserves_off_tab_switches_after_button_state_toggles,
        ),
        (
            "native_switcher_action_restores_active_tab_after_native_toggle",
            native_switcher_action_restores_active_tab_after_native_toggle,
        ),
        (
            "merged_provider_switch_rebuilds_stale_width_switcher_rows",
            merged_provider_switch_rebuilds_stale_width_switcher_rows,
        ),
        (
            "selected_provider_tab_click_does_not_rebuild_open_menu",
            selected_provider_tab_click_does_not_rebuild_open_menu,
        ),
        (
            "merged_provider_switch_updates_live_tab_rows_in_place",
            merged_provider_switch_updates_live_tab_rows_in_place,
        ),
        (
            "smart_provider_switch_resizes_persistent_refresh_row_to_rendered_menu_width",
            smart_provider_switch_resizes_persistent_refresh_row_to_rendered_menu_width,
        ),
        (
            "manual_refresh_keeps_codex_quota_visible_after_switching_away_and_back",
            manual_refresh_keeps_codex_quota_visible_after_switching_away_and_back,
        ),
        (
            "completed_refresh_re_enables_cached_item_when_switching_back",
            completed_refresh_re_enables_cached_item_when_switching_back,
        ),
        (
            "full_cached_reattachment_resynchronizes_detached_refresh_item",
            full_cached_reattachment_resynchronizes_detached_refresh_item,
        ),
        (
            "a_provider_manual_refresh_only_greys_its_own_tab",
            a_provider_manual_refresh_only_greys_its_own_tab,
        ),
        (
            "native_image_menu_rows_are_replaced_during_reconciliation",
            native_image_menu_rows_are_replaced_during_reconciliation,
        ),
        (
            "native_image_submenu_rows_reconcile_in_place",
            native_image_submenu_rows_reconcile_in_place,
        ),
        (
            "provider_switch_does_not_cache_stale_rows_after_required_invalidation",
            provider_switch_does_not_cache_stale_rows_after_required_invalidation,
        ),
        (
            "tab_switch_does_not_replace_quota_indicator_constraints",
            tab_switch_does_not_replace_quota_indicator_constraints,
        ),
        (
            "quota_indicator_constraints_are_replaced_when_ratio_changes",
            quota_indicator_constraints_are_replaced_when_ratio_changes,
        ),
        // Upstream StatusMenuSwitcherTrackingTests.swift (4 declarations)
        (
            "switcher_rebuild_scheduler_runs_during_menu_tracking_exactly_once",
            switcher_rebuild_scheduler_runs_during_menu_tracking_exactly_once,
        ),
        (
            "pointer_switch_defers_structural_menu_rebuild_until_mouse_up",
            pointer_switch_defers_structural_menu_rebuild_until_mouse_up,
        ),
        (
            "pointer_switch_cancels_when_mouse_up_leaves_pressed_segment",
            pointer_switch_cancels_when_mouse_up_leaves_pressed_segment,
        ),
        (
            "unrelated_mouse_up_remains_available_to_normal_menu_items",
            unrelated_mouse_up_remains_available_to_normal_menu_items,
        ),
        // Upstream StatusMenuSwitcherWarmupTests.swift (4 declarations)
        (
            "test_warmup_caches_sibling_selections",
            test_warmup_caches_sibling_selections,
        ),
        (
            "test_warmup_does_not_add_flexible_provider_padding",
            test_warmup_does_not_add_flexible_provider_padding,
        ),
        (
            "test_warmup_skips_selections_already_cached",
            test_warmup_skips_selections_already_cached,
        ),
        (
            "test_warmed_status_submenus_remain_scoped_across_switches_and_reopen",
            test_warmed_status_submenus_remain_scoped_across_switches_and_reopen,
        ),
        // Upstream MenuSessionCoordinatorTests.swift (10 declarations: 7 + 3)
        (
            "invalidation_records_data_structural_and_required_generations_independently",
            invalidation_records_data_structural_and_required_generations_independently,
        ),
        (
            "closed_preparation_distinguishes_no_work_deferred_work_and_required_work",
            closed_preparation_distinguishes_no_work_deferred_work_and_required_work,
        ),
        (
            "stale_content_survives_only_a_data_generation_after_latest_structural_render",
            stale_content_survives_only_a_data_generation_after_latest_structural_render,
        ),
        (
            "removing_menu_clears_all_menu_scoped_lifecycle_state",
            removing_menu_clears_all_menu_scoped_lifecycle_state,
        ),
        (
            "reopening_a_persistent_menu_replaces_its_tracking_session_token",
            reopening_a_persistent_menu_replaces_its_tracking_session_token,
        ),
        (
            "menu_interaction_token_advances_within_one_tracking_session",
            menu_interaction_token_advances_within_one_tracking_session,
        ),
        (
            "replacement_viewport_restore_token_rejects_stale_completion",
            replacement_viewport_restore_token_rejects_stale_completion,
        ),
        (
            "replacement_request_invalidates_prior_token_without_affecting_other_menus",
            replacement_request_invalidates_prior_token_without_affecting_other_menus,
        ),
        (
            "stale_completion_cannot_clear_replacement_request",
            stale_completion_cannot_clear_replacement_request,
        ),
        (
            "cancelling_all_requests_keeps_future_tokens_distinct",
            cancelling_all_requests_keeps_future_tokens_distinct,
        ),
        // Upstream MenuOpenRefreshPlanTests.swift (5 declarations)
        (
            "refresh_all_selects_every_enabled_provider_concurrently",
            refresh_all_selects_every_enabled_provider_concurrently,
        ),
        (
            "refresh_all_skips_dashboard_refresh_when_codex_is_disabled",
            refresh_all_skips_dashboard_refresh_when_codex_is_disabled,
        ),
        (
            "refresh_all_skips_token_cost_refresh_when_no_providers_are_enabled",
            refresh_all_skips_token_cost_refresh_when_no_providers_are_enabled,
        ),
        (
            "ordinary_refresh_selects_only_visible_enabled_retries_sequentially",
            ordinary_refresh_selects_only_visible_enabled_retries_sequentially,
        ),
        (
            "ordinary_refresh_skips_fresh_providers",
            ordinary_refresh_skips_fresh_providers,
        ),
        // Upstream MenuCardRefreshTests.swift (1 declaration)
        (
            "background_refresh_keeps_quota_timing_current",
            background_refresh_keeps_quota_timing_current,
        ),
        // Additional populate_menu, reconciliation, and live-IPC integration tests
        (
            "test_populate_menu_does_not_overwrite_refreshing_override_subtitle",
            test_populate_menu_does_not_overwrite_refreshing_override_subtitle,
        ),
        (
            "test_populate_menu_keeps_selected_provider_refresh_override_when_cached_usage_was_error",
            test_populate_menu_keeps_selected_provider_refresh_override_when_cached_usage_was_error,
        ),
        (
            "test_make_menu_card_model_freezes_selected_provider_content_while_manual_refresh_is_in_flight",
            test_make_menu_card_model_freezes_selected_provider_content_while_manual_refresh_is_in_flight,
        ),
        (
            "test_populate_menu_rebuilds_refresh_subtitle_when_selected_provider_starts_refreshing",
            test_populate_menu_rebuilds_refresh_subtitle_when_selected_provider_starts_refreshing,
        ),
        (
            "test_populate_menu_keeps_refresh_row_interactive_for_other_provider_while_single_provider_refresh_is_in_flight",
            test_populate_menu_keeps_refresh_row_interactive_for_other_provider_while_single_provider_refresh_is_in_flight,
        ),
        (
            "test_populate_menu_does_not_detach_hovered_custom_view_during_provider_switch",
            test_populate_menu_does_not_detach_hovered_custom_view_during_provider_switch,
        ),
        (
            "test_populate_menu_keeps_all_providers_switcher_selected_for_overview_tab",
            test_populate_menu_keeps_all_providers_switcher_selected_for_overview_tab,
        ),
        (
            "test_populate_menu_overview_renders_compact_provider_rows_with_metric_subtitles",
            test_populate_menu_overview_renders_compact_provider_rows_with_metric_subtitles,
        ),
        (
            "test_populate_menu_overview_spend_summary_prepends_card_and_share_action",
            test_populate_menu_overview_spend_summary_prepends_card_and_share_action,
        ),
        (
            "test_populate_menu_overview_spend_summary_omits_card_when_total_is_zero",
            test_populate_menu_overview_spend_summary_omits_card_when_total_is_zero,
        ),
        (
            "test_populate_menu_overview_row_click_switches_to_provider_tab",
            test_populate_menu_overview_row_click_switches_to_provider_tab,
        ),
        (
            "test_populate_menu_overview_attaches_charts_submenu_to_provider_row",
            test_populate_menu_overview_attaches_charts_submenu_to_provider_row,
        ),
        (
            "test_populate_menu_overview_charts_submenu_contains_usage_and_cost_sections",
            test_populate_menu_overview_charts_submenu_contains_usage_and_cost_sections,
        ),
        (
            "test_populate_menu_overview_omits_charts_submenu_when_no_history",
            test_populate_menu_overview_omits_charts_submenu_when_no_history,
        ),
        (
            "test_populate_menu_provider_tab_restores_buy_credits_and_top_level_submenus_after_overview",
            test_populate_menu_provider_tab_restores_buy_credits_and_top_level_submenus_after_overview,
        ),
        (
            "test_populate_menu_open_refresh_preserves_switcher_and_card_views_when_layout_is_unchanged",
            test_populate_menu_open_refresh_preserves_switcher_and_card_views_when_layout_is_unchanged,
        ),
        (
            "test_populate_menu_open_refresh_updates_card_in_place_when_height_stays_compatible",
            test_populate_menu_open_refresh_updates_card_in_place_when_height_stays_compatible,
        ),
        (
            "test_populate_menu_open_refresh_defers_incompatible_card_resize_until_menu_closes",
            test_populate_menu_open_refresh_defers_incompatible_card_resize_until_menu_closes,
        ),
        (
            "test_populate_menu_open_refresh_replaces_action_rows_when_error_creates_recovery_actions",
            test_populate_menu_open_refresh_replaces_action_rows_when_error_creates_recovery_actions,
        ),
        (
            "test_reconcile_menu_content_keeps_unchanged_suffix_items_when_refreshing_section_changes",
            test_reconcile_menu_content_keeps_unchanged_suffix_items_when_refreshing_section_changes,
        ),
        (
            "test_reconcile_menu_content_replaces_image_action_rows_instead_of_mutating_them_in_place",
            test_reconcile_menu_content_replaces_image_action_rows_instead_of_mutating_them_in_place,
        ),
        (
            "test_populate_menu_uses_single_switcher_row_without_fallback_submenu",
            test_populate_menu_uses_single_switcher_row_without_fallback_submenu,
        ),
        (
            "test_open_menu_background_refresh_defers_rebuild_while_submenu_is_open",
            test_open_menu_background_refresh_defers_rebuild_while_submenu_is_open,
        ),
        (
            "test_open_menu_background_refresh_defers_rebuild_while_native_item_is_highlighted",
            test_open_menu_background_refresh_defers_rebuild_while_native_item_is_highlighted,
        ),
        (
            "test_open_menu_rebuild_updates_usage_breakdown_submenu_in_place",
            test_open_menu_rebuild_updates_usage_breakdown_submenu_in_place,
        ),
        (
            "test_open_menu_rebuild_reuses_persistent_refresh_menu_item_instance",
            test_open_menu_rebuild_reuses_persistent_refresh_menu_item_instance,
        ),
        // StatusMenuSwitcherTrackingTests.swift (5 ported tests)
        (
            "test_switcher_button_performs_click_inside_tracking_run_loop_mode",
            test_switcher_button_performs_click_inside_tracking_run_loop_mode,
        ),
        (
            "test_switcher_view_keeps_refresh_paused_across_press_release_and_deferred_selection_apply",
            test_switcher_view_keeps_refresh_paused_across_press_release_and_deferred_selection_apply,
        ),
        (
            "test_switcher_button_restores_selected_state_when_reclicking_current_segment",
            test_switcher_button_restores_selected_state_when_reclicking_current_segment,
        ),
        (
            "test_open_menu_defers_live_rebuild_while_switcher_button_is_pressed",
            test_open_menu_defers_live_rebuild_while_switcher_button_is_pressed,
        ),
        (
            "test_switcher_view_triggers_selection_only_once_when_native_action_and_runtime_click_both_fire",
            test_switcher_view_triggers_selection_only_once_when_native_action_and_runtime_click_both_fire,
        ),
        // Additional warmup regression tests
        (
            "test_warmup_populates_non_selected_switcher_candidates",
            test_warmup_populates_non_selected_switcher_candidates,
        ),
        (
            "test_cached_switcher_content_invalidates_when_menu_content_version_changes",
            test_cached_switcher_content_invalidates_when_menu_content_version_changes,
        ),
        (
            "test_switching_tabs_preserves_prewarmed_sibling_cache_entries",
            test_switching_tabs_preserves_prewarmed_sibling_cache_entries,
        ),
        (
            "test_prewarmed_switcher_content_never_uses_flexible_blank_rows",
            test_prewarmed_switcher_content_never_uses_flexible_blank_rows,
        ),
        // Live IPC, Refresh Intent, Delayed Open-Refresh, and Reconnect tests (4 tests)
        (
            "test_live_ipc_snapshot_push_updates_open_menu_in_place_without_recreating_views",
            test_live_ipc_snapshot_push_updates_open_menu_in_place_without_recreating_views,
        ),
        (
            "test_refresh_action_sends_selected_provider_on_provider_tab_and_none_on_overview",
            test_refresh_action_sends_selected_provider_on_provider_tab_and_none_on_overview,
        ),
        (
            "test_menu_open_schedules_delayed_refresh_and_cancels_on_close",
            test_menu_open_schedules_delayed_refresh_and_cancels_on_close,
        ),
        (
            "test_disconnect_and_reconnect_over_unix_socket_with_status_icon_loading_api",
            test_disconnect_and_reconnect_over_unix_socket_with_status_icon_loading_api,
        ),
    ];

    pub fn main() {
        let mtm = MainThreadMarker::new().expect("live_menu tests must run on the main thread");
        let _app = NSApplication::sharedApplication(mtm);

        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut list_only = false;
        let mut exact = false;
        let mut filters: Vec<String> = Vec::new();
        for arg in args {
            match arg.as_str() {
                "--list" => list_only = true,
                "--exact" => exact = true,
                "--nocapture" | "--show-output" | "--quiet" | "-q" => {}
                a if a.starts_with('-') => {}
                other => filters.push(other.to_string()),
            }
        }

        if list_only {
            for (name, _) in TESTS {
                println!("{name}: test");
            }
            return;
        }

        let selected: Vec<&(&str, TestFn)> = TESTS
            .iter()
            .filter(|(name, _)| {
                if filters.is_empty() {
                    true
                } else if exact {
                    filters.contains(&name.to_string())
                } else {
                    filters.iter().any(|f| name.contains(f))
                }
            })
            .collect();

        println!("running {} tests", selected.len());
        for (name, func) in &selected {
            func(mtm);
            println!("test {name} ... ok");
        }
        println!(
            "\ntest result: ok. {} passed; 0 failed; 0 ignored; 0 measured; {} filtered out\n",
            selected.len(),
            TESTS.len() - selected.len()
        );

        if filters.is_empty()
            && let Some(dir) = std::env::var_os("ARIADUSAGE_WRITE_LIVE_MENU_CAPTURES")
            && !dir.is_empty()
        {
            write_live_menu_captures(mtm, &PathBuf::from(dir));
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn sample_snapshot() -> EngineSnapshot {
        let mut snap = fixture::generate_scenario_snapshot(0);
        // Set `generated_at` 4 minutes after the fixture providers' `updated_at` (`2026-10-06T12:48:00Z`)
        // so deterministic relative timestamps format as `"Updated 4m ago"`.
        snap.generated_at = "2026-10-06T12:52:00Z".parse::<Timestamp>().unwrap();
        snap
    }

    fn dummy_client(socket_dir: &Path) -> EngineClient {
        EngineClient::spawn(socket_dir.join("unused.sock"), |_| {})
            .expect("spawn dummy EngineClient")
    }

    fn make_controller(mtm: MainThreadMarker) -> (tempfile::TempDir, MenuController) {
        let dir = tempfile::tempdir().expect("tempdir");
        let client = dummy_client(dir.path());
        let menu = MenuController::new_with_callbacks(mtm, client, || {}, |_| {}, |_| {});
        (dir, menu)
    }

    fn pump_main_run_loop(duration: Duration) {
        let deadline = Instant::now() + duration;
        let run_loop = NSRunLoop::mainRunLoop();
        while Instant::now() < deadline {
            let until = NSDate::dateWithTimeIntervalSinceNow(0.01);
            // SAFETY: pumping the main run loop in `NSDefaultRunLoopMode` for 10ms slices.
            unsafe {
                let _ = run_loop.runMode_beforeDate(NSDefaultRunLoopMode, &until);
            }
        }
    }

    fn make_scratch_item(
        mtm: MainThreadMarker,
        title: &str,
        symbol: Option<&str>,
    ) -> Retained<NSMenuItem> {
        let item = NSMenuItem::new(mtm);
        item.setTitle(&NSString::from_str(title));
        item.setEnabled(true);
        if let Some(sym) = symbol
            && let Some(img) =
                objc2_app_kit::NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(sym),
                    None,
                )
        {
            item.setImage(Some(&img));
        }
        item
    }

    // -----------------------------------------------------------------------
    // 0. Upstream Exact-Named Test Suites (39 declarations across 6 files)
    // -----------------------------------------------------------------------

    // StatusMenuSwitcherRefreshTests.swift:35
    fn native_switcher_action_preserves_off_tab_switches_after_button_state_toggles(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));

        let selections: Rc<RefCell<Vec<SwitcherSelection>>> = Rc::new(RefCell::new(Vec::new()));
        let selections_cb = selections.clone();
        menu.switcher_view().set_on_select(move |sel| {
            selections_cb.borrow_mut().push(sel);
        });

        // Tag 1 is Claude (Overview=0, Claude=1, Codex=2, Antigravity=3).
        assert!(
            menu.switcher_view()
                .simulate_native_action(1, NSControlStateValueOn)
        );
        assert_eq!(
            *selections.borrow(),
            vec![SwitcherSelection::Provider("claude".to_string())]
        );
    }

    // StatusMenuSwitcherRefreshTests.swift:52
    fn native_switcher_action_restores_active_tab_after_native_toggle(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));

        let selections: Rc<RefCell<Vec<SwitcherSelection>>> = Rc::new(RefCell::new(Vec::new()));
        let selections_cb = selections.clone();
        menu.switcher_view().set_on_select(move |sel| {
            selections_cb.borrow_mut().push(sel);
        });

        // Tag 2 is Codex (already active). Native toggle off must restore `.on` without emitting selection.
        assert!(
            menu.switcher_view()
                .simulate_native_action(2, NSControlStateValueOff)
        );
        assert!(selections.borrow().is_empty());
        assert_eq!(
            menu.switcher_view().button_state(2),
            Some(NSControlStateValueOn)
        );
    }

    // StatusMenuSwitcherRefreshTests.swift:70
    fn merged_provider_switch_rebuilds_stale_width_switcher_rows(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.simulate_will_open();

        let h = menu.switcher_view().frame().size.height;
        menu.switcher_view().setFrameSize(NSSize::new(250.0, h));
        assert_eq!(menu.switcher_view().frame().size.width, 250.0);

        let selected: Rc<RefCell<Option<SwitcherSelection>>> = Rc::new(RefCell::new(None));
        let selected_cb = selected.clone();
        menu.switcher_view().set_on_select(move |sel| {
            *selected_cb.borrow_mut() = Some(sel);
        });

        assert!(menu.switcher_view().simulate_runtime_click(1));
        let next_sel = selected.borrow().clone().expect("selection changed");
        menu.update(Some(&snap), true, Some(&next_sel));

        assert_eq!(menu.switcher_view().frame().size.width, 310.0);
        assert_eq!(
            menu.switcher_view().button_state(1),
            Some(NSControlStateValueOn)
        );
    }

    // StatusMenuSwitcherRefreshTests.swift:137
    fn selected_provider_tab_click_does_not_rebuild_open_menu(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.simulate_will_open();

        let rebuilds_before = menu.body_rebuild_count();
        let in_place_before = menu.in_place_update_count();
        let middle_before = menu.middle_reconcile_count();

        // Tag 2 is the already-selected Codex button.
        assert!(menu.switcher_view().simulate_runtime_click(2));
        assert_eq!(menu.body_rebuild_count(), rebuilds_before);
        assert_eq!(menu.in_place_update_count(), in_place_before);
        assert_eq!(menu.middle_reconcile_count(), middle_before);
        assert_eq!(
            menu.switcher_view().button_state(2),
            Some(NSControlStateValueOn)
        );
    }

    // StatusMenuSwitcherRefreshTests.swift:185
    fn merged_provider_switch_updates_live_tab_rows_in_place(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.simulate_will_open();

        let original_content_ptr = Retained::as_ptr(&menu.menu().itemAtIndex(2).unwrap());

        assert!(menu.switcher_view().simulate_runtime_click(1));
        menu.update(Some(&snap), true, Some(&claude_tab));
        assert_eq!(
            Retained::as_ptr(&menu.menu().itemAtIndex(2).unwrap()),
            original_content_ptr
        );

        assert!(menu.switcher_view().simulate_runtime_click(2));
        menu.update(Some(&snap), true, Some(&codex_tab));
        assert_eq!(
            Retained::as_ptr(&menu.menu().itemAtIndex(2).unwrap()),
            original_content_ptr
        );

        assert!(menu.switcher_view().simulate_runtime_click(1));
        menu.update(Some(&snap), true, Some(&claude_tab));
        assert_eq!(
            Retained::as_ptr(&menu.menu().itemAtIndex(2).unwrap()),
            original_content_ptr
        );

        menu.invalidate_menus(false, true);
        assert_eq!(menu.cached_tab_count(), 0);
    }

    // StatusMenuSwitcherRefreshTests.swift:253
    fn smart_provider_switch_resizes_persistent_refresh_row_to_rendered_menu_width(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.simulate_will_open();
        menu.menu().setMinimumWidth(420.0);

        menu.update(Some(&snap), true, Some(&claude_tab));
        let expected_width = menu.rendered_menu_width();
        assert_eq!(expected_width, 420.0);
        assert!((menu.switcher_view().frame().size.width - expected_width).abs() <= 0.5);
        assert!((menu.card_views()[0].frame().size.width - expected_width).abs() <= 0.5);
    }

    // StatusMenuSwitcherRefreshTests.swift:310
    fn manual_refresh_keeps_codex_quota_visible_after_switching_away_and_back(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let mut snap = sample_snapshot();
        for p in &mut snap.providers {
            let pct = match p.id.as_str() {
                "codex" => 21.0,
                "claude" => 44.0,
                _ => 30.0,
            };
            if let Some(primary) = p.windows.primary.as_mut()
                && let Some(val) = primary.value.as_mut()
            {
                val.used_percent = pct;
            }
        }
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.simulate_will_open();

        menu.start_user_refresh(None, Some(&snap), true, Some(&codex_tab));
        assert!(menu.is_manual_refresh_active());

        // Transiently clear Codex windows while manual refresh is in flight (`store.refreshingProviders.insert(.codex)`).
        let mut snap_empty_codex = snap.clone();
        snap_empty_codex.engine.refreshing = true;
        let codex = snap_empty_codex
            .providers
            .iter_mut()
            .find(|p| p.id.as_str() == "codex")
            .unwrap();
        codex.windows.primary = None;
        codex.windows.secondary = None;
        codex.windows.tertiary = None;
        codex.windows.extra.clear();

        // Switch away to Claude and back to Codex while manual refresh is in flight.
        assert!(menu.switcher_view().simulate_runtime_click(1));
        menu.update(Some(&snap_empty_codex), true, Some(&claude_tab));
        assert!(menu.switcher_view().simulate_runtime_click(2));
        menu.update(Some(&snap_empty_codex), true, Some(&codex_tab));

        let dates = FoundationDateTexts::new();
        let prefs = CardPreferences::default();
        let empty_fallback = card::provider_card(
            snap_empty_codex
                .providers
                .iter()
                .find(|p| p.id.as_str() == "codex")
                .unwrap(),
            &prefs,
            snap_empty_codex.generated_at,
            &dates,
        );
        assert!(empty_fallback.metrics.is_empty());

        let in_flight =
            menu.effective_card_model("codex", empty_fallback.clone(), Some(&snap_empty_codex));
        assert_eq!(in_flight.subtitle.text, "Refreshing…");
        assert_eq!(in_flight.metrics[0].percent_label(), "79% left");
        assert_eq!(
            menu.card_views()[0].model().unwrap().metrics[0].percent_label(),
            "79% left"
        );

        // Completing the refresh clears the frozen card and reveals the empty fallback.
        snap_empty_codex.engine.refreshing = false;
        menu.finish_manual_refresh();
        assert!(!menu.is_manual_refresh_active());
        let completed = menu.effective_card_model("codex", empty_fallback, Some(&snap_empty_codex));
        assert!(completed.metrics.is_empty());
    }

    // StatusMenuSwitcherRefreshTests.swift:413
    fn completed_refresh_re_enables_cached_item_when_switching_back(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.simulate_will_open();

        menu.start_user_refresh(None, Some(&snap), true, Some(&codex_tab));
        assert!(!menu.refresh_item().isEnabled());

        assert!(menu.switcher_view().simulate_runtime_click(1));
        menu.update(Some(&snap), true, Some(&claude_tab));

        menu.finish_manual_refresh();
        assert!(!menu.is_manual_refresh_active());

        assert!(menu.switcher_view().simulate_runtime_click(2));
        menu.update(Some(&snap), true, Some(&codex_tab));
        assert!(menu.refresh_item().isEnabled());
    }

    // StatusMenuSwitcherRefreshTests.swift:479
    fn full_cached_reattachment_resynchronizes_detached_refresh_item(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.simulate_will_open();

        menu.start_user_refresh(None, Some(&snap), true, Some(&codex_tab));
        assert!(!menu.refresh_item().isEnabled());

        menu.menu().removeAllItems();
        // SAFETY: `refresh_item` is a main-thread `NSMenuItem` owned by `menu`.
        assert!(unsafe { menu.refresh_item().menu() }.is_none());

        menu.finish_manual_refresh();
        menu.update(Some(&snap), true, Some(&codex_tab));
        // SAFETY: `refresh_item` is a main-thread `NSMenuItem` owned by `menu`.
        assert!(unsafe { menu.refresh_item().menu() }.is_some());
        assert!(menu.refresh_item().isEnabled());
    }

    // StatusMenuSwitcherRefreshTests.swift:533
    fn a_provider_manual_refresh_only_greys_its_own_tab(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.simulate_will_open();

        // A manual refresh of Claude must leave the Codex tab's Refresh row enabled.
        menu.start_user_refresh(
            Some("claude".to_string()),
            Some(&snap),
            true,
            Some(&codex_tab),
        );
        assert!(menu.refresh_item().isEnabled());

        // Switching to the Claude tab reflects Claude's own in-flight refresh.
        menu.update(Some(&snap), true, Some(&claude_tab));
        assert!(!menu.refresh_item().isEnabled());

        // An all-providers refresh busies every tab regardless of the selected provider.
        menu.start_user_refresh(None, Some(&snap), true, Some(&codex_tab));
        assert!(!menu.refresh_item().isEnabled());
    }

    // StatusMenuSwitcherRefreshTests.swift:578
    fn native_image_menu_rows_are_replaced_during_reconciliation(mtm: MainThreadMarker) {
        let menu = NSMenu::new(mtm);
        let live_item = make_scratch_item(mtm, "Status Page", Some("waveform.path.ecg"));
        menu.addItem(&live_item);
        let shapes = vec![MenuRowShape::LeafAction {
            title: "Status Page".to_string(),
            has_symbol_image: true,
        }];

        let scratch = NSMenu::new(mtm);
        let fresh_item = make_scratch_item(mtm, "Status Page", Some("waveform.path.ecg"));
        scratch.addItem(&fresh_item);

        MenuController::reconcile_menu_from_scratch(&menu, 0, &shapes, &shapes, &scratch);

        assert_eq!(menu.numberOfItems(), 1);
        assert!(std::ptr::eq(&*menu.itemAtIndex(0).unwrap(), &*fresh_item));
        assert!(!std::ptr::eq(&*menu.itemAtIndex(0).unwrap(), &*live_item));
    }

    // StatusMenuSwitcherRefreshTests.swift:608
    fn native_image_submenu_rows_reconcile_in_place(mtm: MainThreadMarker) {
        let menu = NSMenu::new(mtm);
        let live_item = make_scratch_item(mtm, "System Account", Some("person.crop.circle"));
        let live_sub =
            NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("System Account"));
        live_item.setSubmenu(Some(&live_sub));
        menu.addItem(&live_item);
        let shapes = vec![MenuRowShape::Submenu {
            title: "System Account".to_string(),
            has_symbol_image: true,
        }];

        let scratch = NSMenu::new(mtm);
        let fresh_item = make_scratch_item(mtm, "System Account", Some("person.crop.circle"));
        let fresh_sub =
            NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("System Account"));
        fresh_item.setSubmenu(Some(&fresh_sub));
        scratch.addItem(&fresh_item);

        MenuController::reconcile_menu_from_scratch(&menu, 0, &shapes, &shapes, &scratch);

        assert_eq!(menu.numberOfItems(), 1);
        assert!(std::ptr::eq(&*menu.itemAtIndex(0).unwrap(), &*live_item));
        assert!(!std::ptr::eq(&*menu.itemAtIndex(0).unwrap(), &*fresh_item));
    }

    // StatusMenuSwitcherRefreshTests.swift:640
    fn provider_switch_does_not_cache_stale_rows_after_required_invalidation(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.simulate_will_open();

        menu.invalidate_menus(false, true);
        assert_eq!(menu.cached_tab_count(), 0);

        assert!(menu.switcher_view().simulate_runtime_click(1));
        menu.update(Some(&snap), true, Some(&claude_tab));
        assert!(menu.switcher_view().simulate_runtime_click(2));
        menu.update(Some(&snap), true, Some(&codex_tab));

        assert_eq!(menu.rendered_version(), Some(menu.content_version()));
        let req_version = menu.required_rebuild_version();
        for entry in menu.cached_switcher_entries() {
            assert!(entry.required_menu_content_version >= req_version);
        }
    }

    // StatusMenuSwitcherRefreshTests.swift:705
    fn tab_switch_does_not_replace_quota_indicator_constraints(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let mut snap = sample_snapshot();
        snap.providers
            .retain(|p| p.id.as_str() == "codex" || p.id.as_str() == "claude");
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.switcher_view()
            .update_weekly_remaining_ratios(&[None, Some(75.0), Some(75.0)]);

        let initial_constraints = menu
            .switcher_view()
            .quota_indicator_constraint_identifiers();
        assert_eq!(
            initial_constraints.len(),
            2,
            "both providers should have quota indicators"
        );

        menu.switcher_view()
            .update_weekly_remaining_ratios(&[None, Some(75.0), Some(75.0)]);
        let after_first_call = menu
            .switcher_view()
            .quota_indicator_constraint_identifiers();
        assert_eq!(
            after_first_call, initial_constraints,
            "same ratio: constraints must not be replaced"
        );
    }

    // StatusMenuSwitcherRefreshTests.swift:726
    fn quota_indicator_constraints_are_replaced_when_ratio_changes(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let mut snap = sample_snapshot();
        snap.providers
            .retain(|p| p.id.as_str() == "codex" || p.id.as_str() == "claude");
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.switcher_view()
            .update_weekly_remaining_ratios(&[None, Some(75.0), Some(75.0)]);

        let initial_constraints = menu
            .switcher_view()
            .quota_indicator_constraint_identifiers();
        assert_eq!(initial_constraints.len(), 2);

        menu.switcher_view()
            .update_weekly_remaining_ratios(&[None, Some(40.0), Some(40.0)]);
        let after_data_change = menu
            .switcher_view()
            .quota_indicator_constraint_identifiers();
        assert_ne!(
            after_data_change, initial_constraints,
            "changed ratio: constraints should be replaced"
        );
    }

    // StatusMenuSwitcherTrackingTests.swift:10
    fn switcher_rebuild_scheduler_runs_during_menu_tracking_exactly_once(_mtm: MainThreadMarker) {
        let gate = TrackingRunLoopGate::new();
        let mut run_count = 0;
        assert!(gate.run_once(|| {
            run_count += 1;
        }));
        pump_main_run_loop(Duration::from_millis(20));
        assert!(!gate.run_once(|| {
            run_count += 1;
        }));
        assert_eq!(run_count, 1);
    }

    // StatusMenuSwitcherTrackingTests.swift:27
    fn pointer_switch_defers_structural_menu_rebuild_until_mouse_up(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.simulate_will_open();

        let selected: Rc<RefCell<Option<SwitcherSelection>>> =
            Rc::new(RefCell::new(Some(codex_tab.clone())));
        let selected_cb = selected.clone();
        menu.switcher_view().set_on_select(move |sel| {
            *selected_cb.borrow_mut() = Some(sel);
        });

        // Press down on Claude (button 1) while Codex (button 2) is selected.
        menu.switcher_view().begin_pointer_interaction(1);
        assert!(menu.switcher_view().is_pointer_interaction_active());
        assert_eq!(*selected.borrow(), Some(codex_tab));

        // Release on Claude (button 1) -> selection commits and menu updates.
        let committed = menu.switcher_view().end_pointer_interaction(Some(1));
        assert_eq!(
            committed,
            Some(SwitcherSelection::Provider("claude".to_string()))
        );
        assert_eq!(
            *selected.borrow(),
            Some(SwitcherSelection::Provider("claude".to_string()))
        );
        assert!(!menu.switcher_view().is_pointer_interaction_active());
    }

    // StatusMenuSwitcherTrackingTests.swift:101
    fn pointer_switch_cancels_when_mouse_up_leaves_pressed_segment(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.simulate_will_open();

        let selected: Rc<RefCell<Option<SwitcherSelection>>> =
            Rc::new(RefCell::new(Some(codex_tab.clone())));
        let selected_cb = selected.clone();
        menu.switcher_view().set_on_select(move |sel| {
            *selected_cb.borrow_mut() = Some(sel);
        });
        let rebuilds_before = menu.body_rebuild_count();

        // Press down on button 2, release on button 1 -> cancelled.
        menu.switcher_view().begin_pointer_interaction(2);
        let committed = menu.switcher_view().end_pointer_interaction(Some(1));
        assert_eq!(committed, None);
        assert_eq!(*selected.borrow(), Some(codex_tab));
        assert_eq!(menu.body_rebuild_count(), rebuilds_before);
        assert!(!menu.switcher_view().is_pointer_interaction_active());
    }

    // StatusMenuSwitcherTrackingTests.swift:162
    fn unrelated_mouse_up_remains_available_to_normal_menu_items(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));

        assert!(!menu.switcher_view().is_pointer_interaction_active());
        assert_eq!(menu.switcher_view().end_pointer_interaction(Some(1)), None);
    }

    // StatusMenuSwitcherWarmupTests.swift:49
    fn test_warmup_caches_sibling_selections(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.simulate_will_open();
        menu.warm_sibling_content(&snap, true, &codex_tab);

        let entries = menu.cached_switcher_entries();
        let enabled_count = snap.providers.iter().filter(|p| p.enabled).count();
        let cached_provider_count = entries
            .iter()
            .filter(|e| matches!(e.selection, SwitcherSelection::Provider(_)))
            .count();
        assert!(cached_provider_count >= enabled_count);
    }

    // StatusMenuSwitcherWarmupTests.swift:70
    fn test_warmup_does_not_add_flexible_provider_padding(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.simulate_will_open();
        menu.warm_sibling_content(&snap, true, &codex_tab);

        let provider_entries: Vec<_> = menu
            .cached_switcher_entries()
            .into_iter()
            .filter(|e| e.selection != SwitcherSelection::Overview)
            .collect();
        assert!(!provider_entries.is_empty());
        for entry in provider_entries {
            assert!(
                !entry.has_blank_padding_rows,
                "provider tabs must size to their content instead of carrying a flexible blank row"
            );
        }
    }

    // StatusMenuSwitcherWarmupTests.swift:105
    fn test_warmed_status_submenus_remain_scoped_across_switches_and_reopen(mtm: MainThreadMarker) {
        let providers = ["claude", "codex", "antigravity"];
        for initial in providers {
            for target in providers {
                if target == initial {
                    continue;
                }
                let (_dir, menu) = make_controller(mtm);
                let snap = sample_snapshot();
                let init_sel = SwitcherSelection::Provider(initial.to_string());
                let target_sel = SwitcherSelection::Provider(target.to_string());
                menu.update(Some(&snap), true, Some(&init_sel));
                menu.simulate_will_open();
                menu.warm_sibling_content(&snap, true, &init_sel);

                let meta = menu_descriptor::provider_metadata(target);
                let expected_page = meta
                    .status_page_url
                    .or(meta.status_link_url)
                    .expect("expected provider status page URL")
                    .to_string();

                let has_scoped_status_page = |actions: &[MenuEntry]| -> bool {
                    actions.iter().any(|a| match a {
                        MenuEntry::Action {
                            title,
                            action: MenuAction::OpenStatusPage { url },
                            ..
                        } => title == "Status Page" && url == &expected_page,
                        MenuEntry::Submenu { title, items, .. } if title == "Status Page" => {
                            items.iter().any(|item| {
                                matches!(
                                    item,
                                    SubmenuEntry::Action {
                                        action: MenuAction::OpenStatusPage { url },
                                        ..
                                    } if url == &expected_page
                                )
                            })
                        }
                        _ => false,
                    })
                };

                let cached_target = menu
                    .cached_switcher_entries()
                    .into_iter()
                    .find(|e| e.selection == target_sel)
                    .expect("cached target selection");
                assert!(has_scoped_status_page(&cached_target.structure.actions));

                menu.update(Some(&snap), true, Some(&target_sel));
                let live_struct = menu.last_structure().unwrap();
                assert!(has_scoped_status_page(&live_struct.actions));

                menu.simulate_did_close();
                menu.simulate_will_open();
                menu.update(Some(&snap), true, Some(&target_sel));
                let reopened_struct = menu.last_structure().unwrap();
                assert!(has_scoped_status_page(&reopened_struct.actions));
            }
        }
    }

    // MenuSessionCoordinatorTests.swift:6
    fn invalidation_records_data_structural_and_required_generations_independently(
        _mtm: MainThreadMarker,
    ) {
        let mut coordinator = MenuSessionCoordinator::new();

        coordinator.invalidate(false, true);
        assert_eq!(coordinator.current_content_version(), 1);
        assert_eq!(coordinator.structural_content_version(), 1);
        assert_eq!(coordinator.required_rebuild_version(), 1);
        assert_eq!(coordinator.data_only_content_version(), 0);

        coordinator.invalidate(true, false);
        assert_eq!(coordinator.current_content_version(), 2);
        assert_eq!(coordinator.data_only_content_version(), 2);
        assert_eq!(coordinator.structural_content_version(), 1);
        assert_eq!(coordinator.required_rebuild_version(), 1);

        coordinator.invalidate(false, false);
        assert_eq!(coordinator.current_content_version(), 3);
        assert_eq!(coordinator.structural_content_version(), 3);
        assert_eq!(coordinator.required_rebuild_version(), 1);
    }

    // MenuSessionCoordinatorTests.swift:28
    fn closed_preparation_distinguishes_no_work_deferred_work_and_required_work(
        _mtm: MainThreadMarker,
    ) {
        let mut coordinator = MenuSessionCoordinator::new();
        let menu = "menu";

        assert_eq!(
            coordinator.closed_preparation_plan(&[menu]),
            ClosedPreparationPlan::NonDeferred
        );

        coordinator.invalidate(true, false);
        assert_eq!(
            coordinator.closed_preparation_plan(&[menu]),
            ClosedPreparationPlan::None
        );

        coordinator.invalidate(false, true);
        assert_eq!(
            coordinator.closed_preparation_plan(&[menu]),
            ClosedPreparationPlan::Required { version: 2 }
        );

        coordinator.mark_fresh(menu);
        assert_eq!(
            coordinator.closed_preparation_plan(&[menu]),
            ClosedPreparationPlan::NonDeferred
        );
    }

    // MenuSessionCoordinatorTests.swift:45
    fn stale_content_survives_only_a_data_generation_after_latest_structural_render(
        _mtm: MainThreadMarker,
    ) {
        let mut coordinator = MenuSessionCoordinator::new();
        let menu = "menu";

        coordinator.invalidate(false, true);
        coordinator.mark_fresh(menu);
        coordinator.invalidate(true, false);
        assert!(coordinator.can_preserve_stale_content(menu));

        coordinator.invalidate(false, false);
        coordinator.invalidate(true, false);
        assert!(!coordinator.can_preserve_stale_content(menu));
    }

    // MenuSessionCoordinatorTests.swift:60
    fn removing_menu_clears_all_menu_scoped_lifecycle_state(_mtm: MainThreadMarker) {
        let mut coordinator = MenuSessionCoordinator::new();
        let menu = "menu";

        coordinator.mark_fresh(menu);
        coordinator.defer_until_next_open(menu);
        coordinator.defer_parent_rebuild(menu);
        let _ = coordinator.begin_interaction_session(menu);
        let _ = coordinator.arm_viewport_restore(menu);
        coordinator.remove_menu(menu);

        assert_eq!(coordinator.rendered_version(menu), None);
        assert!(!coordinator.is_deferred_until_next_open(menu));
        assert!(!coordinator.has_deferred_parent_rebuild(menu));
        assert_eq!(coordinator.menu_interaction_generation(menu), None);
        assert!(!coordinator.has_pending_viewport_restores());
    }

    // MenuSessionCoordinatorTests.swift:79
    fn reopening_a_persistent_menu_replaces_its_tracking_session_token(_mtm: MainThreadMarker) {
        let mut coordinator = MenuSessionCoordinator::new();

        let closed_session = coordinator.begin_interaction_session("menu");
        coordinator.end_interaction_session("menu");
        let reopened_session = coordinator.begin_interaction_session("menu");

        assert_ne!(closed_session, reopened_session);
        assert!(!coordinator.is_current_menu_interaction("menu", closed_session));
        assert!(coordinator.is_current_menu_interaction("menu", reopened_session));
    }

    // MenuSessionCoordinatorTests.swift:92
    fn menu_interaction_token_advances_within_one_tracking_session(_mtm: MainThreadMarker) {
        let mut coordinator = MenuSessionCoordinator::new();
        let initial = coordinator.begin_interaction_session("menu");
        let replacement = coordinator
            .advance_menu_interaction("menu")
            .expect("active session");

        assert!(!coordinator.is_current_menu_interaction("menu", initial));
        assert!(coordinator.is_current_menu_interaction("menu", replacement));
    }

    // MenuSessionCoordinatorTests.swift:104
    fn replacement_viewport_restore_token_rejects_stale_completion(_mtm: MainThreadMarker) {
        let mut coordinator = MenuSessionCoordinator::new();

        let stale = coordinator.arm_viewport_restore("menu");
        let current = coordinator.arm_viewport_restore("menu");

        assert!(!coordinator.is_current_viewport_restore("menu", stale));
        assert!(!coordinator.consume_viewport_restore_generation("menu", stale));
        assert!(coordinator.is_current_viewport_restore("menu", current));
        assert!(coordinator.consume_viewport_restore_generation("menu", current));
        assert!(!coordinator.has_pending_viewport_restores());
    }

    // MenuSessionCoordinatorTests.swift:122
    fn replacement_request_invalidates_prior_token_without_affecting_other_menus(
        _mtm: MainThreadMarker,
    ) {
        let mut registry = MenuRebuildRequestRegistry::new();

        let first = registry.begin("parent");
        let child = registry.begin("child");
        let replacement = registry.begin("parent");

        assert!(!registry.is_current("parent", first));
        assert!(registry.is_current("parent", replacement));
        assert!(registry.is_current("child", child));
    }

    // MenuSessionCoordinatorTests.swift:135
    fn stale_completion_cannot_clear_replacement_request(_mtm: MainThreadMarker) {
        let mut registry = MenuRebuildRequestRegistry::new();
        let stale = registry.begin("menu");
        let current = registry.begin("menu");

        assert!(!registry.complete("menu", stale));
        assert!(registry.is_current("menu", current));
        assert!(registry.complete("menu", current));
        assert!(!registry.is_current("menu", current));
    }

    // MenuSessionCoordinatorTests.swift:149
    fn cancelling_all_requests_keeps_future_tokens_distinct(_mtm: MainThreadMarker) {
        let mut registry = MenuRebuildRequestRegistry::new();
        let cancelled = registry.begin("menu");

        registry.cancel_all();
        let replacement = registry.begin("menu");

        assert_ne!(cancelled, replacement);
        assert!(registry.is_current("menu", replacement));
    }

    // MenuOpenRefreshPlanTests.swift:7
    fn refresh_all_selects_every_enabled_provider_concurrently(_mtm: MainThreadMarker) {
        let plan = MenuOpenRefreshPlan::resolve(&MenuOpenRefreshInputs {
            refresh_all_on_open: true,
            enabled_providers: vec![
                "codex".to_string(),
                "claude".to_string(),
                "factory".to_string(),
            ],
            visible_providers: vec!["codex".to_string()],
            refreshing_providers: BTreeSet::new(),
            stale_providers: BTreeSet::new(),
            missing_providers: BTreeSet::new(),
        });

        assert_eq!(plan.providers, vec!["codex", "claude", "factory"]);
        assert_eq!(plan.scheduling, RefreshScheduling::Concurrent);
        assert!(plan.refresh_codex_dashboard);
        assert!(plan.refresh_token_cost);
    }

    // MenuOpenRefreshPlanTests.swift:23
    fn refresh_all_skips_dashboard_refresh_when_codex_is_disabled(_mtm: MainThreadMarker) {
        let plan = MenuOpenRefreshPlan::resolve(&MenuOpenRefreshInputs {
            refresh_all_on_open: true,
            enabled_providers: vec!["claude".to_string(), "factory".to_string()],
            visible_providers: vec!["claude".to_string()],
            refreshing_providers: BTreeSet::new(),
            stale_providers: BTreeSet::new(),
            missing_providers: BTreeSet::new(),
        });

        assert_eq!(plan.providers, vec!["claude", "factory"]);
        assert!(!plan.refresh_codex_dashboard);
        assert!(plan.refresh_token_cost);
    }

    // MenuOpenRefreshPlanTests.swift:38
    fn refresh_all_skips_token_cost_refresh_when_no_providers_are_enabled(_mtm: MainThreadMarker) {
        let plan = MenuOpenRefreshPlan::resolve(&MenuOpenRefreshInputs {
            refresh_all_on_open: true,
            enabled_providers: Vec::new(),
            visible_providers: Vec::new(),
            refreshing_providers: BTreeSet::new(),
            stale_providers: BTreeSet::new(),
            missing_providers: BTreeSet::new(),
        });

        assert!(plan.providers.is_empty());
        assert!(!plan.refresh_token_cost);
    }

    // MenuOpenRefreshPlanTests.swift:52
    fn ordinary_refresh_selects_only_visible_enabled_retries_sequentially(_mtm: MainThreadMarker) {
        let plan = MenuOpenRefreshPlan::resolve(&MenuOpenRefreshInputs {
            refresh_all_on_open: false,
            enabled_providers: vec![
                "codex".to_string(),
                "claude".to_string(),
                "factory".to_string(),
            ],
            visible_providers: vec![
                "factory".to_string(),
                "codex".to_string(),
                "claude".to_string(),
                "cursor".to_string(),
            ],
            refreshing_providers: ["factory".to_string()].into_iter().collect(),
            stale_providers: ["codex".to_string()].into_iter().collect(),
            missing_providers: ["claude".to_string(), "cursor".to_string()]
                .into_iter()
                .collect(),
        });

        assert_eq!(plan.providers, vec!["factory", "codex", "claude"]);
        assert_eq!(plan.scheduling, RefreshScheduling::Sequential);
        assert!(!plan.refresh_codex_dashboard);
        assert!(!plan.refresh_token_cost);
    }

    // MenuOpenRefreshPlanTests.swift:68
    fn ordinary_refresh_skips_fresh_providers(_mtm: MainThreadMarker) {
        let plan = MenuOpenRefreshPlan::resolve(&MenuOpenRefreshInputs {
            refresh_all_on_open: false,
            enabled_providers: vec!["codex".to_string()],
            visible_providers: vec!["codex".to_string()],
            refreshing_providers: BTreeSet::new(),
            stale_providers: BTreeSet::new(),
            missing_providers: BTreeSet::new(),
        });

        assert!(plan.providers.is_empty());
    }

    // MenuCardRefreshTests.swift:35
    fn background_refresh_keeps_quota_timing_current(_mtm: MainThreadMarker) {
        let updated_at = Timestamp::from_second(1_800_000_000).unwrap();
        let resets_at = Timestamp::from_second(1_800_000_000 + 4 * 3600 + 40 * 60).unwrap();
        let now_after_10m = Timestamp::from_second(1_800_000_000 + 10 * 60).unwrap();
        let dates = FoundationDateTexts::new();
        let prefs = CardPreferences::default();

        for provider_id in ["claude", "codex"] {
            let mut snap = sample_snapshot();
            {
                let provider = snap
                    .providers
                    .iter_mut()
                    .find(|p| p.id.as_str() == provider_id)
                    .unwrap();
                provider.updated_at = Some(updated_at);
                provider.windows.primary = Some(Metric::value(
                    RateWindow {
                        used_percent: 21.0,
                        window_minutes: Some(300),
                        resets_at: Some(resets_at),
                        reset_description: None,
                        next_regen_percent: None,
                        pace: None,
                        session_equivalent: None,
                    },
                    Some(updated_at),
                    None,
                    Confidence::Exact,
                ));
                provider.windows.secondary = None;
                provider.windows.tertiary = None;
                provider.windows.extra.clear();
            }
            let provider = snap
                .providers
                .iter()
                .find(|p| p.id.as_str() == provider_id)
                .unwrap();

            let completed_model = card::provider_card(provider, &prefs, updated_at, &dates);
            let mut monitor = MenuCardRefreshMonitor::new();
            monitor.begin_manual_refresh(
                ManualRefreshScope::Provider(provider_id.to_string()),
                Some(&snap),
                &prefs,
                updated_at,
                &dates,
            );
            let refreshing_candidate = card::provider_card(provider, &prefs, now_after_10m, &dates);
            let refreshing_model =
                monitor.effective_card_model(provider_id, refreshing_candidate, true);

            let completed_metric = &completed_model.metrics[0];
            let refreshing_metric = &refreshing_model.metrics[0];
            assert_eq!(refreshing_model.subtitle.text, "Refreshing…");
            assert_eq!(
                refreshing_metric.percent_label(),
                completed_metric.percent_label()
            );
            assert_eq!(
                completed_metric.reset_text.as_deref(),
                Some("Resets in 4h 40m")
            );
            assert_eq!(
                refreshing_metric.reset_text.as_deref(),
                Some("Resets in 4h 30m")
            );
        }
    }

    // -----------------------------------------------------------------------
    // 1. Additional StatusMenuSwitcherRefreshTests regression tests
    // -----------------------------------------------------------------------

    // Ported from StatusMenuSwitcherRefreshTests.swift:12
    // test_PopulateMenu_DoesNotOverwriteRefreshingOverrideSubtitle
    fn test_populate_menu_does_not_overwrite_refreshing_override_subtitle(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&claude_tab));

        menu.start_user_refresh(
            Some("claude".to_string()),
            Some(&snap),
            true,
            Some(&claude_tab),
        );
        // Re-populating with the same pre-refresh snapshot must keep `"Refreshing…"`.
        menu.update(Some(&snap), true, Some(&claude_tab));
        let PrimaryContent::Provider { card, .. } = &menu.last_structure().unwrap().primary else {
            panic!("expected provider card");
        };
        assert_eq!(card.subtitle.text, "Refreshing…");
        assert_eq!(card.subtitle.style, SubtitleStyle::Loading);
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:48
    // test_PopulateMenu_KeepsSelectedProviderRefreshOverrideWhenCachedUsageWasError
    fn test_populate_menu_keeps_selected_provider_refresh_override_when_cached_usage_was_error(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let mut snap = sample_snapshot();
        let claude = snap
            .providers
            .iter_mut()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        claude.last_error = Some(ProviderError {
            kind: ProviderErrorKind::NetworkFailure,
            category: ProviderErrorCategory::Network,
            message: "Previous fetch failed".to_string(),
            retry_after_seconds: None,
        });

        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&claude_tab));

        menu.start_user_refresh(
            Some("claude".to_string()),
            Some(&snap),
            true,
            Some(&claude_tab),
        );
        let PrimaryContent::Provider { card, .. } = &menu.last_structure().unwrap().primary else {
            panic!("expected provider card");
        };
        assert_eq!(card.subtitle.text, "Refreshing…");
        assert_eq!(card.subtitle.style, SubtitleStyle::Loading);
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:85
    // test_MakeMenuCardModel_FreezesSelectedProviderContentWhileManualRefreshIsInFlight
    fn test_make_menu_card_model_freezes_selected_provider_content_while_manual_refresh_is_in_flight(
        _mtm: MainThreadMarker,
    ) {
        let dates = FoundationDateTexts::new();
        let prefs = CardPreferences::default();
        let snap1 = sample_snapshot();
        let now = snap1.generated_at;

        let mut monitor = MenuCardRefreshMonitor::new();
        monitor.begin_manual_refresh(
            ManualRefreshScope::Provider("claude".to_string()),
            Some(&snap1),
            &prefs,
            now,
            &dates,
        );

        let mut snap_transient = snap1.clone();
        let claude = snap_transient
            .providers
            .iter_mut()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        claude.windows.primary = None;
        claude.windows.secondary = None;
        claude.windows.tertiary = None;
        claude.windows.extra.clear();

        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        let struct_refreshing = menu_descriptor::build_with_refresh(
            Some(&snap_transient),
            true,
            Some(&claude_tab),
            &prefs,
            now,
            &dates,
            Some(&monitor),
        );
        let PrimaryContent::Provider { card, .. } = &struct_refreshing.primary else {
            panic!("expected provider primary content");
        };
        assert_eq!(card.subtitle.text, "Refreshing…");
        assert_eq!(card.subtitle.style, SubtitleStyle::Loading);
        assert!(
            !card.metrics.is_empty(),
            "frozen card metrics must remain visible while manual refresh is in flight"
        );
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:127
    // test_PopulateMenu_RebuildsRefreshSubtitleWhenSelectedProviderStartsRefreshing
    fn test_populate_menu_rebuilds_refresh_subtitle_when_selected_provider_starts_refreshing(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&claude_tab));
        assert_eq!(
            menu.card_views()[0].model().unwrap().subtitle.text,
            "Updated 4 min ago"
        );

        menu.start_user_refresh(
            Some("claude".to_string()),
            Some(&snap),
            true,
            Some(&claude_tab),
        );
        assert_eq!(
            menu.card_views()[0].model().unwrap().subtitle.text,
            "Refreshing…"
        );
        assert_eq!(
            menu.card_views()[0].model().unwrap().subtitle.style,
            SubtitleStyle::Loading
        );
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:156
    // test_PopulateMenu_KeepsRefreshRowInteractiveForOtherProviderWhileSingleProviderRefreshIsInFlight
    fn test_populate_menu_keeps_refresh_row_interactive_for_other_provider_while_single_provider_refresh_is_in_flight(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        let codex_tab = SwitcherSelection::Provider("codex".to_string());

        menu.start_user_refresh(
            Some("claude".to_string()),
            Some(&snap),
            true,
            Some(&claude_tab),
        );
        assert!(
            !menu.refresh_item().isEnabled(),
            "Refresh must be disabled on the refreshing provider's tab"
        );

        menu.update(Some(&snap), true, Some(&codex_tab));
        assert!(
            menu.refresh_item().isEnabled(),
            "Refresh must remain enabled on other provider tabs while single-provider refresh is in flight"
        );
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:185
    // test_PopulateMenu_DoesNotDetachHoveredCustomViewDuringProviderSwitch
    fn test_populate_menu_does_not_detach_hovered_custom_view_during_provider_switch(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        let claude_tab = SwitcherSelection::Provider("claude".to_string());

        menu.update(Some(&snap), true, Some(&codex_tab));
        menu.simulate_will_open();

        let initial_card_item = menu.menu().itemAtIndex(2).expect("card item at index 2");
        let initial_card_view = menu.card_views()[0].clone();
        menu.simulate_will_highlight_item(Some(&initial_card_item));

        menu.update(Some(&snap), true, Some(&claude_tab));
        let next_card_item = menu.menu().itemAtIndex(2).expect("card item at index 2");
        assert!(
            std::ptr::eq(&*initial_card_item, &*next_card_item),
            "switching provider tabs while open must keep the usage card NSMenuItem attached at index 2"
        );
        assert!(
            std::ptr::eq(&*initial_card_view, &*menu.card_views()[0]),
            "switching provider tabs while open must update the CardView in place"
        );
        assert_eq!(
            menu.card_views()[0].model().unwrap().provider_name,
            "Claude"
        );
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:226
    // test_PopulateMenu_KeepsAllProvidersSwitcherSelectedForOverviewTab
    fn test_populate_menu_keeps_all_providers_switcher_selected_for_overview_tab(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let resolved = menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));
        assert_eq!(resolved, Some(SwitcherSelection::Overview));
        assert_eq!(
            menu.switcher_view().selected(),
            Some(SwitcherSelection::Overview)
        );
        assert_eq!(
            menu.switcher_view().button_state(0),
            Some(NSControlStateValueOn)
        );
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:267
    // test_PopulateMenu_OverviewRendersCompactProviderRowsWithMetricSubtitles
    fn test_populate_menu_overview_renders_compact_provider_rows_with_metric_subtitles(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));
        let PrimaryContent::Overview { cards, .. } = &menu.last_structure().unwrap().primary else {
            panic!("expected overview primary content");
        };
        assert_eq!(cards.len(), 3);
        assert!(cards.iter().all(|c| !c.model.metrics.is_empty()));
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:318
    // test_PopulateMenu_OverviewSpendSummaryPrependsCardAndShareAction
    fn test_populate_menu_overview_spend_summary_prepends_card_and_share_action(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));
        let PrimaryContent::Overview {
            spend_summary,
            share_action,
            ..
        } = &menu.last_structure().unwrap().primary
        else {
            panic!("expected overview primary content");
        };
        assert!(spend_summary.is_some());
        assert!(matches!(
            share_action,
            Some(MenuEntry::Action {
                action: MenuAction::ShareUsageSnapshot,
                ..
            })
        ));
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:361
    // test_PopulateMenu_OverviewSpendSummaryOmitsCardWhenTotalIsZero
    fn test_populate_menu_overview_spend_summary_omits_card_when_total_is_zero(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let mut snap = sample_snapshot();
        for p in &mut snap.providers {
            p.cost_history = None;
            p.credits = None;
            p.cost = None;
        }
        menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));
        let PrimaryContent::Overview {
            spend_summary,
            share_action,
            ..
        } = &menu.last_structure().unwrap().primary
        else {
            panic!("expected overview primary content");
        };
        assert!(spend_summary.is_none());
        assert!(share_action.is_none());
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:395
    // test_PopulateMenu_OverviewRowClickSwitchesToProviderTab
    fn test_populate_menu_overview_row_click_switches_to_provider_tab(mtm: MainThreadMarker) {
        let dir = tempfile::tempdir().expect("tempdir");
        let client = dummy_client(dir.path());
        let selected: Rc<RefCell<Option<SwitcherSelection>>> = Rc::new(RefCell::new(None));
        let selected_cb = selected.clone();
        let menu = MenuController::new_with_callbacks(
            mtm,
            client,
            || {},
            move |sel| {
                *selected_cb.borrow_mut() = Some(sel);
            },
            |_| {},
        );
        let snap = sample_snapshot();
        menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));
        let card_items = menu.card_items();
        // Index 0 is spend summary card; index 1 is the first provider card ("claude").
        let claude_card_item = &card_items[1];
        let target = claude_card_item.target().expect("overview card target");
        let _action = claude_card_item.action().expect("overview card action");
        unsafe {
            let _: () = objc2::msg_send![&*target, selectOverviewRow: &**claude_card_item];
        }
        assert_eq!(
            *selected.borrow(),
            Some(SwitcherSelection::Provider("claude".to_string()))
        );
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:412
    // test_PopulateMenu_OverviewAttachesChartsSubmenuToProviderRow
    fn test_populate_menu_overview_attaches_charts_submenu_to_provider_row(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));
        let card_items = menu.card_items();
        assert!(
            card_items[1].submenu().is_some(),
            "Overview provider row with history must attach a charts submenu"
        );
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:432
    // test_PopulateMenu_OverviewChartsSubmenuContainsUsageAndCostSections
    fn test_populate_menu_overview_charts_submenu_contains_usage_and_cost_sections(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));
        let PrimaryContent::Overview { cards, .. } = &menu.last_structure().unwrap().primary else {
            panic!("expected overview");
        };
        let claude_ov = cards.iter().find(|c| c.provider_id == "claude").unwrap();
        assert_eq!(claude_ov.submenu_title.as_deref(), Some("Cost"));
        assert!(
            claude_ov
                .submenu
                .iter()
                .any(|e| matches!(e, SubmenuEntry::CostHistoryChart(_)))
        );

        // When cost_history is absent, Overview falls back to the Plan Usage chart submenu.
        let mut snap_no_cost = snap;
        for p in &mut snap_no_cost.providers {
            p.cost_history = None;
        }
        menu.update(
            Some(&snap_no_cost),
            true,
            Some(&SwitcherSelection::Overview),
        );
        let PrimaryContent::Overview { cards, .. } = &menu.last_structure().unwrap().primary else {
            panic!("expected overview");
        };
        let claude_ov_plan = cards.iter().find(|c| c.provider_id == "claude").unwrap();
        assert_eq!(claude_ov_plan.submenu_title.as_deref(), Some("Plan Usage"));
        assert!(claude_ov_plan.submenu.iter().any(|e| matches!(
            e,
            SubmenuEntry::QuotaBurndownChart(_) | SubmenuEntry::PlanUtilizationChart(_)
        )));
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:455
    // test_PopulateMenu_OverviewOmitsChartsSubmenuWhenNoHistory
    fn test_populate_menu_overview_omits_charts_submenu_when_no_history(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let mut snap = sample_snapshot();
        for p in &mut snap.providers {
            p.cost_history = None;
            p.utilization = None;
        }
        menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));
        let PrimaryContent::Overview { cards, .. } = &menu.last_structure().unwrap().primary else {
            panic!("expected overview");
        };
        assert!(cards.iter().all(|c| c.submenu.is_empty()));
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:472
    // test_PopulateMenu_ProviderTabRestoresBuyCreditsAndTopLevelSubmenusAfterOverview
    fn test_populate_menu_provider_tab_restores_buy_credits_and_top_level_submenus_after_overview(
        mtm: MainThreadMarker,
    ) {
        let (_dir, mut menu) = make_controller(mtm);
        menu.set_prefs(CardPreferences {
            show_optional_credits_and_extra_usage: true,
            ..CardPreferences::default()
        });
        let snap = sample_snapshot();
        menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));

        let codex_tab = SwitcherSelection::Provider("codex".to_string());
        menu.update(Some(&snap), true, Some(&codex_tab));
        let structure = menu.last_structure().unwrap();
        let PrimaryContent::Provider { buy_credits, .. } = &structure.primary else {
            panic!("expected provider primary content");
        };
        assert!(
            buy_credits.is_some(),
            "Codex tab must restore Buy Credits row"
        );
        assert!(
            !structure.history.is_empty(),
            "Provider tab must restore top-level history submenus after Overview"
        );
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:492
    // test_PopulateMenu_OpenRefreshPreservesSwitcherAndCardViewsWhenLayoutIsUnchanged
    fn test_populate_menu_open_refresh_preserves_switcher_and_card_views_when_layout_is_unchanged(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap1 = sample_snapshot();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap1), true, Some(&claude_tab));
        menu.simulate_will_open();

        let sw_ptr_before = std::ptr::from_ref(menu.switcher_view());
        let card_ptr_before = Retained::as_ptr(&menu.card_views()[0]);
        let rebuilds_before = menu.body_rebuild_count();

        let mut snap2 = snap1.clone();
        snap2.generated_at = Timestamp::from_second(snap1.generated_at.as_second() + 10).unwrap();
        menu.update(Some(&snap2), true, Some(&claude_tab));

        assert_eq!(menu.body_rebuild_count(), rebuilds_before);
        assert_eq!(std::ptr::from_ref(menu.switcher_view()), sw_ptr_before);
        assert_eq!(Retained::as_ptr(&menu.card_views()[0]), card_ptr_before);
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:512
    // test_PopulateMenu_OpenRefreshUpdatesCardInPlaceWhenHeightStaysCompatible
    fn test_populate_menu_open_refresh_updates_card_in_place_when_height_stays_compatible(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap1 = sample_snapshot();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap1), true, Some(&claude_tab));
        menu.simulate_will_open();

        let card_ptr_before = Retained::as_ptr(&menu.card_views()[0]);
        let in_place_before = menu.in_place_update_count();

        let mut snap2 = snap1.clone();
        snap2.generated_at = Timestamp::from_second(snap1.generated_at.as_second() + 15).unwrap();
        let claude = snap2
            .providers
            .iter_mut()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        if let Some(primary) = claude.windows.primary.as_mut()
            && let Some(val) = primary.value.as_mut()
        {
            val.used_percent = 83.0;
        }
        menu.update(Some(&snap2), true, Some(&claude_tab));

        assert_eq!(menu.in_place_update_count(), in_place_before + 1);
        assert_eq!(Retained::as_ptr(&menu.card_views()[0]), card_ptr_before);
        assert_eq!(
            menu.card_views()[0].model().unwrap().metrics[0].percent_label(),
            "17% left"
        );
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:531
    // test_PopulateMenu_OpenRefreshDefersIncompatibleCardResizeUntilMenuCloses
    fn test_populate_menu_open_refresh_defers_incompatible_card_resize_until_menu_closes(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap1 = sample_snapshot();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap1), true, Some(&claude_tab));
        menu.simulate_will_open();

        let card_height_open = menu.card_views()[0].layout().unwrap().size.h;
        let rebuilds_open = menu.body_rebuild_count();

        let mut snap_incompat = snap1.clone();
        snap_incompat.generated_at =
            Timestamp::from_second(snap1.generated_at.as_second() + 20).unwrap();
        let claude = snap_incompat
            .providers
            .iter_mut()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        claude.windows.secondary = None;
        claude.windows.tertiary = None;
        claude.cost = None;

        menu.update(Some(&snap_incompat), true, Some(&claude_tab));
        assert_eq!(menu.body_rebuild_count(), rebuilds_open);
        assert_eq!(
            menu.card_views()[0].layout().unwrap().size.h,
            card_height_open
        );
        assert!(menu.has_deferred_parent_rebuild());
        assert!(menu.is_deferred_until_next_open());

        menu.simulate_did_close();
        menu.update(Some(&snap_incompat), true, Some(&claude_tab));
        assert!(menu.card_views()[0].layout().unwrap().size.h < card_height_open);
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:551
    // test_PopulateMenu_OpenRefreshReplacesActionRowsWhenErrorCreatesRecoveryActions
    fn test_populate_menu_open_refresh_replaces_action_rows_when_error_creates_recovery_actions(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let mut snap1 = sample_snapshot();
        let claude = snap1
            .providers
            .iter_mut()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        // Omit the operational status metric so `Status Page` is not yet in `actions`.
        claude.status = None;

        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap1), true, Some(&claude_tab));
        menu.simulate_will_open();

        let card_ptr_before = Retained::as_ptr(&menu.card_views()[0]);
        let history_ptr_before = Retained::as_ptr(&menu.history_items()[0]);
        let refresh_ptr_before = std::ptr::from_ref(menu.refresh_item());
        let action_count_before = menu.action_items().len();

        // Now a second snapshot reports a minor operational incident so `actions` gains a
        // status secondary-text row while the card height itself remains unchanged.
        let mut snap2 = snap1.clone();
        snap2.generated_at = Timestamp::from_second(snap1.generated_at.as_second() + 15).unwrap();
        let claude2 = snap2
            .providers
            .iter_mut()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        claude2.status = Some(Metric::value(
            ProviderStatus {
                indicator: StatusIndicator::Minor,
                description: Some("Elevated API latency".to_string()),
                updated_at: None,
                page_url: Some("https://status.anthropic.com".to_string()),
                components: Vec::new(),
            },
            snap2.generated_at.into(),
            None,
            Confidence::Exact,
        ));
        menu.update(Some(&snap2), true, Some(&claude_tab));

        assert!(menu.middle_reconcile_count() >= 1);
        assert_eq!(
            Retained::as_ptr(&menu.card_views()[0]),
            card_ptr_before,
            "CardView must remain attached during middle-section reconciliation"
        );
        assert_eq!(
            Retained::as_ptr(&menu.history_items()[0]),
            history_ptr_before,
            "Unchanged prefix history NSMenuItem must remain attached without recreation"
        );
        assert_eq!(
            std::ptr::from_ref(menu.refresh_item()),
            refresh_ptr_before,
            "Persistent Refresh item in meta suffix must remain attached"
        );
        assert_eq!(menu.action_items().len(), action_count_before + 1);
        assert!(menu.consume_viewport_restore());
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:578
    // test_reconcileMenuContent_keepsUnchangedSuffixItemsWhenRefreshingSectionChanges
    fn test_reconcile_menu_content_keeps_unchanged_suffix_items_when_refreshing_section_changes(
        mtm: MainThreadMarker,
    ) {
        let menu = NSMenu::new(mtm);
        menu.addItem(&make_scratch_item(mtm, "Switcher", None));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let card_item = make_scratch_item(mtm, "Card", None);
        let sep_primary = NSMenuItem::separatorItem(mtm);
        let action_row = make_scratch_item(mtm, "Updated 4m ago", None);
        let sep_actions = NSMenuItem::separatorItem(mtm);
        let refresh_item = make_scratch_item(mtm, "Refresh", None);
        let quit_item = make_scratch_item(mtm, "Quit", None);
        for it in [
            &card_item,
            &sep_primary,
            &action_row,
            &sep_actions,
            &refresh_item,
            &quit_item,
        ] {
            menu.addItem(it);
        }

        let existing_shapes = vec![
            MenuRowShape::CardSection {
                provider_id: "claude".to_string(),
                has_submenu: false,
            },
            MenuRowShape::Separator,
            MenuRowShape::SecondaryText,
            MenuRowShape::Separator,
            MenuRowShape::PersistentRefreshAction,
            MenuRowShape::LeafAction {
                title: "Quit".to_string(),
                has_symbol_image: false,
            },
        ];

        let scratch = NSMenu::new(mtm);
        for it in [
            make_scratch_item(mtm, "Card", None),
            NSMenuItem::separatorItem(mtm),
            make_scratch_item(mtm, "Usage", None),
            NSMenuItem::separatorItem(mtm),
            make_scratch_item(mtm, "Updated just now", None),
            NSMenuItem::separatorItem(mtm),
            make_scratch_item(mtm, "Refresh", None),
            make_scratch_item(mtm, "Quit", None),
        ] {
            scratch.addItem(&it);
        }

        let desired_shapes = vec![
            MenuRowShape::CardSection {
                provider_id: "claude".to_string(),
                has_submenu: false,
            },
            MenuRowShape::Separator,
            MenuRowShape::Submenu {
                title: "Usage".to_string(),
                has_symbol_image: false,
            },
            MenuRowShape::Separator,
            MenuRowShape::SecondaryText,
            MenuRowShape::Separator,
            MenuRowShape::PersistentRefreshAction,
            MenuRowShape::LeafAction {
                title: "Quit".to_string(),
                has_symbol_image: false,
            },
        ];

        let plan = MenuController::reconcile_menu_from_scratch(
            &menu,
            2,
            &existing_shapes,
            &desired_shapes,
            &scratch,
        );
        assert_eq!(plan.prefix_count, 2);
        assert_eq!(plan.suffix_count, 4);
        assert!(std::ptr::eq(&*menu.itemAtIndex(2).unwrap(), &*card_item));
        assert!(std::ptr::eq(&*menu.itemAtIndex(8).unwrap(), &*refresh_item));
        assert!(std::ptr::eq(&*menu.itemAtIndex(9).unwrap(), &*quit_item));
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:608
    // test_reconcileMenuContent_replacesImageActionRowsInsteadOfMutatingThemInPlace
    fn test_reconcile_menu_content_replaces_image_action_rows_instead_of_mutating_them_in_place(
        mtm: MainThreadMarker,
    ) {
        let menu = NSMenu::new(mtm);
        menu.addItem(&make_scratch_item(mtm, "Switcher", None));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let old_image_action = make_scratch_item(mtm, "Add Account...", Some("plus"));
        let quit_item = make_scratch_item(mtm, "Quit", None);
        menu.addItem(&old_image_action);
        menu.addItem(&quit_item);

        let existing_shapes = vec![
            MenuRowShape::LeafAction {
                title: "Add Account...".to_string(),
                has_symbol_image: true,
            },
            MenuRowShape::LeafAction {
                title: "Quit".to_string(),
                has_symbol_image: false,
            },
        ];

        let scratch = NSMenu::new(mtm);
        let new_image_action =
            make_scratch_item(mtm, "Switch Account...", Some("arrow.left.arrow.right"));
        scratch.addItem(&new_image_action);
        scratch.addItem(&make_scratch_item(mtm, "Quit", None));

        let desired_shapes = vec![
            MenuRowShape::LeafAction {
                title: "Switch Account...".to_string(),
                has_symbol_image: true,
            },
            MenuRowShape::LeafAction {
                title: "Quit".to_string(),
                has_symbol_image: false,
            },
        ];

        let plan = MenuController::reconcile_menu_from_scratch(
            &menu,
            2,
            &existing_shapes,
            &desired_shapes,
            &scratch,
        );
        assert_eq!(plan.prefix_count, 0);
        assert_eq!(plan.suffix_count, 1);
        assert!(
            !std::ptr::eq(&*menu.itemAtIndex(2).unwrap(), &*old_image_action),
            "image action row must be replaced rather than mutated in place"
        );
        assert!(
            std::ptr::eq(&*menu.itemAtIndex(3).unwrap(), &*quit_item),
            "non-image suffix item must remain attached"
        );
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:639
    // test_PopulateMenu_UsesSingleSwitcherRowWithoutFallbackSubmenu
    fn test_populate_menu_uses_single_switcher_row_without_fallback_submenu(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));

        let switcher_item = menu.menu().itemAtIndex(0).expect("switcher item at 0");
        assert!(!switcher_item.isHidden());
        assert!(switcher_item.view().is_some());
        assert!(
            switcher_item.submenu().is_none(),
            "switcher row must not attach a fallback submenu"
        );
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:662
    // test_openMenuBackgroundRefreshDefersRebuildWhileSubmenuIsOpen
    fn test_open_menu_background_refresh_defers_rebuild_while_submenu_is_open(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap1 = sample_snapshot();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap1), true, Some(&claude_tab));
        menu.simulate_will_open();

        let history_items = menu.history_items();
        menu.simulate_will_highlight_item(Some(&history_items[0]));

        let mut snap_incompat = snap1.clone();
        snap_incompat.generated_at =
            Timestamp::from_second(snap1.generated_at.as_second() + 30).unwrap();
        let claude = snap_incompat
            .providers
            .iter_mut()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        claude.windows.secondary = None;
        claude.windows.tertiary = None;
        claude.cost = None;

        let rebuilds_before = menu.body_rebuild_count();
        menu.update(Some(&snap_incompat), true, Some(&claude_tab));
        assert_eq!(
            menu.body_rebuild_count(),
            rebuilds_before,
            "background refresh must defer structural rebuild while submenu item is open"
        );
        assert!(menu.has_deferred_parent_rebuild());
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:722
    // test_openMenuBackgroundRefreshDefersRebuildWhileNativeItemIsHighlighted
    fn test_open_menu_background_refresh_defers_rebuild_while_native_item_is_highlighted(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap1 = sample_snapshot();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap1), true, Some(&claude_tab));
        menu.simulate_will_open();

        let meta_items = menu.meta_items();
        let settings_item = &meta_items[1];
        menu.simulate_will_highlight_item(Some(settings_item));

        let mut snap_incompat = snap1.clone();
        snap_incompat.generated_at =
            Timestamp::from_second(snap1.generated_at.as_second() + 30).unwrap();
        let claude = snap_incompat
            .providers
            .iter_mut()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        claude.windows.secondary = None;
        claude.windows.tertiary = None;
        claude.cost = None;

        let rebuilds_before = menu.body_rebuild_count();
        menu.update(Some(&snap_incompat), true, Some(&claude_tab));
        assert_eq!(
            menu.body_rebuild_count(),
            rebuilds_before,
            "background refresh must defer structural rebuild while native menu item is highlighted"
        );
        assert!(menu.has_deferred_parent_rebuild());
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:780
    // test_openMenuRebuildUpdatesUsageBreakdownSubmenuInPlace
    fn test_open_menu_rebuild_updates_usage_breakdown_submenu_in_place(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap1 = sample_snapshot();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap1), true, Some(&claude_tab));
        menu.simulate_will_open();

        let usage_item = menu.history_items()[0].clone();
        let usage_sub_ptr = Retained::as_ptr(&usage_item.submenu().unwrap());
        let chart_view_ptr = Retained::as_ptr(
            &usage_item
                .submenu()
                .unwrap()
                .itemAtIndex(0)
                .unwrap()
                .view()
                .unwrap(),
        );

        let mut snap2 = snap1.clone();
        snap2.generated_at = Timestamp::from_second(snap1.generated_at.as_second() + 20).unwrap();
        let claude = snap2
            .providers
            .iter_mut()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        if let Some(util) = claude.utilization.as_mut()
            && let Some(series_list) = util.value.as_mut()
            && let Some(series) = series_list.first_mut()
            && let Some(entry) = series.entries.first_mut()
        {
            entry.used_percent = 91.0;
        }
        menu.update(Some(&snap2), true, Some(&claude_tab));

        let usage_sub_after = menu.history_items()[0].submenu().unwrap();
        assert_eq!(Retained::as_ptr(&usage_sub_after), usage_sub_ptr);
        assert_eq!(
            Retained::as_ptr(&usage_sub_after.itemAtIndex(0).unwrap().view().unwrap()),
            chart_view_ptr,
            "chart NSView inside Usage submenu must be updated in place without recreation"
        );
    }

    // Ported from StatusMenuSwitcherRefreshTests.swift:835
    // test_openMenuRebuildReusesPersistentRefreshMenuItemInstance
    fn test_open_menu_rebuild_reuses_persistent_refresh_menu_item_instance(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let refresh_ptr = std::ptr::from_ref(menu.refresh_item());

        menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));
        assert_eq!(Retained::as_ptr(&menu.meta_items()[0]), refresh_ptr);

        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&claude_tab));
        assert_eq!(Retained::as_ptr(&menu.meta_items()[0]), refresh_ptr);

        menu.update(None, false, None);
        assert_eq!(Retained::as_ptr(&menu.meta_items()[0]), refresh_ptr);
    }

    // -----------------------------------------------------------------------
    // 2. StatusMenuSwitcherTrackingTests.swift (5 tests)
    // -----------------------------------------------------------------------

    // Ported from StatusMenuSwitcherTrackingTests.swift:6
    // test_switcherButtonPerformsClickInsideTrackingRunLoopMode
    fn test_switcher_button_performs_click_inside_tracking_run_loop_mode(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));

        let selected: Rc<RefCell<Vec<SwitcherSelection>>> = Rc::new(RefCell::new(Vec::new()));
        let selected_cb = selected.clone();
        menu.switcher_view().set_on_select(move |sel| {
            selected_cb.borrow_mut().push(sel);
        });

        menu.switcher_view().simulate_runtime_click(1);

        assert_eq!(
            *selected.borrow(),
            vec![SwitcherSelection::Provider("claude".to_string())]
        );
    }

    // Ported from StatusMenuSwitcherTrackingTests.swift:62
    // test_switcherViewKeepsRefreshPausedAcrossPressReleaseAndDeferredSelectionApply
    fn test_switcher_view_keeps_refresh_paused_across_press_release_and_deferred_selection_apply(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));

        let events: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
        let events_cb = events.clone();
        menu.switcher_view()
            .set_on_pointer_interaction(move |active| {
                events_cb.borrow_mut().push(active);
            });

        menu.switcher_view().begin_pointer_interaction(1);
        assert!(menu.switcher_view().is_pointer_interaction_active());
        menu.switcher_view().end_pointer_interaction(Some(1));
        assert!(!menu.switcher_view().is_pointer_interaction_active());
        assert_eq!(*events.borrow(), vec![true, false]);
    }

    // Ported from StatusMenuSwitcherTrackingTests.swift:127
    // test_switcherButtonRestoresSelectedStateWhenReclickingCurrentSegment
    fn test_switcher_button_restores_selected_state_when_reclicking_current_segment(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&claude_tab));

        let selections: Rc<RefCell<Vec<SwitcherSelection>>> = Rc::new(RefCell::new(Vec::new()));
        let selections_cb = selections.clone();
        menu.switcher_view().set_on_select(move |sel| {
            selections_cb.borrow_mut().push(sel);
        });

        // Index 1 is Claude (already selected). Clicking it again when AppKit toggles it off
        // must restore NSControlStateValueOn and not emit a duplicate selection change.
        menu.switcher_view()
            .simulate_native_action(1, NSControlStateValueOff);
        assert!(selections.borrow().is_empty());
        assert_eq!(
            menu.switcher_view().button_state(1),
            Some(NSControlStateValueOn)
        );
        assert_eq!(
            menu.switcher_view().button_state(0),
            Some(NSControlStateValueOff)
        );
    }

    // Ported from StatusMenuSwitcherTrackingTests.swift:178
    // test_openMenuDefersLiveRebuildWhileSwitcherButtonIsPressed
    fn test_open_menu_defers_live_rebuild_while_switcher_button_is_pressed(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap1 = sample_snapshot();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap1), true, Some(&claude_tab));
        menu.simulate_will_open();

        menu.switcher_view().begin_pointer_interaction(1);
        let mut snap2 = snap1.clone();
        snap2.generated_at = Timestamp::from_second(snap1.generated_at.as_second() + 15).unwrap();
        let claude = snap2
            .providers
            .iter_mut()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        if let Some(primary) = claude.windows.primary.as_mut()
            && let Some(val) = primary.value.as_mut()
        {
            val.used_percent = 95.0;
        }

        let in_place_before = menu.in_place_update_count();
        menu.update(Some(&snap2), true, Some(&claude_tab));
        assert_eq!(
            menu.in_place_update_count(),
            in_place_before,
            "live update must be deferred while switcher button is pressed"
        );
        assert!(menu.has_deferred_parent_rebuild());

        menu.switcher_view().end_pointer_interaction(None);
        menu.update(Some(&snap2), true, Some(&claude_tab));
        assert_eq!(
            menu.card_views()[0].model().unwrap().metrics[0].percent_label(),
            "5% left"
        );
    }

    // Ported from StatusMenuSwitcherTrackingTests.swift:222
    // test_switcherViewTriggersSelectionOnlyOnceWhenNativeActionAndRuntimeClickBothFire
    fn test_switcher_view_triggers_selection_only_once_when_native_action_and_runtime_click_both_fire(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));

        let fired: Rc<RefCell<Vec<SwitcherSelection>>> = Rc::new(RefCell::new(Vec::new()));
        let fired_cb = fired.clone();
        menu.switcher_view().set_on_select(move |sel| {
            fired_cb.borrow_mut().push(sel);
        });

        menu.switcher_view().begin_pointer_interaction(2);
        menu.switcher_view()
            .simulate_native_action(2, NSControlStateValueOn);
        menu.switcher_view().end_pointer_interaction(Some(2));

        assert_eq!(
            *fired.borrow(),
            vec![SwitcherSelection::Provider("codex".to_string())],
            "duplicate native action + pointer release within one press must fire on_select only once"
        );
    }

    // -----------------------------------------------------------------------
    // 3. StatusMenuSwitcherWarmupTests.swift (5 tests)
    // -----------------------------------------------------------------------

    // Ported from StatusMenuSwitcherWarmupTests.swift:61
    // test_warmupPopulatesNonSelectedSwitcherCandidates
    fn test_warmup_populates_non_selected_switcher_candidates(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&claude_tab));
        assert_eq!(menu.cached_tab_count(), 1);

        menu.simulate_will_open();
        menu.update(Some(&snap), true, Some(&claude_tab));
        assert_eq!(
            menu.cached_tab_count(),
            4,
            "opening menu must warm Overview + all 3 enabled provider tabs"
        );
    }

    // Ported from StatusMenuSwitcherWarmupTests.swift:86
    // test_warmupSkipsSelectionsAlreadyCached
    fn test_warmup_skips_selections_already_cached(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&claude_tab));
        menu.simulate_will_open();
        menu.update(Some(&snap), true, Some(&claude_tab));

        let builds_after_first_warmup = menu.warmup_build_count();
        assert_eq!(builds_after_first_warmup, 3);

        let newly_warmed = menu.warm_sibling_content(&snap, true, &claude_tab);
        assert!(
            newly_warmed.is_empty(),
            "second warmup at same version/width/prefs must skip selections already cached"
        );
        assert_eq!(menu.warmup_build_count(), builds_after_first_warmup);
    }

    // Ported from StatusMenuSwitcherWarmupTests.swift:115
    // test_cachedSwitcherContentInvalidatesWhenMenuContentVersionChanges
    fn test_cached_switcher_content_invalidates_when_menu_content_version_changes(
        mtm: MainThreadMarker,
    ) {
        let (_dir, menu) = make_controller(mtm);
        let snap = sample_snapshot();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&claude_tab));
        menu.simulate_will_open();
        menu.update(Some(&snap), true, Some(&claude_tab));
        assert_eq!(menu.cached_tab_count(), 4);

        menu.invalidate_menus(false, true);
        assert_eq!(
            menu.cached_tab_count(),
            0,
            "invalidate_menus must clear cached switcher entries when version changes"
        );
    }

    // Ported from StatusMenuSwitcherWarmupTests.swift:141
    // test_switchingTabsPreservesPrewarmedSiblingCacheEntries
    fn test_switching_tabs_preserves_prewarmed_sibling_cache_entries(_mtm: MainThreadMarker) {
        let dates = FoundationDateTexts::new();
        let prefs = CardPreferences::default();
        let snap = sample_snapshot();
        let now = snap.generated_at;
        let mut cache = MergedSwitcherContentCache::new();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());

        let claude_structure = menu_descriptor::build_with_refresh(
            Some(&snap),
            true,
            Some(&claude_tab),
            &prefs,
            now,
            &dates,
            None,
        );
        cache.insert(CachedMergedSwitcherMenuContent {
            selection: claude_tab.clone(),
            required_menu_content_version: 1,
            menu_width: 310.0,
            prefs: prefs.clone(),
            structure: claude_structure,
            has_blank_padding_rows: false,
        });
        cache.warm_sibling_content(
            &snap,
            true,
            &claude_tab,
            1,
            310.0,
            &prefs,
            now,
            &dates,
            None,
        );
        assert_eq!(cache.len(), 4);

        cache.with_preserved_caches(|inner| {
            inner.invalidate();
        });
        assert_eq!(
            cache.len(),
            4,
            "switching tabs with preserved caches must keep prewarmed sibling entries"
        );
    }

    // Ported from StatusMenuSwitcherWarmupTests.swift:164
    // test_prewarmedSwitcherContentNeverUsesFlexibleBlankRows
    fn test_prewarmed_switcher_content_never_uses_flexible_blank_rows(_mtm: MainThreadMarker) {
        let dates = FoundationDateTexts::new();
        let prefs = CardPreferences::default();
        let snap = sample_snapshot();
        let now = snap.generated_at;
        let mut cache = MergedSwitcherContentCache::new();
        let claude_tab = SwitcherSelection::Provider("claude".to_string());

        cache.warm_sibling_content(
            &snap,
            true,
            &claude_tab,
            1,
            310.0,
            &prefs,
            now,
            &dates,
            None,
        );
        for entry in cache.cached_entries() {
            assert!(
                !entry.has_blank_padding_rows,
                "prewarmed switcher content must never use flexible blank padding rows"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 4. Live IPC, Refresh Intent, Delayed Open-Refresh, and Reconnect Tests
    // -----------------------------------------------------------------------

    struct MockDaemonServer {
        stop: Arc<AtomicBool>,
        clients: Arc<Mutex<Vec<UnixStream>>>,
        current_snapshot: Arc<Mutex<EngineSnapshot>>,
        refresh_requests: Arc<Mutex<Vec<Option<String>>>>,
        accept_thread: Option<JoinHandle<()>>,
    }

    impl MockDaemonServer {
        fn bind(socket_path: &Path, initial: EngineSnapshot) -> Self {
            let _ = std::fs::remove_file(socket_path);
            let listener = UnixListener::bind(socket_path).expect("bind mock daemon socket");
            listener
                .set_nonblocking(true)
                .expect("set_nonblocking on listener");

            let stop = Arc::new(AtomicBool::new(false));
            let clients: Arc<Mutex<Vec<UnixStream>>> = Arc::new(Mutex::new(Vec::new()));
            let current_snapshot = Arc::new(Mutex::new(initial));
            let refresh_requests: Arc<Mutex<Vec<Option<String>>>> =
                Arc::new(Mutex::new(Vec::new()));

            let stop_clone = Arc::clone(&stop);
            let clients_clone = Arc::clone(&clients);
            let snap_clone = Arc::clone(&current_snapshot);
            let refresh_clone = Arc::clone(&refresh_requests);

            let accept_thread = std::thread::spawn(move || {
                while !stop_clone.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let push_stream = stream.try_clone().expect("clone stream");
                            clients_clone.lock().unwrap().push(push_stream);
                            let snap_for_conn = Arc::clone(&snap_clone);
                            let refresh_for_conn = Arc::clone(&refresh_clone);
                            let stop_for_conn = Arc::clone(&stop_clone);
                            std::thread::spawn(move || {
                                handle_mock_client(
                                    stream,
                                    snap_for_conn,
                                    refresh_for_conn,
                                    stop_for_conn,
                                );
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            });

            Self {
                stop,
                clients,
                current_snapshot,
                refresh_requests,
                accept_thread: Some(accept_thread),
            }
        }

        fn push_snapshot(&self, snapshot: EngineSnapshot) {
            *self.current_snapshot.lock().unwrap() = snapshot.clone();
            let msg = ServerMessage::Snapshot {
                snapshot: Box::new(snapshot),
            };
            let line = serde_json::to_string(&msg).expect("serialize push") + "\n";
            let mut guard = self.clients.lock().unwrap();
            guard.retain_mut(|stream| {
                stream.write_all(line.as_bytes()).is_ok() && stream.flush().is_ok()
            });
        }

        fn refresh_requests(&self) -> Vec<Option<String>> {
            self.refresh_requests.lock().unwrap().clone()
        }

        fn drop_all_clients(&self) {
            let mut guard = self.clients.lock().unwrap();
            for stream in guard.drain(..) {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }

        fn shutdown(mut self) {
            self.stop.store(true, Ordering::Relaxed);
            self.drop_all_clients();
            if let Some(handle) = self.accept_thread.take() {
                let _ = handle.join();
            }
        }
    }

    fn handle_mock_client(
        stream: UnixStream,
        current_snapshot: Arc<Mutex<EngineSnapshot>>,
        refresh_requests: Arc<Mutex<Vec<Option<String>>>>,
        stop: Arc<AtomicBool>,
    ) {
        let mut writer = match stream.try_clone() {
            Ok(w) => w,
            Err(_) => return,
        };
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(req) = serde_json::from_str::<ClientMessage>(&line) else {
                break;
            };
            match req {
                ClientMessage::Hello { id, .. } => {
                    let resp = ServerMessage::Welcome {
                        protocol: PROTOCOL_V1.to_string(),
                        engine_version: "0.1.0".to_string(),
                        capabilities: vec![
                            "snapshot".to_string(),
                            "settings".to_string(),
                            "notices".to_string(),
                        ],
                        id,
                    };
                    if write_msg(&mut writer, &resp).is_err() {
                        break;
                    }
                }
                ClientMessage::Subscribe { id, .. } => {
                    let resp = ServerMessage::ok(id);
                    if write_msg(&mut writer, &resp).is_err() {
                        break;
                    }
                    let snap = current_snapshot.lock().unwrap().clone();
                    let push = ServerMessage::Snapshot {
                        snapshot: Box::new(snap),
                    };
                    if write_msg(&mut writer, &push).is_err() {
                        break;
                    }
                }
                ClientMessage::GetSettings { scope, id } => {
                    let page = SettingsPage {
                        scope,
                        sections: Vec::new(),
                    };
                    let resp =
                        ServerMessage::ok_with_payload(id, serde_json::to_value(page).unwrap());
                    if write_msg(&mut writer, &resp).is_err() {
                        break;
                    }
                }
                ClientMessage::Refresh { provider, id } => {
                    refresh_requests
                        .lock()
                        .unwrap()
                        .push(provider.as_ref().map(|p| p.as_str().to_string()));
                    let resp = ServerMessage::ok(id);
                    if write_msg(&mut writer, &resp).is_err() {
                        break;
                    }
                }
                _ => {
                    let resp = ServerMessage::ok(None);
                    if write_msg(&mut writer, &resp).is_err() {
                        break;
                    }
                }
            }
        }
    }

    fn write_msg(writer: &mut UnixStream, msg: &ServerMessage) -> std::io::Result<()> {
        let s = serde_json::to_string(msg).expect("serialize ServerMessage") + "\n";
        writer.write_all(s.as_bytes())?;
        writer.flush()
    }

    fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if pred() {
                return true;
            }
            pump_main_run_loop(Duration::from_millis(15));
        }
        pred()
    }

    fn drain_events(
        events: &Arc<Mutex<Vec<EngineEvent>>>,
        menu: &MenuController,
        state: &mut (Option<EngineSnapshot>, bool, Option<SwitcherSelection>),
    ) {
        let pending: Vec<EngineEvent> = events.lock().unwrap().drain(..).collect();
        for ev in pending {
            match ev {
                EngineEvent::Connected { .. } => {
                    state.1 = true;
                    if let Some(sel) = menu.update(state.0.as_ref(), state.1, state.2.as_ref()) {
                        state.2 = Some(sel);
                    }
                }
                EngineEvent::Snapshot(snap) => {
                    state.1 = true;
                    state.0 = Some(*snap);
                    if let Some(sel) = menu.update(state.0.as_ref(), state.1, state.2.as_ref()) {
                        state.2 = Some(sel);
                    }
                }
                EngineEvent::Settings(_) => {
                    state.1 = true;
                }
                EngineEvent::Disconnected => {
                    state.1 = false;
                    let _ = menu.update(state.0.as_ref(), state.1, state.2.as_ref());
                }
            }
        }
    }

    fn test_live_ipc_snapshot_push_updates_open_menu_in_place_without_recreating_views(
        mtm: MainThreadMarker,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let sock_path = dir.path().join("live-menu.sock");
        let snap1 = sample_snapshot();
        let server = MockDaemonServer::bind(&sock_path, snap1.clone());

        let events: Arc<Mutex<Vec<EngineEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);
        let client = EngineClient::spawn(sock_path.clone(), move |ev| {
            events_clone.lock().unwrap().push(ev);
        })
        .expect("spawn EngineClient");

        let menu = MenuController::new_with_callbacks(mtm, client, || {}, |_| {}, |_| {});
        let mut state: (Option<EngineSnapshot>, bool, Option<SwitcherSelection>) = (
            None,
            false,
            Some(SwitcherSelection::Provider("claude".to_string())),
        );

        assert!(
            wait_until(Duration::from_secs(2), || {
                drain_events(&events, &menu, &mut state);
                state.0.is_some() && state.1
            }),
            "client should connect and receive initial snapshot"
        );

        menu.simulate_will_open();
        menu.update(state.0.as_ref(), state.1, state.2.as_ref());
        let initial_rebuilds = menu.body_rebuild_count();
        let initial_in_place = menu.in_place_update_count();
        let initial_middle_reconciles = menu.middle_reconcile_count();
        let initial_item_count = menu.menu().numberOfItems();
        let card_ptr = Retained::as_ptr(&menu.card_views()[0]);
        let history_ptr = Retained::as_ptr(&menu.history_items()[0]);

        let mut snap2 = snap1.clone();
        snap2.generated_at = Timestamp::from_second(snap1.generated_at.as_second() + 30).unwrap();
        let claude = snap2
            .providers
            .iter_mut()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        if let Some(primary) = claude.windows.primary.as_mut()
            && let Some(val) = primary.value.as_mut()
        {
            val.used_percent = 82.0;
        }
        server.push_snapshot(snap2);

        assert!(
            wait_until(Duration::from_secs(2), || {
                drain_events(&events, &menu, &mut state);
                menu.in_place_update_count() > initial_in_place
            }),
            "live snapshot push while open must update menu in place"
        );
        assert_eq!(menu.body_rebuild_count(), initial_rebuilds);
        assert_eq!(menu.middle_reconcile_count(), initial_middle_reconciles);
        assert_eq!(menu.menu().numberOfItems(), initial_item_count);
        assert_eq!(Retained::as_ptr(&menu.card_views()[0]), card_ptr);
        assert_eq!(Retained::as_ptr(&menu.history_items()[0]), history_ptr);
        assert_eq!(
            menu.card_views()[0].model().unwrap().metrics[0].percent_label(),
            "18% left"
        );

        server.shutdown();
    }

    fn test_refresh_action_sends_selected_provider_on_provider_tab_and_none_on_overview(
        mtm: MainThreadMarker,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let sock_path = dir.path().join("refresh-intent.sock");
        let snap = sample_snapshot();
        let server = MockDaemonServer::bind(&sock_path, snap.clone());

        let events: Arc<Mutex<Vec<EngineEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);
        let client = EngineClient::spawn(sock_path.clone(), move |ev| {
            events_clone.lock().unwrap().push(ev);
        })
        .expect("spawn EngineClient");

        let menu = Rc::new(MenuController::new_with_callbacks(
            mtm,
            client,
            || {},
            |_| {},
            |_| {},
        ));
        let callback_log: Rc<RefCell<Vec<Option<String>>>> = Rc::new(RefCell::new(Vec::new()));
        let cb_clone = callback_log.clone();
        let menu_weak = Rc::downgrade(&menu);
        menu.set_on_refresh(move |provider_id| {
            cb_clone.borrow_mut().push(provider_id.clone());
            if let Some(m) = menu_weak.upgrade() {
                m.trigger_refresh(provider_id);
            }
        });

        let mut state: (Option<EngineSnapshot>, bool, Option<SwitcherSelection>) = (
            None,
            false,
            Some(SwitcherSelection::Provider("claude".to_string())),
        );
        assert!(wait_until(Duration::from_secs(2), || {
            drain_events(&events, &menu, &mut state);
            state.0.is_some()
        }));

        // 1. On the Claude provider tab, clicking Refresh (`invoke_refresh_action`) must pass `Some("claude")`
        // and send `ClientMessage::Refresh { provider: Some("claude") }`.
        assert_eq!(menu.current_refresh_provider().as_deref(), Some("claude"));
        menu.invoke_refresh_action();
        assert_eq!(*callback_log.borrow(), vec![Some("claude".to_string())]);
        assert!(
            wait_until(Duration::from_secs(2), || {
                server.refresh_requests() == vec![Some("claude".to_string())]
            }),
            "server should receive per-provider refresh request Some(\"claude\")"
        );

        // 2. Switch to Overview tab: clicking Refresh must pass `None` and send `ClientMessage::Refresh { provider: None }`.
        menu.update(Some(&snap), true, Some(&SwitcherSelection::Overview));
        assert_eq!(menu.current_refresh_provider(), None);
        menu.invoke_refresh_action();
        assert_eq!(
            *callback_log.borrow(),
            vec![Some("claude".to_string()), None]
        );
        assert!(
            wait_until(Duration::from_secs(2), || {
                server.refresh_requests() == vec![Some("claude".to_string()), None]
            }),
            "server should receive global refresh request None on Overview"
        );

        server.shutdown();
    }

    fn test_menu_open_schedules_delayed_refresh_and_cancels_on_close(mtm: MainThreadMarker) {
        let (_dir, menu) = make_controller(mtm);
        menu.set_open_refresh_delay(Duration::from_millis(40));

        let fired: Rc<RefCell<Vec<Option<String>>>> = Rc::new(RefCell::new(Vec::new()));
        let fired_cb = fired.clone();
        menu.set_on_refresh(move |provider_id| {
            fired_cb.borrow_mut().push(provider_id);
        });

        let mut snap = sample_snapshot();
        let claude = snap
            .providers
            .iter_mut()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        if let Some(primary) = claude.windows.primary.as_mut() {
            primary.state = MetricState::Stale;
        }
        let claude_tab = SwitcherSelection::Provider("claude".to_string());
        menu.update(Some(&snap), true, Some(&claude_tab));

        // 1. Open and immediately close before the delay expires: scheduled refresh must be cancelled.
        menu.simulate_will_open();
        menu.simulate_did_close();
        pump_main_run_loop(Duration::from_millis(80));
        assert!(
            fired.borrow().is_empty(),
            "closing the menu before open_refresh_delay must cancel MenuOpenRefreshPlan"
        );

        // 2. Open and keep open across the delay: scheduled refresh must fire for stale provider `"claude"`.
        menu.simulate_will_open();
        pump_main_run_loop(Duration::from_millis(80));
        assert_eq!(
            *fired.borrow(),
            vec![Some("claude".to_string())],
            "keeping the menu open past open_refresh_delay must trigger refresh for stale provider"
        );
        menu.simulate_did_close();
    }

    fn test_disconnect_and_reconnect_over_unix_socket_with_status_icon_loading_api(
        mtm: MainThreadMarker,
    ) {
        // Verify public status icon loading API (`set_loading`, `set_provider_loading`, `set_snapshot`)
        // is safe and non-panicking before and during live menu updates.
        set_loading(true);
        set_provider_loading("claude", true);
        set_snapshot(Some(sample_snapshot()));
        set_provider_loading("claude", false);
        set_loading(false);

        let dir = tempfile::tempdir().expect("tempdir");
        let sock_path = dir.path().join("reconnect.sock");
        let snap = sample_snapshot();
        let server = MockDaemonServer::bind(&sock_path, snap.clone());

        let events: Arc<Mutex<Vec<EngineEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);
        let client = EngineClient::spawn(sock_path.clone(), move |ev| {
            events_clone.lock().unwrap().push(ev);
        })
        .expect("spawn EngineClient");

        let menu = MenuController::new_with_callbacks(mtm, client, || {}, |_| {}, |_| {});
        let mut state: (Option<EngineSnapshot>, bool, Option<SwitcherSelection>) =
            (None, false, Some(SwitcherSelection::Overview));

        assert!(wait_until(Duration::from_secs(2), || {
            drain_events(&events, &menu, &mut state);
            state.1 && state.0.is_some()
        }));

        // Drop server connection -> client emits Disconnected.
        server.shutdown();
        assert!(
            wait_until(Duration::from_secs(2), || {
                drain_events(&events, &menu, &mut state);
                !state.1
            }),
            "client should transition to Disconnected when daemon stops"
        );
        assert!(matches!(
            menu.last_structure().unwrap().primary,
            PrimaryContent::Disconnected { .. }
        ));

        // Restart server on the same socket path -> client reconnects automatically (`RECONNECT_BACKOFF`).
        let server2 = MockDaemonServer::bind(&sock_path, snap);
        assert!(
            wait_until(Duration::from_secs(3), || {
                drain_events(&events, &menu, &mut state);
                state.1
            }),
            "client should automatically reconnect when daemon restarts"
        );
        assert!(matches!(
            menu.last_structure().unwrap().primary,
            PrimaryContent::Overview { .. }
        ));
        server2.shutdown();
    }

    fn render_card_png(mtm: MainThreadMarker, model: &card::CardModel) -> Vec<u8> {
        let card = CardView::new(mtm);
        card.update(model, 310.0);
        let rep = card
            .render_bitmap_with_appearance(false)
            .expect("render card bitmap");
        png_bytes(&rep).expect("encode card bitmap as PNG")
    }

    fn write_live_menu_captures(mtm: MainThreadMarker, capture_dir: &Path) {
        let _ = std::fs::create_dir_all(capture_dir);
        let dates = FoundationDateTexts::new();
        let prefs = CardPreferences::default();
        let snap1 = sample_snapshot();
        let now = snap1.generated_at;

        let claude = snap1
            .providers
            .iter()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        let model_before = card::provider_card(claude, &prefs, now, &dates);
        let png_before = render_card_png(mtm, &model_before);
        std::fs::write(
            capture_dir.join("live-menu-card-before-push.png"),
            png_before,
        )
        .expect("write before png");

        let mut monitor = MenuCardRefreshMonitor::new();
        monitor.begin_manual_refresh(
            ManualRefreshScope::Provider("claude".to_string()),
            Some(&snap1),
            &prefs,
            now,
            &dates,
        );
        let model_refreshing = monitor.effective_card_model("claude", model_before, true);
        let png_refreshing = render_card_png(mtm, &model_refreshing);
        std::fs::write(
            capture_dir.join("live-menu-card-refreshing.png"),
            png_refreshing,
        )
        .expect("write refreshing png");

        let mut snap2 = snap1.clone();
        let claude2 = snap2
            .providers
            .iter_mut()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        if let Some(primary) = claude2.windows.primary.as_mut()
            && let Some(val) = primary.value.as_mut()
        {
            val.used_percent = 82.0;
        }
        let model_after = card::provider_card(claude2, &prefs, now, &dates);
        let png_after = render_card_png(mtm, &model_after);
        std::fs::write(capture_dir.join("live-menu-card-after-push.png"), png_after)
            .expect("write after png");
    }
}
