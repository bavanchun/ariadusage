// Ported from CodexBar Tests/CodexBarTests/AntigravityStatusProbeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use ariadusage_core::model::NamedRateWindow;
use ariadusage_engine::providers::antigravity::*;
use jiff::Timestamp;

// upstream: AntigravityStatusProbeTests.swift:791 claude gpt pool can use thinking variants
#[test]
fn claude_gpt_pool_can_use_thinking_variants() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new("Claude Thinking", "claude-thinking", Some(0.7), None, None),
            AntigravityModelQuota::new("Claude Sonnet 4", "claude-sonnet-4", Some(0.3), None, None),
        ],
        None,
        None,
    );
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        30.
    );
    assert!(usage.primary.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:816 claude gpt pool uses thinking model when it is the only claude option
#[test]
fn claude_gpt_pool_uses_thinking_model_when_it_is_the_only_claude_option() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new("Claude Thinking", "claude-thinking", Some(0.7), None, None),
            AntigravityModelQuota::new(
                "Gemini 3 Pro Low",
                "gemini-3-pro-low",
                Some(0.4),
                None,
                None,
            ),
        ],
        None,
        None,
    );
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        40.
    );
    assert_eq!(
        usage
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        70.
    );
}

// upstream: AntigravityStatusProbeTests.swift:841 gemini pool unavailable when only excluded variants exist
#[test]
fn gemini_pool_unavailable_when_only_excluded_variants_exist() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini Pro Lite",
                "gemini-3-pro-lite",
                Some(0.6),
                None,
                None,
            ),
            AntigravityModelQuota::new("Claude Sonnet 4", "claude-sonnet-4", Some(0.3), None, None),
        ],
        None,
        None,
    );
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        30.
    );
    assert!(usage.primary.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:866 gemini pool chooses most constrained pro variant
#[test]
fn gemini_pool_chooses_most_constrained_pro_variant() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new("Gemini 3 Pro", "gemini-3-pro", Some(0.9), None, None),
            AntigravityModelQuota::new(
                "Gemini 3 Pro Low",
                "gemini-3-pro-low",
                Some(0.4),
                None,
                None,
            ),
        ],
        None,
        None,
    );
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        40.
    );
    assert!(usage.secondary.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:891 gemini pool chooses standard pro when it is more constrained than low variant
#[test]
fn gemini_pool_chooses_standard_pro_when_it_is_more_constrained_than_low_variant() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new("Gemini 3 Pro", "gemini-3-pro", Some(0.1), None, None),
            AntigravityModelQuota::new(
                "Gemini 3 Pro Low",
                "gemini-3-pro-low",
                Some(0.9),
                None,
                None,
            ),
        ],
        None,
        None,
    );
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        10.
    );
    assert!(usage.secondary.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:916 gemini pool ignores reset only placeholder when remaining data exists
#[test]
fn gemini_pool_ignores_reset_only_placeholder_when_remaining_data_exists() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini 3 Pro (Low)",
                "MODEL_PLACEHOLDER_M36",
                None,
                Some(Timestamp::from_second(1970).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro (High)",
                "MODEL_PLACEHOLDER_M37",
                Some(1.0),
                Some(Timestamp::from_second(1970).unwrap()),
                None,
            ),
        ],
        None,
        None,
    );
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        100.
    );
    assert!(usage.secondary.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:941 gemini pool does not fallback to lite flash variant
#[test]
fn gemini_pool_does_not_fallback_to_lite_flash_variant() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini 2 Flash Lite",
                "gemini-2-flash-lite",
                Some(0.2),
                None,
                None,
            ),
            AntigravityModelQuota::new("Claude Sonnet 4", "claude-sonnet-4", Some(0.3), None, None),
        ],
        None,
        None,
    );
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        30.
    );
    assert!(usage.tertiary.is_none());
    assert!(usage.primary.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:967 falls back to labels when model ids are placeholders
#[test]
fn falls_back_to_labels_when_model_ids_are_placeholders() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Claude Sonnet 4.6",
                "MODEL_PLACEHOLDER_M35",
                Some(0.3),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3.1 Pro (Low)",
                "MODEL_PLACEHOLDER_M36",
                Some(0.4),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Flash",
                "MODEL_PLACEHOLDER_M47",
                Some(1.0),
                None,
                None,
            ),
        ],
        None,
        None,
    );
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        40.
    );
    assert_eq!(
        usage
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        30.
    );
    assert!(usage.tertiary.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:999 matches remote antigravity model names with parentheses
#[test]
fn matches_remote_antigravity_model_names_with_parentheses() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Claude Opus 4.6 (Thinking)",
                "MODEL_PLACEHOLDER_M50",
                Some(1.0),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Claude Sonnet 4.6 (Thinking)",
                "MODEL_PLACEHOLDER_M51",
                Some(1.0),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro (High)",
                "MODEL_PLACEHOLDER_M52",
                Some(1.0),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro (Low)",
                "MODEL_PLACEHOLDER_M53",
                Some(1.0),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Flash",
                "MODEL_PLACEHOLDER_M54",
                Some(1.0),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "GPT-OSS 120B (Medium)",
                "MODEL_PLACEHOLDER_M55",
                Some(1.0),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
        ],
        Some("user@example.com".into()),
        Some("Pro".into()),
    );
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        100.
    );
    assert_eq!(
        usage
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        100.
    );
    assert!(usage.tertiary.is_none());
    assert_eq!(
        usage.identity.as_ref().unwrap().account_email.as_deref(),
        Some("user@example.com")
    );
}

