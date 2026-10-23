// Ported from CodexBar Sources/CodexBar/MenuDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+Menu.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuCardItems.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuTracking.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuTextRows.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuWidthCache.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+OverviewSpend.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! The status item's dropdown menu.
//!
//! When more than one provider is enabled, the top row hosts [`SwitcherView`]
//! (`Overview` + one toggle per enabled provider), followed by a separator,
//! the active tab's primary content (Overview rows or the selected provider's
//! usage card), contextual provider actions and the meta section (`Refresh` ⌘R,
//! `Settings...` ⌘,, `About AriadUsage (vX.Y.Z)`, `Quit` ⌘Q).
//!
//! The switcher item stays attached at index `0` across tab switches so
//! clicking a tab rebuilds the menu body in place without closing the menu.

use std::cell::{Cell, RefCell};

use ariadusage_protocol::snapshot::EngineSnapshot;
use jiff::Timestamp;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSColor, NSFont, NSImage, NSLineBreakMode, NSMenu, NSMenuDelegate, NSMenuItem, NSTextField,
    NSView, NSWorkspace,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL};

use super::card_view::CardView;
use super::date_texts::FoundationDateTexts;
use super::switcher_view::SwitcherView;
use crate::card::CardPreferences;
use crate::card_layout;
use crate::engine_client::EngineClient;
use crate::menu_descriptor::{
    self, MenuAction, MenuEntry, MenuStructure, PrimaryContent, SubmenuEntry,
};
use crate::switcher::SwitcherSelection;

/// `highlightMenuCardsOnHover`, default true.
const HIGHLIGHT_MENU_CARDS_ON_HOVER: bool = true;

/// Whether a card row draws its hover highlight (`applyMenuCardHighlight`).
fn card_highlighted(is_hovered: bool, item_enabled: bool, allows_highlight: bool) -> bool {
    is_hovered && item_enabled && allows_highlight && HIGHLIGHT_MENU_CARDS_ON_HOVER
}

struct HighlightableCard {
    item: Retained<NSMenuItem>,
    view: Retained<CardView>,
    allows_highlight: bool,
}

/// State held by the menu's action target and delegate.
pub struct TargetIvars {
    client: EngineClient,
    is_open: Cell<bool>,
    on_update: fn(),
    on_select: fn(SwitcherSelection),
    cards: RefCell<Vec<HighlightableCard>>,
    url_actions: RefCell<Vec<String>>,
    overview_providers: RefCell<Vec<String>>,
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
            // Rebuild before marking the menu open so relative times, countdowns
            // and any deferred structural changes are applied before tracking starts.
            (self.ivars().on_update)();
            self.ivars().is_open.set(true);
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            let ivars = self.ivars();
            ivars.is_open.set(false);
            for card in ivars.cards.borrow().iter() {
                card.view.set_highlighted(false);
            }
            (ivars.on_update)();
        }

        #[unsafe(method(menu:willHighlightItem:))]
        fn menu_will_highlight_item(&self, _menu: &NSMenu, item: Option<&NSMenuItem>) {
            let ivars = self.ivars();
            for card in ivars.cards.borrow().iter() {
                let is_hovered = item.is_some_and(|hi| std::ptr::eq(hi, &*card.item));
                card.view.set_highlighted(card_highlighted(
                    is_hovered,
                    card.item.isEnabled(),
                    card.allows_highlight,
                ));
            }
        }
    }

    impl MenuTarget {
        #[unsafe(method(refresh:))]
        fn refresh(&self, _sender: Option<&AnyObject>) {
            self.ivars().client.refresh(None);
        }

        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: Option<&AnyObject>) {
            // Settings window lands in P5; documented in docs/porting.md.
        }

        #[unsafe(method(accountAction:))]
        fn account_action(&self, _sender: Option<&AnyObject>) {
            // Account switch/login flow lands with Settings in P5; documented in docs/porting.md.
        }

        #[unsafe(method(shareUsageSnapshot:))]
        fn share_usage_snapshot(&self, _sender: Option<&AnyObject>) {
            // Overview share stats sheet lands in a later slice; documented in docs/porting.md.
        }

        #[unsafe(method(openUrlAction:))]
        fn open_url_action(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: `sender` is an `NSMenuItem` created by `MenuController` with an integer tag.
            let tag: isize = unsafe { msg_send![sender, tag] };
            if tag < 0 {
                return;
            }
            let url = self
                .ivars()
                .url_actions
                .borrow()
                .get(tag as usize)
                .cloned();
            if let Some(url) = url
                && let Some(ns_url) = NSURL::URLWithString(&NSString::from_str(&url))
            {
                NSWorkspace::sharedWorkspace().openURL(&ns_url);
            }
        }

        #[unsafe(method(selectOverviewRow:))]
        fn select_overview_row(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: `sender` is an Overview `NSMenuItem` created with an integer tag.
            let tag: isize = unsafe { msg_send![sender, tag] };
            if tag < 0 {
                return;
            }
            let provider_id = self
                .ivars()
                .overview_providers
                .borrow()
                .get(tag as usize)
                .cloned();
            if let Some(id) = provider_id {
                (self.ivars().on_select)(SwitcherSelection::Provider(id));
            }
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
    target: Retained<MenuTarget>,
    switcher_item: Retained<NSMenuItem>,
    switcher_view: Retained<SwitcherView>,
    switcher_sep: Retained<NSMenuItem>,
    on_select: fn(SwitcherSelection),
    dates: FoundationDateTexts,
    prefs: CardPreferences,
    last_structure: RefCell<Option<MenuStructure>>,
    retired_cards: RefCell<Vec<Retained<CardView>>>,
}

