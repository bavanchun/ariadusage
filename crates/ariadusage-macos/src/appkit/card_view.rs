// Ported from CodexBar Sources/CodexBar/MenuCardRefreshMonitor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuPresentation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuCardItems.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+OverviewSpend.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! The usage card hosted as an `NSMenuItem` view.
//!
//! The view lays out labels and bars from [`card_layout::layout`] and draws
//! the dividers and the hover background itself. A new model replaces the
//! subviews in place, so the menu item and its position never change.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSBezierPath, NSBitmapImageFileType, NSBitmapImageRep, NSColor, NSEvent, NSFont,
    NSFontTextStyle, NSFontTextStyleBody, NSFontTextStyleCaption1, NSFontTextStyleCaption2,
    NSFontTextStyleFootnote, NSFontTextStyleHeadline, NSFontTextStyleSubheadline,
    NSFontTextStyleTitle2, NSFontWeightBold, NSFontWeightMedium, NSFontWeightRegular,
    NSFontWeightSemibold, NSLineBreakMode, NSResponder, NSTextAlignment, NSTextField,
    NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{
    NSDictionary, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use super::color::ns_color;
use super::progress_bar_view::ProgressBarView;
use crate::card::{Accent, CardModel};
use crate::card_layout::{
    self, Align, CardLayout, CardSectionMode, Font, Item, TextColor, TextItem, TextMeasure,
    TextStyle, Truncation, Weight,
};
use crate::chart::{MiniBarPoint, MiniBarStripGeometry};
use crate::geometry::{Point, Rect, Size};
use crate::highlight_style::{self, Color};
use crate::menu_descriptor::OverviewSpendSummary;

/// Labels draw their text 2 pt inside their frame on each side.
const LABEL_INSET: f64 = 2.0;
/// The hover background is a rounded rectangle inset from the row.
const SELECTION_INSET_X: f64 = 6.0;
const SELECTION_INSET_Y: f64 = 2.0;
const SELECTION_CORNER_RADIUS: f64 = 6.0;
/// Large enough to never limit a measurement.
const UNBOUNDED: f64 = 100_000.0;

type ClickCallback = Rc<dyn Fn()>;

#[derive(Clone)]
struct MiniStripView {
    label_field: Retained<NSTextField>,
    points: Vec<MiniBarPoint>,
    max_label: Option<String>,
    strip_frame: Rect,
}

/// What the card currently shows.
pub struct CardState {
    model: Option<CardModel>,
    spend_summary: Option<OverviewSpendSummary>,
    section_mode: CardSectionMode,
    layout: Option<CardLayout>,
    highlighted: bool,
    pressed: bool,
    on_click: Option<ClickCallback>,
    labels: Vec<(Retained<NSTextField>, TextColor)>,
    bars: Vec<Retained<ProgressBarView>>,
    mini_strips: Vec<MiniStripView>,
    hovered_bar_index: Option<usize>,
    tracking_area: Option<Retained<NSTrackingArea>>,
    backdrop_dark: Option<bool>,
    subview_rebuilds: usize,
    in_place_updates: usize,
}

impl Default for CardState {
    fn default() -> Self {
        Self {
            model: None,
            spend_summary: None,
            section_mode: CardSectionMode::Full,
            layout: None,
            highlighted: false,
            pressed: false,
            on_click: None,
            labels: Vec::new(),
            bars: Vec::new(),
            mini_strips: Vec::new(),
            hovered_bar_index: None,
            tracking_area: None,
            backdrop_dark: None,
            subview_rebuilds: 0,
            in_place_updates: 0,
        }
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond calling its
    // designated initializer, which `new` does; this class does not
    // implement Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsageCardView"]
    #[ivars = RefCell<CardState>]
    pub struct CardView;

    unsafe impl NSObjectProtocol for CardView {}

    impl CardView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            self.ivars().borrow().on_click.is_some()
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            // SAFETY: `super(self)` is a valid NSView instance on the main thread.
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            let state = self.ivars().borrow();
            if hit.is_some() && (state.on_click.is_some() || !state.mini_strips.is_empty()) {
                std::ptr::from_ref::<Self>(self) as *mut NSView
            } else {
                hit.map_or(std::ptr::null_mut(), |v| Retained::as_ptr(&v) as *mut NSView)
            }
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            // SAFETY: `super(self)` is a valid NSView on the main thread.
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
            self.sync_tracking_area();
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let loc = self.convertPoint_fromView(event.locationInWindow(), None);
            self.update_mini_strip_hover(Some(Point::new(loc.x, loc.y)));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.update_mini_strip_hover(None);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            let mut state = self.ivars().borrow_mut();
            if state.on_click.is_some() {
                state.pressed = true;
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let loc = self.convertPoint_fromView(event.locationInWindow(), None);
            let bounds = self.bounds();
            let inside = loc.x >= 0.0
                && loc.y >= 0.0
                && loc.x <= bounds.size.width
                && loc.y <= bounds.size.height;
            let cb = {
                let mut state = self.ivars().borrow_mut();
                let pressed = state.pressed;
                state.pressed = false;
                if pressed && inside {
                    state.on_click.clone()
                } else {
                    None
                }
            };
            if let Some(cb) = cb {
                cb();
            }
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }
    }
);

fn ns_rect(r: Rect) -> NSRect {
    NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
}

impl CardView {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RefCell::new(CardState::default()));
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(card_layout::MIN_CARD_WIDTH, 1.0),
        );
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Registers a click callback (used on Overview rows to switch to the clicked provider tab).
    pub fn set_on_click(&self, callback: impl Fn() + 'static) {
        self.ivars().borrow_mut().on_click = Some(Rc::new(callback));
    }

    /// Shows `model` at `width` in the full provider card layout. Returns false when nothing changed.
    pub fn update(&self, model: &CardModel, width: f64) -> bool {
        self.update_section(model, CardSectionMode::Full, width)
    }

    /// Shows `model` at `width` in the compact Overview row layout (`OverviewMenuCardRowView`).
    pub fn update_overview(&self, model: &CardModel, width: f64) -> bool {
        self.update_section(model, CardSectionMode::OverviewRow, width)
    }

    /// Shows `model` at `width` in `section_mode` (`Full`, `HeaderAndUsage`, `CreditsOnly`, `ExtraUsageOnly`, `OverviewRow`).
    pub fn update_section(
        &self,
        model: &CardModel,
        section_mode: CardSectionMode,
        width: f64,
    ) -> bool {
        {
            let state = self.ivars().borrow();
            let same_width = state.layout.as_ref().is_some_and(|l| l.size.w == width);
            if same_width
                && state.section_mode == section_mode
                && state.model.as_ref() == Some(model)
            {
                return false;
            }
        }
        let mtm = self.mtm();
        let measure = AppKitMeasure::new(mtm);
        let layout = card_layout::layout_section(model, section_mode, width, &measure);
        self.apply_layout(layout, model.accent, &measure);
        {
            let mut state = self.ivars().borrow_mut();
            state.model = Some(model.clone());
            state.spend_summary = None;
            state.section_mode = section_mode;
            state.hovered_bar_index = None;
        }
        true
    }

    /// Shows `summary` at `width` in the Overview spend summary card layout (`OverviewSpendSummaryCardView`).
    pub fn update_spend_summary(&self, summary: &OverviewSpendSummary, width: f64) -> bool {
        {
            let state = self.ivars().borrow();
            let same_width = state.layout.as_ref().is_some_and(|l| l.size.w == width);
            if same_width && state.spend_summary.as_ref() == Some(summary) {
                return false;
            }
        }
        let mtm = self.mtm();
        let measure = AppKitMeasure::new(mtm);
        let layout = card_layout::spend_summary_layout(summary, width, &measure);
        self.apply_layout(layout, Accent::System, &measure);
        {
            let mut state = self.ivars().borrow_mut();
            state.model = None;
            state.spend_summary = Some(summary.clone());
            state.section_mode = CardSectionMode::Full;
            state.hovered_bar_index = None;
        }
        true
    }

    /// Whether `summary` at `width` produces the same height as the currently rendered layout.
    pub fn has_compatible_spend_summary_height(
        &self,
        summary: &OverviewSpendSummary,
        width: f64,
    ) -> bool {
        let Some(existing_h) = self.ivars().borrow().layout.as_ref().map(|l| l.size.h) else {
            return false;
        };
        let mtm = self.mtm();
        let measure = AppKitMeasure::new(mtm);
        let next = card_layout::spend_summary_layout(summary, width, &measure);
        crate::menu_session::MenuCardRefreshMonitor::has_compatible_tracked_layout(
            existing_h,
            next.size.h,
        )
    }

    /// Whether `model` at `width` in `overview_mode` produces the same height as the
    /// currently rendered layout (`MenuCardRefreshMonitor.hasCompatibleTrackedLayout`).
    pub fn has_compatible_height(
        &self,
        model: &CardModel,
        width: f64,
        overview_mode: bool,
    ) -> bool {
        let mode = if overview_mode {
            CardSectionMode::OverviewRow
        } else {
            CardSectionMode::Full
        };
        self.has_compatible_section_height(model, mode, width)
    }

    /// Whether `model` at `width` in `section_mode` produces the same height as the
    /// currently rendered layout.
    pub fn has_compatible_section_height(
        &self,
        model: &CardModel,
        section_mode: CardSectionMode,
        width: f64,
    ) -> bool {
        let Some(existing_h) = self.ivars().borrow().layout.as_ref().map(|l| l.size.h) else {
            return false;
        };
        let mtm = self.mtm();
        let measure = AppKitMeasure::new(mtm);
        let next = card_layout::layout_section(model, section_mode, width, &measure);
        crate::menu_session::MenuCardRefreshMonitor::has_compatible_tracked_layout(
            existing_h,
            next.size.h,
        )
    }

    fn apply_layout(&self, layout: CardLayout, accent: Accent, measure: &AppKitMeasure) {
        let mtm = self.mtm();
        let highlighted = self.ivars().borrow().highlighted;
        let next_size = NSSize::new(layout.size.w, layout.size.h);
        if self.frame().size != next_size {
            self.setFrameSize(next_size);
        }

        if self.try_apply_layout_in_place(&layout, accent, highlighted, measure) {
            self.setNeedsDisplay(true);
            return;
        }

        for view in self.subviews().iter() {
            view.removeFromSuperview();
        }
        let mut labels = Vec::new();
        let mut bars = Vec::new();
        let mut mini_strips = Vec::new();
        for placed in &layout.items {
            match &placed.item {
                Item::Text(item) => {
                    let field = measure.label(item, highlighted);
                    field.setFrame(label_frame(placed.frame));
                    self.addSubview(&field);
                    labels.push((field, item.color));
                }
                Item::Bar {
                    spec,
                    accessibility_label,
                } => {
                    let bar = ProgressBarView::new(
                        mtm,
                        placed.frame,
                        spec.clone(),
                        accent,
                        accessibility_label,
                    );
                    bar.set_highlighted(highlighted);
                    self.addSubview(&bar);
                    bars.push(bar);
                }
                Item::MiniBarStrip {
                    points,
                    max_label,
                    label_frame: lf,
                    strip_frame,
                } => {
                    let item = TextItem {
                        text: max_label.clone().unwrap_or_default(),
                        font: Font::new(TextStyle::Caption2, Weight::Regular),
                        color: TextColor::Secondary,
                        max_lines: 1,
                        align: Align::Right,
                        truncation: Truncation::Tail,
                    };
                    let field = measure.label(&item, highlighted);
                    field.setFrame(label_frame(*lf));
                    self.addSubview(&field);
                    labels.push((field.clone(), TextColor::Secondary));
                    mini_strips.push(MiniStripView {
                        label_field: field,
                        points: points.clone(),
                        max_label: max_label.clone(),
                        strip_frame: *strip_frame,
                    });
                }
                Item::Divider | Item::ClockIcon { .. } => {}
            }
        }
        {
            let mut state = self.ivars().borrow_mut();
            state.layout = Some(layout);
            state.labels = labels;
            state.bars = bars;
            state.mini_strips = mini_strips;
            state.subview_rebuilds += 1;
        }
        self.sync_tracking_area();
        self.setNeedsDisplay(true);
    }

    /// Updates existing label and progress-bar subviews in place when the sequence
    /// of subview-bearing items (`Text`, `Bar`, `MiniBarStrip`) matches the
    /// currently attached subviews, avoiding `removeFromSuperview` / `addSubview`
    /// churn while the menu is open.
    fn try_apply_layout_in_place(
        &self,
        layout: &CardLayout,
        accent: Accent,
        highlighted: bool,
        measure: &AppKitMeasure,
    ) -> bool {
        let mut state = self.ivars().borrow_mut();
        let Some(prev_layout) = state.layout.as_ref() else {
            return false;
        };
        if !same_subview_item_shapes(&prev_layout.items, &layout.items) {
            return false;
        }

        let mut label_idx = 0usize;
        let mut bar_idx = 0usize;
        let mut strip_idx = 0usize;

        for placed in &layout.items {
            match &placed.item {
                Item::Text(item) => {
                    let Some((field, color)) = state.labels.get_mut(label_idx) else {
                        return false;
                    };
                    measure.configure_label(field, item, highlighted, placed.frame);
                    *color = item.color;
                    label_idx += 1;
                }
                Item::Bar {
                    spec,
                    accessibility_label,
                } => {
                    let Some(bar) = state.bars.get(bar_idx) else {
                        return false;
                    };
                    bar.update(placed.frame, spec.clone(), accent, accessibility_label);
                    bar.set_highlighted(highlighted);
                    bar_idx += 1;
                }
                Item::MiniBarStrip {
                    points,
                    max_label,
                    label_frame: lf,
                    strip_frame,
                } => {
                    let item = TextItem {
                        text: max_label.clone().unwrap_or_default(),
                        font: Font::new(TextStyle::Caption2, Weight::Regular),
                        color: TextColor::Secondary,
                        max_lines: 1,
                        align: Align::Right,
                        truncation: Truncation::Tail,
                    };
                    let Some((field, color)) = state.labels.get_mut(label_idx) else {
                        return false;
                    };
                    measure.configure_label(field, &item, highlighted, *lf);
                    *color = TextColor::Secondary;
                    label_idx += 1;

                    let Some(strip) = state.mini_strips.get_mut(strip_idx) else {
                        return false;
                    };
                    strip.points = points.clone();
                    strip.max_label = max_label.clone();
                    strip.strip_frame = *strip_frame;
                    strip_idx += 1;
                }
                Item::Divider | Item::ClockIcon { .. } => {}
            }
        }

        state.layout = Some(layout.clone());
        state.in_place_updates += 1;
        true
    }

    /// Number of full subview rebuilds performed on this card view.
    pub fn subview_rebuilds(&self) -> usize {
        self.ivars().borrow().subview_rebuilds
    }

    /// Number of in-place subview updates performed on this card view without
    /// removing or recreating subviews.
    pub fn in_place_updates(&self) -> usize {
        self.ivars().borrow().in_place_updates
    }

    fn sync_tracking_area(&self) {
        let mut state = self.ivars().borrow_mut();
        if let Some(old) = state.tracking_area.take() {
            self.removeTrackingArea(&old);
        }
        if !state.mini_strips.is_empty() {
            let options = NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::ActiveAlways
                | NSTrackingAreaOptions::InVisibleRect;
            // SAFETY: `self` is a valid Objective-C object to receive tracking events.
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    self.bounds(),
                    options,
                    Some(self),
                    None,
                )
            };
            self.addTrackingArea(&area);
            state.tracking_area = Some(area);
        }
    }

    fn update_mini_strip_hover(&self, point: Option<Point>) {
        let mut changed = false;
        {
            let mut state = self.ivars().borrow_mut();
            let mut next_hover = None;
            if let Some(pt) = point {
                for strip in &state.mini_strips {
                    if let Some(idx) = MiniBarStripGeometry::hovered_index(
                        pt.x - strip.strip_frame.x,
                        pt.y - strip.strip_frame.y,
                        strip.strip_frame.w,
                        strip.strip_frame.h,
                        strip.points.len(),
                    ) {
                        next_hover = Some(idx);
                        break;
                    }
                }
            }
            if state.hovered_bar_index != next_hover {
                state.hovered_bar_index = next_hover;
                changed = true;
                for strip in &state.mini_strips {
                    if let Some(idx) = next_hover
                        && let Some(pt) = strip.points.get(idx)
                        && let Some(summary) = &pt.hover_summary
                    {
                        strip
                            .label_field
                            .setStringValue(&NSString::from_str(summary));
                        strip.label_field.setAlignment(NSTextAlignment::Left);
                    } else {
                        let fallback = strip.max_label.as_deref().unwrap_or("");
                        strip
                            .label_field
                            .setStringValue(&NSString::from_str(fallback));
                        strip.label_field.setAlignment(NSTextAlignment::Right);
                    }
                }
            }
        }
        if changed {
            self.setNeedsDisplay(true);
        }
    }

    /// Switches between the normal and the highlighted colors.
    pub fn set_highlighted(&self, highlighted: bool) {
        let (labels, bars, accent) = {
            let mut state = self.ivars().borrow_mut();
            if state.highlighted == highlighted {
                return;
            }
            state.highlighted = highlighted;
            let accent = state.model.as_ref().map_or(Accent::System, |m| m.accent);
            (state.labels.clone(), state.bars.clone(), accent)
        };
        for (field, color) in labels {
            field.setTextColor(Some(&ns_color(color.resolve(highlighted), accent)));
        }
        for bar in bars {
            bar.set_highlighted(highlighted);
        }
        self.setNeedsDisplay(true);
    }

    /// The current layout, if a model was shown.
    pub fn layout(&self) -> Option<CardLayout> {
        self.ivars().borrow().layout.clone()
    }

    /// The current card model, if a provider card was shown.
    pub fn model(&self) -> Option<CardModel> {
        self.ivars().borrow().model.clone()
    }

    /// Draws the card and its subviews into a bitmap, as on screen.
    pub fn render_bitmap(&self) -> Option<Retained<NSBitmapImageRep>> {
        let bounds = self.bounds();
        let rep = self.bitmapImageRepForCachingDisplayInRect(bounds)?;
        self.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        Some(rep)
    }

    /// Renders the card into a bitmap under an explicit dark or light `NSAppearance`
    /// with a menu backdrop so dynamic colors resolve and are legible offscreen.
    #[allow(deprecated)]
    pub fn render_bitmap_with_appearance(&self, dark: bool) -> Option<Retained<NSBitmapImageRep>> {
        // SAFETY: `NSAppearanceNameDarkAqua` and `NSAppearanceNameAqua` are valid AppKit globals.
        let name = unsafe {
            if dark {
                NSAppearanceNameDarkAqua
            } else {
                NSAppearanceNameAqua
            }
        };
        let appearance = NSAppearance::appearanceNamed(name)?;
        self.setAppearance(Some(&appearance));
        // SAFETY: `appearance` is a valid `NSAppearance` and we are on the main thread.
        unsafe {
            NSAppearance::setCurrentAppearance(Some(&appearance));
        }
        self.ivars().borrow_mut().backdrop_dark = Some(dark);
        let rep = self.render_bitmap();
        self.ivars().borrow_mut().backdrop_dark = None;
        rep
    }

    #[allow(deprecated)]
    fn draw(&self) {
        let state = self.ivars().borrow();
        if let Some(dark) = state.backdrop_dark {
            if let Some(app) = self.appearance() {
                // SAFETY: `app` is a valid `NSAppearance` on `self`.
                unsafe {
                    NSAppearance::setCurrentAppearance(Some(&app));
                }
            }
            let (r, g, b) = if dark {
                (36.0 / 255.0, 36.0 / 255.0, 39.0 / 255.0)
            } else {
                (216.0 / 255.0, 216.0 / 255.0, 218.0 / 255.0)
            };
            NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.0).setFill();
            NSBezierPath::fillRect(self.bounds());
        }
        let accent = state.model.as_ref().map_or(Accent::System, |m| m.accent);
        if let Some(color) = highlight_style::selection_background(state.highlighted) {
            let bounds = self.bounds();
            let rect = NSRect::new(
                NSPoint::new(SELECTION_INSET_X, SELECTION_INSET_Y),
                NSSize::new(
                    bounds.size.width - 2.0 * SELECTION_INSET_X,
                    bounds.size.height - 2.0 * SELECTION_INSET_Y,
                ),
            );
            ns_color(color, accent).setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                rect,
                SELECTION_CORNER_RADIUS,
                SELECTION_CORNER_RADIUS,
            )
            .fill();
        }
        let Some(layout) = &state.layout else { return };
        for placed in &layout.items {
            match &placed.item {
                Item::Divider => {
                    ns_color(card_layout::divider_color(), accent).setFill();
                    NSBezierPath::fillRect(ns_rect(placed.frame));
                }
                Item::MiniBarStrip {
                    points,
                    strip_frame,
                    ..
                } => {
                    let geom = MiniBarStripGeometry::compute(
                        *strip_frame,
                        points,
                        state.hovered_bar_index,
                    );
                    ns_color(card_layout::divider_color(), accent).setFill();
                    NSBezierPath::fillRect(ns_rect(geom.baseline));
                    let tint_role = highlight_style::progress_tint(state.highlighted).role;
                    for bar in &geom.bars {
                        ns_color(Color::with_alpha(tint_role, bar.alpha), accent).setFill();
                        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                            ns_rect(bar.rect),
                            bar.corner_radius,
                            bar.corner_radius,
                        )
                        .fill();
                    }
                }
                Item::ClockIcon { color } => {
                    draw_clock_icon(placed.frame, *color, state.highlighted, accent);
                }
                Item::Text(_) | Item::Bar { .. } => {}
            }
        }
    }
}

