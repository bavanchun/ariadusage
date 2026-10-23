// Ported from CodexBar Sources/CodexBar/MenuDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+Menu.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! The status item's dropdown menu.
//!
//! The meta section follows CodexBar's order and shortcuts: Refresh (⌘R),
//! About, Quit (⌘Q). Settings joins it with the Settings window.

use ariadusage_protocol::snapshot::EngineSnapshot;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSMenu, NSMenuItem};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};

use crate::engine_client::EngineClient;
use crate::presentation::{self, IconPreferences};

/// State held by the menu's action target.
pub struct TargetIvars {
    client: EngineClient,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class
    // does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsageMenuTarget"]
    #[ivars = TargetIvars]
    pub struct MenuTarget;

    unsafe impl NSObjectProtocol for MenuTarget {}

    impl MenuTarget {
        #[unsafe(method(refresh:))]
        fn refresh(&self, _sender: Option<&AnyObject>) {
            self.ivars().client.refresh(None);
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker, client: EngineClient) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars { client });
        // SAFETY: `init` is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// Owns the menu and rebuilds it from snapshots.
pub struct MenuController {
    mtm: MainThreadMarker,
    menu: Retained<NSMenu>,
    target: Retained<MenuTarget>,
}

impl MenuController {
    pub fn new(mtm: MainThreadMarker, client: EngineClient) -> Self {
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        Self {
            mtm,
            menu,
            target: MenuTarget::new(mtm, client),
        }
    }

    pub fn menu(&self) -> &NSMenu {
        &self.menu
    }

    /// Replaces the menu's items with the current state.
    pub fn rebuild(&self, snapshot: Option<&EngineSnapshot>, connected: bool) {
        self.menu.removeAllItems();
        for line in presentation::provider_lines(snapshot, connected, IconPreferences::default()) {
            let item = self.item(&line.text, None, "");
            item.setEnabled(false);
            self.menu.addItem(&item);
        }
        self.menu.addItem(&NSMenuItem::separatorItem(self.mtm));

        let refresh = self.item("Refresh", Some(sel!(refresh:)), "r");
        // SAFETY: the target lives as long as the controller, which owns the menu.
        unsafe { refresh.setTarget(Some(&self.target)) };
        refresh.setEnabled(connected);
        self.menu.addItem(&refresh);

        // A nil target sends these to NSApplication through the responder chain.
        self.menu.addItem(&self.item(
            "About AriadUsage",
            Some(sel!(orderFrontStandardAboutPanel:)),
            "",
        ));
        self.menu
            .addItem(&self.item("Quit", Some(sel!(terminate:)), "q"));
    }

    fn item(&self, title: &str, action: Option<Sel>, key: &str) -> Retained<NSMenuItem> {
        // SAFETY: the action selectors used here take one sender argument.
        unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm),
                &NSString::from_str(title),
                action,
                &NSString::from_str(key),
            )
        }
    }
}
