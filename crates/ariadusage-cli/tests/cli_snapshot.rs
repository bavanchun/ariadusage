// Ported from CodexBar Tests/CodexBarTests/CLISnapshotTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIUnificationGoldenTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityCLILaneParityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::time::Duration;

use ariadusage_cli::cards_brief_renderer::CLICardsBriefRenderer;
use ariadusage_cli::cards_renderer::{CLICardBuildInput, CLICardModel, CLICardsRenderer};
use ariadusage_cli::format::{ResetTimeDisplayStyle, strip_ansi_codes};
use ariadusage_cli::options::effective_argv;
use ariadusage_cli::output_preferences::OutputFormat;
use ariadusage_cli::provider_meta::{CliProvider, make_provider_window_labels_payload};
use ariadusage_cli::renderer::{CLIRenderer, RenderContext};
use ariadusage_protocol::{
    CliCodexRateLimitResetCredit, CliCodexRateLimitResetCreditsSnapshot, CliCreditsSnapshot,
    CliNamedRateWindow, CliProviderCostSnapshot, CliProviderIdentitySnapshot, CliRateWindow,
    CliUsageSnapshot, ProviderPayload, ProviderStatusPayload, StatusIndicator,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;

// ============================================================================
// CLISnapshotTests.swift
// ============================================================================

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:87 renders text snapshot for codex
#[test]
fn renders_text_snapshot_for_codex() {
    let identity = CliProviderIdentitySnapshot::new(
        Some("codex".to_string()),
        Some("user@example.com".to_string()),
        None,
        Some("pro".to_string()),
    );
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(
            12.0,
            Some(300),
            None,
            Some("today at 3:00 PM".to_string()),
        )),
        Some(CliRateWindow::new(
            25.0,
            Some(10080),
            None,
            Some("Fri at 9:00 AM".to_string()),
        )),
        None,
        Timestamp::UNIX_EPOCH.to_string(),
        Some(identity),
    );
    let credits = CliCreditsSnapshot::new(42.0, Vec::new(), Timestamp::UNIX_EPOCH.to_string());

    let output = CLIRenderer::render_text(
        CliProvider::Codex,
        &snap,
        Some(&credits),
        &RenderContext::new(
            "Codex 1.2.3 (codex-cli)",
            Some(ProviderStatusPayload {
                indicator: StatusIndicator::Minor,
                description: Some("Degraded performance".to_string()),
                updated_at: Some(Timestamp::UNIX_EPOCH.to_string()),
                url: "https://status.example.com".to_string(),
            }),
            false,
            ResetTimeDisplayStyle::Absolute,
        ),
        Timestamp::UNIX_EPOCH,
    );

    assert!(output.contains("Codex 1.2.3 (codex-cli)"));
    assert!(output.contains("Status: Partial outage – Degraded performance"));
    assert!(output.contains("Codex"));
    assert!(output.contains("Session: 88% left"));
    assert!(output.contains("Weekly: 75% left"));
    assert!(output.contains("Credits: 42"));
    assert!(output.contains("Account: user@example.com"));
    assert!(output.contains("Plan: Pro 20x"));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:125 renders Codex limit reset credits
#[test]
fn renders_codex_limit_reset_credits() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let expires_at = now + Duration::from_secs(7200);
    let reset_credits = CliCodexRateLimitResetCreditsSnapshot::new(
        vec![
            CliCodexRateLimitResetCredit::new(
                "credit-1",
                "codex_rate_limits",
                "available",
                Timestamp::UNIX_EPOCH.to_string(),
                Some(expires_at.to_string()),
                None,
                None,
                None,
                None,
            ),
            CliCodexRateLimitResetCredit::new(
                "expired-credit",
                "codex_rate_limits",
                "available",
                Timestamp::UNIX_EPOCH.to_string(),
                Some(now.to_string()),
                None,
                None,
                None,
                None,
            ),
        ],
        99,
        Timestamp::UNIX_EPOCH.to_string(),
    );
    let mut snapshot = CliUsageSnapshot::new(
        Some(CliRateWindow::new(10.0, Some(300), None, None)),
        None,
        None,
        Timestamp::UNIX_EPOCH.to_string(),
        None,
    );
    snapshot.codex_reset_credits = Some(reset_credits);

    let output = CLIRenderer::render_text(
        CliProvider::Codex,
        &snapshot,
        None,
        &RenderContext::new(
            "Codex (oauth)",
            None,
            false,
            ResetTimeDisplayStyle::Countdown,
        ),
        now,
    );

    assert!(output.contains("Limit Reset Credits: 1 available"));
    assert!(output.contains("Next reset credit expires"));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:175 renders Codex prolite plan with multiplier display name
#[test]
fn renders_codex_prolite_plan_with_multiplier_display_name() {
    let identity = CliProviderIdentitySnapshot::new(
        Some("codex".to_string()),
        Some("user@example.com".to_string()),
        None,
        Some("prolite".to_string()),
    );
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(
            12.0,
            Some(300),
            None,
            Some("today at 3:00 PM".to_string()),
        )),
        None,
        None,
        Timestamp::UNIX_EPOCH.to_string(),
        Some(identity),
    );

    let output = CLIRenderer::render_text(
        CliProvider::Codex,
        &snap,
        None,
        &RenderContext::new(
            "Codex 1.2.3 (codex-cli)",
            None,
            false,
            ResetTimeDisplayStyle::Absolute,
        ),
        Timestamp::UNIX_EPOCH,
    );

    assert!(output.contains("Plan: Pro 5x"));
    assert!(!output.contains("Plan: Pro Lite"));
    assert!(!output.contains("Plan: Prolite"));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:204 renders Codex plan only limits as unavailable
#[test]
fn renders_codex_plan_only_limits_as_unavailable() {
    let identity = CliProviderIdentitySnapshot::new(
        Some("codex".to_string()),
        Some("user@example.com".to_string()),
        None,
        Some("pro".to_string()),
    );
    let snap = CliUsageSnapshot::new(
        None,
        None,
        None,
        Timestamp::UNIX_EPOCH.to_string(),
        Some(identity),
    );

    let output = CLIRenderer::render_text(
        CliProvider::Codex,
        &snap,
        None,
        &RenderContext::new(
            "Codex 1.2.3 (codex-cli)",
            None,
            false,
            ResetTimeDisplayStyle::Absolute,
        ),
        Timestamp::UNIX_EPOCH,
    );

    assert!(output.contains("Limits: not available"));
    assert!(output.contains("Account: user@example.com"));
    assert!(output.contains("Plan: Pro 20x"));
    assert!(!output.contains("Session:"));
    assert!(!output.contains("Weekly:"));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:235 renders text snapshot for claude without weekly
#[test]
fn renders_text_snapshot_for_claude_without_weekly() {
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(
            2.0,
            None,
            None,
            Some("3pm (Europe/Vienna)".to_string()),
        )),
        None,
        None,
        Timestamp::UNIX_EPOCH.to_string(),
        None,
    );

    let output = CLIRenderer::render_text(
        CliProvider::Claude,
        &snap,
        None,
        &RenderContext::new(
            "Claude Code 2.0.69 (claude)",
            None,
            false,
            ResetTimeDisplayStyle::Absolute,
        ),
        Timestamp::UNIX_EPOCH,
    );

    assert!(output.contains("Session: 98% left"));
    assert!(!output.contains("Weekly:"));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:257 renders Claude Max multiplier without uppercasing x
