// Ported from CodexBar Sources/CodexBar/StatusItemController+StatusItemVending.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+Animation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/ProviderBrandIcon.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/AdaptiveRefreshPolicy.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/SettingsStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

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
mod notifications;
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
pub use notifications::{
    MacOsNotice, RecordingUserNotificationBackend, UNUserNotificationCenterBackend,
    UserNotificationBackend, has_app_bundle_identity,
};
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
use objc2::{
    AllocAnyThread, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAccessibility, NSAppearance, NSApplication, NSApplicationActivationPolicy, NSBezierPath,
    NSBitmapImageRep, NSCellImagePosition, NSColor, NSCompositingOperation, NSDeviceRGBColorSpace,
    NSGraphicsContext, NSImage, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
    NSWindingRule,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSTimer};

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

/// Maps a boolean low-power flag to its IPC action ID.
pub const fn low_power_mode_action_id(enabled: bool) -> &'static str {
    if enabled {
        "setLowPowerMode.on"
    } else {
        "setLowPowerMode.off"
    }
}

/// Maps `NSProcessInfoThermalState` (`0 = nominal`, `1 = fair`, `2 = serious`, `3 = critical`)
/// to its IPC action ID (`AdaptiveRefreshSignalCollector.currentThermalPressure`,
/// `AdaptiveRefreshPolicy.swift:31-37`).
pub const fn thermal_pressure_action_id(raw_state: isize) -> &'static str {
    match raw_state {
        0 | 1 => "setThermalPressure.nominal",
        _ => "setThermalPressure.constrained",
    }
}

/// Computes which IPC actions (`setLowPowerMode.on`/`off`, `setThermalPressure.nominal`/`constrained`)
/// need to be forwarded when the sampled macOS power or thermal state changes, updating the cached
/// last-forwarded state in place.
pub fn power_and_thermal_actions_to_forward(
    last_low_power: &mut Option<bool>,
    last_thermal_action: &mut Option<&'static str>,
    low_power: bool,
    raw_thermal_state: isize,
) -> Vec<&'static str> {
    let mut actions = Vec::new();
    if *last_low_power != Some(low_power) {
        *last_low_power = Some(low_power);
        actions.push(low_power_mode_action_id(low_power));
    }
    let thermal_action = thermal_pressure_action_id(raw_thermal_state);
    if *last_thermal_action != Some(thermal_action) {
        *last_thermal_action = Some(thermal_action);
        actions.push(thermal_action);
    }
    actions
}

/// Reads `NSProcessInfo.processInfo.isLowPowerModeEnabled` (`SettingsStore.swift:771-790`,
/// `AdaptiveRefreshPolicy.swift:26`).
pub fn current_low_power_mode_enabled() -> bool {
    // SAFETY: `[NSProcessInfo processInfo]` is a thread-safe singleton accessor and
    // `isLowPowerModeEnabled` returns a `BOOL` without side effects.
    unsafe {
        let info: Retained<NSObject> = msg_send![objc2::class!(NSProcessInfo), processInfo];
        msg_send![&*info, isLowPowerModeEnabled]
    }
}

/// Reads `NSProcessInfo.processInfo.thermalState` (`AdaptiveRefreshPolicy.swift:34`).
pub fn current_thermal_state_raw() -> isize {
    // SAFETY: `[NSProcessInfo processInfo]` is a thread-safe singleton accessor and
    // `thermalState` returns an `NSInteger` (`NSProcessInfoThermalState`) without side effects.
    unsafe {
        let info: Retained<NSObject> = msg_send![objc2::class!(NSProcessInfo), processInfo];
        msg_send![&*info, thermalState]
    }
}

/// Forwards the current macOS low-power mode and thermal pressure state to the engine daemon
/// over IPC (`setLowPowerMode.on`/`off`, `setThermalPressure.nominal`/`constrained`).
pub fn forward_power_and_thermal_state(client: &EngineClient) {
    if let Ok(low_power_id) = ariadusage_protocol::ids::ActionId::new(low_power_mode_action_id(
        current_low_power_mode_enabled(),
    )) {
        client.run_action(low_power_id, None);
    }
    if let Ok(thermal_id) = ariadusage_protocol::ids::ActionId::new(thermal_pressure_action_id(
        current_thermal_state_raw(),
    )) {
        client.run_action(thermal_id, None);
    }
}

