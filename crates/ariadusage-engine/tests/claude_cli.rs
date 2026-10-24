// Ported from CodexBar Tests/CodexBarTests/StatusProbeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeCLIScreenTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeCLIScreenProbeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeCLISpendCapTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeCLIScopedWeeklyUsageTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeAllModelsWeeklyDuplicateTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeCLIWorkspaceTrustTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeCLISessionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeCLIPanelTransitionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeCLIAuthStatusProbeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeCLIBackgroundAvailabilityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeCLIUsageSpawnThrottleTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeCLITimeoutRetryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeDirectUsageFallbackTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeProbeWorkingDirectoryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeResetJSONParserTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeResetOccurrenceTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeUsageInsightsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#[path = "support/https.rs"]
mod https;

use std::collections::BTreeMap;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use ariadusage_core::config::CookieSource;
use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::model::{NamedRateWindow, RateWindow, UsageSnapshot};
use ariadusage_core::pipeline::{
    AttemptOutcome, BoxFuture, FetchContext, FetchError, FetchInteraction, FetchPhase, FetchResult,
    FetchRuntime, FetchStrategy, Sleeper, fetch_outcome,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::net::NetConfig;
use ariadusage_engine::brokers::process::ProcessRegistry;
#[cfg(unix)]
use ariadusage_engine::providers::claude::prepared_probe_working_directory;
use ariadusage_engine::providers::claude::{
    CLAUDE_CLI_STRATEGY_ID, CLAUDE_CONFIG_DIR_ENV_KEY, CLAUDE_OAUTH_STRATEGY_ID,
    CLAUDE_SECURE_STORAGE_CONFIG_DIR_ENV_KEY, CLAUDE_WEB_STRATEGY_ID, ClaudeAccountProfile,
    ClaudeCLIAuthStatusProbe, ClaudeCLIAuthenticationStatus, ClaudeCLIBackgroundMarker,
    ClaudeCLIFetchStrategy, ClaudeCLIRateLimitGate, ClaudeCLIScreen, ClaudeCLISession,
    ClaudeCLISpendCapParser, ClaudeCLISubprocessResult, ClaudeCLIUsageLoader,
    ClaudeCLIUsageSpawnThrottle, ClaudeCLIUsageSpawnThrottleKey, ClaudeOAuthEndpointConfig,
    ClaudeOAuthStrategyConfig, ClaudeProbeSessionArtifactCleaner, ClaudeProviderPlan,
    ClaudeScopedWeeklyLimitMapper, ClaudeStatusProbe, ClaudeStatusProbeError,
    ClaudeStatusProbeParser, ClaudeStatusSnapshot, ClaudeWebEndpointConfig, OAuthScopedLimitEntry,
    SESSION_KEY_COOKIE_NAME, SUBSCRIPTION_QUOTA_UNAVAILABLE_DESCRIPTION,
    is_dedicated_probe_working_directory, isolated_probe_working_directory,
    merge_extra_rate_windows, probe_working_directory,
};
#[cfg(target_os = "linux")]
use ariadusage_engine::providers::claude::{ClaudeCLICaptureRequest, ClaudeCLISessionError};
use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use ariadusage_protocol::{Confidence, ProviderId};
use httpmock::prelude::*;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use reqwest::Url;
use tokio_util::sync::CancellationToken;

struct NoopSleeper;

impl Sleeper for NoopSleeper {
    fn sleep<'a>(&'a self, _duration: Duration) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/codexbar/providers/claude")
        .join(name)
}

fn make_call(interaction: FetchInteraction) -> BrokerCall {
    BrokerCall {
        interaction,
        cancel: CancellationToken::new(),
        request_id: "test-claude-cli".to_owned(),
    }
}

fn make_context(
    runtime: FetchRuntime,
    source_mode: SourceMode,
    interaction: FetchInteraction,
    env: BTreeMap<String, SecretString>,
) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("claude").unwrap(),
        runtime,
        interaction,
        phase: FetchPhase::Regular,
        request_id: "test-claude-cli-cx".to_owned(),
        source_mode,
        env,
        include_credits: false,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    }
}

#[test]
fn cursor_and_erase_operations_preserve_visible_cells() {
    let cases = [
        ("abc\rZ", "Zbc"),
        ("one\r\ntwo\nthree", "one\ntwo\nthree"),
        ("abc\u{8}Z", "abZ"),
        ("abc\r\u{1b}[2CZ", "abZ"),
        ("abc\u{1b}[2DZ", "aZc"),
        ("a\u{1b}[2Cb", "a  b"),
        ("abc\r\u{1b}[CZ", "aZc"),
        ("abc\u{1b}[DZ", "abZ"),
        ("one\r\ntwo\u{1b}[1A\u{1b}[1GX", "Xne\ntwo"),
        ("one\r\u{1b}[2Btwo", "one\n\ntwo"),
        ("abc\u{1b}[2GZ", "aZc"),
        ("abc\u{1b}[2;2HZ", "abc\n Z"),
        ("abc\u{1b}[2;2fZ", "abc\n Z"),
        ("abc\u{1b}[0;0HZ", "Zbc"),
        ("one\r\ntwo\u{1b}[HZ", "Zne\ntwo"),
        ("abc\r\u{1b}[2G\u{1b}[K", "a"),
        ("abc\u{1b}[2G\u{1b}[1K", "  c"),
        ("abc\u{1b}[2K", ""),
        ("abc\nxyz\u{1b}[1;2H\u{1b}[J", "a"),
        ("abc\nxyz\u{1b}[2;2H\u{1b}[1J", "\n  z"),
        ("abc\nxyz\u{1b}[2JQ", "\n   Q"),
        ("a\u{1b}[38;5;42mb\u{1b}[0m\u{1b}[?25lc", "abc"),
        ("a\u{1b}]0;hidden\u{7}b\u{1b}]8;;hidden\u{1b}\\c", "abc"),
        ("a\u{1b}]0;hidden", "a"),
        ("a\u{1b}[123;", "a"),
        ("a\u{1b}[?2J\u{1b}[99K\u{1b}[99Jb", "ab"),
        ("Org: 中文\u{1b}[10G Labs", "Org: 中文 Labs"),
        ("🎉 ok\u{1b}[4GX", "🎉 Xk"),
        ("⚠️ ok\u{1b}[4GX", "⚠️ Xk"),
        ("██▌\u{1b}[5G51%", "██▌ 51%"),
        ("中文\u{1b}[2Gab", " ab"),
        ("中文x\u{1b}[4G\u{1b}[K", "中"),
        ("e\u{1b}[0m\u{301}x", "éx"),
    ];

    for (stream, expected) in cases {
        assert_eq!(
            ClaudeCLIScreen::render(stream),
            expected,
            "failed on stream: {stream:?}"
        );
    }
}

#[test]
fn synthetic_differential_frame_matches_visible_panel_golden() {
    let frame = concat!(
        "\u{1b}[HCurrent session\n",
        "3% used\n",
        "Current week (all models)\n",
        "27% used\n",
        "Current week (Fable)\n",
        "abc xye jkl",
        "\u{1b}[6;1H51%\u{1b}[5Gus\u{1b}[8Gd\u{1b}[9G\u{1b}[K"
    );
    let golden = concat!(
        "Current session\n",
        "3% used\n",
        "Current week (all models)\n",
        "27% used\n",
        "Current week (Fable)\n",
        "51% used"
    );
    assert_eq!(ClaudeCLIScreen::render(frame), golden);
    let actual = ClaudeStatusProbeParser::parse(frame, None).unwrap();
    let expected = ClaudeStatusProbeParser::parse(golden, None).unwrap();
    assert_eq!(actual.session_percent_left, expected.session_percent_left);
    assert_eq!(actual.weekly_percent_left, expected.weekly_percent_left);
    assert_eq!(actual.extra_rate_windows, expected.extra_rate_windows);
}

#[test]
fn screen_geometry_bounds_backspace_clear_and_tall_moves() {
    let snap = ClaudeStatusProbeParser::parse("Current session\n39\u{8}5% used", None).unwrap();
    assert_eq!(snap.session_percent_left, Some(65));

    let cleared = "\u{1b}[HCurrent session\n3% used\u{1b}[2J";
    assert!(ClaudeStatusProbeParser::parse(cleared, None).is_err());

    let huge = "9".repeat(100);
    let frame = format!("\u{1b}[{huge};{huge}HQ\u{1b}[{huge}AZ\u{1b}[{huge}DX");
    let rendered = ClaudeCLIScreen::render(&frame);
    let lines: Vec<&str> = rendered.split('\n').collect();
    assert_eq!(lines.len(), ClaudeCLIScreen::ROWS);
    assert!(lines.iter().all(|l| l.len() <= ClaudeCLIScreen::COLUMNS));
    assert_eq!(
        lines[0],
        format!("X{}Z", " ".repeat(ClaudeCLIScreen::COLUMNS - 2))
    );
    assert_eq!(
        *lines.last().unwrap(),
        format!("{}Q", " ".repeat(ClaudeCLIScreen::COLUMNS - 1))
    );

    let full = "x".repeat(ClaudeCLIScreen::COLUMNS);
    assert_eq!(
        ClaudeCLIScreen::render(&format!("{full}Z")),
        format!("{full}\nZ")
    );
    assert_eq!(
        ClaudeCLIScreen::render(&format!("{full}\u{1b}[31mZ")),
        format!("{full}\nZ")
    );
    assert_eq!(
        ClaudeCLIScreen::render(&format!("{full}\u{1b}[99KZ")),
        format!("{full}\nZ")
    );
    let narrow = "x".repeat(ClaudeCLIScreen::COLUMNS - 1);
    assert_eq!(
        ClaudeCLIScreen::render(&format!("{narrow}中Z")),
        format!("{narrow}\n中Z")
    );
    let rows: Vec<String> = (0..=ClaudeCLIScreen::ROWS).map(|i| i.to_string()).collect();
    assert_eq!(
        ClaudeCLIScreen::render(&rows.join("\r\n")),
        rows[1..].join("\n")
    );

    for movement in ["200C", "200G", "200;200H", "200;200f"] {
        let tall = format!("top\u{1b}[200B\u{1b}[{movement}X");
        let tall_rendered = ClaudeCLIScreen::render(&tall);
        let tall_lines: Vec<&str> = tall_rendered.split('\n').collect();
        assert_eq!(tall_lines.len(), 200);
        assert_eq!(tall_lines[0], "top");
        assert_eq!(
            *tall_lines.last().unwrap(),
            format!("{}X", " ".repeat(ClaudeCLIScreen::COLUMNS - 1))
        );
    }

    let erase_below =
        "\u{1b}[180;1HCurrent session\n7% used\u{1b}[200;1Hfooter\u{1b}[180;1H\u{1b}[J";
    assert!(ClaudeCLIScreen::render(erase_below).is_empty());
    assert!(ClaudeStatusProbeParser::parse(erase_below, None).is_err());
}

#[test]
fn parses_all_upstream_ansi_capture_fixtures() {
    let usage_diff =
        fs::read_to_string(fixture_path("usage-pty-differential-redraw.ansi")).unwrap();
    let snap = ClaudeStatusProbeParser::parse(&usage_diff, None).unwrap();
    assert_eq!(snap.session_percent_left, Some(97));
    assert_eq!(snap.weekly_percent_left, Some(73));
    assert_eq!(
        snap.secondary_reset_description.as_deref(),
        Some("Resets Sep 23 at 3pm (Europe/Stockholm)")
    );
    let fable = snap
        .extra_rate_windows
        .iter()
        .find(|w| w.id == "claude-weekly-scoped-fable")
        .expect("should contain scoped Fable window");
    assert_eq!(fable.title, "Fable only");
    assert_eq!(fable.window.used_percent, 51.0);

    let tall_panel = fs::read_to_string(fixture_path("usage-pty-2.1.294-tall-panel.ansi")).unwrap();
    let snap_tall = ClaudeStatusProbeParser::parse(&tall_panel, None).unwrap();
    assert_eq!(snap_tall.session_percent_left, Some(93));
    assert_eq!(
        snap_tall.primary_reset_description.as_deref(),
        Some("Resets 2:30pm (Europe/Malta)")
    );
    assert_eq!(snap_tall.weekly_percent_left, Some(80));
    assert_eq!(
        snap_tall.secondary_reset_description.as_deref(),
        Some("Resets Oct 14, 10pm (Europe/Malta)")
    );
    let fable_tall = snap_tall
        .extra_rate_windows
        .iter()
        .find(|w| w.id == "claude-weekly-scoped-fable")
        .expect("should contain scoped Fable window in tall panel");
    assert_eq!(fable_tall.window.used_percent, 2.0);

    let status_diff =
        fs::read_to_string(fixture_path("status-pty-differential-redraw.ansi")).unwrap();
    let identity = ClaudeStatusProbeParser::parse_identity(None, Some(&status_diff));
    assert_eq!(
        identity.account_email.as_deref(),
        Some("fixture@example.com")
    );
    assert_eq!(
        identity.account_organization.as_deref(),
        Some("Example Org")
    );
    assert_eq!(identity.login_method.as_deref(), Some("Max"));
    let usage_simple = "Current session\n3% used";
    let combined = ClaudeStatusProbeParser::parse(usage_simple, Some(&status_diff)).unwrap();
    assert_eq!(combined.account_email, identity.account_email);
    assert_eq!(combined.account_organization, identity.account_organization);
    assert_eq!(combined.login_method, identity.login_method);
    assert_eq!(combined.raw_text, format!("{usage_simple}{status_diff}"));

    let title = "\u{1b}]0;\u{1b}[HCurrent session 99% used\u{7}";
    let styled_cr = format!("{title}\u{1b}[35mCurrent session\u{1b}[0m\r3% used\r");
    let snap_cr = ClaudeStatusProbeParser::parse(&styled_cr, None).unwrap();
    assert_eq!(snap_cr.session_percent_left, Some(97));

    let padding = "report detail\n".repeat(ClaudeCLIScreen::ROWS + 5);
    let organization = "Example".repeat(30);
    let styled_long = format!(
        "\u{1b}[32mCurrent session\n3% used\n{padding}Org: {organization}\nEmail: fixture@example.com\u{1b}[0m"
    );
    let snap_long = ClaudeStatusProbeParser::parse(&styled_long, None).unwrap();
    assert_eq!(snap_long.session_percent_left, Some(97));
    assert_eq!(
        snap_long.account_organization.as_deref(),
        Some(organization.as_str())
    );
    assert_eq!(
        snap_long.account_email.as_deref(),
        Some("fixture@example.com")
    );

    let dialog = fs::read_to_string(fixture_path("workspace-trust-dialog.ansi")).unwrap();
    let screen = ClaudeCLIScreen::render(&dialog);
    let lines: Vec<&str> = screen.split('\n').collect();
    assert!(lines.contains(&" ❯ No, exit"));
    assert!(lines.contains(&"   Yes, I trust this folder"));
    assert_eq!(
        ClaudeCLISession::workspace_trust_keys(&screen, true),
        Some("\x1b[B")
    );
    assert_eq!(
        ClaudeCLISession::workspace_trust_keys(&screen, false),
        Some("\x1b")
    );

    let select_down = fs::read_to_string(fixture_path("workspace-trust-select-down.ansi")).unwrap();
    let redrawn = ClaudeCLIScreen::render(&format!("{dialog}{select_down}"));
    let redrawn_lines: Vec<&str> = redrawn.split('\n').collect();
    assert!(redrawn_lines.contains(&"   No, exit"));
    assert!(redrawn_lines.contains(&" ❯ Yes, I trust this folder"));
    assert_eq!(
        ClaudeCLISession::workspace_trust_keys(&redrawn, true),
        Some("\r")
    );
    assert_eq!(
        ClaudeCLISession::workspace_trust_keys(&redrawn, false),
        Some("\x1b")
    );
}

