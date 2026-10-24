// Ported from CodexBar Sources/CodexBar/PreferencesView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesSidebar.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesComponents.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesGeneralPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesMenuBarPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesMenuPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesAboutPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuBarLayoutEditor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesICloudSyncPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesSpendDashboardPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesNotificationsPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesAdvancedPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesHooksPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesPluginsPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesProviderDetailView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesProviderSettingsRows.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Native AppKit Settings view hierarchy hosting an `NSOutlineView` source-list
//! sidebar, `NSSearchField`, sort/filter controls, navigation header, and a
//! scrollable detail document built from native `NSSwitch`, `NSPopUpButton`,
//! `NSButton`, `NSSegmentedControl`, `NSColorWell`, `NSSlider`, `NSStepper`,
//! and `NSTextField` controls positioned by
//! [`crate::settings_layout::layout_settings_window`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ariadusage_protocol::ids::{ActionId, ProviderId, SettingId};
use ariadusage_protocol::settings::{SettingsPage, SettingsScope};
use ariadusage_protocol::snapshot::EngineSnapshot;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSBezelStyle, NSBezierPath, NSBitmapImageRep, NSButton, NSButtonType, NSColor,
    NSColorWell, NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSEventModifierFlags,
    NSFont, NSFontWeightBold, NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold,
    NSImage, NSLineBreakMode, NSOutlineView, NSPasteboard, NSPasteboardTypeString, NSPopUpButton,
    NSResponder, NSScrollView, NSSearchField, NSSegmentedControl, NSSlider, NSStepper, NSSwitch,
    NSTableColumn, NSTableRowView, NSTableViewSelectionHighlightStyle, NSTableViewStyle,
    NSTextAlignment, NSTextField, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWorkspace,
};
use objc2_foundation::{
    NSIndexSet, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    NSURL,
};

use crate::card::Rgb;
use crate::engine_client::EngineClient;
use crate::geometry::{Point, Rect, Size};
use crate::settings::{
    ChoiceItem, FormButtonAction, ProviderStatusDotColor, SIDEBAR_DEFAULT_WIDTH, SettingsPane,
    SettingsSelection, SettingsState, SidebarChipStyle, SidebarFilterState, SidebarModel,
    WINDOW_HEIGHT, WINDOW_WIDTH, clamp_sidebar_width, pane_model, secret_set_command_args,
    secret_set_terminal_command,
};
use crate::settings_layout::{
    DETAIL_HEADER_HEIGHT, FormButtonLayout, FormControlLayout, FormRowLayout, FormSectionLayout,
    MenuBarLayoutPreviewGeometry, SECTION_CARD_CORNER_RADIUS, SettingsWindowLayout,
    layout_settings_window,
};

/// Callback invoked when the user changes a setting or triggers an action.
pub type SettingChangeCallback = Rc<dyn Fn(&SettingId, serde_json::Value)>;

/// Action binding registered for a dynamic control tag inside the detail pane.
#[derive(Clone)]
enum ControlBinding {
    Toggle {
        setting_id: SettingId,
    },
    Choice {
        setting_id: SettingId,
        options: Vec<ChoiceItem>,
        pulls_down: bool,
    },
    ButtonAction(FormButtonAction),
    OpenUrl(String),
    CopyCommand(String),
}

/// Decorative icon kind drawn by [`SettingsIconView`].
#[derive(Clone)]
enum IconDrawKind {
    SidebarChip(SidebarChipStyle),
    ProviderGlyph {
        provider_id: String,
        enabled: bool,
        dark: bool,
    },
    StatusDot(ProviderStatusDotColor),
    AppHeroIcon,
    LinkSymbol {
        symbol: &'static str,
        dark: bool,
    },
    ExternalArrow {
        dark: bool,
    },
    BarsMini {
        dark: bool,
        enabled: bool,
    },
    UsageProgressBar {
        percent_used: f64,
        accent_rgb: Rgb,
        dark: bool,
    },
    MetricBox {
        dark: bool,
    },
}

#[derive(Default)]
pub struct IconViewIvars {
    kind: RefCell<Option<IconDrawKind>>,
}

define_class!(
    // SAFETY: `NSView` has no subclassing requirements beyond `initWithFrame:`;
    // `SettingsIconView` does not implement `Drop`.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsageSettingsIconView"]
    #[ivars = IconViewIvars]
    pub struct SettingsIconView;

    unsafe impl NSObjectProtocol for SettingsIconView {}

    impl SettingsIconView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let bounds = self.bounds();
            let rect = Rect::new(0.0, 0.0, bounds.size.width, bounds.size.height);
            let Some(kind) = self.ivars().kind.borrow().clone() else {
                return;
            };
            match kind {
                IconDrawKind::SidebarChip(style) => draw_sidebar_chip(rect, style),
                IconDrawKind::ProviderGlyph {
                    provider_id,
                    enabled,
                    dark,
                } => {
                    let palette = Palette { dark };
                    let color = if enabled {
                        palette.text_primary()
                    } else {
                        palette.text_disabled()
                    };
                    draw_provider_glyph(rect, &provider_id, color);
                }
                IconDrawKind::StatusDot(dot) => {
                    rgb_color(dot.rgb(), 1.0).setFill();
                    NSBezierPath::bezierPathWithOvalInRect(ns_rect(rect)).fill();
                }
                IconDrawKind::AppHeroIcon => {
                    draw_app_brand_icon(rect, 20.0);
                }
                IconDrawKind::LinkSymbol { symbol, dark } => {
                    let palette = Palette { dark };
                    draw_link_symbol(rect, symbol, palette.text_secondary());
                }
                IconDrawKind::ExternalArrow { dark } => {
                    let palette = Palette { dark };
                    draw_external_arrow(rect, palette.text_disabled());
                }
                IconDrawKind::BarsMini { dark, enabled } => {
                    let palette = Palette { dark };
                    let color = if enabled {
                        palette.text_primary()
                    } else {
                        palette.text_disabled()
                    };
                    draw_bars_mini_glyph(rect, color);
                }
                IconDrawKind::UsageProgressBar {
                    percent_used,
                    accent_rgb,
                    dark,
                } => {
                    let palette = Palette { dark };
                    let radius = (rect.h / 2.0).max(2.0);
                    palette.control_bg(true).setFill();
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                        ns_rect(rect),
                        radius,
                        radius,
                    )
                    .fill();
                    let remaining_frac = ((100.0 - percent_used).clamp(0.0, 100.0)) / 100.0;
                    let fill_w = (rect.w * remaining_frac).max(if remaining_frac > 0.0 {
                        4.0
                    } else {
                        0.0
                    });
                    if fill_w > 0.0 {
                        let fill_rect = Rect::new(rect.x, rect.y, fill_w, rect.h);
                        rgb_color(accent_rgb, 1.0).setFill();
                        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                            ns_rect(fill_rect),
                            radius,
                            radius,
                        )
                        .fill();
                    }
                }
                IconDrawKind::MetricBox { dark } => {
                    let palette = Palette { dark };
                    fill_stroke_rounded(
                        rect,
                        8.0,
                        palette.control_bg(false),
                        palette.card_border(),
                    );
                }
            }
        }
    }
);

impl SettingsIconView {
    fn new(mtm: MainThreadMarker, frame: Rect, kind: IconDrawKind) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(IconViewIvars {
            kind: RefCell::new(Some(kind)),
        });
        // SAFETY: `initWithFrame:` is `NSView`'s designated initializer.
        unsafe { msg_send![super(this), initWithFrame: ns_rect(frame)] }
    }
}

/// State drawn by [`SettingsGroupCardView`] (background, border, and 1px row dividers only).
#[derive(Clone, Default)]
pub struct CardViewState {
    dark: bool,
    divider_ys: Vec<f64>,
    layout_preview_boxes: Option<(Rect, Rect, Rect)>,
}

define_class!(
    // SAFETY: `NSView` has no subclassing requirements beyond `initWithFrame:`;
    // `SettingsGroupCardView` does not implement `Drop`.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsageSettingsGroupCardView"]
    #[ivars = RefCell<CardViewState>]
    pub struct SettingsGroupCardView;

    unsafe impl NSObjectProtocol for SettingsGroupCardView {}

    impl SettingsGroupCardView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let state = self.ivars().borrow().clone();
            let palette = Palette { dark: state.dark };
            let bounds = self.bounds();
            let rect = Rect::new(0.0, 0.0, bounds.size.width, bounds.size.height);
            fill_stroke_rounded(
                rect,
                SECTION_CARD_CORNER_RADIUS,
                palette.card_bg(),
                palette.card_border(),
            );
            palette.divider().setFill();
            for y in state.divider_ys {
                let div = Rect::new(12.0, y, (rect.w - 24.0).max(0.0), 1.0);
                NSBezierPath::fillRect(ns_rect(div));
            }
            if let Some((preview_box, pill_box, strip_box)) = state.layout_preview_boxes {
                fill_stroke_rounded(
                    preview_box,
                    6.0,
                    palette.control_bg(false),
                    palette.control_border(false),
                );
                fill_stroke_rounded(
                    pill_box,
                    5.0,
                    palette.sidebar_selected(),
                    palette.card_border(),
                );
                fill_stroke_rounded(
                    strip_box,
                    6.0,
                    palette.control_bg(false),
                    palette.control_border(false),
                );
            }
        }
    }
);

impl SettingsGroupCardView {
    fn new(mtm: MainThreadMarker, frame: Rect, state: CardViewState) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RefCell::new(state));
        // SAFETY: `initWithFrame:` is `NSView`'s designated initializer.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: ns_rect(frame)] };
        // SAFETY: setting accessibility role on `NSView` so AX tools see an `AXGroup` card container.
        unsafe {
            let role = NSString::from_str("AXGroup");
            let subrole = NSString::from_str("AXContentList");
            let _: () = msg_send![&view, setAccessibilityElement: true];
            let _: () = msg_send![&view, setAccessibilityRole: &*role];
            let _: () = msg_send![&view, setAccessibilitySubrole: &*subrole];
        }
        view
    }
}

define_class!(
    // SAFETY: `NSView` has no subclassing requirements beyond `initWithFrame:`;
    // `DetailDocumentView` does not implement `Drop`.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsageSettingsDetailDocumentView"]
    #[ivars = ()]
    pub struct DetailDocumentView;

    unsafe impl NSObjectProtocol for DetailDocumentView {}

    impl DetailDocumentView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl DetailDocumentView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: `initWithFrame:` is `NSView`'s designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

pub struct SidebarRowViewIvars {
    dark: Cell<bool>,
}

define_class!(
    // SAFETY: `NSTableRowView` has no subclassing requirements beyond `initWithFrame:`;
    // `SidebarTableRowView` does not implement `Drop`.
    #[unsafe(super(NSTableRowView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsageSidebarTableRowView"]
    #[ivars = SidebarRowViewIvars]
    pub struct SidebarTableRowView;

    unsafe impl NSObjectProtocol for SidebarTableRowView {}

    impl SidebarTableRowView {
        #[unsafe(method(drawSelectionInRect:))]
        fn draw_selection_in_rect(&self, _dirty: NSRect) {
            let palette = Palette {
                dark: self.ivars().dark.get(),
            };
            let b = self.bounds();
            let pill = Rect::new(
                8.0,
                1.0,
                (b.size.width - 16.0).max(0.0),
                (b.size.height - 2.0).max(0.0),
            );
            palette.sidebar_selected().setFill();
            let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                ns_rect(pill),
                6.0,
                6.0,
            );
            path.fill();
        }
    }
);

impl SidebarTableRowView {
    fn new(mtm: MainThreadMarker, dark: bool) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SidebarRowViewIvars {
            dark: Cell::new(dark),
        });
        // SAFETY: `initWithFrame:` is `NSTableRowView`'s designated initializer.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

pub struct SidebarItemIvars {
    token: String,
    height: f64,
    selectable: bool,
    cell_view: Retained<NSView>,
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements beyond `init`;
    // `SidebarRowItem` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsageSidebarRowItem"]
    #[ivars = SidebarItemIvars]
    pub struct SidebarRowItem;

    unsafe impl NSObjectProtocol for SidebarRowItem {}
);

impl SidebarRowItem {
    fn new(
        mtm: MainThreadMarker,
        token: String,
        height: f64,
        selectable: bool,
        cell_view: Retained<NSView>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SidebarItemIvars {
            token,
            height,
            selectable,
            cell_view,
        });
        // SAFETY: `init` is `NSObject`'s designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// Mutable state owned by [`SettingsView`].
pub struct SettingsViewState {
    pub selection: SettingsSelection,
    pub filter: SidebarFilterState,
    pub settings: SettingsState,
    pub snapshot: Option<EngineSnapshot>,
    pub sidebar_width: f64,
    pub dragging_sidebar: bool,
    pub appearance_dark_override: Option<bool>,
    pub engine_client: Option<EngineClient>,
    pub on_setting_changed: Option<SettingChangeCallback>,
    pub layout: Option<SettingsWindowLayout>,
    pub last_secret_terminal_command: Option<Vec<String>>,
    pub last_refreshed_provider: Option<Option<String>>,
    sidebar_effect_view: Option<Retained<NSVisualEffectView>>,
    search_field: Option<Retained<NSSearchField>>,
    sort_button: Option<Retained<NSButton>>,
    filter_popup: Option<Retained<NSPopUpButton>>,
    sidebar_scroll_view: Option<Retained<NSScrollView>>,
    sidebar_outline_view: Option<Retained<NSOutlineView>>,
    sidebar_column: Option<Retained<NSTableColumn>>,
    sidebar_items: Vec<Retained<SidebarRowItem>>,
    detail_header_group: Option<Retained<NSView>>,
    back_button: Option<Retained<NSButton>>,
    forward_button: Option<Retained<NSButton>>,
    header_title_field: Option<Retained<NSTextField>>,
    detail_scroll_view: Option<Retained<NSScrollView>>,
    detail_document_view: Option<Retained<DetailDocumentView>>,
    control_bindings: Vec<ControlBinding>,
    suppress_outline_selection: Cell<bool>,
}

impl Default for SettingsViewState {
    fn default() -> Self {
        Self {
            selection: SettingsSelection::default(),
            filter: SidebarFilterState::default(),
            settings: SettingsState::default(),
            snapshot: None,
            sidebar_width: SIDEBAR_DEFAULT_WIDTH,
            dragging_sidebar: false,
            appearance_dark_override: None,
            engine_client: None,
            on_setting_changed: None,
            layout: None,
            last_secret_terminal_command: None,
            last_refreshed_provider: None,
            sidebar_effect_view: None,
            search_field: None,
            sort_button: None,
            filter_popup: None,
            sidebar_scroll_view: None,
            sidebar_outline_view: None,
            sidebar_column: None,
            sidebar_items: Vec::new(),
            detail_header_group: None,
            back_button: None,
            forward_button: None,
            header_title_field: None,
            detail_scroll_view: None,
            detail_document_view: None,
            control_bindings: Vec::new(),
            suppress_outline_selection: Cell::new(false),
        }
    }
}

define_class!(
    // SAFETY: `NSView` has no subclassing requirements beyond `initWithFrame:`;
    // `SettingsView` does not implement `Drop`.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsageSettingsView"]
    #[ivars = RefCell<SettingsViewState>]
    pub struct SettingsView;

    unsafe impl NSObjectProtocol for SettingsView {}

    impl SettingsView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, new_size: NSSize) {
            // SAFETY: forwarding `setFrameSize:` to `NSView` superclass.
            let _: () = unsafe { msg_send![super(self), setFrameSize: new_size] };
            self.rebuild_ui();
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw_chrome();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let loc = self.convertPoint_fromView(event.locationInWindow(), None);
            let resize_hit = self
                .ivars()
                .borrow()
                .layout
                .as_ref()
                .is_some_and(|l| l.resize_handle_rect.contains(Point::new(loc.x, loc.y)));
            if resize_hit {
                self.ivars().borrow_mut().dragging_sidebar = true;
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if !self.ivars().borrow().dragging_sidebar {
                return;
            }
            let loc = self.convertPoint_fromView(event.locationInWindow(), None);
            self.ivars().borrow_mut().sidebar_width = clamp_sidebar_width(loc.x);
            self.rebuild_ui();
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.ivars().borrow_mut().dragging_sidebar = false;
        }

        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            self.handle_key_equivalent(event)
        }

