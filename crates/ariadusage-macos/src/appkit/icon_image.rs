// Ported from CodexBar Sources/CodexBar/IconRenderer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuBarLayoutRenderer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Renders an [`IconLayout`] or [`MorphLayout`] into an 18 × 18 pt `NSImage`
//! drawn at 2× (`36 × 36` pixels), plus quota-warning flash overlays, provider
//! tinting, and appearance-specific PNG export for visual verification.

use objc2::rc::Retained;
use objc2::{AllocAnyThread, Message};
use objc2_app_kit::{
    NSAffineTransformNSAppKitAdditions, NSBezierPath, NSBitmapImageFileType, NSBitmapImageRep,
    NSColor, NSCompositingOperation, NSDeviceRGBColorSpace, NSGraphicsContext, NSImage,
};
use objc2_foundation::{NSAffineTransform, NSDictionary, NSPoint, NSRect, NSSize};

use crate::icon::{
    Bar, CANVAS_PX, CritterDecorations, FaceDecoration, FactoryDecoration, GeminiDecoration,
    GrokDecoration, ICON_SCALE, ICON_SIZE_PT, IconLayout, MorphLayout, NotchesDecoration, PointPt,
    RectPt, RectPx, STROKE_WIDTH_PX, StatusOverlay, TiltedEllipse, WarpDecoration,
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

fn ns_point(p: PointPt) -> NSPoint {
    NSPoint::new(p.x, p.y)
}

fn rounded(rect: NSRect, radius: f64) -> Retained<NSBezierPath> {
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius)
}

fn polygon_path(points: &[PointPt]) -> Retained<NSBezierPath> {
    let path = NSBezierPath::bezierPath();
    if let Some((first, rest)) = points.split_first() {
        path.moveToPoint(ns_point(*first));
        for pt in rest {
            path.lineToPoint(ns_point(*pt));
        }
        path.closePath();
    }
    path
}

fn label(alpha: f64) -> Retained<NSColor> {
    NSColor::labelColor().colorWithAlphaComponent(alpha)
}

fn make_bitmap_rep(size: NSSize, pixels: isize) -> Option<Retained<NSBitmapImageRep>> {
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
    Some(rep)
}

/// Draws the icon and returns it as a template image, or `None` if AppKit
/// could not create the bitmap.
pub fn render(layout: &IconLayout) -> Option<Retained<NSImage>> {
    let size = NSSize::new(ICON_SIZE_PT, ICON_SIZE_PT);
    let rep = make_bitmap_rep(size, CANVAS_PX as isize)?;
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    image.addRepresentation(&rep);

    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    context.setShouldAntialias(true);
    for (idx, bar) in layout.bars.iter().enumerate() {
        draw_bar(&context, bar, layout, idx == 0);
    }
    if let Some(overlay) = layout.overlay {
        draw_overlay(&context, overlay);
    }
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    image.setTemplate(true);
    Some(image)
}

/// Draws one frame of the unbraid morph animation (`IconRenderer.makeMorphIcon`).
pub fn render_morph(morph: &MorphLayout) -> Option<Retained<NSImage>> {
    let size = NSSize::new(ICON_SIZE_PT, ICON_SIZE_PT);
    let rep = make_bitmap_rep(size, CANVAS_PX as isize)?;
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    image.addRepresentation(&rep);

    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    context.setShouldAntialias(true);

    for seg in &morph.ribbons {
        if seg.alpha <= 0.0 {
            continue;
        }
        let rect = NSRect::new(
            NSPoint::new(-seg.length / 2.0, -seg.thickness / 2.0),
            NSSize::new(seg.length, seg.thickness),
        );
        let path = rounded(rect, seg.thickness / 2.0);
        let transform = NSAffineTransform::transform();
        transform.translateXBy_yBy(seg.center.x, seg.center.y);
        transform.rotateByDegrees(seg.angle_deg);
        path.transformUsingAffineTransform(&transform);
        label(seg.alpha).setFill();
        path.fill();
    }

    if let Some((bars_layout, bar_t)) = &morph.crossfade_bars
        && let Some(bars_img) = render(bars_layout)
    {
        let canvas = NSRect::new(NSPoint::ZERO, size);
        bars_img.drawInRect_fromRect_operation_fraction(
            canvas,
            NSRect::ZERO,
            NSCompositingOperation::SourceOver,
            *bar_t,
        );
    }

    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    image.setTemplate(true);
    Some(image)
}

