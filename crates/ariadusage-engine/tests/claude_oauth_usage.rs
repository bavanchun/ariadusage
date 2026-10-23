// Ported from CodexBar Tests/CodexBarTests/ClaudeOAuthTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeRateLimitResetCreditsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/ClaudeOAuthDelegatedRefreshLinuxTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#[path = "support/https.rs"]
mod https;

use std::collections::BTreeMap;
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[cfg(target_os = "linux")]
use ariadusage_core::gates::delegated_cooldown::DelegatedRefreshCooldown;
use ariadusage_core::pipeline::{
    FetchContext, FetchError, FetchInteraction, FetchPhase, FetchRuntime, ProviderPlan,
};
#[cfg(target_os = "linux")]
use ariadusage_core::pipeline::{Sleeper, fetch_outcome};
use ariadusage_core::providers::SourceMode;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::net::NetConfig;
#[cfg(target_os = "linux")]
use ariadusage_engine::brokers::oauth::{
    BoxFuture, DelegatedRefreshCoordinator, DelegatedRefreshError, DelegatedRefresher,
};
use ariadusage_engine::providers::claude::{
    CLAUDE_ADMIN_API_STRATEGY_ID, CLAUDE_OAUTH_STRATEGY_ID, ClaudeAccountIdentity,
    ClaudeOAuthEndpointConfig, ClaudeOAuthFetchError, ClaudeOAuthStrategyConfig,
    ClaudeOAuthUsageFetcher, ClaudeOAuthUsageRateLimitGate, ClaudeProviderPlan,
    ClaudeSourceSelectedStrategy, ClaudeVerifiedAccountOwner, parse_retry_after,
};
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use httpmock::Method::GET;
use jiff::{SignedDuration, Timestamp};
use reqwest::Url;
use tokio_util::sync::CancellationToken;

#[test]
fn decodes_and_maps_full_oauth_usage_response_with_extra_usage_cents_to_dollars() {
    let raw = br#"{
        "five_hour": { "utilization": 18.5, "resets_at": "2026-01-15T15:00:00Z" },
        "seven_day": { "utilization": 44.0, "resets_at": "2026-01-20T00:00:00Z" },
        "seven_day_sonnet": { "utilization": 62.0, "resets_at": "2026-01-20T00:00:00Z" },
        "extra_usage": {
            "is_enabled": true,
            "monthly_limit": 20000.0,
            "used_credits": 4550.0,
            "utilization": 22.75,
            "currency": "USD"
        }
    }"#;

    let now = "2026-01-15T10:00:00Z".parse::<Timestamp>().unwrap();
    let decoded = ClaudeOAuthUsageFetcher::decode_usage_response(raw).expect("decode usage");
    let mapped = ClaudeOAuthUsageFetcher::map_usage_response(
        &decoded,
        Some("max"),
        Some("default_claude_max_5x"),
        None,
        None,
        true,
        now,
    )
    .expect("map usage");

    let snap = mapped.snapshot;
    assert_eq!(snap.primary.as_ref().map(|w| w.used_percent), Some(18.5));
    assert_eq!(
        snap.primary.as_ref().and_then(|w| w.window_minutes),
        Some(300)
    );
    assert_eq!(snap.secondary.as_ref().map(|w| w.used_percent), Some(44.0));
    assert_eq!(
        snap.secondary.as_ref().and_then(|w| w.window_minutes),
        Some(10080)
    );
    assert_eq!(snap.tertiary.as_ref().map(|w| w.used_percent), Some(62.0));

    let cost = snap.provider_cost.expect("provider cost");
    assert_eq!(cost.used, 45.50);
    assert_eq!(cost.limit, 200.0);
    assert_eq!(cost.currency_code, "USD");
    assert_eq!(cost.period.as_deref(), Some("Monthly cap"));

    let id = snap.identity.expect("identity");
    assert_eq!(id.login_method.as_deref(), Some("Claude Max 5x"));
}