        // --- Sidebar search, sort, and filter actions ---

        #[unsafe(method(searchFieldChanged:))]
        fn search_field_changed(&self, sender: Option<&NSSearchField>) {
            if let Some(sf) = sender {
                self.apply_search_field_value(sf);
            }
        }

        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _notification: Option<&NSNotification>) {
            let search_field = self.ivars().borrow().search_field.clone();
            if let Some(sf) = search_field {
                self.apply_search_field_value(&sf);
            }
        }

        #[unsafe(method(toggleSortAlphabetically:))]
        fn toggle_sort_alphabetically(&self, _sender: Option<&AnyObject>) {
            {
                let mut state = self.ivars().borrow_mut();
                state.filter.sorted_alphabetically = !state.filter.sorted_alphabetically;
            }
            self.rebuild_sidebar_only();
        }

        #[unsafe(method(sidebarFilterMenuChanged:))]
        fn sidebar_filter_menu_changed(&self, sender: Option<&NSPopUpButton>) {
            let Some(sender) = sender else {
                return;
            };
            let selected = sender.selectedItem();
            let tag = selected.map_or(0, |item| item.tag());
            {
                let mut state = self.ivars().borrow_mut();
                match tag {
                    1 => state.filter.enabled_only = !state.filter.enabled_only,
                    2 => state.filter.sorted_alphabetically = false,
                    _ => {}
                }
            }
            self.rebuild_sidebar_only();
        }

        #[unsafe(method(clearFiltersClicked:))]
        fn clear_filters_clicked(&self, _sender: Option<&AnyObject>) {
            {
                let mut state = self.ivars().borrow_mut();
                state.filter.clear_filters();
                if let Some(sf) = &state.search_field {
                    sf.setStringValue(&NSString::from_str(""));
                }
            }
            self.rebuild_sidebar_only();
        }

        // --- Navigation header actions ---

        #[unsafe(method(navBackClicked:))]
        fn nav_back_clicked(&self, _sender: Option<&AnyObject>) {
            let moved = self.ivars().borrow_mut().selection.go_back();
            if moved {
                self.rebuild_ui();
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(navForwardClicked:))]
        fn nav_forward_clicked(&self, _sender: Option<&AnyObject>) {
            let moved = self.ivars().borrow_mut().selection.go_forward();
            if moved {
                self.rebuild_ui();
                self.setNeedsDisplay(true);
            }
        }

        // --- Detail form control actions ---

        #[unsafe(method(switchToggled:))]
        fn switch_toggled(&self, sender: Option<&NSSwitch>) {
            let Some(sender) = sender else {
                return;
            };
            let idx = sender.tag() as usize;
            let binding = self.ivars().borrow().control_bindings.get(idx).cloned();
            if let Some(ControlBinding::Toggle { setting_id }) = binding {
                let on = sender.state() == NSControlStateValueOn;
                self.apply_setting_change(&setting_id, serde_json::Value::Bool(on));
            }
        }

        #[unsafe(method(checkboxToggled:))]
        fn checkbox_toggled(&self, sender: Option<&NSButton>) {
            let Some(sender) = sender else {
                return;
            };
            let idx = sender.tag() as usize;
            let binding = self.ivars().borrow().control_bindings.get(idx).cloned();
            if let Some(ControlBinding::Toggle { setting_id }) = binding {
                let on = sender.state() == NSControlStateValueOn;
                self.apply_setting_change(&setting_id, serde_json::Value::Bool(on));
            }
        }

        #[unsafe(method(choicePopupChanged:))]
        fn choice_popup_changed(&self, sender: Option<&NSPopUpButton>) {
            let Some(sender) = sender else {
                return;
            };
            let idx = sender.tag() as usize;
            let binding = self.ivars().borrow().control_bindings.get(idx).cloned();
            if let Some(ControlBinding::Choice {
                setting_id,
                options,
                pulls_down,
            }) = binding
            {
                let item_idx = sender.indexOfSelectedItem();
                let opt_idx = if pulls_down {
                    // In a pull-down `NSPopUpButton`, index 0 is the title item and options start at 1.
                    if item_idx >= 1 {
                        Some((item_idx - 1) as usize)
                    } else {
                        None
                    }
                } else if item_idx >= 0 {
                    Some(item_idx as usize)
                } else {
                    None
                };
                if let Some(i) = opt_idx
                    && let Some(choice) = options.get(i)
                {
                    self.apply_setting_change(&setting_id, choice.to_json_value());
                }
            }
        }

        #[unsafe(method(controlActionClicked:))]
        fn control_action_clicked(&self, sender: Option<&NSButton>) {
            let Some(sender) = sender else {
                return;
            };
            let idx = sender.tag() as usize;
            let binding = self.ivars().borrow().control_bindings.get(idx).cloned();
            match binding {
                Some(ControlBinding::ButtonAction(action)) => self.execute_button_action(action),
                Some(ControlBinding::OpenUrl(url)) => open_external_url(&url),
                Some(ControlBinding::CopyCommand(cmd)) => copy_to_pasteboard(&cmd),
                _ => {}
            }
        }

        // --- NSOutlineViewDataSource & NSOutlineViewDelegate ---

        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn outline_view_number_of_children(
            &self,
            _outline_view: Option<&NSOutlineView>,
            item: Option<&AnyObject>,
        ) -> isize {
            if item.is_some() {
                0
            } else {
                self.ivars().borrow().sidebar_items.len() as isize
            }
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn outline_view_is_item_expandable(
            &self,
            _outline_view: Option<&NSOutlineView>,
            _item: Option<&AnyObject>,
        ) -> bool {
            false
        }

        #[unsafe(method(outlineView:child:ofItem:))]
        fn outline_view_child_of_item(
            &self,
            _outline_view: Option<&NSOutlineView>,
            index: isize,
            item: Option<&AnyObject>,
        ) -> *mut AnyObject {
            if item.is_some() || index < 0 {
                return std::ptr::null_mut();
            }
            let state = self.ivars().borrow();
            state
                .sidebar_items
                .get(index as usize)
                .map_or(std::ptr::null_mut(), |s| {
                    Retained::as_ptr(s) as *mut AnyObject
                })
        }

        #[unsafe(method(outlineView:rowViewForItem:))]
        fn outline_view_row_view_for_item(
            &self,
            _outline_view: Option<&NSOutlineView>,
            _item: Option<&AnyObject>,
        ) -> *mut NSTableRowView {
            let dark = self.is_dark();
            let row_view = SidebarTableRowView::new(self.mtm(), dark);
            Retained::autorelease_return(row_view) as *mut NSTableRowView
        }

        #[unsafe(method(outlineView:viewForTableColumn:item:))]
        fn outline_view_view_for_table_column(
            &self,
            _outline_view: Option<&NSOutlineView>,
            _table_column: Option<&NSTableColumn>,
            item: Option<&AnyObject>,
        ) -> *mut NSView {
            let Some(item) = item else {
                return std::ptr::null_mut();
            };
            let state = self.ivars().borrow();
            let item_ptr = item as *const AnyObject;
            for row_item in &state.sidebar_items {
                if std::ptr::eq(Retained::as_ptr(row_item) as *const AnyObject, item_ptr) {
                    return Retained::as_ptr(&row_item.ivars().cell_view) as *mut NSView;
                }
            }
            std::ptr::null_mut()
        }

        #[unsafe(method(outlineView:heightOfRowByItem:))]
        fn outline_view_height_of_row_by_item(
            &self,
            _outline_view: Option<&NSOutlineView>,
            item: Option<&AnyObject>,
        ) -> f64 {
            let Some(item) = item else {
                return 28.0;
            };
            let state = self.ivars().borrow();
            let item_ptr = item as *const AnyObject;
            for row_item in &state.sidebar_items {
                if std::ptr::eq(Retained::as_ptr(row_item) as *const AnyObject, item_ptr) {
                    return row_item.ivars().height;
                }
            }
            28.0
        }

        #[unsafe(method(outlineView:shouldSelectItem:))]
        fn outline_view_should_select_item(
            &self,
            _outline_view: Option<&NSOutlineView>,
            item: Option<&AnyObject>,
        ) -> bool {
            item.is_some_and(|item| {
                let state = self.ivars().borrow();
                let item_ptr = item as *const AnyObject;
                state
                    .sidebar_items
                    .iter()
                    .find(|row_item| {
                        std::ptr::eq(Retained::as_ptr(row_item) as *const AnyObject, item_ptr)
                    })
                    .is_some_and(|row_item| row_item.ivars().selectable)
            })
        }

        #[unsafe(method(outlineViewSelectionDidChange:))]
        fn outline_view_selection_did_change(&self, _notification: Option<&NSNotification>) {
            if self.ivars().borrow().suppress_outline_selection.get() {
                return;
            }
            let target_pane = {
                let state = self.ivars().borrow();
                let Some(ov) = &state.sidebar_outline_view else {
                    return;
                };
                let row = ov.selectedRow();
                if row < 0 {
                    return;
                }
                let Some(row_item) = state.sidebar_items.get(row as usize) else {
                    return;
                };
                let token_str = &row_item.ivars().token;
                if let Some(rest) = token_str.strip_prefix("pane:") {
                    SettingsPane::from_persistence_token(rest)
                } else {
                    token_str
                        .strip_prefix("provider:")
                        .map(|id| SettingsPane::Provider(id.to_string()))
                }
            };
            if let Some(pane) = target_pane {
                let changed = {
                    let mut state = self.ivars().borrow_mut();
                    let was = state.selection.pane().clone();
                    state.selection.select(pane.clone());
                    was != pane
                };
                if changed {
                    self.request_provider_settings_if_needed(&pane);
                    self.rebuild_detail_only();
                    self.setNeedsDisplay(true);
                }
            }
        }
    }
);

fn ns_rect(r: Rect) -> NSRect {
    NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
}

fn srgb(r: f64, g: f64, b: f64, a: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a)
}

fn rgb_color(c: Rgb, alpha: f64) -> Retained<NSColor> {
    srgb(
        f64::from(c.r) / 255.0,
        f64::from(c.g) / 255.0,
        f64::from(c.b) / 255.0,
        alpha,
    )
}

#[derive(Clone, Copy)]
struct Palette {
    dark: bool,
}

impl Palette {
    fn sidebar_bg(self) -> Retained<NSColor> {
        if self.dark {
            srgb(0.149, 0.149, 0.161, 1.0)
        } else {
            srgb(0.914, 0.914, 0.937, 1.0)
        }
    }

    fn detail_bg(self) -> Retained<NSColor> {
        if self.dark {
            srgb(0.118, 0.118, 0.129, 1.0)
        } else {
            srgb(0.949, 0.949, 0.969, 1.0)
        }
    }

    fn divider(self) -> Retained<NSColor> {
        if self.dark {
            srgb(1.0, 1.0, 1.0, 0.09)
        } else {
            srgb(0.0, 0.0, 0.0, 0.10)
        }
    }

    fn card_bg(self) -> Retained<NSColor> {
        if self.dark {
            srgb(1.0, 1.0, 1.0, 0.055)
        } else {
            srgb(1.0, 1.0, 1.0, 0.96)
        }
    }

    fn card_border(self) -> Retained<NSColor> {
        if self.dark {
            srgb(1.0, 1.0, 1.0, 0.08)
        } else {
            srgb(0.0, 0.0, 0.0, 0.08)
        }
    }

    fn sidebar_selected(self) -> Retained<NSColor> {
        if self.dark {
            srgb(1.0, 1.0, 1.0, 0.14)
        } else {
            srgb(0.0, 0.0, 0.0, 0.10)
        }
    }

    fn text_primary(self) -> Retained<NSColor> {
        if self.dark {
            srgb(1.0, 1.0, 1.0, 0.92)
        } else {
            srgb(0.0, 0.0, 0.0, 0.88)
        }
    }

    fn text_secondary(self) -> Retained<NSColor> {
        if self.dark {
            srgb(0.92, 0.92, 0.96, 0.58)
        } else {
            srgb(0.22, 0.22, 0.25, 0.64)
        }
    }

    fn text_disabled(self) -> Retained<NSColor> {
        if self.dark {
            srgb(0.92, 0.92, 0.96, 0.32)
        } else {
            srgb(0.22, 0.22, 0.25, 0.36)
        }
    }

    fn control_bg(self, enabled: bool) -> Retained<NSColor> {
        if self.dark {
            srgb(1.0, 1.0, 1.0, if enabled { 0.11 } else { 0.05 })
        } else {
            srgb(0.0, 0.0, 0.0, if enabled { 0.06 } else { 0.03 })
        }
    }

