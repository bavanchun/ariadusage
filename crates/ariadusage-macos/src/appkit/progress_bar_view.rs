// Ported from CodexBar Sources/CodexBar/UsageProgressBar.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! The usage bar view: draws [`progress_bar::shapes`] in `drawRect:`.
//!
//! The view is layer-backed, so the `destinationOut` punches clear only the
//! bar's own pixels, like the single SwiftUI `Canvas` they replace.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSBezierPath, NSCompositingOperation, NSGraphicsContext, NSResponder, NSView,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

use super::color::ns_color;
use crate::card::Accent;
use crate::geometry::{Rect, Size};
use crate::progress_bar::{self, BarSpec};

/// What the bar draws.
pub struct BarState {
    spec: BarSpec,
    accent: Accent,
    highlighted: bool,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements beyond calling its
    // designated initializer, which `new` does; this class does not
    // implement Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsageProgressBarView"]
    #[ivars = RefCell<BarState>]
    pub struct ProgressBarView;

    unsafe impl NSObjectProtocol for ProgressBarView {}

    impl ProgressBarView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
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

impl ProgressBarView {
    pub fn new(
        mtm: MainThreadMarker,
        frame: Rect,
        spec: BarSpec,
        accent: Accent,
        accessibility_label: &str,
    ) -> Retained<Self> {
        let accessibility_value = progress_bar::accessibility_value(&spec);
        let this = Self::alloc(mtm).set_ivars(RefCell::new(BarState {
            spec,
            accent,
            highlighted: false,
        }));
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: ns_rect(frame)] };
        view.setWantsLayer(true);
        view.setAccessibilityElement(true);
        view.setAccessibilityLabel(Some(&NSString::from_str(accessibility_label)));
        let value = NSString::from_str(&accessibility_value);
        // SAFETY: the accessibility value of a progress element is a string.
        unsafe { view.setAccessibilityValue(Some(&value)) };
        view
    }

    pub fn set_highlighted(&self, highlighted: bool) {
        let changed = {
            let mut state = self.ivars().borrow_mut();
            let changed = state.highlighted != highlighted;
            state.highlighted = highlighted;
            changed
        };
        if changed {
            self.setNeedsDisplay(true);
        }
    }

    fn draw(&self) {
        let bounds = self.bounds();
        let scale = self.window().map_or(2.0, |w| w.backingScaleFactor());
        let state = self.ivars().borrow();
        let shapes = progress_bar::shapes(
            &state.spec,
            Size::new(bounds.size.width, bounds.size.height),
            scale,
        );
        let Some(context) = NSGraphicsContext::currentContext() else {
            return;
        };
        NSGraphicsContext::saveGraphicsState_class();
        NSBezierPath::clipRect(bounds);
        for shape in shapes {
            ns_color(shape.paint.color(state.highlighted), state.accent).setFill();
            let rect = ns_rect(shape.rect);
            let path = if shape.corner_radius > 0.0 {
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    rect,
                    shape.corner_radius,
                    shape.corner_radius,
                )
            } else {
                NSBezierPath::bezierPathWithRect(rect)
            };
            if shape.paint.is_punch() {
                context.setCompositingOperation(NSCompositingOperation::DestinationOut);
                path.fill();
                context.setCompositingOperation(NSCompositingOperation::SourceOver);
            } else {
                path.fill();
            }
        }
        NSGraphicsContext::restoreGraphicsState_class();
    }
}
