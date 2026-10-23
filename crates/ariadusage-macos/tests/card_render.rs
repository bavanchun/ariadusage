//! Draws a usage card offscreen on the main thread and checks the pixels.
//!
//! Set `ARIADUSAGE_CARD_PNG` to a file path to also save the rendering.
//!
//! The binary has no libtest harness, so it answers the harness's `--list`
//! queries itself for cargo test and nextest.

#[cfg(target_os = "macos")]
const TEST_NAME: &str = "renders_the_card";

#[cfg(target_os = "macos")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if has("--list") {
        if !has("--ignored") {
            println!("{TEST_NAME}: test");
        }
        return;
    }
    macos::run();
    println!("test {TEST_NAME} ... ok");
}

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod macos {
    use ariadusage_macos::appkit::{CardView, FoundationDateTexts, png_bytes};
    use ariadusage_macos::card::{
        Accent, CardPreferences, CardPresentation, CardTitles, Rgb, card_model,
    };
    use ariadusage_macos::card_layout::Item;
    use ariadusage_macos::geometry::Rect;
    use ariadusage_protocol::ids::ProviderId;
    use ariadusage_protocol::metric::{Confidence, Metric};
    use ariadusage_protocol::snapshot::ProviderSnapshot;
    use ariadusage_protocol::usage::{Credits, Identity, Pace, PaceStage, RateWindow};
    use jiff::{SignedDuration, Timestamp};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSAppearance, NSApplication, NSBitmapImageRep};
    use objc2_foundation::NSString;

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

    pub fn run() {
        let mtm = MainThreadMarker::new().expect("runs on the main thread");
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
    }
}
