// Ported from CodexBar Sources/CodexBar/StatusItemController+StatusItemVending.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+Animation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! AppKit bridge: the application, its status item and its menu.
//!
//! Everything here runs on the main thread. Engine events arrive on the
//! connection thread and are forwarded to the main queue, where the single
//! [`AppState`] lives in a thread-local.

#![allow(unsafe_code)]

mod card_view;
mod chart_view;
mod color;
mod date_texts;
mod icon_image;
mod menu;
mod progress_bar_view;
mod settings_view;
mod settings_window;
mod switcher_view;

pub use card_view::{CardView, png_bytes};
pub use chart_view::{CostHistoryChartView, PlanUtilizationChartView, QuotaBurndownChartView};
pub use date_texts::FoundationDateTexts;
pub use icon_image::{
    image_png_bytes, quota_warning_flash_image, render as render_icon,
    render_morph as render_morph_icon, render_png_for_appearance, tint_image,
};
pub use menu::MenuController;
pub use settings_view::SettingsView;
pub use settings_window::SettingsWindowController;
pub use switcher_view::SwitcherView;

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use ariadusage_protocol::snapshot::EngineSnapshot;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSAppearance, NSApplication, NSApplicationActivationPolicy, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString, NSTimer};

use crate::card::CardPreferences;
use crate::engine_client::{EngineClient, EngineEvent};
use crate::icon;
use crate::presentation::{self, IconPreferences};
use crate::settings::{SettingsPane, SettingsState};
use crate::switcher::SwitcherSelection;

/// Autosave name of the merged status item, so macOS keeps its position.
const STATUS_ITEM_AUTOSAVE_NAME: &str = "ariadusage-merged";
const STATUS_ITEM_ACCESSIBILITY_TITLE: &str = "AriadUsage";
const STATUS_ITEM_ACCESSIBILITY_IDENTIFIER: &str = "AriadUsage.StatusItem";

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements beyond `init`, and
    // `StatusItemTimerTarget` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsageStatusItemTimerTarget"]
    struct StatusItemTimerTarget;

    unsafe impl NSObjectProtocol for StatusItemTimerTarget {}

    impl StatusItemTimerTarget {
        #[unsafe(method(loadingTimerFired:))]
        fn loading_timer_fired(&self, _timer: &NSTimer) {
            on_loading_timer_tick();
        }

        #[unsafe(method(blinkTimerFired:))]
        fn blink_timer_fired(&self, _timer: &NSTimer) {
            on_blink_timer_tick();
        }

        #[unsafe(method(flashTimerFired:))]
        fn flash_timer_fired(&self, _timer: &NSTimer) {
            on_flash_timer_tick();
        }
    }
);

impl StatusItemTimerTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: `init` is `NSObject`'s designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

struct AppState {
    status_item: Retained<NSStatusItem>,
    timer_target: Retained<StatusItemTimerTarget>,
    loading_timer: Option<Retained<NSTimer>>,
    blink_timer: Option<Retained<NSTimer>>,
    flash_timer: Option<Retained<NSTimer>>,
    menu: menu::MenuController,
    settings_window: SettingsWindowController,
    snapshot: Option<EngineSnapshot>,
    settings_state: SettingsState,
    connected: bool,
    global_loading: bool,
    provider_loading: HashSet<String>,
    selected_tab: Option<SwitcherSelection>,
    last_provider_id: Option<String>,
    prefs: IconPreferences,
    animation: icon::StatusItemAnimationState,
    loading_pattern: icon::LoadingPattern,
    last_render_signature: Option<u64>,
}

