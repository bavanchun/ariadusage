use ariadusage_core::model::{
    NamedRateWindow, ProviderCostSnapshot, ProviderIdentitySnapshot, RateWindow, UsageSnapshot,
};
use ariadusage_core::pace::{ProviderPaceCapability, UsagePace};
use ariadusage_core::projection::{
    ProjectedError, ProjectionInput, RefreshState, category_safe_message, provider_snapshot,
};
use ariadusage_protocol::{
    Confidence, MetricState, PaceStage, ProviderErrorCategory, ProviderErrorKind, ProviderId,
    SourceKind,
};
use jiff::Timestamp;

fn sample_timestamp() -> Timestamp {
    "2026-10-06T12:00:00Z".parse::<Timestamp>().unwrap()
}

fn sample_usage_snapshot() -> UsageSnapshot {
    let now = sample_timestamp();
    let primary = RateWindow::new(
        25.0,
        Some(300),
        Some(now + jiff::Span::new().hours(3)),
        Some("Resets in 3 hours".into()),
        None,
        false,
    )
    .unwrap();

    let secondary = RateWindow::new(
        50.0,
        Some(10080),
        Some(now + jiff::Span::new().hours(96)),
        Some("Resets in 4 days".into()),
        None,
        false,
    )
    .unwrap();

    let extra = vec![
        NamedRateWindow::new(
            "burst",
            "Burst Limit",
            RateWindow::new(
                10.0,
                Some(60),
                Some(now + jiff::Span::new().hours(1)),
                None,
                None,
                false,
            )
            .unwrap(),
        ),
        NamedRateWindow::new(
            "unknown-quota",
            "Preview Quota",
            RateWindow::new(0.0, Some(1440), None, None, None, false).unwrap(),
        )
        .with_usage_known(false),
    ];

    let cost = ProviderCostSnapshot::new(12.50, 50.00, "USD", now);
    let identity = ProviderIdentitySnapshot::new(
        Some(ProviderId::new("claude").unwrap()),
        Some("user@anthropic.com".into()),
        Some("Acme Corp".into()),
        Some("oauth-google".into()),
        Some("acct_12345".into()),
    );

    UsageSnapshot::new(
        Some(primary),
        Some(secondary),
        None,
        Some(extra),
        Some(cost),
        Vec::new(),
        None,
        None,
        now,
        Some(identity),
        Confidence::Exact,
    )
    .unwrap()
}

fn sample_projection_input(provider_id: &str, refresh: RefreshState) -> ProjectionInput {
    ProjectionInput {
        provider: ProviderId::new(provider_id).unwrap(),
        display_name: "Test Provider".into(),
        enabled: true,
        source_mode: "cli".into(),
        refresh,
        kept_snapshot: false,
        as_of: sample_timestamp(),
        source_kind: SourceKind::Cli,
        source_label: "test-cli".into(),
        error: None,
        pace: None,
    }
}

#[test]
fn test_fresh_data_maps_to_value() {
    let snap = sample_usage_snapshot();
    let input = sample_projection_input("claude", RefreshState::Fresh);

    let projected = provider_snapshot(&snap, &input);

    let primary = projected.windows.primary.expect("primary window");
    assert_eq!(primary.state, MetricState::Value);
    assert_eq!(primary.confidence, Confidence::Exact);
    let primary_val = primary.value.expect("primary value");
    assert_eq!(primary_val.used_percent, 25.0);
    assert_eq!(primary_val.window_minutes, Some(300));

    let cost = projected.cost.expect("cost metric");
    assert_eq!(cost.state, MetricState::Value);
    assert_eq!(cost.value.as_ref().unwrap().used, 12.50);
    assert_eq!(cost.value.as_ref().unwrap().limit, Some(50.00));

    let id = projected.identity.expect("identity metric");
    assert_eq!(id.state, MetricState::Value);
    let id_val = id.value.as_ref().unwrap();
    assert_eq!(id_val.account_email.as_deref(), Some("user@anthropic.com"));
    assert_eq!(id_val.plan.as_deref(), Some("oauth-google")); // plan <- login_method

    assert!(projected.last_error.is_none());
}

