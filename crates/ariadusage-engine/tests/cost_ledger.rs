// Ported from CodexBar Tests/CodexBarTests/CostUsageWindowSummaryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsageStoreTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsageStoreCutoverTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::fs;
use std::path::Path;

use ariadusage_engine::cost::ledger::{
    self, CostUsageDailyEntry, CostUsageDailyReport, comparison_summaries, summary_for_last_days,
};
use ariadusage_engine::cost::store::{
    CostUsageStore, CostUsageStoreDayAggregate, CostUsageStoreMetadata,
};
use ariadusage_engine::cost::timestamp::utc_ymd_hms;
use ariadusage_engine::cost::{
    CostScannerOptions, LocalCostProvider, load_cost_history, load_daily_report,
};
use ariadusage_protocol::usage::{CostDailyEntry, CostModelBreakdown, CostProvenance};
use jiff::tz::TimeZone;
use serde_json::json;
use tempfile::TempDir;

fn make_entry(
    day: &str,
    cost: Option<f64>,
    tokens: Option<u64>,
    requests: Option<u64>,
) -> CostDailyEntry {
    CostDailyEntry {
        date: day.to_owned(),
        total_tokens: tokens,
        total_requests: requests,
        cost_usd: cost,
        models_used: Vec::new(),
        model_breakdowns: Vec::new(),
    }
}

// Upstream test ports (each marker identifies one @Test declaration).

// upstream: CostUsageWindowSummaryTests.swift:6 overflow invalidates only its total while unavailable rows stay absent
#[test]
fn overflow_invalidates_only_its_total_while_unavailable_rows_stay_absent() {
    let now = utc_ymd_hms(2026, 7, 1, 0, 0, 0);
    let daily = vec![
        make_entry("2026-06-29", Some(1.0), Some(u64::MAX), Some(3)),
        make_entry("2026-06-30", None, None, None),
        make_entry("2026-07-01", Some(2.0), Some(1), Some(4)),
    ];
    let summary = summary_for_last_days(&daily, 7, now, Some(TimeZone::UTC));
    assert_eq!(summary.total_tokens, None);
    assert_eq!(summary.total_requests, Some(7));
    assert_eq!(summary.cost_usd, Some(3.0));
}

// upstream: CostUsageWindowSummaryTests.swift:25 summaries use calendar windows instead of the last nonempty rows
#[test]
fn summaries_use_calendar_windows_instead_of_the_last_nonempty_rows() {
    let now = utc_ymd_hms(2026, 7, 1, 0, 0, 0);
    let daily = vec![
        make_entry("2026-06-01", Some(1.0), Some(100), Some(1)),
        make_entry("2026-06-25", Some(4.0), Some(400), Some(4)),
        make_entry("2026-07-01", Some(5.0), Some(500), Some(5)),
    ];

    let summary_7 = summary_for_last_days(&daily, 7, now, Some(TimeZone::UTC));
    assert_eq!(summary_7.days, 7);
    assert_eq!(summary_7.entry_count, Some(2));
    assert_eq!(summary_7.cost_usd, Some(9.0));
    assert_eq!(summary_7.total_tokens, Some(900));
    assert_eq!(summary_7.total_requests, Some(9));
}

// upstream: CostUsageWindowSummaryTests.swift:37 comparison periods are unique sorted and bounded by scanned history
#[test]
fn comparison_periods_are_unique_sorted_and_bounded_by_scanned_history() {
    let now = utc_ymd_hms(2026, 7, 1, 0, 0, 0);
    let daily = vec![
        make_entry("2026-06-01", Some(1.0), Some(100), Some(1)),
        make_entry("2026-06-25", Some(4.0), Some(400), Some(4)),
        make_entry("2026-07-01", Some(5.0), Some(500), Some(5)),
    ];

    let comparisons = comparison_summaries(&daily, &[30, 7, 90, 7], 90, now, Some(TimeZone::UTC));
    let days: Vec<u32> = comparisons.iter().map(|c| c.days).collect();
    assert_eq!(days, vec![7, 30]);
}

// upstream: CostUsageWindowSummaryTests.swift:47 summary preserves unavailable totals as nil
#[test]
fn summary_preserves_unavailable_totals_as_nil() {
    let now = utc_ymd_hms(2026, 7, 1, 0, 0, 0);
    let daily = vec![make_entry("2026-07-01", None, None, None)];
    let summary = summary_for_last_days(&daily, 7, now, Some(TimeZone::UTC));
    assert_eq!(summary.cost_usd, None);
    assert_eq!(summary.total_tokens, None);
    assert_eq!(summary.total_requests, None);
}