    fn control_border(self, enabled: bool) -> Retained<NSColor> {
        if self.dark {
            srgb(1.0, 1.0, 1.0, if enabled { 0.18 } else { 0.08 })
        } else {
            srgb(0.0, 0.0, 0.0, if enabled { 0.16 } else { 0.08 })
        }
    }
}

#[derive(Clone, Copy)]
enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
}

fn system_font(size: f64, weight: Weight) -> Retained<NSFont> {
    // SAFETY: `NSFontWeight*` constants are immutable AppKit globals.
    let ns_weight = unsafe {
        match weight {
            Weight::Regular => NSFontWeightRegular,
            Weight::Medium => NSFontWeightMedium,
            Weight::Semibold => NSFontWeightSemibold,
            Weight::Bold => NSFontWeightBold,
        }
    };
    NSFont::systemFontOfSize_weight(size, ns_weight)
}

#[allow(clippy::too_many_arguments)]
fn make_label(
    mtm: MainThreadMarker,
    frame: Rect,
    text: &str,
    size: f64,
    weight: Weight,
    color: &NSColor,
    align: NSTextAlignment,
    max_lines: usize,
    enabled: bool,
) -> Retained<NSTextField> {
    let field = if max_lines > 1 {
        NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm)
    } else {
        NSTextField::labelWithString(&NSString::from_str(text), mtm)
    };
    field.setFrame(ns_rect(frame));
    field.setFont(Some(&system_font(size, weight)));
    field.setTextColor(Some(color));
    field.setAlignment(align);
    field.setEnabled(enabled);
    if max_lines <= 1 {
        field.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        field.setMaximumNumberOfLines(1);
    } else {
        field.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        field.setMaximumNumberOfLines(max_lines as isize);
    }
    field
}

fn make_heading_label(
    mtm: MainThreadMarker,
    frame: Rect,
    text: &str,
    size: f64,
    weight: Weight,
    color: &NSColor,
) -> Retained<NSTextField> {
    let label = make_label(
        mtm,
        frame,
        text,
        size,
        weight,
        color,
        NSTextAlignment::Left,
        1,
        true,
    );
    // SAFETY: setting `AXHeading` role and description on `NSTextField` for VoiceOver/AX tree parity.
    unsafe {
        let role = NSString::from_str("AXHeading");
        let desc = NSString::from_str(text);
        let _: () = msg_send![&label, setAccessibilityRole: &*role];
        let _: () = msg_send![&label, setAccessibilityLabel: &*desc];
    }
    label
}

fn symbol_image(name: &str, desc: Option<&str>) -> Option<Retained<NSImage>> {
    let ns_name = NSString::from_str(name);
    let ns_desc = desc.map(NSString::from_str);
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns_name, ns_desc.as_deref())
}

