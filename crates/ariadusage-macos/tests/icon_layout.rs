// Ported from CodexBar Sources/CodexBar/IconRenderer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/IconRendererHideCrittersTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/IconRendererStatusOverlayTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/IconRendererGrokTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatusItemQuotaWarningFlashTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexbarTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_macos::icon::{
    Bar, DEFAULT_QUOTA_WARNING_THRESHOLDS, IconInput, IconPalette, IconStyle,
    LOADING_PERCENT_EPSILON, LoadingPattern, QUOTA_WARNING_FLASH_DURATION_SECS,
    QuotaWarningThresholds, RectPt, RectPx, StatusLevel, StatusOverlay,
    effective_provider_tint_rgb, fill_width_pixels, layout, loading_blink_amount,
    meets_appearance_contrast, morph_layout, quota_warning_flash_active,
};

const TOP: RectPx = RectPx::new(3, 19, 30, 12);
const BOTTOM: RectPx = RectPx::new(3, 5, 30, 8);
const CREDITS: RectPx = RectPx::new(3, 14, 30, 16);
const CREDITS_BOTTOM: RectPx = RectPx::new(3, 4, 30, 6);

fn lane(rect: RectPx, percent: Option<f64>, alpha: f64) -> Bar {
    Bar {
        rect,
        percent,
        alpha,
        corner_radius_px: rect.h / 2,
    }
}

#[test]
fn fill_width_rounds_and_clamps() {
    assert_eq!(fill_width_pixels(0.0, 30), 0);
    assert_eq!(fill_width_pixels(100.0, 30), 30);
    assert_eq!(fill_width_pixels(150.0, 30), 30);
    assert_eq!(fill_width_pixels(-5.0, 30), 0);
    assert_eq!(fill_width_pixels(50.0, 30), 15);
    assert_eq!(fill_width_pixels(46.0, 30), 14);
    assert_eq!(fill_width_pixels(-1.0, 30), 0);
    assert_eq!(fill_width_pixels(120.0, 30), 30);
    // 30 × 0.1% rounds to zero: a visible-zero marker draws no fill.
    assert_eq!(fill_width_pixels(0.1, 30), 0);
    // 30 × 5% = 1.5 rounds away from zero, like Swift's `.rounded()`.
    assert_eq!(fill_width_pixels(5.0, 30), 2);
}

#[test]
fn two_windows_draw_top_and_bottom_lanes() {
    let out = layout(&IconInput {
        primary: Some(60.0),
        secondary: Some(40.0),
        ..IconInput::default()
    });
    assert_eq!(
        out.bars,
        vec![lane(TOP, Some(60.0), 1.0), lane(BOTTOM, Some(40.0), 1.0)]
    );
    assert_eq!(out.palette, IconPalette::for_stale(false));
    assert_eq!(out.overlay, None);
    assert!(out.critters.is_empty());
}

#[test]
fn single_primary_window_reads_as_one_meter() {
    let out = layout(&IconInput {
        primary: Some(46.0),
        ..IconInput::default()
    });
    assert_eq!(out.bars, vec![lane(CREDITS, Some(46.0), 1.0)]);
}

#[test]
fn claude_keeps_a_dimmed_lane_for_a_missing_weekly_window() {
    let out = layout(&IconInput {
        primary: Some(46.0),
        reserves_missing_secondary_lane: true,
        ..IconInput::default()
    });
    assert_eq!(
        out.bars,
        vec![lane(TOP, Some(46.0), 1.0), lane(BOTTOM, None, 0.45)]
    );
}

#[test]
fn secondary_only_quota_uses_the_prominent_meter() {
    let out = layout(&IconInput {
        secondary: Some(70.0),
        ..IconInput::default()
    });
    assert_eq!(out.bars, vec![lane(CREDITS, Some(70.0), 1.0)]);
}