#[test]
fn renders_claude_max_multiplier_without_uppercasing_x() {
    let identity = CliProviderIdentitySnapshot::new(
        Some("claude".to_string()),
        None,
        None,
        Some("Claude Max 5x".to_string()),
    );
    let snapshot = CliUsageSnapshot::new(
        Some(CliRateWindow::new(2.0, Some(300), None, None)),
        None,
        None,
        Timestamp::UNIX_EPOCH.to_string(),
        Some(identity),
    );

    let output = CLIRenderer::render_text(
        CliProvider::Claude,
        &snapshot,
        None,
        &RenderContext::new(
            "Claude (oauth)",
            None,
            false,
            ResetTimeDisplayStyle::Absolute,
        ),
        Timestamp::UNIX_EPOCH,
    );

    assert!(output.contains("Plan: Claude Max 5x"));
    assert!(!output.contains("Plan: Claude Max 5X"));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:475 renders pace line when weekly has reset
#[test]
fn renders_pace_line_when_weekly_has_reset() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snap = CliUsageSnapshot::new(
        None,
        Some(CliRateWindow::new(
            50.0,
            Some(10080),
            Some((now + Duration::from_secs(3 * 24 * 60 * 60)).to_string()),
            None,
        )),
        None,
        now.to_string(),
        None,
    );

    let output = CLIRenderer::render_text(
        CliProvider::Codex,
        &snap,
        None,
        &RenderContext::new(
            "Codex 0.0.0 (codex-cli)",
            None,
            false,
            ResetTimeDisplayStyle::Countdown,
        ),
        now,
    );

    assert!(output.contains("Pace:"));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:501 configured work days affect weekly text and JSON pace
#[test]
fn configured_work_days_affect_weekly_text_and_json_pace() {
    let tz = TimeZone::system();
    let resets_at = jiff::civil::date(2026, 6, 14)
        .at(0, 0, 0, 0)
        .to_zoned(tz)
        .unwrap()
        .timestamp();
    let now = resets_at - Duration::from_secs(72 * 60 * 60);
    let snap = CliUsageSnapshot::new(
        None,
        Some(CliRateWindow::new(
            60.0,
            Some(10080),
            Some(resets_at.to_string()),
            None,
        )),
        None,
        now.to_string(),
        None,
    );

    let output = CLIRenderer::render_text(
        CliProvider::Codex,
        &snap,
        None,
        &RenderContext::new(
            "Codex 0.0.0 (codex-cli)",
            None,
            false,
            ResetTimeDisplayStyle::Countdown,
        )
        .with_weekly_work_days(Some(5)),
        now,
    );
    assert!(output.contains("Pace: On pace | Expected 60% used | Lasts until reset"));

    let pace = CLIRenderer::provider_pace_payload(CliProvider::Codex, &snap, now, Some(5))
        .and_then(|p| p.secondary)
        .expect("secondary pace");
    assert_eq!(pace.expected_used_percent, 60.0);
    assert_eq!(
        pace.summary,
        "On pace | Expected 60% used | Lasts until reset"
    );
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:896 renders session pace line when session window has reset
#[test]
fn renders_session_pace_line_when_session_window_has_reset() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(
            20.0,
            Some(300),
            Some((now + Duration::from_secs(2 * 60 * 60)).to_string()),
            None,
        )),
        None,
        None,
        now.to_string(),
        None,
    );

    let output = CLIRenderer::render_text(
        CliProvider::Codex,
        &snap,
        None,
        &RenderContext::new(
            "Codex 0.0.0 (codex-cli)",
            None,
            false,
            ResetTimeDisplayStyle::Countdown,
        ),
        now,
    );

    assert!(output.contains("Session: 80% left"));
    assert!(
        output.contains(
            "Pace: 40% in reserve | Expected 60% used | Lasts until reset | 1.5× headroom"
        )
    );
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:925 renders Claude session pace using five hour default window
#[test]
fn renders_claude_session_pace_using_five_hour_default_window() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(
            20.0,
            None,
            Some((now + Duration::from_secs(2 * 60 * 60)).to_string()),
            None,
        )),
        None,
        None,
        now.to_string(),
        None,
    );

    let output = CLIRenderer::render_text(
        CliProvider::Claude,
        &snap,
        None,
        &RenderContext::new(
            "Claude Code 2.0.69 (claude)",
            None,
            false,
            ResetTimeDisplayStyle::Countdown,
        ),
        now,
    );

    assert!(output.contains("Pace: 40% in reserve | Expected 60% used | Lasts until reset"));
    assert!(!output.contains("1.5× headroom"));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:954 renders session pace deficit with run out estimate
#[test]
fn renders_session_pace_deficit_with_run_out_estimate() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(
            50.0,
            Some(300),
            Some((now + Duration::from_secs(4 * 60 * 60)).to_string()),
            None,
        )),
        None,
        None,
        now.to_string(),
        None,
    );

    let output = CLIRenderer::render_text(
        CliProvider::Codex,
        &snap,
        None,
        &RenderContext::new(
            "Codex 0.0.0 (codex-cli)",
            None,
            false,
            ResetTimeDisplayStyle::Countdown,
        ),
        now,
    );

    assert!(output.contains("Pace: 30% in deficit | Expected 20% used | Projected empty in"));
    assert!(!output.contains("Runs out"));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:984 renders session pace on track and lasts until reset
#[test]
fn renders_session_pace_on_track_and_lasts_until_reset() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(
            50.0,
            Some(300),
            Some((now + Duration::from_secs(9_000)).to_string()),
            None,
        )),
        None,
        None,
        now.to_string(),
        None,
    );

    let output = CLIRenderer::render_text(
        CliProvider::Codex,
        &snap,
        None,
        &RenderContext::new(
            "Codex 0.0.0 (codex-cli)",
            None,
            false,
            ResetTimeDisplayStyle::Countdown,
        ),
        now,
    );

    assert!(output.contains("Pace: On pace | Expected 50% used | Lasts until reset"));
}

// partial-upstream: Tests/CodexBarTests/CLISnapshotTests.swift:1013 hides session pace for unsupported provider
#[test]
fn hides_session_pace_for_unsupported_provider() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(
            20.0,
            Some(300),
            Some((now + Duration::from_secs(2 * 60 * 60)).to_string()),
            None,
        )),
        None,
        None,
        now.to_string(),
        None,
    );

    let output = CLIRenderer::render_text(
        CliProvider::Antigravity,
        &snap,
        None,
        &RenderContext::new(
            "Antigravity 0.0.0 (antigravity)",
            None,
            false,
            ResetTimeDisplayStyle::Countdown,
        ),
        now,
    );

    assert!(!output.contains("Pace:"));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:1039 hides session pace for non-session primary window
