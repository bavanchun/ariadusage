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
    // 1. Exact Timestamp assertions from ClaudeResetOccurrenceTests
    let now_vienna = "2026-04-06T09:00:00Z".parse::<Timestamp>().unwrap();
    let vienna = TimeZone::get("Europe/Vienna").unwrap();
    let reset_time = ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
        Some("Resets 11am (Europe/Vienna)"),
        now_vienna,
        Some(Duration::from_secs(5 * 3600)),
        &vienna,
    );
    assert_eq!(
        reset_time,
        Some("2026-04-06T09:00:00Z".parse::<Timestamp>().unwrap())
    );

    // Time-only 5 minutes past today stays today when within 5h session window (StatusProbeTests.swift:668-670),
    // rolls to tomorrow when > 5h past or when no expected_window is provided (StatusProbeTests.swift:758-760)
    let now_late = "2026-04-06T09:05:00Z".parse::<Timestamp>().unwrap();
    let stale_same_day = ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
        Some("Resets 11am (Europe/Vienna)"),
        now_late,
        Some(Duration::from_secs(5 * 3600)),
        &vienna,
    );
    assert_eq!(
        stale_same_day,
        Some("2026-04-06T09:00:00Z".parse::<Timestamp>().unwrap())
    );
    let rolled_tomorrow_forward = ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
        Some("Resets 11am (Europe/Vienna)"),
        now_late,
        None,
        &vienna,
    );
    assert_eq!(
        rolled_tomorrow_forward,
        Some("2026-04-07T09:00:00Z".parse::<Timestamp>().unwrap())
    );
    let now_beyond_5h = "2026-04-06T15:00:00Z".parse::<Timestamp>().unwrap();
    let rolled_tomorrow_beyond_window = ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
        Some("Resets 11am (Europe/Vienna)"),
        now_beyond_5h,
        Some(Duration::from_secs(5 * 3600)),
        &vienna,
    );
    assert_eq!(
        rolled_tomorrow_beyond_window,
        Some("2026-04-07T09:00:00Z".parse::<Timestamp>().unwrap())
    );

    // Month-day near year boundary keeps current year when in future, rolls to next year when in past
    let dec_now = "2026-12-30T12:00:00Z".parse::<Timestamp>().unwrap();
    let dec_future = ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
        Some("Resets Dec 31 at 6pm (UTC)"),
        dec_now,
        Some(Duration::from_secs(7 * 24 * 3600)),
        &TimeZone::UTC,
    );
    assert_eq!(
        dec_future,
        Some("2026-12-31T18:00:00Z".parse::<Timestamp>().unwrap())
    );

    let jan_rollover = ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
        Some("Resets Jan 2 at 5am (UTC)"),
        dec_now,
        Some(Duration::from_secs(7 * 24 * 3600)),
        &TimeZone::UTC,
    );
    assert_eq!(
        jan_rollover,
        Some("2027-01-02T05:00:00Z".parse::<Timestamp>().unwrap())
    );

    let explicit_year_reset = ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
        Some("Resets Oct 14, 2026 at 10:30pm (UTC)"),
        now_vienna,
        Some(Duration::from_secs(7 * 24 * 3600)),
        &TimeZone::UTC,
    );
    assert_eq!(
        explicit_year_reset,
        Some("2026-10-14T22:30:00Z".parse::<Timestamp>().unwrap())
    );

    let dot_minute_reset = ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
        Some("Resets 14.30 (UTC)"),
        now_vienna,
        Some(Duration::from_secs(5 * 3600)),
        &TimeZone::UTC,
    );
    assert_eq!(
        dot_minute_reset,
        Some("2026-04-06T14:30:00Z".parse::<Timestamp>().unwrap())
    );

    // Feb 29 leap year search from non-leap year 2026 -> 2028-02-29T09:00:00Z
    let leap_day_reset = ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
        Some("Resets Feb 29 at 9am (UTC)"),
        now_vienna,
        None,
        &TimeZone::UTC,
    );
    assert_eq!(
        leap_day_reset,
        Some("2028-02-29T09:00:00Z".parse::<Timestamp>().unwrap())
    );

    // Compact resets without space after `Resets` (`Resets4:29am(Asia/Calcutta)` and `ResetsMar13at12:30pm(Asia/Calcutta)`)
    // At 2026-03-10T00:00:00Z (05:30 Asia/Calcutta), 04:29 Asia/Calcutta is 1h01m ago (2026-03-09T22:59:00Z), within the 5h window;
    // without expected_window, it rolls forward to tomorrow (2026-03-10T22:59:00Z).
    let compact_now = "2026-03-10T00:00:00Z".parse::<Timestamp>().unwrap();
    let compact_time = ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
        Some("Resets4:29am(Asia/Calcutta)"),
        compact_now,
        Some(Duration::from_secs(5 * 3600)),
        &TimeZone::UTC,
    );
    assert_eq!(
        compact_time,
        Some("2026-03-09T22:59:00Z".parse::<Timestamp>().unwrap())
    );
    let compact_time_forward = ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
        Some("Resets4:29am(Asia/Calcutta)"),
        compact_now,
        None,
        &TimeZone::UTC,
    );
    assert_eq!(
        compact_time_forward,
        Some("2026-03-10T22:59:00Z".parse::<Timestamp>().unwrap())
    );
    let compact_date = ClaudeStatusProbeParser::parse_reset_date_with_default_tz(
        Some("ResetsMar13at12:30pm(Asia/Calcutta)"),
        compact_now,
        Some(Duration::from_secs(7 * 24 * 3600)),
        &TimeZone::UTC,
    );
    assert_eq!(
        compact_date,
        Some("2026-03-13T07:00:00Z".parse::<Timestamp>().unwrap())
    );

    // Subscription notice without session quota data
    let notice_text = "You are currently using your subscription to power your Claude Code usage.";
    let err = ClaudeStatusProbeParser::parse(notice_text, None).unwrap_err();
    assert_eq!(
        err,
        ClaudeStatusProbeError::ParseFailed(SUBSCRIPTION_QUOTA_UNAVAILABLE_DESCRIPTION.to_owned())
    );
    assert!(
        ClaudeStatusProbeParser::is_subscription_quota_unavailable_description(Some(
            &err.to_string()
        ))
    );

    // Legacy JSON CLI payload parsing (ClaudeResetJSONParserTests)
    let json = br#"{
        "ok": true,
        "session_5h": { "pct_used": 1, "resets": "11am (Europe/Vienna)" },
        "week_all_models": { "pct_used": 8, "resets": "Nov 21 at 5am (Europe/Vienna)" },
        "week_sonnet": { "pct_used": 0, "resets": "Nov 21 at 5am (Europe/Vienna)" },
        "account_email": "secret-user@example.com",
        "account_org": "Secret Org",
        "login_method": "Claude Max"
    }"#;
    let parsed_json = ClaudeStatusProbeParser::parse_json(json, now_vienna)
        .expect("should parse legacy JSON CLI output");
    assert_eq!(parsed_json.primary.as_ref().unwrap().used_percent, 1.0);
    assert_eq!(
        parsed_json.primary.as_ref().unwrap().window_minutes,
        Some(300)
    );
    assert_eq!(
        parsed_json.primary.as_ref().unwrap().resets_at,
        Some("2026-04-06T09:00:00Z".parse::<Timestamp>().unwrap())
    );
    assert_eq!(parsed_json.secondary.as_ref().unwrap().used_percent, 8.0);
    assert_eq!(
        parsed_json.secondary.as_ref().unwrap().window_minutes,
        Some(10080)
    );
    assert_eq!(
        parsed_json.secondary.as_ref().unwrap().resets_at,
        Some("2026-11-21T04:00:00Z".parse::<Timestamp>().unwrap())
    );
    assert_eq!(parsed_json.tertiary.as_ref().unwrap().used_percent, 0.0);

    // Debug redaction check across ClaudeStatusSnapshot, ClaudeCLIUsageSpawnThrottleKey, and ClaudeCLIBackgroundMarker
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
    // 1. Live Claude PTY sample with ANSI redraw, Sonnet, Account, Org, Login method
    let live_sample = concat!(
        "\u{1b}[?2026h\u{1b}[?25l\n",
        " \u{1b}[0mSettings:  Status   Config   \u{1b}[1;36mUsage\u{1b}[0m  (tab to cycle)\n\n",
        " \u{1b}[1mCurrent session\u{1b}[22m\n",
        " \u{1b}[48;5;24m \u{1b}[49m                                                  4% used\n",
        " Resets 12:59pm (Europe/Vienna)\n\n",
        " \u{1b}[1mCurrent week (all models)\u{1b}[22m\n",
        " \u{1b}[48;5;24m██████████████\u{1b}[49m                                     29% used\n",
        " Resets Nov 21 at 4:59am (Europe/Vienna)\n\n",
        " \u{1b}[1mCurrent week (Sonnet)\u{1b}[22m\n",
        " \u{1b}[48;5;24m\u{1b}[49m                                                   0% used\n\n",
        " Account: user@example.com\n",
        " Org: Example Org\n",
        " Login method: Claude Max Account\n"
    );
    let snap = ClaudeStatusProbeParser::parse(live_sample, None).unwrap();
    assert_eq!(snap.session_percent_left, Some(96));
    assert_eq!(snap.weekly_percent_left, Some(71));
    assert_eq!(snap.opus_percent_left, Some(100));
    assert_eq!(snap.account_email.as_deref(), Some("user@example.com"));
    assert_eq!(snap.account_organization.as_deref(), Some("Example Org"));
    assert_eq!(snap.login_method.as_deref(), Some("Max"));
    assert_eq!(
        snap.primary_reset_description.as_deref(),
        Some("Resets 12:59pm (Europe/Vienna)")
    );
    assert_eq!(
        snap.secondary_reset_description.as_deref(),
        Some("Resets Nov 21 at 4:59am (Europe/Vienna)")
    );

    // 2. Enterprise usage without weekly or opus limits (0% used -> 100% left)
    let enterprise = concat!(
        "Settings: Status Config Usage (tab to cycle)\n\n",
        "Current session\n",
        "0% used\n",
        "Resets 3am (Europe/London)\n\n",
        "Esc to exit\n"
    );
    let snap_ent = ClaudeStatusProbeParser::parse(enterprise, None).unwrap();
    assert_eq!(snap_ent.session_percent_left, Some(100));
    assert_eq!(snap_ent.weekly_percent_left, None);
    assert_eq!(snap_ent.opus_percent_left, None);
    assert_eq!(
        snap_ent.primary_reset_description.as_deref(),
        Some("Resets 3am (Europe/London)")
    );

    // 3. Fuzzy all-models weekly usage (`all modls`) + garbled `t` reset
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

    // 4. Compact resets without spaces after `Resets`
    let compact = concat!(
        "Settings: Status Config Usage (tab to cycle)\n\n",
        "Current session\n",
        "6% used\n",
        "Resets4:29am(Asia/Calcutta)\n\n",
        "Current week (all models)\n",
        "44% used\n",
        "ResetsMar13at12:30pm(Asia/Calcutta)\n\n",
        "Current week (Sonnet only)\n",
        "5% used\n",
        "ResetsMar13at8:30pm(Asia/Calcutta)\n"
    );
    let snap_compact = ClaudeStatusProbeParser::parse(compact, None).unwrap();
    assert_eq!(snap_compact.session_percent_left, Some(94));
    assert_eq!(snap_compact.weekly_percent_left, Some(56));
    assert_eq!(snap_compact.opus_percent_left, Some(95));
    assert_eq!(
        snap_compact.primary_reset_description.as_deref(),
        Some("Resets4:29am(Asia/Calcutta)")
    );
    assert_eq!(
        snap_compact.secondary_reset_description.as_deref(),
        Some("ResetsMar13at12:30pm(Asia/Calcutta)")
    );
    assert_eq!(
        snap_compact.opus_reset_description.as_deref(),
        Some("ResetsMar13at8:30pm(Asia/Calcutta)")
    );

    // 5. CR-delimited captures (`\r` without `\n`)
    let cr_capture = concat!(
        "Current session\r",
        "████████████ 30% left\r",
        "Resets 11am (UTC)\r",
        "Current week (all models)\r",
        "████████ 45% left\r",
        "Resets Nov 21 at 5am (UTC)\r",
        "Current week (Fable)\r",
        "██ 15% used\r",
        "Resets Nov 21 at 5am (UTC)\r",
        "Account: cr-user@example.com\r",
        "Org: CR Org\r",
        "Login method: Claude Pro\r"
    );
    let snap_cr = ClaudeStatusProbeParser::parse(cr_capture, None).unwrap();
    assert_eq!(snap_cr.session_percent_left, Some(30));
    assert_eq!(snap_cr.weekly_percent_left, Some(45));
    assert_eq!(
        snap_cr.primary_reset_description.as_deref(),
        Some("Resets 11am (UTC)")
    );
    assert_eq!(
        snap_cr.secondary_reset_description.as_deref(),
        Some("Resets Nov 21 at 5am (UTC)")
    );
    assert_eq!(
        snap_cr.account_email.as_deref(),
        Some("cr-user@example.com")
    );
    assert_eq!(snap_cr.account_organization.as_deref(), Some("CR Org"));
    assert_eq!(snap_cr.login_method.as_deref(), Some("Pro"));
    assert_eq!(snap_cr.extra_rate_windows.len(), 1);
    assert_eq!(snap_cr.extra_rate_windows[0].title, "Fable only");
    assert_eq!(snap_cr.extra_rate_windows[0].window.used_percent, 15.0);

    // 6. Trimming to latest Settings: ... Usage panel ignores earlier scrollback numbers
    let scrollback = concat!(
        "Fixed 100% of lint issues in build log\n",
        "Settings: Status Config Usage\n",
        "Current session\n",
        "12% used\n",
        "Current week (all models)\n",
        "20% used\n"
    );
    let snap_scroll = ClaudeStatusProbeParser::parse(scrollback, None).unwrap();
    assert_eq!(snap_scroll.session_percent_left, Some(88));
    assert_eq!(snap_scroll.weekly_percent_left, Some(80));

    // 7. Usage errors: token expired, authentication_error, rate_limit_error JSON, generic failure, folder trust
    let expired_err = ClaudeStatusProbeParser::parse(
        "Failed to load usage data: {\"error\":{\"message\":\"OAuth token has expired.\",\"details\":{\"error_code\":\"token_expired\"}}}",
        None,
    )
    .unwrap_err();
    assert_eq!(
        expired_err,
        ClaudeStatusProbeError::AuthenticationFailed(
            "OAuth token has expired. (token_expired). Run `claude login` to refresh.".to_owned()
        )
    );

    let auth_err = ClaudeStatusProbeParser::parse(
        "Failed to load usage data: {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"Invalid bearer\"}}",
        None,
    )
    .unwrap_err();
    assert_eq!(
        auth_err,
        ClaudeStatusProbeError::AuthenticationFailed(
            "Claude CLI authentication error: Invalid bearer. Run `claude login`.".to_owned()
        )
    );

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

    let rate_limit_json_err = ClaudeStatusProbeParser::parse(
        "Failed to load usage data: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"This request would exceed your account's rate limit. Please try again later.\"}}",
        None,
    )
    .unwrap_err();
    assert!(rate_limit_json_err.is_rate_limit_error());

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

    // Trust prompt with folder on next line vs same-line text
    let trust_with_folder = ClaudeStatusProbeParser::parse(
        "Do you trust the files in this folder?\n/Users/example/project\n",
        None,
    )
    .unwrap_err();
    assert!(
        trust_with_folder
            .to_string()
            .contains("folder trust prompt (/Users/example/project)")
    );

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

    // 8. `is_retryable` and `is_rate_limit_error` exact semantics
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

    // 9. `clean_plan_name` and login method extraction
    assert_eq!(
        ClaudeStatusProbeParser::clean_plan_name("\u{1b}[32mClaude Max Account\u{1b}[0m"),
        "Max"
    );
    assert_eq!(ClaudeStatusProbeParser::clean_plan_name("oauth"), "Ollama");
    let d_today_bridge = "Use Claude\nd → today\nCurrent session\n10% used";
    let snap_bridge = ClaudeStatusProbeParser::parse(d_today_bridge, None).unwrap();
    assert_eq!(snap_bridge.login_method, None);

    // 10. ESC followed by newline does not swallow the newline
    assert_eq!(
        ClaudeCLIScreen::render("line1\u{1b}\nline2"),
        "line1\nline2"
    );
    // Unqualified U+26A0 (`⚠`) is 1 cell wide, while `⚠️` (`U+26A0 U+FE0F`) is 2 cells wide
    assert_eq!(ClaudeCLIScreen::render("⚠ ok\u{1b}[3GX"), "⚠ Xk");
    assert_eq!(ClaudeCLIScreen::render("⚠️ ok\u{1b}[4GX"), "⚠️ Xk");
}

