// Ported from CodexBar Sources/CodexBar/IconRenderer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/IconRendererHideCrittersTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/IconRendererStatusOverlayTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/IconRendererGrokTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatusItemAnimationCapTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatusItemQuotaWarningFlashTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatusItemBlinkGatingTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatusItemAnimationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatusItemAnimationSignatureTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/QuotaWarningNotificationLogicTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexbarTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};

use ariadusage_macos::icon::{
    BLINK_ACTIVE_TICK_INTERVAL_MS, BLINK_DURATION_SECS, BLINK_IDLE_FALLBACK_INTERVAL_MS, Bar,
    BlinkState, DEFAULT_QUOTA_WARNING_THRESHOLDS, IconInput, IconPalette, IconStyle,
    LOADING_ANIMATION_FPS, LOADING_ANIMATION_MAX_CONTINUOUS_DURATION_SECS,
    LOADING_ANIMATION_PHASE_INCREMENT, LOADING_PERCENT_EPSILON, LoadingPattern, MotionEffect,
    QUOTA_WARNING_FLASH_DURATION_SECS, QuotaWarningNotificationLogic, QuotaWarningThresholds,
    QuotaWarningTracker, RectPt, RectPx, StatusItemAnimationState, StatusLevel, StatusOverlay,
    blink_tick_sleep_duration_ms, effective_provider_tint_rgb, fill_width_pixels,
    icon_render_signature, is_blinking_allowed, layout,
    loading_animation_has_exceeded_continuous_cap, loading_blink_amount, loading_frame_percents,
    meets_appearance_contrast, morph_layout, quota_warning_flash_active, select_motion_effect,
    should_run_blink_task, style_supports_blink,
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
        LoadingPattern::Unbraid.effective_for(IconStyle::Combined, true, Some("codex")),
        LoadingPattern::Cylon
    );
    assert_eq!(
        LoadingPattern::Unbraid.effective_for(IconStyle::Claude, false, Some("claude")),
        LoadingPattern::Cylon
    );
    assert_eq!(
        LoadingPattern::Unbraid.effective_for(IconStyle::Codex, false, Some("codex")),
        LoadingPattern::Unbraid
    );
    assert_eq!(
        LoadingPattern::Unbraid.effective_for(IconStyle::Codex, false, None),
        LoadingPattern::KnightRider
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
        vec![99, 50, 20, 0]
    );
    assert_eq!(
        QuotaWarningThresholds::active(Some(&[20, 50, 20, 0, 150])),
        vec![99, 50, 20]
    );
    assert_eq!(QuotaWarningThresholds::active(None), vec![50, 20]);
    assert_eq!(QuotaWarningThresholds::active(Some(&[])), vec![50, 20]);
    assert_eq!(
        QuotaWarningThresholds::resolved(Some(&[15, 40]), Some(&[50, 20])),
        vec![40, 15]
    );
    assert_eq!(
        QuotaWarningThresholds::resolved_pair(None, None),
        vec![50, 20]
    );
    assert!(quota_warning_flash_active(Some(1000.0), 1059.0));
    assert!(!quota_warning_flash_active(Some(1000.0), 1061.0));
}

#[test]
fn copilot_style_has_no_decorations_and_unshipped_aliases_are_removed() {
    assert!(!IconStyle::Copilot.decorations().face);
    assert_eq!(IconStyle::from_provider_id("copilot"), IconStyle::Copilot);
    assert_eq!(IconStyle::from_provider_id("kiro"), IconStyle::Combined);
    assert_eq!(IconStyle::from_provider_id("droid"), IconStyle::Combined);
}

#[test]
fn loading_animation_cap_expires_only_after_thirty_seconds() {
    let now = 2_000_000_000.0;
    assert_eq!(LOADING_ANIMATION_MAX_CONTINUOUS_DURATION_SECS, 30.0);
    assert!(!loading_animation_has_exceeded_continuous_cap(
        Some(now - 30.0),
        now,
    ));
    assert!(loading_animation_has_exceeded_continuous_cap(
        Some(now - 30.001),
        now,
    ));
}