// upstream: CostUsageWindowSummaryTests.swift:64 comparison summaries keep Gregorian entries under a Buddhist calendar
#[test]
fn comparison_summaries_keep_gregorian_entries_under_a_buddhist_calendar() {
    let bangkok = TimeZone::get("Asia/Bangkok").unwrap_or(TimeZone::UTC);
    // 2026-07-23 12:00:00 in Asia/Bangkok (+07:00) is 2026-07-23 05:00:00 UTC
    let now = utc_ymd_hms(2026, 7, 23, 5, 0, 0);
    let daily = vec![
        make_entry("2026-06-23", Some(1.0), Some(100), Some(1)),
        make_entry("2026-06-24", Some(4.0), Some(400), Some(4)),
        make_entry("2026-07-16", Some(1.0), Some(100), Some(1)),
        make_entry("2026-07-17", Some(2.0), Some(200), Some(2)),
        make_entry("2026-07-23", Some(3.0), Some(300), Some(3)),
    ];

    let summaries = comparison_summaries(&daily, &[7, 30], 90, now, Some(bangkok));
    let seven_days = summaries
        .iter()
        .find(|s| s.days == 7)
        .expect("7-day summary");
    let thirty_days = summaries
        .iter()
        .find(|s| s.days == 30)
        .expect("30-day summary");

    assert_eq!(seven_days.entry_count, Some(2));
    assert_eq!(seven_days.cost_usd, Some(5.0));
    assert_eq!(seven_days.total_tokens, Some(500));
    assert_eq!(thirty_days.entry_count, Some(4));
    assert_eq!(thirty_days.cost_usd, Some(10.0));
    assert_eq!(thirty_days.total_tokens, Some(1000));
}

// upstream: CostUsageStoreCutoverTests.swift:47 fixture report remains identical through the store
#[test]
fn fixture_report_remains_identical_through_the_store() {
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures/codexbar/cost-usage/issue-2037/archived-fork-33ce-3869");
    let manifest_bytes = fs::read(fixture_root.join("manifest.json")).expect("read manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_slice(&manifest_bytes).expect("decode manifest.json");

    let tmp = TempDir::new().expect("tempdir");
    let cache_root = tmp.path().join("cache");
    for file in manifest["files"].as_array().expect("manifest.files") {
        let rel = file["relativePath"].as_str().expect("relativePath");
        let src = fixture_root.join(rel);
        let dst = tmp.path().join(rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::copy(&src, &dst).unwrap();
    }
    let codex_home = tmp.path().join("codex-home");

    let since = utc_ymd_hms(2030, 1, 1, 12, 0, 0);
    let until = utc_ymd_hms(2030, 1, 2, 12, 0, 0);
    let scanned = load_daily_report(
        LocalCostProvider::Codex,
        since,
        until,
        until,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(cache_root.clone()),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );

    let mut store = CostUsageStore::new(Some(&cache_root));
    let (stored, _) = store
        .load_provider_report("codex")
        .expect("stored codex report");
    assert_eq!(stored.data, scanned.data);
    assert_eq!(stored.summary, scanned.summary);
}

// upstream: CostUsageStoreTests.swift:44 database lives beside the legacy artifact directory
#[test]
fn database_lives_beside_the_legacy_artifact_directory() {
    let tmp = TempDir::new().expect("tempdir");
    let store = CostUsageStore::new(Some(tmp.path()));

    assert_eq!(
        store.database_path().file_name().and_then(|s| s.to_str()),
        Some("cost-usage.sqlite")
    );
    assert_eq!(
        store
            .database_path()
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str()),
        Some("cost-usage")
    );
}

fn init_store_and_read_pragmas(store: &mut CostUsageStore) -> (String, i64, i64, i32) {
    let _ = store.load_metadata();
    let conn = rusqlite::Connection::open(store.database_path()).unwrap();
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    let foreign_keys: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    let auto_vacuum: i64 = conn
        .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
        .unwrap();
    let user_version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    (journal_mode, foreign_keys, auto_vacuum, user_version)
}

// upstream: CostUsageStoreTests.swift:54 new database uses WAL
#[test]
fn new_database_uses_wal() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    let (journal_mode, _, _, _) = init_store_and_read_pragmas(&mut store);

    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
}