fn draw_clock_icon(frame: Rect, color: TextColor, highlighted: bool, accent: Accent) {
    let inset = frame.x + 1.0;
    let inset_y = frame.y + 1.0;
    let d = (frame.w - 2.0).max(2.0);
    let circle_rect = NSRect::new(NSPoint::new(inset, inset_y), NSSize::new(d, d));
    let stroke = ns_color(color.resolve(highlighted), accent);
    stroke.setStroke();
    let circle = NSBezierPath::bezierPathWithOvalInRect(circle_rect);
    circle.setLineWidth(1.2);
    circle.stroke();

    let cx = frame.x + frame.w * 0.5;
    let cy = frame.y + frame.h * 0.5;
    let r = d * 0.5;
    let hands = NSBezierPath::bezierPath();
    hands.setLineWidth(1.2);
    hands.moveToPoint(NSPoint::new(cx, cy - r * 0.55));
    hands.lineToPoint(NSPoint::new(cx, cy));
    hands.lineToPoint(NSPoint::new(cx + r * 0.42, cy + r * 0.25));
    hands.stroke();
}

/// PNG bytes of a bitmap from [`CardView::render_bitmap`].
pub fn png_bytes(rep: &NSBitmapImageRep) -> Option<Vec<u8>> {
    let properties = NSDictionary::new();
    // SAFETY: an empty property dictionary is valid for PNG encoding.
    let data =
        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties) }?;
    Some(data.to_vec())
}