#[test]
fn spend_cap_parser_extracts_usage_credits_and_handles_commas() {
    let now = Timestamp::from_second(1_766_500_000).unwrap();
    let panel = concat!(
        "Current session\n",
        "10% used\n",
        "Usage credits\n",
        "████████░░ 25% used\n",
        "$12.50 / $50.00 spent\n",
        "Resets Jan 1"
    );
    let cost = ClaudeCLISpendCapParser::parse(panel, now).expect("should parse spend cap");
    assert_eq!(cost.used, 12.5);
    assert_eq!(cost.limit, 50.0);
    assert_eq!(cost.currency_code, "USD");
    assert_eq!(cost.period.as_deref(), Some("Monthly cap"));
    assert_eq!(cost.resets_at, None);

    // Single-line header + spend cap
    let single_line = "Current session\n10% used\nUsage credits 25% used $12.50 / $50.00 spent";
    let single_cost =
        ClaudeCLISpendCapParser::parse(single_line, now).expect("should parse single-line cap");
    assert_eq!(single_cost.used, 12.5);
    assert_eq!(single_cost.limit, 50.0);

    // CR-delimited spend cap capture
    let cr_panel = "Current session\r10% used\rUsage credits\r████████░░ 25% used\r$12.50 / $50.00 spent\rResets Jan 1";
    let cr_cost = ClaudeCLISpendCapParser::parse(cr_panel, now).expect("should parse CR spend cap");
    assert_eq!(cr_cost.used, 12.5);
    assert_eq!(cr_cost.limit, 50.0);

    let comma_panel = concat!("Usage credits\n", "$1,250.50 / $5,000.00 spent\n");
    let comma_cost =
        ClaudeCLISpendCapParser::parse(comma_panel, now).expect("should parse comma spend cap");
    assert_eq!(comma_cost.used, 1250.5);
    assert_eq!(comma_cost.limit, 5000.0);

    // Non-USD, prefixed, malformed, zero, or out-of-range amounts are ignored
    let rejected_lines = [
        "€40.00 / €100.00 spent",
        "CA$40.00 / CA$100.00 spent",
        "-$40.00 / $100.00 spent",
        "$12,34 / $100.00 spent",
        "$1.23456 / $100.00 spent",
        "$0.00 / $0.00 spent",
        "$10.00 / $1,000,000,000,000.01 spent",
    ];
    for amount_line in rejected_lines {
        assert!(
            ClaudeCLISpendCapParser::parse(&format!("Usage credits\n{amount_line}"), now).is_none(),
            "should reject spend line: {amount_line}"
        );
    }

    // Exactly at 1e12 bound is accepted
    let max_bound =
        ClaudeCLISpendCapParser::parse("Usage credits\n$10.00 / $1,000,000,000,000.00 spent", now)
            .expect("should accept 1e12 limit");
    assert_eq!(max_bound.used, 10.0);
    assert_eq!(max_bound.limit, 1_000_000_000_000.0);

    // Distance bound: 5 filler lines after header (line offset 6) succeeds; 6 filler lines (offset 7) fails
    let within_window = format!(
        "Usage credits\n{}\n$12.50 / $50.00 spent",
        ["line"; 5].join("\n")
    );
    assert!(ClaudeCLISpendCapParser::parse(&within_window, now).is_some());
    let past_window = format!(
        "Usage credits\n{}\n$12.50 / $50.00 spent",
        ["line"; 6].join("\n")
    );
    assert!(ClaudeCLISpendCapParser::parse(&past_window, now).is_none());

    let old = "Usage credits\n$25.00 / $100.00 spent\n";
    assert!(
        ClaudeCLISpendCapParser::parse(&format!("{old}Usage credits\n€40.00 / €100.00 spent"), now)
            .is_none()
    );
    assert!(
        ClaudeCLISpendCapParser::parse(&format!("{old}Usage credits\nLoading usage"), now)
            .is_none()
    );

    let newest_valid = ClaudeCLISpendCapParser::parse(
        "Usage credits\n€40.00 / €100.00 spent\nUsage credits\n$42.00 / $1,000.00 spent",
        now,
    )
    .expect("newest valid spend frame should win");
    assert_eq!(newest_valid.used, 42.0);
    assert_eq!(newest_valid.limit, 1000.0);
    assert_eq!(newest_valid.currency_code, "USD");

    assert!(ClaudeCLISpendCapParser::parse("Current session\n10% used", now).is_none());
}

#[test]
fn parser_reset_dates_subscription_notice_json_and_debug_redaction() {
    // 1. Exact Timestamp assertions from ClaudeResetOccurrenceTests.swift
    let ny = TimeZone::get("America/New_York").unwrap();
    // 2026-11-01 DST fallback in America/New_York:
    // first 1:30am (EDT, UTC-4) = 2026-11-01T05:30:00Z
    // second 1:30am (EST, UTC-5) = 2026-11-01T06:30:00Z
    // tomorrow 1:30am (EST, UTC-5) = 2026-11-02T06:30:00Z
    let dst_first = "2026-11-01T05:30:00Z".parse::<Timestamp>().unwrap();
    let dst_second = "2026-11-01T06:30:00Z".parse::<Timestamp>().unwrap();
    let dst_tomorrow = "2026-11-02T06:30:00Z".parse::<Timestamp>().unwrap();
    for (now_ts, expected_ts) in [
        (
            dst_first
                .checked_sub(jiff::SignedDuration::from_secs(60))
                .unwrap(),
            dst_first,
        ),
        (
            dst_first
                .checked_add(jiff::SignedDuration::from_secs(30 * 60))
                .unwrap(),
            dst_second,
        ),
        (
            dst_second
                .checked_add(jiff::SignedDuration::from_secs(60))
                .unwrap(),
            dst_tomorrow,
        ),
    ] {
        assert_eq!(
            ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
                Some("Resets 1:30am (America/New_York)"),
                now_ts,
                None,
                &ny,
            ),
            Some(expected_ts)
        );
    }
    let between_occurrences = dst_first
        .checked_add(jiff::SignedDuration::from_secs(30 * 60))
        .unwrap();
    let after_second = dst_second
        .checked_add(jiff::SignedDuration::from_secs(60))
        .unwrap();
    assert_eq!(
        ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
            Some("Resets Nov 1, 1:30am (America/New_York)"),
            between_occurrences,
            None,
            &ny,
        ),
        Some(dst_second)
    );
    assert_eq!(
        ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
            Some("Resets Nov 1, 1:30am (America/New_York)"),
            after_second,
            Some(Duration::from_secs(7 * 24 * 60 * 60)),
            &ny,
        ),
        Some(dst_second)
    );
    assert_eq!(
        ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
            Some("Resets Nov 1, 2026, 1:30am (America/New_York)"),
            between_occurrences,
            None,
            &ny,
        ),
        Some(dst_second)
    );
    assert_eq!(
        ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
            Some("Resets Nov 1, 2026, 1:30am (America/New_York)"),
            after_second,
            None,
            &ny,
        ),
        Some(dst_second)
    );

    // ClaudeResetOccurrenceTests.swift:58-82 — parser searches across leap years
    let leap_2028 = "2028-02-29T09:00:00Z".parse::<Timestamp>().unwrap();
    for now_str in ["2025-01-01T00:00:00Z", "2024-03-01T00:00:00Z"] {
        let now_ts = now_str.parse::<Timestamp>().unwrap();
        assert_eq!(
            ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
                Some("Resets Feb 29, 9am (UTC)"),
                now_ts,
                None,
                &TimeZone::UTC,
            ),
            Some(leap_2028)
        );
    }
    assert_eq!(
        ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
            Some("Resets Feb 29, 9am (UTC)"),
            "2024-02-29T10:00:00Z".parse::<Timestamp>().unwrap(),
            Some(Duration::from_secs(7 * 24 * 60 * 60)),
            &TimeZone::UTC,
        ),
        Some("2024-02-29T09:00:00Z".parse::<Timestamp>().unwrap())
    );

    // ClaudeResetOccurrenceTests.swift:85-108 — explicit years authoritative across supported time forms
    let now_2025 = "2025-01-01T00:00:00Z".parse::<Timestamp>().unwrap();
    for (text, expected_str) in [
        (
            "Resets Jan 2, 2026, 10:59pm (Europe/Helsinki)",
            "2026-01-02T20:59:00Z",
        ),
        ("Resets Jan 2 2026 10pm (UTC)", "2026-01-02T22:00:00Z"),
        ("Resets Jan 2, 2026, 22:15 (UTC)", "2026-01-02T22:15:00Z"),
        ("Resets Jan 2, 2026, 22 (UTC)", "2026-01-02T22:00:00Z"),
    ] {
        assert_eq!(
            ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
                Some(text),
                now_2025,
                None,
                &TimeZone::UTC,
            ),
            Some(expected_str.parse::<Timestamp>().unwrap()),
            "failed explicit year case: {text}"
        );
    }
    assert_eq!(
        ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
            Some("Resets Jan 2, 2026, 10:59pm (Europe/Helsinki)"),
            "2027-01-01T00:00:00Z".parse::<Timestamp>().unwrap(),
            None,
            &TimeZone::UTC,
        ),
        Some("2026-01-02T20:59:00Z".parse::<Timestamp>().unwrap())
    );

    // ClaudeResetOccurrenceTests.swift:111-116 — rejects nonexistent explicit local time (DST spring-forward gap)
    assert_eq!(
        ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
            Some("Resets Mar 8, 2026, 2:30am (America/New_York)"),
            "2026-01-01T00:00:00Z".parse::<Timestamp>().unwrap(),
            None,
            &ny,
        ),
        None
    );

    // 2. StatusProbeTests.swift:659-834 reset-date cases
    // StatusProbeTests.swift:659-693 — uses the five hour window to resolve stale claude reset times
    for (now_str, text, expected_str) in [
        (
            "2026-07-09T15:00:00Z",
            "Resets 3pm (UTC)",
            "2026-07-09T15:00:00Z",
        ),
        (
            "2026-07-09T15:05:00Z",
            "Resets 3pm (UTC)",
            "2026-07-09T15:00:00Z",
        ),
        (
            "2026-07-09T20:00:00Z",
            "Resets 3pm (UTC)",
            "2026-07-09T15:00:00Z",
        ),
        (
            "2026-07-09T23:59:00Z",
            "Resets 12:01am (UTC)",
            "2026-07-10T00:01:00Z",
        ),
        (
            "2026-07-10T00:01:00Z",
            "Resets 11:59pm (UTC)",
            "2026-07-09T23:59:00Z",
        ),
    ] {
        assert_eq!(
            ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
                Some(text),
                now_str.parse::<Timestamp>().unwrap(),
                Some(Duration::from_secs(5 * 60 * 60)),
                &TimeZone::UTC,
            ),
            Some(expected_str.parse::<Timestamp>().unwrap()),
            "failed session-window resolution: {text} at {now_str}"
        );
    }

    // StatusProbeTests.swift:696-709 — parses claude reset date and time
    let now_1733690000 = Timestamp::from_second(1_733_690_000).unwrap();
    assert_eq!(
        ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
            Some("Resets Dec 9, 8:59am (Europe/Helsinki)"),
            now_1733690000,
            None,
            &TimeZone::UTC,
        ),
        Some("2024-12-09T06:59:00Z".parse::<Timestamp>().unwrap())
    );

    // StatusProbeTests.swift:712-746 — uses the weekly window to resolve stale claude reset dates
    for (now_str, text, expected_str) in [
        (
            "2026-12-31T23:00:00Z",
            "Resets Jan 2, 3:15am (UTC)",
            "2027-01-02T03:15:00Z",
        ),
        (
            "2026-12-31T23:00:00Z",
            "Resets Jan 2, 3am (UTC)",
            "2027-01-02T03:00:00Z",
        ),
        (
            "2027-01-01T00:05:00Z",
            "Resets Dec 31, 11:59pm (UTC)",
            "2026-12-31T23:59:00Z",
        ),
        (
            "2027-01-01T00:05:00Z",
            "Resets Dec 31, 11pm (UTC)",
            "2026-12-31T23:00:00Z",
        ),
        (
            "2026-07-09T15:05:00Z",
            "Resets Jul 9, 3:00pm (UTC)",
            "2026-07-09T15:00:00Z",
        ),
    ] {
        assert_eq!(
            ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
                Some(text),
                now_str.parse::<Timestamp>().unwrap(),
                Some(Duration::from_secs(7 * 24 * 60 * 60)),
                &TimeZone::UTC,
            ),
            Some(expected_str.parse::<Timestamp>().unwrap()),
            "failed weekly-window resolution: {text} at {now_str}"
        );
    }

    // StatusProbeTests.swift:749-772 — public claude reset parser remains forward looking
    for (now_str, text, expected_str) in [
        (
            "2026-07-09T08:00:00Z",
            "Resets 9pm (UTC)",
            "2026-07-09T21:00:00Z",
        ),
        (
            "2026-07-09T20:00:00Z",
            "Resets 1pm (UTC)",
            "2026-07-10T13:00:00Z",
        ),
        (
            "2026-07-09T12:00:00Z",
            "Resets Jul 1, 9am (UTC)",
            "2027-07-01T09:00:00Z",
        ),
    ] {
        assert_eq!(
            ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
                Some(text),
                now_str.parse::<Timestamp>().unwrap(),
                None,
                &TimeZone::UTC,
            ),
            Some(expected_str.parse::<Timestamp>().unwrap()),
            "failed future resolution: {text} at {now_str}"
        );
    }

    // StatusProbeTests.swift:775-792 — stale same day claude reset resolves to past timestamp within 5h window
    let now_stale_same_day = "2026-07-09T15:05:00Z".parse::<Timestamp>().unwrap();
    let stale_reset_date = ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
        Some("Resets Jul 9, 3:00pm (UTC)"),
        now_stale_same_day,
        Some(Duration::from_secs(5 * 60 * 60)),
        &TimeZone::UTC,
    )
    .unwrap();
    assert_eq!(
        stale_reset_date,
        "2026-07-09T15:00:00Z".parse::<Timestamp>().unwrap()
    );
    assert!(stale_reset_date <= now_stale_same_day);

    // StatusProbeTests.swift:795-802 — parses claude reset with dot separated time
    assert_eq!(
        ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
            Some("Resets Dec 9 at 5.27am (UTC)"),
            now_1733690000,
            None,
            &TimeZone::UTC,
        ),
        Some("2024-12-09T05:27:00Z".parse::<Timestamp>().unwrap())
    );

    // StatusProbeTests.swift:805-824 — parses claude reset with compact times
    assert_eq!(
        ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
            Some("Resets 1pm (UTC)"),
            now_1733690000,
            None,
            &TimeZone::UTC,
        ),
        Some("2024-12-09T13:00:00Z".parse::<Timestamp>().unwrap())
    );
    assert_eq!(
        ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
            Some("Resets Dec 9, 9am"),
            now_1733690000,
            None,
            &TimeZone::UTC,
        ),
        Some("2024-12-09T09:00:00Z".parse::<Timestamp>().unwrap())
    );

    // StatusProbeTests.swift:827-834 — parses claude reset with compact date and time no spaces
    let now_1773097200 = Timestamp::from_second(1_773_097_200).unwrap();
    assert_eq!(
        ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
            Some("ResetsMar13at12:30pm(Asia/Calcutta)"),
            now_1773097200,
            None,
            &TimeZone::UTC,
        ),
        Some("2026-03-13T07:00:00Z".parse::<Timestamp>().unwrap())
    );

    let compact_now = "2026-03-10T00:00:00Z".parse::<Timestamp>().unwrap();
    assert_eq!(
        ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
            Some("Resets4:29am(Asia/Calcutta)"),
            compact_now,
            Some(Duration::from_secs(5 * 3600)),
            &TimeZone::UTC,
        ),
        Some("2026-03-09T22:59:00Z".parse::<Timestamp>().unwrap())
    );
    assert_eq!(
        ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
            Some("Resets4:29am(Asia/Calcutta)"),
            compact_now,
            None,
            &TimeZone::UTC,
        ),
        Some("2026-03-10T22:59:00Z".parse::<Timestamp>().unwrap())
    );

    // 3. ClaudeResetJSONParserTests.swift:7-26 & :29-49
    let json_horizons = br#"{
      "ok": true,
      "session_5h": { "pct_used": 1, "resets": "Jul 9 at 6am (UTC)" },
      "week_all_models": { "pct_used": 2, "resets": "Jul 9 at 6am (UTC)" },
      "week_sonnet": { "pct_used": 3, "resets": "Jul 9 at 6am (UTC)" }
    }"#;
    let now_horizons = "2026-07-09T12:00:00Z".parse::<Timestamp>().unwrap();
    let snap_horizons =
        ClaudeStatusProbeParser::parse_json(json_horizons, now_horizons).expect("valid JSON");
    assert_eq!(
        snap_horizons.primary.as_ref().unwrap().resets_at,
        Some("2027-07-09T06:00:00Z".parse::<Timestamp>().unwrap())
    );
    assert_eq!(
        snap_horizons.secondary.as_ref().unwrap().resets_at,
        Some("2026-07-09T06:00:00Z".parse::<Timestamp>().unwrap())
    );
    assert_eq!(
        snap_horizons.tertiary.as_ref().unwrap().resets_at,
        Some("2026-07-09T06:00:00Z".parse::<Timestamp>().unwrap())
    );
    assert_eq!(snap_horizons.updated_at, now_horizons);

    let json_explicit = br#"{
      "ok": true,
      "session_5h": { "pct_used": 1, "resets": "Jan 2, 2026, 10:59pm (Europe/Helsinki)" },
      "week_all_models": { "pct_used": 2, "resets": "after the next billing sync" }
    }"#;
    let snap_explicit = ClaudeStatusProbeParser::parse_json(json_explicit, now_2025)
        .expect("valid JSON with explicit year and malformed reset");
    assert_eq!(
        snap_explicit.primary.as_ref().unwrap().resets_at,
        Some("2026-01-02T20:59:00Z".parse::<Timestamp>().unwrap())
    );
    assert_eq!(
        snap_explicit
            .primary
            .as_ref()
            .unwrap()
            .reset_description
            .as_deref(),
        Some("Jan 2, 2026, 10:59pm (Europe/Helsinki)")
    );
    assert_eq!(snap_explicit.secondary.as_ref().unwrap().resets_at, None);
    assert_eq!(
        snap_explicit
            .secondary
            .as_ref()
            .unwrap()
            .reset_description
            .as_deref(),
        Some("after the next billing sync")
    );

    // 4. Debug redaction check across ClaudeStatusSnapshot, ClaudeCLIUsageSpawnThrottleKey, and ClaudeCLIBackgroundMarker
    let snap = ClaudeStatusSnapshot {
        session_percent_left: Some(80),
        weekly_percent_left: Some(60),
        opus_percent_left: None,
        extra_rate_windows: Vec::new(),
        provider_cost: None,
        account_email: Some("sensitive-email@example.com".to_owned()),
        account_organization: Some("Sensitive Organization".to_owned()),
        login_method: Some("Claude Pro".to_owned()),
        primary_reset_description: None,
        secondary_reset_description: None,
        opus_reset_description: None,
        raw_text: "sensitive raw text".to_owned(),
    };
    let debug_str = format!("{snap:?}");
    assert!(!debug_str.contains("sensitive-email@example.com"));
    assert!(!debug_str.contains("Sensitive Organization"));
    assert!(!debug_str.contains("sensitive raw text"));

    let key = ClaudeCLIUsageSpawnThrottleKey {
        binary: "/usr/bin/claude".to_owned(),
        account_scope: "sensitive-account-scope".to_owned(),
        use_web_extras: false,
        include_prepaid_balance: false,
    };
    assert!(!format!("{key:?}").contains("sensitive-account-scope"));
    let marker = ClaudeCLIBackgroundMarker {
        binary: "/usr/bin/claude".to_owned(),
        account_scope: "sensitive-account-scope".to_owned(),
    };
    assert!(!format!("{marker:?}").contains("sensitive-account-scope"));
}

