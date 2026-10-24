// Ported from CodexBar Tests/CodexBarTests/AntigravityModelLabelTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityGeminiRenameTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityCompactFallbackTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use ariadusage_core::model::UsageSnapshot;
use ariadusage_engine::providers::antigravity::*;
use jiff::Timestamp;
use std::time::Duration;

// upstream: AntigravityModelLabelTests.swift:5 humanizes raw model ids when label matches model id
#[test]
fn humanizes_raw_model_ids_when_label_matches_model_id() {
    for (id, expected) in [
        ("gemini-3-pro-preview", "Gemini 3 Pro Preview"),
        ("gemini-2.5-flash", "Gemini 2.5 Flash"),
        ("example-3-1-pro-low", "Example 3.1 Pro Low"),
        ("gpt-api-oss", "GPT API OSS"),
    ] {
        assert_eq!(AntigravityStatusSnapshot::humanized_model_id(id), expected);
    }
    assert!(AntigravityStatusSnapshot::humanized_model_id("").is_empty());
}

// upstream: AntigravityModelLabelTests.swift:14 preserves custom model labels
#[test]
fn preserves_custom_model_labels() {
    let quota = AntigravityModelQuota::new(
        "Custom enterprise label",
        "gemini-3-pro-preview",
        Some(1.),
        None,
        None,
    );
    assert_eq!(
        AntigravityStatusSnapshot::quota_display_label(&quota),
        "Custom enterprise label"
    );
}

// upstream: AntigravityModelLabelTests.swift:26 retired flash ids canonicalize to current flash
#[test]
fn retired_flash_ids_canonicalize_to_current_flash() {
    for (id, expected) in [
        ("gemini-3.6-flash", "gemini-3.7-flash"),
        ("gemini-3.6-flash-high", "gemini-3.7-flash"),
        ("GEMINI-3.6-FLASH", "gemini-3.7-flash"),
        ("gemini-3.5-flash-mid", "gemini-3.7-flash"),
        ("gemini-3-flash-agent", "gemini-3.7-flash"),
        ("gemini-3.7-flash", "gemini-3.7-flash"),
        ("claude-sonnet-4-6", "claude-sonnet-4-6"),
    ] {
        assert_eq!(AntigravityStatusSnapshot::canonical_model_id(id), expected);
    }
}

// upstream: AntigravityModelLabelTests.swift:37 humanizes retired flash ids via canonical
#[test]
fn humanizes_retired_flash_ids_via_canonical() {
    assert_eq!(
        AntigravityStatusSnapshot::humanized_model_id("gemini-3.6-flash-high"),
        "Gemini 3.7 Flash"
    );
    let quota = AntigravityModelQuota::new(
        "gemini-3.6-flash",
        "gemini-3.6-flash",
        Some(0.5),
        None,
        None,
    );
    assert_eq!(
        AntigravityStatusSnapshot::quota_display_label(&quota),
        "Gemini 3.7 Flash"
    );
}

// upstream: AntigravityGeminiRenameTests.swift:9 language server under renamed gemini app path classifies as app
#[test]
fn language_server_under_renamed_gemini_app_path_classifies_as_app() {
    assert_eq!(
        AntigravityStatusProbe::antigravity_process_kind(
            "/applications/gemini.app/contents/resources/bin/language_server --csrf_token abc"
        ),
        Some(AntigravityProcessKind::App)
    );
}

// upstream: AntigravityGeminiRenameTests.swift:15 gemini desktop main binary does not classify as language server
#[test]
fn gemini_desktop_main_binary_does_not_classify_as_language_server() {
    assert_eq!(
        AntigravityStatusProbe::antigravity_process_kind(
            "/Applications/Gemini.app/Contents/MacOS/Gemini"
        ),
        None
    );
}