// upstream: AntigravityStatusProbeTests.swift:1053 known model quota rows collapse into two usage pools
#[test]
fn known_model_quota_rows_collapse_into_two_usage_pools() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "GPT-OSS 120B (Medium)",
                "MODEL_PLACEHOLDER_M55",
                Some(0.25),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                Some("tomorrow".into()),
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro (Low)",
                "MODEL_PLACEHOLDER_M53",
                Some(0.5),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Claude Opus 4.6 (Thinking)",
                "MODEL_PLACEHOLDER_M50",
                Some(0.75),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro (High)",
                "MODEL_PLACEHOLDER_M52",
                Some(1.0),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
        ],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Local);
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        50.
    );
    assert_eq!(
        usage
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        25.
    );
    assert!(usage.extra_rate_windows.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:1093 model without remaining fraction stays out of family summary and preserves reset metadata
#[test]
fn model_without_remaining_fraction_stays_out_of_family_summary_and_preserves_reset_metadata() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini 3.1 Pro (Low)",
                "MODEL_PLACEHOLDER_M36",
                None,
                Some(Timestamp::from_second(1_735_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Flash",
                "MODEL_PLACEHOLDER_M47",
                Some(1.0),
                None,
                None,
            ),
        ],
        None,
        None,
    );
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        100.
    );
    assert!(usage.secondary.is_none());
    assert!(usage.tertiary.is_none());
    assert!(usage.extra_rate_windows.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:1121 group without remaining fraction preserves reset metadata as unavailable grouped window
#[test]
fn group_without_remaining_fraction_preserves_reset_metadata_as_unavailable_grouped_window() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![AntigravityModelQuota::new(
            "Gemini 3.1 Pro (Low)",
            "MODEL_PLACEHOLDER_M36",
            None,
            Some(Timestamp::from_second(1_735_000_000).unwrap()),
            None,
        )],
        None,
        None,
    );
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert!(usage.primary.is_none());
    let model_window = usage.extra_rate_windows.as_ref().unwrap().first().unwrap();
    assert_eq!(model_window.id, "antigravity-gemini");
    assert_eq!(model_window.title, "Gemini Models");
    assert_eq!(
        model_window.window.resets_at,
        Some(Timestamp::from_second(1_735_000_000).unwrap())
    );
    assert!(!model_window.usage_known);
}