impl MenuController {
    /// Builds the menu. `on_update` runs when AppKit prepares, opens or closes
    /// the menu; `on_select` runs when the user selects a tab in the switcher
    /// or clicks a provider card on the Overview tab.
    pub fn new(
        mtm: MainThreadMarker,
        client: EngineClient,
        on_update: fn(),
        on_select: fn(SwitcherSelection),
    ) -> Self {
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);

        let switcher_view = SwitcherView::new(mtm);
        switcher_view.set_on_select(on_select);
        let switcher_item = NSMenuItem::new(mtm);
        switcher_item.setTitle(&NSString::from_str(""));
        switcher_item.setView(Some(&switcher_view));
        switcher_item.setEnabled(false);
        switcher_item.setHidden(true);

        let switcher_sep = NSMenuItem::separatorItem(mtm);
        switcher_sep.setHidden(true);

        let target = MenuTarget::new(
            mtm,
            TargetIvars {
                client,
                is_open: Cell::new(false),
                on_update,
                on_select,
                cards: RefCell::new(Vec::new()),
                url_actions: RefCell::new(Vec::new()),
                overview_providers: RefCell::new(Vec::new()),
            },
        );
        menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));

        menu.addItem(&switcher_item);
        menu.addItem(&switcher_sep);

        Self {
            menu,
            target,
            switcher_item,
            switcher_view,
            switcher_sep,
            on_select,
            dates: FoundationDateTexts::new(),
            prefs: CardPreferences::default(),
            last_structure: RefCell::new(None),
            retired_cards: RefCell::new(Vec::new()),
        }
    }

    pub fn menu(&self) -> &NSMenu {
        &self.menu
    }

    /// Reconciles the menu with `snapshot`, `connected` and `selected_tab`.
    /// Returns the resolved [`SwitcherSelection`], if any.
    pub fn update(
        &self,
        snapshot: Option<&EngineSnapshot>,
        connected: bool,
        selected_tab: Option<&SwitcherSelection>,
    ) -> Option<SwitcherSelection> {
        let structure = menu_descriptor::build(
            snapshot,
            connected,
            selected_tab,
            &self.prefs,
            Timestamp::now(),
            &self.dates,
        );
        let resolved = structure.selection.clone();
        self.apply_structure(structure);
        resolved
    }

    fn apply_structure(&self, structure: MenuStructure) {
        let width = measured_menu_width(self.menu.mtm(), &structure);
        let is_open = self.target.ivars().is_open.get();
        let same_selection = self
            .last_structure
            .borrow()
            .as_ref()
            .is_some_and(|prev| prev.selection == structure.selection);

        // While the menu is open on the same tab (a background snapshot update, not a user
        // tab click), freeze the tracked layout (`MenuCardRefreshMonitor.hasCompatibleTrackedLayout`):
        // only apply an in-place card update if the menu structure and card heights match,
        // and otherwise defer the rebuild to `menuDidClose:`.
        if is_open && same_selection {
            if self.try_update_in_place(&structure, width, true) {
                if let Some(selected) = &structure.selection
                    && !structure.switcher.is_empty()
                    && self
                        .switcher_view
                        .update(&structure.switcher, selected, width)
                {
                    self.menu.itemChanged(&self.switcher_item);
                }
                *self.last_structure.borrow_mut() = Some(structure);
            }
            return;
        }

        if let Some(selected) = &structure.selection
            && !structure.switcher.is_empty()
        {
            if self
                .switcher_view
                .update(&structure.switcher, selected, width)
            {
                self.menu.itemChanged(&self.switcher_item);
            }
            self.switcher_item.setHidden(false);
            self.switcher_sep.setHidden(false);
        } else {
            self.switcher_item.setHidden(true);
            self.switcher_sep.setHidden(true);
        }

        if self.try_update_in_place(&structure, width, false) {
            *self.last_structure.borrow_mut() = Some(structure);
            return;
        }

        self.rebuild_body(&structure, width);
        *self.last_structure.borrow_mut() = Some(structure);
    }

    /// Updates the existing [`CardView`](s) in place without mutating the menu
    /// item list when staying on the same tab and all non-card rows match.
    /// When `require_same_height` is true (`is_open`), rejects the update if
    /// any card's layout height would change.
    fn try_update_in_place(
        &self,
        next: &MenuStructure,
        width: f64,
        require_same_height: bool,
    ) -> bool {
        let prev_borrow = self.last_structure.borrow();
        let Some(prev) = prev_borrow.as_ref() else {
            return false;
        };
        if prev.selection != next.selection
            || prev.history != next.history
            || prev.actions != next.actions
            || prev.meta != next.meta
        {
            return false;
        }
        let cards = self.target.ivars().cards.borrow();
        match (&prev.primary, &next.primary) {
            (
                PrimaryContent::Provider {
                    provider_id: prev_id,
                    buy_credits: prev_buy,
                    ..
                },
                PrimaryContent::Provider {
                    provider_id: next_id,
                    card: next_card,
                    buy_credits: next_buy,
                },
            ) => {
                if prev_id != next_id || prev_buy != next_buy {
                    return false;
                }
                let Some(first) = cards.first() else {
                    return false;
                };
                if require_same_height && !first.view.has_compatible_height(next_card, width, false)
                {
                    return false;
                }
                if first.view.update(next_card, width) {
                    self.menu.itemChanged(&first.item);
                }
                true
            }
            (
                PrimaryContent::Overview {
                    spend_summary: prev_summary,
                    share_action: prev_share,
                    cards: prev_cards,
                },
                PrimaryContent::Overview {
                    spend_summary: next_summary,
                    share_action: next_share,
                    cards: next_cards,
                },
            ) => {
                let expected_card_views = next_cards.len() + usize::from(next_summary.is_some());
                if prev_summary.is_some() != next_summary.is_some()
                    || prev_share != next_share
                    || prev_cards.len() != next_cards.len()
                    || cards.len() != expected_card_views
                {
                    return false;
                }
                for (p, n) in prev_cards.iter().zip(next_cards.iter()) {
                    if p.provider_id != n.provider_id {
                        return false;
                    }
                }
                let provider_card_slice = if let Some(summary) = next_summary {
                    let Some(summary_card) = cards.first() else {
                        return false;
                    };
                    if require_same_height
                        && !summary_card
                            .view
                            .has_compatible_spend_summary_height(summary, width)
                    {
                        return false;
                    }
                    &cards[1..]
                } else {
                    &cards[..]
                };
                if require_same_height {
                    for (hc, ov) in provider_card_slice.iter().zip(next_cards.iter()) {
                        if !hc.view.has_compatible_height(&ov.model, width, true) {
                            return false;
                        }
                    }
                }
                if let Some(summary) = next_summary
                    && let Some(summary_card) = cards.first()
                    && summary_card.view.update_spend_summary(summary, width)
                {
                    self.menu.itemChanged(&summary_card.item);
                }
                for (hc, ov) in provider_card_slice.iter().zip(next_cards.iter()) {
                    if hc.view.update_overview(&ov.model, width) {
                        self.menu.itemChanged(&hc.item);
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn rebuild_body(&self, structure: &MenuStructure, width: f64) {
        let mtm = self.menu.mtm();

        // Keep the previous generation of CardViews alive until the next rebuild
        // in case this rebuild was triggered from inside `CardView::mouse_up`.
        {
            let mut retired = self.retired_cards.borrow_mut();
            retired.clear();
            for c in self.target.ivars().cards.borrow().iter() {
                retired.push(c.view.clone());
            }
        }

        while self.menu.numberOfItems() > 2 {
            self.menu.removeItemAtIndex(2);
        }

        let mut cards = Vec::new();
        let mut url_actions = Vec::new();
        let mut overview_providers = Vec::new();

        match &structure.primary {
            PrimaryContent::Disconnected { message } => {
                let status_item = item(mtm, message, None, "");
                status_item.setEnabled(false);
                self.menu.addItem(&status_item);
                self.menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            PrimaryContent::Provider {
                card, buy_credits, ..
            } => {
                let card_view = CardView::new(mtm);
                card_view.update(card, width);
                let card_item = NSMenuItem::new(mtm);
                card_item.setTitle(&NSString::from_str(""));
                card_item.setView(Some(&card_view));
                card_item.setEnabled(false);
                self.menu.addItem(&card_item);
                if let Some(buy) = buy_credits {
                    let buy_item = self.make_entry_item(mtm, buy, width, &mut url_actions);
                    self.menu.addItem(&buy_item);
                }
                self.menu.addItem(&NSMenuItem::separatorItem(mtm));
                cards.push(HighlightableCard {
                    item: card_item,
                    view: card_view,
                    allows_highlight: false,
                });
            }
            PrimaryContent::Overview {
                spend_summary,
                share_action,
                cards: overview_cards,
            } => {
                if let Some(summary) = spend_summary {
                    let summary_view = CardView::new(mtm);
                    summary_view.update_spend_summary(summary, width);
                    let summary_item = NSMenuItem::new(mtm);
                    summary_item.setTitle(&NSString::from_str(""));
                    summary_item.setView(Some(&summary_view));
                    summary_item.setEnabled(false);
                    self.menu.addItem(&summary_item);
                    cards.push(HighlightableCard {
                        item: summary_item,
                        view: summary_view,
                        allows_highlight: false,
                    });
                    if let Some(share) = share_action {
                        let share_item = self.make_entry_item(mtm, share, width, &mut url_actions);
                        self.menu.addItem(&share_item);
                    }
                    self.menu.addItem(&NSMenuItem::separatorItem(mtm));
                }
                for (index, ov) in overview_cards.iter().enumerate() {
                    if index > 0 {
                        self.menu.addItem(&NSMenuItem::separatorItem(mtm));
                    }
                    let card_view = CardView::new(mtm);
                    card_view.update_overview(&ov.model, width);
                    let provider_id = ov.provider_id.clone();
                    let on_select = self.on_select;
                    card_view.set_on_click(move || {
                        on_select(SwitcherSelection::Provider(provider_id.clone()));
                    });

                    let card_item = NSMenuItem::new(mtm);
                    card_item.setTitle(&NSString::from_str(""));
                    card_item.setView(Some(&card_view));
                    card_item.setEnabled(true);
                    card_item.setTag(index as isize);
                    // SAFETY: `self.target` lives as long as `MenuController`, which owns `self.menu`.
                    unsafe {
                        card_item.setTarget(Some(&self.target));
                        card_item.setAction(Some(sel!(selectOverviewRow:)));
                    }
                    self.menu.addItem(&card_item);
                    overview_providers.push(ov.provider_id.clone());
                    cards.push(HighlightableCard {
                        item: card_item,
                        view: card_view,
                        allows_highlight: true,
                    });
                }
                if !overview_cards.is_empty() {
                    self.menu.addItem(&NSMenuItem::separatorItem(mtm));
                }
            }
        }

        if !structure.history.is_empty() {
            for entry in &structure.history {
                let menu_item = self.make_entry_item(mtm, entry, width, &mut url_actions);
                self.menu.addItem(&menu_item);
            }
            self.menu.addItem(&NSMenuItem::separatorItem(mtm));
        }

        if !structure.actions.is_empty() {
            for entry in &structure.actions {
                let menu_item = self.make_entry_item(mtm, entry, width, &mut url_actions);
                self.menu.addItem(&menu_item);
            }
            self.menu.addItem(&NSMenuItem::separatorItem(mtm));
        }

        for entry in &structure.meta {
            let menu_item = self.make_entry_item(mtm, entry, width, &mut url_actions);
            self.menu.addItem(&menu_item);
        }

        let ivars = self.target.ivars();
        *ivars.cards.borrow_mut() = cards;
        *ivars.url_actions.borrow_mut() = url_actions;
        *ivars.overview_providers.borrow_mut() = overview_providers;
    }

    fn make_entry_item(
        &self,
        mtm: MainThreadMarker,
        entry: &MenuEntry,
        width: f64,
        url_actions: &mut Vec<String>,
    ) -> Retained<NSMenuItem> {
        match entry {
            MenuEntry::SecondaryText(text) => make_wrapped_secondary_text_item(mtm, text, width),
            MenuEntry::Action {
                title,
                system_image,
                key_equivalent,
                enabled,
                action,
            } => {
                let it = self.make_action_item(mtm, title, key_equivalent, action, url_actions);
                it.setEnabled(*enabled);
                if let Some(symbol) = system_image {
                    set_symbol_image(&it, symbol);
                }
                it
            }
            MenuEntry::Submenu {
                title,
                system_image,
                items,
            } => {
                let it = item(mtm, title, None, "");
                it.setEnabled(true);
                if let Some(symbol) = system_image {
                    set_symbol_image(&it, symbol);
                }
                let sub = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
                sub.setAutoenablesItems(false);
                for sub_entry in items {
                    match sub_entry {
                        SubmenuEntry::SecondaryText(text) => {
                            let child = item(mtm, text, None, "");
                            child.setEnabled(false);
                            sub.addItem(&child);
                        }
                        SubmenuEntry::Separator => {
                            sub.addItem(&NSMenuItem::separatorItem(mtm));
                        }
                        SubmenuEntry::Action {
                            title: child_title,
                            action,
                            enabled,
                        } => {
                            let child =
                                self.make_action_item(mtm, child_title, "", action, url_actions);
                            child.setEnabled(*enabled);
                            sub.addItem(&child);
                        }
                    }
                }
                it.setSubmenu(Some(&sub));
                it
            }
        }
    }

    fn make_action_item(
        &self,
        mtm: MainThreadMarker,
        title: &str,
        key: &str,
        action: &MenuAction,
        url_actions: &mut Vec<String>,
    ) -> Retained<NSMenuItem> {
        match action {
            MenuAction::Refresh => {
                let it = item(mtm, title, Some(sel!(refresh:)), key);
                // SAFETY: `self.target` lives as long as `MenuController`, which owns `self.menu`.
                unsafe { it.setTarget(Some(&self.target)) };
                it
            }
            MenuAction::Settings => {
                let it = item(mtm, title, Some(sel!(openSettings:)), key);
                // SAFETY: `self.target` lives as long as `MenuController`, which owns `self.menu`.
                unsafe { it.setTarget(Some(&self.target)) };
                it
            }
            MenuAction::ShareUsageSnapshot => {
                let it = item(mtm, title, Some(sel!(shareUsageSnapshot:)), key);
                // SAFETY: `self.target` lives as long as `MenuController`, which owns `self.menu`.
                unsafe { it.setTarget(Some(&self.target)) };
                it
            }
            MenuAction::AddAccount { .. } | MenuAction::SwitchAccount { .. } => {
                let it = item(mtm, title, Some(sel!(accountAction:)), key);
                // SAFETY: `self.target` lives as long as `MenuController`, which owns `self.menu`.
                unsafe { it.setTarget(Some(&self.target)) };
                it
            }
            MenuAction::BuyCredits { url }
            | MenuAction::OpenDashboard { url }
            | MenuAction::OpenStatusPage { url } => {
                let idx = url_actions.len();
                url_actions.push(url.clone());
                let it = item(mtm, title, Some(sel!(openUrlAction:)), key);
                it.setTag(idx as isize);
                // SAFETY: `self.target` lives as long as `MenuController`, which owns `self.menu`.
                unsafe { it.setTarget(Some(&self.target)) };
                it
            }
            MenuAction::About => {
                // A nil target sends these to NSApplication through the responder chain.
                item(mtm, title, Some(sel!(orderFrontStandardAboutPanel:)), key)
            }
            MenuAction::Quit => item(mtm, title, Some(sel!(terminate:)), key),
        }
    }
}

fn set_symbol_image(menu_item: &NSMenuItem, symbol_name: &str) {
    if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(symbol_name),
        None,
    ) {
        image.setTemplate(true);
        image.setSize(NSSize::new(16.0, 16.0));
        menu_item.setImage(Some(&image));
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

/// Measures the menu card width from standard (non-custom-view) menu items only,
/// floored at [`card_layout::MIN_CARD_WIDTH`] (`310.0` pt), matching
/// `StatusItemController.measuredMenuWidth(for:)` (`StatusItemController+MenuWidthCache.swift:67-87`).
fn measured_menu_width(mtm: MainThreadMarker, structure: &MenuStructure) -> f64 {
    let measuring = NSMenu::new(mtm);
    measuring.setAutoenablesItems(false);
    match &structure.primary {
        PrimaryContent::Disconnected { message } => {
            measuring.addItem(&item(mtm, message, None, ""));
        }
        PrimaryContent::Provider { buy_credits, .. } => {
            if let Some(MenuEntry::Action {
                title,
                system_image,
                key_equivalent,
                ..
            }) = buy_credits
            {
                let it = item(mtm, title, None, key_equivalent);
                if let Some(sym) = system_image {
                    set_symbol_image(&it, sym);
                }
                measuring.addItem(&it);
            }
        }
        PrimaryContent::Overview { share_action, .. } => {
            if let Some(MenuEntry::Action {
                title,
                system_image,
                key_equivalent,
                ..
            }) = share_action
            {
                let it = item(mtm, title, None, key_equivalent);
                if let Some(sym) = system_image {
                    set_symbol_image(&it, sym);
                }
                measuring.addItem(&it);
            }
        }
    }
    for entry in structure
        .history
        .iter()
        .chain(structure.actions.iter())
        .chain(structure.meta.iter())
    {
        match entry {
            // SecondaryText rows use `make_wrapped_secondary_text_item` (a custom view
            // constrained to `width`), so they are excluded from the standard item width probe
            // (`StatusItemController+MenuWidthCache.swift:69`).
            MenuEntry::SecondaryText(_) => {}
            MenuEntry::Action {
                title,
                system_image,
                key_equivalent,
                ..
            } => {
                let it = item(mtm, title, None, key_equivalent);
                if let Some(sym) = system_image {
                    set_symbol_image(&it, sym);
                }
                measuring.addItem(&it);
            }
            MenuEntry::Submenu {
                title,
                system_image,
                ..
            } => {
                let it = item(mtm, title, None, "");
                if let Some(sym) = system_image {
                    set_symbol_image(&it, sym);
                }
                measuring.addItem(&it);
            }
        }
    }
    card_layout::rendered_width(measuring.size().width)
}

/// Builds a disabled menu item whose custom view wraps secondary text inside `width`
/// (`StatusItemController.makeWrappedSecondaryTextItem`, `StatusItemController+MenuTextRows.swift:4-43`).
fn make_wrapped_secondary_text_item(
    mtm: MainThreadMarker,
    text: &str,
    width: f64,
) -> Retained<NSMenuItem> {
    let menu_item = item(mtm, "", None, "");
    menu_item.setEnabled(false);
    menu_item.setToolTip(Some(&NSString::from_str(text)));

    let horizontal_leading: f64 = 18.0;
    let horizontal_trailing: f64 = 10.0;
    let vertical_padding: f64 = 2.0;
    let available_width = (width - horizontal_leading - horizontal_trailing).max(120.0);

    let label = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    let font = NSFont::menuFontOfSize(NSFont::smallSystemFontSize());
    label.setFont(Some(&font));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    label.setSelectable(false);
    label.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    label.setMaximumNumberOfLines(0);
    label.setPreferredMaxLayoutWidth(available_width);
    label.setFrameSize(NSSize::new(available_width, 0.0));
    let label_height = label
        .cell()
        .map(|cell| {
            cell.cellSizeForBounds(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(available_width, 100_000.0),
            ))
            .height
            .ceil()
        })
        .unwrap_or_else(|| label.fittingSize().height.ceil());
    let row_height = (label_height + vertical_padding * 2.0).max(18.0);

    let container = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, row_height)),
    );
    label.setFrame(NSRect::new(
        NSPoint::new(horizontal_leading, vertical_padding),
        NSSize::new(available_width, label_height),
    ));
    container.addSubview(&label);
    menu_item.setView(Some(&container));
    menu_item
}