#[test]
fn credits_only_shows_credits_with_a_dimmed_lane() {
    let out = layout(&IconInput {
        credits_remaining: Some(250.0),
        ..IconInput::default()
    });
    assert_eq!(
        out.bars,
        vec![
            lane(CREDITS, Some(25.0), 1.0),
            lane(CREDITS_BOTTOM, None, 0.45)
        ]
    );
}

#[test]
fn credits_are_capped_at_one_thousand() {
    let out = layout(&IconInput {
        credits_remaining: Some(5000.0),
        ..IconInput::default()
    });
    assert_eq!(out.bars[0].percent, Some(100.0));
}

#[test]
fn exhausted_weekly_shows_credits_on_top() {
    let out = layout(&IconInput {
        primary: Some(80.0),
        secondary: Some(0.0),
        credits_remaining: Some(500.0),
        ..IconInput::default()
    });
    assert_eq!(
        out.bars,
        vec![
            lane(CREDITS, Some(50.0), 1.0),
            lane(CREDITS_BOTTOM, Some(0.0), 1.0)
        ]
    );
}

#[test]
fn exhausted_weekly_without_credits_falls_back_to_the_session_lane() {
    let out = layout(&IconInput {
        primary: Some(80.0),
        secondary: Some(0.0),
        ..IconInput::default()
    });
    assert_eq!(
        out.bars,
        vec![
            lane(TOP, Some(80.0), 1.0),
            lane(CREDITS_BOTTOM, Some(0.0), 1.0)
        ]
    );
}

#[test]
fn no_data_draws_empty_lanes() {
    let out = layout(&IconInput::default());
    assert_eq!(
        out.bars,
        vec![lane(TOP, None, 1.0), lane(BOTTOM, None, 0.45)]
    );
    assert!(out.bars.iter().all(|b| b.fill_width_px() == 0));
}

#[test]
fn stale_data_dims_every_alpha() {
    let out = layout(&IconInput {
        primary: Some(50.0),
        secondary: Some(50.0),
        stale: true,
        ..IconInput::default()
    });
    assert_eq!(
        out.palette,
        IconPalette {
            track_fill_alpha: 0.18,
            track_stroke_alpha: 0.28,
            fill_alpha: 1.0,
        }
    );
    assert!(out.bars.iter().all(|b| b.alpha == 1.0));
}

#[test]
fn minor_status_dot_rises_onto_a_single_meter() {
    let two_lanes = layout(&IconInput {
        primary: Some(50.0),
        secondary: Some(50.0),
        status: StatusLevel::Minor,
        ..IconInput::default()
    });
    let one_lane = layout(&IconInput {
        primary: Some(50.0),
        status: StatusLevel::Maintenance,
        ..IconInput::default()
    });
    let dot_y = |overlay| match overlay {
        Some(StatusOverlay::Dot { dot, .. }) => dot.y,
        other => panic!("expected a dot, got {other:?}"),
    };
    assert_eq!(dot_y(two_lanes.overlay), 2.0);
    assert_eq!(dot_y(one_lane.overlay), 5.0);
}

#[test]
fn major_status_draws_an_exclamation_with_a_halo() {
    let out = layout(&IconInput {
        primary: Some(50.0),
        secondary: Some(50.0),
        status: StatusLevel::Major,
        ..IconInput::default()
    });
    assert_eq!(
        out.overlay,
        Some(StatusOverlay::Exclamation {
            line: RectPt {
                x: 12.0,
                y: 4.0,
                w: 2.0,
                h: 6.0
            },
            dot: RectPt {
                x: 12.0,
                y: 2.0,
                w: 2.0,
                h: 2.0
            },
            halo: RectPt {
                x: 11.0,
                y: 1.0,
                w: 4.0,
                h: 10.0
            },
        })
    );
}

#[test]
fn stroke_rect_insets_by_half_the_stroke() {
    assert_eq!(TOP.stroke_rect(), RectPx::new(4, 20, 28, 10));
}

