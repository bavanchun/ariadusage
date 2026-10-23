// Ported from CodexBar Sources/CodexBar/StatusItemController+SwitcherViews.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/ProviderSwitcherButtons.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! AppKit view for the top provider switcher row (`ProviderSwitcherView`).
//!
//! Geometry, button frames, quota bars and hit testing come from [`crate::switcher`].
//! Each segment also hosts an unbordered toggle `NSButton` subview for VoiceOver
//! and Accessibility (`AXButton` / `AXToggle`), while mouse clicks inside the
//! open `NSMenu` are handled directly by [`SwitcherView`] (`hitTest:` +
//! `mouseDown:` / `mouseUp:`) so `NSMenu` tracking never drops a tab switch.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAccessibility, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSBezierPath, NSBitmapImageRep, NSButton, NSButtonType, NSColor, NSControlStateValueOff,
    NSControlStateValueOn, NSEvent, NSFont, NSLineBreakMode, NSResponder, NSTextAlignment,
    NSTextField, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindingRule,
};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

use super::color::ns_color;
use crate::geometry::{Point, Rect, Size};
use crate::highlight_style::{Color, ColorRole};
use crate::switcher::{
    self, PathCmd, SwitcherIcon, SwitcherLayout, SwitcherSegment, SwitcherSegmentLayout,
    SwitcherSelection, SwitcherTextMeasure,
};

/// Labels draw their text 2 pt inside their frame on each side.
const LABEL_INSET: f64 = 2.0;
const UNBOUNDED: f64 = 100_000.0;

type SelectCallback = Rc<dyn Fn(SwitcherSelection)>;

#[derive(Default)]
pub struct SwitcherState {
    segments: Vec<SwitcherSegment>,
    selected: Option<SwitcherSelection>,
    hovered_index: Option<usize>,
    pressed_index: Option<usize>,
    layout: Option<SwitcherLayout>,
    buttons: Vec<Retained<NSButton>>,
    labels: Vec<(Retained<NSTextField>, usize)>,
    tracking_area: Option<Retained<NSTrackingArea>>,
    on_select: Option<SelectCallback>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond calling its
    // designated initializer, which `new` does; this class does not
    // implement Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsageSwitcherView"]
    #[ivars = RefCell<SwitcherState>]
    pub struct SwitcherView;

    unsafe impl NSObjectProtocol for SwitcherView {}

