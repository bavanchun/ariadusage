// Ported from CodexBar Tests/CodexBarTests/CLICardsRendererTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLICardsClaudeSwapTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIUsageClaudeSwapTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::fs;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ariadusage_cli::cards_brief_renderer::CLICardsBriefRenderer;
use ariadusage_cli::cards_renderer::{
    CLICardBuildInput, CLICardFailure, CLICardModel, CLICardsRenderer,
};
use ariadusage_cli::claude_swap_cards::{
    CLIClaudeSwapCards, CLIClaudeSwapCardsRenderOptions, CLIClaudeSwapText, ClaudeSwapAccountList,
    ClaudeSwapAccountProjection, ClaudeSwapAccountReader, ClaudeSwapAccountReaderError,
    ClaudeSwapAccountRow, ClaudeSwapAccountSource, ClaudeSwapListParser, ClaudeSwapListParserError,
    ClaudeSwapUsageStatus, ClaudeSwapUsageWindow, ProviderAccountIdentity,
    ProviderAccountUsageSnapshot,
};
use ariadusage_cli::exit_code::ExitCode;
use ariadusage_cli::format::{ResetTimeDisplayStyle, strip_ansi_codes};
#[cfg(unix)]
use ariadusage_cli::ipc_client::fetch_daemon_snapshot;
use ariadusage_cli::options::{
    CLIUsageFetchSetup, CliSourceMode, UsageCommandOutput, parse_cards_args, parse_usage_args,
};
use ariadusage_cli::output_preferences::CLIOutputPreferences;
use ariadusage_cli::provider_meta::CliProvider;
use ariadusage_cli::renderer::CliCardMetric;
use ariadusage_cli::status_command::execute_status_from_snapshot;
use ariadusage_cli::terminal::TerminalCapabilities;
use ariadusage_cli::usage_command::execute_usage_from_snapshot;
use ariadusage_core::config::{ProviderConfig, decode, encode};
#[cfg(unix)]
use ariadusage_protocol::ipc::{PROTOCOL_V1, ServerMessage};
use ariadusage_protocol::{
    CliCreditsSnapshot, CliProviderIdentitySnapshot, CliRateWindow, CliUsageSnapshot, Confidence,
    EngineInfo, EngineSnapshot, Metric, ProviderError, ProviderErrorCategory, ProviderErrorKind,
    ProviderId, ProviderSnapshot, ProviderStatus, ProviderStatusPayload, RateWindow,
    StatusIndicator,
};
use jiff::Timestamp;

#[allow(clippy::too_many_arguments)]
fn test_card(
    provider: CliProvider,
    title: &str,
    source_label: &str,
    plan_badge: Option<String>,
    account_line: Option<String>,
    info_lines: Vec<String>,
    metrics: Vec<CliCardMetric>,
    extra_lines: Vec<String>,
    status_line: Option<String>,
) -> CLICardModel {
    CLICardModel::new(
        provider,
        title,
        source_label,
        plan_badge,
        account_line,
        false,
        false,
        None,
        info_lines,
        metrics,
        extra_lines,
        None,
        status_line,
    )
}

fn test_metric(label: &str, remaining_percent: f64, reset_text: Option<String>) -> CliCardMetric {
    CliCardMetric::new(label, Some(remaining_percent), reset_text, None, None)
}

