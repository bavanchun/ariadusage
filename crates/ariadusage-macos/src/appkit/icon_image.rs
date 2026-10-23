// Ported from CodexBar Sources/CodexBar/IconRenderer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Renders an [`IconLayout`] into an 18 × 18 pt template `NSImage` drawn at 2×.

use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBezierPath, NSBitmapImageRep, NSColor, NSCompositingOperation, NSDeviceRGBColorSpace,
    NSGraphicsContext, NSImage,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::icon::{
    Bar, CANVAS_PX, ICON_SCALE, ICON_SIZE_PT, IconLayout, RectPt, RectPx, STROKE_WIDTH_PX,
    StatusOverlay,
};

fn px_rect(r: RectPx) -> NSRect {
    let pt = |v: i32| f64::from(v) / ICON_SCALE;
    NSRect::new(
        NSPoint::new(pt(r.x), pt(r.y)),
        NSSize::new(pt(r.w), pt(r.h)),
    )
}

fn pt_rect(r: RectPt) -> NSRect {
    NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.w, r.h))
}

fn rounded(rect: NSRect, radius: f64) -> Retained<NSBezierPath> {
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius)
}

fn label(alpha: f64) -> Retained<NSColor> {
    NSColor::labelColor().colorWithAlphaComponent(alpha)
}

/// Draws the icon and returns it as a template image, or `None` if AppKit
/// could not create the bitmap.
pub fn render(layout: &IconLayout) -> Option<Retained<NSImage>> {
    let size = NSSize::new(ICON_SIZE_PT, ICON_SIZE_PT);
    let pixels = CANVAS_PX as isize;
    // SAFETY: a null plane pointer asks AppKit to allocate the bitmap; the
    // remaining arguments describe a standard 8-bit RGBA layout.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            pixels,
            pixels,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    rep.setSize(size);
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    image.addRepresentation(&rep);

    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    context.setShouldAntialias(true);
    for bar in &layout.bars {
        draw_bar(bar, layout);
    }
    if let Some(overlay) = layout.overlay {
        draw_overlay(&context, overlay);
    }
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    image.setTemplate(true);
    Some(image)
}

fn draw_bar(bar: &Bar, layout: &IconLayout) {
    let palette = layout.palette;
    let radius = f64::from(bar.corner_radius_px) / ICON_SCALE;
    let track = rounded(px_rect(bar.rect), radius);
    label(palette.track_fill_alpha * bar.alpha).setFill();
    track.fill();

    let inset_px = STROKE_WIDTH_PX / 2;
    let stroke_radius = f64::from((bar.corner_radius_px - inset_px).max(0)) / ICON_SCALE;
    let outline = rounded(px_rect(bar.rect.stroke_rect()), stroke_radius);
    outline.setLineWidth(f64::from(STROKE_WIDTH_PX) / ICON_SCALE);
    label(palette.track_stroke_alpha * bar.alpha).setStroke();
    outline.stroke();

    let fill_px = bar.fill_width_px();
    if fill_px > 0 {
        // Clip to the capsule so the progress edge stays straight.
        NSGraphicsContext::saveGraphicsState_class();
        track.addClip();
        label(bar.alpha).setFill();
        let fill = RectPx::new(bar.rect.x, bar.rect.y, fill_px, bar.rect.h);
        NSBezierPath::bezierPathWithRect(px_rect(fill)).fill();
        NSGraphicsContext::restoreGraphicsState_class();
    }
}

fn draw_overlay(context: &NSGraphicsContext, overlay: StatusOverlay) {
    // Clear a halo first so the mark stays readable over the meters.
    let clear = |path: &NSBezierPath| {
        NSGraphicsContext::saveGraphicsState_class();
        context.setCompositingOperation(NSCompositingOperation::Clear);
        path.fill();
        NSGraphicsContext::restoreGraphicsState_class();
    };
    match overlay {
        StatusOverlay::Dot { dot, halo } => {
            clear(&NSBezierPath::bezierPathWithOvalInRect(pt_rect(halo)));
            label(1.0).setFill();
            NSBezierPath::bezierPathWithOvalInRect(pt_rect(dot)).fill();
        }
        StatusOverlay::Exclamation { line, dot, halo } => {
            clear(&rounded(pt_rect(halo), 2.0));
            label(1.0).setFill();
            rounded(pt_rect(line), 1.0).fill();
            NSBezierPath::bezierPathWithOvalInRect(pt_rect(dot)).fill();
        }
    }
}
