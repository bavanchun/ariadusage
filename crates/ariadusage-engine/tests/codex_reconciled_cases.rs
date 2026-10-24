// Ported from CodexBar Tests/CodexBarTests/CodexbarTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::{snapshot::UsageSnapshot, window::RateWindow};
use ariadusage_engine::providers::codex::*;
use ariadusage_protocol::Confidence;
use jiff::Timestamp;
use serde_json::json;
use std::{collections::BTreeMap, fs};
use tempfile::TempDir;
fn now() -> Timestamp {
    Timestamp::from_second(1_800_000_000).unwrap()
}
fn window(used: f64, minutes: i64, reset: Option<i64>) -> RateWindow {
    RateWindow::new(
        used,
        Some(minutes),
        reset.map(|r| Timestamp::from_second(r).unwrap()),
        None,
        None,
        false,
    )
    .unwrap()
}
fn snapshot(
    primary: Option<RateWindow>,
    secondary: Option<RateWindow>,
    date: Timestamp,
) -> UsageSnapshot {
    UsageSnapshot::new(
        primary,
        secondary,
        None,
        None,
        None,
        vec![],
        None,
        None,
        date,
        None,
        Confidence::Exact,
    )
    .unwrap()
}
fn base64url_no_pad(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0x3f) as usize] as char);
        }
    }
    out
}

// upstream: CodexbarTests.swift:636 codex icon promotes weekly only window into primary display lane
#[test]
fn codex_icon_promotes_weekly_only_window_into_primary_display_lane() {
    let snapshot = snapshot(None, Some(window(25., 10080, None)), Timestamp::now());
    let (primary, secondary) = icon_windows(&snapshot, Timestamp::now());
    assert_eq!(primary.map(|w| w.remaining_percent()), Some(75.));
    assert!(secondary.is_none());
}

// upstream: CodexbarTests.swift:649 codex icon uses semantic projection lanes when durations drift
#[test]
fn codex_icon_uses_semantic_projection_lanes_when_durations_drift() {
    let snapshot = snapshot(None, Some(window(25., 11040, None)), Timestamp::now());
    let (primary, secondary) = icon_windows(&snapshot, Timestamp::now());
    assert_eq!(primary.map(|w| w.remaining_percent()), Some(75.));
    assert!(secondary.is_none());
}

// upstream: CodexbarTests.swift:662 codex icon caps session only until exhausted weekly lane resets
#[test]
fn codex_icon_caps_session_only_until_exhausted_weekly_lane_resets() {
    let weekly_reset = Timestamp::from_second(1_800_003_600).unwrap();
    let snapshot = snapshot(
        Some(window(1., 300, Some(1_800_001_800))),
        Some(window(100., 10080, Some(1_800_003_600))),
        Timestamp::from_second(1_799_992_800).unwrap(),
    );
    let (capped_primary, capped_secondary) = icon_windows(&snapshot, now());
    let (reset_primary, reset_secondary) = icon_windows(&snapshot, weekly_reset);
    assert_eq!(capped_primary.map(|w| w.remaining_percent()), Some(0.));
    assert_eq!(capped_secondary.map(|w| w.remaining_percent()), Some(0.));
    assert_eq!(reset_primary.map(|w| w.remaining_percent()), Some(99.));
    assert!(reset_secondary.is_none());
}

// upstream: CodexbarTests.swift:864 account info parses snake case auth token
#[test]
fn account_info_parses_snake_case_auth_token() {
    let home = TempDir::new().unwrap();
    let token = format!(
        "{}.{}.",
        base64url_no_pad(br#"{"alg":"none"}"#),
        base64url_no_pad(
            &serde_json::to_vec(&json!({"email":"user@example.com","chatgpt_plan_type":"pro"}))
                .unwrap()
        )
    );
    let raw = serde_json::to_vec(
        &json!({"tokens":{"id_token":token,"access_token":"access","refresh_token":"refresh"}}),
    )
    .unwrap();
    fs::write(home.path().join("auth.json"), raw).unwrap();
    let env = BTreeMap::from([(
        "CODEX_HOME".into(),
        home.path().to_string_lossy().into_owned(),
    )]);
    let account = CodexCLIUsageFetcher::new(env, Some(home.path().into()), home.path().into())
        .load_account_info();
    assert_eq!(account.email.as_deref(), Some("user@example.com"));
    assert_eq!(account.plan.as_deref(), Some("pro"));
}

// upstream: CodexbarTests.swift:887 account info parses legacy camel case auth token
#[test]
fn account_info_parses_legacy_camel_case_auth_token() {
    let home = TempDir::new().unwrap();
    let token = format!(
        "{}.{}.",
        base64url_no_pad(br#"{"alg":"none"}"#),
        base64url_no_pad(
            &serde_json::to_vec(&json!({"email":"user@example.com","chatgpt_plan_type":"pro"}))
                .unwrap()
        )
    );
    let raw = serde_json::to_vec(
        &json!({"tokens":{"idToken":token,"accessToken":"access","refreshToken":"refresh"}}),
    )
    .unwrap();
    fs::write(home.path().join("auth.json"), raw).unwrap();
    let env = BTreeMap::from([(
        "CODEX_HOME".into(),
        home.path().to_string_lossy().into_owned(),
    )]);
    let account = CodexCLIUsageFetcher::new(env, Some(home.path().into()), home.path().into())
        .load_account_info();
    assert_eq!(account.email.as_deref(), Some("user@example.com"));
    assert_eq!(account.plan.as_deref(), Some("pro"));
}
