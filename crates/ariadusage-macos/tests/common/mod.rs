//! Test doubles for the Foundation date formatters and AppKit text metrics.

#![allow(dead_code)]

use ariadusage_macos::card_layout::{Font, TextMeasure, TextStyle};
use ariadusage_macos::format::DateTexts;
use ariadusage_macos::geometry::Size;
use jiff::Timestamp;
use jiff::tz::TimeZone;

/// Dates in a fixed zone, formatted like the `en_US_POSIX` locale.
pub struct FixedDates {
    pub tz: TimeZone,
}

impl FixedDates {
    pub fn utc() -> Self {
        Self { tz: TimeZone::UTC }
    }

    pub fn zone(name: &str) -> Self {
        Self {
            tz: TimeZone::get(name).expect("known time zone"),
        }
    }
}

impl DateTexts for FixedDates {
    fn time_zone(&self) -> TimeZone {
        self.tz.clone()
    }

    fn hour_minute(&self, date: Timestamp) -> String {
        date.to_zoned(self.tz.clone())
            .strftime("%-I:%M %p")
            .to_string()
    }

    fn month_day_hour_minute(&self, date: Timestamp) -> String {
        date.to_zoned(self.tz.clone())
            .strftime("%b %-d at %-I:%M %p")
            .to_string()
    }

    fn relative_abbreviated(&self, date: Timestamp, now: Timestamp) -> String {
        let seconds = now.duration_since(date).as_secs();
        if seconds >= 3600 {
            format!("{} hr ago", seconds / 3600)
        } else {
            format!("{} min ago", seconds / 60)
        }
    }
}

/// Non-US (`fr_FR`) test double for `DateTexts`: `month_day_year`, `month_day_short`,
/// `detail_month_day_time`, and `format_one_decimal` follow `Locale.current` (`fr_FR`),
/// while `quota_window_range` remains pinned to the `en_US` resource locale.
pub struct FrenchDates {
    pub tz: TimeZone,
}

impl FrenchDates {
    pub fn utc() -> Self {
        Self { tz: TimeZone::UTC }
    }
}

const FR_MONTH_ABBR: [&str; 12] = [
    "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
    "déc.",
];

impl DateTexts for FrenchDates {
    fn time_zone(&self) -> TimeZone {
        self.tz.clone()
    }

    fn hour_minute(&self, date: Timestamp) -> String {
        date.to_zoned(self.tz.clone()).strftime("%H:%M").to_string()
    }

    fn month_day_hour_minute(&self, date: Timestamp) -> String {
        let z = date.to_zoned(self.tz.clone());
        let m = (z.month() - 1).clamp(0, 11) as usize;
        format!(
            "{} {} à {:02}:{:02}",
            z.day(),
            FR_MONTH_ABBR[m],
            z.hour(),
            z.minute()
        )
    }

    fn relative_abbreviated(&self, date: Timestamp, now: Timestamp) -> String {
        let seconds = now.duration_since(date).as_secs();
        format!("il y a {} min", seconds / 60)
    }

    fn month_day_year(&self, date: Timestamp, date_only: bool) -> String {
        let tz = if date_only {
            TimeZone::UTC
        } else {
            self.tz.clone()
        };
        let z = date.to_zoned(tz);
        let m = (z.month() - 1).clamp(0, 11) as usize;
        format!("{} {} {}", z.day(), FR_MONTH_ABBR[m], z.year())
    }

    fn month_day_short(&self, date: Timestamp) -> String {
        let z = date.to_zoned(self.tz.clone());
        let m = (z.month() - 1).clamp(0, 11) as usize;
        format!("{} {}", z.day(), FR_MONTH_ABBR[m])
    }

    fn format_one_decimal(&self, value: f64) -> String {
        ariadusage_macos::format::format_one_decimal_en_us(value)
            .replace(',', "\u{202f}")
            .replace('.', ",")
    }
}

/// Text metrics with a fixed advance per character and a line height per
/// text style.
pub struct FixedMeasure;

pub const CHAR_WIDTH: f64 = 6.0;

pub fn line_height(font: Font) -> f64 {
    match font.style {
        TextStyle::Title2 => 22.0,
        TextStyle::Headline | TextStyle::Body => 16.0,
        TextStyle::Subheadline => 14.0,
        TextStyle::Footnote | TextStyle::Caption | TextStyle::Caption2 => 13.0,
    }
}