/// The label frame for a text frame: labels draw inside a 2 pt inset.
fn label_frame(text: Rect) -> NSRect {
    ns_rect(Rect::new(
        text.x - LABEL_INSET,
        text.y,
        text.w + 2.0 * LABEL_INSET,
        text.h,
    ))
}

fn same_subview_item_shapes(prev: &[card_layout::Placed], next: &[card_layout::Placed]) -> bool {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum SubviewKind {
        Text,
        Bar,
        MiniStrip,
    }
    let kind = |placed: &card_layout::Placed| match &placed.item {
        Item::Text(_) => Some(SubviewKind::Text),
        Item::Bar { .. } => Some(SubviewKind::Bar),
        Item::MiniBarStrip { .. } => Some(SubviewKind::MiniStrip),
        Item::Divider | Item::ClockIcon { .. } => None,
    };
    prev.iter()
        .filter_map(kind)
        .eq(next.iter().filter_map(kind))
}

/// Measures and creates labels with the system text styles.
pub struct AppKitMeasure {
    mtm: MainThreadMarker,
    fonts: RefCell<HashMap<Font, Retained<NSFont>>>,
}

impl AppKitMeasure {
    pub fn new(mtm: MainThreadMarker) -> Self {
        Self {
            mtm,
            fonts: RefCell::new(HashMap::new()),
        }
    }

