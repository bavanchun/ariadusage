// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuPresentation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! The usage card hosted as an `NSMenuItem` view.
//!
//! The view lays out labels and bars from [`card_layout::layout`] and draws
//! the dividers and the hover background itself. A new model replaces the
//! subviews in place, so the menu item and its position never change.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSBitmapImageFileType, NSBitmapImageRep, NSEvent, NSFont, NSFontTextStyle,
    NSFontTextStyleBody, NSFontTextStyleCaption1, NSFontTextStyleFootnote, NSFontTextStyleHeadline,
    NSFontTextStyleSubheadline, NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold,
    NSLineBreakMode, NSResponder, NSTextAlignment, NSTextField, NSView,
};
use objc2_foundation::{
    NSDictionary, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use super::color::ns_color;
use super::progress_bar_view::ProgressBarView;
use crate::card::{Accent, CardModel};
use crate::card_layout::{
    self, Align, CardLayout, Font, Item, TextColor, TextItem, TextMeasure, TextStyle, Truncation,
    Weight,
};
use crate::geometry::{Rect, Size};
use crate::highlight_style;

/// Labels draw their text 2 pt inside their frame on each side.
const LABEL_INSET: f64 = 2.0;
/// The hover background is a rounded rectangle inset from the row.
const SELECTION_INSET_X: f64 = 6.0;
const SELECTION_INSET_Y: f64 = 2.0;
const SELECTION_CORNER_RADIUS: f64 = 6.0;
/// Large enough to never limit a measurement.
const UNBOUNDED: f64 = 100_000.0;

type ClickCallback = Rc<dyn Fn()>;

/// What the card currently shows.
#[derive(Default)]
pub struct CardState {
    model: Option<CardModel>,
    overview_mode: bool,
    layout: Option<CardLayout>,
    highlighted: bool,
    pressed: bool,
    on_click: Option<ClickCallback>,
    labels: Vec<(Retained<NSTextField>, TextColor)>,
    bars: Vec<Retained<ProgressBarView>>,
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
            if hit.is_some() && self.ivars().borrow().on_click.is_some() {
                std::ptr::from_ref::<Self>(self) as *mut NSView
            } else {
                hit.map_or(std::ptr::null_mut(), |v| Retained::as_ptr(&v) as *mut NSView)
            }
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
        self.update_with_mode(model, width, false)
    }

    /// Shows `model` at `width` in the compact Overview row layout (`OverviewMenuCardRowView`).
    pub fn update_overview(&self, model: &CardModel, width: f64) -> bool {
        self.update_with_mode(model, width, true)
    }

    /// Whether `model` at `width` in `overview_mode` produces the same height as the
    /// currently rendered layout (`MenuCardRefreshMonitor.hasCompatibleTrackedLayout`).
    pub fn has_compatible_height(
        &self,
        model: &CardModel,
        width: f64,
        overview_mode: bool,
    ) -> bool {
        let Some(existing_h) = self.ivars().borrow().layout.as_ref().map(|l| l.size.h) else {
            return false;
        };
        let mtm = self.mtm();
        let measure = AppKitMeasure::new(mtm);
        let next = if overview_mode {
            card_layout::overview_layout(model, width, &measure)
        } else {
            card_layout::layout(model, width, &measure)
        };
        (existing_h - next.size.h).abs() <= 0.5
    }

    fn update_with_mode(&self, model: &CardModel, width: f64, overview_mode: bool) -> bool {
        {
            let state = self.ivars().borrow();
            let same_width = state.layout.as_ref().is_some_and(|l| l.size.w == width);
            if same_width
                && state.overview_mode == overview_mode
                && state.model.as_ref() == Some(model)
            {
                return false;
            }
        }
        let mtm = self.mtm();
        let measure = AppKitMeasure::new(mtm);
        let layout = if overview_mode {
            card_layout::overview_layout(model, width, &measure)
        } else {
            card_layout::layout(model, width, &measure)
        };
        let highlighted = self.ivars().borrow().highlighted;

        for view in self.subviews().iter() {
            view.removeFromSuperview();
        }
        self.setFrameSize(NSSize::new(layout.size.w, layout.size.h));
        let mut labels = Vec::new();
        let mut bars = Vec::new();
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
                        model.accent,
                        accessibility_label,
                    );
                    bar.set_highlighted(highlighted);
                    self.addSubview(&bar);
                    bars.push(bar);
                }
                Item::Divider => {}
            }
        }
        {
            let mut state = self.ivars().borrow_mut();
            state.model = Some(model.clone());
            state.overview_mode = overview_mode;
            state.layout = Some(layout);
            state.labels = labels;
            state.bars = bars;
        }
        self.setNeedsDisplay(true);
        true
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

    /// Draws the card and its subviews into a bitmap, as on screen.
    pub fn render_bitmap(&self) -> Option<Retained<NSBitmapImageRep>> {
        let bounds = self.bounds();
        let rep = self.bitmapImageRepForCachingDisplayInRect(bounds)?;
        self.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        Some(rep)
    }

    fn draw(&self) {
        let state = self.ivars().borrow();
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
        ns_color(card_layout::divider_color(), accent).setFill();
        for placed in &layout.items {
            if placed.item == Item::Divider {
                NSBezierPath::fillRect(ns_rect(placed.frame));
            }
        }
    }
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
        field
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
            TextStyle::Headline => NSFontTextStyleHeadline,
            TextStyle::Subheadline => NSFontTextStyleSubheadline,
            TextStyle::Body => NSFontTextStyleBody,
            TextStyle::Footnote => NSFontTextStyleFootnote,
            TextStyle::Caption => NSFontTextStyleCaption1,
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
        }
    };
    NSFont::systemFontOfSize_weight(preferred.pointSize(), weight)
}