pub fn baseline(font: Font) -> f64 {
    match font.style {
        TextStyle::Title2 => 18.0,
        TextStyle::Headline | TextStyle::Body => 13.0,
        TextStyle::Subheadline => 11.0,
        TextStyle::Footnote | TextStyle::Caption | TextStyle::Caption2 => 10.0,
    }
}

impl TextMeasure for FixedMeasure {
    fn measure(&self, text: &str, font: Font, max_lines: usize, max_width: f64) -> Size {
        let natural = text.chars().count() as f64 * CHAR_WIDTH;
        if natural <= max_width {
            return Size::new(natural, line_height(font));
        }
        let wanted = (natural / max_width).ceil() as usize;
        let lines = if max_lines == 0 {
            wanted
        } else {
            wanted.min(max_lines)
        };
        Size::new(max_width, lines as f64 * line_height(font))
    }

    fn first_baseline(&self, font: Font) -> f64 {
        baseline(font)
    }
}

pub fn ts(text: &str) -> Timestamp {
    text.parse().expect("valid timestamp")
}

/// Strict maximum percentage of pixels allowed to differ between a rendered
/// view and its golden capture PNG before the pixel-diff harness fails.
pub const PIXEL_DIFF_TOLERANCE_PERCENT: f64 = 1.0;