#[test]
fn critter_styles_populate_expected_geometry_and_corner_radii() {
    let codex = layout(&IconInput {
        primary: Some(60.0),
        secondary: Some(40.0),
        style: IconStyle::Codex,
        ..IconInput::default()
    });
    assert_eq!(codex.bars[0].corner_radius_px, 6);
    let face = codex.critters.face.expect("codex face");
    assert_eq!(face.left_eye, RectPx::new(9, 23, 4, 4));
    assert_eq!(face.right_eye, RectPx::new(23, 23, 4, 4));
    assert_eq!(face.hat.rect, RectPx::new(9, 27, 18, 4));

    let claude = layout(&IconInput {
        primary: Some(60.0),
        secondary: Some(40.0),
        style: IconStyle::Claude,
        ..IconInput::default()
    });
    assert_eq!(claude.bars[0].corner_radius_px, 0);
    let notches = claude.critters.notches.expect("claude notches");
    assert_eq!(notches.left_arm, RectPx::new(0, 22, 3, 6));
    assert_eq!(notches.right_arm, RectPx::new(33, 22, 3, 6));
    assert_eq!(notches.legs[0], RectPx::new(8, 16, 2, 3));
    assert_eq!(notches.left_eye, RectPx::new(11, 24, 2, 5));
    assert_eq!(notches.right_eye, RectPx::new(23, 24, 2, 5));

    let antigravity = layout(&IconInput {
        primary: Some(60.0),
        secondary: Some(40.0),
        style: IconStyle::Antigravity,
        ..IconInput::default()
    });
    assert!(antigravity.critters.gemini.is_some());
    assert_eq!(
        antigravity.critters.antigravity_dot,
        Some(RectPx::new(34, 28, 3, 3))
    );

    let factory = layout(&IconInput {
        primary: Some(60.0),
        secondary: Some(40.0),
        style: IconStyle::Factory,
        ..IconInput::default()
    });
    let fac = factory.critters.factory.expect("factory");
    assert_eq!(fac.teeth.len(), 4);
    assert_eq!(fac.teeth[0], RectPx::new(12, 31, 3, 2));

    let warp = layout(&IconInput {
        primary: Some(60.0),
        secondary: Some(40.0),
        style: IconStyle::Warp,
        ..IconInput::default()
    });
    assert_eq!(warp.bars[0].corner_radius_px, 3);
    assert!(warp.critters.warp.is_some());

    let grok = layout(&IconInput {
        primary: Some(60.0),
        secondary: Some(40.0),
        style: IconStyle::Grok,
        ..IconInput::default()
    });
    let gr = grok.critters.grok.expect("grok");
    assert_eq!(gr.left_horn, RectPx::new(8, 31, 4, 3));
    assert_eq!(gr.right_horn, RectPx::new(24, 31, 4, 3));
    assert_eq!(gr.visor, RectPx::new(11, 24, 14, 3));
}

#[test]
fn loading_patterns_and_unbraid_fallback_match_upstream() {
    assert_eq!(LoadingPattern::ALL.len(), 6);
    assert_eq!(LoadingPattern::KnightRider.value(0.0), 50.0);
    assert_eq!(LoadingPattern::OutsideIn.value(0.0), 100.0);
    assert_eq!(LoadingPattern::Pulse.value(0.0), 70.0);
    assert_eq!(
        LoadingPattern::Unbraid.effective_for(IconStyle::Combined, true, "codex"),
        LoadingPattern::Cylon
    );
    assert_eq!(
        LoadingPattern::Unbraid.effective_for(IconStyle::Claude, false, "claude"),
        LoadingPattern::Cylon
    );
    assert_eq!(
        LoadingPattern::Unbraid.effective_for(IconStyle::Codex, false, "codex"),
        LoadingPattern::Unbraid
    );
    assert_eq!(LOADING_PERCENT_EPSILON, 0.0001);
    assert!(loading_blink_amount(IconStyle::Warp, Some(0.5), 0.0) > 0.9);
    assert_eq!(
        loading_blink_amount(IconStyle::Codex, Some(0.5), 0.25),
        0.25
    );
}