// upstream: CostUsageStoreTests.swift:63 new database configures busy timeout
#[test]
fn new_database_configures_busy_timeout() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    let configuration = store.configuration().unwrap();
    assert_eq!(configuration.busy_timeout_milliseconds, 5000);
}

// upstream: CostUsageStoreTests.swift:72 new database accepts a fixture busy timeout
#[test]
fn new_database_accepts_a_fixture_busy_timeout() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::with_versions(
        Some(tmp.path()),
        ariadusage_engine::cost::store::combined_schema_version(
            ariadusage_engine::cost::store::BASE_SCHEMA_VERSION,
            ariadusage_engine::cost::store::DEFAULT_PARSER_HASH,
        ),
        ariadusage_engine::cost::store::DEFAULT_PARSER_HASH,
        25,
    );
    let configuration = store.configuration().unwrap();
    assert_eq!(configuration.busy_timeout_milliseconds, 25);
}

// upstream: CostUsageStoreTests.swift:83 new database enables foreign keys
#[test]
fn new_database_enables_foreign_keys() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    let configuration = store.configuration().unwrap();
    assert!(configuration.foreign_keys_enabled);
}

// upstream: CostUsageStoreTests.swift:92 new database uses incremental auto vacuum
#[test]
fn new_database_uses_incremental_auto_vacuum() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    let (_, _, auto_vacuum, _) = init_store_and_read_pragmas(&mut store);

    assert_eq!(auto_vacuum, 2);
}

// upstream: CostUsageStoreTests.swift:101 user version combines schema and parser hash
#[test]
fn user_version_combines_schema_and_parser_hash() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    let (_, _, _, user_version) = init_store_and_read_pragmas(&mut store);

    assert_eq!(
        user_version,
        ariadusage_engine::cost::store::combined_schema_version(
            ariadusage_engine::cost::store::BASE_SCHEMA_VERSION,
            ariadusage_engine::cost::store::DEFAULT_PARSER_HASH,
        )
    );
    assert_ne!(
        ariadusage_engine::cost::store::combined_schema_version(1, "a"),
        ariadusage_engine::cost::store::combined_schema_version(1, "b")
    );
}

// upstream: CostUsageStoreTests.swift:112 schema contains phase two query indexes
#[test]
fn schema_contains_phase_two_query_indexes() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    let _ = store.load_metadata();

    let conn = rusqlite::Connection::open(store.database_path()).unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'index'")
        .unwrap();
    let indexes: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(indexes.iter().any(|n| n == "files_path_idx"));
    assert!(indexes.iter().any(|n| n == "usage_rows_file_idx"));
    assert!(indexes.iter().any(|n| n == "day_aggregates_day_idx"));
}

// Rust-added tests (no direct upstream @Test equivalent; excluded from parity totals).