/// Wraps `base` in the non-template red quota-warning flash overlay
/// (`StatusItemController.quotaWarningFlashImage`).
pub fn quota_warning_flash_image(base: &NSImage) -> Option<Retained<NSImage>> {
    let size = base.size();
    let w_px = (size.width * ICON_SCALE).round().max(1.0) as isize;
    let h_px = (size.height * ICON_SCALE).round().max(1.0) as isize;
    // SAFETY: a null plane pointer asks AppKit to allocate the bitmap; the
    // remaining arguments describe a standard 8-bit RGBA layout.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            w_px,
            h_px,
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
    let output = NSImage::initWithSize(NSImage::alloc(), size);
    output.addRepresentation(&rep);

    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    context.setShouldAntialias(true);

    let rect = NSRect::new(NSPoint::ZERO, size);
    let badge_rect = NSRect::new(
        NSPoint::new(1.0, 1.0),
        NSSize::new((size.width - 2.0).max(0.0), (size.height - 2.0).max(0.0)),
    );
    NSColor::systemRedColor()
        .colorWithAlphaComponent(0.22)
        .setFill();
    rounded(badge_rect, 4.0).fill();

    base.drawInRect_fromRect_operation_fraction(
        rect,
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.0,
    );

    NSGraphicsContext::saveGraphicsState_class();
    context.setCompositingOperation(NSCompositingOperation::SourceAtop);
    NSColor::systemRedColor()
        .colorWithAlphaComponent(0.28)
        .setFill();
    NSBezierPath::fillRect(rect);
    NSGraphicsContext::restoreGraphicsState_class();

    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    output.setTemplate(false);
    Some(output)
}

/// Tints a template icon image with `rgb` (`MenuBarLayoutRenderer.attachmentImage`).
pub fn tint_image(
    base: &NSImage,
    rgb: (u8, u8, u8),
    is_monochrome: bool,
) -> Option<Retained<NSImage>> {
    if !base.isTemplate() {
        return Some(base.retain());
    }
    let size = base.size();
    let w_px = (size.width * ICON_SCALE).round().max(1.0) as isize;
    let h_px = (size.height * ICON_SCALE).round().max(1.0) as isize;
    // SAFETY: a null plane pointer asks AppKit to allocate the bitmap; the
    // remaining arguments describe a standard 8-bit RGBA layout.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            w_px,
            h_px,
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
    let output = NSImage::initWithSize(NSImage::alloc(), size);
    output.addRepresentation(&rep);

    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    context.setShouldAntialias(true);

    let rect = NSRect::new(NSPoint::ZERO, size);
    base.drawInRect_fromRect_operation_fraction(
        rect,
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.0,
    );

    NSGraphicsContext::saveGraphicsState_class();
    context.setCompositingOperation(NSCompositingOperation::SourceAtop);
    let color = NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(rgb.0) / 255.0,
        f64::from(rgb.1) / 255.0,
        f64::from(rgb.2) / 255.0,
        1.0,
    );
    color.setFill();
    NSBezierPath::fillRect(rect);
    NSGraphicsContext::restoreGraphicsState_class();

    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    output.setTemplate(is_monochrome);
    Some(output)
}

/// Renders `layout` as a 36 × 36 PNG image for a dark, light, or highlighted menu bar
/// background (used for visual verification and deterministic tests).
pub fn render_png_for_appearance(
    layout: &IconLayout,
    dark: bool,
    highlighted: bool,
    tint_rgb: Option<(u8, u8, u8)>,
    warning_flash: bool,
) -> Option<Vec<u8>> {
    let base = render(layout)?;
    let foreground = if highlighted {
        (255, 255, 255)
    } else if let Some(rgb) = tint_rgb {
        rgb
    } else if dark {
        (255, 255, 255)
    } else {
        (0, 0, 0)
    };
    let styled = if warning_flash {
        let mono = tint_image(&base, foreground, true)?;
        quota_warning_flash_image(&mono)?
    } else {
        tint_image(&base, foreground, false)?
    };
    image_png_bytes(&styled)
}