#[test]
fn maps_oauth_extra_usage_minor_units_enterprise_spend_limits_and_disabled_state() {
    let now = "2026-01-15T10:00:00Z".parse::<Timestamp>().unwrap();

    // Minor units -> major units with Pro plan
    let minor_raw = br#"{
        "five_hour": { "utilization": 1, "resets_at": "2025-12-25T12:00:00.000Z" },
        "extra_usage": {
            "is_enabled": true,
            "monthly_limit": 2000,
            "used_credits": 520,
            "currency": "USD"
        }
    }"#;
    let minor_dec = ClaudeOAuthUsageFetcher::decode_usage_response(minor_raw).unwrap();
    let minor_map =
        ClaudeOAuthUsageFetcher::map_usage_response(&minor_dec, None, None, None, None, true, now)
            .unwrap();
    let minor_cost = minor_map.snapshot.provider_cost.unwrap();
    assert_eq!(minor_cost.currency_code, "USD");
    assert_eq!(minor_cost.limit, 20.0);
    assert_eq!(minor_cost.used, 5.2);
    assert_eq!(minor_cost.period.as_deref(), Some("Monthly cap"));

    // Enterprise spend limit (not 100x too high)
    let ent_raw = br#"{
        "extra_usage": {
            "is_enabled": true,
            "monthly_limit": 2000,
            "used_credits": 763,
            "utilization": 38.15,
            "currency": "EUR"
        }
    }"#;
    let ent_dec = ClaudeOAuthUsageFetcher::decode_usage_response(ent_raw).unwrap();
    let ent_map = ClaudeOAuthUsageFetcher::map_usage_response(
        &ent_dec,
        Some("enterprise"),
        None,
        None,
        None,
        true,
        now,
    )
    .unwrap();
    assert_eq!(
        ent_map
            .snapshot
            .identity
            .as_ref()
            .and_then(|i| i.login_method.as_deref()),
        Some("Claude Enterprise")
    );
    assert!(ent_map.snapshot.primary.is_none());
    let ent_cost = ent_map.snapshot.provider_cost.unwrap();
    assert_eq!(ent_cost.period.as_deref(), Some("Spend limit"));
    assert_eq!(ent_cost.currency_code, "EUR");
    assert_eq!(ent_cost.limit, 20.0);
    assert_eq!(ent_cost.used, 7.63);
    assert_eq!(
        ent_cost.spend_limit_window().map(|w| w.used_percent),
        Some(38.15)
    );

    // Spend limit without plan metadata
    let no_plan_map =
        ClaudeOAuthUsageFetcher::map_usage_response(&ent_dec, None, None, None, None, true, now)
            .unwrap();
    assert!(no_plan_map.snapshot.identity.is_none());
    assert!(no_plan_map.snapshot.primary.is_none());
    let no_plan_cost = no_plan_map.snapshot.provider_cost.unwrap();
    assert_eq!(no_plan_cost.period.as_deref(), Some("Spend limit"));
    assert_eq!(no_plan_cost.limit, 20.0);
    assert_eq!(no_plan_cost.used, 7.63);

    // Large enterprise spend limit
    let large_ent_raw = br#"{
        "extra_usage": {
            "is_enabled": true,
            "monthly_limit": 1000000,
            "used_credits": 123456,
            "utilization": 12.3456,
            "currency": "USD"
        }
    }"#;
    let large_ent_dec = ClaudeOAuthUsageFetcher::decode_usage_response(large_ent_raw).unwrap();
    let large_ent_map = ClaudeOAuthUsageFetcher::map_usage_response(
        &large_ent_dec,
        Some("enterprise"),
        None,
        None,
        None,
        true,
        now,
    )
    .unwrap();
    let large_cost = large_ent_map.snapshot.provider_cost.unwrap();
    assert_eq!(large_cost.limit, 10000.0);
    assert_eq!(large_cost.used, 1234.56);
    assert_eq!(
        large_cost.spend_limit_window().map(|w| w.used_percent),
        Some(12.3456)
    );

    // High-limit extra usage
    let high_raw = br#"{
        "five_hour": { "utilization": 1, "resets_at": "2025-12-25T12:00:00.000Z" },
        "extra_usage": {
            "is_enabled": true,
            "monthly_limit": 200000,
            "used_credits": 22200,
            "currency": "USD"
        }
    }"#;
    let high_dec = ClaudeOAuthUsageFetcher::decode_usage_response(high_raw).unwrap();
    let high_map = ClaudeOAuthUsageFetcher::map_usage_response(
        &high_dec,
        None,
        Some("claude_pro"),
        None,
        None,
        true,
        now,
    )
    .unwrap();
    let high_cost = high_map.snapshot.provider_cost.unwrap();
    assert_eq!(high_cost.limit, 2000.0);
    assert_eq!(high_cost.used, 222.0);

    // Disabled extra usage
    let disabled_raw = br#"{
        "five_hour": { "utilization": 1, "resets_at": "2025-12-25T12:00:00.000Z" },
        "extra_usage": {
            "is_enabled": false,
            "monthly_limit": 100,
            "used_credits": 10
        }
    }"#;
    let disabled_dec = ClaudeOAuthUsageFetcher::decode_usage_response(disabled_raw).unwrap();
    let disabled_map = ClaudeOAuthUsageFetcher::map_usage_response(
        &disabled_dec,
        None,
        None,
        None,
        None,
        true,
        now,
    )
    .unwrap();
    assert!(disabled_map.snapshot.provider_cost.is_none());
}

#[test]
fn prefers_opus_when_sonnet_missing_and_maps_subscription_type_for_generic_tier() {
    let now = "2026-01-15T10:00:00Z".parse::<Timestamp>().unwrap();
    let raw = br#"{
        "five_hour": { "utilization": 10, "resets_at": "2025-12-25T12:00:00.000Z" },
        "seven_day_opus": { "utilization": 42 }
    }"#;
    let decoded = ClaudeOAuthUsageFetcher::decode_usage_response(raw).unwrap();
    let mapped = ClaudeOAuthUsageFetcher::map_usage_response(
        &decoded,
        Some("pro"),
        Some("default_claude_ai"),
        None,
        None,
        true,
        now,
    )
    .unwrap();
    assert_eq!(
        mapped.snapshot.tertiary.as_ref().map(|w| w.used_percent),
        Some(42.0)
    );
    assert_eq!(
        mapped
            .snapshot
            .identity
            .as_ref()
            .and_then(|i| i.login_method.as_deref()),
        Some("Claude Pro")
    );
}

#[test]
fn ignores_merged_design_and_omelette_windows_and_prefers_populated_routines_alias() {
    let now = "2026-01-15T10:00:00Z".parse::<Timestamp>().unwrap();

    // Design + Omelette ignored; null routines skipped in favor of populated cowork alias
    let mixed_raw = br#"{
        "five_hour": { "utilization": 12.5, "resets_at": "2025-12-25T12:00:00.000Z" },
        "seven_day_design": null,
        "seven_day_omelette": { "utilization": 37, "resets_at": "2025-12-31T00:00:00.000Z" },
        "seven_day_routines": null,
        "seven_day_cowork": { "utilization": 14, "resets_at": "2026-01-01T00:00:00.000Z" }
    }"#;
    let mixed_dec = ClaudeOAuthUsageFetcher::decode_usage_response(mixed_raw).unwrap();
    let mixed_map =
        ClaudeOAuthUsageFetcher::map_usage_response(&mixed_dec, None, None, None, None, true, now)
            .unwrap();
    let extras = mixed_map
        .snapshot
        .extra_rate_windows
        .expect("extra windows");
    assert_eq!(extras.len(), 1);
    assert_eq!(extras[0].id.as_str(), "claude-routines");
    assert_eq!(extras[0].title, "Daily Routines");
    assert_eq!(extras[0].window.used_percent, 14.0);

    // Null cowork omits routines window
    let null_cowork_raw = br#"{
        "five_hour": { "utilization": 12.5, "resets_at": "2025-12-25T12:00:00.000Z" },
        "seven_day_omelette": { "utilization": 29, "resets_at": "2025-12-31T00:00:00.000Z" },
        "seven_day_cowork": null
    }"#;
    let null_dec = ClaudeOAuthUsageFetcher::decode_usage_response(null_cowork_raw).unwrap();
    let null_map =
        ClaudeOAuthUsageFetcher::map_usage_response(&null_dec, None, None, None, None, true, now)
            .unwrap();
    assert!(null_map.snapshot.extra_rate_windows.is_none());
}