thread_local! {
    static STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

fn now_secs() -> f64 {
    jiff::Timestamp::now().as_millisecond() as f64 / 1000.0
}

/// Starts the app and runs the AppKit event loop until the user quits.
pub fn run(socket: PathBuf) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("the app must start on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    if let Ok(appearance) = std::env::var("ARIADUSAGE_APPEARANCE") {
        let name = match appearance.as_str() {
            "light" => Some("NSAppearanceNameAqua"),
            "dark" => Some("NSAppearanceNameDarkAqua"),
            _ => None,
        };
        if let Some(name) = name {
            let app_appearance = NSAppearance::appearanceNamed(&NSString::from_str(name));
            app.setAppearance(app_appearance.as_deref());
        }
    }
    // A menu-bar app has no Dock icon and no main window.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let client = EngineClient::spawn(socket, |event| {
        DispatchQueue::main().exec_async(move || handle_event(event));
    })
    .map_err(|err| format!("could not start the engine connection: {err}"))?;

    let status_item = make_status_item(mtm);
    let timer_target = StatusItemTimerTarget::new(mtm);
    let settings_window = SettingsWindowController::new(mtm, Some(client.clone()));
    settings_window
        .view()
        .set_on_setting_changed(Rc::new(|setting_id, value| {
            STATE.with(|cell| {
                let mut guard = cell.borrow_mut();
                let Some(state) = guard.as_mut() else { return };
                state.settings_state.set_local(setting_id.as_str(), value);
                sync_state_from_settings(state);
            });
            refresh_ui();
        }));
    let menu = menu::MenuController::new(mtm, client, refresh_ui, select_tab, open_settings_window);
    menu.set_on_refresh(|provider_id| {
        trigger_user_refresh(provider_id);
    });
    status_item.setMenu(Some(menu.menu()));
    STATE.with(|cell| {
        let settings_state = SettingsState::default();
        let prefs = IconPreferences {
            show_used: settings_state.menu_bar_show_used,
            hide_critters: settings_state.menu_bar_icon_style == "bars",
            ..IconPreferences::default()
        };
        *cell.borrow_mut() = Some(AppState {
            status_item,
            timer_target,
            loading_timer: None,
            blink_timer: None,
            flash_timer: None,
            menu,
            settings_window,
            snapshot: None,
            settings_state,
            connected: false,
            global_loading: false,
            provider_loading: HashSet::new(),
            selected_tab: None,
            last_provider_id: None,
            prefs,
            animation: icon::StatusItemAnimationState::default(),
            loading_pattern: icon::LoadingPattern::KnightRider,
            last_render_signature: None,
        });
    });
    refresh_ui();

    app.run();
    Ok(())
}

/// Public icon-state API: sets or clears the global refresh-in-progress state
/// (`StatusItemController.updateAnimationState`) and refreshes the status item icon.
pub fn set_loading(loading: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return;
        };
        let Some(state) = guard.as_mut() else { return };
        state.global_loading = loading;
        sync_animation_timers(state);
        render_status_item_icon(mtm, state);
    });
}

/// Public icon-state API: sets or clears the per-provider refresh-in-progress state
/// (`UsageStore.refreshingProviders`) and refreshes the status item icon.
pub fn set_provider_loading(provider_id: &str, loading: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return;
        };
        let Some(state) = guard.as_mut() else { return };
        if loading {
            state.provider_loading.insert(provider_id.to_string());
        } else {
            state.provider_loading.remove(provider_id);
        }
        sync_animation_timers(state);
        render_status_item_icon(mtm, state);
    });
}

/// Public icon-state API: applies a new engine snapshot, records downward quota
/// warning threshold crossings for the 60 s flash overlay, and refreshes the UI.
pub fn set_snapshot(snapshot: Option<EngineSnapshot>) {
    let updated = STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return false;
        };
        let Some(state) = guard.as_mut() else {
            return false;
        };
        if let Some(ref snap) = snapshot {
            state.global_loading = snap.engine.refreshing;
            record_snapshot_quota_warnings(state, snap);
            state.settings_window.apply_snapshot(snap.clone());
        }
        state.snapshot = snapshot;
        true
    });
    if updated {
        refresh_ui();
    }
}

/// Public icon-state API: triggers the 60 s quota-warning red flash overlay on
/// `provider_id` (`StatusItemController.startQuotaWarningFlash`).
pub fn trigger_quota_warning_flash(provider_id: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return;
        };
        let Some(state) = guard.as_mut() else { return };
        let _ = state
            .animation
            .start_quota_warning_flash(provider_id, now_secs());
        state.last_render_signature = None;
        sync_animation_timers(state);
        render_status_item_icon(mtm, state);
    });
}