/// Per-channel (8-bit sRGB/RGBA) rounding tolerance before a pixel is counted
/// as mismatched.
pub const PIXEL_CHANNEL_TOLERANCE: u8 = 8;

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub mod pixel_diff {
    use std::path::{Path, PathBuf};

    use ariadusage_macos::appkit::{
        CardView, CostHistoryChartView, PlanUtilizationChartView, QuotaBurndownChartView,
        SwitcherView, png_bytes,
    };
    use ariadusage_macos::card_layout::CardSectionMode;
    use ariadusage_macos::menu_descriptor::{
        MenuEntry, MenuStructure, PrimaryContent, SubmenuEntry,
    };
    use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSApplication, NSBitmapImageRep, NSColor, NSFont,
        NSLineBreakMode, NSTextAlignment, NSTextField, NSView,
    };
    use objc2_foundation::{NSData, NSPoint, NSRect, NSSize, NSString};

    use super::{PIXEL_CHANNEL_TOLERANCE, PIXEL_DIFF_TOLERANCE_PERCENT};

    /// Environment variable pointing to the external golden captures directory.
    pub const CAPTURES_DIR_ENV: &str = "ARIADUSAGE_CAPTURES_DIR";

    const DARK_MENU_BG: [u8; 4] = [32, 32, 32, 255];
    const DARK_MENU_DIVIDER: [u8; 4] = [65, 65, 68, 255];

    /// Decoded 8-bit straight RGBA pixel buffer (`width * height * 4` bytes).
    #[derive(Clone, Debug)]
    pub struct RgbaImage {
        pub width: usize,
        pub height: usize,
        pub data: Vec<u8>,
    }

    impl RgbaImage {
        pub fn new_solid(width: usize, height: usize, rgba: [u8; 4]) -> Self {
            let mut data = Vec::with_capacity(width * height * 4);
            for _ in 0..(width * height) {
                data.extend_from_slice(&rgba);
            }
            Self {
                width,
                height,
                data,
            }
        }

        pub fn from_bitmap_rep(rep: &NSBitmapImageRep) -> Self {
            if let Some(bytes) = png_bytes(rep)
                && let Some(decoded) = Self::from_png_bytes(&bytes)
            {
                return decoded;
            }
            Self::extract_from_rep(rep)
        }

        pub fn from_png_bytes(bytes: &[u8]) -> Option<Self> {
            let ns_data = NSData::with_bytes(bytes);
            let rep = NSBitmapImageRep::imageRepWithData(&ns_data)?;
            Some(Self::extract_from_rep(&rep))
        }

        pub fn from_png_file(path: &Path) -> Result<Self, String> {
            let bytes = std::fs::read(path)
                .map_err(|e| format!("failed to read PNG `{}`: {e}", path.display()))?;
            Self::from_png_bytes(&bytes)
                .ok_or_else(|| format!("failed to decode PNG `{}`", path.display()))
        }

        fn extract_from_rep(rep: &NSBitmapImageRep) -> Self {
            let width = rep.pixelsWide().max(0) as usize;
            let height = rep.pixelsHigh().max(0) as usize;
            let mut data = vec![0u8; width * height * 4];
            if width == 0 || height == 0 {
                return Self {
                    width,
                    height,
                    data,
                };
            }

            let bps = rep.bitsPerSample() as usize;
            let spp = rep.samplesPerPixel() as usize;
            let bpp = rep.bitsPerPixel() as usize;
            let bpr = rep.bytesPerRow() as usize;
            let is_planar = rep.isPlanar();
            let ptr = rep.bitmapData();

            if !ptr.is_null()
                && !is_planar
                && bps == 8
                && (bpp == 32 || bpp == 24)
                && (spp == 4 || spp == 3)
            {
                let step = bpp / 8;
                for y in 0..height {
                    // SAFETY: `ptr` points to at least `height * bpr` bytes in the non-planar
                    // 8-bit `NSBitmapImageRep`, and `x * step + spp <= bpr`.
                    let row = unsafe { std::slice::from_raw_parts(ptr.add(y * bpr), bpr) };
                    for x in 0..width {
                        let src = x * step;
                        let dst = (y * width + x) * 4;
                        let a = if spp == 4 { row[src + 3] } else { 255 };
                        let (r, g, b) = if a > 0 && a < 255 {
                            (
                                ((u16::from(row[src]) * 255 + u16::from(a) / 2) / u16::from(a))
                                    .min(255) as u8,
                                ((u16::from(row[src + 1]) * 255 + u16::from(a) / 2) / u16::from(a))
                                    .min(255) as u8,
                                ((u16::from(row[src + 2]) * 255 + u16::from(a) / 2) / u16::from(a))
                                    .min(255) as u8,
                            )
                        } else {
                            (row[src], row[src + 1], row[src + 2])
                        };
                        data[dst] = r;
                        data[dst + 1] = g;
                        data[dst + 2] = b;
                        data[dst + 3] = a;
                    }
                }
                return Self {
                    width,
                    height,
                    data,
                };
            }

            for y in 0..height {
                for x in 0..width {
                    let dst = (y * width + x) * 4;
                    if let Some(c) = rep.colorAtX_y(x as isize, y as isize) {
                        data[dst] = (c.redComponent().clamp(0.0, 1.0) * 255.0).round() as u8;
                        data[dst + 1] = (c.greenComponent().clamp(0.0, 1.0) * 255.0).round() as u8;
                        data[dst + 2] = (c.blueComponent().clamp(0.0, 1.0) * 255.0).round() as u8;
                        data[dst + 3] = (c.alphaComponent().clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                }
            }
            Self {
                width,
                height,
                data,
            }
        }

        pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
            let idx = (y * self.width + x) * 4;
            [
                self.data[idx],
                self.data[idx + 1],
                self.data[idx + 2],
                self.data[idx + 3],
            ]
        }

        pub fn set_pixel(&mut self, x: usize, y: usize, rgba: [u8; 4]) {
            if x < self.width && y < self.height {
                let idx = (y * self.width + x) * 4;
                self.data[idx..idx + 4].copy_from_slice(&rgba);
            }
        }

        pub fn crop(&self, x: usize, y: usize, w: usize, h: usize) -> Self {
            let x0 = x.min(self.width);
            let y0 = y.min(self.height);
            let cw = w.min(self.width.saturating_sub(x0));
            let ch = h.min(self.height.saturating_sub(y0));
            let mut out = Self::new_solid(cw, ch, [0, 0, 0, 0]);
            for ry in 0..ch {
                for rx in 0..cw {
                    out.set_pixel(rx, ry, self.pixel(x0 + rx, y0 + ry));
                }
            }
            out
        }

        pub fn blend_over_solid(&self, bg: [u8; 4]) -> Self {
            let mut out = Self::new_solid(self.width, self.height, bg);
            for y in 0..self.height {
                for x in 0..self.width {
                    let fg = self.pixel(x, y);
                    let a = u16::from(fg[3]);
                    let inv = 255 - a;
                    let r = ((u16::from(fg[0]) * a + u16::from(bg[0]) * inv + 127) / 255) as u8;
                    let g = ((u16::from(fg[1]) * a + u16::from(bg[1]) * inv + 127) / 255) as u8;
                    let b = ((u16::from(fg[2]) * a + u16::from(bg[2]) * inv + 127) / 255) as u8;
                    out.set_pixel(x, y, [r, g, b, 255]);
                }
            }
            out
        }

        pub fn to_png_bytes(&self) -> Option<Vec<u8>> {
            if self.width == 0 || self.height == 0 {
                return None;
            }
            // SAFETY: a null plane pointer asks AppKit to allocate an 8-bit 4-channel RGBA buffer.
            let rep = unsafe {
                NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                    NSBitmapImageRep::alloc(),
                    std::ptr::null_mut(),
                    self.width as isize,
                    self.height as isize,
                    8,
                    4,
                    true,
                    false,
                    objc2_app_kit::NSDeviceRGBColorSpace,
                    (self.width * 4) as isize,
                    32,
                )
            }?;
            let ptr = rep.bitmapData();
            if ptr.is_null() {
                return None;
            }
            let bpr = rep.bytesPerRow() as usize;
            for y in 0..self.height {
                // SAFETY: `ptr` has `height * bpr` allocated bytes and `bpr >= width * 4`.
                let dst_row = unsafe { std::slice::from_raw_parts_mut(ptr.add(y * bpr), bpr) };
                for x in 0..self.width {
                    let [r, g, b, a] = self.pixel(x, y);
                    let dst = x * 4;
                    dst_row[dst] = ((u16::from(r) * u16::from(a) + 127) / 255) as u8;
                    dst_row[dst + 1] = ((u16::from(g) * u16::from(a) + 127) / 255) as u8;
                    dst_row[dst + 2] = ((u16::from(b) * u16::from(a) + 127) / 255) as u8;
                    dst_row[dst + 3] = a;
                }
            }
            png_bytes(&rep)
        }
    }

    /// Result of comparing one rendered view against its golden capture PNG.
    #[derive(Clone, Debug)]
    pub struct PixelDiffRow {
        pub view_name: String,
        pub capture_rel_path: String,
        pub actual_size: (usize, usize),
        pub expected_size: (usize, usize),
        pub mismatched_pixels: usize,
        pub total_pixels: usize,
        pub mismatch_percent: f64,
        pub overlap_mismatch_percent: f64,
        pub diff_png_path: PathBuf,
    }

    /// Resolves `ARIADUSAGE_CAPTURES_DIR`. When unset or empty, prints a clear skip
    /// message and returns `None` so CI never requires local captures.
    pub fn captures_dir_from_env(suite_name: &str) -> Option<PathBuf> {
        match std::env::var_os(CAPTURES_DIR_ENV) {
            Some(val) if !val.is_empty() => Some(PathBuf::from(val)),
            _ => {
                println!(
                    "SKIP {suite_name}: {CAPTURES_DIR_ENV} is unset; skipping golden capture pixel diff."
                );
                None
            }
        }
    }

    /// Creates a persistent temporary directory under `<target_dir>/pixel-diff/<suite>-*`
    /// where diff and actual PNGs are written.
    pub fn create_diff_output_dir(suite_name: &str) -> PathBuf {
        let target_dir = resolve_cargo_target_dir();
        let parent = target_dir.join("pixel-diff");
        std::fs::create_dir_all(&parent).expect("create target/pixel-diff directory");
        tempfile::Builder::new()
            .prefix(&format!("{suite_name}-"))
            .tempdir_in(&parent)
            .expect("create temp diff directory under target/pixel-diff")
            .keep()
    }

    fn resolve_cargo_target_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR")
            && !dir.is_empty()
        {
            return PathBuf::from(dir);
        }
        if let Ok(exe) = std::env::current_exe() {
            for ancestor in exe.ancestors() {
                if ancestor.file_name().and_then(|s| s.to_str()) == Some("target") {
                    return ancestor.to_path_buf();
                }
            }
            if let Some(deps_dir) = exe.parent()
                && let Some(profile_dir) = deps_dir.parent()
                && let Some(target_dir) = profile_dir.parent()
            {
                return target_dir.to_path_buf();
            }
        }
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .join("target")
    }

    fn sanitize_filename(name: &str) -> String {
        name.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    }

    /// Compares `actual` against `expected`, writes `{view_name}-diff.png` and
    /// `{view_name}-actual.png` into `diff_dir`, and returns the measured [`PixelDiffRow`].
    ///
    /// Note: `{view_name}-diff.png` uses dimmed pixels from `actual` (fixture-rendered)
    /// for matching regions, crimson `(255, 45, 85)` for mismatched pixels within the
    /// overlapping region, and amber `(255, 159, 10)` for out-of-bounds dimension
    /// differences, so zero pixels from `expected` are ever written to disk.
    pub fn compare_and_write_diff(
        view_name: &str,
        capture_rel_path: &str,
        actual: &RgbaImage,
        expected: &RgbaImage,
        diff_dir: &Path,
    ) -> PixelDiffRow {
        let max_w = actual.width.max(expected.width);
        let max_h = actual.height.max(expected.height);
        let min_w = actual.width.min(expected.width);
        let min_h = actual.height.min(expected.height);
        let total_pixels = (max_w * max_h).max(1);
        let overlap_total = (min_w * min_h).max(1);

        let mut mismatched_pixels = 0usize;
        let mut overlap_mismatched = 0usize;
        let mut diff_img = RgbaImage::new_solid(max_w, max_h, [0, 0, 0, 255]);

        for y in 0..max_h {
            for x in 0..max_w {
                if x < min_w && y < min_h {
                    let a = actual.pixel(x, y);
                    let e = expected.pixel(x, y);
                    let dr = a[0].abs_diff(e[0]);
                    let dg = a[1].abs_diff(e[1]);
                    let db = a[2].abs_diff(e[2]);
                    let da = a[3].abs_diff(e[3]);
                    let max_delta = dr.max(dg).max(db).max(da);
                    if max_delta <= PIXEL_CHANNEL_TOLERANCE {
                        let luma =
                            ((u16::from(a[0]) * 77 + u16::from(a[1]) * 150 + u16::from(a[2]) * 29)
                                >> 8) as u8;
                        let dimmed = (luma / 3).saturating_add(16);
                        diff_img.set_pixel(x, y, [dimmed, dimmed, dimmed, 255]);
                    } else {
                        mismatched_pixels += 1;
                        overlap_mismatched += 1;
                        diff_img.set_pixel(x, y, [255, 45, 85, 255]);
                    }
                } else {
                    mismatched_pixels += 1;
                    diff_img.set_pixel(x, y, [255, 159, 10, 255]);
                }
            }
        }

        let stem = sanitize_filename(view_name);
        let diff_png_path = diff_dir.join(format!("{stem}-diff.png"));
        if let Some(png) = diff_img.to_png_bytes() {
            std::fs::write(&diff_png_path, png).expect("write diff PNG");
        }
        let actual_png_path = diff_dir.join(format!("{stem}-actual.png"));
        if let Some(png) = actual.to_png_bytes() {
            std::fs::write(&actual_png_path, png).expect("write actual PNG");
        }

        let mismatch_percent = (mismatched_pixels as f64 / total_pixels as f64) * 100.0;
        let overlap_mismatch_percent = (overlap_mismatched as f64 / overlap_total as f64) * 100.0;

        PixelDiffRow {
            view_name: view_name.to_string(),
            capture_rel_path: capture_rel_path.to_string(),
            actual_size: (actual.width, actual.height),
            expected_size: (expected.width, expected.height),
            mismatched_pixels,
            total_pixels,
            mismatch_percent,
            overlap_mismatch_percent,
            diff_png_path,
        }
    }

    /// Prints a Markdown table of all per-view mismatch results and asserts that every
    /// view is within [`PIXEL_DIFF_TOLERANCE_PERCENT`].
    pub fn report_and_assert_suite(suite_name: &str, diff_dir: &Path, rows: &[PixelDiffRow]) {
        println!("\n=== Pixel Diff Suite: {suite_name} ===");
        println!(
            "Tolerance: {PIXEL_DIFF_TOLERANCE_PERCENT:.2}% (channel tolerance: ±{PIXEL_CHANNEL_TOLERANCE})"
        );
        println!("Diff output dir: {}", diff_dir.display());
        println!(
            "| View | Capture | Actual (px) | Expected (px) | Mismatched / Total | Mismatch % | Overlap Mismatch % | Status |"
        );
        println!("| --- | --- | --- | --- | --- | --- | --- | --- |");
        let mut failing = Vec::new();
        for row in rows {
            let passed = row.mismatch_percent <= PIXEL_DIFF_TOLERANCE_PERCENT;
            let status = if passed { "PASS" } else { "FAIL" };
            println!(
                "| `{}` | `{}` | `{}x{}` | `{}x{}` | {} / {} | {:.2}% | {:.2}% | {} |",
                row.view_name,
                row.capture_rel_path,
                row.actual_size.0,
                row.actual_size.1,
                row.expected_size.0,
                row.expected_size.1,
                row.mismatched_pixels,
                row.total_pixels,
                row.mismatch_percent,
                row.overlap_mismatch_percent,
                status,
            );
            if !passed {
                failing.push(format!(
                    "{} ({:.2}% > {:.2}%, diff: {})",
                    row.view_name,
                    row.mismatch_percent,
                    PIXEL_DIFF_TOLERANCE_PERCENT,
                    row.diff_png_path.display()
                ));
            }
        }
        println!();
        assert!(
            failing.is_empty(),
            "{suite_name}: {} of {} views exceeded PIXEL_DIFF_TOLERANCE_PERCENT ({PIXEL_DIFF_TOLERANCE_PERCENT:.2}%):\n  - {}",
            failing.len(),
            rows.len(),
            failing.join("\n  - ")
        );
    }

    /// Composites a complete dark-mode menu (`MenuStructure`) offscreen at `width_pt`
    /// (`620 px` wide at `@2x`) for pixel comparison against `dark/tab-*-w0.png`.
    #[allow(deprecated)]
    pub fn render_dark_menu_structure(
        mtm: MainThreadMarker,
        structure: &MenuStructure,
        width_pt: f64,
    ) -> RgbaImage {
        let dark_app =
            NSAppearance::appearanceNamed(&NSString::from_str("NSAppearanceNameDarkAqua"))
                .expect("DarkAqua appearance");
        NSApplication::sharedApplication(mtm).setAppearance(Some(&dark_app));
        // SAFETY: `dark_app` is a valid `NSAppearance` on the main thread.
        unsafe {
            NSAppearance::setCurrentAppearance(Some(&dark_app));
        }

        let width_px = (width_pt * 2.0).round() as usize;
        let mut strips: Vec<RgbaImage> = Vec::new();

        // Top menu padding (5 pt = 10 px).
        strips.push(RgbaImage::new_solid(width_px, 10, DARK_MENU_BG));

        if let Some(selected) = &structure.selection
            && !structure.switcher.is_empty()
        {
            let sw = SwitcherView::new(mtm);
            sw.setAppearance(Some(&dark_app));
            sw.update(&structure.switcher, selected, width_pt);
            let sw_rep = sw.render_bitmap().expect("switcher bitmap");
            let sw_img = RgbaImage::from_bitmap_rep(&sw_rep).blend_over_solid(DARK_MENU_BG);
            strips.push(sw_img);
            strips.push(make_separator_strip(width_px));
        }

        match &structure.primary {
            PrimaryContent::Disconnected { message } => {
                strips.push(render_menu_text_row(
                    mtm, &dark_app, width_pt, message, None, false,
                ));
                strips.push(make_separator_strip(width_px));
            }
            PrimaryContent::Provider {
                card,
                sections,
                buy_credits,
                ..
            } => {
                let mut buy_inserted = false;
                for (sec_idx, section) in sections.iter().enumerate() {
                    if sec_idx > 0 {
                        strips.push(make_separator_strip(width_px));
                    }
                    let cv = CardView::new(mtm);
                    cv.update_section(card, section.mode, width_pt);
                    let rep = cv
                        .render_bitmap_with_appearance(true)
                        .expect("provider card section bitmap");
                    strips.push(RgbaImage::from_bitmap_rep(&rep));
                    if matches!(section.mode, CardSectionMode::CreditsOnly { .. })
                        && let Some(buy) = buy_credits
                    {
                        strips.push(render_menu_entry_row(mtm, &dark_app, width_pt, buy));
                        buy_inserted = true;
                    }
                }
                if !buy_inserted && let Some(buy) = buy_credits {
                    strips.push(render_menu_entry_row(mtm, &dark_app, width_pt, buy));
                }
                strips.push(make_separator_strip(width_px));
            }
            PrimaryContent::Overview {
                spend_summary,
                share_action,
                cards,
            } => {
                if let Some(summary) = spend_summary {
                    let sv = CardView::new(mtm);
                    sv.update_spend_summary(summary, width_pt);
                    let rep = sv
                        .render_bitmap_with_appearance(true)
                        .expect("overview spend summary bitmap");
                    strips.push(RgbaImage::from_bitmap_rep(&rep));
                    if let Some(share) = share_action {
                        strips.push(render_menu_entry_row(mtm, &dark_app, width_pt, share));
                    }
                    strips.push(make_separator_strip(width_px));
                }
                for (idx, ov) in cards.iter().enumerate() {
                    if idx > 0 {
                        strips.push(make_separator_strip(width_px));
                    }
                    let cv = CardView::new(mtm);
                    cv.update_overview(&ov.model, width_pt);
                    let rep = cv
                        .render_bitmap_with_appearance(true)
                        .expect("overview card bitmap");
                    strips.push(RgbaImage::from_bitmap_rep(&rep));
                }
                if !cards.is_empty() {
                    strips.push(make_separator_strip(width_px));
                }
            }
        }

        if !structure.history.is_empty() {
            for entry in &structure.history {
                strips.push(render_menu_entry_row(mtm, &dark_app, width_pt, entry));
            }
            strips.push(make_separator_strip(width_px));
        }

        if !structure.actions.is_empty() {
            for entry in &structure.actions {
                strips.push(render_menu_entry_row(mtm, &dark_app, width_pt, entry));
            }
            strips.push(make_separator_strip(width_px));
        }

        for entry in &structure.meta {
            strips.push(render_menu_entry_row(mtm, &dark_app, width_pt, entry));
        }

        // Bottom menu padding (5 pt = 10 px).
        strips.push(RgbaImage::new_solid(width_px, 10, DARK_MENU_BG));

        vstack_strips(width_px, &strips)
    }

    /// Composites a dark-mode submenu (`&[SubmenuEntry]`) offscreen at `width_pt`
    /// (`620 px` wide at `@2x`) for pixel comparison against `dark/sub-*-w0.png`.
    #[allow(deprecated)]
    pub fn render_dark_submenu(
        mtm: MainThreadMarker,
        items: &[SubmenuEntry],
        width_pt: f64,
    ) -> RgbaImage {
        let dark_app =
            NSAppearance::appearanceNamed(&NSString::from_str("NSAppearanceNameDarkAqua"))
                .expect("DarkAqua appearance");
        NSApplication::sharedApplication(mtm).setAppearance(Some(&dark_app));
        // SAFETY: `dark_app` is a valid `NSAppearance` on the main thread.
        unsafe {
            NSAppearance::setCurrentAppearance(Some(&dark_app));
        }

        let width_px = (width_pt * 2.0).round() as usize;
        let mut strips: Vec<RgbaImage> = Vec::new();
        strips.push(RgbaImage::new_solid(width_px, 10, DARK_MENU_BG));

        for entry in items {
            match entry {
                SubmenuEntry::SecondaryText(text) => {
                    strips.push(render_menu_text_row(
                        mtm, &dark_app, width_pt, text, None, false,
                    ));
                }
                SubmenuEntry::Separator => {
                    strips.push(make_separator_strip(width_px));
                }
                SubmenuEntry::Action { title, enabled, .. } => {
                    strips.push(render_menu_text_row(
                        mtm, &dark_app, width_pt, title, None, *enabled,
                    ));
                }
                SubmenuEntry::QuotaBurndownChart(model) => {
                    let v = QuotaBurndownChartView::new(mtm, (**model).clone());
                    let rep = v
                        .render_bitmap_with_appearance(true)
                        .expect("burndown chart bitmap");
                    strips.push(RgbaImage::from_bitmap_rep(&rep));
                }
                SubmenuEntry::PlanUtilizationChart(model) => {
                    let v = PlanUtilizationChartView::new(mtm, (**model).clone());
                    let rep = v
                        .render_bitmap_with_appearance(true)
                        .expect("utilization chart bitmap");
                    strips.push(RgbaImage::from_bitmap_rep(&rep));
                }
                SubmenuEntry::CostHistoryChart(model) => {
                    let v = CostHistoryChartView::new(mtm, (**model).clone());
                    let rep = v
                        .render_bitmap_with_appearance(true)
                        .expect("cost chart bitmap");
                    strips.push(RgbaImage::from_bitmap_rep(&rep));
                }
            }
        }

        strips.push(RgbaImage::new_solid(width_px, 10, DARK_MENU_BG));
        vstack_strips(width_px, &strips)
    }

    fn make_separator_strip(width_px: usize) -> RgbaImage {
        let mut img = RgbaImage::new_solid(width_px, 22, DARK_MENU_BG);
        let x_start = 28.min(width_px);
        let x_end = width_px.saturating_sub(28);
        for y in 10..12 {
            for x in x_start..x_end {
                img.set_pixel(x, y, DARK_MENU_DIVIDER);
            }
        }
        img
    }

    fn render_menu_entry_row(
        mtm: MainThreadMarker,
        dark_app: &NSAppearance,
        width_pt: f64,
        entry: &MenuEntry,
    ) -> RgbaImage {
        match entry {
            MenuEntry::SecondaryText(text) => {
                render_menu_text_row(mtm, dark_app, width_pt, text, None, false)
            }
            MenuEntry::Action {
                title,
                key_equivalent,
                enabled,
                ..
            } => {
                let right = (!key_equivalent.is_empty())
                    .then(|| format!("⌘{}", key_equivalent.to_uppercase()));
                render_menu_text_row(mtm, dark_app, width_pt, title, right.as_deref(), *enabled)
            }
            MenuEntry::Submenu { title, .. } => {
                render_menu_text_row(mtm, dark_app, width_pt, title, Some("›"), true)
            }
        }
    }

    fn render_menu_text_row(
        mtm: MainThreadMarker,
        dark_app: &NSAppearance,
        width_pt: f64,
        title: &str,
        trailing: Option<&str>,
        enabled: bool,
    ) -> RgbaImage {
        let row_h = 22.0;
        let container = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width_pt, row_h)),
        );
        container.setAppearance(Some(dark_app));

        let label = NSTextField::labelWithString(&NSString::from_str(title), mtm);
        let font = NSFont::menuFontOfSize(13.0);
        label.setFont(Some(&font));
        let color = if enabled {
            NSColor::labelColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        label.setTextColor(Some(&color));
        label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        label.setFrame(NSRect::new(
            NSPoint::new(18.0, 3.0),
            NSSize::new((width_pt - 64.0).max(80.0), 16.0),
        ));
        container.addSubview(&label);

        if let Some(right_text) = trailing {
            let right_label = NSTextField::labelWithString(&NSString::from_str(right_text), mtm);
            right_label.setFont(Some(&font));
            right_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
            right_label.setAlignment(NSTextAlignment::Right);
            right_label.setFrame(NSRect::new(
                NSPoint::new(width_pt - 52.0, 3.0),
                NSSize::new(38.0, 16.0),
            ));
            container.addSubview(&right_label);
        }

        let bounds = container.bounds();
        if let Some(rep) = container.bitmapImageRepForCachingDisplayInRect(bounds) {
            container.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
            RgbaImage::from_bitmap_rep(&rep).blend_over_solid(DARK_MENU_BG)
        } else {
            RgbaImage::new_solid(
                (width_pt * 2.0).round() as usize,
                (row_h * 2.0).round() as usize,
                DARK_MENU_BG,
            )
        }
    }

    fn vstack_strips(width_px: usize, strips: &[RgbaImage]) -> RgbaImage {
        let total_h: usize = strips.iter().map(|s| s.height).sum();
        let mut out = RgbaImage::new_solid(width_px, total_h.max(1), DARK_MENU_BG);
        let mut y_off = 0usize;
        for strip in strips {
            let copy_w = strip.width.min(width_px);
            for y in 0..strip.height {
                for x in 0..copy_w {
                    out.set_pixel(x, y_off + y, strip.pixel(x, y));
                }
            }
            y_off += strip.height;
        }
        out
    }
}