fn sync_power_and_thermal_state(state: &mut AppState) {
    if !state.connected {
        return;
    }
    for action in power_and_thermal_actions_to_forward(
        &mut state.last_forwarded_low_power,
        &mut state.last_forwarded_thermal_action,
        current_low_power_mode_enabled(),
        current_thermal_state_raw(),
    ) {
        if let Ok(action_id) = ariadusage_protocol::ids::ActionId::new(action) {
            state.client.run_action(action_id, None);
        }
    }
}

struct AppState {
    client: EngineClient,
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
    last_forwarded_low_power: Option<bool>,
    last_forwarded_thermal_action: Option<&'static str>,
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
    static NOTIFICATION_BACKEND: RefCell<Box<dyn UserNotificationBackend>> = RefCell::new(
        Box::new(UNUserNotificationCenterBackend::new())
    );
}

/// Replaces the active [`UserNotificationBackend`] on the main thread (for example, with
/// [`RecordingUserNotificationBackend`] in tests).
pub fn set_notification_backend(backend: Box<dyn UserNotificationBackend>) {
    NOTIFICATION_BACKEND.with(|cell| {
        if let Ok(mut guard) = cell.try_borrow_mut() {
            *guard = backend;
        }
    });
}

/// Delivers a [`MacOsNotice`] through the configured [`UserNotificationBackend`], re-dispatching
/// to the main queue on borrow contention.
pub fn deliver_notice(notice: MacOsNotice) {
    let contended = NOTIFICATION_BACKEND.with(|cell| {
        let Ok(guard) = cell.try_borrow() else {
            return true;
        };
        guard.post(&notice);
        false
    });
    if contended {
        DispatchQueue::main().exec_async(move || deliver_notice(notice));
    }
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
            apply_local_setting_change(setting_id.clone(), value);
        }));
    let menu = menu::MenuController::new(
        mtm,
        client.clone(),
        refresh_ui,
        select_tab,
        open_settings_window,
    );
    menu.set_on_refresh(|provider_id| {
        trigger_user_refresh(provider_id);
    });
    status_item.setMenu(Some(menu.menu()));
    STATE.with(|cell| {
        let settings_state = SettingsState::default();
        let icon_style =
            icon::MenuBarIconStyle::from_raw_value(&settings_state.menu_bar_icon_style)
                .unwrap_or_default();
        let prefs = IconPreferences {
            show_used: settings_state.menu_bar_show_used,
            hide_critters: icon_style.hides_critters(),
            icon_style,
            now: Some(jiff::Timestamp::now()),
            ..IconPreferences::default()
        };
        *cell.borrow_mut() = Some(AppState {
            client,
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
            last_forwarded_low_power: None,
            last_forwarded_thermal_action: None,
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
    let contended = STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return true;
        };
        let Some(state) = guard.as_mut() else {
            return false;
        };
        state.global_loading = loading;
        sync_animation_timers(state);
        render_status_item_icon(mtm, state);
        false
    });
    if contended {
        DispatchQueue::main().exec_async(move || set_loading(loading));
    }
}

/// Public icon-state API: sets or clears the per-provider refresh-in-progress state
/// (`UsageStore.refreshingProviders`) and refreshes the status item icon.
pub fn set_provider_loading(provider_id: &str, loading: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let contended = STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return true;
        };
        let Some(state) = guard.as_mut() else {
            return false;
        };
        if loading {
            state.provider_loading.insert(provider_id.to_string());
        } else {
            state.provider_loading.remove(provider_id);
        }
        sync_animation_timers(state);
        render_status_item_icon(mtm, state);
        false
    });
    if contended {
        let owned_id = provider_id.to_string();
        DispatchQueue::main().exec_async(move || set_provider_loading(&owned_id, loading));
    }
}

/// Public icon-state API: applies a new engine snapshot, records downward quota
/// warning threshold crossings for the 60 s flash overlay, and refreshes the UI.
pub fn set_snapshot(snapshot: Option<EngineSnapshot>) {
    enum SetSnapshotOutcome {
        Updated,
        NoState,
        Contended(Option<EngineSnapshot>),
    }
    let outcome = STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return SetSnapshotOutcome::Contended(snapshot);
        };
        let Some(state) = guard.as_mut() else {
            return SetSnapshotOutcome::NoState;
        };
        if let Some(ref snap) = snapshot {
            state.global_loading = snap.engine.refreshing;
            record_snapshot_quota_warnings(state, snap);
            state.settings_window.apply_snapshot(snap.clone());
        }
        state.snapshot = snapshot;
        SetSnapshotOutcome::Updated
    });
    match outcome {
        SetSnapshotOutcome::Updated => refresh_ui(),
        SetSnapshotOutcome::NoState => {}
        SetSnapshotOutcome::Contended(snap) => {
            DispatchQueue::main().exec_async(move || set_snapshot(snap));
        }
    }
}