#[test]
fn surfaces_fable_scoped_weekly_limit_from_limits_array_and_orders_before_routines() {
    let now = "2026-07-03T00:00:00Z".parse::<Timestamp>().unwrap();
    let raw = br#"{
        "five_hour": { "utilization": 11.0, "resets_at": "2026-07-03T00:30:00.282668+00:00" },
        "seven_day": { "utilization": 9.0, "resets_at": "2026-07-08T09:00:00.282694+00:00" },
        "seven_day_opus": null,
        "seven_day_sonnet": null,
        "seven_day_routines": { "utilization": 18, "resets_at": "2026-07-04T00:00:00Z" },
        "limits": [
            null,
            { "kind": "session", "group": "session", "percent": "11", "resets_at": null, "scope": null },
            {
                "kind": "weekly_scoped", "group": "weekly", "percent": 5,
                "resets_at": "2026-07-08T09:00:00.283070+00:00",
                "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null },
                "is_active": false
            }
        ]
    }"#;
    let decoded = ClaudeOAuthUsageFetcher::decode_usage_response(raw).unwrap();
    let mapped =
        ClaudeOAuthUsageFetcher::map_usage_response(&decoded, None, None, None, None, true, now)
            .unwrap();
    let extras = mapped.snapshot.extra_rate_windows.expect("extra windows");
    assert_eq!(
        extras.iter().map(|w| w.title.as_str()).collect::<Vec<_>>(),
        vec!["Fable only", "Daily Routines"]
    );
    assert_eq!(extras[0].id.as_str(), "claude-weekly-scoped-fable");
    assert_eq!(extras[0].window.used_percent, 5.0);
    assert!(extras[0].window.resets_at.is_some());
}

#[test]
fn count_sums_remaining_resets_of_started_unpaused_unexpired_grants() {
    let now = Timestamp::from_second(1_767_225_600).unwrap();
    let day = SignedDuration::from_secs(86_400);
    let iso = |ts: Timestamp| ts.to_string();

    let raw = format!(
        r#"{{
            "five_hour": {{ "utilization": 12.0 }},
            "cedar_ember": {{
                "eligible": true,
                "grants": [
                    {{ "id": "two_left", "resets_total": 2, "resets_left": 2, "starts_at": "{}", "ends_at": "{}", "paused": false, "usable_now": true }},
                    {{ "id": "one_left", "resets_total": 1, "resets_left": 1, "starts_at": "{}", "ends_at": "{}", "paused": false, "usable_now": true }},
                    {{ "id": "no_expiry", "resets_total": 1, "resets_left": 1, "starts_at": "{}", "ends_at": null, "paused": false, "usable_now": true }},
                    {{ "id": "gated", "resets_total": 1, "resets_left": 1, "starts_at": "{}", "ends_at": "{}", "paused": false, "usable_now": false }},
                    {{ "id": "paused", "resets_total": 1, "resets_left": 1, "starts_at": "{}", "ends_at": "{}", "paused": true, "usable_now": true }},
                    {{ "id": "used_up", "resets_total": 1, "resets_left": 0, "starts_at": "{}", "ends_at": "{}", "paused": false, "usable_now": true }},
                    {{ "id": "expired", "resets_total": 1, "resets_left": 1, "starts_at": "{}", "ends_at": "{}", "paused": false, "usable_now": true }},
                    {{ "id": "future", "resets_total": 1, "resets_left": 1, "starts_at": "{}", "ends_at": "{}", "paused": false, "usable_now": true }}
                ]
            }}
        }}"#,
        iso(now.checked_sub(day).unwrap()),
        iso(now
            .checked_add(SignedDuration::from_secs(5 * 86_400))
            .unwrap()),
        iso(now.checked_sub(day).unwrap()),
        iso(now.checked_add(day).unwrap()),
        iso(now.checked_sub(day).unwrap()),
        iso(now.checked_sub(day).unwrap()),
        iso(now
            .checked_add(SignedDuration::from_secs(2 * 86_400))
            .unwrap()),
        iso(now.checked_sub(day).unwrap()),
        iso(now.checked_add(day).unwrap()),
        iso(now.checked_sub(day).unwrap()),
        iso(now.checked_add(day).unwrap()),
        iso(now.checked_sub(day).unwrap()),
        iso(now.checked_sub(SignedDuration::from_secs(60)).unwrap()),
        iso(now.checked_add(day).unwrap()),
        iso(now
            .checked_add(SignedDuration::from_secs(9 * 86_400))
            .unwrap()),
    );

    let decoded = ClaudeOAuthUsageFetcher::decode_usage_response(raw.as_bytes()).unwrap();
    let mapped =
        ClaudeOAuthUsageFetcher::map_usage_response(&decoded, None, None, None, None, true, now)
            .unwrap();
    let resets = mapped.reset_credits.expect("reset credits");
    assert_eq!(resets.resets_left, 5);
    assert_eq!(
        resets.available_expirations(now),
        vec![
            Some(now.checked_add(day).unwrap()),
            Some(
                now.checked_add(SignedDuration::from_secs(2 * 86_400))
                    .unwrap()
            ),
            Some(
                now.checked_add(SignedDuration::from_secs(5 * 86_400))
                    .unwrap()
            ),
            Some(
                now.checked_add(SignedDuration::from_secs(5 * 86_400))
                    .unwrap()
            ),
            None,
        ]
    );

    let later = now.checked_add(SignedDuration::from_secs(129_600)).unwrap();
    assert_eq!(
        resets.available_expirations(later),
        vec![
            Some(
                now.checked_add(SignedDuration::from_secs(2 * 86_400))
                    .unwrap()
            ),
            Some(
                now.checked_add(SignedDuration::from_secs(5 * 86_400))
                    .unwrap()
            ),
            Some(
                now.checked_add(SignedDuration::from_secs(5 * 86_400))
                    .unwrap()
            ),
            None,
        ]
    );
}

#[test]
fn malformed_grants_are_dropped_without_hiding_valid_grants_or_usage_windows() {
    let now = Timestamp::from_second(1_767_225_600).unwrap();
    let raw = br#"{
        "five_hour": { "utilization": 12.0 },
        "cedar_ember": {
            "eligible": true,
            "grants": [
                {"resets_left": "many", "paused": false},
                {"resets_left": 2, "resets_total": 1, "paused": false},
                {"resets_left": -1, "paused": false},
                {"resets_left": 1, "resets_total": 1},
                {"resets_left": 1, "resets_total": 1, "paused": null},
                {"resets_left": 1, "paused": false, "ends_at": "next tuesday"},
                {"resets_left": 1, "paused": false, "starts_at": "soon"},
                {"resets_left": 1, "resets_total": 1, "paused": false, "starts_at": null, "ends_at": null}
            ]
        }
    }"#;

    let decoded = ClaudeOAuthUsageFetcher::decode_usage_response(raw).unwrap();
    let mapped =
        ClaudeOAuthUsageFetcher::map_usage_response(&decoded, None, None, None, None, true, now)
            .unwrap();
    assert_eq!(
        mapped.snapshot.primary.as_ref().map(|w| w.used_percent),
        Some(12.0)
    );
    let resets = mapped.reset_credits.expect("valid grant survives");
    assert_eq!(resets.available_expirations(now), vec![None]);
}