#[test]
fn test_loading_without_kept_data_maps_to_loading() {
    let snap = sample_usage_snapshot();
    let mut input = sample_projection_input("claude", RefreshState::Loading);
    input.kept_snapshot = false;

    let projected = provider_snapshot(&snap, &input);

    let primary = projected.windows.primary.expect("primary window");
    assert_eq!(primary.state, MetricState::Loading);
    assert!(primary.value.is_none());

    let cost = projected.cost.expect("cost metric");
    assert_eq!(cost.state, MetricState::Loading);
    assert!(cost.value.is_none());

    let id = projected.identity.expect("identity metric");
    assert_eq!(id.state, MetricState::Loading);
    assert!(id.value.is_none());
}

#[test]
fn test_loading_with_kept_data_maps_to_stale() {
    let snap = sample_usage_snapshot();
    let mut input = sample_projection_input("claude", RefreshState::Loading);
    input.kept_snapshot = true;

    let projected = provider_snapshot(&snap, &input);

    let primary = projected.windows.primary.expect("primary window");
    assert_eq!(primary.state, MetricState::Stale);
    assert!(primary.value.is_some());
}

#[test]
fn test_error_without_kept_data_maps_to_error() {
    let snap = sample_usage_snapshot();
    let mut input = sample_projection_input("claude", RefreshState::Error);
    input.kept_snapshot = false;
    input.error = Some(
        ProjectedError::new(
            ProviderErrorKind::AuthenticationExpired,
            ProviderErrorCategory::Auth,
        )
        .with_retry(30),
    );

    let projected = provider_snapshot(&snap, &input);

    let primary = projected.windows.primary.expect("primary window");
    assert_eq!(primary.state, MetricState::Error);
    assert!(primary.value.is_none());

    let err = projected.last_error.expect("last_error present");
    assert_eq!(err.kind, ProviderErrorKind::AuthenticationExpired);
    assert_eq!(err.category, ProviderErrorCategory::Auth);
    assert_eq!(err.message, "Authentication or credential failure");
    assert_eq!(err.retry_after_seconds, Some(30));
}

#[test]
fn test_error_with_kept_data_maps_to_stale_with_last_error() {
    let snap = sample_usage_snapshot();
    let mut input = sample_projection_input("claude", RefreshState::Error);
    input.kept_snapshot = true;
    input.error = Some(ProjectedError::new(
        ProviderErrorKind::RateLimited,
        ProviderErrorCategory::Api,
    ));

    let projected = provider_snapshot(&snap, &input);

    let primary = projected.windows.primary.expect("primary window");
    assert_eq!(primary.state, MetricState::Stale);
    assert!(primary.value.is_some());

    let err = projected.last_error.expect("last_error present");
    assert_eq!(err.kind, ProviderErrorKind::RateLimited);
    assert_eq!(err.category, ProviderErrorCategory::Api);
    assert_eq!(err.message, "Provider API failure");
}

#[test]
fn test_stale_without_error_maps_to_stale() {
    let snap = sample_usage_snapshot();
    let input = sample_projection_input("claude", RefreshState::Stale);

    let projected = provider_snapshot(&snap, &input);

    let primary = projected.windows.primary.expect("primary window");
    assert_eq!(primary.state, MetricState::Stale);
    assert!(primary.value.is_some());
    assert!(projected.last_error.is_none());
}

#[test]
fn test_synthetic_placeholder_and_unknown_usage_map_to_unknown() {
    let now = sample_timestamp();
    let placeholder = RateWindow::new(0.0, Some(300), None, None, None, true).unwrap();
    let regular = RateWindow::new(40.0, Some(300), None, None, None, false).unwrap();

    let extra = vec![
        NamedRateWindow::new(
            "known",
            "Known Extra",
            RateWindow::new(15.0, Some(60), None, None, None, false).unwrap(),
        ),
        NamedRateWindow::new(
            "unknown-usage",
            "Unknown Usage",
            RateWindow::new(0.0, Some(60), None, None, None, false).unwrap(),
        )
        .with_usage_known(false),
        NamedRateWindow::new(
            "placeholder-extra",
            "Placeholder Extra",
            RateWindow::new(0.0, Some(60), None, None, None, true).unwrap(),
        ),
    ];

    let snap = UsageSnapshot::new(
        Some(placeholder),
        Some(regular),
        None,
        Some(extra),
        None,
        Vec::new(),
        None,
        None,
        now,
        None,
        Confidence::Exact,
    )
    .unwrap();

    let input = sample_projection_input("claude", RefreshState::Fresh);
    let projected = provider_snapshot(&snap, &input);

    let primary = projected.windows.primary.expect("primary window");
    assert_eq!(primary.state, MetricState::Unknown);
    assert!(
        primary.value.is_none(),
        "placeholder must never carry a value"
    );

    let secondary = projected.windows.secondary.expect("secondary window");
    assert_eq!(secondary.state, MetricState::Value);
    assert!(secondary.value.is_some());

    assert_eq!(projected.windows.extra[0].window.state, MetricState::Value);
    assert!(projected.windows.extra[0].window.value.is_some());

    assert_eq!(
        projected.windows.extra[1].window.state,
        MetricState::Unknown
    );
    assert!(
        projected.windows.extra[1].window.value.is_none(),
        "usageKnown=false must never carry a value"
    );

    assert_eq!(
        projected.windows.extra[2].window.state,
        MetricState::Unknown
    );
    assert!(
        projected.windows.extra[2].window.value.is_none(),
        "placeholder extra window must never carry a value"
    );
}