/// Public icon-state API: triggers the 60 s quota-warning red flash overlay on
/// `provider_id` (`StatusItemController.startQuotaWarningFlash`).
pub fn trigger_quota_warning_flash(provider_id: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let contended = STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return true;
        };
        let Some(state) = guard.as_mut() else {
            return false;
        };
        let _ = state
            .animation
            .start_quota_warning_flash(provider_id, now_secs());
        state.last_render_signature = None;
        sync_animation_timers(state);
        render_status_item_icon(mtm, state);
        false
    });
    if contended {
        let owned_id = provider_id.to_string();
        DispatchQueue::main().exec_async(move || trigger_quota_warning_flash(&owned_id));
    }
}

fn apply_local_setting_change(
    setting_id: ariadusage_protocol::ids::SettingId,
    value: serde_json::Value,
) {
    let contended = STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return true;
        };
        let Some(state) = guard.as_mut() else {
            return false;
        };
        state
            .settings_state
            .set_local(setting_id.as_str(), value.clone());
        sync_state_from_settings(state);
        false
    });
    if contended {
        DispatchQueue::main().exec_async(move || apply_local_setting_change(setting_id, value));
        return;
    }
    refresh_ui();
}

fn sync_state_from_settings(state: &mut AppState) {
    let icon_style =
        icon::MenuBarIconStyle::from_raw_value(&state.settings_state.menu_bar_icon_style)
            .unwrap_or_default();
    state.prefs.show_used = state.settings_state.menu_bar_show_used;
    state.prefs.icon_style = icon_style;
    state.prefs.hide_critters = icon_style.hides_critters();
    state.prefs.now = Some(jiff::Timestamp::now());
    state.last_render_signature = None;
    let card_prefs = CardPreferences {
        show_used: state.settings_state.menu_bar_show_used,
        show_optional_credits_and_extra_usage: state.settings_state.show_optional_credits,
        ..CardPreferences::default()
    };
    state.menu.set_prefs(card_prefs);
}