// upstream: AntigravityStatusProbeTests.swift:1166 filtered variants stay out of summary but remain distinct extras
#[test]
fn filtered_variants_stay_out_of_summary_but_remain_distinct_extras() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini 3 Pro Lite",
                "gemini-3-pro-lite",
                Some(0.6),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Flash Lite",
                "gemini-3-flash-lite",
                Some(0.2),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Tab Autocomplete",
                "tab_autocomplete_model",
                Some(0.9),
                None,
                None,
            ),
        ],
        Some("test@example.com".into()),
        Some("Pro".into()),
    )
    .with_source(AntigravitySnapshotSource::Local);
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert!(usage.primary.is_none());
    assert!(usage.secondary.is_none());
    assert!(usage.tertiary.is_none());
    assert_eq!(
        usage.identity.as_ref().unwrap().account_email.as_deref(),
        Some("test@example.com")
    );
    assert_eq!(
        usage.identity.as_ref().unwrap().login_method.as_deref(),
        Some("Pro")
    );
    assert_eq!(
        usage
            .extra_rate_windows
            .as_ref()
            .unwrap()
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        [
            "gemini-3-pro-lite",
            "gemini-3-flash-lite",
            "tab_autocomplete_model"
        ]
    );
}

// upstream: AntigravityStatusProbeTests.swift:1208 local source collapses opaque model ids into two usage pools
#[test]
fn local_source_collapses_opaque_model_ids_into_two_usage_pools() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Claude Sonnet 4.6 (Thinking)",
                "MODEL_PLACEHOLDER_M60",
                Some(0.8),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Claude Opus 4.6 (Thinking)",
                "MODEL_PLACEHOLDER_M61",
                Some(0.7),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3.1 Pro (High)",
                "MODEL_PLACEHOLDER_M62",
                Some(0.9),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3.1 Pro (Low)",
                "MODEL_PLACEHOLDER_M63",
                Some(0.4),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3.5 Flash (High)",
                "MODEL_PLACEHOLDER_M64",
                Some(0.6),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3.5 Flash (Low)",
                "MODEL_PLACEHOLDER_M65",
                Some(0.3),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3.5 Flash (Medium)",
                "MODEL_PLACEHOLDER_M66",
                Some(0.5),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "GPT-OSS 120B (Medium)",
                "MODEL_PLACEHOLDER_M55",
                Some(1.0),
                Some(Timestamp::from_second(1_775_000_000).unwrap()),
                None,
            ),
        ],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Local);
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        30.
    );
    assert_eq!(
        usage
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        70.
    );
    assert!(usage.extra_rate_windows.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:1274 remote source collapses recognized family models and hides unconsumed junk
#[test]
fn remote_source_collapses_recognized_family_models_and_hides_unconsumed_junk() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini 2.5 Flash Image",
                "gemini-2-5-flash-image",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Tab Flash Lite Vertex",
                "tab_flash_lite_vertex",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new("Gemini 2.5 Pro", "gemini-2-5-pro", Some(1.0), None, None),
            AntigravityModelQuota::new(
                "Gemini 3 Pro (High)",
                "gemini-3-pro-high",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 2.5 Flash Lite",
                "gemini-2-5-flash-lite",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro Image",
                "gemini-3-pro-image",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new("Gemini 3 Flash", "gemini-3-flash", Some(1.0), None, None),
            AntigravityModelQuota::new(
                "Gemini 3.1 Flash Lite",
                "gemini-3-1-flash-lite",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3.1 Pro (Low)",
                "gemini-3-1-pro-low",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3.1 Pro (High)",
                "gemini-3-1-pro-high",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Tab Jump Flash Lite Vertex",
                "tab_jump_flash_lite_vertex",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro (Low)",
                "gemini-3-pro-low",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 2.5 Flash",
                "gemini-2-5-flash",
                Some(1.0),
                None,
                None,
            ),
        ],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Remote);
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        100.
    );
    assert!(usage.secondary.is_none());
    assert!(usage.extra_rate_windows.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:1382 remote source shows consumed junk models despite filter
#[test]
fn remote_source_shows_consumed_junk_models_despite_filter() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Tab Flash Lite Vertex",
                "tab_flash_lite_vertex",
                Some(0.4),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro Image",
                "gemini-3-pro-image",
                Some(0.4),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Tab Jump Flash Lite Vertex",
                "tab_jump_flash_lite_vertex",
                Some(0.9995),
                None,
                None,
            ),
            AntigravityModelQuota::new("Gemini 3 Flash", "gemini-3-flash", Some(1.0), None, None),
        ],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Remote);
    let usage = snapshot.to_usage_snapshot().unwrap();
    let ids = usage
        .extra_rate_windows
        .as_ref()
        .unwrap()
        .iter()
        .map(|w| w.id.as_str())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"tab_flash_lite_vertex"));
    assert!(ids.contains(&"gemini-3-pro-image"));
    assert!(!ids.contains(&"tab_jump_flash_lite_vertex"));
}

