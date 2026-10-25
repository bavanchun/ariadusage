//! Draws usage cards and menu views offscreen on the main thread, checks
//! pixel invariants, and compares dark-mode renderings against external
//! capture PNGs when `ARIADUSAGE_CAPTURES_DIR` is set.
//!
//! Set `ARIADUSAGE_CARD_PNG` to a file path to also save the rendering.
//!
//! The binary has no libtest harness, so it answers the harness's `--list`
//! queries itself for cargo test and nextest.

mod common;

#[cfg(target_os = "macos")]
fn main() {
    macos::main();
}

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::Cell;
    use std::rc::Rc;

    use ariadusage_macos::appkit::{
        CardView, CostHistoryChartView, FoundationDateTexts, PlanUtilizationChartView,
        QuotaBurndownChartView, SwitcherView, png_bytes,
    };
    use ariadusage_macos::card::{
        Accent, CardPreferences, CardPresentation, CardTitles, Rgb, card_model, provider_card,
    };
    use ariadusage_macos::card_layout::{CardSectionMode, Item};
    use ariadusage_macos::chart::{
        CostHistoryChartModel, PlanUtilizationChartModel, QuotaBurndownChartModel,
    };
    use ariadusage_macos::geometry::{Point, Rect};
    use ariadusage_macos::menu_descriptor::{self, MenuEntry, PrimaryContent};
    use ariadusage_macos::switcher::{self, SwitcherSelection};
    use ariadusage_protocol::fixture::generate_scenario_snapshot;
    use ariadusage_protocol::ids::ProviderId;
    use ariadusage_protocol::metric::{Confidence, Metric};
    use ariadusage_protocol::snapshot::ProviderSnapshot;
    use ariadusage_protocol::usage::{Credits, Identity, Pace, PaceStage, RateWindow};
    use jiff::{SignedDuration, Timestamp};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSAppearance, NSApplication, NSBitmapImageRep};
    use objc2_foundation::NSString;

    use crate::common::pixel_diff::{
        RgbaImage, captures_dir_from_env, compare_and_write_diff, create_diff_output_dir,
        render_dark_menu_structure, render_dark_submenu, report_and_assert_suite,
    };

    type TestFn = fn(MainThreadMarker);

    const TESTS: &[(&str, TestFn)] = &[
        ("renders_the_card", renders_the_card),
        (
            "pixel_diff_cards_and_menus_against_captures",
            pixel_diff_cards_and_menus_against_captures,
        ),
    ];

    pub fn main() {
        let mtm = MainThreadMarker::new().expect("runs on the main thread");
        let _app = NSApplication::sharedApplication(mtm);

        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut list_only = false;
        let mut ignored_only = false;
        let mut exact = false;
        let mut filters: Vec<String> = Vec::new();
        for arg in args {
            match arg.as_str() {
                "--list" => list_only = true,
                "--ignored" => ignored_only = true,
                "--exact" => exact = true,
                "--nocapture" | "--show-output" | "--quiet" | "-q" => {}
                a if a.starts_with('-') => {}
                other => filters.push(other.to_string()),
            }
        }

        if list_only {
            if !ignored_only {
                for (name, _) in TESTS {
                    println!("{name}: test");
                }
            }
            return;
        }

        let selected: Vec<&(&str, TestFn)> = TESTS
            .iter()
            .filter(|(name, _)| {
                if filters.is_empty() {
                    true
                } else if exact {
                    filters.iter().any(|f| f == name)
                } else {
                    filters.iter().any(|f| name.contains(f))
                }
            })
            .collect();

        for (name, func) in &selected {
            func(mtm);
            println!("test {name} ... ok");
        }
    }

    fn value<T>(v: T) -> Metric<T> {
        Metric::value(v, None, None, Confidence::Exact)
    }

    fn window(now: Timestamp, used: f64, minutes: u32, resets_in: SignedDuration) -> RateWindow {
        RateWindow {
            window_minutes: Some(minutes),
            resets_at: Some(now + resets_in),
            ..RateWindow::new(used)
        }
    }

    fn snapshot(now: Timestamp) -> ProviderSnapshot {
        let mut p = ProviderSnapshot::new(ProviderId::new("codex").unwrap(), "Codex", true, "cli");
        p.updated_at = Some(now - SignedDuration::from_mins(3));
        p.windows.primary = Some(value(window(
            now,
            42.0,
            300,
            SignedDuration::from_mins(133),
        )));
        let mut weekly = window(now, 50.0, 10080, SignedDuration::from_hours(96));
        weekly.pace = Some(Pace {
            stage: PaceStage::Ahead,
            delta_percent: 7.0,
            expected_used_percent: 43.0,
            will_last_to_reset: false,
            eta_seconds: Some(3 * 24 * 3600),
            speed_multiplier_to_reset: None,
            run_out_probability: None,
            summary: String::new(),
        });
        p.windows.secondary = Some(value(weekly));
        p.credits = Some(value(Credits::new(42.5)));
        p.identity = Some(value(Identity {
            provider_id: ProviderId::new("codex").unwrap(),
            account_email: None,
            account_organization: None,
            login_method: Some("plus".to_string()),
            account_id: None,
            plan: None,
        }));
        p
    }

    /// Alpha of the pixel at a point, in the bitmap's pixel grid.
    fn alpha_at(rep: &NSBitmapImageRep, scale: f64, x: f64, y: f64) -> f64 {
        let color = rep
            .colorAtX_y((x * scale) as isize, (y * scale) as isize)
            .expect("pixel color");
        color.alphaComponent()
    }

    fn renders_the_card(mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);
        // Render in the light appearance whatever the system uses.
        let aqua = NSAppearance::appearanceNamed(&NSString::from_str("NSAppearanceNameAqua"));
        app.setAppearance(aqua.as_deref());
        let now = Timestamp::now();
        let presentation = CardPresentation {
            titles: CardTitles::default(),
            accent: Accent::Brand(Rgb::from_hex("#49A3B0").unwrap()),
        };
        let dates = FoundationDateTexts::new();
        let model = card_model(
            &snapshot(now),
            &presentation,
            &CardPreferences {
                work_days_per_week: Some(5),
                weekly_warning_thresholds: vec![20],
                ..CardPreferences::default()
            },
            now,
            &dates,
        );

        let view = CardView::new(mtm);
        assert!(view.update(&model, 310.0));
        assert!(!view.update(&model, 310.0), "an unchanged model is a no-op");
        assert!(
            view.has_compatible_height(&model, 310.0, false),
            "same-height model is compatible for in-place open-menu update"
        );
        let mut shorter_snap = snapshot(now);
        shorter_snap.credits = None;
        let shorter_model = card_model(
            &shorter_snap,
            &presentation,
            &CardPreferences::default(),
            now,
            &dates,
        );
        assert!(
            !view.has_compatible_height(&shorter_model, 310.0, false),
            "removing the credits section changes card height and must defer while menu is open"
        );
        let layout = view.layout().expect("layout");
        let bounds = view.bounds();
        assert_eq!(bounds.size.width, 310.0);
        assert_eq!(bounds.size.height, layout.size.h);

        let rep = view.render_bitmap().expect("bitmap");
        let scale = rep.pixelsWide() as f64 / bounds.size.width;
        let bars: Vec<Rect> = layout
            .items
            .iter()
            .filter(|p| matches!(p.item, Item::Bar { .. }))
            .map(|p| p.frame)
            .collect();
        assert_eq!(bars.len(), 3, "session, weekly and credits bars");

        // The session bar is 58 % full: opaque fill on the left, a faint
        // track on the right.
        let session = bars[0];
        let mid_y = session.y + session.h / 2.0;
        let fill = alpha_at(&rep, scale, session.x + 10.0, mid_y);
        let track = alpha_at(&rep, scale, session.max_x() - 10.0, mid_y);
        assert!(fill > 0.9, "fill alpha {fill}");
        assert!(track > 0.0 && track < 0.5, "track alpha {track}");

        // The weekly pace tip at 57 % draws an opaque center stripe inside a
        // punched gap.
        let weekly = bars[1];
        let tip_x = weekly.x + weekly.w * 0.57;
        let weekly_mid = weekly.y + weekly.h / 2.0;
        let stripe = alpha_at(&rep, scale, tip_x, weekly_mid);
        let punched = alpha_at(&rep, scale, tip_x - 2.5, weekly_mid);
        assert!(stripe > 0.9, "stripe alpha {stripe}");
        assert!(punched < 0.5, "punched alpha {punched}");

        if let Some(path) = std::env::var_os("ARIADUSAGE_CARD_PNG") {
            let png = png_bytes(&rep).expect("PNG encoding");
            std::fs::write(path, png).expect("write PNG");
        }

        // Verify SwitcherView renders and updates selection on click.
        let fixture = generate_scenario_snapshot(0);
        let segs = switcher::segments(&fixture, false);
        let switcher_view = SwitcherView::new(mtm);
        let selected_count = Rc::new(Cell::new(0usize));
        let selected_count_cb = selected_count.clone();
        switcher_view.set_on_select(move |_| {
            selected_count_cb.set(selected_count_cb.get() + 1);
        });
        assert!(switcher_view.update(&segs, &SwitcherSelection::Overview, 310.0));
        let sw_layout = switcher_view.layout().expect("switcher layout");
        let sw_rep = switcher_view.render_bitmap().expect("switcher bitmap");
        let sw_scale = sw_rep.pixelsWide() as f64 / sw_layout.size.w;

        // Overview button is selected and filled with the accent background.
        let ov_btn = &sw_layout.segments[0];
        let ov_alpha = alpha_at(
            &sw_rep,
            sw_scale,
            ov_btn.frame.x + 3.0,
            ov_btn.frame.y + 3.0,
        );
        assert!(
            ov_alpha > 0.9,
            "selected button background alpha {ov_alpha}"
        );

        // Clicking the Codex segment selects it and invokes the callback.
        let codex_btn = &sw_layout.segments[2];
        let codex_pt = Point::new(
            codex_btn.frame.x + codex_btn.frame.w / 2.0,
            codex_btn.frame.y + codex_btn.frame.h / 2.0,
        );
        let clicked = switcher_view.click_at(codex_pt);
        assert_eq!(
            clicked,
            Some(SwitcherSelection::Provider("codex".to_string()))
        );
        assert_eq!(selected_count.get(), 1);

        // Verify OverviewSpendSummary renders at 310 pt width via CardView::update_spend_summary.
        let enabled: Vec<_> = fixture.providers.iter().filter(|p| p.enabled).collect();
        let summary = ariadusage_macos::menu_descriptor::overview_spend_summary(&enabled)
            .expect("spend summary");
        let summary_view = CardView::new(mtm);
        assert!(summary_view.update_spend_summary(&summary, 310.0));
        assert!(
            !summary_view.update_spend_summary(&summary, 310.0),
            "unchanged spend summary is a no-op"
        );
        assert!(summary_view.has_compatible_spend_summary_height(&summary, 310.0));
        assert_eq!(summary_view.bounds().size.width, 310.0);
        assert!(summary_view.bounds().size.height > 100.0);
        let summary_rep = summary_view.render_bitmap().expect("spend summary bitmap");
        assert!(summary_rep.pixelsWide() >= 310);

        // Verify fixture Claude and Codex cards render all detail sections and submenu charts.
        let fixture_now: Timestamp = "2026-10-06T12:52:00Z".parse().unwrap();
        let claude = fixture
            .providers
            .iter()
            .find(|p| p.id.as_str() == "claude")
            .unwrap();
        let codex = fixture
            .providers
            .iter()
            .find(|p| p.id.as_str() == "codex")
            .unwrap();
        let claude_card = provider_card(claude, &CardPreferences::default(), fixture_now, &dates);
        let codex_card = provider_card(codex, &CardPreferences::default(), fixture_now, &dates);

        let claude_view = CardView::new(mtm);
        assert!(claude_view.update(&claude_card, 310.0));
        assert!(claude_view.bounds().size.height > 400.0);
        let claude_dark = claude_view
            .render_bitmap_with_appearance(true)
            .and_then(|r| png_bytes(&r))
            .expect("claude dark PNG");
        let claude_light = claude_view
            .render_bitmap_with_appearance(false)
            .and_then(|r| png_bytes(&r))
            .expect("claude light PNG");
        assert_ne!(
            claude_dark, claude_light,
            "dark and light card bitmaps must resolve different appearance colors"
        );

        let codex_view = CardView::new(mtm);
        assert!(codex_view.update_section(
            &codex_card,
            CardSectionMode::HeaderAndUsage {
                bottom_padding: 4.0
            },
            310.0
        ));
        assert!(codex_view.bounds().size.height > 300.0);
        assert!(codex_view.render_bitmap().is_some());

        let mut cost_chart_3_models = CostHistoryChartModel::from_provider(claude, "USD").unwrap();
        cost_chart_3_models.detail_viewport_row_count = 3;
        assert_eq!(cost_chart_3_models.preferred_height(), 434.0);

        let cost_chart = CostHistoryChartModel::from_provider(claude, "USD").unwrap();
        let cost_chart_view = CostHistoryChartView::new(mtm, cost_chart);
        assert_eq!(cost_chart_view.bounds().size.width, 310.0);
        assert_eq!(cost_chart_view.bounds().size.height, 378.0);
        let cost_dark = cost_chart_view
            .render_bitmap_with_appearance(true)
            .and_then(|r| png_bytes(&r))
            .expect("cost chart dark PNG");
        let cost_light = cost_chart_view
            .render_bitmap_with_appearance(false)
            .and_then(|r| png_bytes(&r))
            .expect("cost chart light PNG");
        assert_ne!(cost_dark, cost_light);

        let burndown_chart =
            QuotaBurndownChartModel::from_provider(claude, fixture_now, &dates).unwrap();
        let burndown_view = QuotaBurndownChartView::new(mtm, burndown_chart);
        assert_eq!(burndown_view.bounds().size.width, 310.0);
        assert_eq!(burndown_view.bounds().size.height, 259.0);
        assert!(burndown_view.render_bitmap().is_some());

        let util_chart = PlanUtilizationChartModel::from_provider(claude, &dates).unwrap();
        let util_view = PlanUtilizationChartView::new(mtm, util_chart);
        assert_eq!(util_view.bounds().size.width, 310.0);
        assert_eq!(util_view.bounds().size.height, 210.0);
        assert!(util_view.render_bitmap().is_some());

        if let Some(dir) = std::env::var_os("ARIADUSAGE_CAPTURE_DIR") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).expect("create capture dir");
            for (suffix, is_dark) in [("dark", true), ("light", false)] {
                let claude_v = CardView::new(mtm);
                claude_v.update(&claude_card, 310.0);
                if let Some(rep) = claude_v.render_bitmap_with_appearance(is_dark)
                    && let Some(png) = png_bytes(&rep)
                {
                    std::fs::write(dir.join(format!("card-claude-{suffix}.png")), png).unwrap();
                }

                let codex_v = CardView::new(mtm);
                codex_v.update(&codex_card, 310.0);
                if let Some(rep) = codex_v.render_bitmap_with_appearance(is_dark)
                    && let Some(png) = png_bytes(&rep)
                {
                    std::fs::write(dir.join(format!("card-codex-{suffix}.png")), png).unwrap();
                }

                for (pid, p_snap) in [("claude", claude), ("codex", codex)] {
                    if let Some(cc) = CostHistoryChartModel::from_provider(p_snap, "USD") {
                        let cv = CostHistoryChartView::new(mtm, cc);
                        if let Some(rep) = cv.render_bitmap_with_appearance(is_dark)
                            && let Some(png) = png_bytes(&rep)
                        {
                            std::fs::write(
                                dir.join(format!("sub-{pid}-cost-chart-{suffix}.png")),
                                png,
                            )
                            .unwrap();
                        }
                    }
                    if let Some(bc) =
                        QuotaBurndownChartModel::from_provider(p_snap, fixture_now, &dates)
                    {
                        let bv = QuotaBurndownChartView::new(mtm, bc);
                        if let Some(rep) = bv.render_bitmap_with_appearance(is_dark)
                            && let Some(png) = png_bytes(&rep)
                        {
                            std::fs::write(
                                dir.join(format!("sub-{pid}-burndown-chart-{suffix}.png")),
                                png,
                            )
                            .unwrap();
                        }
                    }
                    if let Some(uc) = PlanUtilizationChartModel::from_provider(p_snap, &dates) {
                        let uv = PlanUtilizationChartView::new(mtm, uc);
                        if let Some(rep) = uv.render_bitmap_with_appearance(is_dark)
                            && let Some(png) = png_bytes(&rep)
                        {
                            std::fs::write(
                                dir.join(format!("sub-{pid}-utilization-chart-{suffix}.png")),
                                png,
                            )
                            .unwrap();
                        }
                    }
                }
            }
        }
    }

    // Rust-added: pixel-diff harness comparing dark-mode card, menu tab, and submenu
    // views rendered on fixture data against external golden capture PNGs.
    fn pixel_diff_cards_and_menus_against_captures(mtm: MainThreadMarker) {
        let Some(captures_dir) =
            captures_dir_from_env("pixel_diff_cards_and_menus_against_captures")
        else {
            return;
        };
        let diff_dir = create_diff_output_dir("card-menu-render");

        let fixture_now: Timestamp = "2026-10-06T12:52:00Z".parse().unwrap();
        let mut fixture = generate_scenario_snapshot(0);
        fixture.generated_at = fixture_now;
        let dates = FoundationDateTexts::new();
        let prefs = CardPreferences::default();

        let mut rows = Vec::new();

        // 1. Dark-mode Card views compared against their card regions in dark/tab-*-w0.png.
        let card_cases = [
            (
                "card/claude",
                "claude",
                "dark/tab-claude-w0.png",
                (0usize, 104usize, 620usize, 1596usize),
            ),
            (
                "card/codex",
                "codex",
                "dark/tab-codex-w0.png",
                (0usize, 104usize, 620usize, 1400usize),
            ),
            (
                "card/antigravity",
                "antigravity",
                "dark/tab-antigravity-w0.png",
                (0usize, 104usize, 620usize, 246usize),
            ),
        ];
        for (view_name, provider_id, rel_path, (cx, cy, cw, ch)) in card_cases {
            let p_snap = fixture
                .providers
                .iter()
                .find(|p| p.id.as_str() == provider_id)
                .expect("provider in fixture");
            let model = provider_card(p_snap, &prefs, fixture_now, &dates);
            let card_view = CardView::new(mtm);
            card_view.update(&model, 310.0);
            let rep = card_view
                .render_bitmap_with_appearance(true)
                .expect("dark card bitmap");
            let actual = RgbaImage::from_bitmap_rep(&rep);
            let full_expected =
                RgbaImage::from_png_file(&captures_dir.join(rel_path)).expect("load capture PNG");
            let expected = full_expected.crop(cx, cy, cw, ch);
            rows.push(compare_and_write_diff(
                view_name,
                &format!("{rel_path} [crop {cx},{cy} {cw}x{ch}]"),
                &actual,
                &expected,
                &diff_dir,
            ));
        }

        // Overview spend summary card compared against top card region in dark/tab-overview-w0.png.
        {
            let enabled: Vec<_> = fixture.providers.iter().filter(|p| p.enabled).collect();
            let summary =
                menu_descriptor::overview_spend_summary(&enabled).expect("overview spend summary");
            let summary_view = CardView::new(mtm);
            summary_view.update_spend_summary(&summary, 310.0);
            let rep = summary_view
                .render_bitmap_with_appearance(true)
                .expect("dark overview spend bitmap");
            let actual = RgbaImage::from_bitmap_rep(&rep);
            let rel_path = "dark/tab-overview-w0.png";
            let full_expected =
                RgbaImage::from_png_file(&captures_dir.join(rel_path)).expect("load capture PNG");
            let expected = full_expected.crop(0, 104, 620, 364);
            rows.push(compare_and_write_diff(
                "card/overview-spend",
                "dark/tab-overview-w0.png [crop 0,104 620x364]",
                &actual,
                &expected,
                &diff_dir,
            ));
        }

        // 2. Dark-mode full Menu tab views compared against dark/tab-*-w0.png.
        let menu_tab_cases = [
            (
                "menu/tab-overview",
                SwitcherSelection::Overview,
                "dark/tab-overview-w0.png",
            ),
            (
                "menu/tab-claude",
                SwitcherSelection::Provider("claude".to_string()),
                "dark/tab-claude-w0.png",
            ),
            (
                "menu/tab-codex",
                SwitcherSelection::Provider("codex".to_string()),
                "dark/tab-codex-w0.png",
            ),
            (
                "menu/tab-antigravity",
                SwitcherSelection::Provider("antigravity".to_string()),
                "dark/tab-antigravity-w0.png",
            ),
        ];
        for (view_name, selection, rel_path) in menu_tab_cases {
            let structure = menu_descriptor::build(
                Some(&fixture),
                true,
                Some(&selection),
                &prefs,
                fixture_now,
                &dates,
            );
            let actual = render_dark_menu_structure(mtm, &structure, 310.0);
            let expected =
                RgbaImage::from_png_file(&captures_dir.join(rel_path)).expect("load capture PNG");
            rows.push(compare_and_write_diff(
                view_name, rel_path, &actual, &expected, &diff_dir,
            ));
        }

        // 3. Dark-mode Submenu views compared against the opened submenu panel in dark/sub-*-w0.png.
        let submenu_cases = [
            (
                "menu/sub-claude-cost",
                "claude",
                true,
                "dark/sub-claude-cost-w0.png",
                (0usize, 1702usize, 620usize, 980usize),
            ),
            (
                "menu/sub-claude-plan-usage",
                "claude",
                false,
                "dark/sub-claude-plan-usage-w0.png",
                (0usize, 1702usize, 620usize, 980usize),
            ),
            (
                "menu/sub-codex-cost",
                "codex",
                true,
                "dark/sub-codex-cost-w0.png",
                (0usize, 1598usize, 620usize, 980usize),
            ),
            (
                "menu/sub-codex-plan-usage",
                "codex",
                false,
                "dark/sub-codex-plan-usage-w0.png",
                (0usize, 1598usize, 620usize, 980usize),
            ),
            (
                "menu/sub-antigravity-plan-usage",
                "antigravity",
                false,
                "dark/sub-antigravity-plan-usage-w0.png",
                (0usize, 418usize, 620usize, 440usize),
            ),
        ];
        for (view_name, provider_id, is_cost, rel_path, (cx, cy, cw, ch)) in submenu_cases {
            let selection = SwitcherSelection::Provider(provider_id.to_string());
            let structure = menu_descriptor::build(
                Some(&fixture),
                true,
                Some(&selection),
                &prefs,
                fixture_now,
                &dates,
            );
            let items = if is_cost {
                match &structure.primary {
                    PrimaryContent::Provider { cost_submenu, .. } if !cost_submenu.is_empty() => {
                        cost_submenu.clone()
                    }
                    _ => structure
                        .history
                        .iter()
                        .find_map(|e| match e {
                            MenuEntry::Submenu { title, items, .. }
                                if title.to_lowercase().contains("cost") =>
                            {
                                Some(items.clone())
                            }
                            _ => None,
                        })
                        .unwrap_or_default(),
                }
            } else {
                structure
                    .history
                    .iter()
                    .find_map(|e| match e {
                        MenuEntry::Submenu { title, items, .. }
                            if title.to_lowercase().contains("usage") =>
                        {
                            Some(items.clone())
                        }
                        _ => None,
                    })
                    .unwrap_or_default()
            };
            let actual = render_dark_submenu(mtm, &items, 310.0);
            let full_expected =
                RgbaImage::from_png_file(&captures_dir.join(rel_path)).expect("load capture PNG");
            let expected = full_expected.crop(cx, cy, cw, ch);
            rows.push(compare_and_write_diff(
                view_name,
                &format!("{rel_path} [crop {cx},{cy} {cw}x{ch}]"),
                &actual,
                &expected,
                &diff_dir,
            ));
        }

        report_and_assert_suite(
            "pixel_diff_cards_and_menus_against_captures",
            &diff_dir,
            &rows,
        );
    }
}