    impl SwitcherView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            // SAFETY: `super(self)` is a valid NSView instance on the main thread.
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            if hit.is_some() {
                // Swallow hits on child NSButtons during NSMenu tracking so mouseDown/mouseUp
                // on SwitcherView drives selection reliably (`ProviderSwitcherView.hitTest`).
                std::ptr::from_ref::<Self>(self) as *mut NSView
            } else {
                std::ptr::null_mut()
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let pt = self.event_point(event);
            let mut state = self.ivars().borrow_mut();
            state.pressed_index = state
                .layout
                .as_ref()
                .and_then(|l| switcher::hit_test(l, pt))
                .map(|s| s.index);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let pt = self.event_point(event);
            let pressed = {
                let mut state = self.ivars().borrow_mut();
                let pressed = state.pressed_index.take();
                let released = state
                    .layout
                    .as_ref()
                    .and_then(|l| switcher::hit_test(l, pt))
                    .map(|s| s.index);
                pressed.filter(|&p| Some(p) == released)
            };
            if let Some(index) = pressed {
                self.select_index(index);
            }
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let pt = self.event_point(event);
            let next_hover = {
                let state = self.ivars().borrow();
                state
                    .layout
                    .as_ref()
                    .and_then(|l| switcher::hit_test(l, pt))
                    .map(|s| s.index)
            };
            self.set_hovered_index(next_hover);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.set_hovered_index(None);
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            // SAFETY: `super(self)` is a valid NSView instance on the main thread.
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
            let mut state = self.ivars().borrow_mut();
            if let Some(old) = state.tracking_area.take() {
                self.removeTrackingArea(&old);
            }
            let options = NSTrackingAreaOptions::ActiveAlways
                | NSTrackingAreaOptions::InVisibleRect
                | NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::MouseMoved;
            // SAFETY: `self` is the owner of the tracking area and removes it on update.
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    NSRect::ZERO,
                    options,
                    Some(self),
                    None,
                )
            };
            self.addTrackingArea(&area);
            state.tracking_area = Some(area);
        }

        #[unsafe(method(viewDidMoveToWindow))]
        fn view_did_move_to_window(&self) {
            // SAFETY: `super(self)` is a valid NSView instance on the main thread.
            let _: () = unsafe { msg_send![super(self), viewDidMoveToWindow] };
            if let Some(window) = self.window() {
                window.setAcceptsMouseMovedEvents(true);
            } else {
                self.set_hovered_index(None);
            }
        }

        #[unsafe(method(buttonPressed:))]
        fn button_pressed(&self, sender: Option<&AnyObject>) {
            let Some(sender) = sender else { return };
            // SAFETY: `sender` is one of the child `NSButton` instances created in `rebuild_subviews`.
            let tag: isize = unsafe { msg_send![sender, tag] };
            if tag >= 0 {
                self.select_index(tag as usize);
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

impl SwitcherView {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RefCell::new(SwitcherState::default()));
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(
                crate::card_layout::MIN_CARD_WIDTH,
                switcher::STACKED_ROW_HEIGHT,
            ),
        );
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    pub fn set_on_select(&self, callback: impl Fn(SwitcherSelection) + 'static) {
        self.ivars().borrow_mut().on_select = Some(Rc::new(callback));
    }

    /// Updates the switcher segments, selected tab and width. Returns `true` when the
    /// frame size or content changed.
    pub fn update(
        &self,
        segments: &[SwitcherSegment],
        selected: &SwitcherSelection,
        width: f64,
    ) -> bool {
        {
            let state = self.ivars().borrow();
            let same_width = state.layout.as_ref().is_some_and(|l| l.size.w == width);
            if same_width && state.segments == segments && state.selected.as_ref() == Some(selected)
            {
                return false;
            }
        }

        let mtm = self.mtm();
        let measure = AppKitSwitcherMeasure { mtm };
        let hovered = self.ivars().borrow().hovered_index;
        let layout = switcher::layout(segments, selected, hovered, width, &measure);
        self.setFrameSize(NSSize::new(layout.size.w, layout.size.h));
        self.rebuild_subviews(mtm, &layout);

        {
            let mut state = self.ivars().borrow_mut();
            state.segments = segments.to_vec();
            state.selected = Some(selected.clone());
            state.layout = Some(layout);
        }
        self.setNeedsDisplay(true);
        true
    }

    /// Current layout, if `update` has been called.
    pub fn layout(&self) -> Option<SwitcherLayout> {
        self.ivars().borrow().layout.clone()
    }

    /// Simulates clicking at `point` in view coordinates (for tests and direct activation).
    pub fn click_at(&self, point: Point) -> Option<SwitcherSelection> {
        let index = {
            let state = self.ivars().borrow();
            state
                .layout
                .as_ref()
                .and_then(|l| switcher::hit_test(l, point))
                .map(|s| s.index)
        }?;
        self.select_index(index)
    }

    /// Renders the switcher view and its subviews into a bitmap.
    pub fn render_bitmap(&self) -> Option<Retained<NSBitmapImageRep>> {
        let bounds = self.bounds();
        let rep = self.bitmapImageRepForCachingDisplayInRect(bounds)?;
        self.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        Some(rep)
    }

    fn select_index(&self, index: usize) -> Option<SwitcherSelection> {
        let (selection, changed) = {
            let state = self.ivars().borrow();
            let seg = state.segments.get(index)?;
            let changed = state.selected.as_ref() != Some(&seg.selection);
            (seg.selection.clone(), changed)
        };
        let (segments, width) = {
            let state = self.ivars().borrow();
            let width = state
                .layout
                .as_ref()
                .map_or(self.bounds().size.width, |l| l.size.w);
            (state.segments.clone(), width)
        };
        self.update(&segments, &selection, width);
        if changed {
            let cb = self.ivars().borrow().on_select.clone();
            if let Some(cb) = cb {
                cb(selection.clone());
            }
        }
        Some(selection)
    }

    fn set_hovered_index(&self, hovered_index: Option<usize>) {
        let changed = {
            let mut state = self.ivars().borrow_mut();
            if state.hovered_index == hovered_index {
                false
            } else {
                state.hovered_index = hovered_index;
                if let Some(layout) = &mut state.layout {
                    for seg in &mut layout.segments {
                        seg.hovered = Some(seg.index) == hovered_index;
                    }
                }
                true
            }
        };
        if changed {
            self.setNeedsDisplay(true);
        }
    }

    fn event_point(&self, event: &NSEvent) -> Point {
        let loc_in_window = event.locationInWindow();
        let pt = self.convertPoint_fromView(loc_in_window, None);
        Point::new(pt.x, pt.y)
    }

    fn rebuild_subviews(&self, mtm: MainThreadMarker, layout: &SwitcherLayout) {
        for view in self.subviews().iter() {
            view.removeFromSuperview();
        }
        let mut buttons = Vec::with_capacity(layout.segments.len());
        let mut labels = Vec::with_capacity(layout.segments.len());

        for seg in &layout.segments {
            let button = make_accessibility_toggle(mtm, self, seg);
            self.addSubview(&button);
            buttons.push(button);

            let label = make_title_label(mtm, seg, layout.stacked_icons);
            self.addSubview(&label);
            labels.push((label, seg.index));
        }

        let mut state = self.ivars().borrow_mut();
        state.buttons = buttons;
        state.labels = labels;
    }

    fn draw(&self) {
        let state = self.ivars().borrow();
        let Some(layout) = &state.layout else { return };

        for seg in &layout.segments {
            if seg.selected {
                NSColor::controlAccentColor().setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    ns_rect(seg.frame),
                    switcher::BUTTON_CORNER_RADIUS,
                    switcher::BUTTON_CORNER_RADIUS,
                )
                .fill();
            } else if seg.hovered {
                self.hover_plate_color().setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    ns_rect(seg.frame),
                    switcher::BUTTON_CORNER_RADIUS,
                    switcher::BUTTON_CORNER_RADIUS,
                )
                .fill();
            }

            let fg = if seg.selected {
                NSColor::whiteColor()
            } else {
                NSColor::secondaryLabelColor()
            };
            draw_segment_icon(seg.icon, seg.icon_frame, &fg);

            if let Some(bar) = &seg.quota_bar {
                NSColor::tertiaryLabelColor()
                    .colorWithAlphaComponent(switcher::QUOTA_TRACK_ALPHA)
                    .setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    ns_rect(bar.track),
                    switcher::QUOTA_BAR_CORNER_RADIUS,
                    switcher::QUOTA_BAR_CORNER_RADIUS,
                )
                .fill();

                if let Some(fill_rect) = bar.fill {
                    ns_color(Color::opaque(ColorRole::ProgressTint), seg.accent).setFill();
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                        ns_rect(fill_rect),
                        switcher::QUOTA_BAR_CORNER_RADIUS,
                        switcher::QUOTA_BAR_CORNER_RADIUS,
                    )
                    .fill();
                }
            }
        }
    }

    fn hover_plate_color(&self) -> Retained<NSColor> {
        if self.is_light_mode() {
            NSColor::blackColor().colorWithAlphaComponent(0.095)
        } else {
            NSColor::labelColor().colorWithAlphaComponent(0.06)
        }
    }

    fn is_light_mode(&self) -> bool {
        // SAFETY: `NSAppearanceNameAqua` and `NSAppearanceNameDarkAqua` are valid
        // immutable AppKit appearance name globals.
        unsafe {
            let names = NSArray::from_slice(&[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]);
            let matched = self
                .effectiveAppearance()
                .bestMatchFromAppearancesWithNames(&names);
            matched.is_some_and(|name| *name == *NSAppearanceNameAqua)
        }
    }
}