    fn font(&self, font: Font) -> Retained<NSFont> {
        self.fonts
            .borrow_mut()
            .entry(font)
            .or_insert_with(|| system_font(font))
            .clone()
    }

    /// The text size a label for `item` needs within `width`.
    fn cell_size(&self, item: &TextItem, width: f64) -> Size {
        let field = self.label(item, false);
        let bounds = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(width + 2.0 * LABEL_INSET, UNBOUNDED),
        );
        let Some(cell) = field.cell() else {
            return Size::default();
        };
        let size = cell.cellSizeForBounds(bounds);
        Size::new(size.width - 2.0 * LABEL_INSET, size.height)
    }

    /// A label configured for `item`.
    fn label(&self, item: &TextItem, highlighted: bool) -> Retained<NSTextField> {
        let field = NSTextField::labelWithString(&NSString::from_str(&item.text), self.mtm);
        self.apply_label_style(&field, item, highlighted);
        field
    }

    fn configure_label(
        &self,
        field: &NSTextField,
        item: &TextItem,
        highlighted: bool,
        frame: Rect,
    ) {
        field.setStringValue(&NSString::from_str(&item.text));
        self.apply_label_style(field, item, highlighted);
        let next_frame = label_frame(frame);
        if field.frame() != next_frame {
            field.setFrame(next_frame);
        }
    }

    fn apply_label_style(&self, field: &NSTextField, item: &TextItem, highlighted: bool) {
        field.setFont(Some(&self.font(item.font)));
        field.setTextColor(Some(&ns_color(
            item.color.resolve(highlighted),
            Accent::System,
        )));
        field.setSelectable(false);
        field.setAlignment(match item.align {
            Align::Left => NSTextAlignment::Left,
            Align::Right => NSTextAlignment::Right,
        });
        if item.max_lines == 1 {
            field.setLineBreakMode(match item.truncation {
                Truncation::Tail => NSLineBreakMode::ByTruncatingTail,
                Truncation::Middle => NSLineBreakMode::ByTruncatingMiddle,
            });
            field.setMaximumNumberOfLines(1);
        } else {
            field.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
            field.setMaximumNumberOfLines(item.max_lines as isize);
            if let Some(cell) = field.cell() {
                cell.setWraps(true);
                cell.setTruncatesLastVisibleLine(true);
            }
        }
    }
}