#[test]
fn hides_session_pace_for_non_session_primary_window() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(
            50.0,
            Some(10080),
            Some((now + Duration::from_secs(3 * 24 * 60 * 60)).to_string()),
            None,
        )),
        None,
        None,
        now.to_string(),
        None,
    );

    let output = CLIRenderer::render_text(
        CliProvider::Claude,
        &snap,
        None,
        &RenderContext::new(
            "Claude Code 2.0.69 (claude)",
            None,
            false,
            ResetTimeDisplayStyle::Countdown,
        ),
        now,
    );

    assert!(!output.contains("Pace:"));
    assert!(CLIRenderer::provider_pace_payload(CliProvider::Claude, &snap, now, None).is_none());
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:1069 renders JSON payload
#[test]
fn renders_json_payload() {
    let updated_at = Timestamp::from_second(1_700_000_000).unwrap();
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(50.0, Some(300), None, None)),
        Some(CliRateWindow::new(10.0, Some(10080), None, None)),
        None,
        updated_at.to_string(),
        None,
    );

    let payload = ProviderPayload {
        provider: "codex".to_string(),
        account: None,
        version: Some("1.2.3".to_string()),
        source: "codex-cli".to_string(),
        status: Some(ProviderStatusPayload {
            indicator: StatusIndicator::None,
            description: None,
            updated_at: Some(Timestamp::from_second(1_700_000_010).unwrap().to_string()),
            url: "https://status.example.com".to_string(),
        }),
        usage: Some(snap),
        rate_window_labels: None,
        credits: None,
        reset_credits: None,
        antigravity_plan_info: None,
        openai_dashboard: None,
        diagnostic: Some(
            "Grok team usage is unavailable from the current billing surface.".to_string(),
        ),
        error: None,
        pace: None,
        age_seconds: None,
    };

    let json = serde_json::to_string(&payload).expect("encode JSON payload");
    assert!(json.contains("\"provider\":\"codex\""));
    assert!(json.contains("\"version\":\"1.2.3\""));
    assert!(json.contains("\"status\""));
    assert!(json.contains("status.example.com"));
    assert!(json.contains("Grok team usage is unavailable from the current billing surface."));
    assert!(json.contains("\"primary\""));
    assert!(json.contains("\"windowMinutes\":300"));
    assert!(json.contains(&updated_at.to_string()));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:1111 json pace rounds derived numbers to match usage precision
#[test]
fn json_pace_rounds_derived_numbers_to_match_usage_precision() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(
            79.0,
            Some(300),
            Some((now + Duration::from_secs(5000)).to_string()),
            None,
        )),
        None,
        None,
        now.to_string(),
        None,
    );

    let pace = CLIRenderer::provider_pace_payload(CliProvider::Codex, &snap, now, None);
    let payload = ProviderPayload {
        provider: "codex".to_string(),
        account: None,
        version: None,
        source: "codex-cli".to_string(),
        status: None,
        usage: Some(snap),
        rate_window_labels: None,
        credits: None,
        reset_credits: None,
        antigravity_plan_info: None,
        openai_dashboard: None,
        diagnostic: None,
        error: None,
        pace,
        age_seconds: None,
    };

    let root: serde_json::Value = serde_json::to_value(&payload).unwrap();
    let primary = root.get("pace").and_then(|p| p.get("primary")).unwrap();

    assert_eq!(
        primary.get("expectedUsedPercent").and_then(|v| v.as_f64()),
        Some(72.0)
    );
    assert_eq!(
        primary.get("deltaPercent").and_then(|v| v.as_f64()),
        Some(7.0)
    );
    assert_eq!(
        primary.get("etaSeconds").and_then(|v| v.as_f64()),
        Some(3456.0)
    );
    assert!(primary.get("actualUsedPercent").is_none());
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:1150 json payload includes session and weekly pace with distinct wording
#[test]
fn json_payload_includes_session_and_weekly_pace_with_distinct_wording() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(
            50.0,
            Some(300),
            Some((now + Duration::from_secs(4 * 60 * 60)).to_string()),
            None,
        )),
        Some(CliRateWindow::new(
            90.0,
            Some(10080),
            Some((now + Duration::from_secs(2 * 24 * 60 * 60)).to_string()),
            None,
        )),
        None,
        now.to_string(),
        None,
    );

    let pace = CLIRenderer::provider_pace_payload(CliProvider::Codex, &snap, now, None);
    let payload = ProviderPayload {
        provider: "codex".to_string(),
        account: None,
        version: Some("1.2.3".to_string()),
        source: "codex-cli".to_string(),
        status: None,
        usage: Some(snap),
        rate_window_labels: None,
        credits: None,
        reset_credits: None,
        antigravity_plan_info: None,
        openai_dashboard: None,
        diagnostic: None,
        error: None,
        pace,
        age_seconds: None,
    };

    let root: serde_json::Value = serde_json::to_value(&payload).unwrap();
    let pace_obj = root.get("pace").unwrap();

    let primary = pace_obj.get("primary").unwrap();
    assert_eq!(
        primary.get("stage").and_then(|v| v.as_str()),
        Some("farAhead")
    );
    assert_eq!(
        primary.get("expectedUsedPercent").and_then(|v| v.as_f64()),
        Some(20.0)
    );
    assert_eq!(
        primary.get("deltaPercent").and_then(|v| v.as_f64()),
        Some(30.0)
    );
    assert_eq!(
        primary.get("willLastToReset").and_then(|v| v.as_bool()),
        Some(false)
    );
    assert_eq!(
        primary.get("etaSeconds").and_then(|v| v.as_f64()),
        Some(3600.0)
    );
    assert!(
        primary
            .get("summary")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.contains("30% in deficit | Expected 20% used | Projected empty in"))
    );
    assert!(primary.get("actualUsedPercent").is_none());
    assert!(primary.get("runOutProbability").is_none());

    let secondary = pace_obj.get("secondary").unwrap();
    assert_eq!(
        secondary.get("stage").and_then(|v| v.as_str()),
        Some("farAhead")
    );
    assert!(
        secondary
            .get("summary")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.contains("Runs out in"))
    );
    assert!(
        !secondary
            .get("summary")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.contains("Projected empty"))
    );
}

// partial-upstream: Tests/CodexBarTests/CLISnapshotTests.swift:1205 json omits pace when not applicable
#[test]
fn json_omits_pace_when_not_applicable() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(
            20.0,
            Some(300),
            Some((now + Duration::from_secs(2 * 60 * 60)).to_string()),
            None,
        )),
        None,
        None,
        now.to_string(),
        None,
    );

    let pace = CLIRenderer::provider_pace_payload(CliProvider::Antigravity, &snap, now, None);
    assert!(pace.is_none());
    let payload = ProviderPayload {
        provider: "antigravity".to_string(),
        account: None,
        version: None,
        source: "antigravity".to_string(),
        status: None,
        usage: Some(snap),
        rate_window_labels: None,
        credits: None,
        reset_credits: None,
        antigravity_plan_info: None,
        openai_dashboard: None,
        diagnostic: None,
        error: None,
        pace,
        age_seconds: None,
    };

    let json = serde_json::to_string(&payload).unwrap();
    assert!(!json.contains("\"pace\""));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:1237 json includes only session pace when weekly window missing
#[test]
fn json_includes_only_session_pace_when_weekly_window_missing() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(
            50.0,
            Some(300),
            Some((now + Duration::from_secs(4 * 60 * 60)).to_string()),
            None,
        )),
        None,
        None,
        now.to_string(),
        None,
    );

    let pace = CLIRenderer::provider_pace_payload(CliProvider::Codex, &snap, now, None);
    let payload = ProviderPayload {
        provider: "codex".to_string(),
        account: None,
        version: None,
        source: "codex-cli".to_string(),
        status: None,
        usage: Some(snap),
        rate_window_labels: None,
        credits: None,
        reset_credits: None,
        antigravity_plan_info: None,
        openai_dashboard: None,
        diagnostic: None,
        error: None,
        pace,
        age_seconds: None,
    };

    let root: serde_json::Value = serde_json::to_value(&payload).unwrap();
    let pace_obj = root.get("pace").unwrap();
    assert!(pace_obj.get("primary").is_some_and(|v| v.is_object()));
    assert!(pace_obj.get("secondary").is_none());
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:1270 encodes JSON with secondary null when missing
#[test]
fn encodes_json_with_secondary_null_when_missing() {
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(0.0, None, None, None)),
        None,
        None,
        Timestamp::from_second(1_700_000_000).unwrap().to_string(),
        None,
    );

    let json = serde_json::to_string(&snap).unwrap();
    assert!(json.contains("\"secondary\":null"));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:1289 parses output format