fn sync_state_from_settings(state: &mut AppState) {
    state.prefs.show_used = state.settings_state.menu_bar_show_used;
    state.prefs.hide_critters = state.settings_state.menu_bar_icon_style == "bars";
    state.last_render_signature = None;
    let card_prefs = CardPreferences {
        show_used: state.settings_state.menu_bar_show_used,
        show_optional_credits_and_extra_usage: state.settings_state.show_optional_credits,
        ..CardPreferences::default()
    };
    state.menu.set_prefs(card_prefs);
}

fn open_settings_window(pane: Option<SettingsPane>) {
    STATE.with(|cell| {
        let guard = cell.borrow();
        let Some(state) = guard.as_ref() else { return };
        state.settings_window.show_window(pane);
    });
}

fn make_status_item(mtm: MainThreadMarker) -> Retained<NSStatusItem> {
    let bar = NSStatusBar::systemStatusBar();
    // Create at zero length and set the autosave name before sizing, so the
    // saved position applies before the item first appears.
    let item = bar.statusItemWithLength(0.0);
    item.setAutosaveName(Some(&NSString::from_str(STATUS_ITEM_AUTOSAVE_NAME)));
    item.setLength(NSVariableStatusItemLength);
    if let Some(button) = item.button(mtm) {
        button.setImageScaling(objc2_app_kit::NSImageScaling::ScaleNone);
        button.setAccessibilityIdentifier(Some(&NSString::from_str(
            STATUS_ITEM_ACCESSIBILITY_IDENTIFIER,
        )));
        button.setAccessibilityTitle(Some(&NSString::from_str(STATUS_ITEM_ACCESSIBILITY_TITLE)));
    }
    item
}

/// Updates quota-warning flash state across enabled providers.
///
/// Uses [`icon::DEFAULT_QUOTA_WARNING_THRESHOLDS`] (`[50, 20]`) until user-configured
/// warning thresholds are exposed on the engine snapshot/settings.
fn record_snapshot_quota_warnings(state: &mut AppState, snapshot: &EngineSnapshot) {
    let now = now_secs();
    for provider in &snapshot.providers {
        if !provider.enabled {
            continue;
        }
        let (primary, secondary) =
            presentation::resolved_icon_windows(provider, None, Some(snapshot.generated_at));
        let p_rem = primary
            .as_ref()
            .map(|w| (100.0 - w.used_percent).clamp(0.0, 100.0));
        let p_reset = primary
            .as_ref()
            .and_then(|w| w.resets_at.map(|ts| ts.as_second()));
        let s_rem = secondary
            .as_ref()
            .map(|w| (100.0 - w.used_percent).clamp(0.0, 100.0));
        let s_reset = secondary
            .as_ref()
            .and_then(|w| w.resets_at.map(|ts| ts.as_second()));
        if state.animation.quota_warning_tracker.update_provider(
            provider.id.as_str(),
            p_rem,
            p_reset,
            s_rem,
            s_reset,
            &icon::DEFAULT_QUOTA_WARNING_THRESHOLDS,
            true,
        ) {
            let _ = state
                .animation
                .start_quota_warning_flash(provider.id.as_str(), now);
            state.last_render_signature = None;
        }
    }
}