// upstream: AntigravityStatusProbeTests.swift:1432 remote source image models do not drive family summary bars
#[test]
fn remote_source_image_models_do_not_drive_family_summary_bars() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini 3 Pro Image",
                "gemini-3-pro-image",
                Some(0.2),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro (High)",
                "gemini-3-pro-high",
                Some(0.9),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Flash Image",
                "gemini-3-flash-image",
                Some(0.1),
                None,
                None,
            ),
            AntigravityModelQuota::new("Gemini 3 Flash", "gemini-3-flash", Some(0.8), None, None),
        ],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Remote);
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert!(usage.secondary.is_none());
    assert_eq!(usage.primary.as_ref().unwrap().used_percent, 20.);
    assert!(
        usage
            .extra_rate_windows
            .as_ref()
            .unwrap()
            .iter()
            .any(|w| w.id == "gemini-3-pro-image")
    );
    assert!(
        usage
            .extra_rate_windows
            .as_ref()
            .unwrap()
            .iter()
            .any(|w| w.id == "gemini-3-flash-image")
    );
}

// upstream: AntigravityStatusProbeTests.swift:1473 remote source yields nil extra windows when all models are unconsumed junk
#[test]
fn remote_source_yields_nil_extra_windows_when_all_models_are_unconsumed_junk() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Tab Flash Lite Vertex",
                "tab_flash_lite_vertex",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 2.5 Flash Lite",
                "gemini-2-5-flash-lite",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro Image",
                "gemini-3-pro-image",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new("Unknown Model X", "unknown-model-x", Some(1.0), None, None),
        ],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Remote);
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert!(usage.primary.is_none());
    assert!(usage.secondary.is_none());
    assert!(usage.tertiary.is_none());
    assert!(usage.extra_rate_windows.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:1514 ordering edge cases collapse to most constrained usage pool
#[test]
fn ordering_edge_cases_collapse_to_most_constrained_usage_pool() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini 3 Pro (Low)",
                "MODEL_PLACEHOLDER_M70",
                Some(0.5),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini 3 Pro (High)",
                "MODEL_PLACEHOLDER_M71",
                Some(0.8),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Gemini Pro Experimental",
                "MODEL_PLACEHOLDER_M72",
                Some(0.3),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Claude Sonnet 4",
                "MODEL_PLACEHOLDER_M73",
                Some(0.9),
                None,
                None,
            ),
        ],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Local);
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        30.
    );
    assert_eq!(
        usage
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        90.
    );
    assert!(usage.extra_rate_windows.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:1555 nil version unknown family models sort deterministically by label
#[test]
fn nil_version_unknown_family_models_sort_deterministically_by_label() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Zebra Unknown Model",
                "MODEL_PLACEHOLDER_MA",
                Some(0.5),
                None,
                None,
            ),
            AntigravityModelQuota::new(
                "Alpha Unknown Model",
                "MODEL_PLACEHOLDER_MB",
                Some(0.5),
                None,
                None,
            ),
        ],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Local);
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage
            .extra_rate_windows
            .as_ref()
            .unwrap()
            .iter()
            .map(|w| w.title.as_str())
            .collect::<Vec<_>>(),
        ["Alpha Unknown Model", "Zebra Unknown Model"]
    );
}