#[test]
fn session_scope_probe_session_id_workspace_trust_and_artifact_cleaner() {
    assert!(!ClaudeStatusProbe::should_keep_cli_session_alive(false));
    assert!(ClaudeStatusProbe::should_keep_cli_session_alive(true));

    let temp = tempfile::tempdir().unwrap();
    let first_root = temp.path().join("first");
    let second_root = temp.path().join("second");
    let secure_root = temp.path().join("secure");
    fs::create_dir_all(&first_root).unwrap();
    fs::create_dir_all(&second_root).unwrap();
    fs::create_dir_all(&secure_root).unwrap();

    let call = make_call(FetchInteraction::UserInitiated);
    let mut env1 = BTreeMap::new();
    env1.insert(
        CLAUDE_CONFIG_DIR_ENV_KEY.to_owned(),
        SecretString::new(first_root.to_string_lossy().into_owned()),
    );
    let mut env2 = BTreeMap::new();
    env2.insert(
        CLAUDE_CONFIG_DIR_ENV_KEY.to_owned(),
        SecretString::new(second_root.to_string_lossy().into_owned()),
    );

    fs::write(
        first_root.join(".config.json"),
        r#"{"oauthAccount":{"accountUuid":"account-a"}}"#,
    )
    .unwrap();
    let scope_a1 =
        ClaudeAccountProfile::session_scope(&env1, temp.path(), temp.path(), "fb-1", &call);
    let scope_a2 =
        ClaudeAccountProfile::session_scope(&env1, temp.path(), temp.path(), "fb-2", &call);
    assert_eq!(scope_a1, scope_a2);

    fs::write(
        first_root.join(".config.json"),
        r#"{"oauthAccount":{"accountUuid":"account-b"}}"#,
    )
    .unwrap();
    let scope_b1 =
        ClaudeAccountProfile::session_scope(&env1, temp.path(), temp.path(), "fb-1", &call);
    assert_ne!(scope_a1, scope_b1);

    fs::write(
        second_root.join(".config.json"),
        r#"{"oauthAccount":{"accountUuid":"account-b"}}"#,
    )
    .unwrap();
    let scope_b2 =
        ClaudeAccountProfile::session_scope(&env2, temp.path(), temp.path(), "fb-1", &call);
    assert_ne!(scope_b1, scope_b2);

    let mut env_secure = env1.clone();
    env_secure.insert(
        CLAUDE_SECURE_STORAGE_CONFIG_DIR_ENV_KEY.to_owned(),
        SecretString::new(secure_root.to_string_lossy().into_owned()),
    );
    let scope_b_secure =
        ClaudeAccountProfile::session_scope(&env_secure, temp.path(), temp.path(), "fb-1", &call);
    assert_ne!(scope_b1, scope_b_secure);

    fs::remove_file(first_root.join(".config.json")).unwrap();
    let missing_1 =
        ClaudeAccountProfile::session_scope(&env1, temp.path(), temp.path(), "fb-1", &call);
    let missing_2 =
        ClaudeAccountProfile::session_scope(&env1, temp.path(), temp.path(), "fb-2", &call);
    assert_ne!(missing_1, missing_2);

    // Persisted probe session ID
    let probe_dir = temp.path().join("probe-dir");
    let sid1 = ClaudeCLISession::load_or_create_probe_session_id(&probe_dir);
    let sid2 = ClaudeCLISession::load_or_create_probe_session_id(&probe_dir);
    assert_eq!(sid1, sid2);
    assert_eq!(
        ClaudeCLISession::launch_arguments(&sid1),
        vec![
            "--allowed-tools".to_owned(),
            String::new(),
            "--strict-mcp-config".to_owned(),
            "--settings".to_owned(),
            r#"{"remoteControlAtStartup":false,"disableAllHooks":true}"#.to_owned(),
            "--session-id".to_owned(),
            sid1.clone(),
        ]
    );
    let session_file = probe_dir.join(".ariadusage-session-id");
    assert_eq!(fs::read_to_string(&session_file).unwrap(), sid1);
    #[cfg(unix)]
    {
        let mode = fs::metadata(&session_file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    fs::write(&session_file, "invalid-uuid").unwrap();
    let sid3 = ClaudeCLISession::load_or_create_probe_session_id(&probe_dir);
    assert_eq!(fs::read_to_string(&session_file).unwrap(), sid3);

    let unwritable = Path::new("/dev/null/Ariadusage-ClaudeProbe");
    let fallback1 = ClaudeCLISession::load_or_create_probe_session_id(unwritable);
    let fallback2 = ClaudeCLISession::load_or_create_probe_session_id(unwritable);
    assert_eq!(fallback1, fallback2);

    // Workspace trust prompt keys
    let legacy_trust_prompt = "Do you trust the files in this folder?";
    assert_eq!(
        ClaudeCLISession::workspace_trust_keys(legacy_trust_prompt, true),
        Some("y\r")
    );
    assert_eq!(
        ClaudeCLISession::workspace_trust_keys(legacy_trust_prompt, false),
        Some("\x1b")
    );
    let modern_selection = "Ready to code here?\n\n ❯ No, exit\n   Yes, I trust this folder";
    assert_eq!(
        ClaudeCLISession::workspace_trust_keys(modern_selection, true),
        Some("\x1b[B")
    );
    let unnumbered_prompt = "Ready to code here? Press Enter to continue";
    assert_eq!(
        ClaudeCLISession::workspace_trust_keys(unnumbered_prompt, true),
        Some("\r")
    );
    assert_eq!(
        ClaudeCLISession::workspace_trust_keys(unnumbered_prompt, false),
        Some("\x1b")
    );

    // Probe session artifact cleaner
    let project_dir_name = ClaudeProbeSessionArtifactCleaner::sanitized_project_directory_name(
        &probe_dir.to_string_lossy(),
    );
    let project_dir = first_root.join("projects").join(&project_dir_name);
    fs::create_dir_all(&project_dir).unwrap();
    let stale_jsonl = project_dir.join("stale.jsonl");
    let hidden_jsonl = project_dir.join(".hidden.jsonl");
    let keep_txt = project_dir.join("notes.txt");
    fs::write(&stale_jsonl, "{}\n").unwrap();
    fs::write(&hidden_jsonl, "{}\n").unwrap();
    fs::write(&keep_txt, "keep").unwrap();
    #[cfg(unix)]
    let symlink_jsonl = {
        let target = temp.path().join("outside-target.jsonl");
        fs::write(&target, "{}\n").unwrap();
        let link = project_dir.join("symlink.jsonl");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        link
    };

    let cleaned = ClaudeProbeSessionArtifactCleaner::cleanup_probe_session_artifacts(
        &probe_dir,
        &probe_dir,
        &env1,
        temp.path(),
        temp.path(),
    );
    assert_eq!(cleaned, vec![stale_jsonl.clone()]);
    assert!(!stale_jsonl.exists());
    assert!(hidden_jsonl.exists(), "dotfiles must not be removed");
    assert!(keep_txt.exists());
    #[cfg(unix)]
    assert!(
        symlink_jsonl.symlink_metadata().is_ok(),
        "symlinks must not be removed"
    );

    // Refuse cleanup when probe_dir is outside dedicated_probe_root
    let other_dir = temp.path().join("outside-probe");
    fs::create_dir_all(&other_dir).unwrap();
    assert!(
        ClaudeProbeSessionArtifactCleaner::cleanup_probe_session_artifacts(
            &other_dir,
            &probe_dir,
            &env1,
            temp.path(),
            temp.path(),
        )
        .is_empty()
    );
}

#[test]
fn auth_status_probe_and_rate_limit_gate_behave_as_specified() {
    assert_eq!(
        ClaudeCLIAuthStatusProbe::parse_status(br#"{"loggedIn":true}"#),
        Some(ClaudeCLIAuthenticationStatus::LoggedIn)
    );
    assert_eq!(
        ClaudeCLIAuthStatusProbe::parse_status(br#"{"loggedIn":false}"#),
        Some(ClaudeCLIAuthenticationStatus::LoggedOut)
    );
    assert_eq!(
        ClaudeCLIAuthStatusProbe::evaluate_subprocess_result(None),
        ClaudeCLIAuthenticationStatus::Unavailable
    );
    assert_eq!(
        ClaudeCLIAuthStatusProbe::evaluate_subprocess_result(Some(&ClaudeCLISubprocessResult {
            exit_code: Some(1),
            stdout: b"not json".to_vec(),
            stderr: Vec::new(),
        })),
        ClaudeCLIAuthenticationStatus::Unavailable
    );

    let gate = ClaudeCLIRateLimitGate::new();
    let now = Timestamp::from_second(1_766_500_000).unwrap();
    assert!(
        gate.blocked_until(FetchInteraction::Background, now)
            .is_none()
    );

    gate.record_rate_limit(now);
    assert!(
        gate.blocked_until(FetchInteraction::Background, now)
            .is_some()
    );
    assert!(
        gate.blocked_until(FetchInteraction::UserInitiated, now)
            .is_none(),
        "user-initiated refresh bypasses CLI rate limit gate"
    );

    let after_cooldown = now
        .checked_add(jiff::SignedDuration::from_secs(5 * 60 + 1))
        .unwrap();
    assert!(
        gate.blocked_until(FetchInteraction::Background, after_cooldown)
            .is_none()
    );

    gate.record_rate_limit(now);
    gate.record_success();
    assert!(
        gate.blocked_until(FetchInteraction::Background, now)
            .is_none()
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn pty_session_reuses_account_scope_restarts_on_profile_change_and_cleans_transcripts() {
    let temp = tempfile::tempdir().unwrap();
    let log_path = temp.path().join("launches.log");
    let cli_path = temp.path().join("claude");
    let script = format!(
        "#!/bin/sh\nprintf 'start:%s:%s:%s\\n' \"$CLAUDE_CONFIG_DIR\" \"$CLAUDE_SECURESTORAGE_CONFIG_DIR\" \"$HOME\" >> '{}'\nwhile IFS= read -r line; do\n  case \"$line\" in\n    *\"/status\"*)\n      printf 'Account: %s\\n' \"$CLAUDE_CONFIG_DIR\"\n      ;;\n  esac\ndone\n",
        log_path.display()
    );
    fs::write(&cli_path, script).unwrap();
    fs::set_permissions(&cli_path, fs::Permissions::from_mode(0o755)).unwrap();

    let dedicated_probe_dir = temp.path().join("dedicated-probe");
    let registry = Arc::new(ProcessRegistry::new());
    let launch_gate = Arc::new(LaunchGate::new(Instant::now));
    let session = ClaudeCLISession::new(dedicated_probe_dir.clone(), registry, launch_gate)
        .with_startup_ready_delay(Duration::from_millis(50));

    let first_config = temp.path().join("profile-a");
    let first_secure = temp.path().join("secure-a");
    let first_home = temp.path().join("home-a");
    let second_config = temp.path().join("profile-b");
    let second_secure = temp.path().join("secure-b");
    let second_home = temp.path().join("home-b");

    let proj_name = ClaudeProbeSessionArtifactCleaner::sanitized_project_directory_name(
        &dedicated_probe_dir.to_string_lossy(),
    );
    let stale_1 = first_config
        .join("projects")
        .join(&proj_name)
        .join("stale.jsonl");
    let stale_2 = second_config
        .join("projects")
        .join(&proj_name)
        .join("stale.jsonl");
    fs::create_dir_all(stale_1.parent().unwrap()).unwrap();
    fs::create_dir_all(stale_2.parent().unwrap()).unwrap();
    fs::write(&stale_1, "{}\n").unwrap();
    fs::write(&stale_2, "{}\n").unwrap();

    let mut env1 = BTreeMap::new();
    env1.insert(
        CLAUDE_CONFIG_DIR_ENV_KEY.to_owned(),
        SecretString::new(first_config.to_string_lossy().into_owned()),
    );
    env1.insert(
        CLAUDE_SECURE_STORAGE_CONFIG_DIR_ENV_KEY.to_owned(),
        SecretString::new(first_secure.to_string_lossy().into_owned()),
    );
    env1.insert(
        "HOME".to_owned(),
        SecretString::new(first_home.to_string_lossy().into_owned()),
    );

    let mut env2 = BTreeMap::new();
    env2.insert(
        CLAUDE_CONFIG_DIR_ENV_KEY.to_owned(),
        SecretString::new(second_config.to_string_lossy().into_owned()),
    );
    env2.insert(
        CLAUDE_SECURE_STORAGE_CONFIG_DIR_ENV_KEY.to_owned(),
        SecretString::new(second_secure.to_string_lossy().into_owned()),
    );
    env2.insert(
        "HOME".to_owned(),
        SecretString::new(second_home.to_string_lossy().into_owned()),
    );

    let capture_for = |env: BTreeMap<String, SecretString>, scope: &str, expected_dir: &Path| {
        let req = ClaudeCLICaptureRequest {
            subcommand: "/status".to_owned(),
            binary: cli_path.clone(),
            account_scope: Some(scope.to_owned()),
            timeout: Duration::from_secs(3),
            environment: env,
            idle_timeout: None,
            stop_on_substrings: vec![format!("Account: {}", expected_dir.display())],
            stop_when_normalized: None,
            settle_after_stop: Duration::ZERO,
            send_enter_every: None,
        };
        session.capture(make_call(FetchInteraction::UserInitiated), req)
    };

    let out1 = capture_for(env1, "scope-a", &first_config).await.unwrap();
    let out2 = capture_for(env2.clone(), "scope-b", &second_config)
        .await
        .unwrap();
    let out3 = capture_for(env2, "scope-b", &second_config).await.unwrap();
    session.reset().await;

    assert!(out1.contains(&format!("Account: {}", first_config.display())));
    assert!(out2.contains(&format!("Account: {}", second_config.display())));
    assert!(out3.contains(&format!("Account: {}", second_config.display())));

    let launches: Vec<String> = fs::read_to_string(&log_path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        launches,
        vec![
            format!(
                "start:{}:{}:{}",
                first_config.display(),
                first_secure.display(),
                first_home.display()
            ),
            format!(
                "start:{}:{}:{}",
                second_config.display(),
                second_secure.display(),
                second_home.display()
            ),
        ]
    );
    assert!(!stale_1.exists());
    assert!(!stale_2.exists());
}

#[tokio::test]
async fn direct_cli_fallback_recovers_after_pty_timeout_and_records_rate_limits() {
    let temp = tempfile::tempdir().unwrap();
    let marker_file = temp.path().join("unexpected-spawn.log");
    let cli_path = temp.path().join("claude");
    let script = format!(
        concat!(
            "#!/bin/sh\n",
            "printf 'spawned\\n' >> '{}'\n",
            "for arg in \"$@\"; do\n",
            "  if [ \"$arg\" = \"/usage\" ]; then\n",
            "    printf 'Current session\\n85%% left\\nResets in 2h\\nCurrent week (all models)\\n70%% left\\n'\n",
            "    exit 0\n",
            "  fi\n",
            "done\n",
            "exit 1\n"
        ),
        marker_file.display()
    );
    fs::write(&cli_path, script).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&cli_path, fs::Permissions::from_mode(0o755)).unwrap();

    let dedicated_probe_dir = temp.path().join("dedicated-probe");
    let registry = Arc::new(ProcessRegistry::new());
    let launch_gate = Arc::new(LaunchGate::new(Instant::now));
    let session = Arc::new(ClaudeCLISession::new(
        dedicated_probe_dir,
        Arc::clone(&registry),
        Arc::clone(&launch_gate),
    ));

    // 1. When `fetch_override` is set and `direct_fetch_override` is NOT set, `load_once` and
    // `load_with_retry` must NEVER resolve or spawn the `claude` binary even if it is executable!
    let pty_only_attempts = Arc::new(AtomicUsize::new(0));
    let pty_only_attempts_clone = Arc::clone(&pty_only_attempts);
    let probe_no_direct = ClaudeStatusProbe::new(
        BTreeMap::new(),
        temp.path().to_path_buf(),
        Arc::clone(&session),
    )
    .with_binary(cli_path.to_string_lossy().into_owned())
    .with_fetch_override(Arc::new(move |_, _, _| {
        pty_only_attempts_clone.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(ClaudeStatusProbeError::TimedOut) })
    }));
    let loader_no_direct = ClaudeCLIUsageLoader {
        probe: probe_no_direct,
        rate_limit_gate: Arc::new(ClaudeCLIRateLimitGate::new()),
        registry: Arc::clone(&registry),
        launch_gate: Arc::clone(&launch_gate),
    };
    let now = Timestamp::from_second(1_766_500_000).unwrap();
    let err = loader_no_direct
        .load_with_retry(
            make_call(FetchInteraction::UserInitiated),
            Duration::from_secs(12),
            now,
        )
        .await
        .unwrap_err();
    assert_eq!(err, ClaudeStatusProbeError::TimedOut);
    assert_eq!(pty_only_attempts.load(Ordering::SeqCst), 2);
    assert!(
        !marker_file.exists(),
        "fetch_override without direct_fetch_override must never spawn a subprocess"
    );

    // 2. When `direct_fetch_override` is configured, `load_once` falls back on TimedOut and
    // "could not load usage data", skips fallback on non-fallback parse errors, and records rate limits.
    let pty_attempts = Arc::new(AtomicUsize::new(0));
    let pty_attempts_clone = Arc::clone(&pty_attempts);
    let direct_attempts = Arc::new(AtomicUsize::new(0));
    let direct_attempts_clone = Arc::clone(&direct_attempts);
    let probe_with_direct = ClaudeStatusProbe::new(
        BTreeMap::new(),
        temp.path().to_path_buf(),
        Arc::clone(&session),
    )
    .with_binary("/nonexistent/claude")
    .with_fetch_override(Arc::new(move |_, _, _| {
        pty_attempts_clone.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(ClaudeStatusProbeError::TimedOut) })
    }))
    .with_direct_fetch_override(Arc::new(move |_, _, _| {
        direct_attempts_clone.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(ClaudeStatusSnapshot {
                session_percent_left: Some(85),
                weekly_percent_left: Some(70),
                opus_percent_left: None,
                extra_rate_windows: Vec::new(),
                provider_cost: None,
                account_email: None,
                account_organization: None,
                login_method: None,
                primary_reset_description: Some("Resets in 2h".to_owned()),
                secondary_reset_description: None,
                opus_reset_description: None,
                raw_text: "direct fallback".to_owned(),
            })
        })
    }));

    let rate_limit_gate = Arc::new(ClaudeCLIRateLimitGate::new());
    let loader = ClaudeCLIUsageLoader {
        probe: probe_with_direct,
        rate_limit_gate: Arc::clone(&rate_limit_gate),
        registry: Arc::clone(&registry),
        launch_gate: Arc::clone(&launch_gate),
    };

    let snap = loader
        .load_once(
            make_call(FetchInteraction::UserInitiated),
            Duration::from_secs(12),
            now,
        )
        .await
        .expect("direct CLI fallback should succeed via direct_fetch_override");
    assert_eq!(pty_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(direct_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(snap.session_percent_left, Some(85));
    assert_eq!(snap.weekly_percent_left, Some(70));
    assert!(!marker_file.exists());

    // Direct CLI rate-limit error records cooldown in `rate_limit_gate`
    let probe_direct_rl = ClaudeStatusProbe::new(
        BTreeMap::new(),
        temp.path().to_path_buf(),
        Arc::clone(&session),
    )
    .with_binary("/nonexistent/claude")
    .with_fetch_override(Arc::new(|_, _, _| {
        Box::pin(async {
            Err(ClaudeStatusProbeError::ParseFailed(
                "Claude CLI could not load usage data. Open the CLI and retry `/usage`.".to_owned(),
            ))
        })
    }))
    .with_direct_fetch_override(Arc::new(|_, _, _| {
        Box::pin(async {
            Err(ClaudeStatusProbeError::ParseFailed(
                ClaudeCLIRateLimitGate::MESSAGE.to_owned(),
            ))
        })
    }));
    let loader_rl = ClaudeCLIUsageLoader {
        probe: probe_direct_rl,
        rate_limit_gate: Arc::clone(&rate_limit_gate),
        registry: Arc::clone(&registry),
        launch_gate: Arc::clone(&launch_gate),
    };
    let rl_err = loader_rl
        .load_once(
            make_call(FetchInteraction::Background),
            Duration::from_secs(12),
            now,
        )
        .await
        .unwrap_err();
    assert!(rl_err.is_rate_limit_error());
    assert!(
        rate_limit_gate
            .blocked_until(FetchInteraction::Background, now)
            .is_some()
    );

    // 3. `fetch_direct_usage_snapshot` executes the synthetic `cli_path` script when invoked directly on Unix
    #[cfg(unix)]
    {
        let probe_subproc =
            ClaudeStatusProbe::new(BTreeMap::new(), temp.path().to_path_buf(), session)
                .with_binary(cli_path.to_string_lossy().into_owned());
        let loader_subproc = ClaudeCLIUsageLoader {
            probe: probe_subproc,
            rate_limit_gate: Arc::new(ClaudeCLIRateLimitGate::new()),
            registry,
            launch_gate,
        };
        let subproc_snap = loader_subproc
            .fetch_direct_usage_snapshot(
                make_call(FetchInteraction::UserInitiated),
                Duration::from_secs(5),
            )
            .await
            .expect("synthetic script should parse via fetch_direct_usage_snapshot");
        assert_eq!(subproc_snap.session_percent_left, Some(85));
        assert_eq!(subproc_snap.weekly_percent_left, Some(70));
        assert!(marker_file.exists());
    }
}

#[tokio::test]
async fn spawn_throttle_background_availability_and_reset_boundary_semantics() {
    let temp = tempfile::tempdir().unwrap();
    let config_dir = temp.path().join("claude-profile");
    fs::create_dir_all(&config_dir).unwrap();
    let config_file = config_dir.join(".config.json");
    fs::write(
        &config_file,
        r#"{"oauthAccount":{"accountUuid":"account-a"}}"#,
    )
    .unwrap();

    let mut env = BTreeMap::new();
    env.insert(
        CLAUDE_CONFIG_DIR_ENV_KEY.to_owned(),
        SecretString::new(config_dir.to_string_lossy().into_owned()),
    );

    let clock_secs = Arc::new(AtomicI64::new(1_766_500_000));
    let clock_for_cfg = Arc::clone(&clock_secs);
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_for_probe = Arc::clone(&attempts);

    let net = Arc::new(https::broker(NetConfig::default()));
    let cfg = ClaudeOAuthStrategyConfig::new(net, temp.path(), temp.path())
        .with_has_cli(true)
        .with_clock(move || Timestamp::from_second(clock_for_cfg.load(Ordering::SeqCst)).unwrap())
        .with_cli_status_probe_override(Arc::new(move |_, _, _| {
            let attempt = attempts_for_probe.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                Ok(ClaudeStatusSnapshot {
                    session_percent_left: Some(90 - i32::try_from(attempt).unwrap()),
                    weekly_percent_left: Some(80 - i32::try_from(attempt).unwrap()),
                    opus_percent_left: None,
                    extra_rate_windows: Vec::new(),
                    provider_cost: None,
                    account_email: Some("cli@example.com".to_owned()),
                    account_organization: Some("CLI Org".to_owned()),
                    login_method: Some("Claude Max".to_owned()),
                    primary_reset_description: None,
                    secondary_reset_description: None,
                    opus_reset_description: None,
                    raw_text: "probe raw".to_owned(),
                })
            })
        }));

    let strategy = ClaudeCLIFetchStrategy::new(cfg.clone());
    let bg_cx = make_context(
        FetchRuntime::Daemon,
        SourceMode::Cli,
        FetchInteraction::Background,
        env.clone(),
    );

    // 1. Background refresh reuses cached CLI result within 15-minute spawn floor
    let first = strategy.fetch(&bg_cx).await.unwrap();
    let second = strategy.fetch(&bg_cx).await.unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        first.usage.primary.as_ref().unwrap().used_percent,
        second.usage.primary.as_ref().unwrap().used_percent
    );
    assert_eq!(first.usage.data_confidence, Confidence::PercentOnly);
    assert_eq!(first.source_label, "claude");
    assert_eq!(first.strategy_id, CLAUDE_CLI_STRATEGY_ID);

    // 2. User-initiated refresh always spawns the CLI and establishes background availability
    let user_cx = make_context(
        FetchRuntime::Daemon,
        SourceMode::Cli,
        FetchInteraction::UserInitiated,
        env.clone(),
    );
    let third = strategy.fetch(&user_cx).await.unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_ne!(
        first.usage.primary.as_ref().unwrap().used_percent,
        third.usage.primary.as_ref().unwrap().used_percent
    );

    // 3. Advance clock past 15-minute spawn floor -> background refresh respawns CLI
    clock_secs.fetch_add(15 * 60 + 1, Ordering::SeqCst);
    let fourth = strategy.fetch(&bg_cx).await.unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_ne!(
        third.usage.primary.as_ref().unwrap().used_percent,
        fourth.usage.primary.as_ref().unwrap().used_percent
    );

    // 4. Account profile change invalidates cached CLI result
    fs::write(
        &config_file,
        r#"{"oauthAccount":{"accountUuid":"account-b"}}"#,
    )
    .unwrap();
    let _fifth = strategy.fetch(&bg_cx).await.unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 4);

    // 5. Owner CLI recovery pass bypasses cached result
    let recovery_strategy =
        ClaudeCLIFetchStrategy::new(cfg.clone()).with_owner_cli_recovery_only(true);
    let _sixth = recovery_strategy.fetch(&bg_cx).await.unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 5);

    // 6. Session reset boundary invalidates cached CLI result before spawn floor elapses
    let throttle = ClaudeCLIUsageSpawnThrottle::new();
    let base_now = Timestamp::from_second(1_766_500_000).unwrap();
    let resets_soon = base_now
        .checked_add(jiff::SignedDuration::from_secs(120))
        .unwrap();
    let primary = RateWindow::new(40.0, Some(300), Some(resets_soon), None, None, false).unwrap();
    let usage = UsageSnapshot::new(
        Some(primary),
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        base_now,
        None,
        Confidence::PercentOnly,
    )
    .unwrap();
    let cached_res = FetchResult::new(usage, "claude", CLAUDE_CLI_STRATEGY_ID, SourceKind::Cli);
    let key = ClaudeCLIUsageSpawnThrottleKey {
        binary: "/bin/echo".to_owned(),
        account_scope: "scope-a".to_owned(),
        use_web_extras: false,
        include_prepaid_balance: false,
    };
    throttle.record(cached_res, key.clone(), base_now);
    assert!(
        throttle
            .cached_result(
                &key,
                base_now
                    .checked_add(jiff::SignedDuration::from_secs(60))
                    .unwrap()
            )
            .is_some()
    );
    assert!(
        throttle
            .cached_result(
                &key,
                base_now
                    .checked_add(jiff::SignedDuration::from_secs(300))
                    .unwrap()
            )
            .is_none(),
        "reset boundary must invalidate cached CLI usage result"
    );
}