fn make_accessibility_toggle(
    mtm: MainThreadMarker,
    target: &SwitcherView,
    seg: &SwitcherSegmentLayout,
) -> Retained<NSButton> {
    let button = NSButton::initWithFrame(NSButton::alloc(mtm), ns_rect(seg.frame));
    button.setBordered(false);
    button.setButtonType(NSButtonType::Toggle);
    button.setTitle(&NSString::from_str(""));
    button.setAlternateTitle(&NSString::from_str(""));
    button.setTag(seg.index as isize);
    button.setState(if seg.selected {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    button.setAccessibilityLabel(Some(&NSString::from_str(&seg.title)));
    // SAFETY: `target` is the enclosing `SwitcherView`, which owns `button` as a subview.
    unsafe {
        button.setTarget(Some(target));
        button.setAction(Some(sel!(buttonPressed:)));
    }
    button
}

fn make_title_label(
    mtm: MainThreadMarker,
    seg: &SwitcherSegmentLayout,
    stacked: bool,
) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(&seg.title), mtm);
    let font = NSFont::systemFontOfSize(seg.font_size);
    field.setFont(Some(&font));
    let color = if seg.selected {
        NSColor::whiteColor()
    } else {
        NSColor::secondaryLabelColor()
    };
    field.setTextColor(Some(&color));
    field.setSelectable(false);
    field.setAlignment(if stacked {
        NSTextAlignment::Center
    } else {
        NSTextAlignment::Left
    });
    field.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    field.setMaximumNumberOfLines(1);
    field.setFrame(ns_rect(Rect::new(
        seg.title_frame.x - LABEL_INSET,
        seg.title_frame.y,
        seg.title_frame.w + 2.0 * LABEL_INSET,
        seg.title_frame.h,
    )));
    field
}