#[test]
fn test_no_non_value_state_carries_numeric_values() {
    let snap = sample_usage_snapshot();

    for refresh in [RefreshState::Loading, RefreshState::Error] {
        let mut input = sample_projection_input("claude", refresh);
        input.kept_snapshot = false;
        let projected = provider_snapshot(&snap, &input);

        if let Some(w) = projected.windows.primary {
            assert_ne!(w.state, MetricState::Value);
            assert!(w.value.is_none());
        }
        if let Some(w) = projected.windows.secondary {
            assert_ne!(w.state, MetricState::Value);
            assert!(w.value.is_none());
        }
        for extra in projected.windows.extra {
            assert_ne!(extra.window.state, MetricState::Value);
            assert!(extra.window.value.is_none());
        }
        if let Some(c) = projected.cost {
            assert_ne!(c.state, MetricState::Value);
            assert!(c.value.is_none());
        }
        if let Some(id) = projected.identity {
            assert_ne!(id.state, MetricState::Value);
            assert!(id.value.is_none());
        }
    }
}

#[test]
fn test_identity_silo_cross_provider_isolation() {
    let now = sample_timestamp();
    let identity = ProviderIdentitySnapshot::new(
        Some(ProviderId::new("claude").unwrap()),
        Some("victim-claude-user@example.com".into()),
        Some("Anthropic".into()),
        Some("oauth".into()),
        Some("claude_id_99".into()),
    );

    let snap = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        now,
        Some(identity),
        Confidence::Exact,
    )
    .unwrap();

    // 1. Project for codex -> identity must be SILOED (None)
    let input_codex = sample_projection_input("codex", RefreshState::Fresh);
    let projected_codex = provider_snapshot(&snap, &input_codex);
    assert!(
        projected_codex.identity.is_none(),
        "cross-provider identity must not leak to codex"
    );

    // 2. Project for antigravity -> identity must be SILOED (None)
    let input_agy = sample_projection_input("antigravity", RefreshState::Fresh);
    let projected_agy = provider_snapshot(&snap, &input_agy);
    assert!(
        projected_agy.identity.is_none(),
        "cross-provider identity must not leak to antigravity"
    );

    // 3. Project for claude -> identity projected correctly
    let input_claude = sample_projection_input("claude", RefreshState::Fresh);
    let projected_claude = provider_snapshot(&snap, &input_claude);
    let projected_id = projected_claude.identity.expect("claude identity present");
    assert_eq!(
        projected_id
            .value
            .as_ref()
            .unwrap()
            .account_email
            .as_deref(),
        Some("victim-claude-user@example.com")
    );
    assert_eq!(
        projected_id.value.as_ref().unwrap().plan.as_deref(),
        Some("oauth")
    );
}