// ============================================================================
// CLICardsRendererTests.swift
// ============================================================================

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:7 computes column count from terminal width
#[test]
fn computes_column_count_from_terminal_width() {
    assert_eq!(
        CLICardsRenderer::column_count(80, CLICardsRenderer::MIN_CARD_WIDTH),
        2
    );
    assert_eq!(
        CLICardsRenderer::column_count(120, CLICardsRenderer::MIN_CARD_WIDTH),
        3
    );
    assert_eq!(
        CLICardsRenderer::column_count(160, CLICardsRenderer::MIN_CARD_WIDTH),
        4
    );
    assert_eq!(
        CLICardsRenderer::column_count(30, CLICardsRenderer::MIN_CARD_WIDTH),
        1
    );
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:15 renders single codex card without color
#[test]
fn renders_single_codex_card_without_color() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let identity = CliProviderIdentitySnapshot::new(
        Some("codex".to_string()),
        Some("user@example.com".to_string()),
        None,
        Some("pro".to_string()),
    );
    let snapshot = CliUsageSnapshot::new(
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
    let credits = CliCreditsSnapshot::new(42.0, Vec::new(), now.to_string());
    let card = CLICardsRenderer::make_card(CLICardBuildInput {
        provider: CliProvider::Codex,
        snapshot: &snapshot,
        credits: Some(&credits),
        source: "oauth",
        status: None,
        account_override: None,
        fallback_account_email: None,
        is_active: false,
        uses_last_known_usage: false,
        account_problem: None,
        notes: &[],
        use_color: false,
        reset_style: ResetTimeDisplayStyle::Absolute,
        weekly_work_days: None,
        now,
    });

    let output = CLICardsRenderer::render(&[card], &[], 80, false, false);

    assert!(output.contains("Codex"));
    assert!(output.contains("[oauth]"));
    assert!(output.contains("PLAN Pro 20x"));
    assert!(output.contains("Session"));
    assert!(output.contains("88% left"));
    assert!(output.contains("[ "));
    assert!(output.contains("━"));
    assert!(output.contains("Credits:"));
    assert!(output.contains("42 left"));
    assert!(output.contains("@ user@example.com"));
    assert!(output.contains("╰"));
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:55 card includes account line
#[test]
fn card_includes_account_line() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let identity = CliProviderIdentitySnapshot::new(
        Some("codex".to_string()),
        Some("user@example.com".to_string()),
        None,
        Some("pro".to_string()),
    );
    let snapshot = CliUsageSnapshot::new(
        Some(CliRateWindow::new(12.0, Some(300), None, None)),
        None,
        None,
        Timestamp::UNIX_EPOCH.to_string(),
        Some(identity),
    );
    let card = CLICardsRenderer::make_card(CLICardBuildInput {
        provider: CliProvider::Codex,
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
        reset_style: ResetTimeDisplayStyle::Absolute,
        weekly_work_days: None,
        now,
    });

    let lines = CLICardsRenderer::render_card(&card, 48, false, false);
    let joined = lines.join("\n");

    assert!(joined.contains("@ user@example.com"));
    assert!(joined.contains("Session"));
    assert!(!joined.contains("Plan: Pro 20x"));
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:88 renders two card grid at fixed width
#[test]
fn renders_two_card_grid_at_fixed_width() {
    let codex = test_card(
        CliProvider::Codex,
        "Codex",
        "oauth",
        Some("Pro".to_string()),
        None,
        Vec::new(),
        vec![test_metric("Session", 88.0, None)],
        Vec::new(),
        None,
    );
    let claude = test_card(
        CliProvider::Claude,
        "Claude",
        "web",
        Some("Max".to_string()),
        None,
        Vec::new(),
        vec![test_metric("Session", 50.0, None)],
        Vec::new(),
        None,
    );

    let output = CLICardsRenderer::render(&[codex, claude], &[], 120, false, false);

    assert!(output.contains("Codex"));
    assert!(output.contains("Claude"));
    assert!(output.contains("88% left"));
    assert!(output.contains("50% left"));
    assert!(output.split('╰').count() >= 3);
}

// partial-upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:120 renders failure footer without cards
#[test]
fn renders_failure_footer_without_cards() {
    let failures = vec![CLICardFailure::new(
        CliProvider::Claude,
        None,
        "not configured",
    )];
    let output = CLICardsRenderer::render(&[], &failures, 80, false, false);

    assert!(output.contains("Failed providers:"));
    assert!(output.contains("Claude: not configured"));
}

// partial-upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:131 appends failure footer after successful cards
#[test]
fn appends_failure_footer_after_successful_cards() {
    let card = test_card(
        CliProvider::Codex,
        "Codex",
        "oauth",
        None,
        None,
        Vec::new(),
        vec![test_metric("Session", 88.0, None)],
        Vec::new(),
        None,
    );
    let failures = vec![CLICardFailure::new(
        CliProvider::Antigravity,
        None,
        "timeout",
    )];

    let output = CLICardsRenderer::render(&[card], &failures, 80, false, false);

    assert!(output.contains("88% left"));
    assert!(output.contains("Failed providers:"));
    assert!(output.contains("Antigravity: timeout"));
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:154 brief mode renders usage table
#[test]
fn brief_mode_renders_usage_table() {
    let card = test_card(
        CliProvider::Claude,
        "Claude",
        "web",
        Some("Max".to_string()),
        None,
        Vec::new(),
        vec![test_metric(
            "Session",
            2.0,
            Some("⏳ Resets in 1h 49m".to_string()),
        )],
        Vec::new(),
        None,
    );
    let rows = CLICardsBriefRenderer::make_rows(&[card]);
    let output = CLICardsBriefRenderer::render(&rows, &[], 80, false, false, Timestamp::UNIX_EPOCH);

    assert!(output.contains("ariadusage • AI Usage & Limits"));
    assert!(output.contains("Provider"));
    assert!(output.contains("Claude"));
    assert!(output.contains("web"));
    assert!(output.contains("Max"));
    assert!(output.contains("98%"));
    assert!(output.contains("█"));
    assert!(output.contains("1h 49m"));
    assert!(output.contains("⚠ Warnings:"));
    let table_line = output
        .split('\n')
        .find(|line| line.starts_with('┌'))
        .unwrap_or("");
    let char_count = table_line.chars().count();
    assert!(char_count >= 50);
    assert!(char_count <= 72);
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:188 synthetic quota lanes do not replace real brief usage
#[test]
fn synthetic_quota_lanes_do_not_replace_real_brief_usage() {
    let mut primary = CliRateWindow::new(0.0, Some(300), None, None);
    primary.is_synthetic_placeholder = true;
    let secondary = CliRateWindow::new(20.0, Some(10080), None, None);
    let snapshot = CliUsageSnapshot::new(
        Some(primary),
        Some(secondary),
        None,
        Timestamp::UNIX_EPOCH.to_string(),
        None,
    );
    let card = CLICardsRenderer::make_card(CLICardBuildInput {
        provider: CliProvider::Claude,
        snapshot: &snapshot,
        credits: None,
        source: "oauth",
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
        now: Timestamp::UNIX_EPOCH,
    });
    let rows = CLICardsBriefRenderer::make_rows(std::slice::from_ref(&card));

    assert_eq!(
        card.metrics
            .iter()
            .map(|m| m.label.as_str())
            .collect::<Vec<_>>(),
        vec!["Weekly"]
    );
    assert_eq!(rows.first().and_then(|r| r.used_percent), Some(20.0));
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:221 brief reset summary wraps to terminal width
#[test]
fn brief_reset_summary_wraps_to_terminal_width() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let reset_at = now + Duration::from_secs(3600);
    let card = test_card(
        CliProvider::Codex,
        "Alibaba Token Plan",
        "web",
        Some("International".to_string()),
        None,
        Vec::new(),
        vec![CliCardMetric::new(
            "Monthly budget",
            Some(50.0),
            Some("⏳ Resets July 30 at 11:59 PM".to_string()),
            Some(reset_at),
            None,
        )],
        Vec::new(),
        None,
    );

    let output = CLICardsBriefRenderer::render(
        &CLICardsBriefRenderer::make_rows(&[card]),
        &[],
        40,
        false,
        false,
        now,
    );
    let lines: Vec<&str> = output.split('\n').collect();

    assert!(output.contains("Next reset: Alibaba Token Plan"));
    assert!(lines.iter().all(|line| line.chars().count() <= 40));
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:287 card metrics honor reset display style
#[test]
fn card_metrics_honor_reset_display_style() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let reset_at = now + Duration::from_secs(3600);
    let snapshot = CliUsageSnapshot::new(
        Some(CliRateWindow::new(
            25.0,
            Some(300),
            Some(reset_at.to_string()),
            None,
        )),
        None,
        None,
        now.to_string(),
        None,
    );
    let countdown = CLICardsRenderer::make_card(CLICardBuildInput {
        provider: CliProvider::Codex,
        snapshot: &snapshot,
        credits: None,
        source: "oauth",
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
    let absolute = CLICardsRenderer::make_card(CLICardBuildInput {
        provider: CliProvider::Codex,
        snapshot: &snapshot,
        credits: None,
        source: "oauth",
        status: None,
        account_override: None,
        fallback_account_email: None,
        is_active: false,
        uses_last_known_usage: false,
        account_problem: None,
        notes: &[],
        use_color: false,
        reset_style: ResetTimeDisplayStyle::Absolute,
        weekly_work_days: None,
        now,
    });

    assert_ne!(
        countdown
            .metrics
            .first()
            .and_then(|m| m.reset_text.as_deref()),
        absolute
            .metrics
            .first()
            .and_then(|m| m.reset_text.as_deref())
    );
    assert!(
        countdown
            .metrics
            .first()
            .and_then(|m| m.reset_text.as_deref())
            .is_some_and(|t| t.contains("in 1h"))
    );
    assert_eq!(
        absolute.metrics.first().and_then(|m| m.reset_at),
        Some(reset_at)
    );
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:327 long detail rows stay within card width
#[test]
fn long_detail_rows_stay_within_card_width() {
    let card = test_card(
        CliProvider::Codex,
        "ClawRouter",
        "api",
        None,
        None,
        vec![format!("Workspace: {}", "long-name-".repeat(12))],
        Vec::new(),
        Vec::new(),
        None,
    );

    let lines = CLICardsRenderer::render_card(&card, 38, true, true);
    assert!(
        lines
            .iter()
            .all(|line| strip_ansi_codes(line).chars().count() == 38)
    );
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:344 brief warnings name the actual quota metric
#[test]
fn brief_warnings_name_the_actual_quota_metric() {
    let card = test_card(
        CliProvider::Codex,
        "OpenRouter",
        "api",
        None,
        None,
        Vec::new(),
        vec![test_metric("Spend", 10.0, None)],
        Vec::new(),
        None,
    );

    let output = CLICardsBriefRenderer::render(
        &CLICardsBriefRenderer::make_rows(&[card]),
        &[],
        80,
        false,
        false,
        Timestamp::UNIX_EPOCH,
    );

    assert!(output.contains("OpenRouter Spend: 90% used"));
    assert!(!output.contains("session limit"));
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:368 brief rows preserve account identity
#[test]
fn brief_rows_preserve_account_identity() {
    let cards = vec![
        test_card(
            CliProvider::Codex,
            "Codex",
            "oauth",
            Some("Pro".to_string()),
            Some("one@x.dev".to_string()),
            Vec::new(),
            vec![test_metric("Session", 80.0, None)],
            Vec::new(),
            None,
        ),
        test_card(
            CliProvider::Codex,
            "Codex",
            "oauth",
            Some("Pro".to_string()),
            Some("two@x.dev".to_string()),
            Vec::new(),
            vec![test_metric("Session", 60.0, None)],
            Vec::new(),
            None,
        ),
    ];

    let output = CLICardsBriefRenderer::render(
        &CLICardsBriefRenderer::make_rows(&cards),
        &[],
        80,
        false,
        false,
        Timestamp::UNIX_EPOCH,
    );

    assert!(output.contains("one@x.dev"));
    assert!(output.contains("two@x.dev"));
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:404 brief warnings wrap to terminal width
#[test]
fn brief_warnings_wrap_to_terminal_width() {
    let cards: Vec<CLICardModel> = ["OpenRouter", "Antigravity", "CommandCode"]
        .into_iter()
        .map(|title| {
            test_card(
                CliProvider::Codex,
                title,
                "api",
                None,
                None,
                Vec::new(),
                vec![test_metric("Monthly budget", 5.0, None)],
                Vec::new(),
                None,
            )
        })
        .collect();

    let output = CLICardsBriefRenderer::render(
        &CLICardsBriefRenderer::make_rows(&cards),
        &[],
        40,
        false,
        false,
        Timestamp::UNIX_EPOCH,
    );
    let warning_lines: Vec<&str> = output
        .split('\n')
        .filter(|line| line.contains("Warnings:") || line.contains("% used"))
        .collect();

    assert!(warning_lines.len() > 1);
    assert!(warning_lines.iter().all(|line| line.chars().count() <= 40));
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:433 brief summary ignores unparseable reset labels and fits narrow terminals
#[test]
fn brief_summary_ignores_unparsable_reset_labels_and_fits_narrow_terminals() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let rows = CLICardsBriefRenderer::make_rows(&[
        test_card(
            CliProvider::Codex,
            "Kilo",
            "api",
            None,
            None,
            Vec::new(),
            vec![test_metric(
                "Credits",
                75.0,
                Some("Reset Unlimited".to_string()),
            )],
            Vec::new(),
            None,
        ),
        test_card(
            CliProvider::Codex,
            "Codex",
            "oauth",
            Some("Pro".to_string()),
            None,
            Vec::new(),
            vec![CliCardMetric::new(
                "Session",
                Some(50.0),
                Some("⏳ Resets in 5h".to_string()),
                Some(now + Duration::from_secs(5 * 3600)),
                None,
            )],
            Vec::new(),
            None,
        ),
    ]);

    let output = CLICardsBriefRenderer::render(&rows, &[], 40, false, false, now);
    let lines: Vec<&str> = output.split('\n').collect();

    assert!(output.contains("Next reset: Codex in 5h"));
    assert!(!output.contains("Next reset: Kilo"));
    assert!(lines.iter().all(|line| line.chars().count() <= 40));
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:476 enhanced brief mode fills bars from used percentage
#[test]
fn enhanced_brief_mode_fills_bars_from_used_percentage() {
    let rows = CLICardsBriefRenderer::make_rows(&[
        test_card(
            CliProvider::Codex,
            "Unused",
            "oauth",
            None,
            None,
            Vec::new(),
            vec![test_metric("Session", 100.0, None)],
            Vec::new(),
            None,
        ),
        test_card(
            CliProvider::Codex,
            "Exhausted",
            "api",
            None,
            None,
            Vec::new(),
            vec![test_metric("Session", 0.0, None)],
            Vec::new(),
            None,
        ),
    ]);
    let output = CLICardsBriefRenderer::render(&rows, &[], 80, true, true, Timestamp::UNIX_EPOCH);
    let stripped = strip_ansi_codes(&output);
    let plain_lines: Vec<&str> = stripped.split('\n').collect();
    let unused_line = plain_lines
        .iter()
        .copied()
        .find(|line| line.contains("Unused"))
        .unwrap_or("");
    let exhausted_line = plain_lines
        .iter()
        .copied()
        .find(|line| line.contains("Exhausted"))
        .unwrap_or("");

    assert!(unused_line.contains("0%"));
    assert_eq!(unused_line.chars().filter(|&c| c == '█').count(), 0);
    assert!(exhausted_line.contains("100%"));
    assert_eq!(exhausted_line.chars().filter(|&c| c == '░').count(), 0);
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:517 standard brief mode fills bars from used percentage
#[test]
fn standard_brief_mode_fills_bars_from_used_percentage() {
    let rows = CLICardsBriefRenderer::make_rows(&[
        test_card(
            CliProvider::Codex,
            "Unused",
            "oauth",
            None,
            None,
            Vec::new(),
            vec![test_metric("Session", 100.0, None)],
            Vec::new(),
            None,
        ),
        test_card(
            CliProvider::Codex,
            "Exhausted",
            "api",
            None,
            None,
            Vec::new(),
            vec![test_metric("Session", 0.0, None)],
            Vec::new(),
            None,
        ),
    ]);
    let output = CLICardsBriefRenderer::render(&rows, &[], 80, false, false, Timestamp::UNIX_EPOCH);
    let plain_lines: Vec<&str> = output.split('\n').collect();
    let unused_line = plain_lines
        .iter()
        .copied()
        .find(|line| line.contains("Unused"))
        .unwrap_or("");
    let exhausted_line = plain_lines
        .iter()
        .copied()
        .find(|line| line.contains("Exhausted"))
        .unwrap_or("");

    assert!(unused_line.contains("0%"));
    assert_eq!(unused_line.chars().filter(|&c| c == '█').count(), 0);
    assert!(exhausted_line.contains("100%"));
    assert_eq!(exhausted_line.chars().filter(|&c| c == '░').count(), 0);
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:558 standard card grid shows empty remaining bar at exhaustion
#[test]
fn standard_card_grid_shows_empty_remaining_bar_at_exhaustion() {
    let card = test_card(
        CliProvider::Codex,
        "Exhausted",
        "api",
        None,
        None,
        Vec::new(),
        vec![test_metric("Session", 0.0, None)],
        Vec::new(),
        None,
    );
    let lines = CLICardsRenderer::render_card(&card, 48, false, false);
    let bar_line = lines
        .iter()
        .find(|line| line.contains("[ ") && line.contains(']'))
        .map_or("", String::as_str);
    assert_eq!(bar_line.chars().filter(|&c| c == '━').count(), 0);
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:575 enhanced card grid shows empty remaining bar at exhaustion
#[test]
fn enhanced_card_grid_shows_empty_remaining_bar_at_exhaustion() {
    let card = test_card(
        CliProvider::Codex,
        "Exhausted",
        "api",
        None,
        None,
        Vec::new(),
        vec![test_metric("Session", 0.0, None)],
        Vec::new(),
        None,
    );
    let lines = CLICardsRenderer::render_card(&card, 48, true, true);
    let plain_bar_line = strip_ansi_codes(
        lines
            .iter()
            .find(|line| line.contains("[ ") && line.contains(']'))
            .map_or("", String::as_str),
    );
    assert_eq!(
        plain_bar_line
            .chars()
            .filter(|&c| !c.is_whitespace() && c != '│' && c != '[' && c != ']')
            .count(),
        0
    );
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:593 enhanced mode uses truecolor gradient bars
#[test]
fn enhanced_mode_uses_truecolor_gradient_bars() {
    let card = test_card(
        CliProvider::Codex,
        "Codex",
        "oauth",
        Some("Pro".to_string()),
        None,
        Vec::new(),
        vec![test_metric("Session", 50.0, None)],
        Vec::new(),
        None,
    );
    let output = CLICardsRenderer::render(&[card], &[], 80, true, true);
    assert!(output.contains("38;2;"));
    assert!(output.contains("48;2;"));
    assert!(output.contains("[ "));
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:616 claude swap active account renders without inferred plan
#[test]
fn claude_swap_active_account_renders_without_inferred_plan() {
    let snapshot = CliUsageSnapshot::new(
        Some(CliRateWindow::new(25.0, Some(300), None, None)),
        None,
        None,
        Timestamp::UNIX_EPOCH.to_string(),
        Some(CliProviderIdentitySnapshot::new(
            Some("claude".to_string()),
            Some("active@example.com".to_string()),
            None,
            Some("claude-swap".to_string()),
        )),
    );
    let account = ProviderAccountUsageSnapshot::new(
        ProviderAccountIdentity::new("claude-swap", "2"),
        CliProvider::Claude,
        "active@example.com",
        Some("active@example.com".to_string()),
        None,
        true,
        false,
        false,
        Some(snapshot),
        None,
        "claude-swap",
    );
    let render_options = CLIClaudeSwapCardsRenderOptions {
        status: None,
        use_color: false,
        reset_style: ResetTimeDisplayStyle::Countdown,
        weekly_work_days: None,
        now: Timestamp::UNIX_EPOCH,
    };
    let card = CLICardsRenderer::make_claude_swap_card(&account, &render_options);
    let full = CLICardsRenderer::render_card(&card, 38, false, false).join("\n");
    let brief = CLICardsBriefRenderer::render(
        &CLICardsBriefRenderer::make_rows(std::slice::from_ref(&card)),
        &[],
        40,
        false,
        false,
        Timestamp::UNIX_EPOCH,
    );

    assert_eq!(card.plan_badge, None);
    assert!(full.contains("@ active@example.com [active]"));
    assert!(!full.contains("PLAN Claude-Swap"));
    assert!(brief.contains("[active]"));
    assert!(!brief.contains("Claude-Swap"));
    assert!(full.split('\n').all(|line| line.chars().count() == 38));
    assert!(brief.split('\n').all(|line| line.chars().count() <= 40));
}

// upstream: Tests/CodexBarTests/CLICardsRendererTests.swift:661 claude swap sentinel text survives full and brief projections
#[test]
fn claude_swap_sentinel_text_survives_full_and_brief_projections() {
    let account = ProviderAccountUsageSnapshot::new(
        ProviderAccountIdentity::new("claude-swap", "7"),
        CliProvider::Claude,
        format!("bad\u{1B}[31m\r\n{}", "x".repeat(300)),
        None,
        None,
        true,
        false,
        false,
        None,
        Some("API-key account; subscription usage is unavailable.".to_string()),
        "claude-swap",
    );
    let render_options = CLIClaudeSwapCardsRenderOptions {
        status: None,
        use_color: false,
        reset_style: ResetTimeDisplayStyle::Countdown,
        weekly_work_days: None,
        now: Timestamp::UNIX_EPOCH,
    };
    let card = CLICardsRenderer::make_claude_swap_card(&account, &render_options);
    let full = CLICardsRenderer::render_card(&card, 42, false, false).join("\n");
    let brief = CLICardsBriefRenderer::render(
        &CLICardsBriefRenderer::make_rows(std::slice::from_ref(&card)),
        &[],
        80,
        false,
        false,
        Timestamp::UNIX_EPOCH,
    );
    let brief_row = brief
        .split('\n')
        .find(|line| line.contains("API-key"))
        .unwrap_or("");

    assert_eq!(
        card.account_line.as_ref().map(|s| s.chars().count()),
        Some(CLIClaudeSwapText::LABEL_SCALAR_LIMIT)
    );
    assert!(
        card.account_line
            .as_deref()
            .is_some_and(|s| !s.contains('\u{1B}'))
    );
    assert!(
        card.account_line
            .as_deref()
            .is_some_and(|s| !s.contains('\n'))
    );
    assert!(card.is_active);
    assert!(full.contains("[active]"));
    assert!(full.contains("API-key account;"));
    assert!(full.contains("subscription usage"));
    assert!(full.contains("unavailable."));
    assert!(brief.contains("Claude [active]"));
    assert!(brief.contains("API-key account"));
    assert!(brief_row.ends_with(" — │"));
    assert!(card.metrics.is_empty());
}

// ============================================================================
// CLICardsClaudeSwapTests.swift
// ============================================================================

type NoopClaudeSwapReader =
    fn(String) -> std::future::Ready<Result<ClaudeSwapAccountList, ClaudeSwapAccountReaderError>>;

fn ambient_output(failed: bool) -> UsageCommandOutput {
    let mut output = UsageCommandOutput {
        cards: vec![test_card(
            CliProvider::Claude,
            "Ambient Claude",
            "oauth",
            Some("Max".to_string()),
            Some("ambient@example.com".to_string()),
            Vec::new(),
            vec![test_metric("Session", 50.0, None)],
            Vec::new(),
            None,
        )],
        ..UsageCommandOutput::default()
    };
    if failed {
        output.card_failures = vec![CLICardFailure::new(
            CliProvider::Claude,
            None,
            "ambient failed",
        )];
        output.exit_code = ExitCode::Failure;
    }
    output
}

fn cswap_render_options(status: Option<ProviderStatusPayload>) -> CLIClaudeSwapCardsRenderOptions {
    CLIClaudeSwapCardsRenderOptions {
        status,
        use_color: false,
        reset_style: ResetTimeDisplayStyle::Countdown,
        weekly_work_days: None,
        now: Timestamp::from_second(1_700_000_000).unwrap(),
    }
}

fn cswap_row(
    number: i32,
    active: bool,
    status: ClaudeSwapUsageStatus,
    email: Option<&str>,
    has_usage: bool,
) -> ClaudeSwapAccountRow {
    ClaudeSwapAccountRow::new(
        number,
        email.map_or_else(|| format!("account-{number}@example.com"), str::to_string),
        active,
        status,
        has_usage.then(|| ClaudeSwapUsageWindow::new(f64::from(number * 10), None)),
        None,
    )
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:66 configured executable path strips surrounding quotes
#[test]
fn configured_executable_path_strips_surrounding_quotes() {
    for raw_path in ["  \"/tmp/cswap\"  ", "  '/tmp/cswap'  "] {
        let mut config = ProviderConfig::new(ProviderId::new("claude").unwrap());
        config.extra.insert(
            "claudeSwapExecutablePath".to_string(),
            serde_json::value::RawValue::from_string(serde_json::to_string(raw_path).unwrap())
                .unwrap(),
        );
        assert_eq!(
            CLIClaudeSwapCards::executable_path(Some(&config)),
            "/tmp/cswap"
        );
    }
    assert!(CLIClaudeSwapCards::executable_path(None).is_empty());
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:76 single account config is backward compatible and round trips opt in
#[test]
fn single_account_config_is_backward_compatible_and_round_trips_opt_in() {
    let legacy_data = decode(br#"{"version":1,"providers":[{"id":"claude"}]}"#)
        .unwrap()
        .unwrap();
    let legacy = legacy_data.providers[0].as_typed().unwrap();
    assert!(!CLIClaudeSwapCards::show_single_account(Some(legacy)));

    let mut enabled_data = decode(br#"{"version":1,"providers":[{"id":"claude"}]}"#)
        .unwrap()
        .unwrap();
    enabled_data.providers[0]
        .as_typed_mut()
        .unwrap()
        .extra
        .insert(
            "claudeSwapShowSingleAccount".to_string(),
            serde_json::value::RawValue::from_string("true".to_string()).unwrap(),
        );
    let encoded = encode(&enabled_data);
    let decoded_data = decode(&encoded).unwrap().unwrap();
    let decoded = decoded_data.providers[0].as_typed().unwrap();
    assert!(CLIClaudeSwapCards::show_single_account(Some(decoded)));
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:89 eligibility preserves explicit account and source intent
#[test]
fn eligibility_preserves_explicit_account_and_source_intent() {
    for source_mode in [None, Some("auto")] {
        assert!(CLIClaudeSwapCards::is_eligible(
            CliProvider::Claude,
            true,
            false,
            source_mode,
        ));
    }

    for source_mode in ["web", "cli", "oauth", "api"] {
        assert!(!CLIClaudeSwapCards::is_eligible(
            CliProvider::Claude,
            true,
            false,
            Some(source_mode),
        ));
    }

    assert!(!CLIClaudeSwapCards::is_eligible(
        CliProvider::Claude,
        false,
        false,
        None,
    ));
    assert!(!CLIClaudeSwapCards::is_eligible(
        CliProvider::Claude,
        true,
        true,
        None,
    ));
    assert!(!CLIClaudeSwapCards::is_eligible(
        CliProvider::Codex,
        true,
        false,
        None,
    ));
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:125 bypass does not invoke the adapter when single account cards are enabled
#[tokio::test]
async fn bypass_does_not_invoke_the_adapter_when_single_account_cards_enabled() {
    let counter = Arc::new(AtomicUsize::new(0));
    let ambient = ambient_output(false);
    let ambient_clone = ambient.clone();
    let counter_clone = Arc::clone(&counter);
    let output = CLIClaudeSwapCards::fetch(
        false,
        "/unused/cswap",
        ClaudeSwapAccountSource::ClaudeSwap,
        true,
        &cswap_render_options(None),
        move || async move { ambient_clone },
        Some(move |_| async move {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            Ok(ClaudeSwapAccountList::new(None, Vec::new()))
        }),
        &[],
    )
    .await;

    assert_eq!(counter.load(Ordering::SeqCst), 0);
    assert_eq!(output.cards, ambient.cards);
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:144 zero and one account lists retain ambient output
#[tokio::test]
async fn zero_and_one_account_lists_retain_ambient_output() {
    let ambient_counter = Arc::new(AtomicUsize::new(0));
    let ambient = ambient_output(false);
    for accounts in [
        Vec::new(),
        vec![cswap_row(1, false, ClaudeSwapUsageStatus::Ok, None, true)],
    ] {
        let ambient_clone = ambient.clone();
        let counter_clone = Arc::clone(&ambient_counter);
        let output = CLIClaudeSwapCards::fetch(
            true,
            "/fake/cswap",
            ClaudeSwapAccountSource::ClaudeSwap,
            false,
            &cswap_render_options(None),
            move || async move {
                counter_clone.fetch_add(1, Ordering::SeqCst);
                ambient_clone
            },
            Some(move |_| async move { Ok(ClaudeSwapAccountList::new(None, accounts)) }),
            &[],
        )
        .await;
        assert_eq!(output.cards, ambient.cards);
        assert!(output.card_failures.is_empty());
    }
    assert_eq!(ambient_counter.load(Ordering::SeqCst), 2);
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:166 single account option renders sentinel account instead of ambient output
#[tokio::test]
async fn single_account_option_renders_sentinel_account_instead_of_ambient_output() {
    let ambient_counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = Arc::clone(&ambient_counter);
    let output = CLIClaudeSwapCards::fetch(
        true,
        "/fake/cswap",
        ClaudeSwapAccountSource::ClaudeSwap,
        true,
        &cswap_render_options(None),
        move || async move {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            ambient_output(true)
        },
        Some(|_| async move {
            Ok(ClaudeSwapAccountList::new(
                Some(1),
                vec![cswap_row(
                    1,
                    true,
                    ClaudeSwapUsageStatus::TokenExpired,
                    Some("single@example.com"),
                    false,
                )],
            ))
        }),
        &[],
    )
    .await;

    assert_eq!(ambient_counter.load(Ordering::SeqCst), 0);
    assert_eq!(output.exit_code, ExitCode::Success);
    assert_eq!(output.cards.len(), 1);
    assert_eq!(
        output.cards.first().and_then(|c| c.account_line.as_deref()),
        Some("single@example.com")
    );
    assert_eq!(output.cards.first().map(|c| c.is_active), Some(true));
    assert_eq!(
        output
            .cards
            .first()
            .and_then(|c| c.account_problem.as_deref()),
        Some("Token expired. Switch to this account in claude-swap to refresh it.")
    );
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:198 multi account list skips ambient output and renders in active slot order
#[tokio::test]
async fn multi_account_list_skips_ambient_output_and_renders_in_active_slot_order() {
    let adapter_counter = Arc::new(AtomicUsize::new(0));
    let ambient_counter = Arc::new(AtomicUsize::new(0));
    let list = ClaudeSwapAccountList::new(
        Some(2),
        vec![
            cswap_row(3, false, ClaudeSwapUsageStatus::Ok, None, true),
            cswap_row(2, true, ClaudeSwapUsageStatus::Ok, None, true),
            cswap_row(1, false, ClaudeSwapUsageStatus::Ok, None, true),
        ],
    );
    let status = ProviderStatusPayload {
        indicator: StatusIndicator::Minor,
        description: Some("Degraded performance".to_string()),
        updated_at: Some(Timestamp::UNIX_EPOCH.to_string()),
        url: "https://status.example.com".to_string(),
    };

    let amb_c = Arc::clone(&ambient_counter);
    let adp_c = Arc::clone(&adapter_counter);
    let output = CLIClaudeSwapCards::fetch(
        true,
        "/fake/cswap",
        ClaudeSwapAccountSource::ClaudeSwap,
        false,
        &cswap_render_options(Some(status)),
        move || async move {
            amb_c.fetch_add(1, Ordering::SeqCst);
            ambient_output(true)
        },
        Some(move |_| async move {
            adp_c.fetch_add(1, Ordering::SeqCst);
            Ok(list)
        }),
        &[],
    )
    .await;

    assert_eq!(adapter_counter.load(Ordering::SeqCst), 1);
    assert_eq!(ambient_counter.load(Ordering::SeqCst), 0);
    assert_eq!(
        output
            .cards
            .iter()
            .map(|c| c.account_line.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("account-2@example.com"),
            Some("account-1@example.com"),
            Some("account-3@example.com"),
        ]
    );
    assert_eq!(
        output.cards.iter().map(|c| c.is_active).collect::<Vec<_>>(),
        vec![true, false, false]
    );
    assert!(
        output
            .cards
            .iter()
            .all(|c| c.source_label == "claude-swap" && c.plan_badge.is_none())
    );
    assert!(output.cards.iter().all(|c| {
        c.status_line.as_deref() == Some("Status: Partial outage – Degraded performance")
    }));
    assert!(output.card_failures.is_empty());
    assert_eq!(output.exit_code, ExitCode::Success);
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:241 all sentinel rows remain successful metrics less cards
#[tokio::test]
async fn all_sentinel_rows_remain_successful_metrics_less_cards() {
    let statuses = [
        ClaudeSwapUsageStatus::ApiKey,
        ClaudeSwapUsageStatus::TokenExpired,
        ClaudeSwapUsageStatus::ReloginRequired,
        ClaudeSwapUsageStatus::KeychainUnavailable,
        ClaudeSwapUsageStatus::NoCredentials,
        ClaudeSwapUsageStatus::Unavailable,
        ClaudeSwapUsageStatus::Unknown("future_status".to_string()),
        ClaudeSwapUsageStatus::Ok,
    ];
    let rows: Vec<ClaudeSwapAccountRow> = statuses
        .iter()
        .enumerate()
        .map(|(idx, st)| cswap_row((idx + 1) as i32, false, st.clone(), None, false))
        .collect();
    let output = CLIClaudeSwapCards::fetch(
        true,
        "/fake/cswap",
        ClaudeSwapAccountSource::ClaudeSwap,
        false,
        &cswap_render_options(None),
        || async { ambient_output(true) },
        Some(move |_| async move { Ok(ClaudeSwapAccountList::new(None, rows)) }),
        &[],
    )
    .await;

    assert_eq!(output.exit_code, ExitCode::Success);
    assert_eq!(output.cards.len(), statuses.len());
    assert!(
        output
            .cards
            .iter()
            .all(|c| c.metrics.is_empty() && !c.is_active)
    );
    assert_eq!(
        output
            .cards
            .iter()
            .map(|c| c.account_problem.as_deref())
            .collect::<Vec<_>>(),
        vec![
            Some("API-key account; subscription usage is unavailable."),
            Some("Token expired. Switch to this account in claude-swap to refresh it."),
            Some("Re-login required. Re-authenticate this account in claude-swap."),
            Some("claude-swap could not read the active account's Keychain entry."),
            Some("No stored credentials for this account slot."),
            Some("Usage unavailable."),
            Some("Unrecognized claude-swap status: future_status"),
            Some("No usage windows reported."),
        ]
    );
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:280 active sentinel account remains active and metrics less in full and brief cards
#[tokio::test]
async fn active_sentinel_account_remains_active_and_metrics_less_in_full_and_brief_cards() {
    let problem = "Usage unavailable.";
    let output = CLIClaudeSwapCards::fetch(
        true,
        "/fake/cswap",
        ClaudeSwapAccountSource::ClaudeSwap,
        false,
        &cswap_render_options(None),
        || async { ambient_output(true) },
        Some(|_| async move {
            Ok(ClaudeSwapAccountList::new(
                Some(1),
                vec![
                    cswap_row(
                        1,
                        true,
                        ClaudeSwapUsageStatus::Unavailable,
                        Some("active@example.com"),
                        false,
                    ),
                    cswap_row(2, false, ClaudeSwapUsageStatus::Ok, None, true),
                ],
            ))
        }),
        &[],
    )
    .await;

    assert_eq!(output.exit_code, ExitCode::Success);
    assert!(output.card_failures.is_empty());
    assert_eq!(output.cards.len(), 2);
    let active_card = output.cards.first().unwrap();
    assert_eq!(
        active_card.account_line.as_deref(),
        Some("active@example.com")
    );
    assert!(active_card.is_active);
    assert_eq!(active_card.account_problem.as_deref(), Some(problem));
    assert!(active_card.metrics.is_empty());

    let rows = CLICardsBriefRenderer::make_rows(std::slice::from_ref(active_card));
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows.first().and_then(|r| r.account_label.as_deref()),
        Some("active@example.com")
    );
    assert_eq!(rows.first().map(|r| r.is_active), Some(true));
    assert_eq!(
        rows.first().and_then(|r| r.account_problem.as_deref()),
        Some(problem)
    );
    assert_eq!(rows.first().and_then(|r| r.metric_label.as_deref()), None);
    assert_eq!(rows.first().and_then(|r| r.used_percent), None);
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:318 unavailable at limit windows keep metrics and name the exhausted window
#[tokio::test]
async fn unavailable_at_limit_windows_keep_metrics_and_name_the_exhausted_window() {
    let reset = Timestamp::from_second(1_700_003_600).unwrap();
    let output = CLIClaudeSwapCards::fetch(
        true,
        "/fake/cswap",
        ClaudeSwapAccountSource::ClaudeSwap,
        false,
        &cswap_render_options(None),
        || async { ambient_output(true) },
        Some(move |_| async move {
            Ok(ClaudeSwapAccountList::new(
                Some(1),
                vec![
                    ClaudeSwapAccountRow::new(
                        1,
                        "limited@example.com",
                        true,
                        ClaudeSwapUsageStatus::Unavailable,
                        Some(ClaudeSwapUsageWindow::new(100.0, Some(reset))),
                        Some(ClaudeSwapUsageWindow::new(100.0, Some(reset))),
                    ),
                    cswap_row(2, false, ClaudeSwapUsageStatus::Ok, None, true),
                ],
            ))
        }),
        &[],
    )
    .await;

    assert_eq!(output.exit_code, ExitCode::Success);
    let active_card = output.cards.first().unwrap();
    assert_eq!(
        active_card.account_line.as_deref(),
        Some("limited@example.com")
    );
    assert!(active_card.is_active);
    assert_eq!(
        active_card.account_problem.as_deref(),
        Some("Session limit reached. Resets in 1h. Weekly limit reached. Resets in 1h.")
    );
    assert!(!active_card.metrics.is_empty());
    assert!(
        active_card
            .metrics
            .iter()
            .any(|m| m.remaining_percent == Some(0.0))
    );
    assert!(
        !active_card
            .account_problem
            .as_deref()
            .is_some_and(|p| p.contains("Usage fetch failed"))
    );

    let rows = CLICardsBriefRenderer::make_rows(std::slice::from_ref(active_card));
    assert!(
        rows.first()
            .and_then(|r| r.account_problem.as_deref())
            .is_some_and(|p| p.contains("Session limit reached"))
    );
    assert_eq!(rows.first().and_then(|r| r.used_percent), Some(100.0));
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:354 unavailable null usage retains previous CLI windows
#[tokio::test]
async fn unavailable_null_usage_retains_previous_cli_windows() {
    let reset = Timestamp::from_second(1_700_003_600).unwrap();
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let previous = ClaudeSwapAccountProjection::account_snapshots(
        &ClaudeSwapAccountList::new(
            Some(1),
            vec![ClaudeSwapAccountRow::new(
                1,
                "limited@example.com",
                true,
                ClaudeSwapUsageStatus::Ok,
                Some(ClaudeSwapUsageWindow::new(100.0, Some(reset))),
                Some(ClaudeSwapUsageWindow::new(40.0, Some(reset))),
            )],
        ),
        ClaudeSwapAccountSource::ClaudeSwap,
        &[],
        now,
    );
    let output = CLIClaudeSwapCards::fetch(
        true,
        "/fake/cswap",
        ClaudeSwapAccountSource::ClaudeSwap,
        false,
        &cswap_render_options(None),
        || async { ambient_output(true) },
        Some(|_| async move {
            Ok(ClaudeSwapAccountList::new(
                Some(1),
                vec![
                    cswap_row(
                        1,
                        true,
                        ClaudeSwapUsageStatus::Unavailable,
                        Some("limited@example.com"),
                        false,
                    ),
                    cswap_row(2, false, ClaudeSwapUsageStatus::Ok, None, true),
                ],
            ))
        }),
        &previous,
    )
    .await;

    assert_eq!(output.exit_code, ExitCode::Success);
    let active_card = output.cards.first().unwrap();
    assert_eq!(
        active_card.account_line.as_deref(),
        Some("limited@example.com")
    );
    assert!(!active_card.metrics.is_empty());
    assert!(
        active_card
            .metrics
            .iter()
            .any(|m| m.remaining_percent == Some(0.0))
    );
    assert!(
        active_card
            .account_problem
            .as_deref()
            .is_some_and(|p| p.contains("Session limit reached"))
    );
    assert!(
        !active_card
            .account_problem
            .as_deref()
            .is_some_and(|p| p.contains("Usage fetch failed"))
    );
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:395 blank executable path preserves ambient output and fails distinctly
#[tokio::test]
async fn blank_executable_path_preserves_ambient_output_and_fails_distinctly() {
    let ambient = ambient_output(false);
    let ambient_clone = ambient.clone();
    let none_reader: Option<NoopClaudeSwapReader> = None;
    let output = CLIClaudeSwapCards::fetch(
        true,
        "   ",
        ClaudeSwapAccountSource::ClaudeSwap,
        false,
        &cswap_render_options(None),
        move || async move { ambient_clone },
        none_reader,
        &[],
    )
    .await;

    assert_eq!(output.cards, ambient.cards);
    assert_ne!(output.exit_code, ExitCode::Success);
    assert_eq!(
        output.card_failures,
        vec![CLICardFailure::new(
            CliProvider::Claude,
            Some("claude-swap".to_string()),
            "No claude-swap executable path is configured.",
        )]
    );
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:412 adapter failures follow ambient failures and are bounded and sanitized
#[tokio::test]
async fn adapter_failures_follow_ambient_failures_and_are_bounded_and_sanitized() {
    let raw = format!(
        "\u{1B}]0;owned\u{07}reader\r\nfailed\u{1B}[31m{}",
        "x".repeat(700)
    );
    let output = CLIClaudeSwapCards::fetch(
        true,
        "   ",
        ClaudeSwapAccountSource::ClaudeSwap,
        false,
        &cswap_render_options(None),
        || async { ambient_output(true) },
        Some(move |_| async move { Err(ClaudeSwapAccountReaderError::SubprocessFailed(raw)) }),
        &[],
    )
    .await;

    assert_ne!(output.exit_code, ExitCode::Success);
    assert_eq!(
        output.cards.first().map(|c| c.title.as_str()),
        Some("Ambient Claude")
    );
    assert_eq!(
        output
            .card_failures
            .iter()
            .map(|f| f.account_label.as_deref())
            .collect::<Vec<_>>(),
        vec![None, Some("claude-swap")]
    );
    let diagnostic = output
        .card_failures
        .last()
        .map_or("", |f| f.message.as_str());
    assert!(diagnostic.contains("reader  failed"));
    assert!(!diagnostic.contains('\u{1B}'));
    assert_eq!(
        diagnostic.chars().count(),
        CLIClaudeSwapText::DIAGNOSTIC_SCALAR_LIMIT
    );
}

fn install_fake_executable(script: &str, path: &Path) {
    fs::write(path, script).unwrap();
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:431 fake executable receives only one read only list command
#[tokio::test]
async fn fake_executable_receives_only_one_read_only_list_command() {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("cswap");
    let invocation_marker = dir.path().join("invoked");
    let duplicate_marker = dir.path().join("duplicate");
    let script = format!(
        "#!/bin/sh\nmkdir '{}' || {{\n  touch '{}'\n  exit 2\n}}\n[ \"$#\" -eq 2 ] || exit 2\n[ \"$1\" = \"--list\" ] || exit 2\n[ \"$2\" = \"--json\" ] || exit 2\ncat <<'JSON'\n{{\"schemaVersion\":1,\"activeAccountNumber\":2,\"accounts\":[\n  {{\"number\":1,\"email\":\"one@example.com\",\"active\":false,\"usageStatus\":\"api_key\"}},\n  {{\"number\":2,\"email\":\"two@example.com\",\"active\":true,\"usageStatus\":\"unavailable\"}}\n]}}\nJSON\n",
        invocation_marker.display(),
        duplicate_marker.display()
    );
    install_fake_executable(&script, &executable);

    let none_reader: Option<NoopClaudeSwapReader> = None;
    let output = CLIClaudeSwapCards::fetch(
        true,
        executable.to_str().unwrap(),
        ClaudeSwapAccountSource::ClaudeSwap,
        false,
        &cswap_render_options(None),
        || async { ambient_output(false) },
        none_reader,
        &[],
    )
    .await;

    assert_eq!(output.exit_code, ExitCode::Success);
    assert!(output.card_failures.is_empty());
    assert_eq!(output.cards.len(), 2);
    assert!(invocation_marker.exists());
    assert!(!duplicate_marker.exists());
}

// upstream: Tests/CodexBarTests/CLICardsClaudeSwapTests.swift:470 cancellation drains the adapter child and preserves ambient output
#[tokio::test]
async fn cancellation_drains_the_adapter_child_and_preserves_ambient_output() {
    let cancellation_count = Arc::new(AtomicUsize::new(0));
    let ambient = ambient_output(false);
    let ambient_clone = ambient.clone();
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let cancel_child = cancel_token.clone();
    let count_clone = Arc::clone(&cancellation_count);

    let handle = tokio::spawn(async move {
        CLIClaudeSwapCards::fetch(
            true,
            "/fake/cswap",
            ClaudeSwapAccountSource::ClaudeSwap,
            false,
            &cswap_render_options(None),
            move || async move { ambient_clone },
            Some(move |_| async move {
                tokio::select! {
                    () = tokio::time::sleep(Duration::from_secs(30)) => {
                        Ok(ClaudeSwapAccountList::new(None, Vec::new()))
                    }
                    () = cancel_child.cancelled() => {
                        count_clone.fetch_add(1, Ordering::SeqCst);
                        Err(ClaudeSwapAccountReaderError::Cancelled)
                    }
                }
            }),
            &[],
        )
        .await
    });

    tokio::task::yield_now().await;
    cancel_token.cancel();
    let output = handle.await.unwrap();

    assert_eq!(cancellation_count.load(Ordering::SeqCst), 1);
    assert_eq!(output.cards, ambient.cards);
    assert_eq!(
        output
            .card_failures
            .last()
            .and_then(|f| f.account_label.as_deref()),
        Some("claude-swap")
    );
    assert_ne!(output.exit_code, ExitCode::Success);
}

// ============================================================================
// CLIUsageClaudeSwapTests.swift
// ============================================================================

#[allow(clippy::too_many_arguments)]
async fn fetch_usage_claude_swap(
    arguments: &[&str],
    list: Option<&str>,
    enabled: bool,
    source: ClaudeSwapAccountSource,
    config_source: CliSourceMode,
    json: bool,
    expect_adapter: bool,
) -> UsageCommandOutput {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("cswap");
    let invocation = dir.path().join("invoked");
    let response = list.unwrap_or(
        r#"{"schemaVersion":1,"activeAccountNumber":3,"accounts":[
          {"number":1,"email":"browser@example.com","active":false,"usageStatus":"ok",
           "usage":{"fiveHour":{"pct":0},"sevenDay":{"pct":92}}},
          {"number":3,"email":"active@example.com","active":true,"usageStatus":"ok",
           "usage":{"fiveHour":{"pct":50},"sevenDay":{"pct":41}}}
        ]}"#,
    );
    let script = format!(
        "#!/bin/sh\n[ \"$#\" -eq 2 ] && [ \"$1\" = \"{}\" ] && [ \"$2\" = \"--json\" ] || exit 2\n[ ! -e '{}' ] || exit 2\ntouch '{}'\ncat <<'JSON'\n{response}\nJSON\n",
        source.list_arguments()[0],
        invocation.display(),
        invocation.display(),
    );
    install_fake_executable(&script, &executable);

    let mut argv: Vec<String> = vec![
        "--provider".to_string(),
        "claude".to_string(),
        "--no-color".to_string(),
    ];
    if json {
        argv.push("--json".to_string());
    }
    argv.extend(arguments.iter().map(|s| (*s).to_string()));
    let (_opts, values) = parse_usage_args(&argv).unwrap();
    let output_prefs = CLIOutputPreferences::from_values(&values, true);
    let setup =
        CLIUsageFetchSetup::new(&values, &[CliProvider::Claude], output_prefs, false).unwrap();
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let render_opts = CLIClaudeSwapCardsRenderOptions {
        status: None,
        use_color: setup.command.use_color,
        reset_style: setup.command.reset_style,
        weekly_work_days: None,
        now,
    };

    let output = CLIClaudeSwapCards::fetch_single_provider_usage_output(
        CliProvider::Claude,
        setup.command.provider_runtime == ariadusage_cli::options::ProviderRuntime::App,
        enabled,
        executable.to_str().unwrap(),
        source,
        &setup.token_selection,
        None,
        setup.command.source_mode_override,
        config_source,
        json,
        setup.command.cards_layout,
        &render_opts,
        &[],
        move |suffix| async move {
            let browser_source = format!("web{suffix}");
            let browser_usage = CliUsageSnapshot::new(
                Some(CliRateWindow::new(0.0, Some(300), None, None)),
                Some(CliRateWindow::new(92.0, Some(10080), None, None)),
                None,
                now.to_string(),
                Some(CliProviderIdentitySnapshot::new(
                    Some("claude".to_string()),
                    Some("browser@example.com".to_string()),
                    None,
                    None,
                )),
            );
            let mut out = UsageCommandOutput::default();
            if json {
                out.payload.push(ariadusage_protocol::ProviderPayload {
                    provider: "claude".to_string(),
                    account: Some("browser@example.com".to_string()),
                    version: None,
                    source: browser_source,
                    status: None,
                    usage: Some(browser_usage),
                    rate_window_labels: None,
                    credits: None,
                    reset_credits: None,
                    antigravity_plan_info: None,
                    openai_dashboard: None,
                    diagnostic: None,
                    error: None,
                    pace: None,
                    age_seconds: None,
                });
            } else {
                let ctx = ariadusage_cli::renderer::RenderContext {
                    header: ariadusage_cli::options::format_header(
                        CliProvider::Claude,
                        None,
                        &browser_source,
                    ),
                    status: None,
                    account_override: None,
                    fallback_account_email: None,
                    use_color: false,
                    reset_style: ResetTimeDisplayStyle::Countdown,
                    notes: Vec::new(),
                    weekly_work_days: None,
                };
                out.sections
                    .push(ariadusage_cli::renderer::CLIRenderer::render_text(
                        CliProvider::Claude,
                        &browser_usage,
                        None,
                        &ctx,
                        now,
                    ));
            }
            out
        },
    )
    .await;

    assert_eq!(invocation.exists(), expect_adapter);
    output
}

// upstream: Tests/CodexBarTests/CLIUsageClaudeSwapTests.swift:8 auto usage follows the active swap slot instead of the browser account
#[tokio::test]
async fn auto_usage_follows_the_active_swap_slot_instead_of_the_browser_account() {
    let output = fetch_usage_claude_swap(
        &[],
        None,
        true,
        ClaudeSwapAccountSource::ClaudeSwap,
        CliSourceMode::Auto,
        true,
        true,
    )
    .await;
    let payload = output.payload.first().unwrap();
    assert_eq!(output.payload.len(), 1);
    assert_eq!(output.exit_code, ExitCode::Success);
    assert_eq!(payload.source, "claude-swap");
    assert_eq!(
        payload
            .usage
            .as_ref()
            .and_then(|u| u.identity.as_ref())
            .and_then(|id| id.account_email.as_deref()),
        Some("active@example.com")
    );
    assert_eq!(
        payload
            .usage
            .as_ref()
            .and_then(|u| u.identity.as_ref())
            .and_then(|id| id.account_id.as_deref()),
        Some("claude-swap:3")
    );
    assert_eq!(
        payload
            .usage
            .as_ref()
            .and_then(|u| u.primary.as_ref())
            .map(|w| w.used_percent),
        Some(50.0)
    );
    assert_eq!(
        payload
            .usage
            .as_ref()
            .and_then(|u| u.secondary.as_ref())
            .map(|w| w.used_percent),
        Some(41.0)
    );
}

// upstream: Tests/CodexBarTests/CLIUsageClaudeSwapTests.swift:21 explicit source and token account selection bypass swap
#[tokio::test]
async fn explicit_source_and_token_account_selection_bypass_swap() {
    let explicit = fetch_usage_claude_swap(
        &["--source", "web"],
        None,
        true,
        ClaudeSwapAccountSource::ClaudeSwap,
        CliSourceMode::Auto,
        true,
        false,
    )
    .await;
    assert_eq!(
        explicit.payload.first().map(|p| p.source.as_str()),
        Some("web")
    );
    assert_eq!(
        explicit
            .payload
            .first()
            .and_then(|p| p.usage.as_ref())
            .and_then(|u| u.identity.as_ref())
            .and_then(|id| id.account_email.as_deref()),
        Some("browser@example.com")
    );

    let selected = fetch_usage_claude_swap(
        &["--account", "missing"],
        None,
        true,
        ClaudeSwapAccountSource::ClaudeSwap,
        CliSourceMode::Auto,
        true,
        false,
    )
    .await;
    assert_ne!(selected.exit_code, ExitCode::Success);
    assert!(
        selected
            .payload
            .first()
            .and_then(|p| p.error.as_ref())
            .is_some()
    );
}

// upstream: Tests/CodexBarTests/CLIUsageClaudeSwapTests.swift:31 configured source and disabled integration preserve ordinary fetching
#[tokio::test]
async fn configured_source_and_disabled_integration_preserve_ordinary_fetching() {
    let configured = fetch_usage_claude_swap(
        &[],
        None,
        true,
        ClaudeSwapAccountSource::ClaudeSwap,
        CliSourceMode::Web,
        true,
        false,
    )
    .await;
    assert_eq!(
        configured.payload.first().map(|p| p.source.as_str()),
        Some("web")
    );
    let disabled = fetch_usage_claude_swap(
        &[],
        None,
        false,
        ClaudeSwapAccountSource::ClaudeSwap,
        CliSourceMode::Auto,
        true,
        false,
    )
    .await;
    assert_eq!(
        disabled.payload.first().map(|p| p.source.as_str()),
        Some("web")
    );
}

// upstream: Tests/CodexBarTests/CLIUsageClaudeSwapTests.swift:39 explicit auto still follows a single active slot
#[tokio::test]
async fn explicit_auto_still_follows_a_single_active_slot() {
    let list = r#"{
      "schemaVersion": 1,
      "activeAccountNumber": 3,
      "accounts": [
        {
          "number": 3,
          "email": "single@example.com",
          "active": true,
          "usageStatus": "ok",
          "usage": {
            "fiveHour": {
              "pct": 50
            }
          }
        }
      ]
    }"#;
    let output = fetch_usage_claude_swap(
        &["--source", "auto"],
        Some(list),
        true,
        ClaudeSwapAccountSource::ClaudeSwap,
        CliSourceMode::Web,
        true,
        true,
    )
    .await;
    assert_eq!(output.payload.len(), 1);
    assert_eq!(
        output.payload.first().map(|p| p.source.as_str()),
        Some("claude-swap")
    );
    assert_eq!(
        output
            .payload
            .first()
            .and_then(|p| p.usage.as_ref())
            .and_then(|u| u.identity.as_ref())
            .and_then(|id| id.account_email.as_deref()),
        Some("single@example.com")
    );
}

// upstream: Tests/CodexBarTests/CLIUsageClaudeSwapTests.swift:66 adapter failure labels the fallback source and keeps its own identity
#[tokio::test]
async fn adapter_failure_labels_the_fallback_source_and_keeps_its_own_identity() {
    let output = fetch_usage_claude_swap(
        &[],
        Some("invalid json"),
        true,
        ClaudeSwapAccountSource::ClaudeSwap,
        CliSourceMode::Auto,
        true,
        true,
    )
    .await;
    assert_eq!(output.exit_code, ExitCode::Success);
    assert_eq!(
        output.payload.first().map(|p| p.source.as_str()),
        Some("web (claude-swap fallback)")
    );
    assert_eq!(
        output
            .payload
            .first()
            .and_then(|p| p.usage.as_ref())
            .and_then(|u| u.identity.as_ref())
            .and_then(|id| id.account_email.as_deref()),
        Some("browser@example.com")
    );
    assert_eq!(
        output
            .payload
            .first()
            .and_then(|p| p.usage.as_ref())
            .and_then(|u| u.secondary.as_ref())
            .map(|w| w.used_percent),
        Some(92.0)
    );
}

// upstream: Tests/CodexBarTests/CLIUsageClaudeSwapTests.swift:75 no active slot never borrows an inactive slots usage
#[tokio::test]
async fn no_active_slot_never_borrows_an_inactive_slots_usage() {
    let list = r#"{
      "schemaVersion": 1,
      "activeAccountNumber": null,
      "accounts": [
        {
          "number": 3,
          "email": "inactive@example.com",
          "active": false,
          "usageStatus": "ok",
          "usage": {
            "fiveHour": {
              "pct": 50
            }
          }
        }
      ]
    }"#;
    let output = fetch_usage_claude_swap(
        &[],
        Some(list),
        true,
        ClaudeSwapAccountSource::ClaudeSwap,
        CliSourceMode::Auto,
        true,
        true,
    )
    .await;
    assert_eq!(
        output.payload.first().map(|p| p.source.as_str()),
        Some("web (claude-swap fallback)")
    );
    assert_eq!(
        output
            .payload
            .first()
            .and_then(|p| p.usage.as_ref())
            .and_then(|u| u.identity.as_ref())
            .and_then(|id| id.account_email.as_deref()),
        Some("browser@example.com")
    );
}

// upstream: Tests/CodexBarTests/CLIUsageClaudeSwapTests.swift:101 active slot without usage falls back visibly despite a healthy inactive slot
#[tokio::test]
async fn active_slot_without_usage_falls_back_visibly_despite_a_healthy_inactive_slot() {
    let list = r#"{
      "schemaVersion": 1,
      "activeAccountNumber": 3,
      "accounts": [
        {
          "number": 1,
          "email": "inactive@example.com",
          "active": false,
          "usageStatus": "ok",
          "usage": {
            "fiveHour": {
              "pct": 80
            }
          }
        },
        {
          "number": 3,
          "email": "active@example.com",
          "active": true,
          "usageStatus": "token_expired"
        }
      ]
    }"#;
    let output = fetch_usage_claude_swap(
        &[],
        Some(list),
        true,
        ClaudeSwapAccountSource::ClaudeSwap,
        CliSourceMode::Auto,
        true,
        true,
    )
    .await;
    assert_eq!(
        output.payload.first().map(|p| p.source.as_str()),
        Some("web (claude-swap fallback)")
    );
    assert_eq!(
        output
            .payload
            .first()
            .and_then(|p| p.usage.as_ref())
            .and_then(|u| u.identity.as_ref())
            .and_then(|id| id.account_email.as_deref()),
        Some("browser@example.com")
    );
}

// upstream: Tests/CodexBarTests/CLIUsageClaudeSwapTests.swift:133 last good active slot retains its timestamp and diagnostic
#[tokio::test]
async fn last_good_active_slot_retains_its_timestamp_and_diagnostic() {
    let list = r#"{
      "schemaVersion": 1,
      "activeAccountNumber": 3,
      "accounts": [
        {
          "number": 3,
          "email": "active@example.com",
          "active": true,
          "usageStatus": "token_expired",
          "lastGoodUsage": {
            "fiveHour": {
              "pct": 50
            }
          },
          "lastGoodFetchedAt": "2026-10-18T12:00:00Z"
        }
      ]
    }"#;
    let output = fetch_usage_claude_swap(
        &[],
        Some(list),
        true,
        ClaudeSwapAccountSource::ClaudeSwap,
        CliSourceMode::Auto,
        true,
        true,
    )
    .await;
    assert_eq!(
        output.payload.first().map(|p| p.source.as_str()),
        Some("claude-swap")
    );
    assert_eq!(
        output
            .payload
            .first()
            .and_then(|p| p.usage.as_ref())
            .and_then(|u| u.identity.as_ref())
            .and_then(|id| id.account_email.as_deref()),
        Some("active@example.com")
    );
    assert_eq!(
        output
            .payload
            .first()
            .and_then(|p| p.usage.as_ref())
            .map(|u| u.updated_at.as_str()),
        Some("2026-10-18T12:00:00Z")
    );
    assert!(
        output
            .payload
            .first()
            .and_then(|p| p.diagnostic.as_deref())
            .is_some_and(|d| d.contains("Token expired"))
    );
}

// upstream: Tests/CodexBarTests/CLIUsageClaudeSwapTests.swift:162 text output identifies the active slot and adapter fallback
#[tokio::test]
async fn text_output_identifies_the_active_slot_and_adapter_fallback() {
    let active = fetch_usage_claude_swap(
        &[],
        None,
        true,
        ClaudeSwapAccountSource::ClaudeSwap,
        CliSourceMode::Auto,
        false,
        true,
    )
    .await;
    let active_joined = active.sections.join("");
    assert!(active_joined.contains("claude-swap"));
    assert!(active_joined.contains("active@example.com"));
    assert!(!active_joined.contains("browser@example.com"));

    let fallback = fetch_usage_claude_swap(
        &[],
        Some("invalid json"),
        true,
        ClaudeSwapAccountSource::ClaudeSwap,
        CliSourceMode::Auto,
        false,
        true,
    )
    .await;
    let fallback_joined = fallback.sections.join("");
    assert!(fallback_joined.contains("web (claude-swap fallback)"));
    assert!(fallback_joined.contains("browser@example.com"));
}

// upstream: Tests/CodexBarTests/CLIUsageClaudeSwapTests.swift:173 pooled tcr fallback reports the actual adapter
#[tokio::test]
async fn pooled_tcr_fallback_reports_the_actual_adapter() {
    let output = fetch_usage_claude_swap(
        &[],
        Some(r#"{"schemaVersion":1,"activeAccountNumber":null,"accounts":[]}"#),
        true,
        ClaudeSwapAccountSource::Tcr,
        CliSourceMode::Auto,
        true,
        true,
    )
    .await;
    assert_eq!(
        output.payload.first().map(|p| p.source.as_str()),
        Some("web (tcr fallback)")
    );
    assert_eq!(
        output
            .payload
            .first()
            .and_then(|p| p.usage.as_ref())
            .and_then(|u| u.identity.as_ref())
            .and_then(|id| id.account_email.as_deref()),
        Some("browser@example.com")
    );
}

// ============================================================================
// IPC v1 End-to-End Integration Tests (usage, cards, status)
// ============================================================================

fn sample_engine_snapshot() -> EngineSnapshot {
    let generated_at: Timestamp = "2026-10-24T12:00:00Z".parse().unwrap();
    let mut codex = ProviderSnapshot::new(
        ariadusage_protocol::ProviderId::new("codex").unwrap(),
        "Codex",
        true,
        "oauth",
    );
    codex.updated_at = Some(generated_at);
    let mut codex_primary = RateWindow::new(25.0);
    codex_primary.window_minutes = Some(300);
    codex_primary.resets_at = Some("2026-10-24T15:00:00Z".parse().unwrap());
    codex.windows.primary = Some(Metric::value(
        codex_primary,
        Some(generated_at),
        None,
        Confidence::Exact,
    ));
    codex.status = Some(Metric::value(
        ProviderStatus {
            indicator: StatusIndicator::None,
            description: Some("All Systems Operational".to_string()),
            updated_at: Some(generated_at),
            page_url: Some("https://status.openai.com".to_string()),
            components: Vec::new(),
        },
        Some(generated_at),
        None,
        Confidence::Exact,
    ));

    let mut claude = ProviderSnapshot::new(
        ariadusage_protocol::ProviderId::new("claude").unwrap(),
        "Claude",
        true,
        "web",
    );
    claude.updated_at = Some(generated_at);
    let mut claude_primary = RateWindow::new(40.0);
    claude_primary.window_minutes = Some(300);
    claude_primary.resets_at = Some("2026-10-24T14:00:00Z".parse().unwrap());
    claude.windows.primary = Some(Metric::value(
        claude_primary,
        Some(generated_at),
        None,
        Confidence::Exact,
    ));

    EngineSnapshot::new(
        generated_at,
        300,
        EngineInfo {
            version: "0.1.0".to_string(),
            refreshing: false,
        },
        vec![codex, claude],
    )
}

// Rust-added: IPC v1 end-to-end integration test across usage, cards, and status
#[test]
fn ipc_v1_end_to_end_usage_cards_and_status_commands() {
    let expected_snapshot = sample_engine_snapshot();
    #[cfg(unix)]
    let fetched = {
        let dir = tempfile::Builder::new()
            .prefix("au-ipc-")
            .tempdir_in("/tmp")
            .unwrap();
        let sock_path = dir.path().join("engine.sock");
        let listener = UnixListener::bind(&sock_path).unwrap();
        let server_snap = expected_snapshot.clone();

        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.contains("\"hello\""));
            let welcome = ServerMessage::Welcome {
                protocol: PROTOCOL_V1.to_string(),
                engine_version: "0.1.0".to_string(),
                capabilities: Vec::new(),
                id: None,
            };
            writeln!(stream, "{}", serde_json::to_string(&welcome).unwrap()).unwrap();

            line.clear();
            reader.read_line(&mut line).unwrap();
            assert!(line.contains("\"getSnapshot\""));
            let snap_msg = ServerMessage::Snapshot {
                snapshot: Box::new(server_snap),
            };
            writeln!(stream, "{}", serde_json::to_string(&snap_msg).unwrap()).unwrap();
            stream.flush().unwrap();
        });

        let fetched = fetch_daemon_snapshot(Some(sock_path)).unwrap();
        server.join().unwrap();
        assert_eq!(fetched, expected_snapshot);
        fetched
    };
    #[cfg(not(unix))]
    let fetched = expected_snapshot;

    let now: Timestamp = "2026-10-24T12:00:00Z".parse().unwrap();
    let term_plain = TerminalCapabilities {
        is_tty: false,
        columns: 80,
        width: 80,
        enhanced_cards: false,
        ..TerminalCapabilities::default()
    };

    // 1. `usage` plain text
    let (opts, parsed) =
        parse_usage_args(&["--provider".to_string(), "codex".to_string()]).unwrap();
    let text_res = execute_usage_from_snapshot(&fetched, &opts, &parsed, false, term_plain, now);
    assert_eq!(text_res.exit_code, 0);
    assert!(text_res.stdout.contains("Codex (oauth)"));
    assert!(text_res.stdout.contains("Session: 75% left"));

    // 2. `usage --format json`
    let (json_opts, json_parsed) = parse_usage_args(&[
        "--provider".to_string(),
        "codex".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ])
    .unwrap();
    let json_res =
        execute_usage_from_snapshot(&fetched, &json_opts, &json_parsed, false, term_plain, now);
    assert_eq!(json_res.exit_code, 0);
    assert!(json_res.stdout.contains("\"provider\":\"codex\""));

    // 3. `usage --format toon`
    let (toon_opts, toon_parsed) = parse_usage_args(&[
        "--provider".to_string(),
        "codex".to_string(),
        "--format".to_string(),
        "toon".to_string(),
    ])
    .unwrap();
    let toon_res =
        execute_usage_from_snapshot(&fetched, &toon_opts, &toon_parsed, false, term_plain, now);
    assert_eq!(toon_res.exit_code, 0);
    assert!(toon_res.stdout.contains("provider: codex"));

    // 4. `cards` grid and `cards --brief`
    let (cards_opts, cards_parsed) = parse_cards_args(&["--no-color".to_string()]).unwrap();
    let cards_res =
        execute_usage_from_snapshot(&fetched, &cards_opts, &cards_parsed, true, term_plain, now);
    assert_eq!(cards_res.exit_code, 0);
    assert!(cards_res.stdout.contains("╭"));
    assert!(cards_res.stdout.contains("Codex"));
    assert!(cards_res.stdout.contains("Claude"));

    let (brief_opts, brief_parsed) =
        parse_cards_args(&["--brief".to_string(), "--no-color".to_string()]).unwrap();
    let brief_res =
        execute_usage_from_snapshot(&fetched, &brief_opts, &brief_parsed, true, term_plain, now);
    assert_eq!(brief_res.exit_code, 0);
    assert!(brief_res.stdout.contains("ariadusage • AI Usage & Limits"));

    // 5. `status` text and json
    let status_text = execute_status_from_snapshot(&fetched, Some("codex"), false, false).unwrap();
    assert!(status_text.contains("codex: enabled=true source=oauth status=Operational"));
    let status_json = execute_status_from_snapshot(&fetched, Some("codex"), true, false).unwrap();
    assert!(status_json.contains("\"id\":\"codex\""));
}

// Rust-added: verify ExitCode mapping (2, 3, 4, 64) in execute_usage_from_snapshot
#[test]
fn execute_usage_maps_provider_errors_to_specific_exit_codes() {
    let generated_at: Timestamp = "2026-10-24T12:00:00Z".parse().unwrap();
    let term_plain = TerminalCapabilities {
        is_tty: false,
        columns: 80,
        width: 80,
        enhanced_cards: false,
        ..TerminalCapabilities::default()
    };
    let (opts, parsed) =
        parse_usage_args(&["--provider".to_string(), "codex".to_string()]).unwrap();

    for (kind, msg, expected_code) in [
        (
            ProviderErrorKind::Unknown,
            "Codex CLI is not installed",
            ExitCode::ProviderNotInstalled.raw_value(),
        ),
        (
            ProviderErrorKind::ParseFailure,
            "Could not parse usage response",
            ExitCode::ParseError.raw_value(),
        ),
        (
            ProviderErrorKind::Unknown,
            "Timed out waiting for response",
            ExitCode::Timeout.raw_value(),
        ),
        (
            ProviderErrorKind::Unknown,
            "Unexpected server error",
            ExitCode::UnexpectedFailure.raw_value(),
        ),
    ] {
        let mut codex =
            ProviderSnapshot::new(ProviderId::new("codex").unwrap(), "Codex", true, "cli");
        codex.last_error = Some(ProviderError {
            kind,
            category: ProviderErrorCategory::Unknown,
            message: msg.to_string(),
            retry_after_seconds: None,
        });
        let snapshot = EngineSnapshot::new(
            generated_at,
            300,
            EngineInfo {
                version: "0.1.0".to_string(),
                refreshing: false,
            },
            vec![codex],
        );
        let res =
            execute_usage_from_snapshot(&snapshot, &opts, &parsed, false, term_plain, generated_at);
        assert_eq!(res.exit_code, expected_code);
        assert!(res.stderr.contains(msg));
    }

    let (bad_opts, bad_parsed) =
        parse_usage_args(&["--provider".to_string(), "unknown-provider".to_string()]).unwrap();
    let empty_snapshot = EngineSnapshot::new(
        generated_at,
        300,
        EngineInfo {
            version: "0.1.0".to_string(),
            refreshing: false,
        },
        Vec::new(),
    );
    let arg_res = execute_usage_from_snapshot(
        &empty_snapshot,
        &bad_opts,
        &bad_parsed,
        false,
        term_plain,
        generated_at,
    );
    assert_eq!(arg_res.exit_code, ExitCode::InvalidArguments.raw_value());
}

// Rust-added: verify ~ expansion in ClaudeSwapAccountReader and AriadUsage branding in ClaudeSwapListParserError
#[test]
fn claude_swap_reader_expands_leading_tilde_and_uses_ariadusage_branding() {
    let expanded = ClaudeSwapAccountReader::resolved_executable_path("~/bin/claude-swap").unwrap();
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        let expected = Path::new(&home).join("bin/claude-swap");
        assert_eq!(expanded, expected.to_string_lossy());
    }
    assert_eq!(
        ClaudeSwapAccountReader::resolved_executable_path("  /usr/local/bin/claude-swap  ")
            .unwrap(),
        "/usr/local/bin/claude-swap"
    );

    let err = ClaudeSwapListParser::parse(r#"{"schemaVersion":2,"accounts":[]}"#).unwrap_err();
    assert_eq!(err, ClaudeSwapListParserError::UnsupportedSchemaVersion(2));
    assert_eq!(
        err.to_string(),
        "claude-swap output uses unsupported schema version 2; AriadUsage supports version 1."
    );
}
