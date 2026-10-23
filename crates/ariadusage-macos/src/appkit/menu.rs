// Ported from CodexBar Sources/CodexBar/MenuDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+Menu.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuCardItems.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuTracking.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! The status item's dropdown menu.
//!
//! The usage card for the icon's provider comes first, then the meta
//! section in CodexBar's order and shortcuts: Refresh (⌘R), About, Quit
//! (⌘Q). Settings joins it with the Settings window. The items are created
//! once; snapshots update the card in place so an open menu does not
//! flicker.

use std::cell::Cell;

use ariadusage_protocol::snapshot::ProviderSnapshot;
use jiff::Timestamp;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSMenu, NSMenuDelegate, NSMenuItem};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};

use super::card_view::CardView;
use super::date_texts::FoundationDateTexts;
use crate::card::{self, CardPreferences};
use crate::card_layout;
use crate::engine_client::EngineClient;

/// A card row is highlightable only when it opens a submenu or handles a
/// click (`makeMenuCardItem`); the usage card does neither yet.
const CARD_ALLOWS_MENU_HIGHLIGHT: bool = false;
/// `highlightMenuCardsOnHover`, default true.
const HIGHLIGHT_MENU_CARDS_ON_HOVER: bool = true;

/// Whether the card draws its hover highlight (`applyMenuCardHighlight`).
fn card_highlighted(is_card: bool, item_enabled: bool) -> bool {
    is_card && item_enabled && CARD_ALLOWS_MENU_HIGHLIGHT && HIGHLIGHT_MENU_CARDS_ON_HOVER
}

/// State held by the menu's action target and delegate.
pub struct TargetIvars {
    client: EngineClient,
    is_open: Cell<bool>,
    on_update: fn(),
    card_item: Retained<NSMenuItem>,
    card_view: Retained<CardView>,
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
        fn menu_needs_update(&self, _menu: &NSMenu) {
            (self.ivars().on_update)();
        }

        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _menu: &NSMenu) {
            self.ivars().is_open.set(true);
            // Rebuild so relative times and countdowns are current.
            (self.ivars().on_update)();
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            let ivars = self.ivars();
            ivars.is_open.set(false);
            ivars.card_view.set_highlighted(false);
            (ivars.on_update)();
        }

        #[unsafe(method(menu:willHighlightItem:))]
        fn menu_will_highlight_item(&self, _menu: &NSMenu, item: Option<&NSMenuItem>) {
            let ivars = self.ivars();
            let is_card = item.is_some_and(|item| std::ptr::eq(item, &*ivars.card_item));
            ivars
                .card_view
                .set_highlighted(card_highlighted(is_card, ivars.card_item.isEnabled()));
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
    fn new(mtm: MainThreadMarker, ivars: TargetIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: `init` is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// Owns the menu and updates it from snapshots.
pub struct MenuController {
    menu: Retained<NSMenu>,
    /// The delegate and Refresh target; AppKit holds both weakly.
    _target: Retained<MenuTarget>,
    status: Retained<NSMenuItem>,
    card_item: Retained<NSMenuItem>,
    card_view: Retained<CardView>,
    refresh: Retained<NSMenuItem>,
    dates: FoundationDateTexts,
    prefs: CardPreferences,
}

impl MenuController {
    /// Builds the menu. `on_update` runs when AppKit prepares, opens or closes
    /// the menu so relative times and countdowns stay fresh.
    pub fn new(mtm: MainThreadMarker, client: EngineClient, on_update: fn()) -> Self {
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);

        let status = item(mtm, "", None, "");
        status.setEnabled(false);
        status.setHidden(true);

        let card_view = CardView::new(mtm);
        let card_item = NSMenuItem::new(mtm);
        card_item.setView(Some(&card_view));
        // Without highlight or interactive controls the row stays disabled.
        card_item.setEnabled(CARD_ALLOWS_MENU_HIGHLIGHT);
        card_item.setHidden(true);

        let target = MenuTarget::new(
            mtm,
            TargetIvars {
                client,
                is_open: Cell::new(false),
                on_update,
                card_item: card_item.clone(),
                card_view: card_view.clone(),
            },
        );
        menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));

        menu.addItem(&status);
        menu.addItem(&card_item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let refresh = item(mtm, "Refresh", Some(sel!(refresh:)), "r");
        refresh.setEnabled(false);
        // SAFETY: the target lives as long as the controller, which owns the menu.
        unsafe { refresh.setTarget(Some(&target)) };
        menu.addItem(&refresh);
        // A nil target sends these to NSApplication through the responder chain.
        menu.addItem(&item(
            mtm,
            "About AriadUsage",
            Some(sel!(orderFrontStandardAboutPanel:)),
            "",
        ));
        menu.addItem(&item(mtm, "Quit", Some(sel!(terminate:)), "q"));

        Self {
            menu,
            _target: target,
            status,
            card_item,
            card_view,
            refresh,
            dates: FoundationDateTexts::new(),
            prefs: CardPreferences::default(),
        }
    }

    pub fn menu(&self) -> &NSMenu {
        &self.menu
    }

    /// Shows the card for `provider`, or `status` when there is no
    /// provider to show, and enables Refresh while connected.
    pub fn update(
        &self,
        provider: Option<&ProviderSnapshot>,
        status: Option<&str>,
        connected: bool,
    ) {
        match provider {
            Some(provider) => {
                let model =
                    card::provider_card(provider, &self.prefs, Timestamp::now(), &self.dates);
                let width = card_layout::rendered_width(self.menu.size().width);
                if self.card_view.update(&model, width) {
                    self.menu.itemChanged(&self.card_item);
                }
                self.card_item.setHidden(false);
            }
            None => self.card_item.setHidden(true),
        }
        match status {
            Some(text) => {
                self.status.setTitle(&NSString::from_str(text));
                self.status.setHidden(false);
            }
            None => self.status.setHidden(true),
        }
        self.refresh.setEnabled(connected);
    }
}

fn item(
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
