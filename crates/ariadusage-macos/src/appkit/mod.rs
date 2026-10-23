// Ported from CodexBar Sources/CodexBar/StatusItemController+StatusItemVending.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! AppKit bridge: the application, its status item and its menu.
//!
//! Everything here runs on the main thread. Engine events arrive on the
//! connection thread and are forwarded to the main queue, where the single
//! [`AppState`] lives in a thread-local.

#![allow(unsafe_code)]

mod card_view;
mod color;
mod date_texts;
mod icon_image;
mod menu;
mod progress_bar_view;

pub use card_view::{CardView, png_bytes};
pub use date_texts::FoundationDateTexts;

use std::cell::RefCell;
use std::path::PathBuf;

use ariadusage_protocol::snapshot::EngineSnapshot;
use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSApplicationActivationPolicy, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::NSString;

use crate::engine_client::{EngineClient, EngineEvent};
use crate::icon;
use crate::presentation::{self, IconPreferences};

/// Autosave name of the merged status item, so macOS keeps its position.
const STATUS_ITEM_AUTOSAVE_NAME: &str = "ariadusage-merged";
const STATUS_ITEM_ACCESSIBILITY_TITLE: &str = "AriadUsage";
const STATUS_ITEM_ACCESSIBILITY_IDENTIFIER: &str = "AriadUsage.StatusItem";

struct AppState {
    status_item: Retained<NSStatusItem>,
    menu: menu::MenuController,
    snapshot: Option<EngineSnapshot>,
    connected: bool,
    selected_provider: Option<String>,
    prefs: IconPreferences,
}

thread_local! {
    static STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

/// Starts the app and runs the AppKit event loop until the user quits.
pub fn run(socket: PathBuf) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("the app must start on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    // A menu-bar app has no Dock icon and no main window.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let client = EngineClient::spawn(socket, |event| {
        DispatchQueue::main().exec_async(move || handle_event(event));
    })
    .map_err(|err| format!("could not start the engine connection: {err}"))?;

    let status_item = make_status_item(mtm);
    let menu = menu::MenuController::new(mtm, client, refresh_ui);
    status_item.setMenu(Some(menu.menu()));
    STATE.with(|cell| {
        *cell.borrow_mut() = Some(AppState {
            status_item,
            menu,
            snapshot: None,
            connected: false,
            selected_provider: None,
            prefs: IconPreferences::default(),
        });
    });
    refresh_ui();

    app.run();
    Ok(())
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

fn handle_event(event: EngineEvent) {
    STATE.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(state) = guard.as_mut() else { return };
        match event {
            EngineEvent::Connected { .. } => state.connected = true,
            EngineEvent::Snapshot(snapshot) => {
                state.connected = true;
                state.snapshot = Some(*snapshot);
            }
            EngineEvent::Disconnected => state.connected = false,
        }
    });
    refresh_ui();
}

/// Redraws the icon and updates the menu from the current state.
fn refresh_ui() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    STATE.with(|cell| {
        let guard = cell.borrow();
        let Some(state) = guard.as_ref() else { return };
        let provider = state
            .snapshot
            .as_ref()
            .and_then(|s| presentation::icon_provider(s, state.selected_provider.as_deref()));
        let input = match provider {
            Some(provider) => presentation::icon_input(provider, state.prefs),
            // No data yet: draw empty, dimmed lanes rather than a fake value.
            None => icon::IconInput {
                stale: true,
                ..icon::IconInput::default()
            },
        };
        if let Some(button) = state.status_item.button(mtm)
            && let Some(image) = icon_image::render(&icon::layout(&input))
        {
            button.setImage(Some(&image));
        }
        // The card shows the icon's provider while the engine is connected.
        let card_provider = provider.filter(|_| state.connected);
        let status = presentation::connection_status(state.snapshot.as_ref(), state.connected);
        state
            .menu
            .update(card_provider, status.as_deref(), state.connected);
    });
}