fn open_settings_window(pane: Option<SettingsPane>) {
    let pane_for_retry = pane.clone();
    let contended = STATE.with(move |cell| {
        let Ok(guard) = cell.try_borrow() else {
            return true;
        };
        let Some(state) = guard.as_ref() else {
            return false;
        };
        state.settings_window.show_window(pane);
        false
    });
    if contended {
        DispatchQueue::main().exec_async(move || open_settings_window(pane_for_retry));
    }
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
    let now_ts = jiff::Timestamp::now();
    let now = now_ts.as_millisecond() as f64 / 1000.0;
    for provider in &snapshot.providers {
        if !provider.enabled {
            continue;
        }
        let (primary, secondary) =
            presentation::resolved_icon_windows(provider, None, Some(now_ts));
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

/// Processes an [`EngineEvent`] on the main thread (including `EngineEvent::Notice` delivery
/// through the configured [`UserNotificationBackend`]).
pub fn handle_event(event: EngineEvent) {
    match event {
        EngineEvent::Connected { engine_version } => {
            let contended = STATE.with(|cell| {
                let Ok(mut guard) = cell.try_borrow_mut() else {
                    return true;
                };
                let Some(state) = guard.as_mut() else {
                    return false;
                };
                state.connected = true;
                state.last_forwarded_low_power = None;
                state.last_forwarded_thermal_action = None;
                sync_power_and_thermal_state(state);
                false
            });
            if contended {
                DispatchQueue::main().exec_async(move || {
                    handle_event(EngineEvent::Connected { engine_version });
                });
                return;
            }
            refresh_ui();
        }
        EngineEvent::Snapshot(snapshot) => {
            let contended = STATE.with(|cell| {
                let Ok(mut guard) = cell.try_borrow_mut() else {
                    return true;
                };
                let Some(state) = guard.as_mut() else {
                    return false;
                };
                state.connected = true;
                false
            });
            if contended {
                DispatchQueue::main()
                    .exec_async(move || handle_event(EngineEvent::Snapshot(snapshot)));
                return;
            }
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
            let contended = STATE.with(|cell| {
                let Ok(mut guard) = cell.try_borrow_mut() else {
                    return true;
                };
                let Some(state) = guard.as_mut() else {
                    return false;
                };
                state.connected = true;
                state.settings_state.apply_page(&page);
                sync_state_from_settings(state);
                state.settings_window.apply_settings_page(&page);
                false
            });
            if contended {
                DispatchQueue::main().exec_async(move || handle_event(EngineEvent::Settings(page)));
                return;
            }
            refresh_ui();
        }
        EngineEvent::Notice {
            level: _,
            title,
            message,
            notification_id,
            sound,
        } => {
            let notice = MacOsNotice::from_ipc(title, message, notification_id, sound);
            deliver_notice(notice);
        }
        EngineEvent::Disconnected => {
            let contended = STATE.with(|cell| {
                let Ok(mut guard) = cell.try_borrow_mut() else {
                    return true;
                };
                let Some(state) = guard.as_mut() else {
                    return false;
                };
                state.connected = false;
                state.last_forwarded_low_power = None;
                state.last_forwarded_thermal_action = None;
                state.global_loading = false;
                state.provider_loading.clear();
                state.last_render_signature = None;
                false
            });
            if contended {
                DispatchQueue::main().exec_async(move || handle_event(EngineEvent::Disconnected));
                return;
            }
            refresh_ui();
        }
    }
}

fn select_tab(selection: SwitcherSelection) {
    let contended = STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return true;
        };
        let Some(state) = guard.as_mut() else {
            return false;
        };
        if let SwitcherSelection::Provider(id) = &selection {
            state.last_provider_id = Some(id.clone());
        }
        state.selected_tab = Some(selection.clone());
        false
    });
    if contended {
        DispatchQueue::main().exec_async(move || select_tab(selection));
        return;
    }
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
    let contended = STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return true;
        };
        let Some(state) = guard.as_mut() else {
            return false;
        };
        let resolved = state.menu.start_user_refresh(
            provider_id.clone(),
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
        false
    });
    if contended {
        DispatchQueue::main().exec_async(move || trigger_user_refresh(provider_id));
    }
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
    sync_power_and_thermal_state(state);
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
    let has_blinking_providers = !state.prefs.icon_style.shows_brand_icon_with_percent()
        && target_id.is_some_and(|id| {
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
    let contended = STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return true;
        };
        let Some(state) = guard.as_mut() else {
            return false;
        };
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
        } else {
            sync_power_and_thermal_state(state);
        }
        render_status_item_icon(mtm, state);
        false
    });
    if contended {
        DispatchQueue::main().exec_async(on_loading_timer_tick);
    }
}

fn on_blink_timer_tick() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let contended = STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return true;
        };
        let Some(state) = guard.as_mut() else {
            return false;
        };
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
        false
    });
    if contended {
        DispatchQueue::main().exec_async(on_blink_timer_tick);
    }
}

fn on_flash_timer_tick() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let contended = STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return true;
        };
        let Some(state) = guard.as_mut() else {
            return false;
        };
        state.flash_timer = None;
        let _ = state
            .animation
            .clear_expired_quota_warning_flashes(now_secs());
        state.last_render_signature = None;
        sync_animation_timers(state);
        render_status_item_icon(mtm, state);
        false
    });
    if contended {
        DispatchQueue::main().exec_async(on_flash_timer_tick);
    }
}

/// Renders the 18 × 18 pt monochrome template brand icon for `provider_id` (`ProviderBrandIcon.image`,
/// `ProviderBrandIcon.swift:29-51`).
pub fn render_brand_icon(provider_id: &str) -> Option<Retained<NSImage>> {
    use crate::switcher::{PathCmd, SwitcherIcon, icon_path};

    let icon_kind = match provider_id {
        "claude" => SwitcherIcon::Claude,
        "codex" => SwitcherIcon::Codex,
        "antigravity" => SwitcherIcon::Antigravity,
        _ => SwitcherIcon::for_provider(provider_id),
    };
    let cmds = icon_path(icon_kind, icon::ICON_SIZE_PT);
    if cmds.is_empty() {
        return None;
    }

    let size = NSSize::new(icon::ICON_SIZE_PT, icon::ICON_SIZE_PT);
    let pixels = icon::CANVAS_PX as isize;
    // SAFETY: a null plane pointer asks AppKit to allocate the 8-bit RGBA bitmap buffer.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            pixels,
            pixels,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    rep.setSize(size);
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    image.addRepresentation(&rep);

    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    context.setShouldAntialias(true);

    let flip_y = |y: f64| icon::ICON_SIZE_PT - y;
    let bezier = NSBezierPath::bezierPath();
    bezier.setWindingRule(NSWindingRule::EvenOdd);
    for cmd in cmds {
        match cmd {
            PathCmd::MoveTo(p) => bezier.moveToPoint(NSPoint::new(p.x, flip_y(p.y))),
            PathCmd::LineTo(p) => bezier.lineToPoint(NSPoint::new(p.x, flip_y(p.y))),
            PathCmd::CubicTo(c1, c2, p) => {
                bezier.curveToPoint_controlPoint1_controlPoint2(
                    NSPoint::new(p.x, flip_y(p.y)),
                    NSPoint::new(c1.x, flip_y(c1.y)),
                    NSPoint::new(c2.x, flip_y(c2.y)),
                );
            }
            PathCmd::Close => bezier.closePath(),
        }
    }
    NSColor::labelColor().setFill();
    bezier.fill();

    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    image.setTemplate(true);
    Some(image)
}