fn draw_segment_icon(icon: SwitcherIcon, frame: Rect, color: &NSColor) {
    match icon {
        SwitcherIcon::OverviewGrid => {
            color.setStroke();
            let cells = [
                Rect::new(frame.x + 2.0, frame.y + 2.0, 5.0, 5.0),
                Rect::new(frame.x + 9.0, frame.y + 2.0, 5.0, 5.0),
                Rect::new(frame.x + 2.0, frame.y + 9.0, 5.0, 5.0),
                Rect::new(frame.x + 9.0, frame.y + 9.0, 5.0, 5.0),
            ];
            for cell in cells {
                let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    ns_rect(cell),
                    1.25,
                    1.25,
                );
                path.setLineWidth(1.35);
                path.stroke();
            }
        }
        SwitcherIcon::Fallback => {
            color.setStroke();
            let rect = Rect::new(frame.x + 2.0, frame.y + 2.0, 12.0, 12.0);
            let path =
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(rect), 3.0, 3.0);
            path.setLineWidth(1.5);
            path.stroke();
        }
        SwitcherIcon::Claude | SwitcherIcon::Codex | SwitcherIcon::Antigravity => {
            let cmds = switcher::icon_path(icon, frame.w);
            if cmds.is_empty() {
                return;
            }
            let path = NSBezierPath::bezierPath();
            path.setWindingRule(NSWindingRule::EvenOdd);
            for cmd in cmds {
                match cmd {
                    PathCmd::MoveTo(p) => {
                        path.moveToPoint(NSPoint::new(frame.x + p.x, frame.y + p.y));
                    }
                    PathCmd::LineTo(p) => {
                        path.lineToPoint(NSPoint::new(frame.x + p.x, frame.y + p.y));
                    }
                    PathCmd::CubicTo(c1, c2, p) => {
                        path.curveToPoint_controlPoint1_controlPoint2(
                            NSPoint::new(frame.x + p.x, frame.y + p.y),
                            NSPoint::new(frame.x + c1.x, frame.y + c1.y),
                            NSPoint::new(frame.x + c2.x, frame.y + c2.y),
                        );
                    }
                    PathCmd::Close => {
                        path.closePath();
                    }
                }
            }
            color.setFill();
            path.fill();
        }
    }
}

struct AppKitSwitcherMeasure {
    mtm: MainThreadMarker,
}

impl SwitcherTextMeasure for AppKitSwitcherMeasure {
    fn measure_title(&self, text: &str, font_size: f64) -> Size {
        let field = NSTextField::labelWithString(&NSString::from_str(text), self.mtm);
        let font = NSFont::systemFontOfSize(font_size);
        field.setFont(Some(&font));
        field.setMaximumNumberOfLines(1);
        let Some(cell) = field.cell() else {
            return Size::new(text.len() as f64 * font_size * 0.55, font_size + 3.0);
        };
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(UNBOUNDED, UNBOUNDED));
        let size = cell.cellSizeForBounds(bounds);
        Size::new(
            (size.width - 2.0 * LABEL_INSET).max(0.0).ceil(),
            size.height.ceil(),
        )
    }
}