#[test]
fn test_window_minutes_clamping() {
    let now = sample_timestamp();
    let primary = RateWindow::new(10.0, Some(0), None, None, None, false).unwrap();
    let secondary = RateWindow::new(20.0, Some(-5), None, None, None, false).unwrap();
    let tertiary =
        RateWindow::new(30.0, Some(u32::MAX as i64 + 1), None, None, None, false).unwrap();

    let extra = vec![
        NamedRateWindow::new(
            "valid",
            "Valid Window",
            RateWindow::new(5.0, Some(300), None, None, None, false).unwrap(),
        ),
        NamedRateWindow::new(
            "max-valid",
            "Max Valid",
            RateWindow::new(5.0, Some(u32::MAX as i64), None, None, None, false).unwrap(),
        ),
    ];

    let snap = UsageSnapshot::new(
        Some(primary),
        Some(secondary),
        Some(tertiary),
        Some(extra),
        None,
        Vec::new(),
        None,
        None,
        now,
        None,
        Confidence::Exact,
    )
    .unwrap();

    let input = sample_projection_input("claude", RefreshState::Fresh);
    let projected = provider_snapshot(&snap, &input);

    assert_eq!(
        projected
            .windows
            .primary
            .unwrap()
            .value
            .unwrap()
            .window_minutes,
        None,
        "window_minutes 0 must clamp to None"
    );
    assert_eq!(
        projected
            .windows
            .secondary
            .unwrap()
            .value
            .unwrap()
            .window_minutes,
        None,
        "window_minutes -5 must clamp to None"
    );
    assert_eq!(
        projected
            .windows
            .tertiary
            .unwrap()
            .value
            .unwrap()
            .window_minutes,
        None,
        "window_minutes > u32::MAX must clamp to None"
    );

    assert_eq!(
        projected.windows.extra[0]
            .window
            .value
            .as_ref()
            .unwrap()
            .window_minutes,
        Some(300)
    );
    assert_eq!(
        projected.windows.extra[1]
            .window
            .value
            .as_ref()
            .unwrap()
            .window_minutes,
        Some(u32::MAX)
    );
}

#[test]
fn test_secret_omission_safe_error_messages() {
    // Secret string built at runtime
    let secret_token = format!("{}_{}_{}", "sk-ant-live", "leaked-secret-body", "889900");

    let categories = [
        ProviderErrorCategory::Auth,
        ProviderErrorCategory::Api,
        ProviderErrorCategory::Parse,
        ProviderErrorCategory::Network,
        ProviderErrorCategory::Configuration,
        ProviderErrorCategory::Unknown,
    ];

    let snap = sample_usage_snapshot();

    for cat in categories {
        let mut input = sample_projection_input("claude", RefreshState::Error);
        input.error = Some(ProjectedError::new(ProviderErrorKind::Unknown, cat));

        let projected = provider_snapshot(&snap, &input);
        let err = projected.last_error.expect("last_error present");

        assert!(
            !err.message.contains(&secret_token),
            "error message must never contain raw/secret text"
        );
        assert_eq!(
            err.message,
            category_safe_message(cat),
            "error message must strictly match category safe description"
        );
    }
}

#[test]
fn test_cost_limit_clamping() {
    let now = sample_timestamp();

    // 1. limit <= 0.0 -> None
    let cost_zero = ProviderCostSnapshot::new(10.0, 0.0, "USD", now);
    let snap_zero = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        Some(cost_zero),
        Vec::new(),
        None,
        None,
        now,
        None,
        Confidence::Exact,
    )
    .unwrap();
    let input = sample_projection_input("claude", RefreshState::Fresh);
    let proj_zero = provider_snapshot(&snap_zero, &input);
    assert_eq!(proj_zero.cost.unwrap().value.unwrap().limit, None);

    // 2. limit NaN -> None
    let cost_nan = ProviderCostSnapshot::new(10.0, f64::NAN, "USD", now);
    let snap_nan = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        Some(cost_nan),
        Vec::new(),
        None,
        None,
        now,
        None,
        Confidence::Exact,
    )
    .unwrap();
    let proj_nan = provider_snapshot(&snap_nan, &input);
    assert_eq!(proj_nan.cost.unwrap().value.unwrap().limit, None);

    // 3. limit positive -> Some(limit)
    let cost_pos = ProviderCostSnapshot::new(10.0, 75.50, "USD", now);
    let snap_pos = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        Some(cost_pos),
        Vec::new(),
        None,
        None,
        now,
        None,
        Confidence::Exact,
    )
    .unwrap();
    let proj_pos = provider_snapshot(&snap_pos, &input);
    assert_eq!(proj_pos.cost.unwrap().value.unwrap().limit, Some(75.50));
}

