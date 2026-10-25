// Ported from CodexBar Tests/CodexBarTests/SettingsWindowBehaviorTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Renders all 13 Settings window panes (`General`, `iCloud Sync`, `Usage & Spend`,
//! `Notifications`, `Menu Bar`, `Menu`, `Advanced`, `Hooks`, `Plugins`, `About`,
//! and provider detail pages `Codex`, `Claude`, `Antigravity`) on the
//! main thread in both Light and Dark appearances, verifies native AppKit control
//! subviews (`NSSearchField`, `NSOutlineView`, `NSScrollView`, `NSSwitch`,
//! `NSPopUpButton`, `NSButton`, `NSSegmentedControl`, `NSColorWell`, `NSSlider`,
//! `NSStepper`, `NSTextField`), and checks window chrome, dimensions, and pixels.
//!
//! Set `ARIADUSAGE_SETTINGS_CAPTURE_DIR` to write `{stem}.png` under `{dir}/dark/`
//! and `{dir}/light/` (and `{dir}/`).
//! If `ARIADUSAGE_CBCAP_BIN` is also set to the `cbcap` binary path, dumps the
//! live Accessibility tree of each pane to `{stem}-ax.txt` next to the PNG files.

mod common;

#[cfg(target_os = "macos")]
fn main() {
    macos::main();
}

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use ariadusage_macos::appkit::{SettingsWindowController, png_bytes};
    use ariadusage_macos::settings::{
        SettingsPane, WINDOW_AUTOSAVE_NAME, WINDOW_HEIGHT, WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH,
        WINDOW_WIDTH,
    };
    use ariadusage_protocol::fixture::generate_scenario_snapshot;
    use ariadusage_protocol::ids::{ProviderId, SettingId};
    use ariadusage_protocol::settings::{
        ChoiceOption, DescriptorKind, NumberConfig, SettingDescriptor, SettingsPage, SettingsScope,
        SettingsSection,
    };
    use ariadusage_protocol::snapshot::EngineSnapshot;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSAppearance, NSAppearanceCustomization, NSApplication};
    use objc2_foundation::{NSDate, NSRunLoop, NSString};

    use crate::common::pixel_diff::{
        RgbaImage, captures_dir_from_env, compare_and_write_diff, create_diff_output_dir,
        report_and_assert_suite,
    };

    type TestFn = fn(MainThreadMarker);

    const TESTS: &[(&str, TestFn)] = &[
        (
            "renders_settings_panes_in_light_and_dark_appearances",
            renders_settings_panes_in_light_and_dark_appearances,
        ),
        (
            "pixel_diff_settings_panes_against_captures",
            pixel_diff_settings_panes_against_captures,
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

    fn sample_provider_settings(id: &str, source_mode: &str, secret_set: bool) -> SettingsPage {
        SettingsPage::new(
            SettingsScope::Provider(ProviderId::new(id).unwrap()),
            vec![SettingsSection::new(
                id,
                id,
                vec![
                    SettingDescriptor::new(
                        SettingId::new(format!("providers.{id}.enabled")).unwrap(),
                        "Enabled",
                        DescriptorKind::Toggle { value: true },
                    ),
                    SettingDescriptor::new(
                        SettingId::new(format!("providers.{id}.sourceMode")).unwrap(),
                        "Source mode",
                        DescriptorKind::Choice {
                            selected: source_mode.to_string(),
                            options: vec![
                                ChoiceOption::new("auto", "Auto"),
                                ChoiceOption::new("web", "Web"),
                                ChoiceOption::new("cli", "CLI"),
                                ChoiceOption::new("oauth", "OAuth"),
                                ChoiceOption::new("api", "API"),
                            ],
                        },
                    ),
                    SettingDescriptor::new(
                        SettingId::new(format!("providers.{id}.apiKey")).unwrap(),
                        "API key",
                        DescriptorKind::Secret {
                            is_set: secret_set,
                            source: secret_set.then(|| "keychain".to_string()),
                        },
                    ),
                ],
            )],
        )
    }

    fn sample_snapshot() -> EngineSnapshot {
        let mut snap = generate_scenario_snapshot(0);
        snap.generated_at = "2026-10-06T12:48:05Z".parse().unwrap();
        snap
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

    const ALL_PANES: &[(SettingsPane, &str)] = &[
        (SettingsPane::General, "general"),
        (SettingsPane::ICloudSync, "icloud-sync"),
        (SettingsPane::UsageSpend, "usage-spend"),
        (SettingsPane::Notifications, "notifications"),
        (SettingsPane::MenuBar, "menu-bar"),
        (SettingsPane::Menu, "menu"),
        (SettingsPane::Advanced, "advanced"),
        (SettingsPane::Hooks, "hooks"),
        (SettingsPane::Plugins, "plugins"),
        (SettingsPane::About, "about"),
    ];

    fn all_panes_with_providers() -> Vec<(SettingsPane, &'static str)> {
        let mut panes = ALL_PANES.to_vec();
        panes.push((SettingsPane::Provider("codex".to_string()), "codex"));
        panes.push((SettingsPane::Provider("claude".to_string()), "claude"));
        panes.push((
            SettingsPane::Provider("antigravity".to_string()),
            "antigravity",
        ));
        panes
    }

    fn renders_settings_panes_in_light_and_dark_appearances(mtm: MainThreadMarker) {
        let app = NSApplication::sharedApplication(mtm);

        // Verify SettingsWindowController chrome and invariants.
        let controller = SettingsWindowController::new(mtm, None);
        let win = controller.window();
        assert!(!win.isReleasedWhenClosed());
        assert!(win.titlebarAppearsTransparent());
        assert!(win.autorecalculatesKeyViewLoop());
        assert_eq!(win.frameAutosaveName().to_string(), WINDOW_AUTOSAVE_NAME);
        let min_size = win.contentMinSize();
        assert_eq!(min_size.width, WINDOW_MIN_WIDTH);
        assert_eq!(min_size.height, WINDOW_MIN_HEIGHT);

        let view = controller.view();
        view.set_terminal_launcher(Rc::new(|_args| {}));
        view.apply_settings_page(&sample_app_settings());
        view.apply_settings_page(&sample_provider_settings("codex", "auto", false));
        view.apply_settings_page(&sample_provider_settings("claude", "auto", true));
        view.apply_settings_page(&sample_provider_settings("antigravity", "auto", false));
        view.apply_snapshot(sample_snapshot());

        let bounds = view.bounds();
        assert_eq!(bounds.size.width, WINDOW_WIDTH);
        assert_eq!(bounds.size.height, WINDOW_HEIGHT);

        // Verify live search field filtering and NSOutlineView row counts.
        let outline = view.sidebar_outline_view().expect("sidebar NSOutlineView");
        // 10 app panes + 1 Providers header + 3 providers (Claude, Codex, Antigravity) = 14 rows
        assert_eq!(outline.numberOfRows(), 14);
        view.set_search_text("claude");
        // 10 app panes + 1 Providers header + 1 matching provider = 12 rows
        assert_eq!(outline.numberOfRows(), 12);
        view.set_search_text("no-such-provider");
        // 10 app panes + 1 Providers header + 1 empty filter state row = 12 rows
        assert_eq!(outline.numberOfRows(), 12);
        view.set_search_text("");
        assert_eq!(outline.numberOfRows(), 14);

        // Verify native AppKit controls across Menu and Provider panes.
        view.select_pane(SettingsPane::Menu);
        let menu_classes = view.descendant_class_names();
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
                menu_classes.iter().any(|c| c.contains(required)),
                "expected native AppKit control `{required}` in Menu pane, found {menu_classes:?}"
            );
        }

        view.select_pane(SettingsPane::Provider("codex".to_string()));
        let provider_classes = view.descendant_class_names();
        assert!(
            !provider_classes.iter().any(|c| c.contains("NSSwitch")),
            "expected no `NSSwitch` in Provider pane (header Enabled and row toggles are checkboxes), found {provider_classes:?}"
        );
        for required in [
            "NSPopUpButton",
            "NSButton",
            "NSSegmentedControl",
            "NSColorWell",
            "NSTextField",
        ] {
            assert!(
                provider_classes.iter().any(|c| c.contains(required)),
                "expected native AppKit control `{required}` in Provider pane, found {provider_classes:?}"
            );
        }

        let capture_dir = std::env::var_os("ARIADUSAGE_SETTINGS_CAPTURE_DIR").map(PathBuf::from);
        let cbcap_bin = std::env::var_os("ARIADUSAGE_CBCAP_BIN").map(PathBuf::from);
        if cbcap_bin.is_some() {
            controller.show_window(Some(SettingsPane::General));
            pump_runloop(Duration::from_millis(80));
        }

        let panes = all_panes_with_providers();

        if let Some(base) = &capture_dir {
            let _ = std::fs::remove_file(base.join("cursor.png"));
            for mode in ["dark", "light"] {
                let _ = std::fs::remove_file(base.join(mode).join("cursor.png"));
                let _ = std::fs::remove_file(base.join(mode).join("cursor-ax.txt"));
            }
        }

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

    // Rust-added: pixel-diff harness comparing all 13 dark-mode Settings window panes
    // rendered on fixture data against external golden capture PNG files.
    fn pixel_diff_settings_panes_against_captures(mtm: MainThreadMarker) {
        let Some(captures_dir) =
            captures_dir_from_env("pixel_diff_settings_panes_against_captures")
        else {
            return;
        };
        let diff_dir = create_diff_output_dir("settings-render");

        let app = NSApplication::sharedApplication(mtm);
        let controller = SettingsWindowController::new(mtm, None);
        let win = controller.window();
        let view = controller.view();
        view.set_terminal_launcher(Rc::new(|_args| {}));
        view.apply_settings_page(&sample_app_settings());
        view.apply_settings_page(&sample_provider_settings("codex", "auto", false));
        view.apply_settings_page(&sample_provider_settings("claude", "auto", true));
        view.apply_settings_page(&sample_provider_settings("antigravity", "auto", false));
        view.apply_snapshot(sample_snapshot());

        let dark_appearance =
            NSAppearance::appearanceNamed(&NSString::from_str("NSAppearanceNameDarkAqua"));
        app.setAppearance(dark_appearance.as_deref());
        win.setAppearance(dark_appearance.as_deref());
        view.set_appearance_dark_override(Some(true));

        let mut rows = Vec::new();
        for (pane, stem) in all_panes_with_providers() {
            view.select_pane(pane);
            pump_runloop(Duration::from_millis(30));

            let rep = view.render_bitmap().expect("bitmap representation");
            let actual = RgbaImage::from_bitmap_rep(&rep);
            let rel_path = format!("dark/settings/{stem}.png");
            let expected =
                RgbaImage::from_png_file(&captures_dir.join(&rel_path)).expect("load capture PNG");
            rows.push(compare_and_write_diff(
                &format!("settings/{stem}"),
                &rel_path,
                &actual,
                &expected,
                &diff_dir,
            ));
        }

        report_and_assert_suite(
            "pixel_diff_settings_panes_against_captures",
            &diff_dir,
            &rows,
        );
    }
}