fn handle_event(event: EngineEvent) {
    match event {
        EngineEvent::Connected { .. } => {
            STATE.with(|cell| {
                let Ok(mut guard) = cell.try_borrow_mut() else {
                    return;
                };
                let Some(state) = guard.as_mut() else { return };
                state.connected = true;
            });
            refresh_ui();
        }
        EngineEvent::Snapshot(snapshot) => {
            STATE.with(|cell| {
                let Ok(mut guard) = cell.try_borrow_mut() else {
                    return;
                };
                let Some(state) = guard.as_mut() else { return };
                state.connected = true;
            });
            set_snapshot(Some(*snapshot));
            let finished: Vec<String> = STATE.with(|cell| {
                let Ok(guard) = cell.try_borrow() else {
                    return Vec::new();
                };
                let Some(state) = guard.as_ref() else {
                    return Vec::new();
                };
                let active = state.menu.refreshing_providers(state.snapshot.as_ref());
                state
                    .provider_loading
                    .iter()
                    .filter(|id| !active.contains(*id))
                    .cloned()
                    .collect()
            });
            for id in finished {
                set_provider_loading(&id, false);
            }
        }
        EngineEvent::Settings(page) => {
            STATE.with(|cell| {
                let Ok(mut guard) = cell.try_borrow_mut() else {
                    return;
                };
                let Some(state) = guard.as_mut() else { return };
                state.connected = true;
                state.settings_state.apply_page(&page);
                sync_state_from_settings(state);
                state.settings_window.apply_settings_page(&page);
            });
            refresh_ui();
        }
        EngineEvent::Disconnected => {
            STATE.with(|cell| {
                let Ok(mut guard) = cell.try_borrow_mut() else {
                    return;
                };
                let Some(state) = guard.as_mut() else { return };
                state.connected = false;
                state.global_loading = false;
                state.provider_loading.clear();
                state.last_render_signature = None;
            });
            refresh_ui();
        }
    }
}

fn select_tab(selection: SwitcherSelection) {
    STATE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(state) = guard.as_mut() else { return };
        if let SwitcherSelection::Provider(id) = &selection {
            state.last_provider_id = Some(id.clone());
        }
        state.selected_tab = Some(selection);
    });
    refresh_ui();
}

fn trigger_user_refresh(provider_id: Option<String>) {
    match &provider_id {
        Some(id) => set_provider_loading(id, true),
        None => set_loading(true),
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return;
        };
        let Some(state) = guard.as_mut() else { return };
        let resolved = state.menu.start_user_refresh(
            provider_id,
            state.snapshot.as_ref(),
            state.connected,
            state.selected_tab.as_ref(),
        );
        if let Some(selection) = resolved {
            if let SwitcherSelection::Provider(id) = &selection {
                state.last_provider_id = Some(id.clone());
            }
            state.selected_tab = Some(selection);
        }
        sync_animation_timers(state);
        render_status_item_icon(mtm, state);
    });
}

fn provider_needs_loading_animation(state: &AppState, provider_id: Option<&str>) -> bool {
    if state.global_loading || state.snapshot.as_ref().is_some_and(|s| s.engine.refreshing) {
        return true;
    }
    let Some(id) = provider_id else {
        return false;
    };
    if state.provider_loading.contains(id) {
        return true;
    }
    let Some(snapshot) = state.snapshot.as_ref() else {
        return false;
    };
    let Some(provider) = snapshot.providers.iter().find(|p| p.id.as_str() == id) else {
        return false;
    };
    presentation::should_animate_provider(provider, state.animation.debug_force_animation)
}