#[test]
fn parses_output_format() {
    assert_eq!(
        OutputFormat::from_argument("json"),
        Some(OutputFormat::Json)
    );
    assert_eq!(
        OutputFormat::from_argument("TEXT"),
        Some(OutputFormat::Text)
    );
    assert_eq!(OutputFormat::from_argument("invalid"), None);
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:1296 defaults to usage when no command provided
#[test]
fn defaults_to_usage_when_no_command_provided() {
    assert_eq!(effective_argv(&[]), vec!["usage".to_string()]);
    assert_eq!(
        effective_argv(&["--format".to_string(), "json".to_string()]).first(),
        Some(&"usage".to_string())
    );
    assert_eq!(
        effective_argv(&[
            "usage".to_string(),
            "--format".to_string(),
            "json".to_string()
        ])
        .first(),
        Some(&"usage".to_string())
    );
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:1303 status line is last and colored when TTY
#[test]
fn status_line_is_last_and_colored_when_tty() {
    let identity = CliProviderIdentitySnapshot::new(
        Some("claude".to_string()),
        None,
        None,
        Some("pro".to_string()),
    );
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(0.0, Some(300), None, None)),
        Some(CliRateWindow::new(0.0, Some(10080), None, None)),
        None,
        Timestamp::UNIX_EPOCH.to_string(),
        Some(identity),
    );

    let output = CLIRenderer::render_text(
        CliProvider::Claude,
        &snap,
        None,
        &RenderContext::new(
            "Claude Code 2.0.58 (claude)",
            Some(ProviderStatusPayload {
                indicator: StatusIndicator::Critical,
                description: Some("Major outage".to_string()),
                updated_at: None,
                url: "https://status.claude.com".to_string(),
            }),
            true,
            ResetTimeDisplayStyle::Absolute,
        ),
        Timestamp::UNIX_EPOCH,
    );

    let lines: Vec<&str> = output.lines().collect();
    assert!(
        lines
            .last()
            .is_some_and(|l| l.contains("Status: Critical issue – Major outage"))
    );
    assert!(output.contains("\x1b[31mStatus"));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:1336 output has ansi when TTY even without status
#[test]
fn output_has_ansi_when_tty_even_without_status() {
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(1.0, Some(300), None, None)),
        None,
        None,
        Timestamp::UNIX_EPOCH.to_string(),
        None,
    );

    let output = CLIRenderer::render_text(
        CliProvider::Codex,
        &snap,
        None,
        &RenderContext::new(
            "Codex 0.0.0 (codex-cli)",
            None,
            true,
            ResetTimeDisplayStyle::Absolute,
        ),
        Timestamp::UNIX_EPOCH,
    );

    assert!(output.contains("\x1b["));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:1357 tty output colors header and usage
#[test]
fn tty_output_colors_header_and_usage() {
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(95.0, Some(300), None, None)),
        Some(CliRateWindow::new(80.0, Some(10080), None, None)),
        None,
        Timestamp::UNIX_EPOCH.to_string(),
        None,
    );

    let output = CLIRenderer::render_text(
        CliProvider::Codex,
        &snap,
        None,
        &RenderContext::new(
            "Codex 0.0.0 (codex-cli)",
            None,
            true,
            ResetTimeDisplayStyle::Absolute,
        ),
        Timestamp::UNIX_EPOCH,
    );

    assert!(output.contains("\x1b[1;95m== Codex 0.0.0 (codex-cli) ==\x1b[0m"));
    assert!(output.contains("Session: \x1b[31m5% left\x1b[0m"));
    assert!(output.contains("Weekly: \x1b[33m20% left\x1b[0m"));
}

// upstream: Tests/CodexBarTests/CLISnapshotTests.swift:1380 status line is plain when no TTY
#[test]
fn status_line_is_plain_when_no_tty() {
    let identity = CliProviderIdentitySnapshot::new(
        Some("codex".to_string()),
        None,
        None,
        Some("pro".to_string()),
    );
    let snap = CliUsageSnapshot::new(
        Some(CliRateWindow::new(0.0, Some(300), None, None)),
        Some(CliRateWindow::new(0.0, Some(10080), None, None)),
        None,
        Timestamp::UNIX_EPOCH.to_string(),
        Some(identity),
    );

    let output = CLIRenderer::render_text(
        CliProvider::Codex,
        &snap,
        None,
        &RenderContext::new(
            "Codex 0.6.0 (codex-cli)",
            Some(ProviderStatusPayload {
                indicator: StatusIndicator::None,
                description: Some("Operational".to_string()),
                updated_at: None,
                url: "https://status.openai.com/".to_string(),
            }),
            false,
            ResetTimeDisplayStyle::Absolute,
        ),
        Timestamp::UNIX_EPOCH,
    );

    assert!(!output.contains("\x1b["));
    assert!(output.contains("Status: Operational – Operational"));
}

// ============================================================================
// CLIUnificationGoldenTests.swift
// ============================================================================

const GOLDEN_NOW_SECS: i64 = 1_700_000_000;

struct GoldenFixture {
    provider: CliProvider,
    snapshot: CliUsageSnapshot,
}

fn golden_now() -> Timestamp {
    Timestamp::from_second(GOLDEN_NOW_SECS).unwrap()
}

fn golden_window(used: f64, minutes: Option<i64>, reset_after_secs: Option<u64>) -> CliRateWindow {
    let resets_at = reset_after_secs.map(|s| (golden_now() + Duration::from_secs(s)).to_string());
    CliRateWindow::new(used, minutes, resets_at, None)
}

fn golden_snapshot(
    primary: Option<CliRateWindow>,
    secondary: Option<CliRateWindow>,
    tertiary: Option<CliRateWindow>,
) -> CliUsageSnapshot {
    CliUsageSnapshot::new(primary, secondary, tertiary, golden_now().to_string(), None)
}

fn golden_cost_snapshot(
    used: f64,
    limit: f64,
    period: &str,
    balance: Option<f64>,
) -> CliUsageSnapshot {
    let mut snap = CliUsageSnapshot::new(None, None, None, golden_now().to_string(), None);
    let mut cost = CliProviderCostSnapshot::new(
        used,
        limit,
        "USD",
        Some(period.to_string()),
        None,
        golden_now().to_string(),
    );
    cost.balance = balance;
    snap.provider_cost = Some(cost);
    snap
}

fn golden_pace_fixtures() -> Vec<GoldenFixture> {
    vec![
        GoldenFixture {
            provider: CliProvider::Codex,
            snapshot: golden_snapshot(
                Some(golden_window(5.0, Some(300), Some(4 * 60 * 60))),
                Some(golden_window(25.0, Some(10080), Some(5 * 24 * 60 * 60))),
                None,
            ),
        },
        GoldenFixture {
            provider: CliProvider::Claude,
            snapshot: golden_snapshot(
                Some(golden_window(40.0, Some(300), Some(4 * 60 * 60))),
                Some(golden_window(50.0, Some(10080), Some(5 * 24 * 60 * 60))),
                None,
            ),
        },
    ]
}