// upstream: AntigravityGeminiRenameTests.swift:22 unrelated gemini cli process does not classify
#[test]
fn unrelated_gemini_cli_process_does_not_classify() {
    assert_eq!(
        AntigravityStatusProbe::antigravity_process_kind(
            "node /usr/local/lib/node_modules/@google/gemini-cli/dist/index.js"
        ),
        None
    );
}

// upstream: AntigravityGeminiRenameTests.swift:29 language server under lookalike app name does not classify
#[test]
fn language_server_under_lookalike_app_name_does_not_classify() {
    assert_eq!(
        AntigravityStatusProbe::antigravity_process_kind(
            "/applications/notgemini.app/contents/resources/bin/language_server --csrf_token abc"
        ),
        None
    );
}
fn unclassified(fraction: f64, source: AntigravitySnapshotSource) -> UsageSnapshot {
    AntigravityStatusSnapshot::new(
        vec![AntigravityModelQuota::new(
            "Experimental Model",
            "MODEL_PLACEHOLDER_NEW",
            Some(fraction),
            None,
            None,
        )],
        None,
        None,
    )
    .with_source(source)
    .to_usage_snapshot()
    .unwrap()
}

// upstream: AntigravityCompactFallbackTests.swift:6 model quota reset proximity does not imply window duration
#[test]
fn model_quota_reset_proximity_does_not_imply_window_duration() {
    let reset = Timestamp::now()
        .checked_add(Duration::from_secs(2 * 60 * 60))
        .unwrap();
    let usage = AntigravityStatusSnapshot::new(
        vec![AntigravityModelQuota::new(
            "Gemini 3.1 Pro (Low)",
            "MODEL_PLACEHOLDER_M36",
            Some(0.5),
            Some(reset),
            None,
        )],
        None,
        None,
    )
    .to_usage_snapshot()
    .unwrap();
    assert_eq!(usage.primary.as_ref().unwrap().window_minutes, None);
    assert_eq!(usage.primary.as_ref().unwrap().resets_at, Some(reset));
}

// upstream: AntigravityCompactFallbackTests.swift:27 local unclassified model remains available as compact fallback
#[test]
fn local_unclassified_model_remains_available_as_compact_fallback() {
    let usage = unclassified(0.36, AntigravitySnapshotSource::Local);
    assert!(usage.primary.is_none());
    assert!(usage.secondary.is_none());
    assert!(usage.tertiary.is_none());
    let rows = usage.extra_rate_windows.unwrap();
    assert_eq!(
        rows.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
        ["antigravity-compact-fallback-MODEL_PLACEHOLDER_NEW"]
    );
    assert_eq!(
        rows.iter().map(|w| w.title.as_str()).collect::<Vec<_>>(),
        ["Experimental Model"]
    );
    assert_eq!(
        rows.iter()
            .map(|w| w.window.used_percent)
            .collect::<Vec<_>>(),
        [64.]
    );
}

// upstream: AntigravityCompactFallbackTests.swift:52 remote unclassified model remains detail only
#[test]
fn remote_unclassified_model_remains_detail_only() {
    let usage = unclassified(0.36, AntigravitySnapshotSource::Remote);
    assert!(usage.primary.is_none());
    assert!(usage.secondary.is_none());
    assert_eq!(
        usage
            .extra_rate_windows
            .unwrap()
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        ["MODEL_PLACEHOLDER_NEW"]
    );
}

// upstream: AntigravityCompactFallbackTests.swift:74 fully unused local model remains available as compact fallback
#[test]
fn fully_unused_local_model_remains_available_as_compact_fallback() {
    let usage = unclassified(1., AntigravitySnapshotSource::Local);
    let rows = usage.extra_rate_windows.unwrap();
    assert_eq!(
        rows.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
        ["antigravity-compact-fallback-MODEL_PLACEHOLDER_NEW"]
    );
    assert_eq!(
        rows.iter()
            .map(|w| w.window.used_percent)
            .collect::<Vec<_>>(),
        [0.]
    );
}