// Rust-added: Cost history populates today last 30 days comparison windows and top model.
#[test]
fn cost_history_populates_today_last_30_days_comparison_windows_and_top_model() {
    let tmp = TempDir::new().expect("tempdir");
    let projects_root = tmp.path().join("claude-projects");
    fs::create_dir_all(projects_root.join("proj")).unwrap();

    let now = utc_ymd_hms(2026, 5, 10, 15, 0, 0);
    let entries = vec![
        json!({
            "type": "assistant",
            "timestamp": "2026-05-08T12:00:00Z",
            "requestId": "req-1",
            "message": {
                "id": "msg-1",
                "model": "claude-sonnet-4-20250514",
                "usage": {"input_tokens": 10_000, "output_tokens": 1_000}
            }
        }),
        json!({
            "type": "assistant",
            "timestamp": "2026-05-10T12:00:00Z",
            "requestId": "req-2",
            "message": {
                "id": "msg-2",
                "model": "claude-opus-4-5-20251101",
                "usage": {"input_tokens": 20_000, "output_tokens": 2_000}
            }
        }),
    ];
    let mut text = String::new();
    for e in entries {
        text.push_str(&serde_json::to_string(&e).unwrap());
        text.push('\n');
    }
    fs::write(projects_root.join("proj/sess.jsonl"), text).unwrap();

    let history = load_cost_history(
        LocalCostProvider::Claude,
        now,
        30,
        &CostScannerOptions {
            claude_projects_roots: vec![projects_root],
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );

    assert_eq!(
        history.cost_provenance,
        Some(CostProvenance::ListPriceEstimate)
    );
    assert_eq!(history.daily.len(), 2);
    assert_eq!(history.session_tokens, Some(22_000));
    assert_eq!(history.session_requests, Some(1));
    assert_eq!(history.last_30_days_tokens, Some(33_000));
    assert_eq!(history.last_30_days_requests, Some(2));
    assert!(history.session_cost_usd.unwrap_or(0.0) > 0.0);
    assert!(history.last_30_days_cost_usd.unwrap_or(0.0) > history.session_cost_usd.unwrap_or(0.0));
    assert_eq!(history.comparison_windows.len(), 1);
    assert_eq!(history.comparison_windows[0].days, 7);
    assert_eq!(
        history.top_model.as_ref().map(|m| m.model_name.as_str()),
        Some("claude-opus-4-5")
    );
}

// Rust-added: Cost usage store persists and rebuilds on schema mismatch.
#[test]
fn cost_usage_store_persists_and_rebuilds_on_schema_mismatch() {
    let tmp = TempDir::new().expect("tempdir");
    let cache_root = tmp.path().join("cache");
    let mut store = CostUsageStore::new(Some(&cache_root));

    let meta = CostUsageStoreMetadata {
        last_scan_unix_ms: 1_700_000_000_000,
        scan_since_day: Some("2026-05-01".to_owned()),
        scan_until_day: Some("2026-05-10".to_owned()),
        time_zone_identifier: Some("UTC".to_owned()),
        pricing_key: Some("pricing-v4".to_owned()),
        priority_metadata_key: None,
        priority_turns_cursor: None,
        catch_up_pending: false,
        processed_bytes: Some(1024),
        total_bytes: Some(1024),
        completed_files: Some(1),
        total_files: Some(1),
    };
    assert!(store.save_metadata(&meta));
    assert_eq!(store.load_metadata().as_ref(), Some(&meta));

    let mut agg = CostUsageStoreDayAggregate::zero("2026-05-10", "gpt-5.2-codex");
    agg.input_tokens = 500;
    agg.output_tokens = 100;
    agg.request_count = 2;
    assert!(store.save_day_aggregates(&[agg.clone()]));
    let loaded_aggs = store.load_day_aggregates(Some("2026-05-10"), Some("2026-05-10"));
    assert_eq!(loaded_aggs, vec![agg]);

    let report = CostUsageDailyReport {
        data: vec![CostUsageDailyEntry {
            date: "2026-05-10".to_owned(),
            input_tokens: Some(500),
            output_tokens: Some(100),
            cache_read_tokens: Some(0),
            cache_creation_tokens: None,
            total_tokens: Some(600),
            request_count: Some(2),
            priced_request_count: Some(2),
            unpriced_request_count: None,
            unmetered_request_count: None,
            cost_usd: Some(0.05),
            models_used: vec!["gpt-5.2-codex".to_owned()],
            model_breakdowns: vec![CostModelBreakdown {
                model_name: "gpt-5.2-codex".to_owned(),
                cost_usd: Some(0.05),
                total_tokens: Some(600),
                standard_cost_usd: None,
                priority_cost_usd: None,
                standard_tokens: None,
                priority_tokens: None,
            }],
        }],
        summary: None,
        priority_data_is_partial: false,
    };
    assert!(store.save_provider_report("codex", &report, 1_700_000_000_000));
    let (loaded_report, ts) = store.load_provider_report("codex").expect("load report");
    assert_eq!(ts, 1_700_000_000_000);
    assert_eq!(loaded_report, report);

    // Opening with a different parser hash triggers clean rebuild
    let mut store_v2 =
        CostUsageStore::with_versions(Some(&cache_root), 999, "different_hash", 1_000);
    assert_eq!(store_v2.load_metadata(), None);
    assert_eq!(store_v2.rebuild_count(), 1);
    let protocol_entries: Vec<CostDailyEntry> = report
        .data
        .iter()
        .map(CostUsageDailyEntry::to_protocol_entry)
        .collect();
    assert!(ledger::compute_top_model(&protocol_entries).is_some());
}

// Rust-added: Legacy artifact cleanup deletes json and rebuilds SQLite from source.
#[test]
fn legacy_artifact_cleanup_deletes_json_and_rebuilds_sqlite_from_source() {
    let tmp = TempDir::new().expect("tempdir");
    let cache_root = tmp.path().join("cache");
    let codex_home = tmp.path().join("codex-home");
    let day_dir = codex_home.join("sessions/2026/08/08");
    fs::create_dir_all(&day_dir).unwrap();

    let ts = "2026-08-08T12:00:00Z";
    let session_lines = [
        json!({
            "type": "session_meta",
            "timestamp": ts,
            "payload": {"session_id": "current"}
        }),
        json!({
            "type": "turn_context",
            "timestamp": ts,
            "payload": {"model": "openai/gpt-5.4"}
        }),
        json!({
            "type": "event_msg",
            "timestamp": ts,
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "openai/gpt-5.4",
                    "total_token_usage": {
                        "input_tokens": 7,
                        "cached_input_tokens": 0,
                        "output_tokens": 0
                    }
                }
            }
        }),
    ];
    let mut text = String::new();
    for line in &session_lines {
        text.push_str(&serde_json::to_string(line).unwrap());
        text.push('\n');
    }
    fs::write(day_dir.join("current.jsonl"), text).unwrap();

    let mut store = CostUsageStore::new(Some(&cache_root));
    assert!(store.upsert_file_path("/stale.jsonl", Some("stale"), 1, 1, 1));
    let store_dir = store.database_path().parent().unwrap().to_path_buf();
    fs::create_dir_all(&store_dir).unwrap();
    let legacy = store_dir.join("codex-v11.json");
    let temporary = store_dir.join(".codex-v11.json.fixture.tmp");
    fs::write(&legacy, b"legacy").unwrap();
    fs::write(&temporary, b"temporary").unwrap();

    let day = utc_ymd_hms(2026, 8, 8, 12, 0, 0);
    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(cache_root.clone()),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );

    assert_eq!(
        report.summary.as_ref().and_then(|s| s.total_input_tokens),
        Some(7)
    );
    assert!(!legacy.exists());
    assert!(!temporary.exists());
    assert!(store.database_path().exists());

    let mut reopened = CostUsageStore::new(Some(&cache_root));
    let paths = reopened.stored_file_paths();
    assert!(!paths.contains(&"/stale.jsonl".to_owned()));
    assert!(paths.iter().any(|p| p.ends_with("current.jsonl")));
}

