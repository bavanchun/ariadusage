// Ported from CodexBar Sources/CodexBar/MenuCardRefreshMonitor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuOpenRefreshPlan.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuSessionCoordinator.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+Actions.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+Menu.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuCardItems.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuCardRecycling.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuReconcile.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuSmartUpdate.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuSwitcherWarmup.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuTextRows.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuTracking.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuViewportRestore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuWidthCache.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MergedSwitcherContentCache.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+OverviewSpend.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+PersistentRefreshMenuItem.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+ProviderSwitcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! The status item's dropdown menu.
//!
//! When more than one provider is enabled, the top row hosts [`SwitcherView`]
//! (`Overview` + one toggle per enabled provider), followed by a separator,
//! the active tab's primary content (Overview rows or the selected provider's
//! usage card), contextual provider actions and the meta section (`Refresh` ⌘R,
//! `Settings...` ⌘,, `About AriadUsage (vX.Y.Z)`, `Quit` ⌘Q).
//!
//! The switcher item stays attached at index `0` across tab switches and live
//! snapshot pushes so open-menu updates and tab switches happen in place
//! without flicker, without scroll jumps, and without closing open submenus.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use ariadusage_protocol::ids::ProviderId;
use ariadusage_protocol::snapshot::EngineSnapshot;
use dispatch2::DispatchQueue;
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
use super::chart_view::{CostHistoryChartView, PlanUtilizationChartView, QuotaBurndownChartView};
use super::date_texts::FoundationDateTexts;
use super::switcher_view::SwitcherView;
use crate::card::CardPreferences;
use crate::card_layout::{self, CardSectionMode};
use crate::engine_client::EngineClient;
use crate::menu_descriptor::{
    self, MenuAction, MenuEntry, MenuStructure, PrimaryContent, SubmenuEntry,
};
use crate::menu_session::{
    CachedMergedSwitcherMenuContent, MENU_OPEN_REFRESH_DELAY, MERGED_MENU_ID, ManualRefreshScope,
    MenuCardRefreshMonitor, MenuOpenRefreshPlan, MenuRebuildRequestRegistry, MenuRowShape,
    MenuSessionCoordinator, MergedSwitcherContentCache, ReconcilePlan, classify_snapshot_update,
    reconcile_menu_shapes, shape_for_entry,
};
use crate::settings::SettingsPane;
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

type UpdateCallback = Rc<dyn Fn()>;
type SelectCallback = Rc<dyn Fn(SwitcherSelection)>;
type OpenSettingsCallback = Rc<dyn Fn(Option<SettingsPane>)>;
type RefreshCallback = Rc<dyn Fn(Option<String>)>;

/// State held by the menu's action target and delegate.
pub struct TargetIvars {
    client: EngineClient,
    is_open: Cell<bool>,
    has_native_item_highlight: Cell<bool>,
    has_open_submenu: Cell<bool>,
    on_update: UpdateCallback,
    on_select: SelectCallback,
    on_open_settings: OpenSettingsCallback,
    on_refresh: RefCell<Option<RefreshCallback>>,
    manual_refresh_provider: RefCell<Option<String>>,
    open_refresh_requests: RefCell<MenuRebuildRequestRegistry>,
    open_refresh_delay: Cell<Duration>,
    open_refresh_plan_providers: RefCell<Vec<String>>,
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
            self.on_will_open();
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            self.on_did_close();
        }

        #[unsafe(method(menu:willHighlightItem:))]
        fn menu_will_highlight_item(&self, _menu: &NSMenu, item: Option<&NSMenuItem>) {
            self.on_will_highlight_item(item);
        }
    }

    impl MenuTarget {
        #[unsafe(method(refresh:))]
        fn refresh(&self, _sender: Option<&AnyObject>) {
            let provider_id = self.ivars().manual_refresh_provider.borrow().clone();
            let cb = self.ivars().on_refresh.borrow().clone();
            if let Some(cb) = cb {
                cb(provider_id);
            } else {
                let pid = provider_id.and_then(|s| ProviderId::new(s).ok());
                self.ivars().client.refresh(pid);
            }
        }

        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: Option<&AnyObject>) {
            (self.ivars().on_open_settings)(None);
        }

        #[unsafe(method(openAbout:))]
        fn open_about(&self, _sender: Option<&AnyObject>) {
            (self.ivars().on_open_settings)(Some(SettingsPane::About));
        }

        #[unsafe(method(accountAction:))]
        fn account_action(&self, _sender: Option<&AnyObject>) {
            // Account switch/login flow lands with provider settings panes; documented in docs/porting.md.
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

    fn on_will_open(&self) {
        let ivars = self.ivars();
        ivars.has_native_item_highlight.set(false);
        ivars.has_open_submenu.set(false);
        // Rebuild before marking the menu open so relative times, countdowns
        // and any deferred structural changes are applied before tracking starts.
        (ivars.on_update)();
        ivars.is_open.set(true);

        // Schedule `MenuOpenRefreshPlan` after `open_refresh_delay` (`MENU_OPEN_REFRESH_DELAY`,
        // 1.2s) if any stale or errored providers need refreshing (`StatusItemController+Menu.swift`).
        let token = ivars
            .open_refresh_requests
            .borrow_mut()
            .begin(MERGED_MENU_ID);
        let providers = ivars.open_refresh_plan_providers.borrow().clone();
        if !providers.is_empty() {
            let delay = ivars.open_refresh_delay.get();
            // SAFETY: `self` is a main-thread `Retained<MenuTarget>` object; we retain a pointer
            // that is only dereferenced on `DispatchQueue::main()`.
            let target_retained: Retained<MenuTarget> =
                unsafe { Retained::retain(self as *const MenuTarget as *mut MenuTarget).unwrap() };
            let target_addr = Retained::into_raw(target_retained) as usize;
            let when = dispatch2::DispatchTime::NOW.time(delay.as_nanos() as i64);
            let _ = DispatchQueue::main().after(when, move || {
                // SAFETY: We are on `DispatchQueue::main()` and `target_addr` came from `Retained::into_raw`.
                let target: Retained<MenuTarget> =
                    unsafe { Retained::from_raw(target_addr as *mut MenuTarget).unwrap() };
                let ivars = target.ivars();
                if !ivars.is_open.get()
                    || !ivars
                        .open_refresh_requests
                        .borrow_mut()
                        .complete(MERGED_MENU_ID, token)
                {
                    return;
                }
                let on_refresh = ivars.on_refresh.borrow().clone();
                let client = ivars.client.clone();
                for provider_id in providers {
                    if let Some(cb) = &on_refresh {
                        cb(Some(provider_id));
                    } else if let Ok(pid) = ProviderId::new(provider_id) {
                        client.refresh(Some(pid));
                    }
                }
            });
        }
    }

    fn on_did_close(&self) {
        let ivars = self.ivars();
        ivars.is_open.set(false);
        ivars.has_native_item_highlight.set(false);
        ivars.has_open_submenu.set(false);
        ivars
            .open_refresh_requests
            .borrow_mut()
            .cancel(MERGED_MENU_ID);
        for card in ivars.cards.borrow().iter() {
            card.view.set_highlighted(false);
        }
        (ivars.on_update)();
    }

    fn on_will_highlight_item(&self, item: Option<&NSMenuItem>) {
        let ivars = self.ivars();
        let mut hovered_card = false;
        for card in ivars.cards.borrow().iter() {
            let is_hovered = item.is_some_and(|hi| std::ptr::eq(hi, &*card.item));
            if is_hovered {
                hovered_card = true;
            }
            card.view.set_highlighted(card_highlighted(
                is_hovered,
                card.item.isEnabled(),
                card.allows_highlight,
            ));
        }
        let was_deferring = ivars.has_native_item_highlight.get() || ivars.has_open_submenu.get();
        let is_native = item.is_some() && !hovered_card;
        let has_submenu = item.is_some_and(|hi| hi.hasSubmenu());
        ivars.has_native_item_highlight.set(is_native);
        ivars.has_open_submenu.set(has_submenu);
        let now_deferring = is_native || has_submenu;
        if was_deferring && !now_deferring && ivars.is_open.get() {
            (ivars.on_update)();
        }
    }
}

/// Owns the menu and updates it from snapshots.
pub struct MenuController {
    menu: Retained<NSMenu>,
    target: Retained<MenuTarget>,
    client: EngineClient,
    switcher_item: Retained<NSMenuItem>,
    switcher_view: Retained<SwitcherView>,
    switcher_sep: Retained<NSMenuItem>,
    refresh_item: Retained<NSMenuItem>,
    on_select: SelectCallback,
    dates: FoundationDateTexts,
    prefs: CardPreferences,
    session: RefCell<MenuSessionCoordinator>,
    refresh_monitor: RefCell<MenuCardRefreshMonitor>,
    switcher_cache: RefCell<MergedSwitcherContentCache>,
    last_snapshot: RefCell<Option<EngineSnapshot>>,
    last_connected: Cell<bool>,
    last_structure: RefCell<Option<MenuStructure>>,
    history_items: RefCell<Vec<Retained<NSMenuItem>>>,
    action_items: RefCell<Vec<Retained<NSMenuItem>>>,
    meta_items: RefCell<Vec<Retained<NSMenuItem>>>,
    recycled_cards: RefCell<Vec<Retained<CardView>>>,
    retired_cards: RefCell<Vec<Retained<CardView>>>,
    body_rebuild_count: Cell<usize>,
    in_place_update_count: Cell<usize>,
    middle_reconcile_count: Cell<usize>,
}