// upstream: AntigravityStatusProbeTests.swift:1586 hyphenated raw model ids without display name still map to gemini group
#[test]
fn hyphenated_raw_model_ids_without_display_name_still_map_to_gemini_group() {
    let snapshot = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "gemini-3-pro-preview",
                "gemini-3-pro-preview",
                Some(1.0),
                None,
                None,
            ),
            AntigravityModelQuota::new("gemini-2.5-pro", "gemini-2.5-pro", Some(1.0), None, None),
        ],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Remote);
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        100.
    );
    assert!(usage.extra_rate_windows.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:1145 named rate windows default legacy payloads to known usage
#[test]
fn named_rate_windows_default_legacy_payloads_to_known_usage() {
    let decoded: NamedRateWindow = serde_json::from_str(
        r#"{
  "id": "legacy-window",
  "title": "Legacy Window",
  "window": {
    "usedPercent": 42,
    "windowMinutes": null,
    "resetsAt": null,
    "resetDescription": null,
    "nextRegenPercent": null
  }
}
"#,
    )
    .unwrap();
    assert!(decoded.usage_known);
}

// upstream: AntigravityStatusProbeTests.swift:1614 http probe errors still count as reachable
#[test]
fn http_probe_errors_still_count_as_reachable() {
    assert!(AntigravityStatusProbe::is_reachable_probe_error(
        &AntigravityStatusProbeError::ApiError("HTTP 403: Forbidden".into())
    ));
    assert!(AntigravityStatusProbe::is_reachable_probe_error(
        &AntigravityStatusProbeError::ApiError("HTTP 404: Not Found".into())
    ));
    assert!(!AntigravityStatusProbe::is_reachable_probe_error(
        &AntigravityStatusProbeError::ApiError("Invalid response".into())
    ));
    assert!(!AntigravityStatusProbe::is_reachable_probe_error(
        &AntigravityStatusProbeError::NotRunning
    ));
}

// upstream: AntigravityStatusProbeTests.swift:1628 fallback probe port prefers non extension candidate
#[test]
fn fallback_probe_port_prefers_non_extension_candidate() {
    assert_eq!(
        AntigravityStatusProbe::fallback_probe_port(&[51170, 61775], Some(61775)),
        Some(51170)
    );
    assert_eq!(
        AntigravityStatusProbe::fallback_probe_port(&[61775], Some(61775)),
        Some(61775)
    );
    assert_eq!(
        AntigravityStatusProbe::fallback_probe_port(&[51170, 61775], None),
        Some(51170)
    );
}

// upstream: AntigravityStatusProbeTests.swift:677 parses user status response
#[test]
fn parses_user_status_response() {
    let snapshot = AntigravityStatusProbe::parse_user_status_response(
        r#"{
  "code": 0,
  "userStatus": {
    "email": "test@example.com",
    "planStatus": {
      "planInfo": {
        "planName": "Pro"
      }
    },
    "cascadeModelConfigData": {
      "clientModelConfigs": [
        {
          "label": "Claude 3.5 Sonnet",
          "modelOrAlias": { "model": "claude-3-5-sonnet" },
          "quotaInfo": { "remainingFraction": 0.5, "resetTime": "2025-12-24T10:00:00Z" }
        },
        {
          "label": "Gemini Pro Low",
          "modelOrAlias": { "model": "gemini-pro-low" },
          "quotaInfo": { "remainingFraction": 0.8, "resetTime": "2025-12-24T11:00:00Z" }
        },
        {
          "label": "Gemini Flash",
          "modelOrAlias": { "model": "gemini-flash" },
          "quotaInfo": { "remainingFraction": 0.2, "resetTime": "2025-12-24T12:00:00Z" }
        }
      ]
    }
  }
}
"#
        .as_bytes(),
    )
    .unwrap();
    assert_eq!(snapshot.account_email.as_deref(), Some("test@example.com"));
    assert_eq!(snapshot.account_plan.as_deref(), Some("Pro"));
    assert_eq!(snapshot.model_quotas.len(), 3);
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        20.
    );
    assert_eq!(
        usage
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        50.
    );
    assert!(usage.tertiary.is_none());
}