// Rust-added: Cost usage store writes 0700 dir and 0600 files and rejects symlinks without rebuilding.
#[cfg(unix)]
#[test]
fn cost_usage_store_writes_0700_dir_and_0600_files_and_rejects_symlinks_without_rebuilding() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let tmp = TempDir::new().expect("tempdir");
    let cache_root = tmp.path().join("cache");
    let mut store = CostUsageStore::new(Some(&cache_root));
    let meta = CostUsageStoreMetadata {
        last_scan_unix_ms: 1_700_000_000_000,
        scan_since_day: Some("2026-05-01".to_owned()),
        scan_until_day: Some("2026-05-10".to_owned()),
        time_zone_identifier: Some("UTC".to_owned()),
        pricing_key: Some("pricing-v4".to_owned()),
        priority_metadata_key: None,
        priority_turns_cursor: None,
        catch_up_pending: false,
        processed_bytes: Some(1024),
        total_bytes: Some(1024),
        completed_files: Some(1),
        total_files: Some(1),
    };
    assert!(store.save_metadata(&meta));

    let db_path = store.database_path().to_path_buf();
    let store_dir = db_path.parent().unwrap();
    let dir_mode = fs::symlink_metadata(store_dir)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "store dir mode must be 0700");
    let db_mode = fs::symlink_metadata(&db_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(db_mode, 0o600, "store db mode must be 0600");

    // Replace the database file with a symlink to a target file and verify:
    // 1. CostUsageStore refuses to open it
    // 2. It does NOT call rebuild_database() (which would unlink/clobber or increment rebuild_count)
    // 3. The symlink target remains untouched
    let victim = tmp.path().join("victim.txt");
    fs::write(&victim, b"DO_NOT_OVERWRITE").unwrap();
    let sym_cache = tmp.path().join("sym-cache");
    let mut sym_store = CostUsageStore::new(Some(&sym_cache));
    let sym_db_path = sym_store.database_path().to_path_buf();
    let sym_store_dir = sym_db_path.parent().unwrap();
    fs::create_dir_all(sym_store_dir).unwrap();
    fs::set_permissions(sym_store_dir, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&victim, &sym_db_path).unwrap();

    assert!(!sym_store.save_metadata(&meta));
    assert_eq!(sym_store.rebuild_count(), 0);
    assert_eq!(fs::read(&victim).unwrap(), b"DO_NOT_OVERWRITE");
}