impl SettingsView {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RefCell::new(SettingsViewState::default()));
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(WINDOW_WIDTH, WINDOW_HEIGHT),
        );
        // SAFETY: `initWithFrame:` is `NSView`'s designated initializer.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.init_persistent_subviews();
        view.rebuild_ui();
        view
    }

    fn init_persistent_subviews(&self) {
        let mtm = self.mtm();

        // 1. Sidebar NSVisualEffectView (.sidebar material)
        let sidebar_effect = NSVisualEffectView::new(mtm);
        sidebar_effect.setMaterial(NSVisualEffectMaterial::Sidebar);
        sidebar_effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        sidebar_effect.setState(NSVisualEffectState::Active);
        self.addSubview(&sidebar_effect);

        // 2. Sidebar NSSearchField ("Search providers")
        let search_field = NSSearchField::new(mtm);
        search_field.setPlaceholderString(Some(&NSString::from_str("Search providers")));
        // SAFETY: `self` owns `search_field` as a subview and outlives it.
        unsafe {
            search_field.setTarget(Some(self));
            search_field.setAction(Some(sel!(searchFieldChanged:)));
            let _: () = msg_send![&search_field, setDelegate: Some(self)];
        }
        self.addSubview(&search_field);

        // 3. Sidebar Sort NSButton ("Sort providers alphabetically")
        let sort_button = NSButton::new(mtm);
        sort_button.setBezelStyle(NSBezelStyle::AccessoryBarAction);
        sort_button.setTitle(&NSString::from_str(""));
        if let Some(img) =
            symbol_image("arrow.up.arrow.down", Some("Sort providers alphabetically"))
        {
            sort_button.setImage(Some(&img));
        }
        // SAFETY: setting accessibility description and target/action on `sort_button`.
        unsafe {
            let label = NSString::from_str("Sort providers alphabetically");
            let _: () = msg_send![&sort_button, setAccessibilityLabel: &*label];
            sort_button.setTarget(Some(self));
            sort_button.setAction(Some(sel!(toggleSortAlphabetically:)));
        }
        self.addSubview(&sort_button);

        // 4. Sidebar Provider sorting pull-down NSPopUpButton (`AXMenuButton AXTitle="Provider sorting"`)
        // SAFETY: `initWithFrame:pullsDown:` is `NSPopUpButton`'s designated initializer.
        let filter_popup: Retained<NSPopUpButton> = unsafe {
            msg_send![
                NSPopUpButton::alloc(mtm),
                initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(24.0, 24.0)),
                pullsDown: true
            ]
        };
        filter_popup.setBordered(false);
        // SAFETY: setting accessibility title and target/action on `filter_popup`.
        unsafe {
            let title = NSString::from_str("Provider sorting");
            let _: () = msg_send![&filter_popup, setAccessibilityTitle: &*title];
            let _: () = msg_send![&filter_popup, setAccessibilityLabel: &*title];
            filter_popup.setTarget(Some(self));
            filter_popup.setAction(Some(sel!(sidebarFilterMenuChanged:)));
        }
        self.addSubview(&filter_popup);

        // 5. Sidebar NSScrollView + NSOutlineView (`AXOutline AXDescription="Sidebar"`)
        let sidebar_scroll = NSScrollView::new(mtm);
        sidebar_scroll.setDrawsBackground(false);
        sidebar_scroll.setHasVerticalScroller(true);
        sidebar_scroll.setAutohidesScrollers(true);

        let outline_view = NSOutlineView::new(mtm);
        outline_view.setHeaderView(None);
        outline_view.setStyle(NSTableViewStyle::Plain);
        outline_view.setBackgroundColor(&NSColor::clearColor());
        outline_view.setSelectionHighlightStyle(NSTableViewSelectionHighlightStyle::Regular);
        outline_view.setRowHeight(28.0);
        outline_view.setIntercellSpacing(NSSize::new(0.0, 2.0));
        outline_view.setFloatsGroupRows(false);
        outline_view.setIndentationPerLevel(0.0);

        let col_id = NSString::from_str("SidebarColumn");
        // SAFETY: `initWithIdentifier:` is `NSTableColumn`'s designated initializer.
        let column: Retained<NSTableColumn> =
            unsafe { msg_send![NSTableColumn::alloc(mtm), initWithIdentifier: &*col_id] };
        column.setWidth(SIDEBAR_DEFAULT_WIDTH - 20.0);
        outline_view.addTableColumn(&column);
        // SAFETY: `column` was just added to `outline_view` and is owned by `self`.
        unsafe {
            outline_view.setOutlineTableColumn(Some(&column));
        }

        // SAFETY: setting accessibility description and dataSource/delegate on `outline_view`.
        unsafe {
            let desc = NSString::from_str("Sidebar");
            let _: () = msg_send![&outline_view, setAccessibilityLabel: &*desc];
            let _: () = msg_send![&outline_view, setDataSource: Some(self)];
            let _: () = msg_send![&outline_view, setDelegate: Some(self)];
        }
        sidebar_scroll.setDocumentView(Some(&outline_view));
        self.addSubview(&sidebar_scroll);

        // 6. Detail header Back/Forward group + heading
        let header_group = NSView::new(mtm);
        // SAFETY: marking `header_group` as an `AXGroup` accessibility element.
        unsafe {
            let role = NSString::from_str("AXGroup");
            let _: () = msg_send![&header_group, setAccessibilityElement: true];
            let _: () = msg_send![&header_group, setAccessibilityRole: &*role];
        }

        let back_button = NSButton::new(mtm);
        back_button.setBezelStyle(NSBezelStyle::Toolbar);
        back_button.setTitle(&NSString::from_str(""));
        if let Some(img) = symbol_image("chevron.left", Some("Back")) {
            back_button.setImage(Some(&img));
        }
        // SAFETY: setting accessibility description and target/action on `back_button`.
        unsafe {
            let desc = NSString::from_str("Back");
            let _: () = msg_send![&back_button, setAccessibilityLabel: &*desc];
            back_button.setTarget(Some(self));
            back_button.setAction(Some(sel!(navBackClicked:)));
        }
        header_group.addSubview(&back_button);

        let forward_button = NSButton::new(mtm);
        forward_button.setBezelStyle(NSBezelStyle::Toolbar);
        forward_button.setTitle(&NSString::from_str(""));
        if let Some(img) = symbol_image("chevron.right", Some("Forward")) {
            forward_button.setImage(Some(&img));
        }
        // SAFETY: setting accessibility description and target/action on `forward_button`.
        unsafe {
            let desc = NSString::from_str("Forward");
            let _: () = msg_send![&forward_button, setAccessibilityLabel: &*desc];
            forward_button.setTarget(Some(self));
            forward_button.setAction(Some(sel!(navForwardClicked:)));
        }
        header_group.addSubview(&forward_button);
        self.addSubview(&header_group);

        let header_title_field = make_heading_label(
            mtm,
            Rect::new(0.0, 0.0, 200.0, 22.0),
            "General",
            15.0,
            Weight::Semibold,
            &NSColor::labelColor(),
        );
        self.addSubview(&header_title_field);

        // 7. Detail NSScrollView + DetailDocumentView
        let detail_scroll = NSScrollView::new(mtm);
        detail_scroll.setDrawsBackground(false);
        detail_scroll.setHasVerticalScroller(true);
        detail_scroll.setAutohidesScrollers(true);
        let doc_view = DetailDocumentView::new(
            mtm,
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(560.0, 560.0)),
        );
        detail_scroll.setDocumentView(Some(&doc_view));
        self.addSubview(&detail_scroll);

        let mut state = self.ivars().borrow_mut();
        state.sidebar_effect_view = Some(sidebar_effect);
        state.search_field = Some(search_field);
        state.sort_button = Some(sort_button);
        state.filter_popup = Some(filter_popup);
        state.sidebar_scroll_view = Some(sidebar_scroll);
        state.sidebar_outline_view = Some(outline_view);
        state.sidebar_column = Some(column);
        state.detail_header_group = Some(header_group);
        state.back_button = Some(back_button);
        state.forward_button = Some(forward_button);
        state.header_title_field = Some(header_title_field);
        state.detail_scroll_view = Some(detail_scroll);
        state.detail_document_view = Some(doc_view);
    }

    pub fn set_engine_client(&self, client: EngineClient) {
        self.ivars().borrow_mut().engine_client = Some(client);
    }

    pub fn set_on_setting_changed(&self, callback: SettingChangeCallback) {
        self.ivars().borrow_mut().on_setting_changed = Some(callback);
    }

    pub fn set_appearance_dark_override(&self, dark: Option<bool>) {
        self.ivars().borrow_mut().appearance_dark_override = dark;
        self.rebuild_ui();
        self.setNeedsDisplay(true);
    }

    pub fn apply_settings_page(&self, page: &SettingsPage) {
        self.ivars().borrow_mut().settings.apply_page(page);
        self.rebuild_ui();
        self.setNeedsDisplay(true);
    }

    pub fn apply_snapshot(&self, snapshot: EngineSnapshot) {
        self.ivars().borrow_mut().snapshot = Some(snapshot);
        self.rebuild_ui();
        self.setNeedsDisplay(true);
    }

    pub fn select_pane(&self, pane: SettingsPane) {
        {
            let mut state = self.ivars().borrow_mut();
            state.selection.select(pane.clone());
        }
        self.request_provider_settings_if_needed(&pane);
        self.rebuild_ui();
        self.setNeedsDisplay(true);
    }

    fn request_provider_settings_if_needed(&self, pane: &SettingsPane) {
        if let SettingsPane::Provider(id) = pane
            && matches!(id.as_str(), "codex" | "claude" | "antigravity")
            && let Some(client) = self.ivars().borrow().engine_client.clone()
            && let Ok(pid) = ProviderId::new(id.clone())
        {
            client.get_settings(SettingsScope::Provider(pid));
        }
    }

    pub fn last_secret_terminal_command(&self) -> Option<Vec<String>> {
        self.ivars().borrow().last_secret_terminal_command.clone()
    }

    pub fn last_refreshed_provider(&self) -> Option<Option<String>> {
        self.ivars().borrow().last_refreshed_provider.clone()
    }

    pub fn selected_pane(&self) -> SettingsPane {
        self.ivars().borrow().selection.pane().clone()
    }

    fn apply_search_field_value(&self, sender: &NSSearchField) {
        let text = sender.stringValue().to_string();
        let changed = {
            let mut state = self.ivars().borrow_mut();
            if state.filter.search_text != text {
                state.filter.search_text = text;
                true
            } else {
                false
            }
        };
        if changed {
            self.rebuild_sidebar_only();
        }
    }

    pub fn set_search_text(&self, text: &str) {
        {
            let mut state = self.ivars().borrow_mut();
            state.filter.search_text = text.to_string();
            if let Some(sf) = &state.search_field {
                sf.setStringValue(&NSString::from_str(text));
            }
        }
        self.rebuild_sidebar_only();
    }

    pub fn search_field(&self) -> Option<Retained<NSSearchField>> {
        self.ivars().borrow().search_field.clone()
    }

    pub fn sidebar_outline_view(&self) -> Option<Retained<NSOutlineView>> {
        self.ivars().borrow().sidebar_outline_view.clone()
    }

    pub fn detail_scroll_view(&self) -> Option<Retained<NSScrollView>> {
        self.ivars().borrow().detail_scroll_view.clone()
    }

    /// Returns the Objective-C class names of `self` and all descendant `NSView`s.
    pub fn descendant_class_names(&self) -> Vec<String> {
        fn walk(view: &NSView, out: &mut Vec<String>) {
            out.push(view.class().name().to_string_lossy().into_owned());
            let subviews = view.subviews();
            let count = subviews.len();
            for i in 0..count {
                let child = subviews.objectAtIndex(i);
                walk(&child, out);
            }
        }
        let mut names = Vec::new();
        walk(self, &mut names);
        names
    }

    /// Renders the view and all of its native AppKit subviews into an offscreen
    /// bitmap image representation.
    pub fn render_bitmap(&self) -> Option<Retained<NSBitmapImageRep>> {
        self.layoutSubtreeIfNeeded();
        let bounds = self.bounds();
        let rep = self.bitmapImageRepForCachingDisplayInRect(bounds)?;
        self.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        Some(rep)
    }

    fn is_dark(&self) -> bool {
        if let Some(override_dark) = self.ivars().borrow().appearance_dark_override {
            return override_dark;
        }
        let app = NSApplication::sharedApplication(self.mtm());
        let name = app.effectiveAppearance().name();
        name.to_string().contains("Dark")
    }

    fn recompute_layout(&self) -> (SidebarModel, SettingsWindowLayout) {
        let bounds = self.bounds();
        let win_size = Size::new(bounds.size.width.max(320.0), bounds.size.height.max(240.0));
        let mut state = self.ivars().borrow_mut();
        let sidebar_model =
            SidebarModel::from_snapshot(&state.filter, state.snapshot.as_ref(), true);
        let p_model = pane_model(
            state.selection.pane(),
            &state.settings,
            state.snapshot.as_ref(),
        );
        let layout = layout_settings_window(
            win_size,
            state.sidebar_width,
            0.0,
            &state.selection,
            &state.filter,
            &sidebar_model,
            &p_model,
        );
        state.layout = Some(layout.clone());
        (sidebar_model, layout)
    }

    fn rebuild_ui(&self) {
        let (sidebar_model, layout) = self.recompute_layout();
        self.sync_sidebar_subviews(&sidebar_model, &layout);
        self.sync_detail_subviews(&layout);
        if let Some(win) = self.window() {
            win.recalculateKeyViewLoop();
        }
    }

    fn rebuild_sidebar_only(&self) {
        let (sidebar_model, layout) = self.recompute_layout();
        self.sync_sidebar_subviews(&sidebar_model, &layout);
        if let Some(win) = self.window() {
            win.recalculateKeyViewLoop();
        }
        self.setNeedsDisplay(true);
    }

    fn rebuild_detail_only(&self) {
        let (_sidebar_model, layout) = self.recompute_layout();
        self.sync_detail_subviews(&layout);
        if let Some(win) = self.window() {
            win.recalculateKeyViewLoop();
        }
    }

    fn sync_sidebar_subviews(&self, sidebar_model: &SidebarModel, layout: &SettingsWindowLayout) {
        let mtm = self.mtm();
        let dark = self.is_dark();
        let palette = Palette { dark };

        let mut items: Vec<Retained<SidebarRowItem>> = Vec::new();
        let mut selected_row_index: Option<usize> = None;

        let selected_pane = self.ivars().borrow().selection.pane().clone();
        let col_w = (layout.sidebar.frame.w - 20.0).max(120.0);

        for pane in &sidebar_model.app_panes {
            let token_str = format!("pane:{}", pane.persistence_token());
            if *pane == selected_pane {
                selected_row_index = Some(items.len());
            }

            let cell = NSView::new(mtm);
            cell.setFrame(ns_rect(Rect::new(0.0, 0.0, col_w, 28.0)));
            if let Some(style) = pane.chip_style() {
                let chip = SettingsIconView::new(
                    mtm,
                    Rect::new(6.0, 4.0, 20.0, 20.0),
                    IconDrawKind::SidebarChip(style),
                );
                cell.addSubview(&chip);
            }
            let title = pane.title();
            let label = make_label(
                mtm,
                Rect::new(34.0, 5.0, (col_w - 42.0).max(60.0), 18.0),
                &title,
                13.0,
                Weight::Medium,
                &palette.text_primary(),
                NSTextAlignment::Left,
                1,
                true,
            );
            cell.addSubview(&label);
            items.push(SidebarRowItem::new(mtm, token_str, 28.0, true, cell));
        }

        // Providers section header row (`AXHeading AXDescription="Providers, N on"`)
        let header_cell = NSView::new(mtm);
        header_cell.setFrame(ns_rect(Rect::new(0.0, 0.0, col_w, 24.0)));
        let header_desc = format!("Providers, {}", sidebar_model.providers_header_count_text);
        // SAFETY: configuring `header_cell` as an `AXHeading` accessibility element.
        unsafe {
            let role = NSString::from_str("AXHeading");
            let desc = NSString::from_str(&header_desc);
            let _: () = msg_send![&header_cell, setAccessibilityElement: true];
            let _: () = msg_send![&header_cell, setAccessibilityRole: &*role];
            let _: () = msg_send![&header_cell, setAccessibilityLabel: &*desc];
        }
        let prov_title = make_label(
            mtm,
            Rect::new(6.0, 5.0, 100.0, 15.0),
            "Providers",
            11.0,
            Weight::Semibold,
            &palette.text_secondary(),
            NSTextAlignment::Left,
            1,
            true,
        );
        let prov_count = make_label(
            mtm,
            Rect::new((col_w - 64.0).max(80.0), 5.0, 56.0, 15.0),
            &sidebar_model.providers_header_count_text,
            11.0,
            Weight::Medium,
            &palette.text_secondary(),
            NSTextAlignment::Right,
            1,
            true,
        );
        header_cell.addSubview(&prov_title);
        header_cell.addSubview(&prov_count);
        items.push(SidebarRowItem::new(
            mtm,
            "header:providers".to_string(),
            24.0,
            false,
            header_cell,
        ));

        // Provider rows
        for provider in &sidebar_model.providers {
            let token_str = format!("provider:{}", provider.id);
            if matches!(&selected_pane, SettingsPane::Provider(id) if id == &provider.id) {
                selected_row_index = Some(items.len());
            }

            let cell = NSView::new(mtm);
            cell.setFrame(ns_rect(Rect::new(0.0, 0.0, col_w, 28.0)));
            let glyph = SettingsIconView::new(
                mtm,
                Rect::new(6.0, 4.0, 20.0, 20.0),
                IconDrawKind::ProviderGlyph {
                    provider_id: provider.id.clone(),
                    enabled: provider.enabled,
                    dark,
                },
            );
            cell.addSubview(&glyph);

            let text_color = if provider.enabled {
                palette.text_primary()
            } else {
                palette.text_secondary()
            };
            let label = make_label(
                mtm,
                Rect::new(34.0, 5.0, (col_w - 58.0).max(60.0), 18.0),
                &provider.display_name,
                13.0,
                Weight::Regular,
                &text_color,
                NSTextAlignment::Left,
                1,
                true,
            );
            // SAFETY: setting `accessibilityValue` on the provider label so AX trees report
            // `"<Name> — Provider service status: Operational"` (or `"<Name> — Disabled"`).
            unsafe {
                let ax_val = NSString::from_str(&provider.accessibility_label);
                let _: () = msg_send![&label, setAccessibilityValue: &*ax_val];
            }
            cell.addSubview(&label);

            if let Some(dot_color) = provider.status_dot {
                let dot = SettingsIconView::new(
                    mtm,
                    Rect::new(col_w - 16.0, 11.0, 6.0, 6.0),
                    IconDrawKind::StatusDot(dot_color),
                );
                cell.addSubview(&dot);
            }
            items.push(SidebarRowItem::new(mtm, token_str, 28.0, true, cell));
        }

        if sidebar_model.shows_empty_filter_state {
            let empty_cell = NSView::new(mtm);
            empty_cell.setFrame(ns_rect(Rect::new(0.0, 0.0, col_w, 54.0)));
            let msg = make_label(
                mtm,
                Rect::new(6.0, 4.0, (col_w - 12.0).max(80.0), 16.0),
                "No matching providers",
                12.0,
                Weight::Regular,
                &palette.text_secondary(),
                NSTextAlignment::Left,
                1,
                true,
            );
            let clear_btn = NSButton::new(mtm);
            clear_btn.setFrame(ns_rect(Rect::new(6.0, 24.0, 104.0, 24.0)));
            clear_btn.setBezelStyle(NSBezelStyle::Push);
            clear_btn.setTitle(&NSString::from_str("Clear Filters"));
            // SAFETY: `self` owns `clear_btn` and outlives it.
            unsafe {
                clear_btn.setTarget(Some(self));
                clear_btn.setAction(Some(sel!(clearFiltersClicked:)));
            }
            empty_cell.addSubview(&msg);
            empty_cell.addSubview(&clear_btn);
            items.push(SidebarRowItem::new(
                mtm,
                "empty:no_providers".to_string(),
                54.0,
                false,
                empty_cell,
            ));
        }

        let (
            sidebar_effect,
            search_field,
            sort_button,
            filter_popup,
            sidebar_scroll,
            outline_view,
            column,
            enabled_only,
        ) = {
            let mut state = self.ivars().borrow_mut();
            state.sidebar_items = items;
            (
                state.sidebar_effect_view.clone(),
                state.search_field.clone(),
                state.sort_button.clone(),
                state.filter_popup.clone(),
                state.sidebar_scroll_view.clone(),
                state.sidebar_outline_view.clone(),
                state.sidebar_column.clone(),
                state.filter.enabled_only,
            )
        };

        if let Some(eff) = sidebar_effect {
            eff.setFrame(ns_rect(layout.sidebar.frame));
        }
        if let Some(sf) = search_field {
            sf.setFrame(ns_rect(layout.sidebar.search_bar.search_field_rect));
        }
        if let Some(sb) = sort_button {
            sb.setFrame(ns_rect(layout.sidebar.search_bar.sort_button_rect));
        }
        if let Some(fp) = filter_popup {
            fp.setFrame(ns_rect(layout.sidebar.search_bar.filter_button_rect));
            fp.removeAllItems();
            if let Some(menu) = fp.menu() {
                // SAFETY: `addItemWithTitle:action:keyEquivalent:` with `None` action is safe on `NSMenu`.
                unsafe {
                    let first = menu.addItemWithTitle_action_keyEquivalent(
                        &NSString::from_str(""),
                        None,
                        &NSString::from_str(""),
                    );
                    if let Some(img) = symbol_image("chevron.down", Some("Provider sorting")) {
                        first.setImage(Some(&img));
                    }
                    let item1 = menu.addItemWithTitle_action_keyEquivalent(
                        &NSString::from_str("Only show enabled providers"),
                        None,
                        &NSString::from_str(""),
                    );
                    item1.setTag(1);
                    item1.setState(if enabled_only {
                        NSControlStateValueOn
                    } else {
                        NSControlStateValueOff
                    });
                    let item2 = menu.addItemWithTitle_action_keyEquivalent(
                        &NSString::from_str("Restore default order"),
                        None,
                        &NSString::from_str(""),
                    );
                    item2.setTag(2);
                }
            }
        }
        if let Some(col) = column {
            col.setWidth(col_w);
        }
        if let Some(sc) = sidebar_scroll {
            let scroll_rect = Rect::new(
                0.0,
                80.0,
                layout.sidebar.frame.w,
                (layout.sidebar.frame.h - 80.0).max(80.0),
            );
            sc.setFrame(ns_rect(scroll_rect));
        }
        if let Some(ov) = outline_view {
            self.ivars().borrow().suppress_outline_selection.set(true);
            ov.reloadData();
            if let Some(sel_idx) = selected_row_index {
                let index_set = NSIndexSet::indexSetWithIndex(sel_idx);
                ov.selectRowIndexes_byExtendingSelection(&index_set, false);
            } else {
                // SAFETY: `deselectAll:` with `None` sender is safe on `NSOutlineView`.
                unsafe {
                    ov.deselectAll(None);
                }
            }
            self.ivars().borrow().suppress_outline_selection.set(false);
        }
    }

    fn sync_detail_subviews(&self, layout: &SettingsWindowLayout) {
        let mtm = self.mtm();
        let dark = self.is_dark();
        let palette = Palette { dark };

        let (header_group, back_btn, fwd_btn, title_field, detail_scroll) = {
            let mut state = self.ivars().borrow_mut();
            state.control_bindings.clear();
            (
                state.detail_header_group.clone(),
                state.back_button.clone(),
                state.forward_button.clone(),
                state.header_title_field.clone(),
                state.detail_scroll_view.clone(),
            )
        };

        if let Some(grp) = header_group {
            grp.setFrame(ns_rect(layout.detail_header.nav_group_rect));
        }
        if let Some(back) = back_btn {
            let r = layout.detail_header.back_button_rect;
            let g = layout.detail_header.nav_group_rect;
            back.setFrame(ns_rect(Rect::new(r.x - g.x, r.y - g.y, r.w, r.h)));
            back.setEnabled(layout.detail_header.can_go_back);
        }
        if let Some(fwd) = fwd_btn {
            let r = layout.detail_header.forward_button_rect;
            let g = layout.detail_header.nav_group_rect;
            fwd.setFrame(ns_rect(Rect::new(r.x - g.x, r.y - g.y, r.w, r.h)));
            fwd.setEnabled(layout.detail_header.can_go_forward);
        }
        if let Some(tf) = title_field {
            if let Some(tr) = layout.detail_header.title_rect {
                tf.setHidden(false);
                tf.setFrame(ns_rect(tr));
                tf.setStringValue(&NSString::from_str(&layout.detail_header.title));
                tf.setTextColor(Some(&palette.text_primary()));
                // SAFETY: updating accessibility label on the heading text field.
                unsafe {
                    let desc = NSString::from_str(&layout.detail_header.title);
                    let _: () = msg_send![&tf, setAccessibilityLabel: &*desc];
                }
            } else {
                tf.setHidden(true);
            }
        }

        let Some(scroll) = detail_scroll else {
            return;
        };
        scroll.setFrame(ns_rect(layout.detail_viewport_rect));

        let doc_h = layout
            .detail_content_height
            .max(layout.detail_viewport_rect.h);
        let doc_view = DetailDocumentView::new(
            mtm,
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(layout.detail_viewport_rect.w, doc_h),
            ),
        );

        let dx = layout.detail_viewport_rect.x;
        let dy = DETAIL_HEADER_HEIGHT;
        let to_doc = |r: Rect| Rect::new(r.x - dx, r.y - dy, r.w, r.h);

        // 1. About hero card
        if let Some(hero) = &layout.about_hero {
            let card_doc = to_doc(hero.card_rect);
            let card_view = SettingsGroupCardView::new(
                mtm,
                card_doc,
                CardViewState {
                    dark,
                    ..Default::default()
                },
            );
            let to_card = |r: Rect| {
                let d = to_doc(r);
                Rect::new(d.x - card_doc.x, d.y - card_doc.y, d.w, d.h)
            };

            let icon_view =
                SettingsIconView::new(mtm, to_card(hero.icon_rect), IconDrawKind::AppHeroIcon);
            // SAFETY: exposing `icon_view` as an `AXImage` with description `"AriadUsage"`.
            unsafe {
                let role = NSString::from_str("AXImage");
                let desc = NSString::from_str(&hero.app_name);
                let _: () = msg_send![&icon_view, setAccessibilityElement: true];
                let _: () = msg_send![&icon_view, setAccessibilityRole: &*role];
                let _: () = msg_send![&icon_view, setAccessibilityLabel: &*desc];
            }
            card_view.addSubview(&icon_view);

            let name_btn = NSButton::new(mtm);
            name_btn.setFrame(ns_rect(to_card(hero.name_rect)));
            name_btn.setBordered(false);
            name_btn.setTitle(&NSString::from_str(&hero.app_name));
            name_btn.setFont(Some(&system_font(18.0, Weight::Bold)));
            let tag = self.register_binding(ControlBinding::OpenUrl(hero.homepage_url.clone()));
            name_btn.setTag(tag);
            // SAFETY: `self` owns `name_btn` and outlives it.
            unsafe {
                name_btn.setTarget(Some(self));
                name_btn.setAction(Some(sel!(controlActionClicked:)));
            }
            card_view.addSubview(&name_btn);

            let ver_label = make_label(
                mtm,
                to_card(hero.version_rect),
                &hero.version_text,
                12.0,
                Weight::Medium,
                &palette.text_secondary(),
                NSTextAlignment::Center,
                1,
                true,
            );
            card_view.addSubview(&ver_label);

            let tagline_label = make_label(
                mtm,
                to_card(hero.tagline_rect),
                &hero.tagline,
                12.0,
                Weight::Regular,
                &palette.text_secondary(),
                NSTextAlignment::Center,
                1,
                true,
            );
            card_view.addSubview(&tagline_label);
            doc_view.addSubview(&card_view);
        }

        // 2. Grouped form sections
        for section in &layout.sections {
            self.populate_section_subviews(&doc_view, section, dx, dy, palette);
        }

        // 3. General footer (`Version` + `Quit AriadUsage` button)
        if let Some(footer) = &layout.general_footer {
            let ver_label = make_label(
                mtm,
                to_doc(footer.version_rect),
                &footer.version_text,
                12.0,
                Weight::Regular,
                &palette.text_secondary(),
                NSTextAlignment::Left,
                1,
                true,
            );
            doc_view.addSubview(&ver_label);

            let quit_btn =
                self.make_action_button(to_doc(footer.quit_button.rect), &footer.quit_button, true);
            doc_view.addSubview(&quit_btn);
        }

        // 4. Centered footer (About copyright/license footer)
        if let Some((rect, text)) = &layout.centered_footer {
            let centered = make_label(
                mtm,
                to_doc(*rect),
                text,
                11.0,
                Weight::Regular,
                &palette.text_secondary(),
                NSTextAlignment::Center,
                1,
                true,
            );
            doc_view.addSubview(&centered);
        }

        scroll.setDocumentView(Some(&doc_view));
        self.ivars().borrow_mut().detail_document_view = Some(doc_view);
    }

    fn populate_section_subviews(
        &self,
        doc_view: &DetailDocumentView,
        section: &FormSectionLayout,
        dx: f64,
        dy: f64,
        palette: Palette,
    ) {
        let mtm = self.mtm();
        let to_doc = |r: Rect| Rect::new(r.x - dx, r.y - dy, r.w, r.h);

        if let (Some(title), Some(h_rect)) = (&section.header_title, section.header_rect) {
            let heading = make_heading_label(
                mtm,
                to_doc(h_rect),
                title,
                13.0,
                Weight::Semibold,
                &palette.text_primary(),
            );
            doc_view.addSubview(&heading);
        }

        let card_doc = to_doc(section.card_rect);
        let to_card = |r: Rect| {
            let d = to_doc(r);
            Rect::new(d.x - card_doc.x, d.y - card_doc.y, d.w, d.h)
        };

        let mut divider_ys = Vec::new();
        let mut layout_preview_boxes = None;
        for row in &section.rows {
            if row.show_bottom_divider {
                divider_ys.push(to_card(row.rect).max_y() - 1.0);
            }
            if let FormControlLayout::MenuBarLayoutPreview(geom) = &row.control {
                layout_preview_boxes = Some((
                    to_card(geom.preview_box_rect),
                    to_card(geom.preview_pill_rect),
                    to_card(geom.strip_box_rect),
                ));
            }
        }

        let card_view = SettingsGroupCardView::new(
            mtm,
            card_doc,
            CardViewState {
                dark: palette.dark,
                divider_ys,
                layout_preview_boxes,
            },
        );

        for row in &section.rows {
            self.populate_row_subviews(&card_view, row, &to_card, palette);
        }
        doc_view.addSubview(&card_view);

        for (f_rect, line) in &section.footer_lines {
            let footer_label = make_label(
                mtm,
                to_doc(*f_rect),
                line,
                11.0,
                Weight::Regular,
                &palette.text_secondary(),
                NSTextAlignment::Left,
                3,
                true,
            );
            doc_view.addSubview(&footer_label);
        }
    }

    fn populate_row_subviews(
        &self,
        card_view: &SettingsGroupCardView,
        row: &FormRowLayout,
        to_card: &impl Fn(Rect) -> Rect,
        palette: Palette,
    ) {
        let mtm = self.mtm();
        let row_enabled = row.row_enabled;
        let title_color = if row_enabled {
            palette.text_primary()
        } else {
            palette.text_disabled()
        };
        let sub_color = if row_enabled {
            palette.text_secondary()
        } else {
            palette.text_disabled()
        };

        // External link rows render as a single clickable NSButton with leading/trailing icons.
        if let FormControlLayout::ExternalLink {
            leading_icon_rect,
            trailing_icon_rect,
            symbol,
            url,
        } = &row.control
        {
            let lead = SettingsIconView::new(
                mtm,
                to_card(*leading_icon_rect),
                IconDrawKind::LinkSymbol {
                    symbol,
                    dark: palette.dark,
                },
            );
            card_view.addSubview(&lead);

            let trail = SettingsIconView::new(
                mtm,
                to_card(*trailing_icon_rect),
                IconDrawKind::ExternalArrow { dark: palette.dark },
            );
            card_view.addSubview(&trail);

            let btn = NSButton::new(mtm);
            let btn_rect = to_card(row.title_rect.unwrap_or(row.rect));
            btn.setFrame(ns_rect(btn_rect));
            btn.setBordered(false);
            btn.setAlignment(NSTextAlignment::Left);
            btn.setTitle(&NSString::from_str(&row.title));
            btn.setFont(Some(&system_font(13.0, Weight::Regular)));
            let tag = self.register_binding(ControlBinding::OpenUrl(url.clone()));
            btn.setTag(tag);
            // SAFETY: `self` owns `btn` and outlives it.
            unsafe {
                btn.setTarget(Some(self));
                btn.setAction(Some(sel!(controlActionClicked:)));
            }
            card_view.addSubview(&btn);
            return;
        }

        if let Some(tr) = row.title_rect {
            let title_label = make_label(
                mtm,
                to_card(tr),
                &row.title,
                13.0,
                Weight::Regular,
                &title_color,
                NSTextAlignment::Left,
                1,
                row_enabled,
            );
            card_view.addSubview(&title_label);
        }

        if let (Some(sub), Some(sr)) = (&row.subtitle, row.subtitle_rect) {
            let sub_label = make_label(
                mtm,
                to_card(sr),
                sub,
                11.0,
                Weight::Regular,
                &sub_color,
                NSTextAlignment::Left,
                3,
                row_enabled,
            );
            card_view.addSubview(&sub_label);
        }

        match &row.control {
            FormControlLayout::Toggle {
                rect,
                value,
                enabled,
                setting_id,
            } => {
                let sw = NSSwitch::new(mtm);
                sw.setFrame(ns_rect(to_card(*rect)));
                sw.setState(if *value {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
                sw.setEnabled(*enabled && row_enabled);
                // SAFETY: setting accessibility title and target/action on `NSSwitch`.
                unsafe {
                    let title = NSString::from_str(&row.title);
                    let _: () = msg_send![&sw, setAccessibilityLabel: &*title];
                    if let Some(id) = setting_id {
                        let tag = self.register_binding(ControlBinding::Toggle {
                            setting_id: id.clone(),
                        });
                        sw.setTag(tag);
                        sw.setTarget(Some(self));
                        sw.setAction(Some(sel!(switchToggled:)));
                    }
                }
                card_view.addSubview(&sw);
            }
            FormControlLayout::Choice {
                rect,
                selected_label,
                options,
                enabled,
                setting_id,
            } => {
                let popup = self.make_pulldown_choice_popup(
                    to_card(*rect),
                    selected_label,
                    options,
                    *enabled && row_enabled,
                    setting_id.clone(),
                );
                card_view.addSubview(&popup);
            }
            FormControlLayout::Buttons { buttons } => {
                for b in buttons {
                    let btn = self.make_action_button(to_card(b.rect), b, row_enabled);
                    card_view.addSubview(&btn);
                }
            }
            FormControlLayout::ShortcutRecorder {
                rect,
                placeholder,
                enabled,
            } => {
                let sf = NSSearchField::new(mtm);
                sf.setFrame(ns_rect(to_card(*rect)));
                sf.setPlaceholderString(Some(&NSString::from_str(placeholder)));
                sf.setEnabled(*enabled && row_enabled);
                card_view.addSubview(&sf);
            }
            FormControlLayout::TextField {
                rect,
                value,
                placeholder,
                enabled,
            } => {
                let tf = NSTextField::new(mtm);
                tf.setFrame(ns_rect(to_card(*rect)));
                tf.setStringValue(&NSString::from_str(value));
                tf.setPlaceholderString(Some(&NSString::from_str(placeholder)));
                tf.setEnabled(*enabled && row_enabled);
                card_view.addSubview(&tf);
            }
            FormControlLayout::Slider {
                value_label_rect,
                slider_rect,
                reset_button_rect,
                value,
                min,
                max,
                value_label,
                reset_title,
                enabled,
            } => {
                let is_en = *enabled && row_enabled;
                let val_lbl = make_label(
                    mtm,
                    to_card(*value_label_rect),
                    value_label,
                    12.0,
                    Weight::Regular,
                    &sub_color,
                    NSTextAlignment::Right,
                    1,
                    is_en,
                );
                card_view.addSubview(&val_lbl);

                let slider = NSSlider::new(mtm);
                slider.setFrame(ns_rect(to_card(*slider_rect)));
                slider.setMinValue(*min);
                slider.setMaxValue(*max);
                slider.setDoubleValue(*value);
                slider.setEnabled(is_en);
                card_view.addSubview(&slider);

                let reset_btn = NSButton::new(mtm);
                reset_btn.setFrame(ns_rect(to_card(*reset_button_rect)));
                reset_btn.setBezelStyle(NSBezelStyle::Push);
                reset_btn.setTitle(&NSString::from_str(reset_title));
                reset_btn.setEnabled(is_en);
                card_view.addSubview(&reset_btn);
            }
            FormControlLayout::StepperPopup {
                field_rect,
                stepper_rect,
                popup_rect,
                value_text,
                popup_label,
                enabled,
            } => {
                let is_en = *enabled && row_enabled;
                let tf = NSTextField::new(mtm);
                tf.setFrame(ns_rect(to_card(*field_rect)));
                tf.setStringValue(&NSString::from_str(value_text));
                tf.setAlignment(NSTextAlignment::Right);
                tf.setEnabled(is_en);
                card_view.addSubview(&tf);

                let stepper = NSStepper::new(mtm);
                stepper.setFrame(ns_rect(to_card(*stepper_rect)));
                stepper.setEnabled(is_en);
                // SAFETY: setting accessibility description on `NSStepper`.
                unsafe {
                    let desc = NSString::from_str(value_text);
                    let _: () = msg_send![&stepper, setAccessibilityLabel: &*desc];
                }
                card_view.addSubview(&stepper);

                let popup = self.make_bordered_popup(to_card(*popup_rect), popup_label, is_en);
                card_view.addSubview(&popup);
            }
            FormControlLayout::StatusNote {
                text_rect,
                lines,
                command_rect,
                copy_button_rect,
                command,
            } => {
                let joined = lines.join(" ");
                let note_lbl = make_label(
                    mtm,
                    to_card(*text_rect),
                    &joined,
                    11.0,
                    Weight::Regular,
                    &palette.text_secondary(),
                    NSTextAlignment::Left,
                    2,
                    true,
                );
                card_view.addSubview(&note_lbl);

                if let (Some(cmd), Some(cr)) = (command, command_rect) {
                    let cmd_lbl = make_label(
                        mtm,
                        to_card(*cr),
                        cmd,
                        10.5,
                        Weight::Medium,
                        &palette.text_primary(),
                        NSTextAlignment::Left,
                        1,
                        true,
                    );
                    // SAFETY: `NSFontWeightRegular` is an immutable AppKit font weight constant.
                    let mono = NSFont::monospacedSystemFontOfSize_weight(10.5, unsafe {
                        NSFontWeightRegular
                    });
                    cmd_lbl.setFont(Some(&mono));
                    card_view.addSubview(&cmd_lbl);

                    if let Some(cbr) = copy_button_rect {
                        let copy_btn = NSButton::new(mtm);
                        copy_btn.setFrame(ns_rect(to_card(*cbr)));
                        copy_btn.setBezelStyle(NSBezelStyle::Push);
                        copy_btn.setTitle(&NSString::from_str("Copy"));
                        let tag = self.register_binding(ControlBinding::CopyCommand(cmd.clone()));
                        copy_btn.setTag(tag);
                        // SAFETY: setting accessibility description and target/action on `copy_btn`.
                        unsafe {
                            let desc = NSString::from_str("Copy command");
                            let _: () = msg_send![&copy_btn, setAccessibilityLabel: &*desc];
                            copy_btn.setTarget(Some(self));
                            copy_btn.setAction(Some(sel!(controlActionClicked:)));
                        }
                        card_view.addSubview(&copy_btn);
                    }
                }
            }
            FormControlLayout::MenuBarLayoutPreview(geom) => {
                self.populate_layout_preview_subviews(
                    card_view,
                    geom,
                    to_card,
                    row_enabled,
                    palette,
                );
            }
            FormControlLayout::Checkbox {
                rect,
                value,
                enabled,
                setting_id,
            } => {
                let is_en = *enabled && row_enabled;
                let cb = NSButton::new(mtm);
                cb.setFrame(ns_rect(to_card(*rect)));
                cb.setButtonType(NSButtonType::Switch);
                cb.setTitle(&NSString::from_str(&row.title));
                cb.setState(if *value {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
                cb.setEnabled(is_en);
                // SAFETY: setting accessibility description and target/action on `cb`.
                unsafe {
                    let desc = NSString::from_str(&row.title);
                    let _: () = msg_send![&cb, setAccessibilityLabel: &*desc];
                    if let Some(id) = setting_id {
                        let tag = self.register_binding(ControlBinding::Toggle {
                            setting_id: id.clone(),
                        });
                        cb.setTag(tag);
                        cb.setTarget(Some(self));
                        cb.setAction(Some(sel!(checkboxToggled:)));
                    }
                }
                card_view.addSubview(&cb);
            }
            FormControlLayout::BorderedChoice {
                rect,
                selected_label,
                options,
                enabled,
                setting_id,
            } => {
                let popup = self.make_bordered_choice_popup(
                    to_card(*rect),
                    selected_label,
                    options,
                    *enabled && row_enabled,
                    setting_id.clone(),
                );
                card_view.addSubview(&popup);
            }
            FormControlLayout::Segmented {
                segmented_rect,
                trailing_note_rect,
                labels,
                selected_index,
                trailing_note,
                enabled,
            } => {
                let is_en = *enabled && row_enabled;
                let seg = NSSegmentedControl::new(mtm);
                seg.setFrame(ns_rect(to_card(*segmented_rect)));
                seg.setSegmentCount(labels.len() as isize);
                for (i, lbl) in labels.iter().enumerate() {
                    seg.setLabel_forSegment(&NSString::from_str(lbl), i as isize);
                    let w = (lbl.chars().count() as f64 * 6.8 + 18.0).max(44.0);
                    seg.setWidth_forSegment(w, i as isize);
                }
                if *selected_index < labels.len() {
                    seg.setSelectedSegment(*selected_index as isize);
                }
                seg.setEnabled(is_en);
                // SAFETY: setting accessibility label on `NSSegmentedControl`.
                unsafe {
                    let desc = NSString::from_str(&row.title);
                    let _: () = msg_send![&seg, setAccessibilityLabel: &*desc];
                }
                card_view.addSubview(&seg);

                if let (Some(note), Some(nr)) = (trailing_note, trailing_note_rect) {
                    let note_lbl = make_label(
                        mtm,
                        to_card(*nr),
                        note,
                        11.5,
                        Weight::Regular,
                        &sub_color,
                        NSTextAlignment::Left,
                        1,
                        is_en,
                    );
                    card_view.addSubview(&note_lbl);
                }
            }
            FormControlLayout::ReadOnlyValue {
                value_rect,
                value,
                monospaced,
            } => {
                if !value.is_empty() {
                    let align = if row.title.is_empty() {
                        NSTextAlignment::Left
                    } else {
                        NSTextAlignment::Right
                    };
                    let val_lbl = make_label(
                        mtm,
                        to_card(*value_rect),
                        value,
                        12.0,
                        Weight::Regular,
                        &sub_color,
                        align,
                        if row.title.is_empty() { 2 } else { 1 },
                        row_enabled,
                    );
                    if *monospaced {
                        // SAFETY: `NSFontWeightRegular` is an immutable AppKit font weight constant.
                        let mono = NSFont::monospacedSystemFontOfSize_weight(11.5, unsafe {
                            NSFontWeightRegular
                        });
                        val_lbl.setFont(Some(&mono));
                    }
                    card_view.addSubview(&val_lbl);
                }
            }
            FormControlLayout::SecretStatus {
                status_rect,
                button,
                is_set,
                status_text,
            } => {
                let status_color = if *is_set {
                    palette.text_primary()
                } else {
                    palette.text_secondary()
                };
                let status_lbl = make_label(
                    mtm,
                    to_card(*status_rect),
                    status_text,
                    12.0,
                    Weight::Medium,
                    &status_color,
                    NSTextAlignment::Right,
                    1,
                    row_enabled,
                );
                card_view.addSubview(&status_lbl);

                let btn = self.make_action_button(to_card(button.rect), button, row_enabled);
                card_view.addSubview(&btn);
            }
            FormControlLayout::ColorHex {
                field_rect,
                swatch_rect,
                hex_value,
                placeholder,
                swatch_rgb,
                enabled,
            } => {
                let is_en = *enabled && row_enabled;
                let base_label = if row.title.is_empty() {
                    "Accent color"
                } else {
                    row.title.as_str()
                };
                let tf = NSTextField::new(mtm);
                tf.setFrame(ns_rect(to_card(*field_rect)));
                tf.setStringValue(&NSString::from_str(hex_value));
                tf.setPlaceholderString(Some(&NSString::from_str(placeholder)));
                tf.setEnabled(is_en);
                // SAFETY: setting accessibility description on hex `NSTextField`.
                unsafe {
                    let desc = NSString::from_str(&format!("{base_label}, Hex color value"));
                    let _: () = msg_send![&tf, setAccessibilityLabel: &*desc];
                }
                card_view.addSubview(&tf);

                // SAFETY: `initWithFrame:` is `NSColorWell`'s designated initializer.
                let well: Retained<NSColorWell> = unsafe {
                    msg_send![
                        NSColorWell::alloc(mtm),
                        initWithFrame: ns_rect(to_card(*swatch_rect))
                    ]
                };
                well.setColor(&rgb_color(*swatch_rgb, 1.0));
                well.setEnabled(is_en);
                // SAFETY: setting accessibility description on `NSColorWell`.
                unsafe {
                    let desc = NSString::from_str(base_label);
                    let _: () = msg_send![&well, setAccessibilityLabel: &*desc];
                }
                card_view.addSubview(&well);
            }
            FormControlLayout::ProviderHeader {
                icon_rect,
                title_rect,
                subtitle_rect,
                refresh_button,
                toggle_rect,
                provider_id,
                display_name,
                subtitle,
                enabled,
                toggle_setting_id,
            } => {
                let glyph = SettingsIconView::new(
                    mtm,
                    to_card(*icon_rect),
                    IconDrawKind::ProviderGlyph {
                        provider_id: provider_id.clone(),
                        enabled: *enabled,
                        dark: palette.dark,
                    },
                );
                card_view.addSubview(&glyph);

                let name_lbl = make_label(
                    mtm,
                    to_card(*title_rect),
                    display_name,
                    15.0,
                    Weight::Semibold,
                    &palette.text_primary(),
                    NSTextAlignment::Left,
                    1,
                    true,
                );
                card_view.addSubview(&name_lbl);

                let sub_lbl = make_label(
                    mtm,
                    to_card(*subtitle_rect),
                    subtitle,
                    11.5,
                    Weight::Regular,
                    &palette.text_secondary(),
                    NSTextAlignment::Left,
                    1,
                    true,
                );
                card_view.addSubview(&sub_lbl);

                let ref_btn = NSButton::new(mtm);
                ref_btn.setFrame(ns_rect(to_card(refresh_button.rect)));
                ref_btn.setBordered(false);
                ref_btn.setTitle(&NSString::from_str(""));
                if let Some(img) = symbol_image("arrow.clockwise", Some("Refresh")) {
                    ref_btn.setImage(Some(&img));
                } else {
                    ref_btn.setTitle(&NSString::from_str("↻"));
                }
                ref_btn.setEnabled(refresh_button.enabled && row_enabled);
                let ref_tag = self
                    .register_binding(ControlBinding::ButtonAction(refresh_button.action.clone()));
                ref_btn.setTag(ref_tag);
                // SAFETY: setting accessibility description and target/action on `ref_btn`.
                unsafe {
                    let desc = NSString::from_str("Refresh");
                    let _: () = msg_send![&ref_btn, setAccessibilityLabel: &*desc];
                    ref_btn.setTarget(Some(self));
                    ref_btn.setAction(Some(sel!(controlActionClicked:)));
                }
                card_view.addSubview(&ref_btn);

                let sw = NSSwitch::new(mtm);
                sw.setFrame(ns_rect(to_card(*toggle_rect)));
                sw.setState(if *enabled {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
                sw.setEnabled(toggle_setting_id.is_some() && row_enabled);
                // SAFETY: setting accessibility description `"Enabled"` and target/action on `NSSwitch`.
                unsafe {
                    let desc = NSString::from_str("Enabled");
                    let _: () = msg_send![&sw, setAccessibilityLabel: &*desc];
                    if let Some(id) = toggle_setting_id {
                        let tag = self.register_binding(ControlBinding::Toggle {
                            setting_id: id.clone(),
                        });
                        sw.setTag(tag);
                        sw.setTarget(Some(self));
                        sw.setAction(Some(sel!(switchToggled:)));
                    }
                }
                card_view.addSubview(&sw);
            }
            FormControlLayout::UsageWindowBar {
                title_rect,
                reset_rect,
                bar_rect,
                pace_rect,
                title,
                percent_used,
                percent_label,
                reset_label,
                pace_label,
                accent_rgb,
            } => {
                let left_text = format!("{title} {percent_label}");
                let left_lbl = make_label(
                    mtm,
                    to_card(*title_rect),
                    &left_text,
                    12.0,
                    Weight::Medium,
                    &palette.text_primary(),
                    NSTextAlignment::Left,
                    1,
                    true,
                );
                card_view.addSubview(&left_lbl);

                let right_lbl = make_label(
                    mtm,
                    to_card(*reset_rect),
                    reset_label,
                    11.5,
                    Weight::Regular,
                    &palette.text_secondary(),
                    NSTextAlignment::Right,
                    1,
                    true,
                );
                card_view.addSubview(&right_lbl);

                let bar_view = SettingsIconView::new(
                    mtm,
                    to_card(*bar_rect),
                    IconDrawKind::UsageProgressBar {
                        percent_used: *percent_used,
                        accent_rgb: *accent_rgb,
                        dark: palette.dark,
                    },
                );
                // SAFETY: exposing `bar_view` as `"Usage remaining"` in the AX tree.
                unsafe {
                    let role = NSString::from_str("AXValueIndicator");
                    let desc = NSString::from_str("Usage remaining");
                    let _: () = msg_send![&bar_view, setAccessibilityElement: true];
                    let _: () = msg_send![&bar_view, setAccessibilityRole: &*role];
                    let _: () = msg_send![&bar_view, setAccessibilityLabel: &*desc];
                }
                card_view.addSubview(&bar_view);

                if let (Some(pace), Some(pr)) = (pace_label, pace_rect) {
                    let pace_lbl = make_label(
                        mtm,
                        to_card(*pr),
                        pace,
                        11.0,
                        Weight::Regular,
                        &palette.text_secondary(),
                        NSTextAlignment::Left,
                        1,
                        true,
                    );
                    card_view.addSubview(&pace_lbl);
                }
            }
            FormControlLayout::EmptyStateCard {
                icon_rect,
                title_rect,
                subtitle_rect,
                symbol,
                title,
                subtitle,
            } => {
                let icon_view = SettingsIconView::new(
                    mtm,
                    to_card(*icon_rect),
                    IconDrawKind::SidebarChip(SidebarChipStyle::Symbol {
                        symbol,
                        color: Rgb {
                            r: 88,
                            g: 86,
                            b: 214,
                        },
                    }),
                );
                card_view.addSubview(&icon_view);

                let t_lbl = make_label(
                    mtm,
                    to_card(*title_rect),
                    title,
                    14.0,
                    Weight::Semibold,
                    &palette.text_primary(),
                    NSTextAlignment::Center,
                    1,
                    true,
                );
                card_view.addSubview(&t_lbl);

                let s_lbl = make_label(
                    mtm,
                    to_card(*subtitle_rect),
                    subtitle,
                    12.0,
                    Weight::Regular,
                    &palette.text_secondary(),
                    NSTextAlignment::Center,
                    2,
                    true,
                );
                card_view.addSubview(&s_lbl);
            }
            FormControlLayout::SpendSummaryGrid {
                currency_rect,
                coverage_rect,
                metric_boxes,
                token_items,
                footer_rect,
                currency_label,
                coverage_label,
                footer_note,
            } => {
                card_view.addSubview(&make_label(
                    mtm,
                    to_card(*currency_rect),
                    currency_label,
                    12.0,
                    Weight::Semibold,
                    &palette.text_primary(),
                    NSTextAlignment::Left,
                    1,
                    true,
                ));
                card_view.addSubview(&make_label(
                    mtm,
                    to_card(*coverage_rect),
                    coverage_label,
                    11.5,
                    Weight::Regular,
                    &palette.text_secondary(),
                    NSTextAlignment::Left,
                    1,
                    true,
                ));
                for (box_r, m_title, m_val) in metric_boxes {
                    let bc = to_card(*box_r);
                    let bg = SettingsIconView::new(
                        mtm,
                        bc,
                        IconDrawKind::MetricBox { dark: palette.dark },
                    );
                    card_view.addSubview(&bg);
                    card_view.addSubview(&make_label(
                        mtm,
                        Rect::new(bc.x + 10.0, bc.y + 8.0, (bc.w - 20.0).max(40.0), 15.0),
                        m_title,
                        11.0,
                        Weight::Medium,
                        &palette.text_secondary(),
                        NSTextAlignment::Left,
                        1,
                        true,
                    ));
                    card_view.addSubview(&make_label(
                        mtm,
                        Rect::new(bc.x + 10.0, bc.y + 26.0, (bc.w - 20.0).max(40.0), 20.0),
                        m_val,
                        14.0,
                        Weight::Semibold,
                        &palette.text_primary(),
                        NSTextAlignment::Left,
                        1,
                        true,
                    ));
                }
                for (tok_r, t_title, t_val) in token_items {
                    let tc = to_card(*tok_r);
                    card_view.addSubview(&make_label(
                        mtm,
                        Rect::new(tc.x, tc.y + 2.0, tc.w, 15.0),
                        t_title,
                        11.0,
                        Weight::Regular,
                        &palette.text_secondary(),
                        NSTextAlignment::Left,
                        1,
                        true,
                    ));
                    card_view.addSubview(&make_label(
                        mtm,
                        Rect::new(tc.x, tc.y + 18.0, tc.w, 16.0),
                        t_val,
                        12.0,
                        Weight::Medium,
                        &palette.text_primary(),
                        NSTextAlignment::Left,
                        1,
                        true,
                    ));
                }
                card_view.addSubview(&make_label(
                    mtm,
                    to_card(*footer_rect),
                    footer_note,
                    11.0,
                    Weight::Regular,
                    &palette.text_secondary(),
                    NSTextAlignment::Left,
                    1,
                    true,
                ));
            }
            FormControlLayout::ExternalLink { .. } => {}
        }
    }

    fn populate_layout_preview_subviews(
        &self,
        card_view: &SettingsGroupCardView,
        geom: &MenuBarLayoutPreviewGeometry,
        to_card: &impl Fn(Rect) -> Rect,
        row_enabled: bool,
        palette: Palette,
    ) {
        let mtm = self.mtm();
        let is_en = geom.enabled && row_enabled;
        let label_color = if is_en {
            palette.text_primary()
        } else {
            palette.text_disabled()
        };
        let sec_color = if is_en {
            palette.text_secondary()
        } else {
            palette.text_disabled()
        };

        card_view.addSubview(&make_label(
            mtm,
            to_card(geom.scope_label_rect),
            "Edit layout for",
            12.0,
            Weight::Regular,
            &label_color,
            NSTextAlignment::Left,
            1,
            is_en,
        ));
        card_view.addSubview(&self.make_pulldown_choice_popup(
            to_card(geom.scope_picker_rect),
            &geom.scope_label,
            &[],
            is_en,
            None,
        ));
        card_view.addSubview(&make_label(
            mtm,
            to_card(geom.preset_label_rect),
            "Preset",
            12.0,
            Weight::Regular,
            &label_color,
            NSTextAlignment::Right,
            1,
            is_en,
        ));
        card_view.addSubview(&self.make_pulldown_choice_popup(
            to_card(geom.preset_picker_rect),
            &geom.preset_label,
            &[],
            is_en,
            None,
        ));

        card_view.addSubview(&make_label(
            mtm,
            to_card(geom.preview_title_rect),
            "Default layout preview",
            11.0,
            Weight::Medium,
            &sec_color,
            NSTextAlignment::Left,
            1,
            is_en,
        ));

        let pill_card = to_card(geom.preview_pill_rect);
        let mini_bars = SettingsIconView::new(
            mtm,
            Rect::new(pill_card.x + 8.0, pill_card.y + 6.0, 14.0, 12.0),
            IconDrawKind::BarsMini {
                dark: palette.dark,
                enabled: is_en,
            },
        );
        card_view.addSubview(&mini_bars);
        card_view.addSubview(&make_label(
            mtm,
            Rect::new(pill_card.x + 26.0, pill_card.y + 4.0, 42.0, 16.0),
            &geom.preview_percent_text,
            12.0,
            Weight::Semibold,
            &label_color,
            NSTextAlignment::Left,
            1,
            is_en,
        ));

        let reset_btn = NSButton::new(mtm);
        reset_btn.setFrame(ns_rect(to_card(geom.reset_button_rect)));
        reset_btn.setBezelStyle(NSBezelStyle::Push);
        reset_btn.setTitle(&NSString::from_str("Reset"));
        reset_btn.setEnabled(is_en);
        card_view.addSubview(&reset_btn);

        card_view.addSubview(&make_label(
            mtm,
            to_card(geom.strip_title_rect),
            "Menu bar strip",
            11.0,
            Weight::Medium,
            &sec_color,
            NSTextAlignment::Left,
            1,
            is_en,
        ));
        card_view.addSubview(&make_label(
            mtm,
            to_card(geom.line_label_rect),
            "Line 1",
            11.0,
            Weight::Medium,
            &sec_color,
            NSTextAlignment::Left,
            1,
            is_en,
        ));

        for chip in &geom.strip_chips {
            let btn = NSButton::new(mtm);
            btn.setFrame(ns_rect(to_card(chip.rect)));
            btn.setBezelStyle(NSBezelStyle::Badge);
            btn.setTitle(&NSString::from_str(&chip.label));
            btn.setEnabled(is_en);
            card_view.addSubview(&btn);
        }

        let remove_btn = NSButton::new(mtm);
        remove_btn.setFrame(ns_rect(to_card(geom.remove_button_rect)));
        remove_btn.setBezelStyle(NSBezelStyle::AccessoryBar);
        remove_btn.setTitle(&NSString::from_str("Drag here to remove"));
        remove_btn.setEnabled(is_en);
        card_view.addSubview(&remove_btn);

        for group in &geom.palette_groups {
            card_view.addSubview(&make_label(
                mtm,
                to_card(group.title_rect),
                &group.title,
                11.0,
                Weight::Semibold,
                &sec_color,
                NSTextAlignment::Left,
                1,
                is_en,
            ));
            for chip in &group.chips {
                let btn = NSButton::new(mtm);
                btn.setFrame(ns_rect(to_card(chip.rect)));
                btn.setBezelStyle(NSBezelStyle::Badge);
                btn.setTitle(&NSString::from_str(&chip.label));
                btn.setEnabled(is_en);
                card_view.addSubview(&btn);
            }
        }

        card_view.addSubview(&make_label(
            mtm,
            to_card(geom.conditionals_title_rect),
            "Conditionals",
            11.0,
            Weight::Semibold,
            &sec_color,
            NSTextAlignment::Left,
            1,
            is_en,
        ));
        let cond_btn = NSButton::new(mtm);
        cond_btn.setFrame(ns_rect(to_card(geom.new_conditional_button_rect)));
        cond_btn.setBezelStyle(NSBezelStyle::Badge);
        cond_btn.setTitle(&NSString::from_str("New Conditional"));
        cond_btn.setEnabled(is_en);
        card_view.addSubview(&cond_btn);

        card_view.addSubview(&make_label(
            mtm,
            to_card(geom.size_label_rect),
            "Size",
            11.0,
            Weight::Regular,
            &sec_color,
            NSTextAlignment::Left,
            1,
            is_en,
        ));
        card_view.addSubview(&self.make_bordered_popup(
            to_card(geom.size_popup_rect),
            "Medium",
            is_en,
        ));

        card_view.addSubview(&make_label(
            mtm,
            to_card(geom.gap_label_rect),
            "Gap",
            11.0,
            Weight::Regular,
            &sec_color,
            NSTextAlignment::Left,
            1,
            is_en,
        ));
        card_view.addSubview(&self.make_bordered_popup(
            to_card(geom.gap_popup_rect),
            "Normal",
            is_en,
        ));

        card_view.addSubview(&make_label(
            mtm,
            to_card(geom.vertical_label_rect),
            "Vertical",
            11.0,
            Weight::Regular,
            &sec_color,
            NSTextAlignment::Left,
            1,
            is_en,
        ));
        card_view.addSubview(&make_label(
            mtm,
            to_card(geom.vertical_value_rect),
            "0 pt",
            11.0,
            Weight::Regular,
            &sec_color,
            NSTextAlignment::Right,
            1,
            is_en,
        ));
        let v_stepper = NSStepper::new(mtm);
        v_stepper.setFrame(ns_rect(to_card(geom.vertical_stepper_rect)));
        v_stepper.setEnabled(is_en);
        // SAFETY: setting accessibility description `"Vertical"` on `v_stepper`.
        unsafe {
            let desc = NSString::from_str("Vertical");
            let _: () = msg_send![&v_stepper, setAccessibilityLabel: &*desc];
        }
        card_view.addSubview(&v_stepper);

        let mono_cb = NSButton::new(mtm);
        mono_cb.setFrame(ns_rect(to_card(geom.monospaced_checkbox_rect)));
        mono_cb.setButtonType(NSButtonType::Switch);
        mono_cb.setTitle(&NSString::from_str("Monospaced digits"));
        mono_cb.setState(NSControlStateValueOn);
        mono_cb.setEnabled(is_en);
        card_view.addSubview(&mono_cb);

        let labels_cb = NSButton::new(mtm);
        labels_cb.setFrame(ns_rect(to_card(geom.window_labels_checkbox_rect)));
        labels_cb.setButtonType(NSButtonType::Switch);
        labels_cb.setTitle(&NSString::from_str("Show window labels"));
        labels_cb.setState(NSControlStateValueOff);
        labels_cb.setEnabled(is_en);
        card_view.addSubview(&labels_cb);
    }

    fn make_pulldown_choice_popup(
        &self,
        frame: Rect,
        selected_label: &str,
        options: &[ChoiceItem],
        enabled: bool,
        setting_id: Option<SettingId>,
    ) -> Retained<NSPopUpButton> {
        let mtm = self.mtm();
        // SAFETY: `initWithFrame:pullsDown:` is `NSPopUpButton`'s designated initializer.
        // Using `pullsDown = true` and `bordered = false` produces `AXMenuButton` with
        // `AXTitle = selected_label`, matching SwiftUI `.pickerStyle(.menu)`.
        let popup: Retained<NSPopUpButton> = unsafe {
            msg_send![
                NSPopUpButton::alloc(mtm),
                initWithFrame: ns_rect(frame),
                pullsDown: true
            ]
        };
        popup.setBordered(false);
        popup.setAlignment(NSTextAlignment::Right);
        popup.setFont(Some(&system_font(12.0, Weight::Regular)));
        if let Some(menu) = popup.menu() {
            // SAFETY: `addItemWithTitle:action:keyEquivalent:` with `None` action is safe on `NSMenu`
            // and preserves duplicate titles without deduplicating item 0.
            unsafe {
                let _ = menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(selected_label),
                    None,
                    &NSString::from_str(""),
                );
                for opt in options {
                    let item = menu.addItemWithTitle_action_keyEquivalent(
                        &NSString::from_str(opt.label()),
                        None,
                        &NSString::from_str(""),
                    );
                    item.setState(if opt.label() == selected_label {
                        NSControlStateValueOn
                    } else {
                        NSControlStateValueOff
                    });
                }
            }
        }
        popup.setEnabled(enabled);
        // SAFETY: setting accessibility title and target/action on `popup`.
        unsafe {
            let title = NSString::from_str(selected_label);
            let _: () = msg_send![&popup, setAccessibilityTitle: &*title];
            if let Some(id) = setting_id {
                let tag = self.register_binding(ControlBinding::Choice {
                    setting_id: id,
                    options: options.to_vec(),
                    pulls_down: true,
                });
                popup.setTag(tag);
                popup.setTarget(Some(self));
                popup.setAction(Some(sel!(choicePopupChanged:)));
            }
        }
        popup
    }

    fn make_bordered_choice_popup(
        &self,
        frame: Rect,
        selected_label: &str,
        options: &[ChoiceItem],
        enabled: bool,
        setting_id: Option<SettingId>,
    ) -> Retained<NSPopUpButton> {
        let mtm = self.mtm();
        // SAFETY: `initWithFrame:pullsDown:` with `pullsDown = false` and `bordered = true`
        // produces a standard `AXPopUpButton`.
        let popup: Retained<NSPopUpButton> = unsafe {
            msg_send![
                NSPopUpButton::alloc(mtm),
                initWithFrame: ns_rect(frame),
                pullsDown: false
            ]
        };
        popup.setBordered(true);
        popup.setFont(Some(&system_font(12.0, Weight::Regular)));
        if options.is_empty() {
            popup.addItemWithTitle(&NSString::from_str(selected_label));
        } else {
            let mut selected_idx = 0isize;
            for (idx, opt) in options.iter().enumerate() {
                popup.addItemWithTitle(&NSString::from_str(opt.label()));
                if opt.label() == selected_label {
                    selected_idx = idx as isize;
                }
            }
            popup.selectItemAtIndex(selected_idx);
        }
        popup.setEnabled(enabled);
        // SAFETY: setting target/action on `popup`.
        unsafe {
            if let Some(id) = setting_id {
                let tag = self.register_binding(ControlBinding::Choice {
                    setting_id: id,
                    options: options.to_vec(),
                    pulls_down: false,
                });
                popup.setTag(tag);
                popup.setTarget(Some(self));
                popup.setAction(Some(sel!(choicePopupChanged:)));
            }
        }
        popup
    }

    fn make_bordered_popup(
        &self,
        frame: Rect,
        selected_label: &str,
        enabled: bool,
    ) -> Retained<NSPopUpButton> {
        self.make_bordered_choice_popup(frame, selected_label, &[], enabled, None)
    }

    fn make_action_button(
        &self,
        frame: Rect,
        layout: &FormButtonLayout,
        row_enabled: bool,
    ) -> Retained<NSButton> {
        let mtm = self.mtm();
        let btn = NSButton::new(mtm);
        btn.setFrame(ns_rect(frame));
        btn.setBezelStyle(NSBezelStyle::Push);
        btn.setTitle(&NSString::from_str(&layout.title));
        btn.setEnabled(layout.enabled && row_enabled);
        let tag = self.register_binding(ControlBinding::ButtonAction(layout.action.clone()));
        btn.setTag(tag);
        // SAFETY: `self` owns `btn` and outlives it.
        unsafe {
            btn.setTarget(Some(self));
            btn.setAction(Some(sel!(controlActionClicked:)));
        }
        btn
    }

    fn register_binding(&self, binding: ControlBinding) -> isize {
        let mut state = self.ivars().borrow_mut();
        let idx = state.control_bindings.len();
        state.control_bindings.push(binding);
        idx as isize
    }

    fn apply_setting_change(&self, setting_id: &SettingId, value: serde_json::Value) {
        let (client, callback) = {
            let mut state = self.ivars().borrow_mut();
            state.settings.set_local(setting_id.as_str(), value.clone());
            (
                state.engine_client.clone(),
                state.on_setting_changed.clone(),
            )
        };
        if let Some(c) = client {
            c.set_setting(setting_id.clone(), value.clone());
        }
        if let Some(cb) = callback {
            cb(setting_id, value);
        }
        if setting_id.as_str().ends_with(".enabled") {
            self.rebuild_ui();
        } else {
            self.rebuild_detail_only();
        }
        self.setNeedsDisplay(true);
    }

    fn execute_button_action(&self, action: FormButtonAction) {
        match action {
            FormButtonAction::None => {}
            FormButtonAction::QuitApp => {
                let app = NSApplication::sharedApplication(self.mtm());
                app.terminate(None);
            }
            FormButtonAction::OpenUrl(url) => open_external_url(&url),
            FormButtonAction::RefreshProvider(provider_id) => {
                let client = {
                    let mut state = self.ivars().borrow_mut();
                    state.last_refreshed_provider = Some(provider_id.clone());
                    state.engine_client.clone()
                };
                if let Some(c) = client {
                    let pid = provider_id.and_then(|s| ProviderId::new(s).ok());
                    c.refresh(pid);
                }
            }
            FormButtonAction::LaunchSecretSetInTerminal { setting_id } => {
                let args = secret_set_command_args(&setting_id);
                let cmd_line = secret_set_terminal_command(&setting_id);
                self.ivars().borrow_mut().last_secret_terminal_command = Some(args.to_vec());
                copy_to_pasteboard(&cmd_line);
                if std::env::var_os("NEXTEST").is_none()
                    && std::env::var_os("ARIADUSAGE_DISABLE_TERMINAL_LAUNCH").is_none()
                {
                    let _ = std::process::Command::new("/usr/bin/open")
                        .args(["-a", "Terminal"])
                        .spawn();
                }
            }
            FormButtonAction::RunEngineAction { action_id } => {
                let client = self.ivars().borrow().engine_client.clone();
                if let Some(c) = client
                    && let Ok(aid) = ActionId::new(action_id)
                {
                    c.run_action(aid, Some(true));
                }
            }
            FormButtonAction::CopyText(text) => copy_to_pasteboard(&text),
        }
    }

    fn handle_key_equivalent(&self, event: &NSEvent) -> bool {
        let flags = event.modifierFlags();
        if !flags.contains(NSEventModifierFlags::Command) {
            return false;
        }
        let Some(chars) = event.charactersIgnoringModifiers() else {
            return false;
        };
        match chars.to_string().as_str() {
            "[" => {
                let moved = self.ivars().borrow_mut().selection.go_back();
                if moved {
                    self.rebuild_ui();
                    self.setNeedsDisplay(true);
                }
                moved
            }
            "]" => {
                let moved = self.ivars().borrow_mut().selection.go_forward();
                if moved {
                    self.rebuild_ui();
                    self.setNeedsDisplay(true);
                }
                moved
            }
            "f" | "F" => {
                if let Some(sf) = self.search_field()
                    && let Some(win) = self.window()
                {
                    let _ = win.makeFirstResponder(Some(&sf));
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    fn draw_chrome(&self) {
        let palette = Palette {
            dark: self.is_dark(),
        };
        let Some(layout) = self.ivars().borrow().layout.clone() else {
            return;
        };

        // Sidebar and detail backgrounds
        palette.sidebar_bg().setFill();
        NSBezierPath::fillRect(ns_rect(layout.sidebar.frame));

        let detail_rect = Rect::new(
            layout.divider_rect.max_x(),
            0.0,
            (layout.window_size.w - layout.divider_rect.max_x()).max(0.0),
            layout.window_size.h,
        );
        palette.detail_bg().setFill();
        NSBezierPath::fillRect(ns_rect(detail_rect));

        // 1px vertical sidebar divider and detail header bottom divider
        palette.divider().setFill();
        NSBezierPath::fillRect(ns_rect(layout.divider_rect));
        let header_div = Rect::new(
            layout.detail_header.frame.x,
            layout.detail_header.frame.max_y() - 1.0,
            layout.detail_header.frame.w,
            1.0,
        );
        NSBezierPath::fillRect(ns_rect(header_div));
    }
}

fn open_external_url(url_str: &str) {
    let ns = NSString::from_str(url_str);
    if let Some(url) = NSURL::URLWithString(&ns) {
        let ws = NSWorkspace::sharedWorkspace();
        let _ = ws.openURL(&url);
    }
}

fn copy_to_pasteboard(text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    // SAFETY: `NSPasteboardTypeString` is an immutable AppKit constant.
    let pb_type = unsafe { NSPasteboardTypeString };
    pb.setString_forType(&NSString::from_str(text), pb_type);
}

fn fill_stroke_rounded(
    rect: Rect,
    radius: f64,
    fill: Retained<NSColor>,
    border: Retained<NSColor>,
) {
    let path =
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(rect), radius, radius);
    fill.setFill();
    path.fill();
    border.setStroke();
    path.setLineWidth(1.0);
    path.stroke();
}

fn draw_sidebar_chip(rect: Rect, style: SidebarChipStyle) {
    match style {
        SidebarChipStyle::AppIcon => {
            draw_app_brand_icon(rect, 5.0);
        }
        SidebarChipStyle::Symbol { symbol, color } => {
            rgb_color(color, 1.0).setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(rect), 5.0, 5.0).fill();
            let white = srgb(1.0, 1.0, 1.0, 0.96);
            draw_chip_glyph(rect, symbol, white);
        }
    }
}

fn draw_app_brand_icon(rect: Rect, radius: f64) {
    fill_stroke_rounded(
        rect,
        radius,
        srgb(0.11, 0.12, 0.17, 1.0),
        srgb(0.28, 0.32, 0.45, 0.85),
    );
    let pad_x = rect.w * 0.20;
    let bar_h = (rect.h * 0.13).max(2.0);
    let gap = (rect.h * 0.09).max(1.5);
    let total_h = 3.0 * bar_h + 2.0 * gap;
    let start_y = rect.y + (rect.h - total_h) / 2.0;
    let max_w = rect.w - 2.0 * pad_x;
    let widths = [max_w * 0.92, max_w * 0.68, max_w * 0.44];
    let colors = [
        srgb(0.96, 0.65, 0.14, 1.0),
        srgb(0.22, 0.74, 0.97, 1.0),
        srgb(0.34, 0.84, 0.55, 1.0),
    ];
    for (i, (w, c)) in widths.iter().zip(colors.iter()).enumerate() {
        let by = start_y + (i as f64) * (bar_h + gap);
        let r = Rect::new(rect.x + pad_x, by, *w, bar_h);
        c.setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            ns_rect(r),
            bar_h / 2.0,
            bar_h / 2.0,
        )
        .fill();
    }
}

fn draw_chip_glyph(rect: Rect, symbol: &str, color: Retained<NSColor>) {
    color.setFill();
    color.setStroke();
    let cx = rect.mid_x();
    let cy = rect.mid_y();
    match symbol {
        "gearshape.fill" => {
            let outer = Rect::new(cx - 5.0, cy - 5.0, 10.0, 10.0);
            let p = NSBezierPath::bezierPathWithOvalInRect(ns_rect(outer));
            p.setLineWidth(2.2);
            p.stroke();
        }
        "icloud.fill" => {
            let base = Rect::new(cx - 6.0, cy - 3.0, 12.0, 6.5);
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(base), 3.0, 3.0).fill();
            let top = Rect::new(cx - 3.5, cy - 5.5, 6.5, 6.0);
            NSBezierPath::bezierPathWithOvalInRect(ns_rect(top)).fill();
        }
        "chart.bar.fill" => {
            for (dx, h) in [(-4.5, 5.0), (-1.0, 8.0), (2.5, 10.5)] {
                let r = Rect::new(cx + dx, cy + 5.0 - h, 2.3, h);
                NSBezierPath::fillRect(ns_rect(r));
            }
        }
        "bell.badge.fill" => {
            let bell = Rect::new(cx - 4.5, cy - 4.5, 9.0, 8.0);
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(bell), 3.5, 3.5).fill();
            let clapper = Rect::new(cx - 1.5, cy + 4.0, 3.0, 1.8);
            NSBezierPath::bezierPathWithOvalInRect(ns_rect(clapper)).fill();
        }
        "menubar.rectangle" => {
            let box_r = Rect::new(cx - 6.0, cy - 4.5, 12.0, 9.0);
            let p =
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(box_r), 2.0, 2.0);
            p.setLineWidth(1.4);
            p.stroke();
            NSBezierPath::fillRect(ns_rect(Rect::new(cx - 5.5, cy - 4.0, 11.0, 2.5)));
        }
        "filemenu.and.selection" => {
            for dy in [-3.5, 0.0, 3.5] {
                NSBezierPath::fillRect(ns_rect(Rect::new(cx - 5.0, cy + dy - 0.8, 10.0, 1.6)));
            }
        }
        "slider.horizontal.3" => {
            for (dy, kx) in [(-3.5, -2.0), (0.0, 2.0), (3.5, -1.0)] {
                NSBezierPath::fillRect(ns_rect(Rect::new(cx - 5.5, cy + dy - 0.6, 11.0, 1.2)));
                let knob = Rect::new(cx + kx - 1.5, cy + dy - 1.8, 3.6, 3.6);
                NSBezierPath::bezierPathWithOvalInRect(ns_rect(knob)).fill();
            }
        }
        "bolt.horizontal.circle.fill" => {
            let bolt = NSBezierPath::bezierPath();
            bolt.moveToPoint(NSPoint::new(cx + 1.0, cy - 5.5));
            bolt.lineToPoint(NSPoint::new(cx - 4.0, cy + 0.5));
            bolt.lineToPoint(NSPoint::new(cx - 0.5, cy + 0.5));
            bolt.lineToPoint(NSPoint::new(cx - 1.5, cy + 5.5));
            bolt.lineToPoint(NSPoint::new(cx + 4.0, cy - 0.5));
            bolt.lineToPoint(NSPoint::new(cx + 0.5, cy - 0.5));
            bolt.closePath();
            bolt.fill();
        }
        "puzzlepiece.extension.fill" => {
            let body = Rect::new(cx - 4.5, cy - 4.0, 8.5, 8.5);
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(body), 2.0, 2.0).fill();
            let tab = Rect::new(cx + 2.5, cy - 1.8, 3.5, 3.6);
            NSBezierPath::bezierPathWithOvalInRect(ns_rect(tab)).fill();
        }
        _ => {
            let dot = Rect::new(cx - 3.5, cy - 3.5, 7.0, 7.0);
            NSBezierPath::bezierPathWithOvalInRect(ns_rect(dot)).fill();
        }
    }
}

fn draw_provider_glyph(rect: Rect, provider_id: &str, color: Retained<NSColor>) {
    color.setStroke();
    color.setFill();
    let cx = rect.mid_x();
    let cy = rect.mid_y();
    match provider_id {
        "codex" => {
            let outer = Rect::new(cx - 6.0, cy - 6.0, 12.0, 12.0);
            let p = NSBezierPath::bezierPathWithOvalInRect(ns_rect(outer));
            p.setLineWidth(1.5);
            p.stroke();
            let inner = Rect::new(cx - 2.2, cy - 2.2, 4.4, 4.4);
            NSBezierPath::bezierPathWithOvalInRect(ns_rect(inner)).fill();
        }
        "claude" => {
            for angle_deg in [0.0_f64, 45.0, 90.0, 135.0] {
                let rad = angle_deg.to_radians();
                let dx = 5.5 * rad.cos();
                let dy = 5.5 * rad.sin();
                let p = NSBezierPath::bezierPath();
                p.moveToPoint(NSPoint::new(cx - dx, cy - dy));
                p.lineToPoint(NSPoint::new(cx + dx, cy + dy));
                p.setLineWidth(1.5);
                p.stroke();
            }
        }
        _ => {
            let diamond = NSBezierPath::bezierPath();
            diamond.moveToPoint(NSPoint::new(cx, cy - 6.0));
            diamond.lineToPoint(NSPoint::new(cx + 6.0, cy));
            diamond.lineToPoint(NSPoint::new(cx, cy + 6.0));
            diamond.lineToPoint(NSPoint::new(cx - 6.0, cy));
            diamond.closePath();
            diamond.setLineWidth(1.5);
            diamond.stroke();
        }
    }
}

fn draw_bars_mini_glyph(rect: Rect, color: Retained<NSColor>) {
    color.setFill();
    for (i, w_frac) in [0.95, 0.70, 0.45].iter().enumerate() {
        let r = Rect::new(
            rect.x,
            rect.y + 2.0 + (i as f64) * 4.0,
            rect.w * w_frac,
            2.2,
        );
        NSBezierPath::fillRect(ns_rect(r));
    }
}

fn draw_link_symbol(rect: Rect, symbol: &str, color: Retained<NSColor>) {
    color.setStroke();
    let cx = rect.mid_x();
    let cy = rect.mid_y();
    let p = NSBezierPath::bezierPath();
    match symbol {
        "chevron.left.slash.chevron.right" => {
            p.moveToPoint(NSPoint::new(cx - 2.5, cy - 3.5));
            p.lineToPoint(NSPoint::new(cx - 6.0, cy));
            p.lineToPoint(NSPoint::new(cx - 2.5, cy + 3.5));
            p.moveToPoint(NSPoint::new(cx + 2.5, cy - 3.5));
            p.lineToPoint(NSPoint::new(cx + 6.0, cy));
            p.lineToPoint(NSPoint::new(cx + 2.5, cy + 3.5));
        }
        "tag" => {
            let r = Rect::new(cx - 5.0, cy - 4.0, 10.0, 8.0);
            let box_p =
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(r), 2.0, 2.0);
            box_p.setLineWidth(1.3);
            box_p.stroke();
            return;
        }
        _ => {
            let r = Rect::new(cx - 5.5, cy - 4.5, 11.0, 9.0);
            let bubble =
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(r), 3.0, 3.0);
            bubble.setLineWidth(1.3);
            bubble.stroke();
            return;
        }
    }
    p.setLineWidth(1.3);
    p.stroke();
}

fn draw_external_arrow(rect: Rect, color: Retained<NSColor>) {
    color.setStroke();
    let p = NSBezierPath::bezierPath();
    p.moveToPoint(NSPoint::new(rect.x + 2.5, rect.max_y() - 2.5));
    p.lineToPoint(NSPoint::new(rect.max_x() - 2.5, rect.y + 2.5));
    p.moveToPoint(NSPoint::new(rect.x + 5.5, rect.y + 2.5));
    p.lineToPoint(NSPoint::new(rect.max_x() - 2.5, rect.y + 2.5));
    p.lineToPoint(NSPoint::new(rect.max_x() - 2.5, rect.max_y() - 5.5));
    p.setLineWidth(1.4);
    p.stroke();
}