#[tokio::test]
async fn claude_provider_plan_executes_cli_and_auto_fallback_and_enriches_web_extras() {
    let temp = tempfile::tempdir().unwrap();
    let server = https::start_server();
    let base_url = Url::parse(&server.url("")).unwrap();
    let oauth_ep = ClaudeOAuthEndpointConfig::with_base_url(base_url.clone()).unwrap();
    let web_ep = ClaudeWebEndpointConfig::with_base_url(base_url.join("/api").unwrap()).unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));
    let provider_id = ProviderId::new("claude").unwrap();

    let orgs_mock = server.mock(|when, then| {
        when.method(GET).path("/api/organizations");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"[{"uuid":"org-123","name":"CLI Org","capabilities":["chat"]}]"#);
    });
    let usage_mock = server.mock(|when, then| {
        when.method(GET).path("/api/organizations/org-123/usage");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{
                    "five_hour": { "utilization": 15, "resets_at": "2025-12-23T16:00:00.000Z" },
                    "seven_day": { "utilization": 30, "resets_at": "2025-12-29T23:00:00.000Z" }
                }"#,
            );
    });
    let account_mock = server.mock(|when, then| {
        when.method(GET).path("/api/account");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{
                    "email_address": "cli@example.com",
                    "memberships": [{
                        "organization": {
                            "uuid": "org-123",
                            "name": "CLI Org",
                            "rate_limit_tier": "claude_max",
                            "billing_type": "stripe"
                        }
                    }]
                }"#,
            );
    });
    let overage_mock = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/overage_spend_limit");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{"monthly_credit_limit":5000,"currency":"USD","used_credits":1250,"is_enabled":true}"#,
            );
    });
    let _prepaid_mock = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/prepaid/credits");
        then.status(404).body("{}");
    });

    let cfg = ClaudeOAuthStrategyConfig::new(net, temp.path(), temp.path())
        .with_endpoint(oauth_ep)
        .with_web_endpoint(web_ep)
        .with_has_cli(true)
        .with_web_extras_enabled(true)
        .with_cookie_source(Some(CookieSource::Manual))
        .with_manual_cookie_header(Some(SecretString::new(format!(
            "{SESSION_KEY_COOKIE_NAME}=sk-ant-web-cli-test"
        ))))
        .with_cli_status_probe_override(Arc::new(|_, _, _| {
            Box::pin(async {
                Ok(ClaudeStatusSnapshot {
                    session_percent_left: Some(85),
                    weekly_percent_left: Some(70),
                    opus_percent_left: Some(60),
                    extra_rate_windows: Vec::new(),
                    provider_cost: None,
                    account_email: Some("cli@example.com".to_owned()),
                    account_organization: Some("CLI Org".to_owned()),
                    login_method: Some("Claude Max".to_owned()),
                    primary_reset_description: Some("Resets in 2h".to_owned()),
                    secondary_reset_description: Some("Resets in 3d".to_owned()),
                    opus_reset_description: None,
                    raw_text: "Current session\n85% left".to_owned(),
                })
            })
        }));

    let plan = ClaudeProviderPlan::new(cfg);

    // Daemon Auto without OAuth credentials falls back from OAuth to CLI and enriches with web extras!
    let auto_cx = make_context(
        FetchRuntime::Daemon,
        SourceMode::Auto,
        FetchInteraction::UserInitiated,
        BTreeMap::new(),
    );
    let outcome = fetch_outcome(&provider_id, &plan, &auto_cx, &NoopSleeper).await;
    let result = outcome.result.expect("Daemon Auto should succeed via CLI");
    assert_eq!(result.strategy_id, CLAUDE_CLI_STRATEGY_ID);
    assert_eq!(result.source_label, "claude");
    assert_eq!(result.usage.data_confidence, Confidence::PercentOnly);
    assert_eq!(result.usage.primary.as_ref().unwrap().used_percent, 15.0);
    assert_eq!(result.usage.secondary.as_ref().unwrap().used_percent, 30.0);
    assert_eq!(result.usage.tertiary.as_ref().unwrap().used_percent, 40.0);
    let cost = result
        .usage
        .provider_cost
        .as_ref()
        .expect("web extras should enrich provider_cost");
    assert_eq!(cost.used, 12.5);
    assert_eq!(cost.limit, 50.0);

    assert_eq!(outcome.attempts.len(), 2);
    assert_eq!(outcome.attempts[0].strategy_id, CLAUDE_OAUTH_STRATEGY_ID);
    assert_eq!(outcome.attempts[1].strategy_id, CLAUDE_CLI_STRATEGY_ID);
    assert_eq!(outcome.attempts[1].outcome(), AttemptOutcome::Succeeded);

    orgs_mock.assert();
    usage_mock.assert();
    account_mock.assert();
    overage_mock.assert();
}

#[tokio::test]
async fn cli_runtime_auto_and_logged_out_guard_and_subscription_notice_no_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));
    let provider_id = ProviderId::new("claude").unwrap();

    // 1. In CLI runtime, if `claude auth status --json` reports LoggedOut, `claude.cli` is unavailable.
    let cfg_logged_out = ClaudeOAuthStrategyConfig::new(Arc::clone(&net), temp.path(), temp.path())
        .with_has_cli(true)
        .with_cookie_source(Some(CookieSource::Off))
        .with_cli_auth_status_override(|_, _| {
            Box::pin(async { ClaudeCLIAuthenticationStatus::LoggedOut })
        })
        .with_cli_status_probe_override(Arc::new(|_, _, _| {
            Box::pin(async {
                panic!("probe should not run when CLI auth status is LoggedOut");
            })
        }));
    let plan_logged_out = ClaudeProviderPlan::new(cfg_logged_out);
    let cli_cx = make_context(
        FetchRuntime::Cli,
        SourceMode::Auto,
        FetchInteraction::UserInitiated,
        BTreeMap::new(),
    );
    let outcome_logged_out =
        fetch_outcome(&provider_id, &plan_logged_out, &cli_cx, &NoopSleeper).await;
    assert!(matches!(
        outcome_logged_out.result,
        Err(FetchError::NoAvailableStrategy(_))
    ));

    // 2. In Daemon Auto, subscription notice without session quota data does NOT fall back to web.
    let cfg_sub_notice = ClaudeOAuthStrategyConfig::new(net, temp.path(), temp.path())
        .with_has_cli(true)
        .with_cookie_source(Some(CookieSource::Manual))
        .with_manual_cookie_header(Some(SecretString::new(format!(
            "{SESSION_KEY_COOKIE_NAME}=sk-ant-web-sub-notice"
        ))))
        .with_cli_status_probe_override(Arc::new(|_, _, _| {
            Box::pin(async {
                Err(ClaudeStatusProbeError::ParseFailed(
                    SUBSCRIPTION_QUOTA_UNAVAILABLE_DESCRIPTION.to_owned(),
                ))
            })
        }));
    let plan_sub_notice = ClaudeProviderPlan::new(cfg_sub_notice);
    let daemon_auto_cx = make_context(
        FetchRuntime::Daemon,
        SourceMode::Auto,
        FetchInteraction::UserInitiated,
        BTreeMap::new(),
    );
    let outcome_sub = fetch_outcome(
        &provider_id,
        &plan_sub_notice,
        &daemon_auto_cx,
        &NoopSleeper,
    )
    .await;
    let err = outcome_sub.result.unwrap_err();
    match err {
        FetchError::Classified(classified) => {
            assert_eq!(classified.kind, ProviderErrorKind::ParseFailure);
            assert!(
                ClaudeStatusProbeParser::is_subscription_quota_unavailable_description(Some(
                    &classified.message
                ))
            );
        }
        other => panic!("expected Classified error, got {other:?}"),
    }
    // Web strategy should not have been attempted because CLI subscription notice is terminal
    assert!(
        outcome_sub
            .attempts
            .iter()
            .all(|a| a.strategy_id != CLAUDE_WEB_STRATEGY_ID)
    );
}