/// Overlays the bottom-right status indicator dot or exclamation mark onto `brand`, returning the
/// original `brand` image unchanged when `status` is [`icon::StatusLevel::None`]
/// (`StatusItemController.brandImageWithStatusOverlay`, `StatusItemController+Animation.swift:1333-1373`).
pub fn brand_image_with_status_overlay(
    brand: &NSImage,
    status: icon::StatusLevel,
) -> Option<Retained<NSImage>> {
    let size = brand.size();
    let Some(overlay) = icon::brand_status_overlay_for_width(status, size.width) else {
        return Some(brand.retain());
    };

    let w_px = (size.width * icon::ICON_SCALE).round().max(1.0) as isize;
    let h_px = (size.height * icon::ICON_SCALE).round().max(1.0) as isize;
    // SAFETY: a null plane pointer asks AppKit to allocate the 8-bit RGBA bitmap buffer.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            w_px,
            h_px,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    rep.setSize(size);
    let output = NSImage::initWithSize(NSImage::alloc(), size);
    output.addRepresentation(&rep);

    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    context.setShouldAntialias(true);

    let canvas = NSRect::new(NSPoint::ZERO, size);
    brand.drawInRect_fromRect_operation_fraction(
        canvas,
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.0,
    );

    NSColor::labelColor().setFill();
    match overlay {
        icon::BrandStatusOverlay::Dot { dot_rect } => {
            let rect = NSRect::new(
                NSPoint::new(dot_rect.x, dot_rect.y),
                NSSize::new(dot_rect.w, dot_rect.h),
            );
            NSBezierPath::bezierPathWithOvalInRect(rect).fill();
        }
        icon::BrandStatusOverlay::Exclamation {
            line_rect,
            line_radius_pt,
            dot_rect,
        } => {
            let lrect = NSRect::new(
                NSPoint::new(line_rect.x, line_rect.y),
                NSSize::new(line_rect.w, line_rect.h),
            );
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                lrect,
                line_radius_pt,
                line_radius_pt,
            )
            .fill();
            let drect = NSRect::new(
                NSPoint::new(dot_rect.x, dot_rect.y),
                NSSize::new(dot_rect.w, dot_rect.h),
            );
            NSBezierPath::bezierPathWithOvalInRect(drect).fill();
        }
    }

    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    output.setTemplate(brand.isTemplate());
    Some(output)
}

/// Returns the maximum alpha component (`0.0..=1.0`) across all pixels of `image` when rendered
/// into a 2× bitmap representation (`StatusItemAnimationTests.maxAlpha`,
/// `StatusItemAnimationTests.swift:1739-1750`).
pub fn max_bitmap_alpha(image: &NSImage) -> f64 {
    let size = image.size();
    let w_px = (size.width * icon::ICON_SCALE).round().max(1.0) as isize;
    let h_px = (size.height * icon::ICON_SCALE).round().max(1.0) as isize;
    // SAFETY: a null plane pointer asks AppKit to allocate the 8-bit RGBA bitmap buffer.
    let Some(rep) = (unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            w_px,
            h_px,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }) else {
        return 0.0;
    };
    rep.setSize(size);
    let Some(context) = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep) else {
        return 0.0;
    };
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    let canvas = NSRect::new(NSPoint::ZERO, size);
    image.drawInRect_fromRect_operation_fraction(
        canvas,
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.0,
    );
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    let ptr = rep.bitmapData();
    if ptr.is_null() {
        return 0.0;
    }
    let bytes_per_row = rep.bytesPerRow();
    let samples = rep.samplesPerPixel().max(4);
    let mut max_a: u8 = 0;
    for y in 0..h_px {
        for x in 0..w_px {
            let offset = (y * bytes_per_row + x * samples + (samples - 1)) as usize;
            // SAFETY: `offset` is within `h_px * bytes_per_row` allocated by `rep`.
            let alpha = unsafe { *ptr.add(offset) };
            if alpha > max_a {
                max_a = alpha;
            }
        }
    }
    f64::from(max_a) / 255.0
}