fn golden_text_fixtures() -> Vec<GoldenFixture> {
    let mut list = vec![GoldenFixture {
        provider: CliProvider::Claude,
        snapshot: golden_cost_snapshot(0.0, 0.0, "Extra usage", Some(40.0)),
    }];
    list.extend(golden_pace_fixtures());
    list
}

// partial-upstream: Tests/CodexBarTests/CLIUnificationGoldenTests.swift:9 affected provider text output golden
#[test]
fn affected_provider_text_output_golden() {
    let now = golden_now();
    let output = golden_text_fixtures()
        .into_iter()
        .map(|fixture| {
            CLIRenderer::render_text(
                fixture.provider,
                &fixture.snapshot,
                None,
                &RenderContext::new(
                    fixture.provider.raw_value(),
                    None,
                    false,
                    ResetTimeDisplayStyle::Countdown,
                ),
                now,
            )
        })
        .collect::<Vec<_>>()
        .join("\n---\n");

    let expected = [
        "== claude ==",
        "Extra usage balance: $40.00",
        "---",
        "== codex ==",
        "Session: 95% left [===========-]",
        "Pace: 15% in reserve | Expected 20% used | Lasts until reset",
        "Resets in 4h",
        "Weekly: 75% left [=========---]",
        "Pace: 4% in reserve | Expected 29% used | Lasts until reset",
        "Resets in 5d",
        "---",
        "== claude ==",
        "Session: 60% left [=======-----]",
        "Pace: 20% in deficit | Expected 20% used | Projected empty in 1h 30m",
        "Resets in 4h",
        "Weekly: 50% left [======------]",
        "Pace: 21% in deficit | Expected 29% used | Runs out in 2d",
        "Resets in 5d",
    ]
    .join("\n");

    assert_eq!(output, expected);
}

// partial-upstream: Tests/CodexBarTests/CLIUnificationGoldenTests.swift:188 affected provider JSON output golden
#[test]
fn affected_provider_json_output_golden() {
    let now = golden_now();
    let payloads: Vec<ProviderPayload> = golden_pace_fixtures()
        .into_iter()
        .map(|fixture| {
            let rate_window_labels =
                make_provider_window_labels_payload(fixture.provider, Some(&fixture.snapshot), now);
            let pace =
                CLIRenderer::provider_pace_payload(fixture.provider, &fixture.snapshot, now, None);
            ProviderPayload {
                provider: fixture.provider.raw_value().to_string(),
                account: None,
                version: None,
                source: "fixture".to_string(),
                status: None,
                usage: Some(fixture.snapshot),
                rate_window_labels,
                credits: None,
                reset_credits: None,
                antigravity_plan_info: None,
                openai_dashboard: None,
                diagnostic: None,
                error: None,
                pace,
                age_seconds: None,
            }
        })
        .collect();
    assert!(!payloads.is_empty());

    let actual: serde_json::Value = serde_json::to_value(&payloads).unwrap();
    let expected = serde_json::json!([
      {
        "pace": {
          "primary": {
            "deltaPercent": -15.0,
            "expectedUsedPercent": 20.0,
            "stage": "farBehind",
            "summary": "15% in reserve | Expected 20% used | Lasts until reset",
            "willLastToReset": true
          },
          "secondary": {
            "deltaPercent": -4.0,
            "expectedUsedPercent": 29.0,
            "stage": "slightlyBehind",
            "summary": "4% in reserve | Expected 29% used | Lasts until reset",
            "willLastToReset": true
          }
        },
        "provider": "codex",
        "rateWindowLabels": {
          "primary": "Session",
          "secondary": "Weekly"
        },
        "source": "fixture",
        "usage": {
          "primary": {
            "resetsAt": "2023-11-15T02:13:20Z",
            "usedPercent": 5.0,
            "windowMinutes": 300
          },
          "secondary": {
            "resetsAt": "2023-11-19T22:13:20Z",
            "usedPercent": 25.0,
            "windowMinutes": 10080
          },
          "tertiary": null,
          "updatedAt": "2023-11-14T22:13:20Z"
        }
      },
      {
        "pace": {
          "primary": {
            "deltaPercent": 20.0,
            "etaSeconds": 5400.0,
            "expectedUsedPercent": 20.0,
            "stage": "farAhead",
            "summary": "20% in deficit | Expected 20% used | Projected empty in 1h 30m",
            "willLastToReset": false
          },
          "secondary": {
            "deltaPercent": 21.0,
            "etaSeconds": 172800.0,
            "expectedUsedPercent": 29.0,
            "stage": "farAhead",
            "summary": "21% in deficit | Expected 29% used | Runs out in 2d",
            "willLastToReset": false
          }
        },
        "provider": "claude",
        "rateWindowLabels": {
          "primary": "Session",
          "secondary": "Weekly"
        },
        "source": "fixture",
        "usage": {
          "primary": {
            "resetsAt": "2023-11-15T02:13:20Z",
            "usedPercent": 40.0,
            "windowMinutes": 300
          },
          "secondary": {
            "resetsAt": "2023-11-19T22:13:20Z",
            "usedPercent": 50.0,
            "windowMinutes": 10080
          },
          "tertiary": null,
          "updatedAt": "2023-11-14T22:13:20Z"
        }
      }
    ]);

    assert_eq!(actual, expected);
}

// ============================================================================
// AntigravityCLILaneParityTests.swift
// ============================================================================

const AGY_LANE_NOW_SECS: i64 = 1_800_000_000;

fn agy_now() -> Timestamp {
    Timestamp::from_second(AGY_LANE_NOW_SECS).unwrap()
}

fn agy_identity() -> CliProviderIdentitySnapshot {
    CliProviderIdentitySnapshot::new(
        Some("antigravity".to_string()),
        Some("quota-user@example.com".to_string()),
        None,
        Some("google ai pro".to_string()),
    )
}

fn agy_render_context() -> RenderContext {
    RenderContext::new(
        "Antigravity (cli)",
        None,
        false,
        ResetTimeDisplayStyle::Countdown,
    )
}

fn agy_rendered_text(snapshot: &CliUsageSnapshot) -> String {
    CLIRenderer::render_text(
        CliProvider::Antigravity,
        snapshot,
        None,
        &agy_render_context(),
        agy_now(),
    )
}

fn agy_card(snapshot: &CliUsageSnapshot) -> CLICardModel {
    CLICardsRenderer::make_card(CLICardBuildInput {
        provider: CliProvider::Antigravity,
        snapshot,
        credits: None,
        source: "fixture",
        status: None,
        account_override: None,
        fallback_account_email: None,
        is_active: false,
        uses_last_known_usage: false,
        account_problem: None,
        notes: &[],
        use_color: false,
        reset_style: ResetTimeDisplayStyle::Countdown,
        weekly_work_days: None,
        now: agy_now(),
    })
}

fn agy_card_metric_labels(snapshot: &CliUsageSnapshot) -> Vec<String> {
    CLIRenderer::collect_card_metrics(
        CliProvider::Antigravity,
        snapshot,
        ResetTimeDisplayStyle::Countdown,
        agy_now(),
        None,
    )
    .into_iter()
    .map(|m| m.label)
    .collect()
}

