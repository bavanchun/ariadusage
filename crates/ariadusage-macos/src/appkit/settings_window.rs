// Ported from CodexBar Sources/CodexBar/SettingsWindowController.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/DockIconController.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! AppKit window controller and delegate for the AriadUsage Settings window.
//!
//! Creates and reuses a single `NSWindow` with transparent full-size content view,
//! frame autosave `"AriadUsage.SettingsWindow"`, and manages `NSApp` activation policy
//! between `.accessory` (menu-bar only) and `.regular` (Dock icon + main menu active
//! while Settings is open or miniaturized).

use std::cell::Cell;

use ariadusage_protocol::settings::SettingsPage;
use ariadusage_protocol::snapshot::EngineSnapshot;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSTitlebarSeparatorStyle,
    NSWindow, NSWindowDelegate, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{
    NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use super::settings_view::SettingsView;
use crate::engine_client::EngineClient;
use crate::settings::{
    DockWindowDescriptor, SettingsPane, WINDOW_AUTOSAVE_NAME, WINDOW_HEIGHT, WINDOW_IDENTIFIER,
    WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH, WINDOW_WIDTH, should_use_regular_activation_policy,
};

#[derive(Default)]
pub struct WindowDelegateIvars {
    closing: Cell<bool>,
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements beyond `init`, and
    // `SettingsWindowDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsageSettingsWindowDelegate"]
    #[ivars = WindowDelegateIvars]
    pub struct SettingsWindowDelegate;

    unsafe impl NSObjectProtocol for SettingsWindowDelegate {}

    unsafe impl NSWindowDelegate for SettingsWindowDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            self.ivars().closing.set(true);
            let mtm = self.mtm();
            let descriptor = DockWindowDescriptor {
                identifier: Some(WINDOW_IDENTIFIER.to_string()),
                frame_autosave_name: WINDOW_AUTOSAVE_NAME.to_string(),
                title: "Settings".to_string(),
                width: WINDOW_WIDTH,
                height: WINDOW_HEIGHT,
                is_visible: false,
                is_miniaturized: false,
                can_become_key: true,
                is_known_settings_window: true,
                is_status_bar_window: false,
            };
            if !should_use_regular_activation_policy(&[descriptor]) {
                let app = NSApplication::sharedApplication(mtm);
                app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            }
        }

        #[unsafe(method(windowDidMiniaturize:))]
        fn window_did_miniaturize(&self, _notification: &NSNotification) {
            let mtm = self.mtm();
            let descriptor = DockWindowDescriptor {
                identifier: Some(WINDOW_IDENTIFIER.to_string()),
                frame_autosave_name: WINDOW_AUTOSAVE_NAME.to_string(),
                title: "Settings".to_string(),
                width: WINDOW_WIDTH,
                height: WINDOW_HEIGHT,
                is_visible: false,
                is_miniaturized: true,
                can_become_key: true,
                is_known_settings_window: true,
                is_status_bar_window: false,
            };
            if should_use_regular_activation_policy(&[descriptor]) {
                let app = NSApplication::sharedApplication(mtm);
                app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            }
        }
    }
);

impl SettingsWindowDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WindowDelegateIvars::default());
        // SAFETY: `init` is `NSObject`'s designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// Manages the singleton Settings `NSWindow` and its hosted [`SettingsView`].
pub struct SettingsWindowController {
    mtm: MainThreadMarker,
    window: Retained<NSWindow>,
    view: Retained<SettingsView>,
    _delegate: Retained<SettingsWindowDelegate>,
}

impl SettingsWindowController {
    pub fn new(mtm: MainThreadMarker, engine_client: Option<EngineClient>) -> Self {
        let view = SettingsView::new(mtm);
        if let Some(client) = engine_client {
            view.set_engine_client(client);
        }

        let style_mask = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::FullSizeContentView;

        let content_rect = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        );

        // SAFETY: `initWithContentRect:styleMask:backing:defer:` is `NSWindow`'s designated
        // initializer and is called on the main thread (`mtm`).
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                content_rect,
                style_mask,
                NSBackingStoreType::Buffered,
                false,
            )
        };

        // SAFETY: `window` is owned by `SettingsWindowController` and kept alive across close/reopen.
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setTitle(&NSString::from_str("Settings"));
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
        window.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
        window.setContentMinSize(NSSize::new(WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT));
        window.setContentView(Some(&view));
        window.setAutorecalculatesKeyViewLoop(true);
        if let Some(sf) = view.search_field() {
            window.setInitialFirstResponder(Some(&sf));
        }

        // SAFETY: `setIdentifier:` accepts an `NSString` (`NSUserInterfaceItemIdentifier`) on `NSWindow`.
        let ident = NSString::from_str(WINDOW_IDENTIFIER);
        let _: () = unsafe { msg_send![&window, setIdentifier: &*ident] };

        let autosave = NSString::from_str(WINDOW_AUTOSAVE_NAME);
        let restored = window.setFrameAutosaveName(&autosave);
        if !restored {
            window.center();
        }

        let delegate = SettingsWindowDelegate::new(mtm);
        let proto = ProtocolObject::from_ref(&*delegate);
        window.setDelegate(Some(proto));

        Self {
            mtm,
            window,
            view,
            _delegate: delegate,
        }
    }

    pub fn view(&self) -> &SettingsView {
        &self.view
    }

    pub fn window(&self) -> &NSWindow {
        &self.window
    }

    pub fn apply_settings_page(&self, page: &SettingsPage) {
        self.view.apply_settings_page(page);
    }

    pub fn apply_snapshot(&self, snapshot: EngineSnapshot) {
        self.view.apply_snapshot(snapshot);
    }

    /// Opens the Settings window (promoting `NSApp` to `.regular`), optionally selecting `pane`.
    pub fn show_window(&self, pane: Option<SettingsPane>) {
        if let Some(target_pane) = pane {
            self.view.select_pane(target_pane);
        }

        let app = NSApplication::sharedApplication(self.mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.finishLaunching();

        if self.window.isMiniaturized() {
            self.window.deminiaturize(None);
        }

        self.window.recalculateKeyViewLoop();
        self.window.makeKeyAndOrderFront(None);
        if let Some(sf) = self.view.search_field() {
            let _ = self.window.makeFirstResponder(Some(&sf));
        } else {
            let _ = self.window.makeFirstResponder(Some(&self.view));
        }
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}
