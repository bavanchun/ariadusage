// Ported from CodexBar Sources/CodexBar/CostHistoryChartMenuView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/QuotaBurndownChartMenuView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PlanUtilizationHistoryChartMenuView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/ProviderMenuChartContent.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! AppKit `NSView` hosts for the `Plan Usage` and `Cost` submenu charts:
//! - [`CostHistoryChartView`] (`CostHistoryChartMenuView`)
//! - [`QuotaBurndownChartView`] (`QuotaBurndownChartMenuView`)
//! - [`PlanUtilizationChartView`] (`PlanUtilizationHistoryChartMenuView`)
//!
//! All chart geometry is computed by pure-Rust functions in [`crate::chart`];
//! these views draw the computed geometry with `NSBezierPath` and `NSString`
//! and handle hover tracking and segment switching.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSBezierPath, NSBitmapImageRep, NSColor, NSEvent, NSFont, NSFontAttributeName,
    NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold, NSForegroundColorAttributeName,
    NSResponder, NSStringDrawing, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{
    NSDictionary, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use crate::card::Rgb;
use crate::chart::{
    ChartMetric, CostHistoryChartModel, PlanUtilizationChartModel, QuotaBurndownChartModel,
    SUBMENU_CHART_CANVAS_HEIGHT, SUBMENU_CHART_H_PADDING, SUBMENU_CHART_V_PADDING,
    SUBMENU_CHART_WIDTH,
};
use crate::geometry::{Point, Rect};

fn ns_rect(r: Rect) -> NSRect {
    NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
}

fn rgb_color(rgb: Rgb, alpha: f64) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(rgb.r) / 255.0,
        f64::from(rgb.g) / 255.0,
        f64::from(rgb.b) / 255.0,
        alpha,
    )
}

fn draw_text(text: &str, point: NSPoint, size: f64, weight: f64, color: &NSColor) {
    let font = NSFont::systemFontOfSize_weight(size, weight);
    // SAFETY: `NSFontAttributeName` and `NSForegroundColorAttributeName` are valid attribute keys.
    let attrs = unsafe {
        NSDictionary::from_slices(
            &[NSFontAttributeName, NSForegroundColorAttributeName],
            &[font.as_ref() as &AnyObject, color.as_ref() as &AnyObject],
        )
    };
    let ns_str = NSString::from_str(text);
    // SAFETY: `drawAtPoint:withAttributes:` is safe on the main thread inside `drawRect:`.
    unsafe { ns_str.drawAtPoint_withAttributes(point, Some(&attrs)) };
}

fn measure_text(text: &str, size: f64, weight: f64) -> NSSize {
    let font = NSFont::systemFontOfSize_weight(size, weight);
    // SAFETY: `NSFontAttributeName` is a valid attribute key.
    let attrs = unsafe { NSDictionary::from_slices(&[NSFontAttributeName], &[font.as_ref()]) };
    let ns_str = NSString::from_str(text);
    // SAFETY: `sizeWithAttributes:` is safe on the main thread.
    unsafe { ns_str.sizeWithAttributes(Some(&attrs)) }
}

/// Computes the compact width for a leading segmented picker (`Session | Weekly`, etc.).
fn compact_picker_width(titles: &[&str], max_width: f64) -> f64 {
    // SAFETY: `NSFontWeightSemibold` is an immutable AppKit global.
    let w_semi = unsafe { NSFontWeightSemibold };
    let total: f64 = titles
        .iter()
        .map(|t| (measure_text(t, 11.0, w_semi).width + 26.0).max(58.0))
        .sum();
    total.min(max_width)
}