/// Extracts PNG bytes from an `NSImage` by rendering it onto a 36 × 36 RGBA bitmap.
pub fn image_png_bytes(image: &NSImage) -> Option<Vec<u8>> {
    let size = image.size();
    let w_px = (size.width * ICON_SCALE).round().max(1.0) as isize;
    let h_px = (size.height * ICON_SCALE).round().max(1.0) as isize;
    // SAFETY: a null plane pointer asks AppKit to allocate the bitmap; the
    // remaining arguments describe a standard 8-bit RGBA layout.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            w_px,
            h_px,
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
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    let rect = NSRect::new(NSPoint::ZERO, size);
    image.drawInRect_fromRect_operation_fraction(
        rect,
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.0,
    );
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    // SAFETY: passing an empty dictionary for PNG properties is standard AppKit usage.
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(data.to_vec())
}

fn draw_bar(context: &NSGraphicsContext, bar: &Bar, layout: &IconLayout, is_top_bar: bool) {
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

    if is_top_bar && !layout.critters.is_empty() {
        draw_critters(context, &layout.critters);
    }
}

fn draw_critters(context: &NSGraphicsContext, critters: &CritterDecorations) {
    let alpha = critters.alpha;
    if let Some(face) = &critters.face {
        draw_face(context, face, alpha);
    }
    if let Some(notches) = &critters.notches {
        draw_notches(context, notches, alpha);
    }
    if let Some(gemini) = &critters.gemini {
        draw_gemini(context, gemini, alpha);
    }
    if let Some(dot) = critters.antigravity_dot {
        NSGraphicsContext::saveGraphicsState_class();
        label(alpha).setFill();
        NSBezierPath::bezierPathWithOvalInRect(px_rect(dot)).fill();
        NSGraphicsContext::restoreGraphicsState_class();
    }
    if let Some(factory) = &critters.factory {
        draw_factory(context, factory, alpha);
    }
    if let Some(warp) = &critters.warp {
        draw_warp(context, warp, alpha);
    }
    if let Some(grok) = &critters.grok {
        draw_grok(context, grok, alpha);
    }
}

fn draw_face(context: &NSGraphicsContext, face: &FaceDecoration, alpha: f64) {
    NSGraphicsContext::saveGraphicsState_class();
    context.setShouldAntialias(false);
    context.setCompositingOperation(NSCompositingOperation::Clear);
    NSBezierPath::bezierPathWithRect(px_rect(face.left_eye)).fill();
    NSBezierPath::bezierPathWithRect(px_rect(face.right_eye)).fill();
    NSGraphicsContext::restoreGraphicsState_class();

    if let (Some(left_blink), Some(right_blink)) = (face.left_blink, face.right_blink) {
        NSGraphicsContext::saveGraphicsState_class();
        context.setShouldAntialias(false);
        label(alpha).setFill();
        NSBezierPath::bezierPathWithRect(px_rect(left_blink)).fill();
        NSBezierPath::bezierPathWithRect(px_rect(right_blink)).fill();
        NSGraphicsContext::restoreGraphicsState_class();
    }

    NSGraphicsContext::saveGraphicsState_class();
    if face.hat.tilt_radians.abs() > 0.0001 {
        let t = NSAffineTransform::transform();
        t.translateXBy_yBy(face.hat.pivot.x, face.hat.pivot.y);
        t.rotateByRadians(face.hat.tilt_radians);
        t.translateXBy_yBy(-face.hat.pivot.x, -face.hat.pivot.y + face.hat.y_offset_pt);
        t.concat();
    }
    label(alpha).setFill();
    NSBezierPath::bezierPathWithRect(px_rect(face.hat.rect)).fill();
    NSGraphicsContext::restoreGraphicsState_class();
}

fn draw_notches(context: &NSGraphicsContext, notches: &NotchesDecoration, alpha: f64) {
    NSGraphicsContext::saveGraphicsState_class();
    context.setShouldAntialias(false);
    label(alpha).setFill();
    NSBezierPath::bezierPathWithRect(px_rect(notches.left_arm)).fill();
    NSBezierPath::bezierPathWithRect(px_rect(notches.right_arm)).fill();
    for leg in &notches.legs {
        NSBezierPath::bezierPathWithRect(px_rect(*leg)).fill();
    }

    context.setCompositingOperation(NSCompositingOperation::Clear);
    NSBezierPath::bezierPathWithRect(px_rect(notches.left_eye)).fill();
    NSBezierPath::bezierPathWithRect(px_rect(notches.right_eye)).fill();

    if let (Some(left_blink), Some(right_blink)) = (notches.left_blink, notches.right_blink) {
        context.setCompositingOperation(NSCompositingOperation::SourceOver);
        label(alpha).setFill();
        NSBezierPath::bezierPathWithRect(px_rect(left_blink)).fill();
        NSBezierPath::bezierPathWithRect(px_rect(right_blink)).fill();
    }
    NSGraphicsContext::restoreGraphicsState_class();
}