#[test]
fn quota_warning_thresholds_and_flash_duration_match_upstream() {
    assert_eq!(DEFAULT_QUOTA_WARNING_THRESHOLDS, [50, 20]);
    assert_eq!(QUOTA_WARNING_FLASH_DURATION_SECS, 60.0);
    assert_eq!(
        QuotaWarningThresholds::sanitized(&[20, 50, 20, 0, 150]),
        vec![99, 50, 20, 1]
    );
    assert_eq!(QuotaWarningThresholds::active(None), vec![50, 20]);
    assert_eq!(QuotaWarningThresholds::active(Some(&[])), vec![50, 20]);
    assert_eq!(
        QuotaWarningThresholds::resolved(Some(&[15, 40]), Some(&[50, 20])),
        vec![40, 15]
    );
    assert!(quota_warning_flash_active(Some(1000.0), 1059.0));
    assert!(!quota_warning_flash_active(Some(1000.0), 1061.0));
}

#[test]
fn provider_tint_contrast_checks_match_upstream() {
    // Bright white meets dark contrast, fails light contrast
    assert!(meets_appearance_contrast(
        1.0,
        1.0,
        1.0,
        "NSAppearanceNameDarkAqua"
    ));
    assert!(!meets_appearance_contrast(
        1.0,
        1.0,
        1.0,
        "NSAppearanceNameAqua"
    ));
    // Deep black meets light contrast, fails dark contrast
    assert!(meets_appearance_contrast(
        0.0,
        0.0,
        0.0,
        "NSAppearanceNameAqua"
    ));
    assert!(!meets_appearance_contrast(
        0.0,
        0.0,
        0.0,
        "NSAppearanceNameDarkAqua"
    ));

    // Highlighted, stale, high-contrast, or accessibility appearances suppress tint
    assert_eq!(
        effective_provider_tint_rgb(
            (255, 255, 255),
            true,
            true,
            false,
            false,
            "NSAppearanceNameDarkAqua"
        ),
        None
    );
    assert_eq!(
        effective_provider_tint_rgb(
            (255, 255, 255),
            true,
            false,
            true,
            false,
            "NSAppearanceNameDarkAqua"
        ),
        None
    );
    assert_eq!(
        effective_provider_tint_rgb(
            (255, 255, 255),
            true,
            false,
            false,
            false,
            "NSAppearanceNameAccessibilityHighContrastDarkAqua"
        ),
        None
    );
    assert_eq!(
        effective_provider_tint_rgb(
            (255, 255, 255),
            true,
            false,
            false,
            false,
            "NSAppearanceNameDarkAqua"
        ),
        Some((255, 255, 255))
    );
}

#[cfg(target_os = "macos")]
mod macos_pixel_tests {
    use super::*;
    use ariadusage_macos::appkit::{
        image_png_bytes, quota_warning_flash_image, render_icon, render_morph_icon,
        render_png_for_appearance,
    };
    use objc2::rc::Retained;
    use objc2_app_kit::{NSBitmapImageRep, NSImage};
    use std::collections::VecDeque;

    fn bitmap_rep(image: &NSImage) -> Retained<NSBitmapImageRep> {
        let reps = image.representations();
        for i in 0..reps.count() {
            let rep = reps.objectAtIndex(i);
            if let Ok(bm) = rep.downcast::<NSBitmapImageRep>()
                && bm.pixelsWide() == 36
                && bm.pixelsHigh() == 36
            {
                return bm;
            }
        }
        panic!("expected 36x36 NSBitmapImageRep");
    }

    fn alpha_at(rep: &NSBitmapImageRep, x: isize, y: isize) -> f64 {
        rep.colorAtX_y(x, y)
            .map(|c| c.alphaComponent())
            .unwrap_or(0.0)
    }