#[test]
fn all_models_weekly_duplicate_and_scoped_weekly_rows() {
    // 1. Garbled all-models copy does not duplicate the weekly row
    let sample1 = concat!(
        "Settings:  Status   Config   Usage  (tab to cycle)\n\n",
        " Current session\n",
        " ▌                                                  0% used\n",
        " Resets 1:10pm (Asia/Seoul)\n\n",
        " Current week (all models)\n",
        " █████████████████████████████████▌                66% used\n",
        " Resets Jul 24 at 2pm (Asia/Seoul)\n\n",
        " Current week (all modls)\n",
        " █████████████████████████████████▌                67% used\n",
        " Resets Jul 24 at 1:59pm (Asia/Seoul)\n\n",
        " Current week (Fable)\n",
        " ████████████████████████████████████              71% used\n",
        " Resets Jul 24 at 2pm (Asia/Seoul)\n"
    );
    let snap1 = ClaudeStatusProbeParser::parse(sample1, None).unwrap();
    assert_eq!(snap1.weekly_percent_left, Some(34));
    let titles1: Vec<&str> = snap1
        .extra_rate_windows
        .iter()
        .map(|w| w.title.as_str())
        .collect();
    assert_eq!(titles1, vec!["Fable only"]);

    // 2. Exact all-models row wins over an earlier garbled duplicate
    let sample2 = concat!(
        "Settings:  Status   Config   Usage  (tab to cycle)\n\n",
        " Current session\n",
        " ▌                                                  0% used\n",
        " Resets 1:10pm (Asia/Seoul)\n\n",
        " Current week (all modls)\n",
        " █████████████████████████████████▌                67% used\n",
        " Resets Jul 24 at 1:59pm (Asia/Seoul)\n\n",
        " Current week (all models)\n",
        " █████████████████████████████████▌                66% used\n",
        " Resets Jul 24 at 2pm (Asia/Seoul)\n"
    );
    let snap2 = ClaudeStatusProbeParser::parse(sample2, None).unwrap();
    assert_eq!(snap2.weekly_percent_left, Some(34));
    assert_eq!(
        snap2.secondary_reset_description.as_deref(),
        Some("Resets Jul 24 at 2pm (Asia/Seoul)")
    );
    assert!(snap2.extra_rate_windows.is_empty());

    // 3. Garbled all-models line still populates the weekly limit when alone
    let sample3 = concat!(
        " Current session\n",
        " ▌                                                  0% used\n",
        " Resets 1:10pm (Asia/Seoul)\n\n",
        " Current week (all modls)\n",
        " █████████████████████████████████▌                66% used\n",
        " Resets Jul 24 at 2pm (Asia/Seoul)\n\n",
        " Current week (Fable)\n",
        " ████████████████████████████████████              71% used\n",
        " Resets Jul 24 at 2pm (Asia/Seoul)\n"
    );
    let snap3 = ClaudeStatusProbeParser::parse(sample3, None).unwrap();
    assert_eq!(snap3.weekly_percent_left, Some(34));
    assert_eq!(
        snap3.secondary_reset_description.as_deref(),
        Some("Resets Jul 24 at 2pm (Asia/Seoul)")
    );
    let titles3: Vec<&str> = snap3
        .extra_rate_windows
        .iter()
        .map(|w| w.title.as_str())
        .collect();
    assert_eq!(titles3, vec!["Fable only"]);

    // 4. Single-character insertion ("all modelss") is treated as all-models
    let sample4 = concat!(
        " Current session\n",
        " ▌                                                  0% used\n\n",
        " Current week (all modelss)\n",
        " █████████████████████████████████▌                66% used\n\n",
        " Current week (Fable)\n",
        " ████████████████████████████████████              71% used\n"
    );
    let snap4 = ClaudeStatusProbeParser::parse(sample4, None).unwrap();
    assert_eq!(snap4.weekly_percent_left, Some(34));
    let titles4: Vec<&str> = snap4
        .extra_rate_windows
        .iter()
        .map(|w| w.title.as_str())
        .collect();
    assert_eq!(titles4, vec!["Fable only"]);

    // 5. Genuine scoped model ("Haiku") is preserved and leaves weekly limit unset
    let sample5 = concat!(
        " Current session\n",
        " ▌                                                  0% used\n\n",
        " Current week (Haiku)\n",
        " █▌                                                 10% used\n"
    );
    let snap5 = ClaudeStatusProbeParser::parse(sample5, None).unwrap();
    assert_eq!(snap5.weekly_percent_left, None);
    let titles5: Vec<&str> = snap5
        .extra_rate_windows
        .iter()
        .map(|w| w.title.as_str())
        .collect();
    assert_eq!(titles5, vec!["Haiku only"]);

    // 6. Heavier corruption ("almdls", edit distance 3) stays a visible scoped row
    let sample6 = concat!(
        " Current session\n",
        " ▌                                                  0% used\n\n",
        " Current week (almdls)\n",
        " █████████████████████████████████▌                66% used\n"
    );
    let snap6 = ClaudeStatusProbeParser::parse(sample6, None).unwrap();
    assert_eq!(snap6.weekly_percent_left, None);
    let titles6: Vec<&str> = snap6
        .extra_rate_windows
        .iter()
        .map(|w| w.title.as_str())
        .collect();
    assert_eq!(titles6, vec!["almdls only"]);
}

#[test]
fn redirected_probe_paths_are_never_trusted_or_prepared() {
    let temp = tempfile::tempdir().unwrap();
    let dedicated = temp.path().join("ariadusage").join("ClaudeProbe");
    assert_eq!(probe_working_directory(&dedicated), dedicated);
    assert!(is_dedicated_probe_working_directory(&dedicated, &dedicated));
    assert!(!is_dedicated_probe_working_directory(
        temp.path(),
        &dedicated
    ));

    let unavailable = Path::new("/dev/null/ariadusage/ClaudeProbe");
    let fallback = probe_working_directory(unavailable);
    assert_eq!(fallback, std::env::temp_dir());
    assert!(!is_dedicated_probe_working_directory(
        &fallback,
        unavailable
    ));
    assert!(isolated_probe_working_directory(unavailable).is_err());

    #[cfg(unix)]
    for redirect_parent in [false, true] {
        let root = temp.path().join(format!("redirect-{redirect_parent}"));
        let project = root.join("user-project");
        let parent = root.join("ariadusage");
        let probe = parent.join("ClaudeProbe");
        fs::create_dir_all(&project).unwrap();
        if redirect_parent {
            fs::create_dir_all(project.join("ClaudeProbe")).unwrap();
            std::os::unix::fs::symlink(&project, &parent).unwrap();
        } else {
            fs::create_dir_all(&parent).unwrap();
            std::os::unix::fs::symlink(&project, &probe).unwrap();
        }
        assert!(!is_dedicated_probe_working_directory(&probe, &probe));
        let prepared = prepared_probe_working_directory(&probe);
        assert_eq!(prepared, std::env::temp_dir());
        let target = if redirect_parent {
            project.join("ClaudeProbe")
        } else {
            project
        };
        assert!(!target.join(".claude").exists());
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn panel_transition_and_workspace_trust_in_pty() {
    let temp = tempfile::tempdir().unwrap();

    // 1. Reused session dismisses panels before capturing identity and refreshing usage
    let panel_dir = temp.path().join("panel-transition");
    fs::create_dir_all(&panel_dir).unwrap();
    let binary = panel_dir.join("fake-claude");
    let log_path = panel_dir.join("commands.log");
    let script = r#"#!/bin/bash
/bin/stty raw -echo -icrnl
printf 'launch\n' >> "$HOME/commands.log"
panel=closed
command=''
while IFS= read -r -n 1 key; do
  case "$key" in
    $'\e')
      printf 'dismiss:%s\n' "$panel" >> "$HOME/commands.log"
      /bin/sleep 0.05
      panel=closed
      command=''
      printf 'prompt\r\n'
      ;;
    $'\r'|'')
      if [[ "$panel" == closed ]]; then
        printf '%s\n' "$command" >> "$HOME/commands.log"
        case "$command" in
          /usage)
            printf 'Current session\r\n9%% used\r\nDONE\r\n'
            panel=usage
            ;;
          /status)
            printf 'Email: fixture@example.com\r\nLogin method: Claude Max Account\r\nDONE\r\n'
            panel=status
            ;;
        esac
      fi
      command=''
      ;;
    *) command+="$key" ;;
  esac
done
"#;
    fs::write(&binary, script).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();

    let registry = Arc::new(ProcessRegistry::new());
    let launch_gate = Arc::new(LaunchGate::new(Instant::now));
    let session = ClaudeCLISession::new(
        panel_dir.join("ariadusage").join("ClaudeProbe"),
        Arc::clone(&registry),
        Arc::clone(&launch_gate),
    )
    .with_working_directory(panel_dir.clone())
    .with_startup_ready_delay(Duration::from_millis(50));

    let mut env = BTreeMap::new();
    env.insert(
        "HOME".to_owned(),
        SecretString::new(panel_dir.to_string_lossy().into_owned()),
    );
    env.insert(
        "CLAUDE_CONFIG_DIR".to_owned(),
        SecretString::new(panel_dir.to_string_lossy().into_owned()),
    );
    env.insert(
        "CLAUDE_SECURESTORAGE_CONFIG_DIR".to_owned(),
        SecretString::new(panel_dir.to_string_lossy().into_owned()),
    );

    let capture_cmd = |cmd: &str| {
        let req = ClaudeCLICaptureRequest {
            subcommand: cmd.to_owned(),
            binary: binary.clone(),
            account_scope: Some("synthetic-account".to_owned()),
            timeout: Duration::from_secs(3),
            environment: env.clone(),
            idle_timeout: Some(Duration::from_millis(500)),
            stop_on_substrings: vec!["DONE".to_owned()],
            stop_when_normalized: None,
            settle_after_stop: Duration::ZERO,
            send_enter_every: None,
        };
        session.capture(make_call(FetchInteraction::UserInitiated), req)
    };

    let usage_out = capture_cmd("/usage").await.unwrap();
    let status_out = capture_cmd("/status").await.unwrap();
    let snapshot = ClaudeStatusProbeParser::parse(&usage_out, Some(&status_out)).unwrap();
    assert_eq!(snapshot.session_percent_left, Some(91));
    assert_eq!(
        snapshot.account_email.as_deref(),
        Some("fixture@example.com")
    );
    assert_eq!(snapshot.login_method.as_deref(), Some("Max"));
    let refreshed = capture_cmd("/usage").await.unwrap();
    assert_eq!(
        ClaudeStatusProbeParser::parse(&refreshed, None)
            .unwrap()
            .session_percent_left,
        Some(91)
    );
    session.reset().await;
    assert_eq!(
        fs::read_to_string(&log_path).unwrap(),
        "launch\n/usage\ndismiss:usage\n/status\ndismiss:status\n/usage\n"
    );

    // 2. Workspace trust dialog in PTY: accepted inside dedicated probe dir, cancelled outside
    let trust_dir = temp.path().join("workspace-trust");
    fs::create_dir_all(&trust_dir).unwrap();
    for name in [
        "workspace-trust-dialog.ansi",
        "workspace-trust-select-down.ansi",
    ] {
        fs::copy(fixture_path(name), trust_dir.join(name)).unwrap();
    }
    let trust_binary = trust_dir.join("fake-claude");
    let trust_script = r#"#!/bin/bash
/bin/stty raw -echo -icrnl
/bin/cat "$HOME/workspace-trust-dialog.ansi"
selected=no
while IFS= read -r -n 1 key; do
  case "$key" in
    $'\e')
      key=''
      IFS= read -r -t 1 -n 2 key
      if [[ -z "$key" ]]; then
        printf 'cancel\n' >> "$HOME/keys.log"
        exit 0
      fi
      printf 'key:%s\n' "$key" >> "$HOME/keys.log"
      if [[ "$key" == '[B' && "$selected" == no ]]; then
        selected=yes
        /bin/cat "$HOME/workspace-trust-select-down.ansi"
      fi
      ;;
    $'\r'|'')
      printf 'confirm:%s\n' "$selected" >> "$HOME/keys.log"
      [[ "$selected" == yes ]] || exit 1
      break
      ;;
  esac
done
printf '\033[2J\033[Hready\r\n'
command=''
while IFS= read -r -n 1 key; do
  case "$key" in
    $'\r'|'')
      [[ "$command" == /exit ]] && exit 0
      printf 'command:%s\n' "$command" >> "$HOME/keys.log"
      [[ "$command" == /status ]] && printf 'Account: trusted\r\nDONE\r\n'
      command=''
      ;;
    *) command+="$key" ;;
  esac
done
"#;
    fs::write(&trust_binary, trust_script).unwrap();
    fs::set_permissions(&trust_binary, fs::Permissions::from_mode(0o755)).unwrap();

    let mut trust_env = BTreeMap::new();
    trust_env.insert(
        "HOME".to_owned(),
        SecretString::new(trust_dir.to_string_lossy().into_owned()),
    );
    trust_env.insert(
        "CLAUDE_CONFIG_DIR".to_owned(),
        SecretString::new(trust_dir.to_string_lossy().into_owned()),
    );
    trust_env.insert(
        "CLAUDE_SECURESTORAGE_CONFIG_DIR".to_owned(),
        SecretString::new(trust_dir.to_string_lossy().into_owned()),
    );

    let dedicated_probe = trust_dir.join("ariadusage").join("ClaudeProbe");
    let trusted_session = ClaudeCLISession::new(
        dedicated_probe.clone(),
        Arc::clone(&registry),
        Arc::clone(&launch_gate),
    );
    let trusted_req = ClaudeCLICaptureRequest {
        subcommand: "/status".to_owned(),
        binary: trust_binary.clone(),
        account_scope: Some("synthetic-account".to_owned()),
        timeout: Duration::from_secs(5),
        environment: trust_env.clone(),
        idle_timeout: None,
        stop_on_substrings: vec!["DONE".to_owned()],
        stop_when_normalized: None,
        settle_after_stop: Duration::ZERO,
        send_enter_every: None,
    };
    let status = trusted_session
        .capture(make_call(FetchInteraction::UserInitiated), trusted_req)
        .await
        .unwrap();
    trusted_session.reset().await;
    assert!(status.contains("Account: trusted"));
    assert_eq!(
        fs::read_to_string(trust_dir.join("keys.log")).unwrap(),
        "key:[B\nconfirm:yes\ncommand:/status\n"
    );

    // Outside dedicated probe directory -> trust dialog is cancelled with Escape (`\x1b`)
    fs::remove_file(trust_dir.join("keys.log")).unwrap();
    let untrusted_session = ClaudeCLISession::new(
        dedicated_probe,
        Arc::clone(&registry),
        Arc::clone(&launch_gate),
    )
    .with_working_directory(trust_dir.clone());
    let untrusted_req = ClaudeCLICaptureRequest {
        subcommand: "/status".to_owned(),
        binary: trust_binary,
        account_scope: Some("synthetic-account".to_owned()),
        timeout: Duration::from_secs(5),
        environment: trust_env,
        idle_timeout: None,
        stop_on_substrings: vec!["DONE".to_owned()],
        stop_when_normalized: None,
        settle_after_stop: Duration::ZERO,
        send_enter_every: None,
    };
    let err = untrusted_session
        .capture(make_call(FetchInteraction::UserInitiated), untrusted_req)
        .await
        .unwrap_err();
    untrusted_session.reset().await;
    assert_eq!(err, ClaudeCLISessionError::ProcessExited);
    assert_eq!(
        fs::read_to_string(trust_dir.join("keys.log")).unwrap(),
        "cancel\n"
    );
}

