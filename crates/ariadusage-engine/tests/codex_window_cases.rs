// Ported from CodexBar Tests/CodexBarTests/CodexCLIWindowNormalizationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::{
    identity::ProviderIdentitySnapshot, snapshot::UsageSnapshot, window::RateWindow,
};
use ariadusage_engine::providers::codex::*;
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::Timestamp;
fn now() -> Timestamp {
    Timestamp::now()
}
fn window(used: f64, minutes: i64) -> RateWindow {
    RateWindow::new(used, Some(minutes), None, None, None, false).unwrap()
}
fn status(five: Option<i64>, weekly: Option<i64>) -> CodexStatusSnapshot {
    CodexStatusSnapshot {
        credits: None,
        codex_credit_limit: None,
        five_hour_percent_left: five,
        weekly_percent_left: weekly,
        five_hour_reset_description: five.map(|_| "resets soon".into()),
        weekly_reset_description: weekly.map(|_| "resets next week".into()),
        five_hour_resets_at: None,
        weekly_resets_at: None,
        raw_text: if five.is_some() {
            "5h limit: 69% left".into()
        } else if weekly.is_some() {
            "Weekly limit: 95% left".into()
        } else {
            String::new()
        },
    }
}

// upstream: CodexCLIWindowNormalizationTests.swift:6 normalizer maps lone weekly window into secondary
#[test]
fn normalizer_maps_lone_weekly_window_into_secondary() {
    let (primary, secondary) = CodexRateWindowNormalizer::normalize(Some(window(5.0, 10080)), None);
    assert!(primary.is_none());
    assert_eq!(secondary.as_ref().map(|w| w.used_percent), Some(5.0));
    assert_eq!(secondary.unwrap().window_minutes, Some(10080));
}

// upstream: CodexCLIWindowNormalizationTests.swift:20 normalizer keeps lone session window in primary
#[test]
fn normalizer_keeps_lone_session_window_in_primary() {
    let (primary, secondary) = CodexRateWindowNormalizer::normalize(Some(window(31.0, 300)), None);
    assert_eq!(primary.as_ref().map(|w| w.used_percent), Some(31.0));
    assert_eq!(primary.unwrap().window_minutes, Some(300));
    assert!(secondary.is_none());
}

// upstream: CodexCLIWindowNormalizationTests.swift:34 normalizer keeps session and weekly ordering unchanged
#[test]
fn normalizer_keeps_session_and_weekly_ordering_unchanged() {
    let (primary, secondary) =
        CodexRateWindowNormalizer::normalize(Some(window(31.0, 300)), Some(window(26.0, 10080)));
    assert_eq!(primary.as_ref().map(|w| w.used_percent), Some(31.0));
    assert_eq!(primary.unwrap().window_minutes, Some(300));
    assert_eq!(secondary.as_ref().map(|w| w.used_percent), Some(26.0));
    assert_eq!(secondary.unwrap().window_minutes, Some(10080));
}

// upstream: CodexCLIWindowNormalizationTests.swift:54 normalizer swaps reversed weekly and unknown windows
#[test]
fn normalizer_swaps_reversed_weekly_and_unknown_windows() {
    let (primary, secondary) =
        CodexRateWindowNormalizer::normalize(Some(window(43.0, 10080)), Some(window(17.0, 540)));
    assert_eq!(primary.as_ref().map(|w| w.used_percent), Some(17.0));
    assert_eq!(primary.unwrap().window_minutes, Some(540));
    assert_eq!(secondary.as_ref().map(|w| w.used_percent), Some(43.0));
    assert_eq!(secondary.unwrap().window_minutes, Some(10080));
}

// upstream: CodexCLIWindowNormalizationTests.swift:74 maps weekly only RPC limits into secondary
#[test]
fn maps_weekly_only_rpc_limits_into_secondary() {
    let snapshot = CodexCLIUsageFetcher::map_codex_rpc_limits_for_testing(
        Some((5.0, 10080, None)),
        None,
        None,
        now(),
    )
    .unwrap();
    assert!(snapshot.primary.is_none());
    assert_eq!(
        snapshot.secondary.as_ref().map(|w| w.used_percent),
        Some(5.0)
    );
    assert_eq!(snapshot.secondary.unwrap().window_minutes, Some(10080));
}

// upstream: CodexCLIWindowNormalizationTests.swift:85 maps session only RPC limits into primary
#[test]
fn maps_session_only_rpc_limits_into_primary() {
    let snapshot = CodexCLIUsageFetcher::map_codex_rpc_limits_for_testing(
        Some((31.0, 300, None)),
        None,
        None,
        now(),
    )
    .unwrap();
    assert_eq!(
        snapshot.primary.as_ref().map(|w| w.used_percent),
        Some(31.0)
    );
    assert_eq!(snapshot.primary.unwrap().window_minutes, Some(300));
    assert!(snapshot.secondary.is_none());
}