    fn render_style(style: IconStyle, weekly: Option<f64>, hide_critters: bool) -> Vec<u8> {
        let policy = style.quota_layout_policy();
        let l = layout(&IconInput {
            primary: Some(60.0),
            secondary: weekly,
            style,
            hide_critters,
            reserves_missing_secondary_lane: policy.reserves_missing_secondary_lane,
            treats_exhausted_secondary_as_missing: policy.treats_exhausted_secondary_as_missing,
            ..IconInput::default()
        });
        let img = render_icon(&l).expect("render_icon");
        assert!(img.isTemplate());
        image_png_bytes(&img).expect("png")
    }

    #[test]
    fn hiding_critters_removes_every_decorated_style_twist_and_matches_plain_bars() {
        let styles = [
            IconStyle::Codex,
            IconStyle::Claude,
            IconStyle::Gemini,
            IconStyle::Antigravity,
            IconStyle::Factory,
            IconStyle::Warp,
            IconStyle::Grok,
        ];
        let reference = render_style(IconStyle::Cursor, Some(40.0), true);
        for style in styles {
            let decorated = render_style(style, Some(40.0), false);
            let hidden = render_style(style, Some(40.0), true);
            assert_ne!(
                decorated, hidden,
                "style {style:?} should differ when critters are shown"
            );
            assert_eq!(
                hidden, reference,
                "style {style:?} with hide_critters should match plain bars"
            );
        }
        // Cursor is undecorated so hide_critters is a no-op
        assert_eq!(
            render_style(IconStyle::Cursor, Some(40.0), false),
            reference
        );
    }

    #[test]
    fn grok_visor_decorates_either_single_quota_and_respects_hide_critters() {
        for primary_only in [true, false] {
            let render = |hidden: bool, style: IconStyle| {
                let l = layout(&IconInput {
                    primary: primary_only.then_some(60.0),
                    secondary: (!primary_only).then_some(60.0),
                    style,
                    hide_critters: hidden,
                    ..IconInput::default()
                });
                image_png_bytes(&render_icon(&l).unwrap()).unwrap()
            };
            assert_ne!(
                render(false, IconStyle::Grok),
                render(true, IconStyle::Grok)
            );
            assert_eq!(
                render(true, IconStyle::Grok),
                render(true, IconStyle::Cursor)
            );
        }
    }

    #[test]
    fn hiding_critters_removes_warp_eyes_without_weekly_quota() {
        let decorated = render_style(IconStyle::Warp, None, false);
        let plain = render_style(IconStyle::Warp, None, true);
        assert_ne!(decorated, plain);
    }

    #[test]
    fn morph_icon_honors_hide_critters_at_full_progress() {
        let decorated = render_morph_icon(&morph_layout(1.0, IconStyle::Codex, false)).unwrap();
        let plain = render_morph_icon(&morph_layout(1.0, IconStyle::Codex, true)).unwrap();
        assert_ne!(image_png_bytes(&decorated), image_png_bytes(&plain));
    }

    #[test]
    fn status_badge_attaches_to_either_prominent_single_quota_meter() {
        for secondary_only in [false, true] {
            for status in [
                StatusLevel::Minor,
                StatusLevel::Maintenance,
                StatusLevel::Major,
            ] {
                let make = |st: StatusLevel| {
                    let l = layout(&IconInput {
                        primary: (!secondary_only).then_some(100.0),
                        secondary: secondary_only.then_some(100.0),
                        status: st,
                        hide_critters: true,
                        ..IconInput::default()
                    });
                    bitmap_rep(&render_icon(&l).unwrap())
                };
                let plain_rep = make(StatusLevel::None);
                let marked_rep = make(status);
                let mut cutout_pixels = 0;
                let mut glyph_pixels = 0;
                for y in 0..36 {
                    for x in 0..36 {
                        let pa = alpha_at(&plain_rep, x, y);
                        let ma = alpha_at(&marked_rep, x, y);
                        if pa > 0.5 && ma < 0.05 {
                            cutout_pixels += 1;
                        }
                        if pa < 0.05 && ma > 0.5 {
                            glyph_pixels += 1;
                        }
                    }
                }
                assert!(
                    cutout_pixels >= 8,
                    "expected >= 8 halo cutout pixels for {status:?} (secondary_only={secondary_only}), got {cutout_pixels}"
                );
                assert!(
                    glyph_pixels >= 4,
                    "expected >= 4 glyph pixels for {status:?} (secondary_only={secondary_only}), got {glyph_pixels}"
                );
            }
        }
    }