impl MenuController {
    /// Builds the menu. `on_update` runs when AppKit prepares, opens or closes
    /// the menu; `on_select` runs when the user selects a tab in the switcher
    /// or clicks a provider card on the Overview tab; `on_open_settings` opens
    /// the Settings window (optionally selecting a specific pane).
    pub fn new(
        mtm: MainThreadMarker,
        client: EngineClient,
        on_update: fn(),
        on_select: fn(SwitcherSelection),
        on_open_settings: fn(Option<SettingsPane>),
    ) -> Self {
        Self::new_with_callbacks(mtm, client, on_update, on_select, on_open_settings)
    }

    /// Builds the menu with boxed callbacks (used by [`Self::new`] and integration tests).
    pub fn new_with_callbacks(
        mtm: MainThreadMarker,
        client: EngineClient,
        on_update: impl Fn() + 'static,
        on_select: impl Fn(SwitcherSelection) + 'static,
        on_open_settings: impl Fn(Option<SettingsPane>) + 'static,
    ) -> Self {
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);

        let on_update_rc: UpdateCallback = Rc::new(on_update);
        let on_select_rc: SelectCallback = Rc::new(on_select);
        let on_open_settings_rc: OpenSettingsCallback = Rc::new(on_open_settings);

        let switcher_view = SwitcherView::new(mtm);
        {
            let on_select_cb = on_select_rc.clone();
            switcher_view.set_on_select(move |sel| on_select_cb(sel));
        }
        {
            let on_update_cb = on_update_rc.clone();
            switcher_view.set_on_pointer_interaction(move |active| {
                if !active {
                    on_update_cb();
                }
            });
        }
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
                client: client.clone(),
                is_open: Cell::new(false),
                has_native_item_highlight: Cell::new(false),
                has_open_submenu: Cell::new(false),
                on_update: on_update_rc,
                on_select: on_select_rc.clone(),
                on_open_settings: on_open_settings_rc,
                on_refresh: RefCell::new(None),
                manual_refresh_provider: RefCell::new(None),
                open_refresh_requests: RefCell::new(MenuRebuildRequestRegistry::new()),
                open_refresh_delay: Cell::new(MENU_OPEN_REFRESH_DELAY),
                open_refresh_plan_providers: RefCell::new(Vec::new()),
                cards: RefCell::new(Vec::new()),
                url_actions: RefCell::new(Vec::new()),
                overview_providers: RefCell::new(Vec::new()),
            },
        );
        menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));

        // Persistent Refresh menu item (`StatusItemController+PersistentRefreshMenuItem.swift`).
        let refresh_item = item(mtm, "Refresh", Some(sel!(refresh:)), "r");
        set_symbol_image(&refresh_item, "arrow.clockwise");
        // SAFETY: `target` lives as long as `MenuController`, which owns `refresh_item`.
        unsafe { refresh_item.setTarget(Some(&target)) };

        menu.addItem(&switcher_item);
        menu.addItem(&switcher_sep);

        Self {
            menu,
            target,
            client,
            switcher_item,
            switcher_view,
            switcher_sep,
            refresh_item,
            on_select: on_select_rc,
            dates: FoundationDateTexts::new(),
            prefs: CardPreferences::default(),
            session: RefCell::new(MenuSessionCoordinator::new()),
            refresh_monitor: RefCell::new(MenuCardRefreshMonitor::new()),
            switcher_cache: RefCell::new(MergedSwitcherContentCache::new()),
            last_snapshot: RefCell::new(None),
            last_connected: Cell::new(false),
            last_structure: RefCell::new(None),
            history_items: RefCell::new(Vec::new()),
            action_items: RefCell::new(Vec::new()),
            meta_items: RefCell::new(Vec::new()),
            recycled_cards: RefCell::new(Vec::new()),
            retired_cards: RefCell::new(Vec::new()),
            body_rebuild_count: Cell::new(0),
            in_place_update_count: Cell::new(0),
            middle_reconcile_count: Cell::new(0),
        }
    }

    /// Registers a custom callback invoked when the `"Refresh"` menu item is triggered.
    pub fn set_on_refresh(&self, callback: impl Fn(Option<String>) + 'static) {
        *self.target.ivars().on_refresh.borrow_mut() = Some(Rc::new(callback));
    }

    pub fn set_prefs(&mut self, prefs: CardPreferences) {
        if self.prefs != prefs {
            self.prefs = prefs;
            self.switcher_cache.borrow_mut().invalidate();
        }
    }

    pub fn menu(&self) -> &NSMenu {
        &self.menu
    }

    /// The top provider switcher view.
    pub fn switcher_view(&self) -> &SwitcherView {
        &self.switcher_view
    }

    /// The persistent `"Refresh"` menu item (`StatusItemController+PersistentRefreshMenuItem.swift`).
    pub fn refresh_item(&self) -> &NSMenuItem {
        &self.refresh_item
    }

    /// Currently attached [`CardView`] instances in menu order.
    pub fn card_views(&self) -> Vec<Retained<CardView>> {
        self.target
            .ivars()
            .cards
            .borrow()
            .iter()
            .map(|c| c.view.clone())
            .collect()
    }

    /// Currently attached card [`NSMenuItem`]s in menu order.
    pub fn card_items(&self) -> Vec<Retained<NSMenuItem>> {
        self.target
            .ivars()
            .cards
            .borrow()
            .iter()
            .map(|c| c.item.clone())
            .collect()
    }

    /// Currently attached history section [`NSMenuItem`]s (`Usage` / `Cost` submenus).
    pub fn history_items(&self) -> Vec<Retained<NSMenuItem>> {
        self.history_items.borrow().clone()
    }

    /// Currently attached contextual action [`NSMenuItem`]s (including `"Updated ..."` freshness text).
    pub fn action_items(&self) -> Vec<Retained<NSMenuItem>> {
        self.action_items.borrow().clone()
    }

    /// Currently attached meta [`NSMenuItem`]s (`Refresh`, `Settings...`, `About`, `Quit`).
    pub fn meta_items(&self) -> Vec<Retained<NSMenuItem>> {
        self.meta_items.borrow().clone()
    }

    /// Last applied [`MenuStructure`], if any.
    pub fn last_structure(&self) -> Option<MenuStructure> {
        self.last_structure.borrow().clone()
    }

    /// Number of full menu body rebuilds performed since creation.
    pub fn body_rebuild_count(&self) -> usize {
        self.body_rebuild_count.get()
    }

    /// Number of in-place menu updates performed without mutating the menu item list.
    pub fn in_place_update_count(&self) -> usize {
        self.in_place_update_count.get()
    }

    /// Number of middle-slice menu reconciles performed while preserving top and meta items.
    pub fn middle_reconcile_count(&self) -> usize {
        self.middle_reconcile_count.get()
    }

    /// Number of pre-warmed tab structures currently held in the merged switcher cache.
    pub fn cached_tab_count(&self) -> usize {
        self.switcher_cache.borrow().len()
    }

    /// Number of sibling tab warmup builds performed by [`MergedSwitcherContentCache`].
    pub fn warmup_build_count(&self) -> usize {
        self.switcher_cache.borrow().warmup_build_count()
    }

    /// Cached `(selection, required_menu_content_version)` entries currently in the switcher cache.
    pub fn cached_tab_versions(&self) -> Vec<(SwitcherSelection, u64)> {
        self.switcher_cache
            .borrow()
            .cached_entries()
            .into_iter()
            .map(|e| (e.selection, e.required_menu_content_version))
            .collect()
    }

    /// Overrides the delay before `MenuOpenRefreshPlan` fires after `menuWillOpen:`
    /// (defaults to [`MENU_OPEN_REFRESH_DELAY`], `1.2s`).
    pub fn set_open_refresh_delay(&self, delay: Duration) {
        self.target.ivars().open_refresh_delay.set(delay);
    }

    /// Returns the provider ID that the persistent `"Refresh"` menu item currently targets
    /// (`Some(id)` on a provider tab, `None` on Overview).
    pub fn current_refresh_provider(&self) -> Option<String> {
        self.target.ivars().manual_refresh_provider.borrow().clone()
    }

    /// Invokes the persistent `"Refresh"` menu item's native Objective-C action (`refresh:`)
    /// on its target (`MenuTarget`).
    pub fn invoke_refresh_action(&self) {
        // SAFETY: `self.target` is a valid `MenuTarget` and `self.refresh_item` is a valid `NSMenuItem`.
        let _: () = unsafe { msg_send![&*self.target, refresh: &*self.refresh_item] };
    }

    /// Invalidates menu content versions (`StatusItemController.invalidateMenus`).
    pub fn invalidate_menus(&self, allows_stale_content: bool, requires_rebuild: bool) {
        self.session
            .borrow_mut()
            .invalidate(allows_stale_content, requires_rebuild);
        self.switcher_cache.borrow_mut().invalidate();
    }

    /// Pre-warms non-selected sibling switcher tabs into the merged switcher cache.
    pub fn warm_sibling_content(
        &self,
        snapshot: &EngineSnapshot,
        connected: bool,
        active_selection: &SwitcherSelection,
    ) -> Vec<SwitcherSelection> {
        let now = effective_now(Some(snapshot));
        let req_version = self.session.borrow().required_rebuild_version();
        let width = self
            .last_structure
            .borrow()
            .as_ref()
            .map(|s| measured_menu_width(self.menu.mtm(), s))
            .unwrap_or(card_layout::MIN_CARD_WIDTH);
        let monitor = self.refresh_monitor.borrow();
        self.switcher_cache.borrow_mut().warm_sibling_content(
            snapshot,
            connected,
            active_selection,
            req_version,
            width,
            &self.prefs,
            now,
            &self.dates,
            Some(&monitor),
        )
    }

    /// Whether a parent menu rebuild was deferred during an open tracking session.
    pub fn has_deferred_parent_rebuild(&self) -> bool {
        self.session
            .borrow()
            .has_deferred_parent_rebuild(MERGED_MENU_ID)
    }

    /// Whether a menu rebuild is deferred until the next `menuWillOpen:`.
    pub fn is_deferred_until_next_open(&self) -> bool {
        self.session
            .borrow()
            .is_deferred_until_next_open(MERGED_MENU_ID)
    }

    /// Consumes and returns whether a viewport scroll restore is needed after an open-menu reconcile.
    pub fn consume_viewport_restore(&self) -> bool {
        self.session
            .borrow_mut()
            .consume_viewport_restore(MERGED_MENU_ID)
    }

    /// Current menu content version in [`MenuSessionCoordinator`].
    pub fn content_version(&self) -> u64 {
        self.session.borrow().current_content_version()
    }

    /// Required structural rebuild version in [`MenuSessionCoordinator`].
    pub fn required_rebuild_version(&self) -> u64 {
        self.session.borrow().required_rebuild_version()
    }

    /// Last rendered version recorded for the merged menu in [`MenuSessionCoordinator`].
    pub fn rendered_version(&self) -> Option<u64> {
        self.session.borrow().rendered_version(MERGED_MENU_ID)
    }

    /// Whether a manual refresh is currently in flight.
    pub fn is_manual_refresh_active(&self) -> bool {
        self.refresh_monitor.borrow().is_refreshing()
    }

    /// Returns the set of provider IDs currently refreshing (either via a user-initiated
    /// manual refresh or engine-reported loading state).
    pub fn refreshing_providers(
        &self,
        snapshot: Option<&EngineSnapshot>,
    ) -> std::collections::BTreeSet<String> {
        self.refresh_monitor.borrow().refreshing_providers(snapshot)
    }

    /// Effective rendered width of the menu (`StatusItemController.renderedMenuWidth`,
    /// `StatusItemController+MenuTracking.swift:295-319`).
    pub fn rendered_menu_width(&self) -> f64 {
        card_layout::rendered_width(self.menu.size().width.max(self.menu.minimumWidth()))
    }

    /// Returns a snapshot of the currently cached switcher tab entries.
    pub fn cached_switcher_entries(&self) -> Vec<CachedMergedSwitcherMenuContent> {
        self.switcher_cache.borrow().cached_entries()
    }

    /// Clears in-flight manual refresh state on the refresh monitor.
    pub fn finish_manual_refresh(&self) {
        self.refresh_monitor.borrow_mut().clear();
        self.switcher_cache.borrow_mut().invalidate();
    }

    /// Resolves the effective [`crate::card::CardModel`] for `provider_id` through
    /// [`MenuCardRefreshMonitor::effective_card_model`] (`MenuCardRefreshMonitor.model(for:fallback:)`).
    pub fn effective_card_model(
        &self,
        provider_id: &str,
        fallback: crate::card::CardModel,
        snapshot: Option<&EngineSnapshot>,
    ) -> crate::card::CardModel {
        let monitor = self.refresh_monitor.borrow();
        let is_refreshing = monitor.is_provider_refreshing(provider_id, snapshot);
        monitor.effective_card_model(provider_id, fallback, is_refreshing)
    }

    /// Starts a user-initiated refresh (`FetchInteraction::UserInitiated`) for `provider_id`
    /// (`None` = global refresh, `Some(id)` = per-provider refresh), freezing the active
    /// card's bars, setting `"Refreshing…"` subtitle, disabling `"Refresh"` on the
    /// refreshing tab, and sending the IPC `Refresh` request (`StatusItemController+Actions.swift`).
    pub fn start_user_refresh(
        &self,
        provider_id: Option<String>,
        snapshot: Option<&EngineSnapshot>,
        connected: bool,
        selected_tab: Option<&SwitcherSelection>,
    ) -> Option<SwitcherSelection> {
        let now = effective_now(snapshot);
        let scope = match &provider_id {
            Some(id) => ManualRefreshScope::Provider(id.clone()),
            None => ManualRefreshScope::Global,
        };
        self.refresh_monitor.borrow_mut().begin_manual_refresh(
            scope,
            snapshot,
            &self.prefs,
            now,
            &self.dates,
        );
        *self.last_snapshot.borrow_mut() = snapshot.cloned();
        self.last_connected.set(connected);
        self.switcher_cache.borrow_mut().invalidate();
        let pid = provider_id.and_then(|s| ProviderId::new(s).ok());
        self.client.refresh(pid);
        self.update_internal(snapshot, connected, selected_tab, false)
    }

    /// Starts a user-initiated refresh using the most recently applied snapshot,
    /// connection state, and selected tab.
    pub fn trigger_refresh(&self, provider_id: Option<String>) -> Option<SwitcherSelection> {
        let snapshot = self.last_snapshot.borrow().clone();
        let connected = self.last_connected.get();
        let selected_tab = self
            .last_structure
            .borrow()
            .as_ref()
            .and_then(|s| s.selection.clone());
        self.start_user_refresh(
            provider_id,
            snapshot.as_ref(),
            connected,
            selected_tab.as_ref(),
        )
    }

    /// Simulates `menuWillOpen:` for tests without entering the modal AppKit menu tracking loop.
    pub fn simulate_will_open(&self) {
        self.target.on_will_open();
    }

    /// Simulates `menuDidClose:` for tests.
    pub fn simulate_did_close(&self) {
        self.target.on_did_close();
    }

    /// Simulates `menu:willHighlightItem:` for tests.
    pub fn simulate_will_highlight_item(&self, item: Option<&NSMenuItem>) {
        self.target.on_will_highlight_item(item);
    }

    /// Whether the menu is currently open.
    pub fn is_open(&self) -> bool {
        self.target.ivars().is_open.get()
    }

    /// Reconciles the menu with `snapshot`, `connected` and `selected_tab`.
    /// Returns the resolved [`SwitcherSelection`], if any.
    pub fn update(
        &self,
        snapshot: Option<&EngineSnapshot>,
        connected: bool,
        selected_tab: Option<&SwitcherSelection>,
    ) -> Option<SwitcherSelection> {
        let snapshot_changed = {
            let prev = self.last_snapshot.borrow();
            prev.as_ref() != snapshot || self.last_connected.get() != connected
        };
        self.update_internal(snapshot, connected, selected_tab, snapshot_changed)
    }

    fn update_internal(
        &self,
        snapshot: Option<&EngineSnapshot>,
        connected: bool,
        selected_tab: Option<&SwitcherSelection>,
        snapshot_changed: bool,
    ) -> Option<SwitcherSelection> {
        let now = effective_now(snapshot);

        if snapshot_changed {
            let delta = match snapshot {
                Some(next) if self.last_connected.get() == connected => {
                    classify_snapshot_update(self.last_snapshot.borrow().as_ref(), next)
                }
                _ => crate::menu_session::SnapshotDelta {
                    allows_stale_content: false,
                    requires_rebuild: true,
                },
            };
            self.session
                .borrow_mut()
                .invalidate(delta.allows_stale_content, delta.requires_rebuild);
            self.switcher_cache.borrow_mut().invalidate();
            if !connected {
                self.refresh_monitor.borrow_mut().clear();
            } else if let Some(snap) = snapshot {
                self.refresh_monitor.borrow_mut().reconcile_snapshot(
                    snap,
                    &self.prefs,
                    now,
                    &self.dates,
                );
            }
            *self.last_snapshot.borrow_mut() = snapshot.cloned();
            self.last_connected.set(connected);
        }

        let is_open = self.target.ivars().is_open.get();
        if is_open {
            self.session
                .borrow_mut()
                .begin_interaction_session(MERGED_MENU_ID);
        } else {
            self.session
                .borrow_mut()
                .end_interaction_session(MERGED_MENU_ID);
        }

        let structure = {
            let monitor = self.refresh_monitor.borrow();
            menu_descriptor::build_with_refresh(
                snapshot,
                connected,
                selected_tab,
                &self.prefs,
                now,
                &self.dates,
                Some(&monitor),
            )
        };
        let resolved = structure.selection.clone();
        if let Some(snap) = snapshot {
            let refreshing = self
                .refresh_monitor
                .borrow()
                .refreshing_providers(Some(snap));
            let plan = MenuOpenRefreshPlan::from_snapshot(
                snap,
                resolved.as_ref().or(selected_tab),
                &refreshing,
                false,
            );
            *self.target.ivars().open_refresh_plan_providers.borrow_mut() = plan.providers;
        } else {
            self.target
                .ivars()
                .open_refresh_plan_providers
                .borrow_mut()
                .clear();
        }
        let width = measured_menu_width(self.menu.mtm(), &structure).max(self.menu.minimumWidth());
        let req_version = self.session.borrow().required_rebuild_version();

        if let Some(sel) = &resolved {
            self.switcher_cache
                .borrow_mut()
                .insert(CachedMergedSwitcherMenuContent {
                    selection: sel.clone(),
                    required_menu_content_version: req_version,
                    menu_width: width,
                    prefs: self.prefs.clone(),
                    structure: structure.clone(),
                    has_blank_padding_rows: false,
                });
            if is_open
                && let Some(snap) = snapshot
                && !structure.switcher.is_empty()
            {
                let monitor = self.refresh_monitor.borrow();
                self.switcher_cache.borrow_mut().warm_sibling_content(
                    snap,
                    connected,
                    sel,
                    req_version,
                    width,
                    &self.prefs,
                    now,
                    &self.dates,
                    Some(&monitor),
                );
            }
        }

        self.apply_structure(structure, width);
        resolved
    }

    fn should_defer_open_menu_rebuild(&self) -> bool {
        let ivars = self.target.ivars();
        self.switcher_view.is_pointer_interaction_active()
            || ivars.has_native_item_highlight.get()
            || ivars.has_open_submenu.get()
    }

    fn apply_structure(&self, structure: MenuStructure, width: f64) {
        let is_open = self.target.ivars().is_open.get();
        let same_selection = self
            .last_structure
            .borrow()
            .as_ref()
            .is_some_and(|prev| prev.selection == structure.selection);

        let refresh_provider = match &structure.selection {
            Some(SwitcherSelection::Provider(id)) => Some(id.clone()),
            Some(SwitcherSelection::Overview) => None,
            None => match &structure.primary {
                PrimaryContent::Provider { provider_id, .. } => Some(provider_id.clone()),
                _ => None,
            },
        };
        *self.target.ivars().manual_refresh_provider.borrow_mut() = refresh_provider;

        // When the user is actively pressing a switcher button while the menu is open,
        // defer background menu updates until mouseUp finishes (`StatusMenuSwitcherTrackingTests`).
        if is_open && same_selection && self.switcher_view.is_pointer_interaction_active() {
            self.session
                .borrow_mut()
                .defer_parent_rebuild(MERGED_MENU_ID);
            return;
        }

        // Update the top switcher view in place if visible.
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
        } else if !is_open || !same_selection {
            self.switcher_item.setHidden(true);
            self.switcher_sep.setHidden(true);
        }

        let menu_items_attached = self.menu.numberOfItems() >= 3;

        // Fast path: update existing CardViews, submenus, secondary text rows, and actions in place
        // without adding or removing any NSMenuItem on `self.menu`.
        if menu_items_attached
            && self.try_update_in_place(&structure, width, is_open && same_selection)
        {
            self.in_place_update_count
                .set(self.in_place_update_count.get() + 1);
            self.session
                .borrow_mut()
                .mark_rendered(MERGED_MENU_ID, None);
            *self.last_structure.borrow_mut() = Some(structure);
            return;
        }

        // If the menu is open, reconcile only the changed middle (history/actions) rows while
        // keeping primary cards and meta rows intact when possible (`StatusItemController+MenuReconcile.swift`).
        if is_open && menu_items_attached {
            if same_selection {
                if !self.should_defer_open_menu_rebuild()
                    && self.try_reconcile_open_menu_middle(&structure, width, true)
                {
                    self.middle_reconcile_count
                        .set(self.middle_reconcile_count.get() + 1);
                    let mut sess = self.session.borrow_mut();
                    sess.mark_viewport_restore_needed(MERGED_MENU_ID);
                    sess.mark_rendered(MERGED_MENU_ID, None);
                    drop(sess);
                    *self.last_structure.borrow_mut() = Some(structure);
                    return;
                }
                // Incompatible card height or active native item highlight / open submenu:
                // update open submenus and the persistent Refresh item in place and defer
                // the structural rebuild until the menu closes or highlight clears.
                self.update_open_submenus_and_refresh_only(&structure);
                let mut sess = self.session.borrow_mut();
                sess.defer_parent_rebuild(MERGED_MENU_ID);
                sess.defer_until_next_open(MERGED_MENU_ID);
                return;
            }
            // User switched provider tabs while the menu is open: keep the primary card
            // item attached at index 2 and reconcile only the middle section (`StatusMenuSwitcherRefreshTests.swift:185`).
            if self.try_reconcile_open_menu_middle(&structure, width, false) {
                self.middle_reconcile_count
                    .set(self.middle_reconcile_count.get() + 1);
                let mut sess = self.session.borrow_mut();
                sess.mark_viewport_restore_needed(MERGED_MENU_ID);
                sess.mark_rendered(MERGED_MENU_ID, None);
                drop(sess);
                *self.last_structure.borrow_mut() = Some(structure);
                return;
            }
        }

        self.rebuild_body(&structure, width);
        self.body_rebuild_count
            .set(self.body_rebuild_count.get() + 1);
        let mut sess = self.session.borrow_mut();
        let _ = sess.consume_viewport_restore(MERGED_MENU_ID);
        sess.mark_rendered(MERGED_MENU_ID, None);
        drop(sess);
        *self.last_structure.borrow_mut() = Some(structure);
    }

    /// Updates existing [`CardView`](s), chart submenus, secondary text rows, and
    /// action items in place without mutating `self.menu`'s item list when the
    /// row shapes match (`MenuRowShape::can_update_in_place_to`). When
    /// `require_same_height` is true (`is_open && same_selection`), rejects
    /// the update if any card's layout height would change beyond tolerance.
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
        if !same_entry_shapes(&prev.history, &next.history)
            || !same_entry_shapes(&prev.actions, &next.actions)
            || !same_entry_shapes(&prev.meta, &next.meta)
        {
            return false;
        }
        let history_items = self.history_items.borrow();
        let action_items = self.action_items.borrow();
        let meta_items = self.meta_items.borrow();
        if history_items.len() != next.history.len()
            || action_items.len() != next.actions.len()
            || meta_items.len() != next.meta.len()
        {
            return false;
        }

        let mut next_url_actions = Vec::new();
        if !self.try_update_primary_cards_in_place(
            &prev.primary,
            &next.primary,
            width,
            require_same_height,
            &mut next_url_actions,
        ) {
            return false;
        }

        for (item, entry) in history_items.iter().zip(next.history.iter()) {
            self.update_entry_item_in_place(item, entry, width, &mut next_url_actions);
        }
        for (item, entry) in action_items.iter().zip(next.actions.iter()) {
            self.update_entry_item_in_place(item, entry, width, &mut next_url_actions);
        }
        for (item, entry) in meta_items.iter().zip(next.meta.iter()) {
            self.update_entry_item_in_place(item, entry, width, &mut next_url_actions);
        }

        *self.target.ivars().url_actions.borrow_mut() = next_url_actions;
        true
    }

    fn try_update_primary_cards_in_place(
        &self,
        prev_primary: &PrimaryContent,
        next_primary: &PrimaryContent,
        width: f64,
        require_same_height: bool,
        url_actions: &mut Vec<String>,
    ) -> bool {
        let mut cards = self.target.ivars().cards.borrow_mut();
        match (prev_primary, next_primary) {
            (
                PrimaryContent::Provider {
                    sections: prev_sections,
                    buy_credits: prev_buy,
                    ..
                },
                PrimaryContent::Provider {
                    card: next_card,
                    sections: next_sections,
                    cost_submenu: next_cost_sub,
                    buy_credits: next_buy,
                    ..
                },
            ) => {
                if prev_sections.len() != next_sections.len()
                    || prev_buy != next_buy
                    || cards.len() != next_sections.len()
                {
                    return false;
                }
                for (ps, ns) in prev_sections.iter().zip(next_sections.iter()) {
                    let prev_has_sub = ps.has_cost_submenu;
                    let next_has_sub = ns.has_cost_submenu && !next_cost_sub.is_empty();
                    if require_same_height && (ps.mode != ns.mode || prev_has_sub != next_has_sub) {
                        return false;
                    }
                }
                if require_same_height {
                    for (hc, sec) in cards.iter().zip(next_sections.iter()) {
                        if !hc
                            .view
                            .has_compatible_section_height(next_card, sec.mode, width)
                        {
                            return false;
                        }
                    }
                }
                let mtm = self.menu.mtm();
                let mut buy_recorded = false;
                for (hc, sec) in cards.iter_mut().zip(next_sections.iter()) {
                    if hc.view.update_section(next_card, sec.mode, width) {
                        self.menu.itemChanged(&hc.item);
                    }
                    let next_has_sub = sec.has_cost_submenu && !next_cost_sub.is_empty();
                    if next_has_sub {
                        if let Some(sub) = hc.item.submenu() {
                            self.update_submenu_in_place(&sub, next_cost_sub, url_actions);
                        } else {
                            let sub = self.make_submenu_from_items(
                                mtm,
                                "Cost",
                                next_cost_sub,
                                url_actions,
                            );
                            hc.item.setSubmenu(Some(&sub));
                        }
                        hc.item.setEnabled(true);
                        hc.allows_highlight = true;
                    } else if hc.item.submenu().is_some() {
                        hc.item.setSubmenu(None);
                        hc.item.setEnabled(false);
                        hc.allows_highlight = false;
                    }
                    if matches!(sec.mode, CardSectionMode::CreditsOnly { .. })
                        && let Some(buy) = next_buy
                    {
                        record_entry_urls(buy, url_actions);
                        buy_recorded = true;
                    }
                }
                if !buy_recorded && let Some(buy) = next_buy {
                    record_entry_urls(buy, url_actions);
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
                    if p.provider_id != n.provider_id
                        || p.submenu.is_empty() != n.submenu.is_empty()
                    {
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
                if let Some(share) = next_share {
                    record_entry_urls(share, url_actions);
                }
                for (hc, ov) in provider_card_slice.iter().zip(next_cards.iter()) {
                    if hc.view.update_overview(&ov.model, width) {
                        self.menu.itemChanged(&hc.item);
                    }
                    if !ov.submenu.is_empty()
                        && let Some(sub) = hc.item.submenu()
                    {
                        self.update_submenu_in_place(&sub, &ov.submenu, url_actions);
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// Updates a provider tab's primary section during an open-menu tab switch while
    /// keeping the first card (`index 2`) attached in `self.menu` even when the number
    /// of secondary card sections or `Buy Credits...` rows differs between providers.
    fn reconcile_provider_primary_keeping_first_card(
        &self,
        next_primary: &PrimaryContent,
        width: f64,
        old_middle_start: isize,
        url_actions: &mut Vec<String>,
    ) -> Option<isize> {
        let PrimaryContent::Provider {
            card: next_card,
            sections: next_sections,
            cost_submenu: next_cost_sub,
            buy_credits: next_buy,
            ..
        } = next_primary
        else {
            return None;
        };
        let first_sec = *next_sections.first()?;
        let mtm = self.menu.mtm();

        let first_card = {
            let mut cards = self.target.ivars().cards.borrow_mut();
            if cards.is_empty() {
                return None;
            }
            let first = cards.remove(0);
            let mut recycled = self.recycled_cards.borrow_mut();
            for extra in cards.drain(..) {
                recycled.push(extra.view);
            }
            first
        };
        if self.menu.indexOfItem(&first_card.item) != 2 {
            self.target.ivars().cards.borrow_mut().push(first_card);
            return None;
        }

        // Remove old secondary primary items in `3..old_middle_start` while keeping index 2 attached.
        for _ in 3..old_middle_start {
            self.menu.removeItemAtIndex(3);
        }

        // Update index 2 (`first_card`) in place.
        if first_card
            .view
            .update_section(next_card, first_sec.mode, width)
        {
            self.menu.itemChanged(&first_card.item);
        }
        let mut first_card = first_card;
        let first_has_sub = first_sec.has_cost_submenu && !next_cost_sub.is_empty();
        if first_has_sub {
            if let Some(sub) = first_card.item.submenu() {
                self.update_submenu_in_place(&sub, next_cost_sub, url_actions);
            } else {
                let sub = self.make_submenu_from_items(mtm, "Cost", next_cost_sub, url_actions);
                first_card.item.setSubmenu(Some(&sub));
            }
            first_card.item.setEnabled(true);
            first_card.allows_highlight = true;
        } else if first_card.item.submenu().is_some() {
            first_card.item.setSubmenu(None);
            first_card.item.setEnabled(false);
            first_card.allows_highlight = false;
        }

        let mut new_cards = vec![first_card];
        let mut insert_idx: isize = 3;
        let mut buy_inserted = false;
        if matches!(first_sec.mode, CardSectionMode::CreditsOnly { .. })
            && let Some(buy) = next_buy
        {
            let buy_item = self.make_entry_item(mtm, buy, width, url_actions);
            self.menu.insertItem_atIndex(&buy_item, insert_idx);
            insert_idx += 1;
            buy_inserted = true;
        }

        for section in next_sections.iter().skip(1) {
            self.menu
                .insertItem_atIndex(&NSMenuItem::separatorItem(mtm), insert_idx);
            insert_idx += 1;

            let view = self.take_or_create_card_view(mtm);
            view.update_section(next_card, section.mode, width);
            let item = NSMenuItem::new(mtm);
            let has_sub = section.has_cost_submenu && !next_cost_sub.is_empty();
            item.setEnabled(has_sub);
            item.setView(Some(&view));
            if has_sub {
                let sub = self.make_submenu_from_items(mtm, "Cost", next_cost_sub, url_actions);
                item.setSubmenu(Some(&sub));
            }
            self.menu.insertItem_atIndex(&item, insert_idx);
            insert_idx += 1;
            new_cards.push(HighlightableCard {
                item,
                view,
                allows_highlight: has_sub,
            });

            if matches!(section.mode, CardSectionMode::CreditsOnly { .. })
                && let Some(buy) = next_buy
            {
                let buy_item = self.make_entry_item(mtm, buy, width, url_actions);
                self.menu.insertItem_atIndex(&buy_item, insert_idx);
                insert_idx += 1;
                buy_inserted = true;
            }
        }

        if !buy_inserted && let Some(buy) = next_buy {
            let buy_item = self.make_entry_item(mtm, buy, width, url_actions);
            self.menu.insertItem_atIndex(&buy_item, insert_idx);
            insert_idx += 1;
        }

        self.menu
            .insertItem_atIndex(&NSMenuItem::separatorItem(mtm), insert_idx);
        insert_idx += 1;

        *self.target.ivars().cards.borrow_mut() = new_cards;
        Some(insert_idx)
    }

    /// Reconciles only the middle (`history` and `actions`) section of an open menu
    /// using [`reconcile_menu_shapes`] so unchanged prefix and suffix middle rows
    /// stay attached and update in place (`StatusItemController+MenuReconcile.swift`).
    fn try_reconcile_open_menu_middle(
        &self,
        next: &MenuStructure,
        width: f64,
        require_same_height: bool,
    ) -> bool {
        let prev_borrow = self.last_structure.borrow();
        let Some(prev) = prev_borrow.as_ref() else {
            return false;
        };
        if !same_entry_shapes(&prev.meta, &next.meta) {
            return false;
        }

        // Locate where the first meta item currently sits in `self.menu`.
        let meta_items = self.meta_items.borrow();
        let Some(first_meta) = meta_items.first() else {
            return false;
        };
        let meta_start_index = self.menu.indexOfItem(first_meta);
        if meta_start_index < 2 {
            return false;
        }
        // Locate where the primary section ends (just after the separator following the last card).
        let last_card_index = {
            let cards = self.target.ivars().cards.borrow();
            let Some(last_card) = cards.last() else {
                return false;
            };
            self.menu.indexOfItem(&last_card.item)
        };
        if last_card_index < 2 {
            return false;
        }
        // Include any trailing primary separator (and optional Buy Credits row).
        let middle_start = {
            let mut idx = last_card_index + 1;
            while idx < meta_start_index {
                let Some(it) = self.menu.itemAtIndex(idx) else {
                    break;
                };
                idx += 1;
                if it.isSeparatorItem() {
                    break;
                }
            }
            idx
        };
        if middle_start > meta_start_index {
            return false;
        }

        let prev_slots = middle_slots_for_structure(prev);
        let next_slots = middle_slots_for_structure(next);
        if (meta_start_index - middle_start) as usize != prev_slots.len() {
            return false;
        }

        let mut next_url_actions = Vec::new();
        let middle_start = if self.try_update_primary_cards_in_place(
            &prev.primary,
            &next.primary,
            width,
            require_same_height,
            &mut next_url_actions,
        ) {
            middle_start
        } else if !require_same_height {
            let Some(new_middle_start) = self.reconcile_provider_primary_keeping_first_card(
                &next.primary,
                width,
                middle_start,
                &mut next_url_actions,
            ) else {
                return false;
            };
            new_middle_start
        } else {
            return false;
        };

        let middle_plan = reconcile_middle_slots(&prev_slots, &next_slots);

        // Remove only the changed span `middle_plan.remove_range` inside `[middle_start, meta_start_index)`.
        let remove_at = middle_start + middle_plan.remove_range.start as isize;
        for _ in 0..middle_plan.remove_range.len() {
            self.menu.removeItemAtIndex(remove_at);
        }

        // Insert only `middle_plan.insert_range` items at `remove_at`.
        let mtm = self.menu.mtm();
        let mut dummy_insert_urls = Vec::new();
        for (offset, slot_idx) in middle_plan.insert_range.clone().enumerate() {
            let new_item = match next_slots[slot_idx] {
                MiddleSlot::History(entry) | MiddleSlot::Action(entry) => {
                    self.make_entry_item(mtm, entry, width, &mut dummy_insert_urls)
                }
                MiddleSlot::Separator => NSMenuItem::separatorItem(mtm),
            };
            self.menu
                .insertItem_atIndex(&new_item, remove_at + offset as isize);
        }

        // Walk all middle slots in order to update kept prefix/suffix items in place,
        // record `next_url_actions` in menu order, and collect `history_items` / `action_items`.
        let mut new_history_items = Vec::with_capacity(next.history.len());
        let mut new_action_items = Vec::with_capacity(next.actions.len());
        for (idx, slot) in next_slots.iter().enumerate() {
            let Some(menu_item) = self.menu.itemAtIndex(middle_start + idx as isize) else {
                return false;
            };
            match *slot {
                MiddleSlot::History(entry) => {
                    self.update_entry_item_in_place(
                        &menu_item,
                        entry,
                        width,
                        &mut next_url_actions,
                    );
                    new_history_items.push(menu_item);
                }
                MiddleSlot::Action(entry) => {
                    self.update_entry_item_in_place(
                        &menu_item,
                        entry,
                        width,
                        &mut next_url_actions,
                    );
                    new_action_items.push(menu_item);
                }
                MiddleSlot::Separator => {}
            }
        }

        for (item, entry) in meta_items.iter().zip(next.meta.iter()) {
            self.update_entry_item_in_place(item, entry, width, &mut next_url_actions);
        }

        drop(meta_items);
        *self.history_items.borrow_mut() = new_history_items;
        *self.action_items.borrow_mut() = new_action_items;
        *self.target.ivars().url_actions.borrow_mut() = next_url_actions;
        true
    }

    /// Updates open submenus and the persistent `"Refresh"` item in place even when
    /// a parent menu structural rebuild is deferred.
    fn update_open_submenus_and_refresh_only(&self, next: &MenuStructure) {
        for entry in &next.meta {
            if let MenuEntry::Action {
                enabled,
                action: MenuAction::Refresh,
                ..
            } = entry
                && self.refresh_item.isEnabled() != *enabled
            {
                self.refresh_item.setEnabled(*enabled);
                self.menu.itemChanged(&self.refresh_item);
            }
        }
        let history_items = self.history_items.borrow();
        if history_items.len() == next.history.len() {
            let mut url_actions = self.target.ivars().url_actions.borrow_mut();
            for (item, entry) in history_items.iter().zip(next.history.iter()) {
                if let MenuEntry::Submenu { items, .. } = entry
                    && let Some(sub) = item.submenu()
                {
                    self.update_submenu_in_place(&sub, items, &mut url_actions);
                }
            }
        }
    }

    fn update_entry_item_in_place(
        &self,
        item: &NSMenuItem,
        entry: &MenuEntry,
        width: f64,
        url_actions: &mut Vec<String>,
    ) {
        match entry {
            MenuEntry::SecondaryText(text) => {
                if update_wrapped_secondary_text_item(item, text, width) {
                    self.menu.itemChanged(item);
                }
            }
            MenuEntry::Action {
                title,
                system_image,
                enabled,
                action,
                ..
            } => {
                let mut changed = false;
                let current_title = item.title().to_string();
                if current_title != *title {
                    item.setTitle(&NSString::from_str(title));
                    changed = true;
                }
                if item.isEnabled() != *enabled {
                    item.setEnabled(*enabled);
                    changed = true;
                }
                if let Some(symbol) = system_image {
                    set_symbol_image(item, symbol);
                }
                if let MenuAction::BuyCredits { url }
                | MenuAction::OpenDashboard { url }
                | MenuAction::OpenStatusPage { url } = action
                {
                    let idx = url_actions.len();
                    url_actions.push(url.clone());
                    item.setTag(idx as isize);
                }
                if changed {
                    self.menu.itemChanged(item);
                }
            }
            MenuEntry::Submenu {
                title,
                system_image,
                items,
            } => {
                if item.title().to_string() != *title {
                    item.setTitle(&NSString::from_str(title));
                    self.menu.itemChanged(item);
                }
                if let Some(symbol) = system_image {
                    set_symbol_image(item, symbol);
                }
                if let Some(sub) = item.submenu() {
                    self.update_submenu_in_place(&sub, items, url_actions);
                }
            }
        }
    }

    fn update_submenu_in_place(
        &self,
        sub: &NSMenu,
        items: &[SubmenuEntry],
        url_actions: &mut Vec<String>,
    ) {
        let count = sub.numberOfItems() as usize;
        if count == items.len() && submenu_shapes_compatible(sub, items) {
            for (idx, sub_entry) in items.iter().enumerate() {
                let Some(child) = sub.itemAtIndex(idx as isize) else {
                    continue;
                };
                match sub_entry {
                    SubmenuEntry::SecondaryText(text) => {
                        if child.title().to_string() != *text {
                            child.setTitle(&NSString::from_str(text));
                            sub.itemChanged(&child);
                        }
                    }
                    SubmenuEntry::Separator => {}
                    SubmenuEntry::Action {
                        title: child_title,
                        action,
                        enabled,
                    } => {
                        let mut changed = false;
                        if child.title().to_string() != *child_title {
                            child.setTitle(&NSString::from_str(child_title));
                            changed = true;
                        }
                        if child.isEnabled() != *enabled {
                            child.setEnabled(*enabled);
                            changed = true;
                        }
                        if let MenuAction::BuyCredits { url }
                        | MenuAction::OpenDashboard { url }
                        | MenuAction::OpenStatusPage { url } = action
                        {
                            let url_idx = url_actions.len();
                            url_actions.push(url.clone());
                            child.setTag(url_idx as isize);
                        }
                        if changed {
                            sub.itemChanged(&child);
                        }
                    }
                    SubmenuEntry::QuotaBurndownChart(model) => {
                        if let Some(view) = child.view()
                            && let Some(chart) = view.downcast_ref::<QuotaBurndownChartView>()
                            && chart.update((**model).clone())
                        {
                            sub.itemChanged(&child);
                        }
                    }
                    SubmenuEntry::PlanUtilizationChart(model) => {
                        if let Some(view) = child.view()
                            && let Some(chart) = view.downcast_ref::<PlanUtilizationChartView>()
                            && chart.update((**model).clone())
                        {
                            sub.itemChanged(&child);
                        }
                    }
                    SubmenuEntry::CostHistoryChart(model) => {
                        if let Some(view) = child.view()
                            && let Some(chart) = view.downcast_ref::<CostHistoryChartView>()
                            && chart.update((**model).clone())
                        {
                            sub.itemChanged(&child);
                        }
                    }
                }
            }
            return;
        }

        // Never call `removeAllItems()` on an open submenu while the user is tracking inside it.
        if self.target.ivars().is_open.get() && self.target.ivars().has_open_submenu.get() {
            let mut sess = self.session.borrow_mut();
            sess.defer_parent_rebuild(MERGED_MENU_ID);
            sess.defer_until_next_open(MERGED_MENU_ID);
            return;
        }

        // Fallback when closed: repopulate the same `NSMenu` instance so the parent
        // `NSMenuItem`'s submenu reference remains valid.
        let mtm = sub.mtm();
        sub.removeAllItems();
        self.populate_submenu_items(mtm, sub, items, url_actions);
    }

    fn take_or_create_card_view(&self, mtm: MainThreadMarker) -> Retained<CardView> {
        if let Some(card_view) = self.recycled_cards.borrow_mut().pop() {
            card_view.set_highlighted(false);
            card_view.set_on_click(|| {});
            return card_view;
        }
        CardView::new(mtm)
    }

    fn rebuild_body(&self, structure: &MenuStructure, width: f64) {
        let mtm = self.menu.mtm();

        // Move the previous generation of retired CardViews into the recycle pool
        // (`StatusItemController+MenuCardRecycling.swift`), while keeping the just-detached
        // generation alive in `retired_cards` until the next rebuild in case this rebuild
        // was triggered from inside `CardView::mouse_up`.
        {
            let mut recycled = self.recycled_cards.borrow_mut();
            let mut retired = self.retired_cards.borrow_mut();
            recycled.append(&mut *retired);
            for c in self.target.ivars().cards.borrow().iter() {
                retired.push(c.view.clone());
            }
        }

        if self.menu.numberOfItems() < 2 {
            self.menu.removeAllItems();
            self.menu.addItem(&self.switcher_item);
            self.menu.addItem(&self.switcher_sep);
        }
        while self.menu.numberOfItems() > 2 {
            self.menu.removeItemAtIndex(2);
        }

        let mut cards = Vec::new();
        let mut url_actions = Vec::new();
        let mut overview_providers = Vec::new();
        let mut history_items = Vec::new();
        let mut action_items = Vec::new();
        let mut meta_items = Vec::new();

        match &structure.primary {
            PrimaryContent::Disconnected { message } => {
                let status_item = item(mtm, message, None, "");
                status_item.setEnabled(false);
                self.menu.addItem(&status_item);
                self.menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            PrimaryContent::Provider {
                card,
                sections,
                cost_submenu,
                buy_credits,
                ..
            } => {
                let mut buy_inserted = false;
                for (sec_idx, section) in sections.iter().enumerate() {
                    if sec_idx > 0 {
                        self.menu.addItem(&NSMenuItem::separatorItem(mtm));
                    }
                    let card_view = self.take_or_create_card_view(mtm);
                    card_view.update_section(card, section.mode, width);
                    let card_item = NSMenuItem::new(mtm);
                    card_item.setTitle(&NSString::from_str(""));
                    card_item.setView(Some(&card_view));
                    let has_sub = section.has_cost_submenu && !cost_submenu.is_empty();
                    if has_sub {
                        let sub = self.make_submenu_from_items(
                            mtm,
                            "Cost",
                            cost_submenu,
                            &mut url_actions,
                        );
                        card_item.setSubmenu(Some(&sub));
                        card_item.setEnabled(true);
                    } else {
                        card_item.setEnabled(false);
                    }
                    self.menu.addItem(&card_item);
                    cards.push(HighlightableCard {
                        item: card_item,
                        view: card_view,
                        allows_highlight: has_sub,
                    });
                    if matches!(section.mode, CardSectionMode::CreditsOnly { .. })
                        && let Some(buy) = buy_credits
                    {
                        let buy_item = self.make_entry_item(mtm, buy, width, &mut url_actions);
                        self.menu.addItem(&buy_item);
                        buy_inserted = true;
                    }
                }
                if !buy_inserted && let Some(buy) = buy_credits {
                    let buy_item = self.make_entry_item(mtm, buy, width, &mut url_actions);
                    self.menu.addItem(&buy_item);
                }
                self.menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
            PrimaryContent::Overview {
                spend_summary,
                share_action,
                cards: overview_cards,
            } => {
                if let Some(summary) = spend_summary {
                    let summary_view = self.take_or_create_card_view(mtm);
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
                    let card_view = self.take_or_create_card_view(mtm);
                    card_view.update_overview(&ov.model, width);
                    let provider_id = ov.provider_id.clone();
                    let on_select = self.on_select.clone();
                    card_view.set_on_click(move || {
                        on_select(SwitcherSelection::Provider(provider_id.clone()));
                    });

                    let card_item = NSMenuItem::new(mtm);
                    card_item.setTitle(&NSString::from_str(""));
                    card_item.setView(Some(&card_view));
                    card_item.setEnabled(true);
                    card_item.setTag(index as isize);
                    if !ov.submenu.is_empty() {
                        let title = ov.submenu_title.as_deref().unwrap_or("Cost");
                        let sub =
                            self.make_submenu_from_items(mtm, title, &ov.submenu, &mut url_actions);
                        card_item.setSubmenu(Some(&sub));
                    }
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
                history_items.push(menu_item);
            }
            self.menu.addItem(&NSMenuItem::separatorItem(mtm));
        }

        if !structure.actions.is_empty() {
            for entry in &structure.actions {
                let menu_item = self.make_entry_item(mtm, entry, width, &mut url_actions);
                self.menu.addItem(&menu_item);
                action_items.push(menu_item);
            }
            self.menu.addItem(&NSMenuItem::separatorItem(mtm));
        }

        for entry in &structure.meta {
            let menu_item = self.make_entry_item(mtm, entry, width, &mut url_actions);
            self.menu.addItem(&menu_item);
            meta_items.push(menu_item);
        }

        let ivars = self.target.ivars();
        *ivars.cards.borrow_mut() = cards;
        *ivars.url_actions.borrow_mut() = url_actions;
        *ivars.overview_providers.borrow_mut() = overview_providers;
        *self.history_items.borrow_mut() = history_items;
        *self.action_items.borrow_mut() = action_items;
        *self.meta_items.borrow_mut() = meta_items;
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
                if matches!(action, MenuAction::Refresh) {
                    self.refresh_item.setTitle(&NSString::from_str(title));
                    self.refresh_item
                        .setKeyEquivalent(&NSString::from_str(key_equivalent));
                    self.refresh_item.setEnabled(*enabled);
                    if let Some(symbol) = system_image {
                        set_symbol_image(&self.refresh_item, symbol);
                    }
                    return self.refresh_item.clone();
                }
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
                let sub = self.make_submenu_from_items(mtm, title, items, url_actions);
                it.setSubmenu(Some(&sub));
                it
            }
        }
    }

    fn make_submenu_from_items(
        &self,
        mtm: MainThreadMarker,
        title: &str,
        items: &[SubmenuEntry],
        url_actions: &mut Vec<String>,
    ) -> Retained<NSMenu> {
        let sub = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
        sub.setAutoenablesItems(false);
        self.populate_submenu_items(mtm, &sub, items, url_actions);
        sub
    }

    fn populate_submenu_items(
        &self,
        mtm: MainThreadMarker,
        sub: &NSMenu,
        items: &[SubmenuEntry],
        url_actions: &mut Vec<String>,
    ) {
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
                    let child = self.make_action_item(mtm, child_title, "", action, url_actions);
                    child.setEnabled(*enabled);
                    sub.addItem(&child);
                }
                SubmenuEntry::QuotaBurndownChart(model) => {
                    let view = QuotaBurndownChartView::new(mtm, (**model).clone());
                    let child = NSMenuItem::new(mtm);
                    child.setTitle(&NSString::from_str(""));
                    child.setView(Some(&view));
                    child.setEnabled(false);
                    sub.addItem(&child);
                }
                SubmenuEntry::PlanUtilizationChart(model) => {
                    let view = PlanUtilizationChartView::new(mtm, (**model).clone());
                    let child = NSMenuItem::new(mtm);
                    child.setTitle(&NSString::from_str(""));
                    child.setView(Some(&view));
                    child.setEnabled(false);
                    sub.addItem(&child);
                }
                SubmenuEntry::CostHistoryChart(model) => {
                    let view = CostHistoryChartView::new(mtm, (**model).clone());
                    let child = NSMenuItem::new(mtm);
                    child.setTitle(&NSString::from_str(""));
                    child.setView(Some(&view));
                    child.setEnabled(false);
                    sub.addItem(&child);
                }
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
                let it = item(mtm, title, Some(sel!(openAbout:)), key);
                // SAFETY: `self.target` lives as long as `MenuController`, which owns `self.menu`.
                unsafe { it.setTarget(Some(&self.target)) };
                it
            }
            MenuAction::Quit => item(mtm, title, Some(sel!(terminate:)), key),
        }
    }

    /// Reconciles `menu` starting at `from_index` against `scratch` (which holds
    /// the items starting at `from_index` in the desired layout) using [`reconcile_menu_shapes`]
    /// (`StatusItemController.reconcileMenuContent`, `StatusItemController+MenuReconcile.swift:30-77`).
    pub fn reconcile_menu_from_scratch(
        menu: &NSMenu,
        from_index: usize,
        existing_shapes: &[MenuRowShape],
        desired_shapes: &[MenuRowShape],
        scratch: &NSMenu,
    ) -> ReconcilePlan {
        let plan = reconcile_menu_shapes(existing_shapes, desired_shapes);

        for i in 0..plan.prefix_count {
            if let (Some(existing), Some(desired)) = (
                menu.itemAtIndex((from_index + i) as isize),
                scratch.itemAtIndex(i as isize),
            ) {
                sync_menu_item_from_desired(menu, &existing, &desired);
            }
        }

        let remove_at = (from_index + plan.remove_range.start) as isize;
        for _ in 0..plan.remove_range.len() {
            menu.removeItemAtIndex(remove_at);
        }

        for (offset, scratch_idx) in plan.insert_range.clone().enumerate() {
            if let Some(desired) = scratch.itemAtIndex(scratch_idx as isize) {
                scratch.removeItem(&desired);
                // Maintain `scratch` indexing by inserting a placeholder at `scratch_idx`.
                let placeholder = NSMenuItem::separatorItem(menu.mtm());
                scratch.insertItem_atIndex(&placeholder, scratch_idx as isize);
                menu.insertItem_atIndex(&desired, remove_at + offset as isize);
            }
        }

        for s in 0..plan.suffix_count {
            let menu_idx = from_index + desired_shapes.len() - plan.suffix_count + s;
            let scratch_idx = desired_shapes.len() - plan.suffix_count + s;
            if let (Some(existing), Some(desired)) = (
                menu.itemAtIndex(menu_idx as isize),
                scratch.itemAtIndex(scratch_idx as isize),
            ) {
                sync_menu_item_from_desired(menu, &existing, &desired);
            }
        }

        plan
    }
}

#[derive(Clone, Copy, PartialEq)]
enum MiddleSlot<'a> {
    History(&'a MenuEntry),
    Action(&'a MenuEntry),
    Separator,
}

impl MiddleSlot<'_> {
    fn shape(&self) -> MenuRowShape {
        match self {
            Self::History(entry) | Self::Action(entry) => shape_for_entry(entry),
            Self::Separator => MenuRowShape::Separator,
        }
    }

    fn can_update_in_place_to(&self, desired: &Self) -> bool {
        self == desired || self.shape().can_update_in_place_to(&desired.shape())
    }
}

fn middle_slots_for_structure(structure: &MenuStructure) -> Vec<MiddleSlot<'_>> {
    let mut slots = Vec::new();
    if !structure.history.is_empty() {
        for entry in &structure.history {
            slots.push(MiddleSlot::History(entry));
        }
        slots.push(MiddleSlot::Separator);
    }
    if !structure.actions.is_empty() {
        for entry in &structure.actions {
            slots.push(MiddleSlot::Action(entry));
        }
        slots.push(MiddleSlot::Separator);
    }
    slots
}

fn reconcile_middle_slots(
    existing: &[MiddleSlot<'_>],
    desired: &[MiddleSlot<'_>],
) -> ReconcilePlan {
    let mut prefix_count = 0;
    while prefix_count < existing.len()
        && prefix_count < desired.len()
        && existing[prefix_count].can_update_in_place_to(&desired[prefix_count])
    {
        prefix_count += 1;
    }

    let mut suffix_count = 0;
    while prefix_count + suffix_count < existing.len()
        && prefix_count + suffix_count < desired.len()
        && existing[existing.len() - 1 - suffix_count]
            .can_update_in_place_to(&desired[desired.len() - 1 - suffix_count])
    {
        suffix_count += 1;
    }

    ReconcilePlan {
        prefix_count,
        suffix_count,
        remove_range: prefix_count..(existing.len() - suffix_count),
        insert_range: prefix_count..(desired.len() - suffix_count),
    }
}

fn sync_menu_item_from_desired(menu: &NSMenu, existing: &NSMenuItem, desired: &NSMenuItem) {
    if existing.isSeparatorItem() && desired.isSeparatorItem() {
        return;
    }
    let mut changed = false;
    if existing.title() != desired.title() {
        existing.setTitle(&desired.title());
        changed = true;
    }
    if existing.isEnabled() != desired.isEnabled() {
        existing.setEnabled(desired.isEnabled());
        changed = true;
    }
    if existing.state() != desired.state() {
        existing.setState(desired.state());
        changed = true;
    }
    if existing.tag() != desired.tag() {
        existing.setTag(desired.tag());
        changed = true;
    }
    if existing.keyEquivalent() != desired.keyEquivalent() {
        existing.setKeyEquivalent(&desired.keyEquivalent());
        changed = true;
    }
    if existing.toolTip() != desired.toolTip() {
        existing.setToolTip(desired.toolTip().as_deref());
        changed = true;
    }
    // SAFETY: `target` and `action` on `desired` are valid Objective-C references/selectors.
    unsafe {
        existing.setTarget(desired.target().as_deref());
        existing.setAction(desired.action());
    }
    if changed {
        menu.itemChanged(existing);
    }
}

fn same_entry_shapes(prev: &[MenuEntry], next: &[MenuEntry]) -> bool {
    if prev.len() != next.len() {
        return false;
    }
    prev.iter().zip(next.iter()).all(|(a, b)| {
        a == b
            || (shape_for_entry(a).can_update_in_place_to(&shape_for_entry(b))
                && match (a, b) {
                    (MenuEntry::SecondaryText(_), MenuEntry::SecondaryText(_)) => true,
                    (
                        MenuEntry::Action {
                            action: act_a,
                            key_equivalent: key_a,
                            ..
                        },
                        MenuEntry::Action {
                            action: act_b,
                            key_equivalent: key_b,
                            ..
                        },
                    ) => same_action_kind(act_a, act_b) && key_a == key_b,
                    (MenuEntry::Submenu { .. }, MenuEntry::Submenu { .. }) => true,
                    _ => false,
                })
    })
}

fn same_action_kind(a: &MenuAction, b: &MenuAction) -> bool {
    matches!(
        (a, b),
        (MenuAction::Refresh, MenuAction::Refresh)
            | (MenuAction::Settings, MenuAction::Settings)
            | (
                MenuAction::ShareUsageSnapshot,
                MenuAction::ShareUsageSnapshot
            )
            | (MenuAction::AddAccount { .. }, MenuAction::AddAccount { .. })
            | (
                MenuAction::SwitchAccount { .. },
                MenuAction::SwitchAccount { .. }
            )
            | (MenuAction::BuyCredits { .. }, MenuAction::BuyCredits { .. })
            | (
                MenuAction::OpenDashboard { .. },
                MenuAction::OpenDashboard { .. }
            )
            | (
                MenuAction::OpenStatusPage { .. },
                MenuAction::OpenStatusPage { .. }
            )
            | (MenuAction::About, MenuAction::About)
            | (MenuAction::Quit, MenuAction::Quit)
    )
}

fn submenu_shapes_compatible(sub: &NSMenu, items: &[SubmenuEntry]) -> bool {
    for (idx, entry) in items.iter().enumerate() {
        let Some(child) = sub.itemAtIndex(idx as isize) else {
            return false;
        };
        let ok = match entry {
            SubmenuEntry::SecondaryText(_) => {
                !child.isSeparatorItem() && child.view().is_none() && child.action().is_none()
            }
            SubmenuEntry::Separator => child.isSeparatorItem(),
            SubmenuEntry::Action { .. } => !child.isSeparatorItem() && child.view().is_none(),
            SubmenuEntry::QuotaBurndownChart(_) => child
                .view()
                .is_some_and(|v| v.downcast_ref::<QuotaBurndownChartView>().is_some()),
            SubmenuEntry::PlanUtilizationChart(_) => child
                .view()
                .is_some_and(|v| v.downcast_ref::<PlanUtilizationChartView>().is_some()),
            SubmenuEntry::CostHistoryChart(_) => child
                .view()
                .is_some_and(|v| v.downcast_ref::<CostHistoryChartView>().is_some()),
        };
        if !ok {
            return false;
        }
    }
    true
}

fn record_entry_urls(entry: &MenuEntry, url_actions: &mut Vec<String>) {
    match entry {
        MenuEntry::Action { action, .. } => record_action_url(action, url_actions),
        MenuEntry::Submenu { items, .. } => {
            for sub in items {
                if let SubmenuEntry::Action { action, .. } = sub {
                    record_action_url(action, url_actions);
                }
            }
        }
        MenuEntry::SecondaryText(_) => {}
    }
}

fn record_action_url(action: &MenuAction, url_actions: &mut Vec<String>) {
    if let MenuAction::BuyCredits { url }
    | MenuAction::OpenDashboard { url }
    | MenuAction::OpenStatusPage { url } = action
    {
        url_actions.push(url.clone());
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

/// Returns `Timestamp::now()` for live snapshots, or `max_updated + 4 min` when
/// running against a static fixture snapshot more than 24h away from wall clock.
fn effective_now(snapshot: Option<&EngineSnapshot>) -> Timestamp {
    let real_now = Timestamp::now();
    let Some(max_updated) =
        snapshot.and_then(|s| s.providers.iter().filter_map(|p| p.updated_at).max())
    else {
        return real_now;
    };
    if (real_now.as_second() - max_updated.as_second()).abs() > 86_400 {
        Timestamp::from_second(max_updated.as_second() + 240).unwrap_or(real_now)
    } else {
        real_now
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
    let label_height = measure_wrapped_label_height(&label, available_width);
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

/// Updates an existing wrapped secondary text menu item in place without recreating
/// its container `NSView` or `NSTextField`. Returns `true` if text or frame changed.
fn update_wrapped_secondary_text_item(menu_item: &NSMenuItem, text: &str, width: f64) -> bool {
    let Some(container) = menu_item.view() else {
        return false;
    };
    let same_text = menu_item
        .toolTip()
        .is_some_and(|tt| tt.to_string() == *text);
    let same_width = (container.frame().size.width - width).abs() <= f64::EPSILON;
    if same_text && same_width {
        return false;
    }

    menu_item.setToolTip(Some(&NSString::from_str(text)));
    let horizontal_leading: f64 = 18.0;
    let horizontal_trailing: f64 = 10.0;
    let vertical_padding: f64 = 2.0;
    let available_width = (width - horizontal_leading - horizontal_trailing).max(120.0);

    let subviews = container.subviews();
    let Some(first_subview) = subviews.iter().next() else {
        return false;
    };
    let Some(label) = first_subview.downcast_ref::<NSTextField>() else {
        return false;
    };
    label.setStringValue(&NSString::from_str(text));
    label.setPreferredMaxLayoutWidth(available_width);
    let label_height = measure_wrapped_label_height(label, available_width);
    let row_height = (label_height + vertical_padding * 2.0).max(18.0);
    let next_container_size = NSSize::new(width, row_height);
    if container.frame().size != next_container_size {
        container.setFrameSize(next_container_size);
    }
    let next_label_frame = NSRect::new(
        NSPoint::new(horizontal_leading, vertical_padding),
        NSSize::new(available_width, label_height),
    );
    if label.frame() != next_label_frame {
        label.setFrame(next_label_frame);
    }
    container.setNeedsDisplay(true);
    true
}

fn measure_wrapped_label_height(label: &NSTextField, available_width: f64) -> f64 {
    label
        .cell()
        .map(|cell| {
            cell.cellSizeForBounds(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(available_width, 100_000.0),
            ))
            .height
            .ceil()
        })
        .unwrap_or_else(|| label.fittingSize().height.ceil())
}