fn agy_quota_summary_snapshot(
    gemini_session_percent: f64,
    gemini_weekly_percent: f64,
    third_party_session_percent: f64,
    include_unknown_third_party_lane: bool,
) -> CliUsageSnapshot {
    let now = agy_now();
    let gemini_session = CliRateWindow::new(
        gemini_session_percent,
        Some(300),
        Some((now + Duration::from_secs(34 * 60)).to_string()),
        None,
    );
    let gemini_weekly = CliRateWindow::new(
        gemini_weekly_percent,
        Some(10080),
        Some((now + Duration::from_secs((2 * 24 + 15) * 60 * 60)).to_string()),
        None,
    );
    let third_party_session = CliRateWindow::new(
        third_party_session_percent,
        Some(300),
        Some((now + Duration::from_secs(5 * 60 * 60)).to_string()),
        None,
    );

    let mut extras = vec![
        CliNamedRateWindow::new(
            "antigravity-quota-summary-gemini-5h",
            "Gemini 5-hour",
            gemini_session.clone(),
        ),
        CliNamedRateWindow::new(
            "antigravity-quota-summary-gemini-weekly",
            "Gemini weekly",
            gemini_weekly,
        ),
        CliNamedRateWindow::new(
            "antigravity-quota-summary-3p-5h",
            "Claude/GPT 5-hour",
            third_party_session.clone(),
        ),
    ];
    if include_unknown_third_party_lane {
        extras.push(CliNamedRateWindow::with_usage_known(
            "antigravity-quota-summary-3p-weekly",
            "Claude/GPT weekly",
            CliRateWindow::new(0.0, Some(10080), None, None),
            false,
        ));
    }

    let mut snap = CliUsageSnapshot::new(
        Some(gemini_session),
        Some(third_party_session),
        None,
        now.to_string(),
        Some(agy_identity()),
    );
    snap.extra_rate_windows = Some(extras);
    snap
}

fn agy_unknown_snapshot(include_known_lane: bool) -> CliUsageSnapshot {
    let now = agy_now();
    let unknown = CliNamedRateWindow::with_usage_known(
        "antigravity-quota-summary-gemini-weekly",
        "Gemini weekly",
        CliRateWindow::new(
            0.0,
            Some(10080),
            Some((now + Duration::from_secs(7 * 86400)).to_string()),
            None,
        ),
        false,
    );
    let known = CliNamedRateWindow::new(
        "antigravity-quota-summary-3p-5h",
        "Claude/GPT 5-hour",
        CliRateWindow::new(25.0, Some(300), None, None),
    );
    let mut extras = vec![unknown];
    if include_known_lane {
        extras.push(known.clone());
    }
    let mut snap = CliUsageSnapshot::new(
        None,
        include_known_lane.then_some(known.window),
        None,
        now.to_string(),
        Some(agy_identity()),
    );
    snap.extra_rate_windows = Some(extras);
    snap
}

fn agy_legacy_model_quotas_snapshot() -> CliUsageSnapshot {
    let now = agy_now();
    CliUsageSnapshot::new(
        Some(CliRateWindow::new(2.0, Some(300), None, None)),
        Some(CliRateWindow::new(4.0, Some(10080), None, None)),
        None,
        now.to_string(),
        Some(agy_identity()),
    )
}

// upstream: Tests/CodexBarTests/AntigravityCLILaneParityTests.swift:155 unknown and disabled first buckets stay unavailable in text and cards
#[test]
fn unknown_and_disabled_first_buckets_stay_unavailable_in_text_and_cards() {
    for disabled in [false, true] {
        let now = agy_now();
        let unknown_used_percent = if disabled { 70.0 } else { 0.0 };
        let unknown_lane = CliNamedRateWindow::with_usage_known(
            "antigravity-quota-summary-gemini-5h",
            "Gemini 5-hour",
            CliRateWindow::new(
                unknown_used_percent,
                Some(300),
                None,
                Some("Fixture session reset".to_string()),
            ),
            false,
        );
        let known_lane = CliNamedRateWindow::new(
            "antigravity-quota-summary-gemini-weekly",
            "Gemini weekly",
            CliRateWindow::new(4.0, Some(10080), None, None),
        );
        let mut snapshot = CliUsageSnapshot::new(
            Some(known_lane.window.clone()),
            None,
            None,
            now.to_string(),
            None,
        );
        snapshot.extra_rate_windows = Some(vec![unknown_lane, known_lane]);

        assert!(
            snapshot
                .extra_rate_windows
                .as_ref()
                .and_then(|w| w.first())
                .is_some_and(|w| !w.usage_known)
        );
        let text = agy_rendered_text(&snapshot);
        let compact = CLIRenderer::render_card_body_lines_with_identity(
            CliProvider::Antigravity,
            &snapshot,
            None,
            &agy_render_context(),
            now,
            false,
        )
        .join("\n");
        for rendered in [&text, &compact] {
            assert!(rendered.contains("Gemini 5-hour: Unavailable"));
            assert!(rendered.contains("Fixture session reset"));
            assert!(!rendered.contains("100% left"));
            assert!(!rendered.contains("30% left"));
            assert!(rendered.contains("Gemini weekly: 96% left"));
        }

        let card = CLICardsRenderer::make_card(CLICardBuildInput {
            provider: CliProvider::Antigravity,
            snapshot: &snapshot,
            credits: None,
            source: "cli",
            status: None,
            account_override: None,
            fallback_account_email: None,
            is_active: false,
            uses_last_known_usage: false,
            account_problem: None,
            notes: &[],
            use_color: false,
            reset_style: ResetTimeDisplayStyle::Countdown,
            weekly_work_days: None,
            now,
        });
        assert_eq!(
            card.metrics
                .iter()
                .map(|m| m.remaining_percent.is_some())
                .collect::<Vec<_>>(),
            vec![false, true]
        );
        let full = CLICardsRenderer::render(std::slice::from_ref(&card), &[], 80, false, false);
        assert!(
            full.lines()
                .find(|l| l.contains("Gemini 5-hour"))
                .is_some_and(|l| l.contains("Unavailable"))
        );
        assert!(full.contains("Fixture session reset"));
        assert!(!full.contains("100% left"));
        assert!(!full.contains("30% left"));

        let rows = CLICardsBriefRenderer::make_rows(&[card]);
        assert!(rows.first().unwrap().used_percent.is_none());
        assert!(
            rows.first()
                .and_then(|r| r.reset_label.as_deref())
                .is_some_and(|l| l.contains("Fixture session reset"))
        );
        let brief = CLICardsBriefRenderer::render(&rows, &[], 80, false, false, now);
        assert!(!brief.contains('%'));
    }
}

// upstream: Tests/CodexBarTests/AntigravityCLILaneParityTests.swift:215 quota summary snapshot renders one lane per bucket with no collapsed lane or pace
#[test]
fn quota_summary_snapshot_renders_one_lane_per_bucket_with_no_collapsed_lane_or_pace() {
    let text = agy_rendered_text(&agy_quota_summary_snapshot(2.0, 4.0, 1.0, false));

    assert!(text.contains("== Antigravity (cli) =="));
    assert!(text.contains("Gemini 5-hour: 98% left"));
    assert!(text.contains("Resets in 34m"));
    assert!(text.contains("Gemini weekly: 96% left"));
    assert!(text.contains("Resets in 2d 15h"));
    assert!(text.contains("Claude/GPT 5-hour: 99% left"));
    assert!(text.contains("Resets in 5h"));
    assert!(text.contains("Account: quota-user@example.com"));
    assert!(text.contains("Plan: Google Ai Pro"));

    assert!(!text.contains("Gemini Models"));
    assert!(!text.contains("Claude and GPT"));
    assert!(!text.contains("Pace:"));
}