    #[test]
    fn grok_visor_and_antennae_use_expected_grid_positions() {
        let l = layout(&IconInput {
            primary: Some(50.0),
            secondary: Some(50.0),
            style: IconStyle::Grok,
            ..IconInput::default()
        });
        let rep = bitmap_rep(&render_icon(&l).unwrap());
        let max_either = |x: isize, y: isize| alpha_at(&rep, x, y).max(alpha_at(&rep, x, 35 - y));
        let min_either = |x: isize, y: isize| alpha_at(&rep, x, y).min(alpha_at(&rep, x, 35 - y));
        assert!(max_either(10, 32) > 0.9);
        assert!(max_either(26, 32) > 0.9);
        assert!(min_either(18, 25) < 0.05);
    }

    #[test]
    fn codex_eyes_punch_through_when_primary_unknown() {
        let l = layout(&IconInput {
            primary: None,
            secondary: Some(1.0),
            style: IconStyle::Codex,
            ..IconInput::default()
        });
        let rep = bitmap_rep(&render_icon(&l).unwrap());
        let is_transparent = |x: isize, y: isize| alpha_at(&rep, x, y) < 0.05;
        let mut visited = [[false; 36]; 36];
        let mut queue = VecDeque::new();
        let enqueue = |x: isize,
                       y: isize,
                       visited: &mut [[bool; 36]; 36],
                       q: &mut VecDeque<(isize, isize)>| {
            if (0..36).contains(&x) && (0..36).contains(&y) {
                let (ux, uy) = (x as usize, y as usize);
                if !visited[uy][ux] && is_transparent(x, y) {
                    visited[uy][ux] = true;
                    q.push_back((x, y));
                }
            }
        };
        for i in 0..36 {
            enqueue(i, 0, &mut visited, &mut queue);
            enqueue(i, 35, &mut visited, &mut queue);
            enqueue(0, i, &mut visited, &mut queue);
            enqueue(35, i, &mut visited, &mut queue);
        }
        while let Some((x, y)) = queue.pop_front() {
            enqueue(x + 1, y, &mut visited, &mut queue);
            enqueue(x - 1, y, &mut visited, &mut queue);
            enqueue(x, y + 1, &mut visited, &mut queue);
            enqueue(x, y - 1, &mut visited, &mut queue);
        }
        let mut internal_holes = 0;
        for y in 0..36 {
            for x in 0..36 {
                if is_transparent(x, y) && !visited[y as usize][x as usize] {
                    internal_holes += 1;
                }
            }
        }
        assert!(
            internal_holes >= 16,
            "expected >= 16 internal hole pixels, got {internal_holes}"
        );
    }

    #[test]
    fn warp_eyes_cut_out_at_expected_centers() {
        let l = layout(&IconInput {
            primary: Some(50.0),
            secondary: Some(50.0),
            style: IconStyle::Warp,
            ..IconInput::default()
        });
        let rep = bitmap_rep(&render_icon(&l).unwrap());
        let min_near = |cx: isize, cy: isize, r: isize| {
            let mut m = 1.0_f64;
            for y in (cy - r).max(0)..=(cy + r).min(35) {
                for x in (cx - r).max(0)..=(cx + r).min(35) {
                    m = m.min(alpha_at(&rep, x, y));
                }
            }
            m
        };
        let min_either =
            |cx: isize, cy: isize, r: isize| min_near(cx, cy, r).min(min_near(cx, 35 - cy, r));
        assert!(min_either(11, 25, 2) < 0.05);
        assert!(min_either(25, 25, 2) < 0.05);
        let mid_alpha = alpha_at(&rep, 18, 25).max(alpha_at(&rep, 18, 35 - 25));
        assert!(mid_alpha > 0.05);
    }