// upstream: AntigravityStatusProbeTests.swift:727 prefers user tier name over generic plan info
#[test]
fn prefers_user_tier_name_over_generic_plan_info() {
    let snapshot = AntigravityStatusProbe::parse_user_status_response(
        r#"{
  "code": 0,
  "userStatus": {
    "email": "ultra@example.com",
    "userTier": {
      "id": "google_ai_ultra",
      "name": "Google AI Ultra",
      "description": "Ultra tier"
    },
    "planStatus": {
      "planInfo": {
        "planName": "Pro"
      }
    },
    "cascadeModelConfigData": {
      "clientModelConfigs": []
    }
  }
}
"#
        .as_bytes(),
    )
    .unwrap();
    assert_eq!(snapshot.account_email.as_deref(), Some("ultra@example.com"));
    assert_eq!(snapshot.account_plan.as_deref(), Some("Google AI Ultra"));
    assert!(snapshot.model_quotas.is_empty());
}

// upstream: AntigravityStatusProbeTests.swift:759 falls back to plan info when user tier name is blank
#[test]
fn falls_back_to_plan_info_when_user_tier_name_is_blank() {
    let snapshot = AntigravityStatusProbe::parse_user_status_response(
        r#"{
  "code": 0,
  "userStatus": {
    "email": "fallback@example.com",
    "userTier": {
      "id": "google_ai_ultra",
      "name": "   ",
      "description": "Ultra tier"
    },
    "planStatus": {
      "planInfo": {
        "planName": "Pro"
      }
    },
    "cascadeModelConfigData": {
      "clientModelConfigs": []
    }
  }
}
"#
        .as_bytes(),
    )
    .unwrap();
    assert_eq!(
        snapshot.account_email.as_deref(),
        Some("fallback@example.com")
    );
    assert_eq!(snapshot.account_plan.as_deref(), Some("Pro"));
    assert!(snapshot.model_quotas.is_empty());
}
#[allow(dead_code)]
#[path = "support/antigravity-quota.rs"]
mod quota;

// upstream: AntigravityStatusProbeTests.swift:208 local snapshot score prefers quota summary over legacy model quotas
#[test]
fn local_snapshot_score_prefers_quota_summary_over_legacy_model_quotas() {
    let legacy = AntigravityStatusSnapshot::new(
        vec![
            AntigravityModelQuota::new(
                "Gemini 3 Pro Low",
                "gemini-3-pro-low",
                Some(0.9),
                Some(Timestamp::from_second(1970).unwrap()),
                None,
            ),
            AntigravityModelQuota::new(
                "Claude Sonnet",
                "claude-sonnet",
                Some(0.5),
                Some(Timestamp::from_second(1970).unwrap()),
                None,
            ),
        ],
        Some("user@example.com".into()),
        Some("Pro".into()),
    )
    .with_source(AntigravitySnapshotSource::Local);
    let summary = AntigravityStatusProbe::parse_quota_summary_response(
        quota::quota_summary(0.91, 0.82, 0.73, 0.64).as_bytes(),
    )
    .unwrap()
    .with_identity(Some(&legacy));
    assert!(
        AntigravityStatusProbe::local_snapshot_score(&summary)
            > AntigravityStatusProbe::local_snapshot_score(&legacy)
    );
}