fn sync_animation_timers(state: &mut AppState) {
    let now = now_secs();
    let selected_provider_id = match state.selected_tab.as_ref() {
        Some(SwitcherSelection::Provider(id)) => Some(id.as_str()),
        _ => state.last_provider_id.as_deref(),
    };
    let target_provider = state
        .snapshot
        .as_ref()
        .and_then(|s| presentation::icon_provider(s, selected_provider_id));
    let target_id = target_provider.map(|p| p.id.as_str());

    let needs_loading = provider_needs_loading_animation(state, target_id);
    let any_enabled_loading = state.global_loading
        || !state.provider_loading.is_empty()
        || state.snapshot.as_ref().is_some_and(|s| {
            presentation::any_enabled_provider_needs_loading_animation(
                s,
                state.animation.debug_force_animation,
            )
        });
    let loading_active = state.animation.sync_loading_animation_with_any_loading(
        needs_loading,
        any_enabled_loading,
        now,
    );

    if loading_active {
        if state.loading_timer.is_none() {
            let interval = 1.0 / icon::LOADING_ANIMATION_FPS;
            // SAFETY: `timer_target` is a valid `NSObject` handling `loadingTimerFired:`.
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    interval,
                    &state.timer_target,
                    sel!(loadingTimerFired:),
                    None,
                    true,
                )
            };
            state.loading_timer = Some(timer);
        }
    } else if let Some(timer) = state.loading_timer.take() {
        timer.invalidate();
    }

    let providers_vec: Vec<String> = target_id.map(|id| vec![id.to_string()]).unwrap_or_default();
    for id in &providers_vec {
        state
            .animation
            .blink_states
            .entry(id.clone())
            .or_insert_with(|| icon::BlinkState::seeded(now, 1));
    }
    let has_blinking_providers = target_id.is_some_and(|id| {
        icon::style_supports_blink(
            icon::IconStyle::from_provider_id(id),
            state.prefs.hide_critters,
        )
    });
    let blinking_allowed = icon::is_blinking_allowed(
        state.settings_state.random_blink_enabled,
        state.animation.blink_force_until_secs,
        now,
    );
    let should_blink = icon::should_run_blink_task(
        blinking_allowed,
        loading_active,
        state.prefs.hide_critters,
        has_blinking_providers,
    );
    if should_blink {
        if state.blink_timer.is_none() {
            let delay_secs = (icon::blink_tick_sleep_duration_ms(
                &state.animation.blink_states,
                &providers_vec,
                now,
            ) as f64)
                / 1000.0;
            // SAFETY: `timer_target` is a valid `NSObject` handling `blinkTimerFired:`.
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    delay_secs,
                    &state.timer_target,
                    sel!(blinkTimerFired:),
                    None,
                    false,
                )
            };
            state.blink_timer = Some(timer);
        }
    } else {
        if let Some(timer) = state.blink_timer.take() {
            timer.invalidate();
        }
        state.animation.clear_motion();
    }

    let _ = state.animation.clear_expired_quota_warning_flashes(now);
    let any_flash_active =
        target_id.is_some_and(|id| state.animation.is_quota_warning_flash_active(id, now));
    if any_flash_active {
        if state.flash_timer.is_none() {
            // SAFETY: `timer_target` is a valid `NSObject` handling `flashTimerFired:`.
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    icon::QUOTA_WARNING_FLASH_DURATION_SECS,
                    &state.timer_target,
                    sel!(flashTimerFired:),
                    None,
                    false,
                )
            };
            state.flash_timer = Some(timer);
        }
    } else if let Some(timer) = state.flash_timer.take() {
        timer.invalidate();
    }
}

fn on_loading_timer_tick() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    STATE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(state) = guard.as_mut() else { return };
        let now = now_secs();
        let selected_provider_id = match state.selected_tab.as_ref() {
            Some(SwitcherSelection::Provider(id)) => Some(id.as_str()),
            _ => state.last_provider_id.as_deref(),
        };
        let target_id = state
            .snapshot
            .as_ref()
            .and_then(|s| presentation::icon_provider(s, selected_provider_id))
            .map(|p| p.id.as_str());
        let needs_loading = provider_needs_loading_animation(state, target_id);
        if !state.animation.advance_loading_frame(needs_loading, now) {
            sync_animation_timers(state);
        }
        render_status_item_icon(mtm, state);
    });
}

fn on_blink_timer_tick() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    STATE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(state) = guard.as_mut() else { return };
        state.blink_timer = None;
        let now = now_secs();
        let selected_provider_id = match state.selected_tab.as_ref() {
            Some(SwitcherSelection::Provider(id)) => Some(id.as_str()),
            _ => state.last_provider_id.as_deref(),
        };
        let target_id = state
            .snapshot
            .as_ref()
            .and_then(|s| presentation::icon_provider(s, selected_provider_id))
            .map(|p| p.id.as_str())
            .map(str::to_string);
        if let Some(id) = target_id {
            let _ = state.animation.tick_blink(
                &[id],
                state.settings_state.random_blink_enabled,
                state.prefs.hide_critters,
                now,
            );
        }
        sync_animation_timers(state);
        render_status_item_icon(mtm, state);
    });
}