// upstream: Tests/CodexBarTests/AntigravityCLILaneParityTests.swift:237 an untouched quota family drops out of the rendered lanes
#[test]
fn an_untouched_quota_family_drops_out_of_the_rendered_lanes() {
    let text = agy_rendered_text(&agy_quota_summary_snapshot(2.0, 4.0, 0.0, false));

    assert!(text.contains("Gemini 5-hour: 98% left"));
    assert!(text.contains("Gemini weekly: 96% left"));
    assert!(!text.contains("Claude/GPT"));
}

// upstream: Tests/CodexBarTests/AntigravityCLILaneParityTests.swift:249 a lane with unknown usage keeps its family visible
#[test]
fn a_lane_with_unknown_usage_keeps_its_family_visible() {
    let text = agy_rendered_text(&agy_quota_summary_snapshot(2.0, 4.0, 0.0, true));

    assert!(text.contains("Claude/GPT 5-hour: 100% left"));
    assert!(text.contains("Claude/GPT weekly: Unavailable"));
    assert!(!text.contains("Claude/GPT weekly: 100% left"));
}

// upstream: Tests/CodexBarTests/AntigravityCLILaneParityTests.swift:262 every family untouched keeps all lanes visible
#[test]
fn every_family_untouched_keeps_all_lanes_visible() {
    let text = agy_rendered_text(&agy_quota_summary_snapshot(0.0, 0.0, 0.0, false));

    assert!(text.contains("Gemini 5-hour: 100% left"));
    assert!(text.contains("Gemini weekly: 100% left"));
    assert!(text.contains("Claude/GPT 5-hour: 100% left"));
}

// upstream: Tests/CodexBarTests/AntigravityCLILaneParityTests.swift:274 legacy modelQuotas snapshot keeps the collapsed lane rendering unchanged
#[test]
fn legacy_model_quotas_snapshot_keeps_the_collapsed_lane_rendering_unchanged() {
    let text = agy_rendered_text(&agy_legacy_model_quotas_snapshot());

    assert!(text.contains("Gemini Models: 98% left"));
    assert!(text.contains("Claude and GPT: 96% left"));
    assert!(!text.contains("Gemini 5-hour"));
    assert!(!text.contains("Claude/GPT"));
}

// upstream: Tests/CodexBarTests/AntigravityCLILaneParityTests.swift:284 cards metric collection mirrors the quota summary lanes
#[test]
fn cards_metric_collection_mirrors_the_quota_summary_lanes() {
    let snapshot = agy_quota_summary_snapshot(2.0, 4.0, 1.0, false);
    let metrics = CLIRenderer::collect_card_metrics(
        CliProvider::Antigravity,
        &snapshot,
        ResetTimeDisplayStyle::Countdown,
        agy_now(),
        None,
    );

    assert_eq!(
        metrics.iter().map(|m| m.label.as_str()).collect::<Vec<_>>(),
        vec!["Gemini 5-hour", "Gemini weekly", "Claude/GPT 5-hour"]
    );
    assert_eq!(
        metrics
            .iter()
            .map(|m| m.remaining_percent.map(f64::round))
            .collect::<Vec<_>>(),
        vec![Some(98.0), Some(96.0), Some(99.0)]
    );
}

// upstream: Tests/CodexBarTests/AntigravityCLILaneParityTests.swift:300 cards metric collection drops an untouched family
#[test]
fn cards_metric_collection_drops_an_untouched_family() {
    let labels = agy_card_metric_labels(&agy_quota_summary_snapshot(2.0, 4.0, 0.0, false));
    assert_eq!(labels, vec!["Gemini 5-hour", "Gemini weekly"]);
}

// upstream: Tests/CodexBarTests/AntigravityCLILaneParityTests.swift:310 cards metric collection keeps legacy lanes when there is no quota summary
#[test]
fn cards_metric_collection_keeps_legacy_lanes_when_there_is_no_quota_summary() {
    assert_eq!(
        agy_card_metric_labels(&agy_legacy_model_quotas_snapshot()),
        vec!["Gemini Models", "Claude and GPT"]
    );
}

// upstream: Tests/CodexBarTests/AntigravityCLILaneParityTests.swift:315 unknown quota text and compact rows retain reset context without invented capacity
#[test]
fn unknown_quota_text_and_compact_rows_retain_reset_context_without_invented_capacity() {
    let snapshot = agy_unknown_snapshot(false);
    let text = agy_rendered_text(&snapshot);
    let compact = CLIRenderer::render_card_body_lines_with_identity(
        CliProvider::Antigravity,
        &snapshot,
        None,
        &agy_render_context(),
        agy_now(),
        false,
    )
    .join("\n");
    for output in [&text, &compact] {
        assert!(output.contains("Gemini weekly: Unavailable"));
        assert!(output.contains("Resets in 7d"));
        assert!(!output.contains("% left"));
        assert!(!output.contains("[============]"));
    }
}

// upstream: Tests/CodexBarTests/AntigravityCLILaneParityTests.swift:334 full cards show unknown quota without a percentage or bar
#[test]
fn full_cards_show_unknown_quota_without_a_percentage_or_bar() {
    for enhanced in [false, true] {
        let card = agy_card(&agy_unknown_snapshot(false));
        assert!(card.metrics.first().unwrap().remaining_percent.is_none());
        let output = strip_ansi_codes(&CLICardsRenderer::render(&[card], &[], 80, true, enhanced));
        assert!(output.contains("Gemini weekly"));
        assert!(output.contains("Unavailable"));
        assert!(output.contains("Reset in 7d"));
        assert!(!output.contains("% left"));
        assert!(!output.contains('━'));
    }
}

// upstream: Tests/CodexBarTests/AntigravityCLILaneParityTests.swift:347 brief cards preserve an unknown first quota ahead of a known quota
#[test]
fn brief_cards_preserve_an_unknown_first_quota_ahead_of_a_known_quota() {
    let card = agy_card(&agy_unknown_snapshot(true));
    let rows = CLICardsBriefRenderer::make_rows(std::slice::from_ref(&card));
    assert_eq!(card.metrics.len(), 2);
    assert_eq!(
        rows.first().and_then(|r| r.metric_label.as_deref()),
        Some("Gemini weekly")
    );
    assert!(rows.first().unwrap().used_percent.is_none());
    assert_eq!(
        rows.first().and_then(|r| r.reset_at),
        Some(agy_now() + Duration::from_secs(7 * 86400))
    );
    let output = CLICardsBriefRenderer::render(&rows, &[], 120, false, false, agy_now());
    assert!(output.contains("7d"));
    assert!(!output.contains("0%"));
}