impl TextMeasure for AppKitMeasure {
    fn measure(&self, text: &str, font: Font, max_lines: usize, max_width: f64) -> Size {
        let item = TextItem {
            text: text.to_string(),
            font,
            color: TextColor::Primary,
            max_lines,
            align: Align::Left,
            truncation: Truncation::Tail,
        };
        let width = max_width.min(UNBOUNDED);
        let size = self.cell_size(&item, width);
        let mut height = size.h;
        if max_lines > 1 {
            // The cell measures every wrapped line; the label shows at most
            // `max_lines` of them.
            let line = self.cell_size(
                &TextItem {
                    max_lines: 1,
                    ..item
                },
                UNBOUNDED,
            );
            height = height.min(line.h * max_lines as f64);
        }
        Size::new(size.w.clamp(0.0, width).ceil(), height.ceil())
    }

    fn first_baseline(&self, font: Font) -> f64 {
        self.font(font).ascender()
    }
}

fn text_style(style: TextStyle) -> &'static NSFontTextStyle {
    // SAFETY: the text style constants are immutable AppKit globals.
    unsafe {
        match style {
            TextStyle::Title2 => NSFontTextStyleTitle2,
            TextStyle::Headline => NSFontTextStyleHeadline,
            TextStyle::Subheadline => NSFontTextStyleSubheadline,
            TextStyle::Body => NSFontTextStyleBody,
            TextStyle::Footnote => NSFontTextStyleFootnote,
            TextStyle::Caption => NSFontTextStyleCaption1,
            TextStyle::Caption2 => NSFontTextStyleCaption2,
        }
    }
}

/// The system font at the text style's size with an explicit weight, as
/// SwiftUI's `.font(.style).fontWeight(...)` resolves on macOS.
fn system_font(font: Font) -> Retained<NSFont> {
    let options = NSDictionary::new();
    // SAFETY: an empty options dictionary is valid for this call.
    let preferred =
        unsafe { NSFont::preferredFontForTextStyle_options(text_style(font.style), &options) };
    // SAFETY: the font weight constants are immutable AppKit globals.
    let weight = unsafe {
        match font.weight {
            Weight::Regular => NSFontWeightRegular,
            Weight::Medium => NSFontWeightMedium,
            Weight::Semibold => NSFontWeightSemibold,
            Weight::Bold => NSFontWeightBold,
        }
    };
    NSFont::systemFontOfSize_weight(preferred.pointSize(), weight)
}