fn on_flash_timer_tick() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    STATE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(state) = guard.as_mut() else { return };
        state.flash_timer = None;
        let _ = state
            .animation
            .clear_expired_quota_warning_flashes(now_secs());
        state.last_render_signature = None;
        sync_animation_timers(state);
        render_status_item_icon(mtm, state);
    });
}

fn render_status_item_icon(mtm: MainThreadMarker, state: &mut AppState) {
    let now = now_secs();
    let selected_provider_id = match state.selected_tab.as_ref() {
        Some(SwitcherSelection::Provider(id)) => Some(id.as_str()),
        _ => state.last_provider_id.as_deref(),
    };
    let merged = state.settings_state.menu_bar_merge_icons;
    let (provider, mut input) = presentation::status_item_icon_input(
        state.snapshot.as_ref(),
        selected_provider_id,
        merged,
        state.prefs,
    );
    if !state.connected && state.snapshot.is_some() {
        input.stale = true;
    }
    let provider_id = provider.map(|p| p.id.as_str());
    let warning_flash =
        provider_id.is_some_and(|id| state.animation.is_quota_warning_flash_active(id, now));
    let needs_loading = provider_needs_loading_animation(state, provider_id);
    let phase = state.animation.active_loading_phase(needs_loading);

    let effective_pattern = state
        .loading_pattern
        .effective_for(input.style, merged, provider_id);

    if let Some(phase_val) = phase {
        if effective_pattern == icon::LoadingPattern::Unbraid {
            let signature = icon::icon_render_signature(
                &input,
                state.prefs.show_used,
                false,
                Some(phase_val),
                effective_pattern,
                warning_flash,
            );
            if state.last_render_signature == Some(signature) {
                return;
            }
            state.last_render_signature = Some(signature);
            let morph_progress = effective_pattern.value(phase_val) / 100.0;
            if let Some(button) = state.status_item.button(mtm)
                && let Some(base) = icon_image::render_morph(&icon::morph_layout(
                    morph_progress,
                    input.style,
                    input.hide_critters,
                ))
            {
                let image = if warning_flash {
                    icon_image::quota_warning_flash_image(&base).unwrap_or(base)
                } else {
                    base
                };
                button.setImage(Some(&image));
            }
            return;
        }
        let (primary, secondary) = icon::loading_frame_percents(effective_pattern, phase_val);
        input.primary = Some(primary);
        input.secondary = Some(secondary);
        input.credits_remaining = None;
        input.stale = false;
    }

    let (blink, wiggle, tilt) =
        provider_id.map_or((0.0, 0.0, 0.0), |id| state.animation.motion_for(id));
    if merged && input.style == icon::IconStyle::Combined {
        input.blink = 0.0;
        input.wiggle = 0.0;
        input.tilt = 0.0;
    } else {
        input.blink = if merged {
            blink
        } else {
            icon::loading_blink_amount(input.style, phase, blink)
        };
        input.wiggle = wiggle;
        input.tilt = tilt;
    }

    let signature = icon::icon_render_signature(
        &input,
        state.prefs.show_used,
        false,
        phase,
        effective_pattern,
        warning_flash,
    );
    if state.last_render_signature == Some(signature) {
        return;
    }
    state.last_render_signature = Some(signature);

    if let Some(button) = state.status_item.button(mtm)
        && let Some(base) = icon_image::render(&icon::layout(&input))
    {
        let image = if warning_flash {
            icon_image::quota_warning_flash_image(&base).unwrap_or(base)
        } else {
            base
        };
        button.setImage(Some(&image));
    }
}

/// Redraws the icon and updates the menu from the current state.
fn refresh_ui() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    STATE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(state) = guard.as_mut() else { return };
        let resolved = state.menu.update(
            state.snapshot.as_ref(),
            state.connected,
            state.selected_tab.as_ref(),
        );
        if let Some(selection) = resolved {
            if let SwitcherSelection::Provider(id) = &selection {
                state.last_provider_id = Some(id.clone());
            }
            state.selected_tab = Some(selection);
        }
        sync_animation_timers(state);
        render_status_item_icon(mtm, state);
    });
}
