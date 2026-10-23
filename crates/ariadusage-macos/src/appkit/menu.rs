// Ported from CodexBar Sources/CodexBar/MenuDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+Menu.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! The status item's dropdown menu.
//!
//! The meta section follows CodexBar's order and shortcuts: Refresh (⌘R),
//! About, Quit (⌘Q). Settings joins it with the Settings window.

use std::cell::{Cell, RefCell};

use ariadusage_protocol::snapshot::EngineSnapshot;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSMenu, NSMenuDelegate, NSMenuItem};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};

use crate::engine_client::EngineClient;
use crate::presentation::{self, IconPreferences};

/// State held by the menu's action target and delegate.
pub struct TargetIvars {
    client: EngineClient,
    is_open: Cell<bool>,
    connected: Cell<bool>,
    lines: RefCell<Vec<String>>,
    provider_items: RefCell<Vec<Retained<NSMenuItem>>>,
    refresh_item: Retained<NSMenuItem>,
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

    unsafe impl NSMenuDelegate for MenuTarget {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            self.sync_items(menu);
        }

        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _menu: &NSMenu) {
            self.ivars().is_open.set(true);
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, menu: &NSMenu) {
            self.ivars().is_open.set(false);
            self.sync_items(menu);
        }
    }

    impl MenuTarget {
        #[unsafe(method(refresh:))]
        fn refresh(&self, _sender: Option<&AnyObject>) {
            self.ivars().client.refresh(None);
        }
    }
);

impl MenuTarget {
    fn new(
        mtm: MainThreadMarker,
        client: EngineClient,
        refresh_item: Retained<NSMenuItem>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars {
            client,
            is_open: Cell::new(false),
            connected: Cell::new(false),
            lines: RefCell::new(Vec::new()),
            provider_items: RefCell::new(Vec::new()),
            refresh_item,
        });
        // SAFETY: `init` is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }

    /// Updates the menu without removing items while it is open, so an open
    /// menu never loses its highlight or drops a click.
    fn apply_to_menu(&self, menu: &NSMenu) {
        if self.ivars().is_open.get() {
            self.ivars()
                .refresh_item
                .setEnabled(self.ivars().connected.get());
            let lines = self.ivars().lines.borrow();
            let items = self.ivars().provider_items.borrow();
            for (item, line) in items.iter().zip(lines.iter()) {
                item.setTitle(&NSString::from_str(line));
            }
        } else {
            self.sync_items(menu);
        }
    }

    /// Synchronizes the provider rows and Refresh state with the latest
    /// snapshot. When the row count is unchanged, titles are updated in place.
    fn sync_items(&self, menu: &NSMenu) {
        let mtm = self.mtm();
        self.ivars()
            .refresh_item
            .setEnabled(self.ivars().connected.get());
        let lines = self.ivars().lines.borrow();
        let mut items = self.ivars().provider_items.borrow_mut();
        if items.len() == lines.len() {
            for (item, line) in items.iter().zip(lines.iter()) {
                item.setTitle(&NSString::from_str(line));
            }
            return;
        }
        for item in items.drain(..) {
            menu.removeItem(&item);
        }
        for (idx, line) in lines.iter().enumerate() {
            let row = make_item(mtm, line, None, "");
            row.setEnabled(false);
            menu.insertItem_atIndex(&row, idx as isize);
            items.push(row);
        }
    }
}

/// Owns the menu and updates it from snapshots.
pub struct MenuController {
    menu: Retained<NSMenu>,
    target: Retained<MenuTarget>,
}

impl MenuController {
    pub fn new(mtm: MainThreadMarker, client: EngineClient) -> Self {
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);

        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let refresh = make_item(mtm, "Refresh", Some(sel!(refresh:)), "r");
        refresh.setEnabled(false);
        menu.addItem(&refresh);

        // A nil target sends these to NSApplication through the responder chain.
        menu.addItem(&make_item(
            mtm,
            "About AriadUsage",
            Some(sel!(orderFrontStandardAboutPanel:)),
            "",
        ));
        menu.addItem(&make_item(mtm, "Quit", Some(sel!(terminate:)), "q"));

        let target = MenuTarget::new(mtm, client, refresh.clone());
        // SAFETY: the target lives as long as the controller, which owns the menu.
        unsafe { refresh.setTarget(Some(&target)) };
        menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));

        Self { menu, target }
    }

    pub fn menu(&self) -> &NSMenu {
        &self.menu
    }

    /// Updates the menu's items with the current state.
    pub fn rebuild(&self, snapshot: Option<&EngineSnapshot>, connected: bool) {
        let lines = presentation::provider_lines(snapshot, connected, IconPreferences::default())
            .into_iter()
            .map(|line| line.text)
            .collect();
        self.target.ivars().connected.set(connected);
        *self.target.ivars().lines.borrow_mut() = lines;
        self.target.apply_to_menu(&self.menu);
    }
}

fn make_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
) -> Retained<NSMenuItem> {
    // SAFETY: the action selectors used here take one sender argument.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    }
}