#[test]
fn implausibly_large_inventory_shows_nothing_but_keeps_usage_windows() {
    let now = Timestamp::from_second(1_767_225_600).unwrap();

    // Exactly 50 succeeds
    let at_limit_raw = br#"{
        "five_hour": { "utilization": 12.0 },
        "cedar_ember": {
            "eligible": true,
            "grants": [
                {
                    "id": "grant-secret",
                    "paused": false,
                    "resets_total": 50,
                    "resets_left": 50,
                    "starts_at": "2025-12-31T00:00:00Z",
                    "ends_at": "2026-01-02T00:00:00Z"
                }
            ]
        }
    }"#;
    let at_limit_dec = ClaudeOAuthUsageFetcher::decode_usage_response(at_limit_raw).unwrap();
    let at_limit_map = ClaudeOAuthUsageFetcher::map_usage_response(
        &at_limit_dec,
        None,
        None,
        None,
        None,
        true,
        now,
    )
    .unwrap();
    let at_limit_credits = at_limit_map.reset_credits.expect("at limit");
    assert_eq!(at_limit_credits.available_expirations(now).len(), 50);

    // 51 in one grant or 26 + 26 across two grants returns None
    for over_limit_raw in [
        br#"{
            "five_hour": { "utilization": 12.0 },
            "cedar_ember": {
                "eligible": true,
                "grants": [
                    { "paused": false, "resets_total": 51, "resets_left": 51 }
                ]
            }
        }"#
        .as_slice(),
        br#"{
            "five_hour": { "utilization": 12.0 },
            "cedar_ember": {
                "eligible": true,
                "grants": [
                    { "paused": false, "resets_total": 26, "resets_left": 26 },
                    { "paused": false, "resets_total": 26, "resets_left": 26 }
                ]
            }
        }"#
        .as_slice(),
    ] {
        let dec = ClaudeOAuthUsageFetcher::decode_usage_response(over_limit_raw).unwrap();
        let map =
            ClaudeOAuthUsageFetcher::map_usage_response(&dec, None, None, None, None, true, now)
                .unwrap();
        assert!(map.reset_credits.is_none());
        assert_eq!(
            map.snapshot.primary.as_ref().map(|w| w.used_percent),
            Some(12.0)
        );
    }

    // Future grant exceeding 50 does not trip the cap for a started grant
    let future_plus_started_raw = br#"{
        "five_hour": { "utilization": 12.0 },
        "cedar_ember": {
            "eligible": true,
            "grants": [
                {
                    "paused": false,
                    "resets_total": 51,
                    "resets_left": 51,
                    "starts_at": "2026-01-05T00:00:00Z",
                    "ends_at": "2026-01-10T00:00:00Z"
                },
                {
                    "paused": false,
                    "resets_total": 1,
                    "resets_left": 1,
                    "starts_at": "2025-12-31T00:00:00Z",
                    "ends_at": "2026-01-02T00:00:00Z"
                }
            ]
        }
    }"#;
    let dec = ClaudeOAuthUsageFetcher::decode_usage_response(future_plus_started_raw).unwrap();
    let map = ClaudeOAuthUsageFetcher::map_usage_response(&dec, None, None, None, None, true, now)
        .unwrap();
    let resets = map.reset_credits.expect("started grant kept");
    assert_eq!(
        resets.available_expirations(now),
        vec![Some("2026-01-02T00:00:00Z".parse::<Timestamp>().unwrap())]
    );
}

#[test]
fn ineligible_absent_or_unreadable_reset_credits_block_shows_nothing() {
    let now = Timestamp::from_second(1_767_225_600).unwrap();
    for block in [
        r#"{"eligible": false, "grants": [{"paused": false, "resets_left": 1}]}"#,
        r#"{"grants": [{"paused": false, "resets_left": 1}]}"#,
        r#"{"eligible": null, "grants": [{"paused": false, "resets_left": 1}]}"#,
        r#"{"eligible": "yes", "grants": [{"paused": false, "resets_left": 1}]}"#,
        r#"{"eligible": true, "grants": []}"#,
        "null",
        "[]",
    ] {
        let raw = format!(r#"{{"five_hour": {{"utilization": 12.0}}, "cedar_ember": {block}}}"#);
        let dec = ClaudeOAuthUsageFetcher::decode_usage_response(raw.as_bytes()).unwrap();
        let map =
            ClaudeOAuthUsageFetcher::map_usage_response(&dec, None, None, None, None, true, now)
                .unwrap();
        assert!(map.reset_credits.is_none());
        assert_eq!(
            map.snapshot.primary.as_ref().map(|w| w.used_percent),
            Some(12.0)
        );
    }

    // A non-boolean `usable_now` is ignored rather than dropping a valid grant.
    let lenient_usable_now = r#"{"five_hour": {"utilization": 12.0}, "cedar_ember": {"eligible": true, "grants": [{"paused": false, "resets_left": 1, "resets_total": 1, "usable_now": "yes"}]}}"#;
    let dec =
        ClaudeOAuthUsageFetcher::decode_usage_response(lenient_usable_now.as_bytes()).unwrap();
    let map = ClaudeOAuthUsageFetcher::map_usage_response(&dec, None, None, None, None, true, now)
        .unwrap();
    let credits = map.reset_credits.expect("valid grant retained");
    assert_eq!(credits.resets_left, 1);
}

#[test]
fn maps_iguana_necktie_cloud_credits_detail_section() {
    let raw = br#"{
        "five_hour": { "utilization": 12.0 },
        "iguana_necktie": {
            "limit_dollars": 100.0,
            "remaining_dollars": 67.5,
            "resets_at": "2026-01-25T00:00:00Z"
        }
    }"#;

    let now = "2026-01-15T10:00:00Z".parse::<Timestamp>().unwrap();
    let decoded = ClaudeOAuthUsageFetcher::decode_usage_response(raw).expect("decode usage");
    let mapped =
        ClaudeOAuthUsageFetcher::map_usage_response(&decoded, None, None, None, None, true, now)
            .expect("map usage");

    let row = mapped
        .snapshot
        .detail_row_by_id("claude-cloud-credits")
        .expect("cloud credits row");
    assert_eq!(row.label, "Cloud credits");
    assert_eq!(row.value, "$67.50 of $100.00 remaining");
    assert_eq!(row.usage_value, Some(32.5));
}