/// Draws a horizontal segmented control in `rect` and returns the segment rects.
fn draw_segmented_picker(rect: Rect, titles: &[&str], selected: usize) -> Vec<Rect> {
    if titles.is_empty() {
        return Vec::new();
    }
    let track_color = NSColor::secondaryLabelColor().colorWithAlphaComponent(0.14);
    track_color.setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(rect), 6.0, 6.0).fill();

    let count = titles.len() as f64;
    let seg_w = rect.w / count;
    let mut seg_rects = Vec::with_capacity(titles.len());
    for (idx, title) in titles.iter().enumerate() {
        let seg_rect = Rect::new(rect.x + idx as f64 * seg_w, rect.y, seg_w, rect.h);
        seg_rects.push(seg_rect);
        if idx == selected {
            let pill = Rect::new(
                seg_rect.x + 1.5,
                seg_rect.y + 1.5,
                (seg_rect.w - 3.0).max(1.0),
                (seg_rect.h - 3.0).max(1.0),
            );
            NSColor::secondaryLabelColor()
                .colorWithAlphaComponent(0.28)
                .setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(ns_rect(pill), 5.0, 5.0).fill();
        }
        // SAFETY: weight constants are immutable AppKit globals.
        let weight = unsafe {
            if idx == selected {
                NSFontWeightSemibold
            } else {
                NSFontWeightMedium
            }
        };
        let color = if idx == selected {
            NSColor::labelColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        let sz = measure_text(title, 11.0, weight);
        let tx = seg_rect.x + (seg_rect.w - sz.width) * 0.5;
        let ty = seg_rect.y + (seg_rect.h - sz.height) * 0.5;
        draw_text(title, NSPoint::new(tx, ty), 11.0, weight, &color);
    }
    seg_rects
}