#[test]
fn scoped_weekly_usage_and_usage_insights_parity() {
    let now = "2026-04-06T09:00:00Z".parse::<Timestamp>().unwrap();

    // 1. Later complete scoped panel replaces earlier complete value (`ClaudeCLIScopedWeeklyUsageTests.swift:228-248`)
    let multi_panel = concat!(
        "Current session\n",
        "10% used\n",
        "Current week (all models)\n",
        "25% used\n",
        "Current week (Fable)\n",
        "20% used\n",
        "Current week (Fable)\n",
        "40% used\n",
        "Resets Apr 10 at 5pm (UTC)\n",
        "Current week (Opus 4.6)\n",
        "60% used\n",
        "Current week (Sonnet)\n",
        "15% used\n"
    );
    let snap = ClaudeStatusProbeParser::parse_at(multi_panel, None, now, &TimeZone::UTC).unwrap();
    assert_eq!(snap.session_percent_left, Some(90));
    assert_eq!(snap.weekly_percent_left, Some(75));
    assert_eq!(snap.opus_percent_left, Some(85));
    let ids: Vec<&str> = snap
        .extra_rate_windows
        .iter()
        .map(|w| w.id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            "claude-weekly-scoped-fable",
            "claude-weekly-scoped-opus-4-6"
        ]
    );
    assert_eq!(snap.extra_rate_windows[0].title, "Fable only");
    assert_eq!(snap.extra_rate_windows[0].window.used_percent, 40.0);
    assert_eq!(
        snap.extra_rate_windows[0].window.resets_at,
        Some("2026-04-10T17:00:00Z".parse::<Timestamp>().unwrap())
    );
    assert_eq!(snap.extra_rate_windows[1].title, "Opus 4.6 only");
    assert_eq!(snap.extra_rate_windows[1].window.used_percent, 60.0);

    // Model name in CLI panel already ending with "only" is not doubled to "only only"
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

    // Model name already ending with "only" is not doubled to "only only"
    let already_only =
        ClaudeScopedWeeklyLimitMapper::extra_rate_window("Fable only", 35.0, None, None).unwrap();
    assert_eq!(already_only.id, "claude-weekly-scoped-fable-only");
    assert_eq!(already_only.title, "Fable only");

    // 2. Web extras merge deduplicates unambiguous scoped weekly windows and preserves ambiguous ones
    let cli_extra = vec![NamedRateWindow::new(
        "claude-weekly-scoped-fable",
        "Fable only",
        RateWindow::new(40.0, Some(10080), None, None, None, false).unwrap(),
    )];
    let web_unambiguous = ClaudeScopedWeeklyLimitMapper::named_windows(&[OAuthScopedLimitEntry {
        kind: Some("weekly_scoped".to_owned()),
        group: Some("weekly".to_owned()),
        percent: Some(42.0),
        resets_at: None,
        model_id: Some("claude-fable-5-1".to_owned()),
        model_name: Some("Fable".to_owned()),
    }]);
    let merged_unambiguous = merge_extra_rate_windows(&cli_extra, &web_unambiguous);
    assert_eq!(
        merged_unambiguous.len(),
        1,
        "unambiguous web scoped window for the same model must be deduplicated"
    );

    let web_ambiguous = ClaudeScopedWeeklyLimitMapper::named_windows(&[
        OAuthScopedLimitEntry {
            kind: Some("weekly_scoped".to_owned()),
            group: Some("weekly".to_owned()),
            percent: Some(42.0),
            resets_at: None,
            model_id: Some("claude-fable-5-0".to_owned()),
            model_name: Some("Fable".to_owned()),
        },
        OAuthScopedLimitEntry {
            kind: Some("weekly_scoped".to_owned()),
            group: Some("weekly".to_owned()),
            percent: Some(55.0),
            resets_at: None,
            model_id: Some("claude-fable-5-1".to_owned()),
            model_name: Some("Fable".to_owned()),
        },
    ]);
    let merged_ambiguous = merge_extra_rate_windows(&cli_extra, &web_ambiguous);
    assert_eq!(
        merged_ambiguous.len(),
        3,
        "ambiguous multiple web windows sharing a model title must be preserved"
    );

    // 3. ClaudeUsageInsightsTests: subscription notice + insights marker returns insights message,
    // while valid quota rows + insights marker succeeds without treating insight percentages as session quota.
    let insights_only = concat!(
        "You are currently using your subscription to power your Claude Code usage.\n",
        "What's contributing to your limits usage?\n",
        "  • 45% CLAUDE.md and project context\n",
        "  • 30% MCP server tool definitions\n"
    );
    let insights_err = ClaudeStatusProbeParser::parse(insights_only, None).unwrap_err();
    assert_eq!(
        insights_err,
        ClaudeStatusProbeError::ParseFailed(
            "Claude CLI /usage returned usage insights without quota data.".to_owned()
        )
    );

    let insights_with_quota = concat!(
        "Current session\n",
        "12% used\n",
        "Current week (all models)\n",
        "34% used\n",
        "What's contributing to your limits usage?\n",
        "  • 45% CLAUDE.md and project context\n",
        "  • 30% MCP server tool definitions\n"
    );
    let snap_insights = ClaudeStatusProbeParser::parse(insights_with_quota, None).unwrap();
    assert_eq!(snap_insights.session_percent_left, Some(88));
    assert_eq!(snap_insights.weekly_percent_left, Some(66));
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