#[test]
fn parses_retry_after_delta_seconds_and_http_dates_and_rejects_malformed_dates() {
    let now = "1994-11-06T08:49:00Z".parse::<Timestamp>().unwrap();
    assert_eq!(
        parse_retry_after(Some("37"), now),
        Some("1994-11-06T08:49:37Z".parse::<Timestamp>().unwrap())
    );
    assert_eq!(
        parse_retry_after(Some("1.5"), now),
        Some("1994-11-06T08:49:01.500Z".parse::<Timestamp>().unwrap())
    );
    assert_eq!(parse_retry_after(Some("0"), now), Some(now));
    assert_eq!(
        parse_retry_after(Some("Sun, 06 Nov 1994 08:49:37 GMT"), now),
        Some("1994-11-06T08:49:37Z".parse::<Timestamp>().unwrap())
    );
    assert_eq!(
        parse_retry_after(Some("Sunday, 06-Nov-94 08:49:37 GMT"), now),
        Some("1994-11-06T08:49:37Z".parse::<Timestamp>().unwrap())
    );
    assert_eq!(
        parse_retry_after(Some("Sun Nov  6 08:49:37 1994"), now),
        Some("1994-11-06T08:49:37Z".parse::<Timestamp>().unwrap())
    );

    // Malformed or negative Retry-After values must return None without panicking.
    assert_eq!(parse_retry_after(Some("-5"), now), None);
    assert_eq!(
        parse_retry_after(Some("Sun, 32 Nov 1994 08:49:37 GMT"), now),
        None
    );
    assert_eq!(
        parse_retry_after(Some("Sun, 30 Feb 1994 08:49:37 GMT"), now),
        None
    );
    assert_eq!(
        parse_retry_after(Some("Sun, 06 Nov 1994 25:49:37 GMT"), now),
        None
    );
    assert_eq!(
        parse_retry_after(Some("Sun, 06 Nov 1994 08:60:37 GMT"), now),
        None
    );
    assert_eq!(
        parse_retry_after(Some("Sun, 06 Nov 1994 08:49:60 GMT"), now),
        None
    );
}