// Rust-added: --all-accounts fetches and renders every selected account in order
#[test]
fn all_accounts_fetches_and_renders_every_selected_account_in_order() {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use ariadusage_cli::options::parse_usage_args;
    use ariadusage_cli::terminal::TerminalCapabilities;
    use ariadusage_cli::usage_command::execute_in_process_usage_with_runtime;
    use ariadusage_core::config::{
        Config, ProviderConfig, ProviderEntry, TokenAccountMeta, TokenAccountsMeta,
    };
    use ariadusage_core::model::{RateWindow, UsageSnapshot};
    use ariadusage_core::pipeline::{
        BoxFuture, FetchContext, FetchError, FetchResult, FetchStrategy, ProviderPlan,
    };
    use ariadusage_core::providers::SourceMode;
    use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
    use ariadusage_engine::brokers::secret_store::{SecretStore, service::ServiceBackend};
    use ariadusage_engine::config_store::ConfigStore;
    use ariadusage_engine::registry::{ProviderPlanFactory, ProviderPlanInputs, ProviderRegistry};
    use ariadusage_engine::runtime::EngineRuntime;
    use ariadusage_protocol::ids::ProviderId;
    use ariadusage_protocol::metric::{Confidence, SourceKind};

    struct AccountRecordingStrategy {
        label: String,
        used_percent: f64,
        now: Timestamp,
    }

    impl FetchStrategy for AccountRecordingStrategy {
        fn id(&self) -> &str {
            "codex.oauth"
        }

        fn kind(&self) -> SourceKind {
            SourceKind::Oauth
        }

        fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
            Box::pin(async { true })
        }

        fn fetch<'a>(
            &'a self,
            _cx: &'a FetchContext,
        ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
            let label = self.label.clone();
            let used_percent = self.used_percent;
            let now = self.now;
            Box::pin(async move {
                let primary = RateWindow::new(
                    used_percent,
                    Some(300),
                    Some(now + Duration::from_secs(3600)),
                    None,
                    None,
                    false,
                )
                .unwrap();
                let usage = UsageSnapshot::new(
                    Some(primary),
                    None,
                    None,
                    None,
                    None,
                    Vec::new(),
                    None,
                    None,
                    now,
                    None,
                    Confidence::Exact,
                )
                .unwrap();
                Ok(FetchResult::new(
                    usage,
                    "oauth",
                    format!("codex.oauth.{label}"),
                    SourceKind::Oauth,
                ))
            })
        }

        fn should_fallback(&self, _err: &FetchError, _cx: &FetchContext) -> bool {
            false
        }
    }

    struct AccountRecordingPlan {
        label: String,
        used_percent: f64,
        now: Timestamp,
    }

    impl ProviderPlan for AccountRecordingPlan {
        fn resolve_strategies<'a>(
            &'a self,
            _cx: &'a FetchContext,
        ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
            let strategy: Box<dyn FetchStrategy> = Box::new(AccountRecordingStrategy {
                label: self.label.clone(),
                used_percent: self.used_percent,
                now: self.now,
            });
            Box::pin(async move { vec![strategy] })
        }
    }

    struct AccountRecordingFactory {
        fetched_labels: Arc<Mutex<Vec<String>>>,
        now: Timestamp,
    }

    impl ProviderPlanFactory for AccountRecordingFactory {
        fn build_plan(
            &self,
            _provider_config: Option<&ProviderConfig>,
            selected_account: Option<&TokenAccountMeta>,
            _inputs: &ProviderPlanInputs,
        ) -> Arc<dyn ProviderPlan> {
            let label = selected_account
                .map(|a| a.label.clone())
                .unwrap_or_else(|| "default".to_string());
            self.fetched_labels.lock().unwrap().push(label.clone());
            let used_percent = if label == "alpha@example.com" {
                25.0
            } else {
                60.0
            };
            Arc::new(AccountRecordingPlan {
                label,
                used_percent,
                now: self.now,
            })
        }
    }

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let cfg_path = temp.path().join("config.json");

    let codex_id = ProviderId::new("codex").unwrap();
    let mut codex_cfg = ProviderConfig::new(codex_id.clone());
    codex_cfg.enabled = Some(true);
    codex_cfg.source = Some(SourceMode::Oauth);
    codex_cfg.token_accounts = Some(TokenAccountsMeta {
        version: 1,
        active_index: 0,
        accounts: vec![
            TokenAccountMeta {
                id: "00000000-0000-0000-0000-000000000001".to_string(),
                label: "alpha@example.com".to_string(),
                added_at: 1_700_000_000.0,
                last_used: None,
                external_identifier: None,
                usage_scope: None,
                organization_id: None,
                workspace_id: None,
                claude_config_directory: None,
                seat_credit_entitlement: None,
                model_rate_limits: None,
                extra: BTreeMap::new(),
            },
            TokenAccountMeta {
                id: "00000000-0000-0000-0000-000000000002".to_string(),
                label: "beta@example.com".to_string(),
                added_at: 1_700_000_001.0,
                last_used: None,
                external_identifier: None,
                usage_scope: None,
                organization_id: None,
                workspace_id: None,
                claude_config_directory: None,
                seat_credit_entitlement: None,
                model_rate_limits: None,
                extra: BTreeMap::new(),
            },
        ],
    });
    let config = Config::new(
        Config::CURRENT_VERSION,
        vec![ProviderEntry::Typed(codex_cfg)],
    );

    let fetched_labels = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ProviderRegistry::empty().with_directories(temp.path(), temp.path());
    registry.register_factory(
        codex_id,
        Arc::new(AccountRecordingFactory {
            fetched_labels: Arc::clone(&fetched_labels),
            now,
        }),
    );

    let runtime = Arc::new(EngineRuntime::new(
        ConfigStore::new(cfg_path),
        Arc::new(SecretStore::new(ServiceBackend::new(true))),
        Arc::new(NetBroker::new(NetConfig::default()).unwrap()),
        registry,
    ));

    let argv: Vec<String> = ["--provider", "codex", "--all-accounts", "--no-color"]
        .into_iter()
        .map(str::to_string)
        .collect();
    let (opts, parsed) = parse_usage_args(&argv).expect("parse --all-accounts");
    let text_res = execute_in_process_usage_with_runtime(
        &runtime,
        &config,
        &opts,
        &parsed,
        false,
        TerminalCapabilities::default(),
        now,
    );
    assert_eq!(text_res.exit_code, 0);
    assert_eq!(
        *fetched_labels.lock().unwrap(),
        vec![
            "alpha@example.com".to_string(),
            "beta@example.com".to_string()
        ]
    );
    let alpha_pos = text_res
        .stdout
        .find("Account: alpha@example.com")
        .expect("alpha account rendered");
    let beta_pos = text_res
        .stdout
        .find("Account: beta@example.com")
        .expect("beta account rendered");
    assert!(
        alpha_pos < beta_pos,
        "accounts must render in selection order"
    );
    assert!(text_res.stdout.contains("75% left"));
    assert!(text_res.stdout.contains("40% left"));

    let json_argv: Vec<String> = ["--provider", "codex", "--all-accounts", "--format", "json"]
        .into_iter()
        .map(str::to_string)
        .collect();
    let (json_opts, json_parsed) = parse_usage_args(&json_argv).expect("parse json --all-accounts");
    let json_res = execute_in_process_usage_with_runtime(
        &runtime,
        &config,
        &json_opts,
        &json_parsed,
        false,
        TerminalCapabilities::default(),
        now,
    );
    assert_eq!(json_res.exit_code, 0);
    let payloads: Vec<ProviderPayload> =
        serde_json::from_str(&json_res.stdout).expect("valid JSON array");
    assert_eq!(payloads.len(), 2);
    assert_eq!(payloads[0].account.as_deref(), Some("alpha@example.com"));
    assert_eq!(
        payloads[0]
            .usage
            .as_ref()
            .and_then(|u| u.primary.as_ref())
            .map(|w| w.used_percent),
        Some(25.0)
    );
    assert_eq!(payloads[1].account.as_deref(), Some("beta@example.com"));
    assert_eq!(
        payloads[1]
            .usage
            .as_ref()
            .and_then(|u| u.primary.as_ref())
            .map(|w| w.used_percent),
        Some(60.0)
    );
}