#[tokio::test]
async fn cli_timeout_retry_and_rate_limit_cooldown_semantics() {
    let temp = tempfile::tempdir().unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));

    // 1. SourceMode::Cli retries with longer timeout (24s -> 60s) after transient probe failure
    let timeouts_cli: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let timeouts_cli_clone = Arc::clone(&timeouts_cli);
    let cfg_retry = ClaudeOAuthStrategyConfig::new(Arc::clone(&net), temp.path(), temp.path())
        .with_has_cli(true)
        .with_cli_binary("/nonexistent/claude")
        .with_cli_status_probe_override(Arc::new(move |_, timeout, _| {
            let attempt = {
                let mut guard = timeouts_cli_clone.lock().unwrap();
                guard.push(timeout.as_secs());
                guard.len()
            };
            Box::pin(async move {
                if attempt == 1 {
                    Err(ClaudeStatusProbeError::TimedOut)
                } else {
                    Ok(ClaudeStatusSnapshot {
                        session_percent_left: Some(91),
                        weekly_percent_left: Some(88),
                        opus_percent_left: None,
                        extra_rate_windows: Vec::new(),
                        provider_cost: None,
                        account_email: Some("cli@example.com".to_owned()),
                        account_organization: Some("CLI Org".to_owned()),
                        login_method: Some("cli".to_owned()),
                        primary_reset_description: None,
                        secondary_reset_description: None,
                        opus_reset_description: None,
                        raw_text: "probe raw".to_owned(),
                    })
                }
            })
        }));
    let strategy_retry = ClaudeCLIFetchStrategy::new(cfg_retry);
    let cli_cx = make_context(
        FetchRuntime::Daemon,
        SourceMode::Cli,
        FetchInteraction::UserInitiated,
        BTreeMap::new(),
    );
    let res = strategy_retry.fetch(&cli_cx).await.unwrap();
    assert_eq!(*timeouts_cli.lock().unwrap(), vec![24, 60]);
    assert_eq!(res.usage.primary.as_ref().unwrap().used_percent, 9.0);
    assert_eq!(res.usage.secondary.as_ref().unwrap().used_percent, 12.0);

    // 2. SourceMode::Auto does NOT retry unrecoverable parse failure (12s only)
    let timeouts_parse: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let timeouts_parse_clone = Arc::clone(&timeouts_parse);
    let cfg_parse = ClaudeOAuthStrategyConfig::new(Arc::clone(&net), temp.path(), temp.path())
        .with_has_cli(true)
        .with_cli_binary("/nonexistent/claude")
        .with_cookie_source(Some(CookieSource::Off))
        .with_cli_status_probe_override(Arc::new(move |_, timeout, _| {
            timeouts_parse_clone.lock().unwrap().push(timeout.as_secs());
            Box::pin(async {
                Err(ClaudeStatusProbeError::ParseFailed(
                    "Missing Current session.".to_owned(),
                ))
            })
        }));
    let strategy_parse = ClaudeCLIFetchStrategy::new(cfg_parse);
    let auto_cx = make_context(
        FetchRuntime::Daemon,
        SourceMode::Auto,
        FetchInteraction::UserInitiated,
        BTreeMap::new(),
    );
    assert!(strategy_parse.fetch(&auto_cx).await.is_err());
    assert_eq!(*timeouts_parse.lock().unwrap(), vec![12]);

    // 3. SourceMode::Auto retries loading panel (12s -> 60s) even when web fallback is available
    let timeouts_loading: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let timeouts_loading_clone = Arc::clone(&timeouts_loading);
    let cfg_loading = ClaudeOAuthStrategyConfig::new(net, temp.path(), temp.path())
        .with_has_cli(true)
        .with_cli_binary("/nonexistent/claude")
        .with_cookie_source(Some(CookieSource::Manual))
        .with_manual_cookie_header(Some(SecretString::new(format!(
            "{SESSION_KEY_COOKIE_NAME}=sk-ant-web-loading"
        ))))
        .with_cli_status_probe_override(Arc::new(move |_, timeout, _| {
            let attempt = {
                let mut guard = timeouts_loading_clone.lock().unwrap();
                guard.push(timeout.as_secs());
                guard.len()
            };
            Box::pin(async move {
                if attempt == 1 {
                    Err(ClaudeStatusProbeError::ParseFailed(
                        "Claude CLI /usage is still loading usage data.".to_owned(),
                    ))
                } else {
                    Ok(ClaudeStatusSnapshot {
                        session_percent_left: Some(95),
                        weekly_percent_left: Some(93),
                        opus_percent_left: None,
                        extra_rate_windows: Vec::new(),
                        provider_cost: None,
                        account_email: Some("loading-cli@example.com".to_owned()),
                        account_organization: Some("Loading CLI Org".to_owned()),
                        login_method: Some("cli".to_owned()),
                        primary_reset_description: None,
                        secondary_reset_description: None,
                        opus_reset_description: None,
                        raw_text: "probe raw".to_owned(),
                    })
                }
            })
        }));
    let strategy_loading = ClaudeCLIFetchStrategy::new(cfg_loading);
    let res_loading = strategy_loading.fetch(&auto_cx).await.unwrap();
    assert_eq!(*timeouts_loading.lock().unwrap(), vec![12, 60]);
    assert_eq!(
        res_loading.usage.primary.as_ref().unwrap().used_percent,
        5.0
    );
}