#[test]
fn capped_animation_marker_does_not_qualify_as_a_fresh_cap() {
    let now = 2_000_000_000.0;
    assert!(!loading_animation_has_exceeded_continuous_cap(None, now));
    let mut state = StatusItemAnimationState::default();
    assert!(state.sync_loading_animation(true, now));
    assert!(!state.sync_loading_animation(true, now + 30.001));
    assert!(state.loading_capped);
    assert!(!state.sync_loading_animation(true, now + 31.0));
}

#[test]
fn quota_warning_notification_logic_crossing_recovery_and_zero_threshold() {
    // does nothing without crossing
    assert_eq!(
        QuotaWarningNotificationLogic::crossed_threshold(
            Some(60.0),
            55.0,
            &[50, 20],
            &BTreeSet::new()
        ),
        None
    );

    // detects downward crossing
    assert_eq!(
        QuotaWarningNotificationLogic::crossed_threshold(
            Some(55.0),
            45.0,
            &[50, 20],
            &BTreeSet::new()
        ),
        Some(50)
    );

    // skips already fired thresholds
    let already_50: BTreeSet<i32> = [50].into_iter().collect();
    assert_eq!(
        QuotaWarningNotificationLogic::crossed_threshold(Some(55.0), 45.0, &[50, 20], &already_50),
        None
    );

    // chooses most severe threshold when crossing several at once
    assert_eq!(
        QuotaWarningNotificationLogic::crossed_threshold(
            Some(80.0),
            10.0,
            &[50, 20],
            &BTreeSet::new()
        ),
        Some(20)
    );

    // startup below threshold warns once at most severe threshold
    assert_eq!(
        QuotaWarningNotificationLogic::crossed_threshold(None, 10.0, &[50, 20], &BTreeSet::new()),
        Some(20)
    );

    // warning marks threshold and higher thresholds fired
    let fired = QuotaWarningNotificationLogic::fired_thresholds_after_warning(20, &[50, 20]);
    assert_eq!(fired, [50, 20].into_iter().collect::<BTreeSet<i32>>());

    // recovery clears only thresholds below current remaining
    let cleared = QuotaWarningNotificationLogic::thresholds_to_clear(30.0, &fired);
    assert_eq!(cleared, [20].into_iter().collect::<BTreeSet<i32>>());

    // zero threshold does not post quota warning
    let already_10: BTreeSet<i32> = [10].into_iter().collect();
    assert_eq!(
        QuotaWarningNotificationLogic::crossed_threshold(Some(10.0), 0.0, &[10, 0], &already_10),
        None
    );
    assert_eq!(
        QuotaWarningNotificationLogic::fired_thresholds_after_warning(10, &[10, 0]),
        already_10
    );

    // QuotaWarningTracker clears fired state on reset timestamp change and recovery
    let mut tracker = QuotaWarningTracker::default();
    assert!(tracker.update_provider(
        "codex",
        Some(45.0),
        Some(1_000),
        None,
        None,
        &DEFAULT_QUOTA_WARNING_THRESHOLDS,
        true
    ));
    assert!(!tracker.update_provider(
        "codex",
        Some(40.0),
        Some(1_000),
        None,
        None,
        &DEFAULT_QUOTA_WARNING_THRESHOLDS,
        true
    ));
    assert!(tracker.update_provider(
        "codex",
        Some(15.0),
        Some(1_000),
        None,
        None,
        &DEFAULT_QUOTA_WARNING_THRESHOLDS,
        true
    ));
    // Reset timestamp changes -> clears fired thresholds, warns again at 45%
    assert!(tracker.update_provider(
        "codex",
        Some(45.0),
        Some(2_000),
        None,
        None,
        &DEFAULT_QUOTA_WARNING_THRESHOLDS,
        true
    ));
}