#[tokio::test]
async fn concurrent_oauth_cooldown_writes_keep_every_account_and_latest_deadline() {
    let gate = Arc::new(ClaudeOAuthUsageRateLimitGate::new());
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let short_deadline = now.checked_add(SignedDuration::from_secs(60)).unwrap();
    let long_deadline = now.checked_add(SignedDuration::from_secs(600)).unwrap();

    let mut handles = Vec::new();
    for index in 0..100usize {
        let gate_clone = gate.clone();
        handles.push(tokio::spawn(async move {
            let token = if index.is_multiple_of(2) {
                "test-auth-token"
            } else {
                "test-token-placeholder"
            };
            let retry_after = if index.is_multiple_of(3) {
                long_deadline
            } else {
                short_deadline
            };
            gate_clone.record_rate_limit(token, Some(retry_after), now);
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }

    assert_eq!(
        gate.active_cooldown_until("test-auth-token", now),
        Some(long_deadline)
    );
    assert_eq!(
        gate.active_cooldown_until("test-token-placeholder", now),
        Some(long_deadline)
    );
}

#[tokio::test]
async fn rate_limit_gate_suppresses_background_calls_and_allows_user_initiated_bypass_per_account()
{
    let server = https::start_server();
    let usage_mock = server.mock(|when, then| {
        when.method(GET).path("/api/oauth/usage");
        then.status(200)
            .body(r#"{"five_hour":{"utilization":22.0}}"#);
    });

    let base_url = Url::parse(&server.url("")).unwrap();
    let endpoint = ClaudeOAuthEndpointConfig::with_base_url(base_url).unwrap();
    let net = https::broker(NetConfig::default());
    let gate = ClaudeOAuthUsageRateLimitGate::new();
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let token_a = SecretString::new("sk-ant-oat01-rl-1");
    let token_b = SecretString::new("sk-ant-oat01-rl-2");

    let cooldown_until = gate.record_rate_limit(
        token_a.expose_secret(),
        Some(now.checked_add(SignedDuration::from_secs(120)).unwrap()),
        now,
    );

    let bg_call = BrokerCall {
        interaction: FetchInteraction::Background,
        cancel: CancellationToken::new(),
        request_id: "bg-1".to_owned(),
    };
    let err = ClaudeOAuthUsageFetcher::fetch_usage_response(
        &net, &endpoint, &gate, &token_a, &bg_call, now,
    )
    .await
    .unwrap_err();
    assert_eq!(
        err,
        ClaudeOAuthFetchError::RateLimited {
            retry_after: Some(cooldown_until),
            cooldown_until,
            cached_gate: true,
        }
    );
    assert_eq!(usage_mock.calls(), 0);

    // Account B is unaffected by Account A's cooldown.
    let resp_b = ClaudeOAuthUsageFetcher::fetch_usage_response(
        &net, &endpoint, &gate, &token_b, &bg_call, now,
    )
    .await
    .expect("account B succeeds in background");
    assert_eq!(
        resp_b.five_hour.as_ref().and_then(|w| w.utilization),
        Some(22.0)
    );
    assert_eq!(usage_mock.calls(), 1);

    // User-initiated call on Account A bypasses the gate and clears it on HTTP 200.
    let ui_call = BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "ui-1".to_owned(),
    };
    let resp_a = ClaudeOAuthUsageFetcher::fetch_usage_response(
        &net, &endpoint, &gate, &token_a, &ui_call, now,
    )
    .await
    .expect("user initiated succeeds");
    assert_eq!(
        resp_a.five_hour.as_ref().and_then(|w| w.utilization),
        Some(22.0)
    );
    assert_eq!(usage_mock.calls(), 2);
    assert_eq!(
        gate.active_cooldown_until(token_a.expose_secret(), now),
        None
    );
}

#[test]
fn oauth_usage_user_agent_uses_claude_code_version_and_skips_detector_when_disabled() {
    assert_eq!(
        ClaudeOAuthUsageFetcher::claude_code_user_agent(Some("2.1.70 (Claude Code)")),
        "claude-code/2.1.70"
    );
    assert_eq!(
        ClaudeOAuthUsageFetcher::claude_code_user_agent(None),
        "claude-code/2.1.0"
    );

    let mut detection_count = 0usize;
    let fallback = ClaudeOAuthUsageFetcher::claude_code_user_agent_with_detector(false, || {
        detection_count += 1;
        Some("2.1.70 (Claude Code)".to_owned())
    });
    assert_eq!(fallback, "claude-code/2.1.0");
    assert_eq!(detection_count, 0);

    let detected = ClaudeOAuthUsageFetcher::claude_code_user_agent_with_detector(true, || {
        detection_count += 1;
        Some("2.1.70 (Claude Code)".to_owned())
    });
    assert_eq!(detected, "claude-code/2.1.70");
    assert_eq!(detection_count, 1);
}

#[test]
fn scope_based_strategy_resolution_matches_descriptor() {
    let s1 = ClaudeProviderPlan::resolve_usage_strategy(SourceMode::Auto, false, true, true, true)
        .expect("s1");
    assert_eq!(s1.data_source, ClaudeSourceSelectedStrategy::Oauth);

    let s2 = ClaudeProviderPlan::resolve_usage_strategy(SourceMode::Auto, false, true, true, false)
        .expect("s2");
    assert_eq!(s2.data_source, ClaudeSourceSelectedStrategy::Cli);

    let s3 = ClaudeProviderPlan::resolve_usage_strategy(SourceMode::Auto, false, true, false, true)
        .expect("s3");
    assert_eq!(s3.data_source, ClaudeSourceSelectedStrategy::Oauth);

    let s4 =
        ClaudeProviderPlan::resolve_usage_strategy(SourceMode::Auto, false, false, true, false)
            .expect("s4");
    assert_eq!(s4.data_source, ClaudeSourceSelectedStrategy::Cli);

    let none_auto =
        ClaudeProviderPlan::resolve_usage_strategy(SourceMode::Auto, false, false, false, false);
    assert_eq!(none_auto, None);

    let explicit_oauth_fallback =
        ClaudeProviderPlan::resolve_usage_strategy(SourceMode::Oauth, false, false, false, false)
            .expect("explicit oauth fallback");
    assert_eq!(
        explicit_oauth_fallback.data_source,
        ClaudeSourceSelectedStrategy::Oauth
    );
}

#[tokio::test]
async fn retries_inventory_fallback_on_400_and_non_scope_403_with_claude_code_user_agent() {
    let server = https::start_server();
    let primary_400 = server.mock(|when, then| {
        when.method(GET)
            .path("/api/oauth/usage")
            .query_param("cedar_ember", "1")
            .header("user-agent", "claude-cli/2.1.0 (external, cli)");
        then.status(400)
            .body(r#"{"error":"unknown query parameter"}"#);
    });
    let fallback_200 = server.mock(|when, then| {
        when.method(GET)
            .path("/api/oauth/usage")
            .header("user-agent", "claude-code/2.1.0");
        then.status(200)
            .body(r#"{"five_hour":{"utilization":31.0}}"#);
    });

    let base_url = Url::parse(&server.url("")).unwrap();
    let endpoint = ClaudeOAuthEndpointConfig::with_base_url(base_url).unwrap();
    let net = https::broker(NetConfig::default());
    let gate = ClaudeOAuthUsageRateLimitGate::new();
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let token = SecretString::new("sk-ant-oat01-fb-1");

    let resp = ClaudeOAuthUsageFetcher::fetch_usage_response(
        &net,
        &endpoint,
        &gate,
        &token,
        &https::call(),
        now,
    )
    .await
    .expect("fallback succeeds");
    assert_eq!(
        resp.five_hour.as_ref().and_then(|w| w.utilization),
        Some(31.0)
    );
    assert_eq!(primary_400.calls(), 1);
    assert_eq!(fallback_200.calls(), 1);
}

#[tokio::test]
async fn does_not_retry_on_403_missing_user_profile_scope() {
    let server = https::start_server();
    let primary_403 = server.mock(|when, then| {
        when.method(GET)
            .path("/api/oauth/usage")
            .query_param("cedar_ember", "1");
        then.status(403)
            .body(r#"{"error":"OAuth token requires user:profile scope"}"#);
    });

    let base_url = Url::parse(&server.url("")).unwrap();
    let endpoint = ClaudeOAuthEndpointConfig::with_base_url(base_url).unwrap();
    let net = https::broker(NetConfig::default());
    let gate = ClaudeOAuthUsageRateLimitGate::new();
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let token = SecretString::new("sk-ant-oat01-ns-1");

    let err = ClaudeOAuthUsageFetcher::fetch_usage_response(
        &net,
        &endpoint,
        &gate,
        &token,
        &https::call(),
        now,
    )
    .await
    .unwrap_err();
    assert_eq!(err, ClaudeOAuthFetchError::MissingUserProfileScope);
    assert_eq!(primary_403.calls(), 1);
}

#[tokio::test]
async fn classifies_401_403_429_and_500_http_responses() {
    let server = https::start_server();
    let base_url = Url::parse(&server.url("")).unwrap();
    let endpoint = ClaudeOAuthEndpointConfig::with_base_url(base_url).unwrap();
    let net = https::broker(NetConfig::default());
    let gate = ClaudeOAuthUsageRateLimitGate::new();
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let token = SecretString::new("sk-ant-oat01-st-1");

    // 401 -> AuthenticationExpired
    let mut mock_401 = server.mock(|when, then| {
        when.method(GET).path("/api/oauth/usage");
        then.status(401).body("unauthorized");
    });
    let err_401 = ClaudeOAuthUsageFetcher::fetch_usage_response(
        &net,
        &endpoint,
        &gate,
        &token,
        &https::call(),
        now,
    )
    .await
    .unwrap_err()
    .into_fetch_error();
    mock_401.delete();
    assert!(matches!(
        err_401,
        FetchError::Classified(ref c)
            if c.kind == ProviderErrorKind::AuthenticationExpired && c.transport.is_none()
    ));

    // 429 with Retry-After -> RateLimited
    let mut mock_429 = server.mock(|when, then| {
        when.method(GET).path("/api/oauth/usage");
        then.status(429)
            .header("Retry-After", "45")
            .body("rate limited");
    });
    let raw_429 = ClaudeOAuthUsageFetcher::fetch_usage_response(
        &net,
        &endpoint,
        &gate,
        &token,
        &https::call(),
        now,
    )
    .await
    .unwrap_err();
    mock_429.delete();
    let expected_until = now.checked_add(SignedDuration::from_secs(45)).unwrap();
    assert_eq!(
        raw_429,
        ClaudeOAuthFetchError::RateLimited {
            retry_after: Some(expected_until),
            cooldown_until: expected_until,
            cached_gate: false,
        }
    );
    let err_429 = raw_429.into_fetch_error();
    assert!(matches!(
        err_429,
        FetchError::Classified(ref c)
            if c.kind == ProviderErrorKind::RateLimited && c.transport.is_none()
    ));
    gate.clear(token.expose_secret());

    // 503 -> ProviderUnavailable
    let mut mock_503 = server.mock(|when, then| {
        when.method(GET).path("/api/oauth/usage");
        then.status(503).body("unavailable");
    });
    let err_503 = ClaudeOAuthUsageFetcher::fetch_usage_response(
        &net,
        &endpoint,
        &gate,
        &token,
        &https::call(),
        now,
    )
    .await
    .unwrap_err()
    .into_fetch_error();
    mock_503.delete();
    assert!(matches!(
        err_503,
        FetchError::Classified(ref c)
            if c.kind == ProviderErrorKind::ProviderUnavailable && c.transport.is_none()
    ));
}

#[test]
fn decodes_oauth_profile_computes_verified_owner_id_and_redacts_debug() {
    let raw = br#"{
        "account": {
            "uuid": "Acc-UUID-123",
            "emailAddress": "synthetic-user@example.invalid"
        },
        "organization": {
            "uuid": "Org-UUID-456",
            "name": "Synthetic Workspace"
        }
    }"#;

    let profile = ClaudeAccountIdentity::decode_profile(raw).expect("decode profile");
    assert_eq!(profile.account_uuid.as_deref(), Some("Acc-UUID-123"));
    assert_eq!(
        profile.email.as_deref(),
        Some("synthetic-user@example.invalid")
    );
    assert_eq!(profile.organization_uuid.as_deref(), Some("Org-UUID-456"));
    assert_eq!(
        profile.organization_name.as_deref(),
        Some("Synthetic Workspace")
    );

    let owner_id = profile.verified_owner_id().expect("verified owner id");
    assert!(owner_id.starts_with("claude-owner-v1:"));
    assert_eq!(
        Some(owner_id.clone()),
        ClaudeVerifiedAccountOwner::owner_id(
            Some("acc-uuid-123"),
            Some("other@example.invalid"),
            Some("org-uuid-456"),
        )
    );
    // Missing organization UUID returns None
    assert_eq!(
        ClaudeVerifiedAccountOwner::owner_id(
            Some("acc-uuid-123"),
            Some("synthetic-user@example.invalid"),
            None,
        ),
        None
    );

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let usage =
        ClaudeOAuthUsageFetcher::decode_usage_response(br#"{"five_hour":{"utilization":15.0}}"#)
            .unwrap();
    let mapped = ClaudeOAuthUsageFetcher::map_usage_response(
        &usage,
        Some("pro"),
        None,
        Some(&profile),
        None,
        true,
        now,
    )
    .unwrap();
    let id = mapped.snapshot.identity.as_ref().expect("identity");
    assert_eq!(
        id.account_email.as_deref(),
        Some("synthetic-user@example.invalid")
    );
    assert_eq!(id.account_organization, None);
    assert_eq!(id.account_id.as_deref(), Some(owner_id.as_str()));

    let debug_str = format!("{mapped:?}");
    assert!(!debug_str.contains("synthetic-user@example.invalid"));
    assert!(!debug_str.contains("Synthetic Workspace"));
    assert!(!debug_str.contains(&owner_id));
}

#[cfg(target_os = "linux")]
struct NoopSleeper;

#[cfg(target_os = "linux")]
impl Sleeper for NoopSleeper {
    fn sleep<'a>(&'a self, _duration: Duration) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

#[cfg(target_os = "linux")]
struct RewritingRefresher {
    cred_path: std::path::PathBuf,
    touches: AtomicUsize,
}

#[cfg(target_os = "linux")]
impl DelegatedRefresher for RewritingRefresher {
    fn is_available(&self) -> bool {
        true
    }

    fn touch(&self, _timeout: Duration) -> BoxFuture<'_, Result<(), DelegatedRefreshError>> {
        self.touches.fetch_add(1, Ordering::SeqCst);
        let cred_path = self.cred_path.clone();
        Box::pin(async move {
            let stage = cred_path.with_extension("tmp");
            std::fs::write(
                &stage,
                br#"{
                    "claudeAiOauth": {
                        "accessToken": "sk-ant-oat01-new-1",
                        "refreshToken": "sk-ant-ort01-ref-1",
                        "expiresAt": 1893456000000.0,
                        "scopes": ["user:profile"],
                        "rateLimitTier": "default_claude_max_20x",
                        "subscriptionType": "max"
                    }
                }"#,
            )
            .unwrap();
            std::fs::rename(&stage, &cred_path).unwrap();
            Ok(())
        })
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn executes_end_to_end_oauth_strategy_and_delegates_cli_refresh_when_expired() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let work = temp.path().join("work");
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::create_dir_all(&work).unwrap();

    let cred_path = claude_dir.join(".credentials.json");
    // Write initially expired credentials (expiresAt = 1000 ms)
    std::fs::write(
        &cred_path,
        br#"{
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-exp-1",
                "refreshToken": "sk-ant-ort01-ref-1",
                "expiresAt": 1000.0,
                "scopes": ["user:profile"],
                "rateLimitTier": "default_claude_max_5x",
                "subscriptionType": "max"
            }
        }"#,
    )
    .unwrap();

    let server = https::start_server();
    let usage_mock = server.mock(|when, then| {
        when.method(GET)
            .path("/api/oauth/usage")
            .query_param("cedar_ember", "1")
            .header("authorization", "Bearer sk-ant-oat01-new-1");
        then.status(200).body(
            r#"{
                "five_hour": { "utilization": 14.0, "resets_at": "2026-01-15T15:00:00Z" },
                "seven_day": { "utilization": 39.0, "resets_at": "2026-01-20T00:00:00Z" }
            }"#,
        );
    });
    let profile_mock = server.mock(|when, then| {
        when.method(GET)
            .path("/api/oauth/profile")
            .header("authorization", "Bearer sk-ant-oat01-new-1")
            .header("accept", "application/json");
        then.status(200).body(
            r#"{
                "account": {
                    "uuid": "acc-1",
                    "emailAddress": "synthetic@example.invalid"
                },
                "organization": {
                    "uuid": "org-1",
                    "name": "Synthetic Org"
                }
            }"#,
        );
    });

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let refresher = Arc::new(RewritingRefresher {
        cred_path: cred_path.clone(),
        touches: AtomicUsize::new(0),
    });
    let cooldown = DelegatedRefreshCooldown::new(move || now);
    let coordinator = Arc::new(
        DelegatedRefreshCoordinator::new(refresher.clone(), cooldown, None)
            .with_observation_ticks(vec![Duration::from_millis(5)]),
    );

    let base_url = Url::parse(&server.url("")).unwrap();
    let endpoint = ClaudeOAuthEndpointConfig::with_base_url(base_url).unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));

    let config = ClaudeOAuthStrategyConfig::new(net, &work, &home)
        .with_endpoint(endpoint)
        .with_delegated_refresh(coordinator)
        .with_clock(move || now);
    let plan = ClaudeProviderPlan::new(config);

    let provider_id = ProviderId::new("claude").unwrap();
    let cx = FetchContext {
        provider: provider_id.clone(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "e2e-1".to_owned(),
        source_mode: SourceMode::Auto,
        env: BTreeMap::new(),
        include_credits: true,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    };

    let outcome = fetch_outcome(&provider_id, &plan, &cx, &NoopSleeper).await;
    let res = outcome.result.expect("fetch outcome succeeds");
    assert_eq!(res.strategy_id, "claude.oauth");
    assert_eq!(res.source_label, "oauth");
    assert_eq!(refresher.touches.load(Ordering::SeqCst), 1);
    assert_eq!(usage_mock.calls(), 1);
    assert_eq!(profile_mock.calls(), 1);

    assert_eq!(
        res.usage.primary.as_ref().map(|w| w.used_percent),
        Some(14.0)
    );
    assert_eq!(
        res.usage.secondary.as_ref().map(|w| w.used_percent),
        Some(39.0)
    );
    let id = res.usage.identity.expect("identity");
    assert_eq!(id.login_method.as_deref(), Some("Claude Max 20x"));
    assert_eq!(
        id.account_email.as_deref(),
        Some("synthetic@example.invalid")
    );
    assert_eq!(id.account_organization, None);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cli_oauth_and_background_daemon_oauth_do_not_delegate_refresh_without_opt_in() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let work = temp.path().join("work");
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::create_dir_all(&work).unwrap();

    let cred_path = claude_dir.join(".credentials.json");
    std::fs::write(
        &cred_path,
        br#"{
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat01-exp-1",
                "refreshToken": "sk-ant-ort01-ref-1",
                "expiresAt": 1000.0,
                "scopes": ["user:profile"]
            }
        }"#,
    )
    .unwrap();

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let refresher = Arc::new(RewritingRefresher {
        cred_path: cred_path.clone(),
        touches: AtomicUsize::new(0),
    });
    let cooldown = DelegatedRefreshCooldown::new(move || now);
    let coordinator = Arc::new(DelegatedRefreshCoordinator::new(
        refresher.clone(),
        cooldown,
        None,
    ));
    let net = Arc::new(https::broker(NetConfig::default()));
    let config = ClaudeOAuthStrategyConfig::new(net, &work, &home)
        .with_delegated_refresh(coordinator)
        .with_clock(move || now);
    let plan = ClaudeProviderPlan::new(config);
    let provider_id = ProviderId::new("claude").unwrap();

    // 1. CLI explicit OAuth does not delegate refresh even for UserInitiated.
    let cli_cx = FetchContext {
        provider: provider_id.clone(),
        runtime: FetchRuntime::Cli,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "cli-oauth-1".to_owned(),
        source_mode: SourceMode::Oauth,
        env: BTreeMap::new(),
        include_credits: true,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    };
    let cli_outcome = fetch_outcome(&provider_id, &plan, &cli_cx, &NoopSleeper).await;
    assert!(cli_outcome.result.is_err());
    assert_eq!(refresher.touches.load(Ordering::SeqCst), 0);

    // 2. Daemon background OAuth does not delegate refresh when allow_background_delegated_refresh is false.
    let bg_cx = FetchContext {
        provider: provider_id.clone(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::Background,
        phase: FetchPhase::Regular,
        request_id: "bg-oauth-1".to_owned(),
        source_mode: SourceMode::Oauth,
        env: BTreeMap::new(),
        include_credits: true,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    };
    let bg_outcome = fetch_outcome(&provider_id, &plan, &bg_cx, &NoopSleeper).await;
    assert!(bg_outcome.result.is_err());
    assert_eq!(refresher.touches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn resolve_strategies_respects_admin_api_pre_gate_and_selected_account_boundary() {
    let temp = tempfile::tempdir().expect("tempdir");
    let net = Arc::new(https::broker(NetConfig::default()));
    let config = ClaudeOAuthStrategyConfig::new(net, temp.path(), temp.path());
    let plan = ClaudeProviderPlan::new(config.clone());
    let provider_id = ProviderId::new("claude").unwrap();

    // Auto with ANTHROPIC_ADMIN_KEY short-circuits to claude.admin-api
    let mut admin_env = BTreeMap::new();
    admin_env.insert(
        "ANTHROPIC_ADMIN_KEY".to_owned(),
        SecretString::new("sk-ant-admin01-key"),
    );
    let auto_admin_cx = FetchContext {
        provider: provider_id.clone(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "admin-1".to_owned(),
        source_mode: SourceMode::Auto,
        env: admin_env,
        include_credits: true,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    };
    let strategies = plan.resolve_strategies(&auto_admin_cx).await;
    assert_eq!(strategies.len(), 1);
    assert_eq!(strategies[0].id(), CLAUDE_ADMIN_API_STRATEGY_ID);

    // Selected account with OAuth token in Web mode returns empty (authority boundary)
    let mut selected_config = config;
    selected_config.token_account_token = Some(SecretString::new("sk-ant-oat01-sel-1"));
    let selected_plan = ClaudeProviderPlan::new(selected_config);
    let selected_web_cx = FetchContext {
        provider: provider_id.clone(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "sel-web-1".to_owned(),
        source_mode: SourceMode::Web,
        env: BTreeMap::new(),
        include_credits: true,
        selected_token_account: Some("acct-1".to_owned()),
        cancel: CancellationToken::new(),
    };
    assert!(
        selected_plan
            .resolve_strategies(&selected_web_cx)
            .await
            .is_empty()
    );

    // Selected account with OAuth token in Auto mode routes to no strategy (matching upstream)
    let selected_auto_cx = FetchContext {
        source_mode: SourceMode::Auto,
        ..selected_web_cx.clone()
    };
    assert!(
        selected_plan
            .resolve_strategies(&selected_auto_cx)
            .await
            .is_empty()
    );

    // Selected account with OAuth token in Oauth mode routes to claude.oauth
    let selected_oauth_cx = FetchContext {
        source_mode: SourceMode::Oauth,
        ..selected_web_cx
    };
    let selected_strategies = selected_plan.resolve_strategies(&selected_oauth_cx).await;
    assert_eq!(selected_strategies.len(), 1);
    assert_eq!(selected_strategies[0].id(), CLAUDE_OAUTH_STRATEGY_ID);
}