#[test]
fn upstream_status_probe_parser_parity_suite() {
    // StatusProbeTests.swift:101-129 — `parse claude status`
    let sample_basic = concat!(
        "Settings: Status   Config   Usage (tab to cycle)\n\n",
        "Current session\n",
        "1% used  (Resets 5am (Europe/Vienna))\n",
        "Current week (all models)\n",
        "1% used  (Resets Dec 2 at 12am (Europe/Vienna))\n",
        "Current week (Sonnet only)\n",
        "1% used (Resets Dec 2 at 12am (Europe/Vienna))\n\n",
        "Nov 24, 2025 update:\n",
        "We've increased your limits and removed the Opus cap,\n",
        "so you can use Opus 4.5 up to your overall limit.\n",
        "Sonnet now has its own limit—it's set to match your previous overall limit,\n",
        "so you can use just as much as before.\n",
        "Account: user@example.com\n",
        "Org: Example Org"
    );
    let snap_basic = ClaudeStatusProbeParser::parse(sample_basic, None).unwrap();
    assert_eq!(snap_basic.session_percent_left, Some(99));
    assert_eq!(snap_basic.weekly_percent_left, Some(99));
    assert_eq!(snap_basic.opus_percent_left, Some(99));
    assert_eq!(
        snap_basic.account_email.as_deref(),
        Some("user@example.com")
    );
    assert_eq!(
        snap_basic.account_organization.as_deref(),
        Some("Example Org")
    );
    assert_eq!(
        snap_basic.primary_reset_description.as_deref(),
        Some("Resets 5am (Europe/Vienna)")
    );
    assert_eq!(
        snap_basic.secondary_reset_description.as_deref(),
        Some("Resets Dec 2 at 12am (Europe/Vienna)")
    );
    assert_eq!(
        snap_basic.opus_reset_description.as_deref(),
        Some("Resets Dec 2 at 12am (Europe/Vienna)")
    );

    // StatusProbeTests.swift:132-151 — `parse claude status with ANSI`
    let sample_ansi = concat!(
        "\u{1b}[35mCurrent session\u{1b}[0m\n",
        "40% used  (Resets 11am)\n",
        "Current week (all models)\n",
        "10% used  (Resets Nov 27)\n",
        "Current week (Sonnet only)\n",
        "0% used (Resets Nov 27)\n",
        "Account: user@example.com\n",
        "Org: ACME\n",
        "\u{1b}[0m"
    );
    let snap_ansi = ClaudeStatusProbeParser::parse(sample_ansi, None).unwrap();
    assert_eq!(snap_ansi.session_percent_left, Some(60));
    assert_eq!(snap_ansi.weekly_percent_left, Some(90));
    assert_eq!(snap_ansi.opus_percent_left, Some(100));
    assert_eq!(
        snap_ansi.primary_reset_description.as_deref(),
        Some("Resets 11am")
    );
    assert_eq!(
        snap_ansi.secondary_reset_description.as_deref(),
        Some("Resets Nov 27")
    );
    assert_eq!(
        snap_ansi.opus_reset_description.as_deref(),
        Some("Resets Nov 27")
    );

    // StatusProbeTests.swift:154-172 — `parse claude status legacy opus label`
    let sample_legacy_opus = concat!(
        "Current session\n",
        "12% used  (Resets 11am)\n",
        "Current week (all models)\n",
        "55% used  (Resets Nov 21)\n",
        "Current week (Opus)\n",
        "5% used (Resets Nov 21)\n",
        "Account: user@example.com\n",
        "Org: Example Org"
    );
    let snap_legacy_opus = ClaudeStatusProbeParser::parse(sample_legacy_opus, None).unwrap();
    assert_eq!(snap_legacy_opus.session_percent_left, Some(88));
    assert_eq!(snap_legacy_opus.weekly_percent_left, Some(45));
    assert_eq!(snap_legacy_opus.opus_percent_left, Some(95));
    assert_eq!(
        snap_legacy_opus.primary_reset_description.as_deref(),
        Some("Resets 11am")
    );
    assert_eq!(
        snap_legacy_opus.secondary_reset_description.as_deref(),
        Some("Resets Nov 21")
    );
    assert_eq!(
        snap_legacy_opus.opus_reset_description.as_deref(),
        Some("Resets Nov 21")
    );

    // StatusProbeTests.swift:175-185 — `parse claude status remaining keyword`
    let sample_remaining = concat!(
        "Current session\n",
        "12% remaining (Resets 11am)\n",
        "Current week (all models)\n",
        "40% remaining (Resets Nov 21)"
    );
    let snap_remaining = ClaudeStatusProbeParser::parse(sample_remaining, None).unwrap();
    assert_eq!(snap_remaining.session_percent_left, Some(12));
    assert_eq!(snap_remaining.weekly_percent_left, Some(40));

    // StatusProbeTests.swift:188-199 — `parse claude status enterprise session only`
    let sample_enterprise = concat!(
        "Current session\n",
        "█                                                  2% used\n",
        "Resets 3pm (Europe/Vienna)"
    );
    let snap_enterprise = ClaudeStatusProbeParser::parse(sample_enterprise, None).unwrap();
    assert_eq!(snap_enterprise.session_percent_left, Some(98));
    assert_eq!(snap_enterprise.weekly_percent_left, None);
    assert_eq!(
        snap_enterprise.primary_reset_description.as_deref(),
        Some("Resets 3pm (Europe/Vienna)")
    );
    assert_eq!(snap_enterprise.secondary_reset_description, None);

    // StatusProbeTests.swift:202-221 — `parse claude status reset mappings with CR line endings`
    let sample_cr = concat!(
        "Current  session\r",
        "██████████████████████████████████████████████████  17% used\r",
        "Resets 12:59pm (Europe/Paris)\r",
        "Current week (all models)\r",
        "██████████████████████████████████████████████████   4% used\r",
        "Resets Dec 24 at 3:59pm (Europe/Paris)\r",
        "Current week (Sonnet only)\r",
        "██████████████████████████████████████████████████   3% used\r",
        "Resets Dec 23 at 3:59am (Europe/Paris)\r"
    );
    let snap_cr = ClaudeStatusProbeParser::parse(sample_cr, None).unwrap();
    assert_eq!(snap_cr.session_percent_left, Some(83));
    assert_eq!(snap_cr.weekly_percent_left, Some(96));
    assert_eq!(snap_cr.opus_percent_left, Some(97));
    assert_eq!(
        snap_cr.primary_reset_description.as_deref(),
        Some("Resets 12:59pm (Europe/Paris)")
    );
    assert_eq!(
        snap_cr.secondary_reset_description.as_deref(),
        Some("Resets Dec 24 at 3:59pm (Europe/Paris)")
    );
    assert_eq!(
        snap_cr.opus_reset_description.as_deref(),
        Some("Resets Dec 23 at 3:59am (Europe/Paris)")
    );

    // StatusProbeTests.swift:224-237 — `parse claude status reset mappings does not promote weekly reset to session`
    let sample_no_promote = concat!(
        "Current session\n",
        "██████████████████████████████████████████████████  17% used\n",
        "Current week (all models)\n",
        "██████████████████████████████████████████████████   4% used\n",
        "Resets Dec 24 at 3:59pm (Europe/Paris)"
    );
    let snap_no_promote = ClaudeStatusProbeParser::parse(sample_no_promote, None).unwrap();
    assert_eq!(snap_no_promote.session_percent_left, Some(83));
    assert_eq!(snap_no_promote.weekly_percent_left, Some(96));
    assert_eq!(snap_no_promote.primary_reset_description, None);
    assert_eq!(
        snap_no_promote.secondary_reset_description.as_deref(),
        Some("Resets Dec 24 at 3:59pm (Europe/Paris)")
    );

    // StatusProbeTests.swift:240-262 — `parse claude status with plan and ansi noise`
    let sample_plan_ansi = concat!(
        "Current session\n",
        "10% used\n",
        "Current week (all models)\n",
        "20% used\n",
        "Current week (Opus)\n",
        "30% used\n",
        "Settings: Status   Config   Usage\n\n",
        "Login method: \u{1b}[22mClaude Max Account\u{1b}[0m\n",
        "Account: user@example.com\n",
        "Org: ACME"
    );
    let snap_plan_ansi = ClaudeStatusProbeParser::parse(sample_plan_ansi, None).unwrap();
    assert_eq!(snap_plan_ansi.login_method.as_deref(), Some("Max"));
    assert_eq!(
        snap_plan_ansi.account_email.as_deref(),
        Some("user@example.com")
    );
    assert_eq!(snap_plan_ansi.account_organization.as_deref(), Some("ACME"));

    // StatusProbeTests.swift:265-290 — `parse claude status with extra usage section`
    let sample_extra_section = concat!(
        "Settings:  Status   Config   Usage  (tab to cycle)\n\n",
        " Current session\n",
        " ▌                                                  1% used\n",
        " Resets 3:59pm (Europe/Helsinki)\n\n",
        " Current week (all models)\n",
        " ▌                                                  1% used\n",
        " Resets Jan 2, 2026, 10:59pm (Europe/Helsinki)\n\n",
        " Current week (Sonnet only)\n",
        "                                                    0% used\n\n",
        " Extra usage\n",
        " Extra usage not enabled • /extra-usage to enable"
    );
    let snap_extra_section = ClaudeStatusProbeParser::parse(sample_extra_section, None).unwrap();
    assert_eq!(snap_extra_section.session_percent_left, Some(99));
    assert_eq!(snap_extra_section.weekly_percent_left, Some(99));
    assert_eq!(snap_extra_section.opus_percent_left, Some(100));
    assert_eq!(
        snap_extra_section.primary_reset_description.as_deref(),
        Some("Resets 3:59pm (Europe/Helsinki)")
    );
    assert_eq!(
        snap_extra_section.secondary_reset_description.as_deref(),
        Some("Resets Jan 2, 2026, 10:59pm (Europe/Helsinki)")
    );

    // StatusProbeTests.swift:293-319 — `parse claude status ignores status bar context percent`
    let sample_status_bar = concat!(
        "Claude Code v2.1.29\n",
        "22:47 |  | Opus 4.5 | default | ░░░░░░░░░░ 0%  ◯ /ide for Visual Studio Code\n\n",
        "Settings:  Status   Config   Usage  (tab to cycle)\n",
        "Loading usage data…\n",
        "Esc to cancel\n\n",
        "Curretsession\n",
        "███████▌15%used\n",
        "Resets 11:30pm (Asia/Calcutta)\n\n",
        "Current week (all models)\n",
        "█▌                                                 3% used\n",
        "Resets Feb 12 at 1:30pm (Asia/Calcutta)\n\n",
        "Current week (Sonnet only)\n",
        "▌                                                  1% used\n",
        "Resets Feb 12 at 1:30pm (Asia/Calcutta)"
    );
    let snap_status_bar = ClaudeStatusProbeParser::parse(sample_status_bar, None).unwrap();
    assert_eq!(snap_status_bar.session_percent_left, Some(85));
    assert_eq!(snap_status_bar.weekly_percent_left, Some(97));
    assert_eq!(snap_status_bar.opus_percent_left, Some(99));

    // StatusProbeTests.swift:322-343 — `parse claude status loading panel surfaces loading stall`
    let sample_loading_stall = concat!(
        "Claude Code v2.1.29\n",
        "22:47 |  | Opus 4.5 | default | ░░░░░░░░░░ 0%  ◯ /ide for Visual Studio Code\n\n",
        "Settings:  Status   Config   Usage  (tab to cycle)\n",
        "Loading usage data…\n",
        "Esc to cancel"
    );
    match ClaudeStatusProbeParser::parse(sample_loading_stall, None).unwrap_err() {
        ClaudeStatusProbeError::ParseFailed(message) => {
            assert!(message.to_ascii_lowercase().contains("loading"));
        }
        other => panic!("unexpected error for loading stall: {other:?}"),
    }

    // StatusProbeTests.swift:346-370 — `parse claude retained usage panel classifies latest loading panel`
    let sample_retained_loading = concat!(
        "Settings:  Status   Config   Usage  (tab to cycle)\n",
        "Current session\n",
        "███████▌15%used\n",
        "Resets 11:30pm (Asia/Calcutta)\n\n",
        "Current week (all models)\n",
        "█▌ 3% used\n",
        "Resets Feb 12 at 1:30pm (Asia/Calcutta)\n\n",
        "Settings:  Status   Config   Usage  (tab to cycle)\n",
        "Loading usage data…\n",
        "Esc to cancel"
    );
    match ClaudeStatusProbeParser::parse(sample_retained_loading, None).unwrap_err() {
        ClaudeStatusProbeError::ParseFailed(message) => {
            assert!(message.to_ascii_lowercase().contains("loading"));
        }
        other => panic!("unexpected error for retained loading panel: {other:?}"),
    }

    // StatusProbeTests.swift:373-391 — `parse claude status status only output does not fallback to zero`
    let sample_status_only = concat!(
        "Claude Code v2.1.32\n",
        "01:07 |  | Opus 4.6 | default | ░░░░░░░░░░ 0% left\n",
        "Status: Partially Degraded Service\n",
        "/status"
    );
    assert!(matches!(
        ClaudeStatusProbeParser::parse(sample_status_only, None),
        Err(ClaudeStatusProbeError::ParseFailed(_))
    ));

    // StatusProbeTests.swift:394-414 — `parse claude status placeholder usage window does not use status bar percent`
    let sample_placeholder = concat!(
        "Claude Code v2.1.32\n",
        "01:07 |  | Opus 4.6 | default | ░░░░░░░░░░ 0% left\n",
        "Settings: Status   Config   Usage\n",
        "Current session\n",
        "Current week (all models)\n",
        "Current week (Sonnet only)"
    );
    assert!(matches!(
        ClaudeStatusProbeParser::parse(sample_placeholder, None),
        Err(ClaudeStatusProbeError::ParseFailed(_))
    ));

    // StatusProbeTests.swift:417-438 — `parse claude status compact markers still parse`
    let sample_compact_markers = concat!(
        "Settings:StatusConfigUsage(←/→ortabtocycle)\n",
        "Loadingusagedata…\n",
        "Curretsession\n",
        "███6%used\n",
        "Resets4:29am(Asia/Calcutta)\n",
        "Currentweek(allmodels)\n",
        "██4%used\n",
        "ResetsFeb12at1:29pm(Asia/Calcutta)\n",
        "Currentweek(Sonnetonly)\n",
        "▌1%used\n",
        "ResetsFeb12at1:29pm(Asia/Calcutta)"
    );
    let snap_compact_markers =
        ClaudeStatusProbeParser::parse(sample_compact_markers, None).unwrap();
    assert_eq!(snap_compact_markers.session_percent_left, Some(94));
    assert_eq!(snap_compact_markers.weekly_percent_left, Some(96));
    assert_eq!(snap_compact_markers.opus_percent_left, Some(99));
    assert_eq!(
        snap_compact_markers.secondary_reset_description.as_deref(),
        Some("ResetsFeb12at1:29pm(Asia/Calcutta)")
    );
    assert_eq!(
        snap_compact_markers.opus_reset_description.as_deref(),
        Some("ResetsFeb12at1:29pm(Asia/Calcutta)")
    );

    // StatusProbeTests.swift:441-457 — `parse claude status with bracket plan noise no esc`
    let sample_bracket_plan = concat!(
        "Current session\n",
        "10% used\n",
        "Current week (all models)\n",
        "20% used\n",
        "Current week (Opus)\n",
        "30% used\n",
        "Login method: [22m Claude Max Account\n",
        "Account: user@example.com"
    );
    let snap_bracket_plan = ClaudeStatusProbeParser::parse(sample_bracket_plan, None).unwrap();
    assert_eq!(snap_bracket_plan.login_method.as_deref(), Some("Max"));

    // StatusProbeTests.swift:460-480 — `surfaces claude token expired`
    let sample_token_expired = concat!(
        "Settings:  Status   Config   Usage\n\n",
        "Error: Failed to load usage data: {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\n",
        "\"message\":\"OAuth token has expired. Please obtain a new token or refresh your existing token.\",\n",
        "\"details\":{\"error_visibility\":\"user_facing\",\"error_code\":\"token_expired\"}},",
        "\"request_id\":\"req_123\"}"
    );
    match ClaudeStatusProbeParser::parse(sample_token_expired, None).unwrap_err() {
        ClaudeStatusProbeError::AuthenticationFailed(message) => {
            let lower = message.to_ascii_lowercase();
            assert!(lower.contains("token"));
            assert!(lower.contains("login"));
        }
        other => panic!("unexpected error for token expired: {other:?}"),
    }

    // StatusProbeTests.swift:483-506 & 870-887: login failures vs hook noise outside usage error
    for (err_type, msg) in [
        ("error", "OAuth account information not found in config"),
        (
            "error",
            "Your account does not have access to Claude Code. Please run /login",
        ),
        ("error", "API Error: 401"),
        ("permission_error", "API Error: 403"),
        (
            "error",
            "Claude CLI token expired. Run `claude login` to refresh.",
        ),
    ] {
        let sample = format!(
            "Error: Failed to load usage data: {{\"error\":{{\"type\":\"{err_type}\",\"message\":\"{msg}\"}}}}"
        );
        assert!(matches!(
            ClaudeStatusProbeParser::parse(&sample, None),
            Err(ClaudeStatusProbeError::AuthenticationFailed(_))
        ));
    }
    let hook_warning = concat!(
        "Hook warning: forbidden command skipped\n",
        "Error: Failed to load usage data: Session quota fields were unavailable\n"
    );
    assert!(matches!(
        ClaudeStatusProbeParser::parse(hook_warning, None),
        Err(ClaudeStatusProbeError::ParseFailed(_))
    ));

    // StatusProbeTests.swift:509-525 — `surfaces claude rate limited compact usage error`
    let sample_rate_limited_compact = concat!(
        "Settings:StatusConfigUsage(←/→ortabtocycle)\n",
        "Error:Failedtoloadusagedata:{\"error\":{\"message\":\"Ratelimited.Pleasetryagainlater.\",\"type\":\"rate_limit_error\"}}"
    );
    match ClaudeStatusProbeParser::parse(sample_rate_limited_compact, None).unwrap_err() {
        ClaudeStatusProbeError::ParseFailed(message) => {
            let lower = message.to_ascii_lowercase();
            assert!(lower.contains("rate"));
            assert!(lower.contains("limit"));
        }
        other => panic!("unexpected error for rate limited compact: {other:?}"),
    }

    // StatusProbeTests.swift:528-545 — `surfaces claude folder trust prompt`
    let sample_trust_plain = "Do you trust the files in this folder?\n\n/Users/example/project";
    match ClaudeStatusProbeParser::parse(sample_trust_plain, None).unwrap_err() {
        ClaudeStatusProbeError::ParseFailed(message) => {
            assert!(message.to_ascii_lowercase().contains("trust"));
            assert!(message.contains("/Users/example/project"));
            assert!(message.contains("cd \"/Users/example/project\" && claude"));
        }
        other => panic!("unexpected error for folder trust prompt: {other:?}"),
    }

    // StatusProbeTests.swift:548-560 — `surfaces claude folder trust prompt with CRLF and spaces`
    let sample_trust_crlf =
        "Do you trust the files in this folder?\r\n\r\n/Users/example/My Project\r\n";
    match ClaudeStatusProbeParser::parse(sample_trust_crlf, None).unwrap_err() {
        ClaudeStatusProbeError::ParseFailed(message) => {
            assert!(message.contains("/Users/example/My Project"));
            assert!(message.contains("cd \"/Users/example/My Project\" && claude"));
        }
        other => panic!("unexpected error for CRLF folder trust prompt: {other:?}"),
    }

    // StatusProbeTests.swift:563-578 — `surfaces claude folder trust prompt without folder path`
    let sample_trust_no_path = "Do you trust the files in this folder?";
    match ClaudeStatusProbeParser::parse(sample_trust_no_path, None).unwrap_err() {
        ClaudeStatusProbeError::ParseFailed(message) => {
            let lower = message.to_ascii_lowercase();
            assert!(lower.contains("trust"));
            assert!(lower.contains("auto-accept"));
        }
        other => panic!("unexpected error for folder trust prompt without path: {other:?}"),
    }

    let trust_same_line_noise = ClaudeStatusProbeParser::parse(
        "Do you trust the files in this folder? extra text on same line\n/Users/example/project\n",
        None,
    )
    .unwrap_err();
    assert!(
        !trust_same_line_noise
            .to_string()
            .contains("/Users/example/project"),
        "same-line text after trust prompt must prevent capturing the following line as folder hint"
    );

    // StatusProbeTests.swift:581-603 — `surfaces claude subscription notice without quota data`
    let sample_subscription_only =
        "You are currently using your subscription to power your Claude Code usage";
    match ClaudeStatusProbeParser::parse(sample_subscription_only, None).unwrap_err() {
        ClaudeStatusProbeError::ParseFailed(message) => {
            let lower = message.to_ascii_lowercase();
            assert!(lower.contains("subscription"));
            assert!(!lower.contains("still loading"));
            assert!(
                ClaudeStatusProbeParser::is_subscription_quota_unavailable_description(Some(
                    &message
                ))
            );
        }
        other => panic!("unexpected error for subscription notice: {other:?}"),
    }

    // StatusProbeTests.swift:606-631 — `parse claude status subscription notice is distinct from loading stall`
    match ClaudeStatusProbeParser::parse(sample_subscription_only, None).unwrap_err() {
        ClaudeStatusProbeError::ParseFailed(sub_message) => {
            assert!(!sub_message.to_ascii_lowercase().contains("still loading"));
        }
        other => panic!("unexpected error for subscription notice: {other:?}"),
    }
    let loading_only = concat!(
        "Settings:  Status   Config   Usage  (tab to cycle)\n",
        "Loading usage data…\n",
        "Esc to cancel"
    );
    match ClaudeStatusProbeParser::parse(loading_only, None).unwrap_err() {
        ClaudeStatusProbeError::ParseFailed(load_message) => {
            assert!(load_message.to_ascii_lowercase().contains("loading"));
        }
        other => panic!("unexpected error for loading panel: {other:?}"),
    }

    // StatusProbeTests.swift:634-656 — `parse claude status mixed loading and subscription notice surfaces subscription error`
    let mixed_capture = concat!(
        "Settings:  Status   Config   Usage  (tab to cycle)\n",
        "Loading usage data…\n",
        "Esc to cancel\n\n",
        "You are currently using your subscription to power your Claude Code usage"
    );
    match ClaudeStatusProbeParser::parse(mixed_capture, None).unwrap_err() {
        ClaudeStatusProbeError::ParseFailed(message) => {
            let lower = message.to_ascii_lowercase();
            assert!(lower.contains("subscription"));
            assert!(!lower.contains("still loading"));
        }
        other => panic!("unexpected error for mixed capture: {other:?}"),
    }

    // Fuzzy all-models weekly usage (`all modls`) + garbled `t` reset
    let team_weekly = concat!(
        "Settings: Status Config Usage (tab to cycle)\n\n",
        "Current session\n",
        "13% used\n",
        "Resets 1:59pm (Europe/Brussels)\n\n",
        "Current week (all modls)\n",
        "7% used\n",
        "Resets Jan 24 t 10:59am (Europe/Brussels)\n"
    );
    let snap_team = ClaudeStatusProbeParser::parse(team_weekly, None).unwrap();
    assert_eq!(snap_team.session_percent_left, Some(87));
    assert_eq!(snap_team.weekly_percent_left, Some(93));
    assert_eq!(snap_team.opus_percent_left, None);
    assert_eq!(
        snap_team.secondary_reset_description.as_deref(),
        Some("Resets Jan 24 at 10:59am (Europe/Brussels)")
    );

    // Case-sensitivity of `extract_usage_error_json`: lowercase `failed to load usage data:` falls back to line scanner
    let lowercase_failed = ClaudeStatusProbeParser::parse(
        "failed to load usage data: {\"error\":{\"message\":\"Custom message\"}}",
        None,
    )
    .unwrap_err();
    assert_eq!(
        lowercase_failed,
        ClaudeStatusProbeError::ParseFailed(
            "Claude CLI could not load usage data. Open the CLI and retry `/usage`.".to_owned()
        )
    );

    // `is_retryable` and `is_rate_limit_error` exact semantics
    assert!(ClaudeStatusProbeError::TimedOut.is_retryable());
    assert!(
        ClaudeStatusProbeError::ParseFailed(
            "Claude CLI /usage is still loading usage data.".to_owned()
        )
        .is_retryable()
    );
    assert!(
        !ClaudeStatusProbeError::ParseFailed("request timed out in output".to_owned())
            .is_retryable(),
        "ParseFailed is only retryable when 'still loading usage' is present"
    );
    assert!(
        !ClaudeStatusProbeError::ParseFailed("rate limited".to_owned()).is_rate_limit_error(),
        "bare 'rate limited' without 'claude cli' and 'usage' must not trigger CLI rate-limit gate"
    );
    assert!(
        ClaudeStatusProbeError::ParseFailed("Claude CLI usage endpoint is rate limited".to_owned())
            .is_rate_limit_error()
    );
    assert!(
        ClaudeStatusProbeError::ParseFailed("error: rate_limit_error".to_owned())
            .is_rate_limit_error()
    );

    // `clean_plan_name` and login method extraction
    assert_eq!(
        ClaudeStatusProbeParser::clean_plan_name("\u{1b}[32mClaude Max Account\u{1b}[0m"),
        "Max"
    );
    assert_eq!(ClaudeStatusProbeParser::clean_plan_name("oauth"), "Ollama");
    let d_today_bridge = "Use Claude\nd → today\nCurrent session\n10% used";
    let snap_bridge = ClaudeStatusProbeParser::parse(d_today_bridge, None).unwrap();
    assert_eq!(snap_bridge.login_method, None);

    // ESC followed by newline does not swallow the newline
    assert_eq!(
        ClaudeCLIScreen::render("line1\u{1b}\nline2"),
        "line1\nline2"
    );
    // Unqualified U+26A0 (`⚠`) is 1 cell wide, while `⚠️` (`U+26A0 U+FE0F`) is 2 cells wide
    assert_eq!(ClaudeCLIScreen::render("⚠ ok\u{1b}[3GX"), "⚠ Xk");
    assert_eq!(ClaudeCLIScreen::render("⚠️ ok\u{1b}[4GX"), "⚠️ Xk");
}