// upstream: CodexCLIWindowNormalizationTests.swift:96 maps reversed weekly and unknown RPC limits
#[test]
fn maps_reversed_weekly_and_unknown_rpc_limits() {
    let snapshot = CodexCLIUsageFetcher::map_codex_rpc_limits_for_testing(
        Some((43.0, 10080, None)),
        Some((17.0, 540, None)),
        None,
        now(),
    )
    .unwrap();
    assert_eq!(
        snapshot.primary.as_ref().map(|w| w.used_percent),
        Some(17.0)
    );
    assert_eq!(snapshot.primary.unwrap().window_minutes, Some(540));
    assert_eq!(
        snapshot.secondary.as_ref().map(|w| w.used_percent),
        Some(43.0)
    );
    assert_eq!(snapshot.secondary.unwrap().window_minutes, Some(10080));
}

// upstream: CodexCLIWindowNormalizationTests.swift:108 throws when RPC limits contain no windows
#[test]
fn throws_when_rpc_limits_contain_no_windows() {
    assert!(matches!(
        CodexCLIUsageFetcher::map_codex_rpc_limits_for_testing(None, None, None, now()),
        Err(UsageError::NoRateLimitsFound)
    ));
}

// upstream: CodexCLIWindowNormalizationTests.swift:115 maps plan only RPC limits into empty identified snapshot
#[test]
fn maps_plan_only_rpc_limits_into_empty_identified_snapshot() {
    let snapshot =
        CodexCLIUsageFetcher::map_codex_rpc_limits_for_testing(None, None, Some("pro"), now())
            .unwrap();
    assert!(snapshot.primary.is_none());
    assert!(snapshot.secondary.is_none());
    assert_eq!(
        snapshot.identity.as_ref().unwrap().login_method.as_deref(),
        Some("pro")
    );
    assert_eq!(
        UsageLimitsAvailability::resolve(
            ProviderId::new("codex").unwrap(),
            Some(&snapshot),
            None,
            None
        ),
        UsageLimitsAvailability::Unavailable
    );
}

// upstream: CodexCLIWindowNormalizationTests.swift:128 codex no rate limit error means limits unavailable without snapshot
#[test]
fn codex_no_rate_limit_error_means_limits_unavailable_without_snapshot() {
    let account = AccountInfo {
        email: Some("user@example.com".into()),
        plan: None,
    };
    assert_eq!(
        UsageLimitsAvailability::resolve(
            ProviderId::new("codex").unwrap(),
            None,
            Some(&account),
            Some(&UsageError::NoRateLimitsFound.to_string())
        ),
        UsageLimitsAvailability::Unavailable
    );
}

// upstream: CodexCLIWindowNormalizationTests.swift:139 codex no rate limit error stays available without account context
#[test]
fn codex_no_rate_limit_error_stays_available_without_account_context() {
    let account = AccountInfo {
        email: None,
        plan: None,
    };
    assert_eq!(
        UsageLimitsAvailability::resolve(
            ProviderId::new("codex").unwrap(),
            None,
            Some(&account),
            Some(&UsageError::NoRateLimitsFound.to_string())
        ),
        UsageLimitsAvailability::Available
    );
}

// upstream: CodexCLIWindowNormalizationTests.swift:150 codex windowed snapshot wins over stale no rate limit error
#[test]
fn codex_windowed_snapshot_wins_over_stale_no_rate_limit_error() {
    let identity = ProviderIdentitySnapshot::new(
        Some(ProviderId::new("codex").unwrap()),
        Some("user@example.com".into()),
        None,
        Some("pro".into()),
        None,
    );
    let snapshot = UsageSnapshot::new(
        Some(window(25., 300)),
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        now(),
        Some(identity),
        Confidence::Exact,
    )
    .unwrap();
    assert_eq!(
        UsageLimitsAvailability::resolve(
            ProviderId::new("codex").unwrap(),
            Some(&snapshot),
            None,
            Some(&UsageError::NoRateLimitsFound.to_string())
        ),
        UsageLimitsAvailability::Available
    );
}

// upstream: CodexCLIWindowNormalizationTests.swift:174 maps weekly only status snapshot into secondary
#[test]
fn maps_weekly_only_status_snapshot_into_secondary() {
    let snapshot =
        CodexCLIUsageFetcher::map_codex_status_for_testing(&status(None, Some(95)), now()).unwrap();
    assert!(snapshot.primary.is_none());
    assert_eq!(snapshot.secondary.as_ref().unwrap().used_percent, 5.);
    assert_eq!(snapshot.secondary.unwrap().window_minutes, Some(10080));
}

// upstream: CodexCLIWindowNormalizationTests.swift:192 maps five hour only status snapshot into primary
#[test]
fn maps_five_hour_only_status_snapshot_into_primary() {
    let snapshot =
        CodexCLIUsageFetcher::map_codex_status_for_testing(&status(Some(69), None), now()).unwrap();
    assert_eq!(snapshot.primary.as_ref().unwrap().used_percent, 31.);
    assert_eq!(snapshot.primary.unwrap().window_minutes, Some(300));
    assert!(snapshot.secondary.is_none());
}

// upstream: CodexCLIWindowNormalizationTests.swift:210 throws when status snapshot contains no windows
#[test]
fn throws_when_status_snapshot_contains_no_windows() {
    assert!(matches!(
        CodexCLIUsageFetcher::map_codex_status_for_testing(&status(None, None), now()),
        Err(UsageError::NoRateLimitsFound)
    ));
}