fn draw_gemini(context: &NSGraphicsContext, gemini: &GeminiDecoration, alpha: f64) {
    NSGraphicsContext::saveGraphicsState_class();
    context.setCompositingOperation(NSCompositingOperation::Clear);
    polygon_path(&gemini.left_star).fill();
    polygon_path(&gemini.right_star).fill();
    NSGraphicsContext::restoreGraphicsState_class();

    if let (Some(left_blink), Some(right_blink)) =
        (&gemini.left_blink_star, &gemini.right_blink_star)
    {
        NSGraphicsContext::saveGraphicsState_class();
        label(alpha).setFill();
        polygon_path(left_blink).fill();
        polygon_path(right_blink).fill();
        NSGraphicsContext::restoreGraphicsState_class();
    }

    NSGraphicsContext::saveGraphicsState_class();
    label(alpha).setFill();
    polygon_path(&gemini.top_point).fill();
    polygon_path(&gemini.bottom_point).fill();
    polygon_path(&gemini.left_point).fill();
    polygon_path(&gemini.right_point).fill();
    NSGraphicsContext::restoreGraphicsState_class();
}

fn draw_factory(context: &NSGraphicsContext, factory: &FactoryDecoration, alpha: f64) {
    NSGraphicsContext::saveGraphicsState_class();
    context.setCompositingOperation(NSCompositingOperation::Clear);
    polygon_path(&factory.left_asterisk).fill();
    polygon_path(&factory.right_asterisk).fill();
    NSGraphicsContext::restoreGraphicsState_class();

    if let (Some(left_blink), Some(right_blink)) =
        (&factory.left_blink_asterisk, &factory.right_blink_asterisk)
    {
        NSGraphicsContext::saveGraphicsState_class();
        label(alpha).setFill();
        polygon_path(left_blink).fill();
        polygon_path(right_blink).fill();
        NSGraphicsContext::restoreGraphicsState_class();
    }

    NSGraphicsContext::saveGraphicsState_class();
    label(alpha).setFill();
    for tooth in &factory.teeth {
        NSBezierPath::bezierPathWithRect(px_rect(*tooth)).fill();
    }
    NSGraphicsContext::restoreGraphicsState_class();
}

fn draw_warp(context: &NSGraphicsContext, warp: &WarpDecoration, alpha: f64) {
    let draw_eye = |eye: &TiltedEllipse| {
        NSGraphicsContext::saveGraphicsState_class();
        if warp.eyes_filled {
            context.setCompositingOperation(NSCompositingOperation::SourceOver);
            label(alpha).setFill();
        } else {
            context.setCompositingOperation(NSCompositingOperation::Clear);
        }
        let t = NSAffineTransform::transform();
        t.translateXBy_yBy(eye.center.x, eye.center.y);
        t.rotateByRadians(eye.tilt_radians);
        t.concat();
        let rect = NSRect::new(
            NSPoint::new(-eye.width_pt / 2.0, -eye.height_pt / 2.0),
            NSSize::new(eye.width_pt, eye.height_pt),
        );
        NSBezierPath::bezierPathWithOvalInRect(rect).fill();
        NSGraphicsContext::restoreGraphicsState_class();
    };
    draw_eye(&warp.left_eye);
    draw_eye(&warp.right_eye);
}

fn draw_grok(context: &NSGraphicsContext, grok: &GrokDecoration, alpha: f64) {
    NSGraphicsContext::saveGraphicsState_class();
    label(alpha).setFill();
    NSBezierPath::bezierPathWithRect(px_rect(grok.left_horn)).fill();
    NSBezierPath::bezierPathWithRect(px_rect(grok.right_horn)).fill();
    context.setCompositingOperation(NSCompositingOperation::Clear);
    rounded(px_rect(grok.visor), grok.visor_corner_radius_pt).fill();
    NSGraphicsContext::restoreGraphicsState_class();
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