#[tokio::test]
async fn scoped_weekly_usage_and_usage_insights_parity() {
    // 1. ClaudeCLIScopedWeeklyUsageTests.swift:7-50 — `CLI usage surfaces Fable scoped weekly limit`
    let cli_usage_fable = concat!(
        "Settings  Status  Config  Usage  Stats\n\n",
        "Current session\n",
        "9% used\n",
        "Resets 2:09pm (Europe/Prague)\n\n",
        "Current week (all models)\n",
        "67% used\n",
        "Resets Jul 10 t 2:59am (Europe/Prague)\n\n",
        "Current week (Fable)\n",
        "68% used\n",
        "Reset Jul 10 at 2:59am (Europe/Prague)\n\n",
        "Current week (Example Model)\n",
        "12% used"
    );
    let status_fable = ClaudeStatusProbeParser::parse(cli_usage_fable, None).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));
    let cfg = ClaudeOAuthStrategyConfig::new(net, temp.path(), temp.path())
        .with_has_cli(true)
        .with_cli_binary("/nonexistent/claude")
        .with_cli_status_probe_override(Arc::new(move |_, _, _| {
            let cloned = status_fable.clone();
            Box::pin(async move { Ok(cloned) })
        }));
    let strategy = ClaudeCLIFetchStrategy::new(cfg);
    let cx = make_context(
        FetchRuntime::Daemon,
        SourceMode::Cli,
        FetchInteraction::UserInitiated,
        BTreeMap::new(),
    );
    let snapshot = strategy.fetch(&cx).await.unwrap().usage;
    let fable = snapshot
        .extra_rate_windows
        .as_ref()
        .unwrap()
        .iter()
        .find(|w| w.id == "claude-weekly-scoped-fable")
        .expect("should contain Fable scoped window");
    assert_eq!(fable.title, "Fable only");
    assert_eq!(fable.window.used_percent, 68.0);
    assert_eq!(
        fable.window.reset_description.as_deref(),
        Some("Reset Jul 10 at 2:59am (Europe/Prague)")
    );
    let example = snapshot
        .extra_rate_windows
        .as_ref()
        .unwrap()
        .iter()
        .find(|w| w.id == "claude-weekly-scoped-example-model")
        .expect("should contain Example Model scoped window");
    assert_eq!(example.title, "Example Model only");
    assert_eq!(example.window.used_percent, 12.0);
    assert_eq!(
        example.window.reset_description.as_deref(),
        Some("Resets Jul 10 at 2:59am (Europe/Prague)")
    );
    assert!(snapshot.tertiary.is_none());

    // 2. ClaudeCLIScopedWeeklyUsageTests.swift:52-67 — `scoped weekly panel does not become all models weekly usage`
    let snap_scoped_only = ClaudeStatusProbeParser::parse(
        "Current session\n9% used\n\nCurrent week (Fable)\n68% used",
        None,
    )
    .unwrap();
    assert_eq!(snap_scoped_only.weekly_percent_left, None);
    assert_eq!(snap_scoped_only.secondary_reset_description, None);
    assert_eq!(
        snap_scoped_only
            .extra_rate_windows
            .iter()
            .map(|w| w.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Fable only"]
    );

    // 3. ClaudeCLIScopedWeeklyUsageTests.swift:69-81 — `compact scoped weekly label is parsed`
    let snap_compact_scoped = ClaudeStatusProbeParser::parse(
        "Current session\n9% used\n\nCurrentweek(Fable)\n68% used",
        None,
    )
    .unwrap();
    assert_eq!(
        snap_compact_scoped
            .extra_rate_windows
            .iter()
            .map(|w| w.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Fable only"]
    );
    assert_eq!(
        snap_compact_scoped.extra_rate_windows[0]
            .window
            .used_percent,
        68.0
    );

    // 4. ClaudeCLIScopedWeeklyUsageTests.swift:83-98 — `overlapping scoped model names do not cross panel boundaries`
    let snap_overlapping = ClaudeStatusProbeParser::parse(
        concat!(
            "Current session\n",
            "9% used\n\n",
            "Current week (Example Model)\n",
            "rendering\n\n",
            "Current week (Example Model Plus)\n",
            "42% used"
        ),
        None,
    )
    .unwrap();
    assert_eq!(
        snap_overlapping
            .extra_rate_windows
            .iter()
            .map(|w| w.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Example Model Plus only"]
    );
    assert_eq!(
        snap_overlapping.extra_rate_windows[0].window.used_percent,
        42.0
    );

    // 5. ClaudeCLIScopedWeeklyUsageTests.swift:100-118 — `informational Sonnet prose does not duplicate a scoped limit`
    let snap_sonnet_prose = ClaudeStatusProbeParser::parse(
        concat!(
            "Current session\n",
            "9% used\n\n",
            "Current week (all models)\n",
            "20% used\n\n",
            "Current week (Fable)\n",
            "42% used\n\n",
            "Sonnet now has its own limit."
        ),
        None,
    )
    .unwrap();
    assert_eq!(snap_sonnet_prose.opus_percent_left, None);
    assert_eq!(
        snap_sonnet_prose
            .extra_rate_windows
            .iter()
            .map(|w| w.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Fable only"]
    );
    assert_eq!(
        snap_sonnet_prose.extra_rate_windows[0].window.used_percent,
        42.0
    );

    // 6. ClaudeCLIScopedWeeklyUsageTests.swift:120-136 — `Sonnet prefixed scoped model does not become legacy quota`
    let snap_sonnet_variant = ClaudeStatusProbeParser::parse(
        concat!(
            "Current session\n",
            "9% used\n\n",
            "Current week (all models)\n",
            "20% used\n\n",
            "Current week (Sonnet Test Variant)\n",
            "42% used"
        ),
        None,
    )
    .unwrap();
    assert_eq!(snap_sonnet_variant.opus_percent_left, None);
    assert_eq!(
        snap_sonnet_variant
            .extra_rate_windows
            .iter()
            .map(|w| w.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Sonnet Test Variant only"]
    );
    assert_eq!(
        snap_sonnet_variant.extra_rate_windows[0]
            .window
            .used_percent,
        42.0
    );

    // 7. ClaudeCLIScopedWeeklyUsageTests.swift:138-162 — `later complete scoped panel replaces partial redraw`
    let spacer = vec!["rendering"; 14].join("\n");
    let snap_partial_redraw = ClaudeStatusProbeParser::parse(
        &format!(
            concat!(
                "Current session\n",
                "9% used\n\n",
                "Current week (all models)\n",
                "67% used\n\n",
                "Current week (Fable)\n",
                "{}\n\n",
                "Current week (Fable)\n",
                "70% used\n",
                "Reset Jul 10 at 2:59am (Europe/Prague)"
            ),
            spacer
        ),
        None,
    )
    .unwrap();
    assert_eq!(snap_partial_redraw.extra_rate_windows.len(), 1);
    assert_eq!(
        snap_partial_redraw.extra_rate_windows[0]
            .window
            .used_percent,
        70.0
    );
    assert_eq!(
        snap_partial_redraw.extra_rate_windows[0]
            .window
            .reset_description
            .as_deref(),
        Some("Reset Jul 10 at 2:59am (Europe/Prague)")
    );

    // 8. ClaudeCLIScopedWeeklyUsageTests.swift:164-182 — `incomplete scoped panel stops at session redraw`
    let snap_incomplete_scoped = ClaudeStatusProbeParser::parse(
        concat!(
            "Current week (Fable)\n",
            "rendering\n\n",
            "Current session\n",
            "9% used\n\n",
            "Current week (all models)\n",
            "20% used"
        ),
        None,
    )
    .unwrap();
    assert_eq!(snap_incomplete_scoped.session_percent_left, Some(91));
    assert_eq!(snap_incomplete_scoped.weekly_percent_left, Some(80));
    assert!(snap_incomplete_scoped.extra_rate_windows.is_empty());

    // 9. ClaudeCLIScopedWeeklyUsageTests.swift:184-202 — `incomplete all models panel does not consume scoped percentage`
    let snap_incomplete_all_models = ClaudeStatusProbeParser::parse(
        concat!(
            "Current session\n",
            "9% used\n\n",
            "Current week (all models)\n",
            "rendering\n\n",
            "Current week (Fable)\n",
            "42% used"
        ),
        None,
    )
    .unwrap();
    assert_eq!(snap_incomplete_all_models.weekly_percent_left, None);
    assert_eq!(
        snap_incomplete_all_models
            .extra_rate_windows
            .iter()
            .map(|w| w.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Fable only"]
    );
    assert_eq!(
        snap_incomplete_all_models.extra_rate_windows[0]
            .window
            .used_percent,
        42.0
    );

    // 10. ClaudeCLIScopedWeeklyUsageTests.swift:204-225 — `incomplete Opus panel does not consume prefixed scoped percentage`
    let snap_incomplete_opus = ClaudeStatusProbeParser::parse(
        concat!(
            "Current session\n",
            "9% used\n\n",
            "Current week (all models)\n",
            "20% used\n\n",
            "Current week (Opus)\n",
            "rendering\n\n",
            "Current week (Opus Test Variant)\n",
            "42% used"
        ),
        None,
    )
    .unwrap();
    assert_eq!(snap_incomplete_opus.opus_percent_left, None);
    assert_eq!(
        snap_incomplete_opus
            .extra_rate_windows
            .iter()
            .map(|w| w.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Opus Test Variant only"]
    );
    assert_eq!(
        snap_incomplete_opus.extra_rate_windows[0]
            .window
            .used_percent,
        42.0
    );

    // 11. ClaudeCLIScopedWeeklyUsageTests.swift:227-248 — `later complete scoped panel replaces earlier complete value`
    let snap_later_complete = ClaudeStatusProbeParser::parse(
        concat!(
            "Current session\n",
            "9% used\n\n",
            "Current week (all models)\n",
            "67% used\n\n",
            "Current week (Fable)\n",
            "20% used\n\n",
            "Current week (Fable)\n",
            "70% used"
        ),
        None,
    )
    .unwrap();
    assert_eq!(snap_later_complete.extra_rate_windows.len(), 1);
    assert_eq!(
        snap_later_complete.extra_rate_windows[0]
            .window
            .used_percent,
        70.0
    );

    // Model name in CLI panel already ending with "only" is not doubled to "only only"
    let now = "2026-04-06T09:00:00Z".parse::<Timestamp>().unwrap();
    let cli_only_panel = concat!(
        "Current session\n",
        "10% used\n",
        "Current week (Fable only)\n",
        "35% used\n"
    );
    let snap_only =
        ClaudeStatusProbeParser::parse_at(cli_only_panel, None, now, &TimeZone::UTC).unwrap();
    assert_eq!(snap_only.extra_rate_windows.len(), 1);
    assert_eq!(
        snap_only.extra_rate_windows[0].id,
        "claude-weekly-scoped-fable-only"
    );
    assert_eq!(snap_only.extra_rate_windows[0].title, "Fable only");
    assert_eq!(snap_only.extra_rate_windows[0].window.used_percent, 35.0);

    // 12. ClaudeCLIScopedWeeklyUsageTests.swift:250-286 — `web extra windows merge with CLI scoped weekly limits`
    let fable_cli = NamedRateWindow::new(
        "claude-weekly-scoped-fable",
        "Fable only",
        RateWindow::new(
            68.0,
            Some(7 * 24 * 60),
            None,
            Some("Resets Jul 10 at 2:59am (Europe/Prague)".to_owned()),
            None,
            false,
        )
        .unwrap(),
    );
    let web_fable = ClaudeScopedWeeklyLimitMapper::named_windows(&[OAuthScopedLimitEntry {
        kind: Some("weekly_scoped".to_owned()),
        group: Some("weekly".to_owned()),
        percent: Some(70.0),
        resets_at: None,
        model_id: Some("test-only-fable-id".to_owned()),
        model_name: Some("Fable".to_owned()),
    }])
    .into_iter()
    .next()
    .unwrap();
    let routines = NamedRateWindow::new(
        "claude-routines",
        "Daily Routines",
        RateWindow::new(11.0, Some(7 * 24 * 60), None, None, None, false).unwrap(),
    );
    let merged = merge_extra_rate_windows(&[fable_cli], &[web_fable.clone(), routines]);
    assert_eq!(
        merged.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
        vec!["claude-weekly-scoped-fable", "claude-routines"]
    );
    assert_eq!(web_fable.id, "claude-weekly-scoped-test-only-fable-id");
    assert_eq!(merged.first().unwrap().window.used_percent, 68.0);
    assert_eq!(merged.last().unwrap().title, "Daily Routines");

    // 13. ClaudeCLIScopedWeeklyUsageTests.swift:288-309 — `same title web limits keep distinct stable IDs`
    let web_limits: Vec<NamedRateWindow> = ["first-id", "second-id"]
        .into_iter()
        .map(|id| {
            NamedRateWindow::new(
                format!("claude-weekly-scoped-{id}"),
                "Example Model only",
                RateWindow::new(25.0, Some(7 * 24 * 60), None, None, None, false).unwrap(),
            )
        })
        .collect();
    let merged_web_only = merge_extra_rate_windows(&[], &web_limits);
    assert_eq!(
        merged_web_only
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "claude-weekly-scoped-first-id",
            "claude-weekly-scoped-second-id"
        ]
    );

    // 14. ClaudeCLIScopedWeeklyUsageTests.swift:311-341 — `ambiguous same title web limits survive CLI merge`
    let cli_example = NamedRateWindow::new(
        "claude-weekly-scoped-example-model",
        "Example Model only",
        RateWindow::new(20.0, Some(7 * 24 * 60), None, None, None, false).unwrap(),
    );
    let merged_ambiguous = merge_extra_rate_windows(&[cli_example], &web_limits);
    assert_eq!(
        merged_ambiguous
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "claude-weekly-scoped-example-model",
            "claude-weekly-scoped-first-id",
            "claude-weekly-scoped-second-id",
        ]
    );

    // 15. ClaudeUsageInsightsTests.swift:6-45 — exact upstream insights fixture
    let upstream_insights = concat!(
        "What's contributing to your limits usage?\n",
        "Approximate, based on local sessions on this machine — does not include other devices or claude.ai.\n",
        "Last 24h · 100 requests · 2 sessions\n",
        "  30% of your usage was at >150k context\n",
        "  20% of your usage was while 4+ sessions ran in parallel\n",
        "  Top MCP servers: Claude Browser 10%, claude-in-chrome 5%\n",
        "Last 7d · 1000 requests · 20 sessions\n",
        "  Top skills: Claude Max 20%"
    );
    let snap_insights_quota = ClaudeStatusProbeParser::parse(
        &format!(
            concat!(
                "Settings Status Config Usage Stats\n",
                "Current session\n",
                "██████ 13% used\n",
                "Resets 2:20pm (Asia/Singapore)\n",
                "Current week (all models)\n",
                "█ 2% used\n",
                "Resets Oct 3 at 8am (Asia/Singapore)\n",
                "{}"
            ),
            upstream_insights
        ),
        None,
    )
    .unwrap();
    assert_eq!(snap_insights_quota.session_percent_left, Some(87));
    assert_eq!(snap_insights_quota.weekly_percent_left, Some(98));
    assert_eq!(snap_insights_quota.login_method, None);
    assert_eq!(
        snap_insights_quota.primary_reset_description.as_deref(),
        Some("Resets 2:20pm (Asia/Singapore)")
    );

    let insights_err = ClaudeStatusProbeParser::parse(
        &format!(
            "You are currently using your subscription to power your Claude Code usage\n{upstream_insights}"
        ),
        None,
    )
    .unwrap_err();
    assert_eq!(
        insights_err,
        ClaudeStatusProbeError::ParseFailed(
            "Claude CLI /usage returned usage insights without quota data.".to_owned()
        )
    );
}

#[tokio::test]
async fn fetch_identity_and_touch_oauth_auth_path_honor_overrides_without_spawning_binary() {
    let temp = tempfile::tempdir().unwrap();
    let marker_file = temp.path().join("unexpected-identity-spawn.log");
    let cli_path = temp.path().join("claude");
    let script = format!(
        "#!/bin/sh\nprintf 'spawned\\n' >> '{}'\nexit 0\n",
        marker_file.display()
    );
    fs::write(&cli_path, script).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&cli_path, fs::Permissions::from_mode(0o755)).unwrap();

    let registry = Arc::new(ProcessRegistry::new());
    let launch_gate = Arc::new(LaunchGate::new(Instant::now));
    let session = Arc::new(ClaudeCLISession::new(
        temp.path().join("dedicated-probe"),
        Arc::clone(&registry),
        Arc::clone(&launch_gate),
    ));

    let override_calls = Arc::new(AtomicUsize::new(0));
    let override_calls_clone = Arc::clone(&override_calls);
    let probe_with_fetch_override = ClaudeStatusProbe::new(
        BTreeMap::new(),
        temp.path().to_path_buf(),
        Arc::clone(&session),
    )
    .with_binary(cli_path.to_string_lossy().into_owned())
    .with_fetch_override(Arc::new(move |_, _, _| {
        override_calls_clone.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(ClaudeStatusSnapshot {
                session_percent_left: Some(80),
                weekly_percent_left: Some(70),
                opus_percent_left: None,
                extra_rate_windows: Vec::new(),
                provider_cost: None,
                account_email: Some("override@example.com".to_owned()),
                account_organization: Some("Override Org".to_owned()),
                login_method: Some("Max".to_owned()),
                primary_reset_description: None,
                secondary_reset_description: None,
                opus_reset_description: None,
                raw_text: "override".to_owned(),
            })
        })
    }));

    let identity = probe_with_fetch_override
        .fetch_identity(
            make_call(FetchInteraction::UserInitiated),
            Duration::from_secs(5),
        )
        .await
        .expect("fetch_identity should return override identity without spawning");
    assert_eq!(
        identity.account_email.as_deref(),
        Some("override@example.com")
    );
    assert_eq!(
        identity.account_organization.as_deref(),
        Some("Override Org")
    );
    assert_eq!(identity.login_method.as_deref(), Some("Max"));

    probe_with_fetch_override
        .touch_oauth_auth_path(
            make_call(FetchInteraction::UserInitiated),
            Duration::from_secs(5),
        )
        .await
        .expect("touch_oauth_auth_path should honor fetch_override without spawning");
    assert_eq!(override_calls.load(Ordering::SeqCst), 2);
    assert!(
        !marker_file.exists(),
        "fetch_identity and touch_oauth_auth_path must never spawn a binary when fetch_override is set"
    );

    // When only `direct_fetch_override` is set, both return `ClaudeNotInstalled` without resolving or spawning
    let probe_with_direct_only =
        ClaudeStatusProbe::new(BTreeMap::new(), temp.path().to_path_buf(), session)
            .with_binary(cli_path.to_string_lossy().into_owned())
            .with_direct_fetch_override(Arc::new(|_, _, _| {
                Box::pin(async { Err(ClaudeStatusProbeError::TimedOut) })
            }));
    assert_eq!(
        probe_with_direct_only
            .fetch_identity(
                make_call(FetchInteraction::UserInitiated),
                Duration::from_secs(5),
            )
            .await,
        Err(ClaudeStatusProbeError::ClaudeNotInstalled)
    );
    assert_eq!(
        probe_with_direct_only
            .touch_oauth_auth_path(
                make_call(FetchInteraction::UserInitiated),
                Duration::from_secs(5),
            )
            .await,
        Err(ClaudeStatusProbeError::ClaudeNotInstalled)
    );
    assert!(!marker_file.exists());
}

#[tokio::test]
async fn cancellation_propagates_without_retry_or_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_clone = Arc::clone(&attempts);
    let cfg = ClaudeOAuthStrategyConfig::new(net, temp.path(), temp.path())
        .with_has_cli(true)
        .with_cli_binary("/nonexistent/claude")
        .with_cli_status_probe_override(Arc::new(move |_, _, _| {
            attempts_clone.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(ClaudeStatusProbeError::Cancelled) })
        }));
    let strategy = ClaudeCLIFetchStrategy::new(cfg);
    let cx = make_context(
        FetchRuntime::Daemon,
        SourceMode::Cli,
        FetchInteraction::UserInitiated,
        BTreeMap::new(),
    );
    let res = strategy.fetch(&cx).await;
    assert!(matches!(res, Err(FetchError::Cancelled)));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}