#[test]
fn test_pace_projection_and_summary_formatting() {
    // 1. On track pace
    let pace_on_track = UsagePace {
        stage: PaceStage::OnTrack,
        delta_percent: 0.8,
        expected_used_percent: 25.4,
        actual_used_percent: 26.2,
        eta_seconds: None,
        will_last_to_reset: true,
        run_out_probability: None,
        speed_multiplier_to_reset: Some(1.0),
    };
    let snap = sample_usage_snapshot();
    let mut input = sample_projection_input("claude", RefreshState::Fresh);
    input.pace = Some(pace_on_track);

    let proj = provider_snapshot(&snap, &input);
    let pace_metric = proj.pace.expect("pace projected");
    assert_eq!(pace_metric.state, MetricState::Value);
    let p = pace_metric.value.expect("pace value");
    assert_eq!(p.stage, PaceStage::OnTrack);
    assert_eq!(p.delta_percent, 1.0);
    assert_eq!(p.expected_used_percent, 25.0);
    assert_eq!(p.summary, "On pace | Expected 25% used | Lasts until reset");

    // 2. Headroom on Codex
    let pace_headroom = UsagePace {
        stage: PaceStage::FarBehind,
        delta_percent: -20.0,
        expected_used_percent: 40.0,
        actual_used_percent: 20.0,
        eta_seconds: None,
        will_last_to_reset: true,
        run_out_probability: None,
        speed_multiplier_to_reset: Some(2.0),
    };
    let mut input_codex = sample_projection_input("codex", RefreshState::Fresh);
    input_codex.pace = Some(pace_headroom.clone());
    let proj_codex = provider_snapshot(&snap, &input_codex);
    assert_eq!(
        proj_codex.pace.unwrap().value.unwrap().summary,
        "20% in reserve | Expected 40% used | Lasts until reset | 1.5× headroom"
    );

    // 3. Same pace on Claude (no headroom hint capability)
    let mut input_claude = sample_projection_input("claude", RefreshState::Fresh);
    input_claude.pace = Some(pace_headroom);
    let proj_claude = provider_snapshot(&snap, &input_claude);
    assert_eq!(
        proj_claude.pace.unwrap().value.unwrap().summary,
        "20% in reserve | Expected 40% used | Lasts until reset"
    );

    // 4. Deficit with countdown
    let pace_deficit = UsagePace {
        stage: PaceStage::Ahead,
        delta_percent: 15.0,
        expected_used_percent: 50.0,
        actual_used_percent: 65.0,
        eta_seconds: Some(7200.0), // 2 hours
        will_last_to_reset: false,
        run_out_probability: None,
        speed_multiplier_to_reset: None,
    };
    let mut input_deficit = sample_projection_input("claude", RefreshState::Fresh);
    input_deficit.pace = Some(pace_deficit);
    let proj_deficit = provider_snapshot(&snap, &input_deficit);
    let p_def = proj_deficit.pace.unwrap().value.unwrap();
    assert_eq!(p_def.eta_seconds, Some(7200));
    assert_eq!(
        p_def.summary,
        "15% in deficit | Expected 50% used | Runs out in 2h"
    );

    // 5. Runs out now
    let pace_now = UsagePace {
        stage: PaceStage::FarAhead,
        delta_percent: 30.0,
        expected_used_percent: 50.0,
        actual_used_percent: 80.0,
        eta_seconds: Some(0.0),
        will_last_to_reset: false,
        run_out_probability: None,
        speed_multiplier_to_reset: None,
    };
    let mut input_now = sample_projection_input("claude", RefreshState::Fresh);
    input_now.pace = Some(pace_now);
    let proj_now = provider_snapshot(&snap, &input_now);
    assert_eq!(
        proj_now.pace.unwrap().value.unwrap().summary,
        "30% in deficit | Expected 50% used | Runs out now"
    );
}

#[test]
fn test_monthly_boundary_at_timestamp_max() {
    // Tests the re-expressed CodexBar ProviderNumericBoundaryTests.swift:79
    // Timestamp::MAX is at the year 9999 boundary.
    let max_ts = Timestamp::MAX;
    let window = RateWindow::new(
        15.0,
        Some(30 * 24 * 60), // monthly sentinel
        Some(max_ts),
        None,
        None,
        false,
    )
    .unwrap();

    let cap = ProviderPaceCapability::calendar_month_reset_window();
    let resolved = cap.resolved_reset_window_for_pace(&window);

    // Window duration must be resolved without overflow or panic
    assert!(resolved.window_minutes.is_some());
    let minutes = resolved.window_minutes.unwrap();
    assert!(minutes > 0);
    // 31 days in November/December is 44,640 minutes
    assert!((28 * 24 * 60..=31 * 24 * 60).contains(&minutes));

    // Also verify pace computation near Timestamp::MAX does not overflow
    let now = max_ts - jiff::Span::new().hours(24);
    let tz = jiff::tz::TimeZone::UTC;
    let pace = UsagePace::weekly(&resolved, now, Some(10080), None, &tz);
    assert!(pace.is_some());
    let p = pace.unwrap();
    assert!(p.expected_used_percent > 0.0);
}