#[allow(deprecated)]
fn fill_menu_backdrop(bounds: NSRect, dark: bool) {
    // SAFETY: `NSAppearanceNameDarkAqua` and `NSAppearanceNameAqua` are valid AppKit globals.
    let name = unsafe {
        if dark {
            NSAppearanceNameDarkAqua
        } else {
            NSAppearanceNameAqua
        }
    };
    if let Some(app) = NSAppearance::appearanceNamed(name) {
        // SAFETY: `app` is a valid `NSAppearance` and we are on the main thread inside `drawRect:`.
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
    NSBezierPath::fillRect(bounds);
}

fn draw_solid_hline(x1: f64, x2: f64, y: f64) {
    let path = NSBezierPath::bezierPath();
    path.setLineWidth(1.0);
    path.moveToPoint(NSPoint::new(x1, y));
    path.lineToPoint(NSPoint::new(x2, y));
    path.stroke();
}

// ---------------------------------------------------------------------------
// 1. CostHistoryChartView (`CostHistoryChartMenuView`)
// ---------------------------------------------------------------------------

pub struct CostChartState {
    model: CostHistoryChartModel,
    metric: ChartMetric,
    hovered_index: Option<usize>,
    segment_rects: Vec<(Rect, ChartMetric)>,
    tracking_area: Option<Retained<NSTrackingArea>>,
    backdrop_dark: Option<bool>,
}

define_class!(
    // SAFETY: NSView subclass initialized with `initWithFrame:`; no Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsageCostHistoryChartView"]
    #[ivars = RefCell<CostChartState>]
    pub struct CostHistoryChartView;

    unsafe impl NSObjectProtocol for CostHistoryChartView {}

    impl CostHistoryChartView {
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
            let bounds = self.bounds();
            if point.x >= 0.0
                && point.y >= 0.0
                && point.x <= bounds.size.width
                && point.y <= bounds.size.height
            {
                std::ptr::from_ref::<Self>(self) as *mut NSView
            } else {
                std::ptr::null_mut()
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
            let mut changed = false;
            {
                let mut state = self.ivars().borrow_mut();
                let canvas_rect = Rect::new(
                    SUBMENU_CHART_H_PADDING,
                    SUBMENU_CHART_V_PADDING + 22.0 + 10.0,
                    SUBMENU_CHART_WIDTH - 2.0 * SUBMENU_CHART_H_PADDING,
                    SUBMENU_CHART_CANVAS_HEIGHT,
                );
                let geom = state.model.geometry(canvas_rect, state.metric, state.hovered_index);
                let next = geom.hovered_index(Point::new(loc.x, loc.y));
                if state.hovered_index != next {
                    state.hovered_index = next;
                    changed = true;
                }
            }
            if changed {
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            let mut state = self.ivars().borrow_mut();
            if state.hovered_index.is_some() {
                state.hovered_index = None;
                drop(state);
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let loc = self.convertPoint_fromView(event.locationInWindow(), None);
            let pt = Point::new(loc.x, loc.y);
            let mut changed = false;
            {
                let mut state = self.ivars().borrow_mut();
                for (rect, metric) in state.segment_rects.clone() {
                    if rect.contains(pt) && state.metric != metric {
                        state.metric = metric;
                        changed = true;
                        break;
                    }
                }
            }
            if changed {
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }
    }
);

impl CostHistoryChartView {
    pub fn new(mtm: MainThreadMarker, model: CostHistoryChartModel) -> Retained<Self> {
        let height = model.preferred_height();
        let metric = model.default_metric;
        let state = CostChartState {
            model,
            metric,
            hovered_index: None,
            segment_rects: Vec::new(),
            tracking_area: None,
            backdrop_dark: None,
        };
        let this = Self::alloc(mtm).set_ivars(RefCell::new(state));
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(SUBMENU_CHART_WIDTH, height),
        );
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.sync_tracking_area();
        view
    }

    /// Updates the chart model in place while preserving user-selected metric and hover state
    /// where valid. Returns `true` if the model changed.
    pub fn update(&self, model: CostHistoryChartModel) -> bool {
        let next_size = {
            let mut state = self.ivars().borrow_mut();
            if state.model == model {
                return false;
            }
            if !model.available_metrics.contains(&state.metric) {
                state.metric = model.default_metric;
            }
            if state
                .hovered_index
                .is_some_and(|idx| idx >= model.points.len())
            {
                state.hovered_index = None;
            }
            let h = model.preferred_height();
            state.model = model;
            NSSize::new(SUBMENU_CHART_WIDTH, h)
        };
        if self.frame().size != next_size {
            self.setFrameSize(next_size);
        }
        self.setNeedsDisplay(true);
        true
    }

    pub fn render_bitmap(&self) -> Option<Retained<NSBitmapImageRep>> {
        let bounds = self.bounds();
        let rep = self.bitmapImageRepForCachingDisplayInRect(bounds)?;
        self.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        Some(rep)
    }

    /// Renders the view into a bitmap under an explicit dark or light `NSAppearance`
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

    fn sync_tracking_area(&self) {
        let mut state = self.ivars().borrow_mut();
        if let Some(old) = state.tracking_area.take() {
            self.removeTrackingArea(&old);
        }
        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::ActiveAlways
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: `self` is a valid Objective-C tracking owner.
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

    fn draw(&self) {
        let mut state = self.ivars().borrow_mut();
        if let Some(dark) = state.backdrop_dark {
            fill_menu_backdrop(self.bounds(), dark);
        }
        let selected = state.hovered_index.or(state.model.default_selected_index);
        let inner_w = SUBMENU_CHART_WIDTH - 2.0 * SUBMENU_CHART_H_PADDING;
        // Top clear gutter (22 + 10 pt) above the chart (`CostHistoryChartMenuView.swift:139-142`).
        let canvas_rect = Rect::new(
            SUBMENU_CHART_H_PADDING,
            SUBMENU_CHART_V_PADDING + 22.0 + 10.0,
            inner_w,
            SUBMENU_CHART_CANVAS_HEIGHT,
        );
        let geom = state.model.geometry(canvas_rect, state.metric, selected);

        // SAFETY: font weight globals are immutable AppKit constants.
        let (w_reg, w_semi) = unsafe { (NSFontWeightRegular, NSFontWeightSemibold) };
        let sec_color = NSColor::secondaryLabelColor();
        let tert_color = NSColor::tertiaryLabelColor();
        let pri_color = NSColor::labelColor();

        // Leading Y-axis labels without grid lines (`CostHistoryChartMenuView.swift:177-191`).
        for (y, label) in &geom.y_ticks {
            let sz = measure_text(label, 10.0, w_reg);
            let lx = (geom.plot_rect.x - 6.0 - sz.width).max(canvas_rect.x);
            let ly = y - sz.height * 0.5;
            draw_text(label, NSPoint::new(lx, ly), 10.0, w_reg, &tert_color);
        }

        // Selection band
        if let Some(band) = geom.selection_band {
            sec_color.colorWithAlphaComponent(0.08).setFill();
            NSBezierPath::fillRect(ns_rect(band));
        }

        // Bars + opaque systemYellow peak caps (`CostHistoryChartMenuView.swift:173`).
        for bar in &geom.bars {
            let is_sel = selected == Some(bar.index);
            let alpha = if is_sel { 1.0 } else { 0.85 };
            rgb_color(state.model.bar_color, alpha).setFill();
            NSBezierPath::fillRect(ns_rect(bar.base_rect));
            if let Some(cap) = bar.peak_cap_rect {
                NSColor::systemYellowColor().setFill();
                NSBezierPath::fillRect(ns_rect(cap));
            }
        }

        // X-axis start/end labels
        for (idx, (cx, label)) in geom.x_labels.iter().enumerate() {
            let sz = measure_text(label, 10.0, w_reg);
            let lx = if idx == 0 {
                cx.max(geom.plot_rect.x)
            } else {
                (cx - sz.width).min(geom.plot_rect.max_x() - sz.width)
            };
            draw_text(
                label,
                NSPoint::new(lx, geom.plot_rect.max_y() + 3.0),
                10.0,
                w_reg,
                &tert_color,
            );
        }

        let mut y = canvas_rect.max_y() + 10.0;
        state.segment_rects.clear();
        if state.model.available_metrics.len() > 1 {
            let titles: Vec<&str> = state
                .model
                .available_metrics
                .iter()
                .map(|m| m.title())
                .collect();
            let sel_idx = state
                .model
                .available_metrics
                .iter()
                .position(|m| *m == state.metric)
                .unwrap_or(0);
            let picker_w = 120.0;
            let picker_x = SUBMENU_CHART_WIDTH - SUBMENU_CHART_H_PADDING - picker_w;
            let picker_rect = Rect::new(picker_x, y, picker_w, 22.0);
            let rects = draw_segmented_picker(picker_rect, &titles, sel_idx);
            let metrics = state.model.available_metrics.clone();
            for (r, m) in rects.into_iter().zip(metrics) {
                state.segment_rects.push((r, m));
            }
            y += 22.0 + 10.0;
        }

        // Selected day primary line
        let detail = state.model.detail_primary(selected);
        draw_text(
            &detail,
            NSPoint::new(SUBMENU_CHART_H_PADDING, y),
            11.0,
            w_reg,
            &sec_color,
        );
        y += 16.0;

        // Model breakdown rows
        if state.model.detail_viewport_row_count > 0 {
            y += 6.0;
            let row_h = if state.model.detail_viewport_row_count > 1 {
                50.0
            } else {
                36.0
            };
            let row_gap = 6.0;
            let viewport_h = state.model.detail_viewport_row_count as f64 * row_h
                + (state.model.detail_viewport_row_count.saturating_sub(1)) as f64 * row_gap;
            let rows = selected
                .and_then(|idx| state.model.points.get(idx))
                .map(|p| p.models.as_slice())
                .unwrap_or(&[]);
            for (row_idx, row) in rows
                .iter()
                .take(state.model.detail_viewport_row_count)
                .enumerate()
            {
                let ry = y + row_idx as f64 * (row_h + row_gap);
                let bar_rect = Rect::new(SUBMENU_CHART_H_PADDING, ry + 2.0, 2.0, 28.0);
                rgb_color(state.model.bar_color, row.accent_opacity).setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    ns_rect(bar_rect),
                    1.0,
                    1.0,
                )
                .fill();
                let tx = SUBMENU_CHART_H_PADDING + 8.0;
                draw_text(
                    &row.title,
                    NSPoint::new(tx, ry + 1.0),
                    11.0,
                    w_semi,
                    &pri_color,
                );
                if let Some(sub) = &row.subtitle {
                    draw_text(sub, NSPoint::new(tx, ry + 16.0), 10.0, w_reg, &sec_color);
                }
            }
            y += viewport_h;
        }

        if let Some(total) = &state.model.total_line {
            y += 10.0;
            draw_text(
                total,
                NSPoint::new(SUBMENU_CHART_H_PADDING, y),
                11.0,
                w_reg,
                &sec_color,
            );
            y += 16.0;
        }

        if let Some(disclaimer) = &state.model.disclaimer_line {
            y += 2.0;
            draw_text(
                disclaimer,
                NSPoint::new(SUBMENU_CHART_H_PADDING, y),
                10.0,
                w_reg,
                &tert_color,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. QuotaBurndownChartView (`QuotaBurndownChartMenuView`)
// ---------------------------------------------------------------------------

pub struct BurndownChartState {
    model: QuotaBurndownChartModel,
    series_index: usize,
    segment_rects: Vec<Rect>,
    backdrop_dark: Option<bool>,
}

define_class!(
    // SAFETY: NSView subclass initialized with `initWithFrame:`; no Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsageQuotaBurndownChartView"]
    #[ivars = RefCell<BurndownChartState>]
    pub struct QuotaBurndownChartView;

    unsafe impl NSObjectProtocol for QuotaBurndownChartView {}

    impl QuotaBurndownChartView {
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
            let bounds = self.bounds();
            if point.x >= 0.0
                && point.y >= 0.0
                && point.x <= bounds.size.width
                && point.y <= bounds.size.height
            {
                std::ptr::from_ref::<Self>(self) as *mut NSView
            } else {
                std::ptr::null_mut()
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let loc = self.convertPoint_fromView(event.locationInWindow(), None);
            let pt = Point::new(loc.x, loc.y);
            let mut next_height = None;
            {
                let mut state = self.ivars().borrow_mut();
                for (idx, rect) in state.segment_rects.clone().into_iter().enumerate() {
                    if rect.contains(pt) && state.series_index != idx {
                        state.series_index = idx;
                        next_height = Some(state.model.preferred_height(idx));
                        break;
                    }
                }
            }
            if let Some(h) = next_height {
                self.setFrameSize(NSSize::new(SUBMENU_CHART_WIDTH, h));
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }
    }
);

impl QuotaBurndownChartView {
    pub fn new(mtm: MainThreadMarker, model: QuotaBurndownChartModel) -> Retained<Self> {
        let series_index = model.default_series_index;
        let height = model.preferred_height(series_index);
        let state = BurndownChartState {
            model,
            series_index,
            segment_rects: Vec::new(),
            backdrop_dark: None,
        };
        let this = Self::alloc(mtm).set_ivars(RefCell::new(state));
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(SUBMENU_CHART_WIDTH, height),
        );
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    /// Updates the burndown chart model in place while preserving the selected series index
    /// where valid. Returns `true` if the model changed.
    pub fn update(&self, model: QuotaBurndownChartModel) -> bool {
        let next_size = {
            let mut state = self.ivars().borrow_mut();
            if state.model == model {
                return false;
            }
            if state.series_index >= model.series.len() {
                state.series_index = model.default_series_index;
            }
            let h = model.preferred_height(state.series_index);
            state.model = model;
            NSSize::new(SUBMENU_CHART_WIDTH, h)
        };
        if self.frame().size != next_size {
            self.setFrameSize(next_size);
        }
        self.setNeedsDisplay(true);
        true
    }

    pub fn render_bitmap(&self) -> Option<Retained<NSBitmapImageRep>> {
        let bounds = self.bounds();
        let rep = self.bitmapImageRepForCachingDisplayInRect(bounds)?;
        self.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        Some(rep)
    }

    /// Renders the view into a bitmap under an explicit dark or light `NSAppearance`
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

    fn draw(&self) {
        let mut state = self.ivars().borrow_mut();
        if let Some(dark) = state.backdrop_dark {
            fill_menu_backdrop(self.bounds(), dark);
        }
        let inner_w = SUBMENU_CHART_WIDTH - 2.0 * SUBMENU_CHART_H_PADDING;
        let mut y = SUBMENU_CHART_V_PADDING;

        state.segment_rects.clear();
        if state.model.series.len() > 1 {
            let titles: Vec<&str> = state
                .model
                .series
                .iter()
                .map(|s| s.title.as_str())
                .collect();
            let picker_w = compact_picker_width(&titles, inner_w);
            let picker_rect = Rect::new(SUBMENU_CHART_H_PADDING, y, picker_w, 24.0);
            state.segment_rects = draw_segmented_picker(picker_rect, &titles, state.series_index);
            y += 24.0 + 10.0;
        }

        let canvas_rect = Rect::new(
            SUBMENU_CHART_H_PADDING,
            y,
            inner_w,
            SUBMENU_CHART_CANVAS_HEIGHT,
        );
        let geom = state.model.geometry(canvas_rect, state.series_index);

        // SAFETY: font weight globals are immutable AppKit constants.
        let w_reg = unsafe { NSFontWeightRegular };
        let sec_color = NSColor::secondaryLabelColor();
        let tert_color = NSColor::tertiaryLabelColor();
        let grid_color = sec_color.colorWithAlphaComponent(0.18);

        for (ty, label) in geom.y_ticks {
            grid_color.setStroke();
            draw_solid_hline(geom.plot_rect.x, geom.plot_rect.max_x(), ty);
            let sz = measure_text(label, 10.0, w_reg);
            let lx = canvas_rect.max_x() - sz.width;
            let ly = ty - sz.height * 0.5;
            draw_text(label, NSPoint::new(lx, ly), 10.0, w_reg, &tert_color);
        }

        // Ideal diagonal dashed line
        sec_color.colorWithAlphaComponent(0.45).setStroke();
        let ideal = NSBezierPath::bezierPath();
        ideal.setLineWidth(1.25);
        let dash = [4.0f64, 3.0f64];
        // SAFETY: `dash` has 2 elements and outlives `setLineDash:count:phase:`.
        unsafe { ideal.setLineDash_count_phase(dash.as_ptr(), dash.len() as isize, 0.0) };
        ideal.moveToPoint(NSPoint::new(geom.ideal_start.x, geom.ideal_start.y));
        ideal.lineToPoint(NSPoint::new(geom.ideal_end.x, geom.ideal_end.y));
        ideal.stroke();

        // Observed line + endpoint dot (`QuotaBurndownChartMenuView.swift` has no area fill).
        if geom.observed_points.len() >= 2 {
            let first = geom.observed_points[0];
            let line = NSBezierPath::bezierPath();
            line.setLineWidth(2.0);
            line.moveToPoint(NSPoint::new(first.x, first.y));
            for pt in &geom.observed_points[1..] {
                line.lineToPoint(NSPoint::new(pt.x, pt.y));
            }
            rgb_color(state.model.accent_color, 0.95).setStroke();
            line.stroke();
        }
        if let Some(end) = geom.endpoint {
            let r = 3.5;
            let dot = NSRect::new(
                NSPoint::new(end.x - r, end.y - r),
                NSSize::new(r * 2.0, r * 2.0),
            );
            rgb_color(state.model.accent_color, 1.0).setFill();
            NSBezierPath::bezierPathWithOvalInRect(dot).fill();
        }

        y = canvas_rect.max_y() + 10.0;
        if let Some(series) = state.model.series.get(state.series_index) {
            draw_text(
                &series.start_label,
                NSPoint::new(geom.plot_rect.x, y),
                10.0,
                w_reg,
                &sec_color,
            );
            let r_sz = measure_text(&series.reset_label, 10.0, w_reg);
            draw_text(
                &series.reset_label,
                NSPoint::new((canvas_rect.max_x() - r_sz.width).max(geom.plot_rect.x), y),
                10.0,
                w_reg,
                &sec_color,
            );
            y += 15.0 + 10.0;
            draw_text(
                &series.remaining_line,
                NSPoint::new(SUBMENU_CHART_H_PADDING, y),
                11.0,
                w_reg,
                &sec_color,
            );
            y += 15.0;
            if let Some(note) = &series.last_known_note {
                y += 10.0;
                draw_text(
                    note,
                    NSPoint::new(SUBMENU_CHART_H_PADDING, y),
                    10.0,
                    w_reg,
                    &tert_color,
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 3. PlanUtilizationChartView (`PlanUtilizationHistoryChartMenuView`)
// ---------------------------------------------------------------------------

pub struct UtilizationChartState {
    model: PlanUtilizationChartModel,
    series_index: usize,
    hovered_bar_index: Option<usize>,
    segment_rects: Vec<Rect>,
    tracking_area: Option<Retained<NSTrackingArea>>,
    backdrop_dark: Option<bool>,
}

define_class!(
    // SAFETY: NSView subclass initialized with `initWithFrame:`; no Drop impl.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AriadUsagePlanUtilizationChartView"]
    #[ivars = RefCell<UtilizationChartState>]
    pub struct PlanUtilizationChartView;

    unsafe impl NSObjectProtocol for PlanUtilizationChartView {}

    impl PlanUtilizationChartView {
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
            let bounds = self.bounds();
            if point.x >= 0.0
                && point.y >= 0.0
                && point.x <= bounds.size.width
                && point.y <= bounds.size.height
            {
                std::ptr::from_ref::<Self>(self) as *mut NSView
            } else {
                std::ptr::null_mut()
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
            let mut changed = false;
            {
                let mut state = self.ivars().borrow_mut();
                let top_offset = if state.model.series.len() > 1 {
                    SUBMENU_CHART_V_PADDING + 24.0 + 10.0
                } else {
                    SUBMENU_CHART_V_PADDING
                };
                let canvas_rect = Rect::new(
                    SUBMENU_CHART_H_PADDING,
                    top_offset,
                    SUBMENU_CHART_WIDTH - 2.0 * SUBMENU_CHART_H_PADDING,
                    SUBMENU_CHART_CANVAS_HEIGHT,
                );
                let geom = state.model.geometry(
                    canvas_rect,
                    state.series_index,
                    state.hovered_bar_index,
                );
                let next = geom.hovered_index(Point::new(loc.x, loc.y));
                if state.hovered_bar_index != next {
                    state.hovered_bar_index = next;
                    changed = true;
                }
            }
            if changed {
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            let mut state = self.ivars().borrow_mut();
            if state.hovered_bar_index.is_some() {
                state.hovered_bar_index = None;
                drop(state);
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let loc = self.convertPoint_fromView(event.locationInWindow(), None);
            let pt = Point::new(loc.x, loc.y);
            let mut changed = false;
            {
                let mut state = self.ivars().borrow_mut();
                for (idx, rect) in state.segment_rects.clone().into_iter().enumerate() {
                    if rect.contains(pt) && state.series_index != idx {
                        state.series_index = idx;
                        state.hovered_bar_index = None;
                        changed = true;
                        break;
                    }
                }
            }
            if changed {
                self.setNeedsDisplay(true);
            }
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }
    }
);

impl PlanUtilizationChartView {
    pub fn new(mtm: MainThreadMarker, model: PlanUtilizationChartModel) -> Retained<Self> {
        let height = model.preferred_height();
        let series_index = model.default_series_index;
        let state = UtilizationChartState {
            model,
            series_index,
            hovered_bar_index: None,
            segment_rects: Vec::new(),
            tracking_area: None,
            backdrop_dark: None,
        };
        let this = Self::alloc(mtm).set_ivars(RefCell::new(state));
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(SUBMENU_CHART_WIDTH, height),
        );
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.sync_tracking_area();
        view
    }

    /// Updates the utilization chart model in place while preserving user-selected series
    /// and hover state where valid. Returns `true` if the model changed.
    pub fn update(&self, model: PlanUtilizationChartModel) -> bool {
        let next_size = {
            let mut state = self.ivars().borrow_mut();
            if state.model == model {
                return false;
            }
            if state.series_index >= model.series.len() {
                state.series_index = model.default_series_index;
            }
            let bar_len = model
                .series
                .get(state.series_index)
                .map_or(0, |s| s.bars.len());
            if state.hovered_bar_index.is_some_and(|idx| idx >= bar_len) {
                state.hovered_bar_index = None;
            }
            let h = model.preferred_height();
            state.model = model;
            NSSize::new(SUBMENU_CHART_WIDTH, h)
        };
        if self.frame().size != next_size {
            self.setFrameSize(next_size);
        }
        self.setNeedsDisplay(true);
        true
    }

    pub fn render_bitmap(&self) -> Option<Retained<NSBitmapImageRep>> {
        let bounds = self.bounds();
        let rep = self.bitmapImageRepForCachingDisplayInRect(bounds)?;
        self.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        Some(rep)
    }

    /// Renders the view into a bitmap under an explicit dark or light `NSAppearance`
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

    fn sync_tracking_area(&self) {
        let mut state = self.ivars().borrow_mut();
        if let Some(old) = state.tracking_area.take() {
            self.removeTrackingArea(&old);
        }
        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::ActiveAlways
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: `self` is a valid Objective-C tracking owner.
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

    fn draw(&self) {
        let mut state = self.ivars().borrow_mut();
        if let Some(dark) = state.backdrop_dark {
            fill_menu_backdrop(self.bounds(), dark);
        }
        let inner_w = SUBMENU_CHART_WIDTH - 2.0 * SUBMENU_CHART_H_PADDING;
        let mut y = SUBMENU_CHART_V_PADDING;

        state.segment_rects.clear();
        if state.model.series.len() > 1 {
            let titles: Vec<&str> = state
                .model
                .series
                .iter()
                .map(|s| s.title.as_str())
                .collect();
            let picker_w = compact_picker_width(&titles, inner_w);
            let picker_rect = Rect::new(SUBMENU_CHART_H_PADDING, y, picker_w, 24.0);
            state.segment_rects = draw_segmented_picker(picker_rect, &titles, state.series_index);
            y += 24.0 + 10.0;
        }

        let canvas_rect = Rect::new(
            SUBMENU_CHART_H_PADDING,
            y,
            inner_w,
            SUBMENU_CHART_CANVAS_HEIGHT,
        );
        let geom = state
            .model
            .geometry(canvas_rect, state.series_index, state.hovered_bar_index);

        // SAFETY: font weight globals are immutable AppKit constants.
        let w_reg = unsafe { NSFontWeightRegular };
        let sec_color = NSColor::secondaryLabelColor();
        let tert_color = NSColor::tertiaryLabelColor();

        // Note: `PlanUtilizationHistoryChartMenuView.swift:128` sets `.chartYAxis(.hidden)`.
        // Capacity track + fill bars
        let track_color = sec_color.colorWithAlphaComponent(0.16);
        for bar in &geom.bars {
            track_color.setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                ns_rect(bar.track_rect),
                2.0,
                2.0,
            )
            .fill();
            if bar.fill_rect.h > 0.0 {
                let alpha = if state.hovered_bar_index == Some(bar.index) {
                    1.0
                } else {
                    0.85
                };
                rgb_color(state.model.accent_color, alpha).setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    ns_rect(bar.fill_rect),
                    2.0,
                    2.0,
                )
                .fill();
            }
        }

        if let Some(rx) = geom.hover_rule_x {
            sec_color.colorWithAlphaComponent(0.45).setStroke();
            let rule = NSBezierPath::bezierPath();
            rule.setLineWidth(1.0);
            let dash = [2.0f64, 2.0f64];
            // SAFETY: `dash` has 2 elements and outlives `setLineDash:count:phase:`.
            unsafe { rule.setLineDash_count_phase(dash.as_ptr(), dash.len() as isize, 0.0) };
            rule.moveToPoint(NSPoint::new(rx, geom.plot_rect.y));
            rule.lineToPoint(NSPoint::new(rx, geom.plot_rect.max_y()));
            rule.stroke();
        }

        let mut last_label_max_x: Option<f64> = None;
        for (cx, label) in &geom.x_labels {
            let sz = measure_text(label, 10.0, w_reg);
            let lx = (cx - sz.width * 0.5).clamp(
                geom.plot_rect.x,
                (geom.plot_rect.max_x() - sz.width).max(geom.plot_rect.x),
            );
            if last_label_max_x.is_some_and(|prev_max| lx < prev_max + 4.0) {
                continue;
            }
            draw_text(
                label,
                NSPoint::new(lx, geom.plot_rect.max_y() + 3.0),
                10.0,
                w_reg,
                &tert_color,
            );
            last_label_max_x = Some(lx + sz.width);
        }

        y = canvas_rect.max_y() + 10.0;
        let selected_idx = state.hovered_bar_index.or_else(|| {
            state
                .model
                .series
                .get(state.series_index)
                .and_then(|s| s.bars.len().checked_sub(1))
        });
        let detail = state.model.detail_text(state.series_index, selected_idx);
        draw_text(
            &detail,
            NSPoint::new(SUBMENU_CHART_H_PADDING, y),
            11.0,
            w_reg,
            &sec_color,
        );
    }
}