fn set_status_item_button_content(
    button: &objc2_app_kit::NSStatusBarButton,
    image: &NSImage,
    title: Option<&str>,
) {
    let formatted_title = icon::button_title(title, true, false);
    button.setImage(Some(image));
    button.setTitle(&NSString::from_str(&formatted_title));
    let position = if formatted_title.is_empty() {
        NSCellImagePosition::ImageOnly
    } else {
        NSCellImagePosition::ImageLeft
    };
    button.setImagePosition(position);
}

fn render_status_item_icon(mtm: MainThreadMarker, state: &mut AppState) {
    let now_ts = jiff::Timestamp::now();
    let now = now_ts.as_millisecond() as f64 / 1000.0;
    state.prefs.now = Some(now_ts);
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

    if state.prefs.icon_style.shows_brand_icon_with_percent()
        && let Some(brand_base) = render_brand_icon(provider_id.unwrap_or("codex"))
    {
        let display_text =
            provider.and_then(|p| presentation::menu_bar_display_text(p, state.prefs));
        let signature = icon::brand_icon_render_signature(
            provider_id.unwrap_or("codex"),
            input.style,
            input.status,
            display_text.as_deref(),
            warning_flash,
            false,
            None,
        );
        if state.last_render_signature == Some(signature) {
            return;
        }
        state.last_render_signature = Some(signature);
        let badged =
            brand_image_with_status_overlay(&brand_base, input.status).unwrap_or(brand_base);
        let displayed_image = if warning_flash {
            icon_image::quota_warning_flash_image(&badged).unwrap_or(badged)
        } else {
            badged
        };
        if let Some(button) = state.status_item.button(mtm) {
            set_status_item_button_content(&button, &displayed_image, display_text.as_deref());
        }
        return;
    }

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
                set_status_item_button_content(&button, &image, None);
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
        set_status_item_button_content(&button, &image, None);
    }
}

/// Redraws the icon and updates the menu from the current state.
fn refresh_ui() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let contended = STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return true;
        };
        let Some(state) = guard.as_mut() else {
            return false;
        };
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
        false
    });
    if contended {
        DispatchQueue::main().exec_async(refresh_ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Rust-added: smoke test for macOS NSProcessInfo power and thermal state readers, IPC action mapping, and change-only forwarding.
    #[test]
    fn macos_process_info_power_and_thermal_actions_map_cleanly() {
        assert_eq!(low_power_mode_action_id(true), "setLowPowerMode.on");
        assert_eq!(low_power_mode_action_id(false), "setLowPowerMode.off");
        assert_eq!(thermal_pressure_action_id(0), "setThermalPressure.nominal");
        assert_eq!(thermal_pressure_action_id(1), "setThermalPressure.nominal");
        assert_eq!(
            thermal_pressure_action_id(2),
            "setThermalPressure.constrained"
        );
        assert_eq!(
            thermal_pressure_action_id(3),
            "setThermalPressure.constrained"
        );

        let mut last_low = None;
        let mut last_thermal = None;
        assert_eq!(
            power_and_thermal_actions_to_forward(&mut last_low, &mut last_thermal, false, 0),
            vec!["setLowPowerMode.off", "setThermalPressure.nominal"]
        );
        assert_eq!(
            power_and_thermal_actions_to_forward(&mut last_low, &mut last_thermal, false, 1),
            Vec::<&str>::new()
        );
        assert_eq!(
            power_and_thermal_actions_to_forward(&mut last_low, &mut last_thermal, true, 2),
            vec!["setLowPowerMode.on", "setThermalPressure.constrained"]
        );
        assert_eq!(
            power_and_thermal_actions_to_forward(&mut last_low, &mut last_thermal, true, 3),
            Vec::<&str>::new()
        );

        let _ = current_low_power_mode_enabled();
        let raw_thermal = current_thermal_state_raw();
        assert!((0..=3).contains(&raw_thermal));
    }
}
