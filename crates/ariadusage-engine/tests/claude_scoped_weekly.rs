// Ported from CodexBar Tests/CodexBarTests/ClaudeScopedWeeklyLimitMapperTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_engine::providers::claude::{ClaudeScopedWeeklyLimitMapper, OAuthScopedLimitEntry};
use jiff::Timestamp;
use serde_json::json;

fn make_limit(
    kind: &str,
    group: &str,
    percent: f64,
    model_id: Option<&str>,
    model_name: Option<&str>,
    resets_at: Option<Timestamp>,
) -> OAuthScopedLimitEntry {
    OAuthScopedLimitEntry {
        kind: Some(kind.to_owned()),
        group: Some(group.to_owned()),
        percent: Some(percent),
        resets_at: resets_at.map(|ts| ts.to_string()),
        model_id: model_id.map(str::to_owned),
        model_name: model_name.map(str::to_owned),
    }
}

#[test]
fn model_id_provides_a_stable_safe_identifier_and_duplicate_limits_collapse() {
    let reset = Timestamp::from_second(1_783_507_200).unwrap();
    let limits = vec![
        make_limit(
            "weekly_scoped",
            "weekly",
            5.0,
            Some("claude/fable.5:promo"),
            Some("Fable"),
            Some(reset),
        ),
        make_limit(
            "weekly_scoped",
            "weekly",
            5.0,
            Some("claude/fable.5:promo"),
            Some("Fable renamed"),
            Some(reset),
        ),
    ];

    let windows = ClaudeScopedWeeklyLimitMapper::named_windows_with_reset_description(
        &limits,
        Some(&|_ts| "Jul 8".to_owned()),
    );
    assert_eq!(windows.len(), 1);
    let window = &windows[0];
    assert_eq!(
        window.id.as_str(),
        "claude-weekly-scoped-claude-fable-5-promo"
    );
    assert_eq!(window.title, "Fable only");
    assert_eq!(window.window.resets_at, Some(reset));
    assert_eq!(window.window.reset_description.as_deref(), Some("Jul 8"));
}

#[test]
fn display_name_supplies_the_identifier_when_the_api_omits_a_model_id() {
    let limits = vec![make_limit(
        "weekly_scoped",
        "weekly",
        5.0,
        Some("  "),
        Some(" Team / Research "),
        None,
    )];

    let windows = ClaudeScopedWeeklyLimitMapper::named_windows(&limits);
    assert_eq!(windows.len(), 1);
    let window = &windows[0];
    assert_eq!(window.id.as_str(), "claude-weekly-scoped-team-research");
    assert_eq!(window.title, "Team / Research only");
}

#[test]
fn unrelated_malformed_and_unnamed_limits_are_ignored() {
    let limits = vec![
        make_limit("session", "weekly", 5.0, None, Some("Fable"), None),
        make_limit("weekly_scoped", "monthly", 5.0, None, Some("Fable"), None),
        make_limit(
            "weekly_scoped",
            "weekly",
            f64::NAN,
            None,
            Some("Fable"),
            None,
        ),
        make_limit("weekly_scoped", "weekly", 5.0, None, Some("  "), None),
    ];

    assert!(ClaudeScopedWeeklyLimitMapper::named_windows(&limits).is_empty());
}

#[test]
fn all_models_scope_stays_in_the_primary_weekly_lane() {
    let limits = vec![
        make_limit(
            "weekly_scoped",
            "weekly",
            5.0,
            None,
            Some("All models"),
            None,
        ),
        make_limit(
            "weekly_scoped",
            "weekly",
            5.0,
            Some("claude/all_models"),
            Some("Weekly"),
            None,
        ),
        make_limit("weekly_scoped", "weekly", 5.0, None, Some("Fable"), None),
    ];

    let windows = ClaudeScopedWeeklyLimitMapper::named_windows(&limits);
    assert_eq!(
        windows.iter().map(|w| w.title.as_str()).collect::<Vec<_>>(),
        vec!["Fable only"]
    );
}

#[test]
fn lossy_array_parser_skips_non_object_elements_without_failing() {
    let raw = json!([
        "not-an-object",
        123,
        {
            "kind": "weekly_scoped",
            "group": "weekly",
            "percent": 15,
            "scope": { "model": { "display_name": "Sonnet" } }
        }
    ]);

    let entries = OAuthScopedLimitEntry::parse_lossy_array(&raw);
    let windows = ClaudeScopedWeeklyLimitMapper::named_windows(&entries);
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].id.as_str(), "claude-weekly-scoped-sonnet");
    assert_eq!(windows[0].title, "Sonnet only");
    assert_eq!(windows[0].window.used_percent, 15.0);
}