#[test]
fn status_item_blink_gating_and_animation_state_lifecycle() {
    assert_eq!(LOADING_ANIMATION_FPS, 30.0);
    assert!((LOADING_ANIMATION_PHASE_INCREMENT - 2.7 / 30.0).abs() < 1e-12);
    assert_eq!(BLINK_ACTIVE_TICK_INTERVAL_MS, 75);
    assert_eq!(BLINK_IDLE_FALLBACK_INTERVAL_MS, 1000);
    assert!((BLINK_DURATION_SECS - 0.36).abs() < 1e-9);

    assert!(style_supports_blink(IconStyle::Codex, false));
    assert!(!style_supports_blink(IconStyle::Codex, true));
    assert!(style_supports_blink(IconStyle::Claude, false));
    assert!(style_supports_blink(IconStyle::Warp, false));
    assert!(!style_supports_blink(IconStyle::Combined, false));
    assert!(!style_supports_blink(IconStyle::Copilot, false));

    assert_eq!(
        select_motion_effect(IconStyle::Claude, 0.10),
        MotionEffect::Wiggle
    );
    assert_eq!(
        select_motion_effect(IconStyle::Claude, 0.50),
        MotionEffect::Blink
    );
    assert_eq!(
        select_motion_effect(IconStyle::Codex, 0.10),
        MotionEffect::Tilt
    );
    assert_eq!(
        select_motion_effect(IconStyle::Codex, 0.50),
        MotionEffect::Blink
    );

    let now = 2_000_000_000.0;
    assert!(is_blinking_allowed(true, None, now));
    assert!(!is_blinking_allowed(false, None, now));
    assert!(is_blinking_allowed(false, Some(now + 0.5), now));

    assert!(should_run_blink_task(true, false, false, true));
    assert!(!should_run_blink_task(true, true, false, true));
    assert!(!should_run_blink_task(true, false, true, true));
    assert!(!should_run_blink_task(true, false, false, false));

    let mut state = StatusItemAnimationState::default();
    let providers = vec!["codex".to_string()];

    // Empty blink states -> sleep duration is 75ms to initialize
    assert_eq!(
        blink_tick_sleep_duration_ms(&state.blink_states, &providers, now),
        75
    );

    // Seed blink state -> sleep duration is >= 3000ms (initial delay in 3.0..=12.0s)
    state
        .blink_states
        .insert("codex".to_string(), BlinkState::seeded(now, 1));
    assert!(blink_tick_sleep_duration_ms(&state.blink_states, &providers, now) >= 3000);

    // Force blink now -> sleep duration becomes 75ms (active frame cadence)
    let forced = state.force_blink_now(&providers, false, now);
    assert_eq!(forced, providers);
    assert_eq!(
        blink_tick_sleep_duration_ms(&state.blink_states, &providers, now),
        75
    );

    // Force blink when hide_critters == true is a no-op
    assert!(state.force_blink_now(&providers, true, now).is_empty());

    // Tick at midpoint of blink (now + 0.18s) -> motion amount > 0.9
    let changed = state.tick_blink(&providers, true, false, now + 0.18);
    assert_eq!(changed, providers);
    let (blink, wiggle, tilt) = state.motion_for("codex");
    assert!(blink > 0.9 || wiggle > 0.9 || tilt.abs() > 0.1);
    assert_eq!(state.motion_for("claude"), (0.0, 0.0, 0.0));

    // Tick after blink duration (now + 0.40s) -> blink finishes and resets to 0
    let _ = state.tick_blink(&providers, true, false, now + 0.40);
    assert_eq!(state.motion_for("codex"), (0.0, 0.0, 0.0));

    let mut custom_states = BTreeMap::new();
    custom_states.insert(
        "codex".to_string(),
        BlinkState {
            next_blink_secs: now + 0.25,
            blink_start_secs: None,
            pending_second_start_secs: None,
            effect: MotionEffect::Blink,
        },
    );
    assert_eq!(
        blink_tick_sleep_duration_ms(&custom_states, &providers, now),
        250
    );

    // Loading animation lifecycle & 30s cap
    assert!(state.sync_loading_animation(true, now));
    assert_eq!(state.active_loading_phase(true), Some(0.0));
    assert!(state.advance_loading_frame(true, now + 0.033));
    assert!(state.active_loading_phase(true).unwrap() > 0.0);

    // Exceeding 30s continuous cap stops loading animation until loading clears
    assert!(!state.sync_loading_animation(true, now + 30.5));
    assert_eq!(state.active_loading_phase(true), None);
    assert!(!state.sync_loading_animation(true, now + 31.0));
    assert!(!state.sync_loading_animation(false, now + 32.0));
    // Fresh loading run after clearing starts again
    assert!(state.sync_loading_animation(true, now + 33.0));
    assert_eq!(state.active_loading_phase(true), Some(0.0));

    // Quota warning flash lifecycle + signature
    let until = state.start_quota_warning_flash("codex", now);
    assert_eq!(until, now + QUOTA_WARNING_FLASH_DURATION_SECS);
    assert!(state.is_quota_warning_flash_active("codex", now + 59.0));
    assert!(!state.is_quota_warning_flash_active("codex", now + 61.0));

    let input = IconInput {
        primary: Some(50.0),
        ..IconInput::default()
    };
    let sig_no_flash = icon_render_signature(
        &input,
        false,
        false,
        None,
        LoadingPattern::KnightRider,
        false,
    );
    let sig_flash = icon_render_signature(
        &input,
        false,
        false,
        None,
        LoadingPattern::KnightRider,
        true,
    );
    assert_ne!(sig_no_flash, sig_flash);
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

#[test]
fn morph_layout_interpolates_ribbons_and_critters() {
    let start = morph_layout(0.0, IconStyle::Codex, false);
    assert_eq!(start.ribbons.len(), 3);
    assert!(start.crossfade_bars.is_none());
    assert!((start.ribbons[0].angle_deg - (-30.0)).abs() < 1e-6);

    let end = morph_layout(1.0, IconStyle::Codex, false);
    assert_eq!(end.ribbons.len(), 3);
    let (end_layout, end_alpha) = end.crossfade_bars.expect("crossfade bars at t=1.0");
    assert!((end_alpha - 1.0).abs() < 1e-6);
    assert!(end.ribbons[0].angle_deg.abs() < 1e-6);
    assert!(end.ribbons[1].angle_deg.abs() < 1e-6);
    assert!(end.ribbons[2].alpha <= 0.01);
    assert!(end_layout.critters.face.is_some());

    let end_bars = morph_layout(1.0, IconStyle::Codex, true);
    let (end_bars_layout, _) = end_bars.crossfade_bars.expect("crossfade bars at t=1.0");
    assert!(end_bars_layout.critters.is_empty());
}

// --- Ported from StatusItemAnimationTests.swift, StatusItemQuotaWarningFlashTests.swift,
//     StatusItemAnimationCapTests.swift, StatusItemBlinkGatingTests.swift, and
//     StatusItemAnimationSignatureTests.swift ---

#[test]
fn known_unavailable_limits_stop_loading_animation() {
    use ariadusage_macos::presentation::should_animate_provider;
    use ariadusage_protocol::ids::ProviderId;
    use ariadusage_protocol::snapshot::ProviderSnapshot;

    // Fresh enabled provider with no updated_at, no measured windows, and no error animates:
    let mut provider =
        ProviderSnapshot::new(ProviderId::new("cursor").unwrap(), "Cursor", true, "web");
    assert!(should_animate_provider(&provider, false));

    // Once updated_at is populated (even with primary=None and secondary=None, as for an empty/unavailable plan),
    // `should_animate_provider` returns false and stops the loading animation:
    provider.updated_at = Some("2026-04-06T12:00:00Z".parse().unwrap());
    assert!(!should_animate_provider(&provider, false));

    let mut state = StatusItemAnimationState::default();
    let now = 2_000_000_000.0;
    assert!(state.sync_loading_animation(true, now));
    assert_eq!(state.active_loading_phase(true), Some(0.0));
    assert!(!state.sync_loading_animation(should_animate_provider(&provider, false), now + 1.0));
    assert_eq!(state.active_loading_phase(false), None);
}

#[test]
fn merged_icon_loading_animation_tracks_selected_provider_only() {
    use ariadusage_macos::presentation::{
        any_enabled_provider_needs_loading_animation, needs_menu_bar_icon_animation,
    };
    use ariadusage_protocol::ids::ProviderId;
    use ariadusage_protocol::metric::{Confidence, Metric};
    use ariadusage_protocol::snapshot::{EngineInfo, EngineSnapshot, ProviderSnapshot};
    use ariadusage_protocol::usage::RateWindow;

    let mut codex = ProviderSnapshot::new(ProviderId::new("codex").unwrap(), "Codex", true, "cli");
    codex.windows.primary = Some(Metric::value(
        RateWindow::new(50.0),
        None,
        None,
        Confidence::Exact,
    ));
    codex.updated_at = Some("2026-04-06T12:00:00Z".parse().unwrap());

    // Claude is enabled and has no data / no error -> loading:
    let claude = ProviderSnapshot::new(ProviderId::new("claude").unwrap(), "Claude", true, "cli");

    let snapshot = EngineSnapshot::new(
        "2026-04-06T12:00:00Z".parse().unwrap(),
        900,
        EngineInfo {
            version: "0.1.0".to_string(),
            refreshing: false,
        },
        vec![codex, claude],
    );

    // Selected provider is Codex (already loaded), while Claude is still loading:
    // Merged mode tracks the selected provider (`needs_menu_bar_icon_animation` == false for Codex),
    // whereas split mode animates if any enabled provider is loading (`any_enabled_provider_needs_loading_animation` == true).
    assert!(!needs_menu_bar_icon_animation(
        &snapshot,
        Some("codex"),
        true,
        false,
    ));
    assert!(needs_menu_bar_icon_animation(
        &snapshot,
        Some("claude"),
        true,
        false,
    ));
    assert!(any_enabled_provider_needs_loading_animation(
        &snapshot, false
    ));
    assert!(needs_menu_bar_icon_animation(
        &snapshot,
        Some("codex"),
        false,
        false,
    ));

    // When switching selection in merged mode from Claude (loading) to Codex (loaded) while Claude is still loading,
    // the timer stops (`sync_loading_animation_with_any_loading` returns false).
    let mut anim = StatusItemAnimationState::default();
    let now = 2_000_000_000.0;
    assert!(anim.sync_loading_animation_with_any_loading(true, true, now));
    assert!(!anim.sync_loading_animation_with_any_loading(false, true, now + 0.5));
}

#[test]
fn merged_icon_loading_animation_does_not_flip_layout_when_weekly_hits_zero() {
    // At phase = pi / 2, KnightRider secondary raw value is `50 + 50 * sin(pi / 2 + pi) = 0.0`.
    // Without `LOADING_PERCENT_EPSILON` clamping, `secondary == Some(0.0)` triggers the weekly-exhausted
    // layout (`CREDITS_RECT` + `CREDITS_BOTTOM_RECT`) instead of normal `TOP_RECT` + `BOTTOM_RECT`.
    let phase = std::f64::consts::FRAC_PI_2;
    let raw_secondary =
        LoadingPattern::KnightRider.value(phase + LoadingPattern::KnightRider.secondary_offset());
    assert!(raw_secondary.abs() < 1e-9);

    let (primary, secondary) = loading_frame_percents(LoadingPattern::KnightRider, phase);
    assert!((primary - 100.0).abs() < 1e-9);
    assert_eq!(secondary, LOADING_PERCENT_EPSILON);

    let frame_layout = layout(&IconInput {
        primary: Some(primary),
        secondary: Some(secondary),
        style: IconStyle::Combined,
        ..IconInput::default()
    });
    assert_eq!(frame_layout.bars.len(), 2);
    assert_eq!(frame_layout.bars[0].rect, TOP);
    assert_eq!(frame_layout.bars[1].rect, BOTTOM);
    // And fill width of `LOADING_PERCENT_EPSILON` (0.0001%) on 30px is 0px:
    assert_eq!(
        fill_width_pixels(frame_layout.bars[1].percent.unwrap(), 30),
        0
    );
}

#[test]
fn warp_no_bonus_layout_is_preserved_in_show_used_mode_when_bonus_is_exhausted() {
    use ariadusage_macos::presentation::resolved_icon_percents_for_style;
    use ariadusage_protocol::ids::ProviderId;
    use ariadusage_protocol::metric::{Confidence, Metric};
    use ariadusage_protocol::snapshot::ProviderSnapshot;
    use ariadusage_protocol::usage::RateWindow;

    let mut warp = ProviderSnapshot::new(ProviderId::new("warp").unwrap(), "Warp", true, "web");
    warp.windows.primary = Some(Metric::value(
        RateWindow::new(25.0),
        None,
        None,
        Confidence::Exact,
    ));
    warp.windows.secondary = Some(Metric::value(
        RateWindow::new(100.0),
        None,
        None,
        Confidence::Exact,
    ));

    let (p_used, s_used) =
        resolved_icon_percents_for_style(&warp, IconStyle::Warp, true, None, None);
    let policy = IconStyle::Warp.quota_layout_policy();
    let used_layout = layout(&IconInput {
        primary: p_used,
        secondary: s_used,
        style: IconStyle::Warp,
        reserves_missing_secondary_lane: policy.reserves_missing_secondary_lane,
        treats_exhausted_secondary_as_missing: policy.treats_exhausted_secondary_as_missing,
        ..IconInput::default()
    });
    let no_bonus_layout = layout(&IconInput {
        primary: Some(25.0),
        secondary: None,
        style: IconStyle::Warp,
        reserves_missing_secondary_lane: policy.reserves_missing_secondary_lane,
        treats_exhausted_secondary_as_missing: policy.treats_exhausted_secondary_as_missing,
        ..IconInput::default()
    });
    assert_eq!(used_layout, no_bonus_layout);
}

#[test]
fn warp_bonus_lane_is_preserved_in_show_used_mode_when_bonus_is_unused() {
    use ariadusage_macos::presentation::resolved_icon_percents_for_style;
    use ariadusage_protocol::ids::ProviderId;
    use ariadusage_protocol::metric::{Confidence, Metric};
    use ariadusage_protocol::snapshot::ProviderSnapshot;
    use ariadusage_protocol::usage::RateWindow;

    let mut warp = ProviderSnapshot::new(ProviderId::new("warp").unwrap(), "Warp", true, "web");
    warp.windows.primary = Some(Metric::value(
        RateWindow::new(25.0),
        None,
        None,
        Confidence::Exact,
    ));
    warp.windows.secondary = Some(Metric::value(
        RateWindow::new(0.0),
        None,
        None,
        Confidence::Exact,
    ));

    let (p_used, s_used) =
        resolved_icon_percents_for_style(&warp, IconStyle::Warp, true, None, None);
    let policy = IconStyle::Warp.quota_layout_policy();
    let unused_bonus_layout = layout(&IconInput {
        primary: p_used,
        secondary: s_used,
        style: IconStyle::Warp,
        reserves_missing_secondary_lane: policy.reserves_missing_secondary_lane,
        treats_exhausted_secondary_as_missing: policy.treats_exhausted_secondary_as_missing,
        ..IconInput::default()
    });
    let no_bonus_layout = layout(&IconInput {
        primary: Some(25.0),
        secondary: None,
        style: IconStyle::Warp,
        reserves_missing_secondary_lane: policy.reserves_missing_secondary_lane,
        treats_exhausted_secondary_as_missing: policy.treats_exhausted_secondary_as_missing,
        ..IconInput::default()
    });
    assert_ne!(unused_bonus_layout, no_bonus_layout);
    assert_eq!(unused_bonus_layout.bars[1].percent, Some(0.1));
}

#[test]
fn split_provider_icon_skips_unchanged_render_signature() {
    let mut anim = StatusItemAnimationState::default();
    let input = IconInput {
        primary: Some(50.0),
        secondary: Some(75.0),
        style: IconStyle::Codex,
        ..IconInput::default()
    };
    let sig = icon_render_signature(
        &input,
        false,
        false,
        None,
        LoadingPattern::KnightRider,
        false,
    );
    assert!(!anim.should_skip_provider_icon_render("codex", sig));
    assert!(anim.should_skip_provider_icon_render("codex", sig));

    let updated = IconInput {
        primary: Some(45.0),
        ..input
    };
    let sig2 = icon_render_signature(
        &updated,
        false,
        false,
        None,
        LoadingPattern::KnightRider,
        false,
    );
    assert!(!anim.should_skip_provider_icon_render("codex", sig2));
}

#[test]
fn quota_warning_flash_state_lasts_for_configured_duration() {
    let mut anim = StatusItemAnimationState::default();
    let now = 2_000_000_000.0;
    let until = anim.start_quota_warning_flash("codex", now);
    assert_eq!(until, now + QUOTA_WARNING_FLASH_DURATION_SECS);
    assert!(anim.is_quota_warning_flash_active("codex", now));
    assert!(anim.is_quota_warning_flash_active("codex", now + 59.0));
    assert!(!anim.is_quota_warning_flash_active("codex", now + 61.0));
    assert!(!anim.is_quota_warning_flash_active("claude", now));
}

#[test]
fn merged_icon_render_signature_includes_quota_warning_flash_for_selected_provider() {
    let mut anim = StatusItemAnimationState::default();
    let now = 2_000_000_000.0;
    let input = IconInput {
        primary: Some(40.0),
        secondary: Some(60.0),
        style: IconStyle::Combined,
        ..IconInput::default()
    };
    let base_sig = icon_render_signature(
        &input,
        false,
        false,
        None,
        LoadingPattern::KnightRider,
        anim.is_quota_warning_flash_active("codex", now),
    );
    let _ = anim.start_quota_warning_flash("codex", now);
    let flash_sig = icon_render_signature(
        &input,
        false,
        false,
        None,
        LoadingPattern::KnightRider,
        anim.is_quota_warning_flash_active("codex", now),
    );
    assert_ne!(base_sig, flash_sig);
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
        // SourceOver 0.28 red fill covers the entire 36x36 square including (0, 0).
        let corner = rep.colorAtX_y(0, 0).unwrap();
        assert!(corner.alphaComponent() > 0.25);
        assert!(corner.redComponent() > corner.blueComponent());
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

        // Also export exact-percent matches for the golden status-item captures
        let golden_dark_match = layout(&IconInput {
            primary: Some(90.0),
            secondary: Some(73.0),
            hide_critters: true,
            ..IconInput::default()
        });
        std::fs::write(
            out_dir.join("bars-golden-dark-match.png"),
            render_png_for_appearance(&golden_dark_match, true, false, None, false).unwrap(),
        )
        .unwrap();

        let golden_light_match = layout(&IconInput {
            primary: None,
            secondary: None,
            reserves_missing_secondary_lane: true,
            stale: true,
            hide_critters: true,
            ..IconInput::default()
        });
        std::fs::write(
            out_dir.join("bars-golden-light-match.png"),
            render_png_for_appearance(&golden_light_match, true, false, None, false).unwrap(),
        )
        .unwrap();
    }
}