    #[test]
    fn quota_warning_flash_image_draws_non_template_red_overlay() {
        let base_layout = layout(&IconInput {
            primary: Some(100.0),
            secondary: Some(100.0),
            ..IconInput::default()
        });
        let base = render_icon(&base_layout).unwrap();
        assert!(base.isTemplate());
        let flashed = quota_warning_flash_image(&base).unwrap();
        assert!(!flashed.isTemplate());
        let rep = bitmap_rep(&flashed);
        let center = rep.colorAtX_y(18, 18).unwrap();
        assert!(center.redComponent() > center.blueComponent());
    }

    #[test]
    fn export_icon_captures_when_env_set() {
        let Ok(dir) = std::env::var("ARIADUSAGE_ICON_CAPTURES_DIR") else {
            return;
        };
        let out_dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&out_dir).expect("create captures dir");

        let styles = [
            ("bars", IconStyle::Combined, true),
            ("codex", IconStyle::Codex, false),
            ("claude", IconStyle::Claude, false),
            ("gemini", IconStyle::Gemini, false),
            ("antigravity", IconStyle::Antigravity, false),
            ("cursor", IconStyle::Cursor, false),
            ("factory", IconStyle::Factory, false),
            ("grok", IconStyle::Grok, false),
            ("copilot", IconStyle::Copilot, false),
            ("warp", IconStyle::Warp, false),
        ];

        for (name, style, hide_critters) in styles {
            let policy = style.quota_layout_policy();
            let l = layout(&IconInput {
                primary: Some(72.0),
                secondary: Some(45.0),
                style,
                hide_critters,
                reserves_missing_secondary_lane: policy.reserves_missing_secondary_lane,
                treats_exhausted_secondary_as_missing: policy.treats_exhausted_secondary_as_missing,
                ..IconInput::default()
            });
            for (mode, dark, highlighted, flash) in [
                ("dark", true, false, false),
                ("light", false, false, false),
                ("highlighted", true, true, false),
                ("warning-flash", true, false, true),
            ] {
                let png = render_png_for_appearance(&l, dark, highlighted, None, flash)
                    .expect("render png");
                std::fs::write(out_dir.join(format!("{name}-{mode}.png")), png).expect("write png");
            }
        }

        // Also export stale, minor overlay, major overlay, and morph frames
        let stale_layout = layout(&IconInput {
            primary: Some(58.0),
            secondary: Some(40.0),
            stale: true,
            style: IconStyle::Claude,
            ..IconInput::default()
        });
        std::fs::write(
            out_dir.join("claude-stale-dark.png"),
            render_png_for_appearance(&stale_layout, true, false, None, false).unwrap(),
        )
        .unwrap();

        let minor_layout = layout(&IconInput {
            primary: Some(72.0),
            secondary: Some(45.0),
            status: StatusLevel::Minor,
            style: IconStyle::Codex,
            ..IconInput::default()
        });
        std::fs::write(
            out_dir.join("codex-minor-dark.png"),
            render_png_for_appearance(&minor_layout, true, false, None, false).unwrap(),
        )
        .unwrap();

        let major_layout = layout(&IconInput {
            primary: Some(72.0),
            secondary: Some(45.0),
            status: StatusLevel::Major,
            style: IconStyle::Codex,
            ..IconInput::default()
        });
        std::fs::write(
            out_dir.join("codex-major-dark.png"),
            render_png_for_appearance(&major_layout, true, false, None, false).unwrap(),
        )
        .unwrap();

        let morph = render_morph_icon(&morph_layout(0.35, IconStyle::Codex, false)).unwrap();
        std::fs::write(
            out_dir.join("codex-morph-35.png"),
            image_png_bytes(&morph).unwrap(),
        )
        .unwrap();
    }
}
