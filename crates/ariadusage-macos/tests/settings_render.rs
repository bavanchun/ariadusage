// Ported from CodexBar Tests/CodexBarTests/SettingsWindowBehaviorTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Renders the Settings window panes (`General`, `Menu Bar`, `Menu`, `About`)
//! on the main thread in both Light and Dark appearances, verifies native
//! AppKit control subviews (`NSSearchField`, `NSOutlineView`, `NSScrollView`,
//! `NSSwitch`, `NSPopUpButton`, `NSButton`, `NSSlider`, `NSStepper`), and
//! checks window chrome, dimensions, and pixels.
//!
//! Set `ARIADUSAGE_SETTINGS_CAPTURE_DIR` to write `general.png`, `menu-bar.png`,
//! `menu.png`, and `about.png` under `{dir}/dark/` and `{dir}/light/` (and `{dir}/`).
//! If `ARIADUSAGE_CBCAP_BIN` is also set to the `cbcap` binary path, dumps the
//! live Accessibility tree of each pane to `{stem}-ax.txt` next to the PNG files.

#[cfg(target_os = "macos")]
const TEST_NAME: &str = "renders_settings_panes_in_light_and_dark_appearances";

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
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    use ariadusage_macos::appkit::{SettingsWindowController, png_bytes};
    use ariadusage_macos::settings::{
        SettingsPane, WINDOW_AUTOSAVE_NAME, WINDOW_HEIGHT, WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH,
        WINDOW_WIDTH,
    };
    use ariadusage_protocol::ids::{ProviderId, SettingId};
    use ariadusage_protocol::metric::{Confidence, Metric};
    use ariadusage_protocol::settings::{
        ChoiceOption, DescriptorKind, NumberConfig, SettingDescriptor, SettingsPage, SettingsScope,
        SettingsSection,
    };
    use ariadusage_protocol::snapshot::{EngineInfo, EngineSnapshot, ProviderSnapshot};
    use ariadusage_protocol::usage::{CostHistory, ProviderStatus, StatusIndicator};
    use jiff::Timestamp;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSAppearance, NSAppearanceCustomization, NSApplication};
    use objc2_foundation::{NSDate, NSRunLoop, NSString};

    fn sample_app_settings() -> SettingsPage {
        SettingsPage::new(
            SettingsScope::App,
            vec![SettingsSection::new(
                "general",
                "General",
                vec![
                    SettingDescriptor::new(
                        SettingId::new("refreshInterval").unwrap(),
                        "Refresh interval",
                        DescriptorKind::Number {
                            value: 900.0,
                            config: NumberConfig::default(),
                        },
                    ),
                    SettingDescriptor::new(
                        SettingId::new("menuBar.iconStyle").unwrap(),
                        "Menu bar style",
                        DescriptorKind::Choice {
                            selected: "bars".to_string(),
                            options: vec![
                                ChoiceOption::new("bars", "Bars"),
                                ChoiceOption::new("critters", "Critters"),
                            ],
                        },
                    ),
                    SettingDescriptor::new(
                        SettingId::new("menuBar.showUsed").unwrap(),
                        "Usage bars fill",
                        DescriptorKind::Toggle { value: false },
                    ),
                    SettingDescriptor::new(
                        SettingId::new("menuBar.mergeIcons").unwrap(),
                        "Merge icons",
                        DescriptorKind::Toggle { value: true },
                    ),
                    SettingDescriptor::new(
                        SettingId::new("menu.showOptionalCreditsAndExtraUsage").unwrap(),
                        "Show optional credits, spending & billing details",
                        DescriptorKind::Toggle { value: true },
                    ),
                ],
            )],
        )
    }

    fn sample_snapshot() -> EngineSnapshot {
        let mut codex =
            ProviderSnapshot::new(ProviderId::new("codex").unwrap(), "Codex", true, "cli");
        codex.status = Some(Metric::value(
            ProviderStatus {
                indicator: StatusIndicator::None,
                description: Some("All Systems Operational".to_string()),
                updated_at: None,
                page_url: None,
                components: Vec::new(),
            },
            None,
            None,
            Confidence::Exact,
        ));
        codex.cost_history = Some(Metric::value(
            CostHistory {
                session_cost_usd: Some(0.18),
                session_tokens: Some(460_000),
                last_30_days_cost_usd: Some(285.81),
                last_30_days_tokens: Some(72_000_000),
                history_days: Some(30),
                currency_code: Some("USD".to_string()),
                ..CostHistory::default()
            },
            None,
            None,
            Confidence::Exact,
        ));

        let mut claude =
            ProviderSnapshot::new(ProviderId::new("claude").unwrap(), "Claude", true, "cli");
        claude.status = codex.status.clone();
        claude.cost_history = Some(Metric::value(
            CostHistory {
                session_cost_usd: Some(1.42),
                session_tokens: Some(890_000),
                last_30_days_cost_usd: Some(254.09),
                last_30_days_tokens: Some(61_000_000),
                history_days: Some(30),
                currency_code: Some("USD".to_string()),
                ..CostHistory::default()
            },
            None,
            None,
            Confidence::Exact,
        ));

        let mut antigravity = ProviderSnapshot::new(
            ProviderId::new("antigravity").unwrap(),
            "Antigravity",
            true,
            "local",
        );
        antigravity.status = codex.status.clone();

        EngineSnapshot {
            schema_version: 1,
            generated_at: Timestamp::now(),
            stale_after_seconds: 900,
            engine: EngineInfo {
                version: "0.0.0".to_string(),
                refreshing: false,
            },
            providers: vec![codex, claude, antigravity],
        }
    }

    fn pump_runloop(duration: Duration) {
        let start = Instant::now();
        let rl = NSRunLoop::currentRunLoop();
        while start.elapsed() < duration {
            let step = NSDate::dateWithTimeIntervalSinceNow(0.015);
            rl.runUntilDate(&step);
        }
    }

    fn dump_ax_tree_with_cbcap(cbcap_bin: &Path, out_path: &Path) {
        let pid = std::process::id().to_string();
        let out_str = out_path.to_string_lossy().to_string();
        let bin = cbcap_bin.to_path_buf();
        let handle = std::thread::spawn(move || {
            Command::new(bin)
                .args(["dump", &pid, &out_str])
                .status()
                .expect("spawn cbcap dump")
        });
        while !handle.is_finished() {
            pump_runloop(Duration::from_millis(15));
        }
        let status = handle.join().expect("join cbcap thread");
        assert!(status.success(), "cbcap dump failed with {status}");
    }

    pub fn run() {
        let mtm = MainThreadMarker::new().expect("runs on the main thread");
        let app = NSApplication::sharedApplication(mtm);

        // Verify SettingsWindowController chrome and invariants.
        let controller = SettingsWindowController::new(mtm, None);
        let win = controller.window();
        assert!(!win.isReleasedWhenClosed());
        assert!(win.titlebarAppearsTransparent());
        assert_eq!(win.frameAutosaveName().to_string(), WINDOW_AUTOSAVE_NAME);
        let min_size = win.contentMinSize();
        assert_eq!(min_size.width, WINDOW_MIN_WIDTH);
        assert_eq!(min_size.height, WINDOW_MIN_HEIGHT);

        let view = controller.view();
        view.apply_settings_page(&sample_app_settings());
        view.apply_snapshot(sample_snapshot());

        let bounds = view.bounds();
        assert_eq!(bounds.size.width, WINDOW_WIDTH);
        assert_eq!(bounds.size.height, WINDOW_HEIGHT);

        // Verify live search field filtering and NSOutlineView row counts.
        let outline = view.sidebar_outline_view().expect("sidebar NSOutlineView");
        // 10 app panes + 1 Providers header + 3 providers = 14 rows
        assert_eq!(outline.numberOfRows(), 14);
        view.set_search_text("claude");
        // 10 app panes + 1 Providers header + 1 matching provider = 12 rows
        assert_eq!(outline.numberOfRows(), 12);
        view.set_search_text("no-such-provider");
        // 10 app panes + 1 Providers header + 1 empty filter state row = 12 rows
        assert_eq!(outline.numberOfRows(), 12);
        view.set_search_text("");
        assert_eq!(outline.numberOfRows(), 14);

        // Verify native AppKit controls across panes.
        view.select_pane(SettingsPane::Menu);
        let classes = view.descendant_class_names();
        for required in [
            "NSSearchField",
            "NSOutlineView",
            "NSScrollView",
            "NSSwitch",
            "NSPopUpButton",
            "NSButton",
            "NSSlider",
            "NSStepper",
            "NSTextField",
        ] {
            assert!(
                classes.iter().any(|c| c.contains(required)),
                "expected native AppKit control `{required}` in view hierarchy, found {classes:?}"
            );
        }

        let capture_dir = std::env::var_os("ARIADUSAGE_SETTINGS_CAPTURE_DIR").map(PathBuf::from);
        let cbcap_bin = std::env::var_os("ARIADUSAGE_CBCAP_BIN").map(PathBuf::from);
        if cbcap_bin.is_some() {
            controller.show_window(Some(SettingsPane::General));
            pump_runloop(Duration::from_millis(80));
        }

        let panes = [
            (SettingsPane::General, "general"),
            (SettingsPane::MenuBar, "menu-bar"),
            (SettingsPane::Menu, "menu"),
            (SettingsPane::About, "about"),
        ];

        for (dark, mode_name, appearance_name) in [
            (false, "light", "NSAppearanceNameAqua"),
            (true, "dark", "NSAppearanceNameDarkAqua"),
        ] {
            let appearance = NSAppearance::appearanceNamed(&NSString::from_str(appearance_name));
            app.setAppearance(appearance.as_deref());
            win.setAppearance(appearance.as_deref());
            view.set_appearance_dark_override(Some(dark));

            for (pane, stem) in &panes {
                let filename = format!("{stem}.png");
                view.select_pane(pane.clone());
                pump_runloop(Duration::from_millis(30));

                let rep = view.render_bitmap().expect("bitmap representation");
                let png = png_bytes(&rep).expect("PNG bytes");
                assert!(!png.is_empty());

                if let Some(base) = &capture_dir {
                    let sub = base.join(mode_name);
                    std::fs::create_dir_all(&sub).expect("create mode capture dir");
                    std::fs::write(sub.join(&filename), &png).expect("write mode PNG");
                    if dark {
                        std::fs::create_dir_all(base).expect("create base capture dir");
                        std::fs::write(base.join(&filename), &png).expect("write base PNG");
                    }
                    if let Some(cbcap) = &cbcap_bin {
                        let ax_path = sub.join(format!("{stem}-ax.txt"));
                        dump_ax_tree_with_cbcap(cbcap, &ax_path);
                    }
                }

                let scale = rep.pixelsWide() as f64 / bounds.size.width;
                let sample = rep
                    .colorAtX_y((840.0 * scale) as isize, (20.0 * scale) as isize)
                    .expect("detail header pixel");
                let r = sample.redComponent();
                if dark {
                    assert!(r < 0.30, "expected dark detail background, got r={r}");
                } else {
                    assert!(r > 0.80, "expected light detail background, got r={r}");
                }
            }
        }
    }
}
