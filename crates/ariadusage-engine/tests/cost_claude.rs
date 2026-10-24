// Ported from CodexBar Tests/CodexBarTests/CostUsageScannerTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::fs;

use ariadusage_engine::cost::claude::{self, ClaudeLogProviderFilter, ClaudePathRole};
use ariadusage_engine::cost::pricing::{self, ClaudeCostTokens};
use ariadusage_engine::cost::timestamp::{self, DayRange, utc_ymd_hms};
use ariadusage_engine::cost::{CostScannerOptions, LocalCostProvider, load_daily_report};
use jiff::tz::TimeZone;
use serde_json::{Value, json};
use tempfile::TempDir;

fn write_jsonl(path: &std::path::Path, entries: &[Value]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    let mut text = String::new();
    for entry in entries {
        text.push_str(&serde_json::to_string(entry).expect("serialize json"));
        text.push('\n');
    }
    fs::write(path, text).expect("write jsonl");
}

// Upstream test ports (each marker identifies one @Test declaration).

// upstream: CostUsageScannerTests.swift:99 vertex daily report filters claude logs
#[test]
fn vertex_daily_report_filters_claude_logs() {
    let tmp = TempDir::new().expect("tempdir");
    let projects_root = tmp.path().join("claude-projects");
    let day = utc_ymd_hms(2025, 12, 20, 12, 0, 0);
    let iso0 = "2025-12-20T12:00:00Z";

    let vertex_entry = json!({
        "type": "assistant",
        "timestamp": iso0,
        "metadata": {
            "provider": "vertexai",
            "projectId": "vertex-project",
            "location": "us-central1"
        },
        "message": {
            "model": "claude-sonnet-4-20250514",
            "usage": {
                "input_tokens": 10,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "output_tokens": 5
            }
        }
    });
    let claude_entry = json!({
        "type": "assistant",
        "timestamp": iso0,
        "metadata": {
            "provider": "anthropic"
        },
        "message": {
            "model": "claude-sonnet-4-20250514",
            "usage": {
                "input_tokens": 200,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "output_tokens": 100
            }
        }
    });

    write_jsonl(
        &projects_root.join("project-a/session-a.jsonl"),
        &[vertex_entry, claude_entry],
    );

    let vertex_opts = CostScannerOptions {
        claude_projects_roots: vec![projects_root],
        cache_root: Some(tmp.path().join("cache")),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let report = load_daily_report(LocalCostProvider::VertexAi, day, day, day, &vertex_opts);
    assert_eq!(report.data.len(), 1);
    assert_eq!(report.data[0].input_tokens, Some(10));
    assert_eq!(report.data[0].output_tokens, Some(5));
    assert_eq!(report.data[0].total_tokens, Some(15));
}

// upstream: CostUsageScannerTests.swift:165 vertex daily report detects by vrtx id prefix
#[test]
fn vertex_daily_report_detects_by_vrtx_id_prefix() {
    let tmp = TempDir::new().expect("tempdir");
    let projects_root = tmp.path().join("claude-projects");
    let day = utc_ymd_hms(2025, 12, 20, 12, 0, 0);
    let iso0 = "2025-12-20T12:00:00Z";
    let iso1 = "2025-12-20T12:00:01Z";

    let vertex_entry = json!({
        "type": "assistant",
        "timestamp": iso0,
        "requestId": "req_vrtx_011CWjK86SWeFuXqZKUtgB1H",
        "message": {
            "id": "msg_vrtx_0154LUXjFVzQGUca3yK2RUeo",
            "model": "claude-opus-4-5-20251101",
            "usage": {
                "input_tokens": 100,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "output_tokens": 50
            }
        }
    });
    let claude_entry = json!({
        "type": "assistant",
        "timestamp": iso1,
        "requestId": "req_011CW7BFSFkbK9qJrV8kiptH",
        "message": {
            "id": "msg_0152zX6DsQYcwH1qiXi4B3y2",
            "model": "claude-opus-4-5-20251101",
            "usage": {
                "input_tokens": 200,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "output_tokens": 100
            }
        }
    });

    write_jsonl(
        &projects_root.join("project-a/session-a.jsonl"),
        &[vertex_entry, claude_entry],
    );

    let vertex_opts = CostScannerOptions {
        claude_projects_roots: vec![projects_root.clone()],
        cache_root: Some(tmp.path().join("cache")),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let vertex_report = load_daily_report(LocalCostProvider::VertexAi, day, day, day, &vertex_opts);
    assert_eq!(vertex_report.data.len(), 1);
    assert_eq!(vertex_report.data[0].input_tokens, Some(100));
    assert_eq!(vertex_report.data[0].output_tokens, Some(50));
    assert_eq!(vertex_report.data[0].total_tokens, Some(150));

    let claude_opts = CostScannerOptions {
        claude_projects_roots: vec![projects_root],
        claude_provider_filter: Some(ClaudeLogProviderFilter::ExcludeVertexAi),
        cache_root: Some(tmp.path().join("cache")),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let claude_report = load_daily_report(LocalCostProvider::Claude, day, day, day, &claude_opts);
    assert_eq!(claude_report.data.len(), 1);
    assert_eq!(claude_report.data[0].input_tokens, Some(200));
    assert_eq!(claude_report.data[0].output_tokens, Some(100));
    assert_eq!(claude_report.data[0].total_tokens, Some(300));
}

// upstream: CostUsageScannerTests.swift:246 claude report preserves per-request threshold pricing
#[test]
fn claude_report_preserves_per_request_threshold_pricing() {
    let tmp = TempDir::new().expect("tempdir");
    let projects_root = tmp.path().join("claude-projects");
    let day = utc_ymd_hms(2026, 5, 9, 12, 0, 0);
    let model = "claude-sonnet-4-5";

    let first_entry = json!({
        "type": "assistant",
        "timestamp": "2026-05-09T12:00:00Z",
        "requestId": "req_one",
        "message": {
            "id": "msg_one",
            "model": model,
            "usage": {
                "input_tokens": 150_000,
                "output_tokens": 0
            }
        }
    });
    let second_entry = json!({
        "type": "assistant",
        "timestamp": "2026-05-09T12:00:01Z",
        "requestId": "req_two",
        "message": {
            "id": "msg_two",
            "model": model,
            "usage": {
                "input_tokens": 150_000,
                "output_tokens": 0
            }
        }
    });

    write_jsonl(
        &projects_root.join("project-a/threshold.jsonl"),
        &[first_entry, second_entry],
    );

    let opts = CostScannerOptions {
        claude_projects_roots: vec![projects_root],
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let report = load_daily_report(LocalCostProvider::Claude, day, day, day, &opts);
    let expected_request_cost = pricing::claude_cost_usd(
        model,
        ClaudeCostTokens {
            input: 150_000,
            cache_read: 0,
            cache_creation: 0,
            cache_creation_1h: 0,
            output: 0,
        },
        None,
        None,
        None,
    )
    .unwrap_or(0.0);
    let aggregate_cost = pricing::claude_cost_usd(
        model,
        ClaudeCostTokens {
            input: 300_000,
            cache_read: 0,
            cache_creation: 0,
            cache_creation_1h: 0,
            output: 0,
        },
        None,
        None,
        None,
    )
    .unwrap_or(0.0);
    let expected_cost = expected_request_cost * 2.0;

    assert_eq!(report.data.len(), 1);
    assert_eq!(report.data[0].input_tokens, Some(300_000));
    let actual_cost = report.data[0].cost_usd.unwrap_or(0.0);
    assert!((actual_cost - expected_cost).abs() < 1e-6);
    assert!((actual_cost - aggregate_cost).abs() > 1e-6);
}

// upstream: CostUsageScannerTests.swift:321 claude parses large lines with usage at tail
#[test]
fn claude_parses_large_lines_with_usage_at_tail() {
    let tmp = TempDir::new().expect("tempdir");
    let projects_root = tmp.path().join("claude-projects");
    let file_path = projects_root.join("project-a/large-line.jsonl");
    let day = utc_ymd_hms(2025, 12, 28, 12, 0, 0);
    let large_text = "a".repeat(70_000);

    let assistant = json!({
        "type": "assistant",
        "timestamp": "2025-12-28T12:00:00Z",
        "message": {
            "model": "claude-sonnet-4-20250514",
            "content": [{"type": "text", "text": large_text}],
            "usage": {
                "input_tokens": 3714,
                "output_tokens": 1
            }
        }
    });
    write_jsonl(&file_path, &[assistant]);

    let opts = CostScannerOptions {
        claude_projects_roots: vec![projects_root],
        cache_root: Some(tmp.path().join("cache")),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let report = load_daily_report(LocalCostProvider::Claude, day, day, day, &opts);
    assert_eq!(report.data.len(), 1);
    assert_eq!(report.data[0].input_tokens, Some(3714));
    assert_eq!(report.data[0].output_tokens, Some(1));
    assert_eq!(report.data[0].total_tokens, Some(3715));
}

// upstream: CostUsageScannerTests.swift:366 claude daily report refreshes when file changes
#[test]
fn claude_daily_report_refreshes_when_file_changes() {
    let tmp = TempDir::new().expect("tempdir");
    let projects_root = tmp.path().join("claude-projects");
    let file_path = projects_root.join("project-a/session-a.jsonl");
    let day = utc_ymd_hms(2025, 12, 20, 12, 0, 0);
    let model = "claude-sonnet-4-20250514";

    let first = json!({
        "type": "assistant",
        "timestamp": "2025-12-20T12:00:00Z",
        "message": {
            "model": model,
            "usage": {
                "input_tokens": 200,
                "cache_creation_input_tokens": 50,
                "cache_read_input_tokens": 25,
                "output_tokens": 80
            }
        }
    });
    write_jsonl(&file_path, std::slice::from_ref(&first));

    let opts = CostScannerOptions {
        claude_projects_roots: vec![projects_root],
        cache_root: Some(tmp.path().join("cache")),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first_report = load_daily_report(LocalCostProvider::Claude, day, day, day, &opts);
    assert_eq!(first_report.data[0].total_tokens, Some(355));

    let second = json!({
        "type": "assistant",
        "timestamp": "2025-12-20T12:00:01Z",
        "message": {
            "model": model,
            "usage": {
                "input_tokens": 40,
                "cache_creation_input_tokens": 10,
                "cache_read_input_tokens": 5,
                "output_tokens": 20
            }
        }
    });
    write_jsonl(&file_path, &[first, second]);

    let second_report = load_daily_report(LocalCostProvider::Claude, day, day, day, &opts);
    assert_eq!(second_report.data[0].total_tokens, Some(430));
}

// upstream: CostUsageScannerTests.swift:905 claude incremental parsing reads appended lines only
#[test]
fn claude_incremental_parsing_reads_appended_lines_only() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("project-a/session-a.jsonl");
    let day = utc_ymd_hms(2025, 12, 20, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let model = "claude-sonnet-4-20250514";

    let first = json!({
        "type": "assistant",
        "timestamp": "2025-12-20T12:00:00Z",
        "message": {
            "model": model,
            "usage": {
                "input_tokens": 200,
                "cache_creation_input_tokens": 50,
                "cache_read_input_tokens": 25,
                "output_tokens": 80
            }
        }
    });
    write_jsonl(&file_path, std::slice::from_ref(&first));

    let first_parse = claude::parse_claude_file(
        &file_path,
        &range,
        ClaudeLogProviderFilter::All,
        0,
        ClaudePathRole::Parent,
        None,
    );
    assert!(first_parse.parsed_bytes > 0);

    let second = json!({
        "type": "assistant",
        "timestamp": "2025-12-20T12:00:01Z",
        "message": {
            "model": model,
            "usage": {
                "input_tokens": 40,
                "cache_creation_input_tokens": 10,
                "cache_read_input_tokens": 5,
                "output_tokens": 20
            }
        }
    });
    write_jsonl(&file_path, &[first, second]);

    let delta = claude::parse_claude_file(
        &file_path,
        &range,
        ClaudeLogProviderFilter::All,
        first_parse.parsed_bytes,
        ClaudePathRole::Parent,
        None,
    );
    assert_eq!(delta.rows.len(), 1);
    let row = &delta.rows[0];
    assert_eq!(row.day_key, "2025-12-20");
    assert_eq!(row.model, pricing::normalize_claude_model(model));
    assert_eq!(row.input, 40);
    assert_eq!(row.cache_read, 5);
    assert_eq!(row.cache_create, 10);
    assert_eq!(row.output, 20);
}

// upstream: CostUsageScannerTests.swift:974 day key from timestamp matches ISO parsing
#[test]
fn day_key_from_timestamp_matches_iso_parsing() {
    for ts in ["2025-12-20T23:59:59Z", "2025-12-20T23:59:59+02:00"] {
        let fast = timestamp::day_key_from_timestamp(ts, Some(TimeZone::UTC));
        assert!(fast.is_some());
    }
    assert_eq!(
        timestamp::day_key_from_timestamp("2025-12-20T23:59:59Z", Some(TimeZone::UTC))
            .map(|(d, _)| d.0),
        Some("2025-12-20".to_owned())
    );
    assert_eq!(
        timestamp::day_key_from_timestamp("2025-12-20T23:59:59+02:00", Some(TimeZone::UTC))
            .map(|(d, _)| d.0),
        Some("2025-12-20".to_owned())
    );
}

// upstream: CostUsageScannerTests.swift:988 claude deduplicates streaming chunks
#[test]
fn claude_deduplicates_streaming_chunks() {
    let tmp = TempDir::new().expect("tempdir");
    let projects_root = tmp.path().join("claude-projects");
    let day = utc_ymd_hms(2025, 12, 20, 12, 0, 0);

    let chunk = |ts: &str| {
        json!({
            "type": "assistant",
            "timestamp": ts,
            "requestId": "req_01XYZ789",
            "message": {
                "id": "msg_01ABC123",
                "model": "claude-sonnet-4-20250514",
                "usage": {
                    "input_tokens": 100,
                    "cache_creation_input_tokens": 50,
                    "cache_read_input_tokens": 25,
                    "output_tokens": 10
                }
            }
        })
    };
    write_jsonl(
        &projects_root.join("project-a/session-a.jsonl"),
        &[
            chunk("2025-12-20T12:00:00Z"),
            chunk("2025-12-20T12:00:01Z"),
            chunk("2025-12-20T12:00:02Z"),
        ],
    );

    let opts = CostScannerOptions {
        claude_projects_roots: vec![projects_root],
        cache_root: Some(tmp.path().join("cache")),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let report = load_daily_report(LocalCostProvider::Claude, day, day, day, &opts);
    assert_eq!(report.data.len(), 1);
    assert_eq!(report.data[0].input_tokens, Some(100));
    assert_eq!(report.data[0].cache_creation_tokens, Some(50));
    assert_eq!(report.data[0].cache_read_tokens, Some(25));
    assert_eq!(report.data[0].output_tokens, Some(10));
    assert_eq!(report.data[0].total_tokens, Some(185));
}

// upstream: CostUsageScannerTests.swift:1076 claude counts entries without ids as separate
#[test]
fn claude_counts_entries_without_ids_as_separate() {
    let tmp = TempDir::new().expect("tempdir");
    let projects_root = tmp.path().join("claude-projects");
    let day = utc_ymd_hms(2025, 12, 20, 12, 0, 0);

    let entry1 = json!({
        "type": "assistant",
        "timestamp": "2025-12-20T12:00:00Z",
        "message": {
            "model": "claude-sonnet-4-20250514",
            "usage": {
                "input_tokens": 100,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "output_tokens": 50
            }
        }
    });
    let entry2 = json!({
        "type": "assistant",
        "timestamp": "2025-12-20T12:00:01Z",
        "message": {
            "model": "claude-sonnet-4-20250514",
            "usage": {
                "input_tokens": 200,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "output_tokens": 100
            }
        }
    });
    write_jsonl(
        &projects_root.join("project-a/session-a.jsonl"),
        &[entry1, entry2],
    );

    let opts = CostScannerOptions {
        claude_projects_roots: vec![projects_root],
        cache_root: Some(tmp.path().join("cache")),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let report = load_daily_report(LocalCostProvider::Claude, day, day, day, &opts);
    assert_eq!(report.data.len(), 1);
    assert_eq!(report.data[0].input_tokens, Some(300));
    assert_eq!(report.data[0].output_tokens, Some(150));
    assert_eq!(report.data[0].total_tokens, Some(450));
}

// upstream: CostUsageScannerTests.swift:1140 claude counts different request ids separately
#[test]
fn claude_counts_different_request_ids_separately() {
    let tmp = TempDir::new().expect("tempdir");
    let projects_root = tmp.path().join("claude-projects");
    let day = utc_ymd_hms(2025, 12, 20, 12, 0, 0);

    let entry1 = json!({
        "type": "assistant",
        "timestamp": "2025-12-20T12:00:00Z",
        "requestId": "req_01AAA",
        "message": {
            "id": "msg_01ABC123",
            "model": "claude-sonnet-4-20250514",
            "usage": {
                "input_tokens": 10,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "output_tokens": 5
            }
        }
    });
    let entry2 = json!({
        "type": "assistant",
        "timestamp": "2025-12-20T12:00:01Z",
        "requestId": "req_01BBB",
        "message": {
            "id": "msg_01ABC123",
            "model": "claude-sonnet-4-20250514",
            "usage": {
                "input_tokens": 20,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 0,
                "output_tokens": 10
            }
        }
    });
    write_jsonl(
        &projects_root.join("project-a/session-a.jsonl"),
        &[entry1, entry2],
    );

    let opts = CostScannerOptions {
        claude_projects_roots: vec![projects_root],
        cache_root: Some(tmp.path().join("cache")),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let report = load_daily_report(LocalCostProvider::Claude, day, day, day, &opts);
    assert_eq!(report.data.len(), 1);
    assert_eq!(report.data[0].input_tokens, Some(30));
    assert_eq!(report.data[0].output_tokens, Some(15));
    assert_eq!(report.data[0].total_tokens, Some(45));
}

// Rust-added tests (no direct upstream @Test equivalent; excluded from parity totals).

// Rust-added: JSONL line debug redacts line bytes.
#[test]
fn jsonl_line_debug_redacts_line_bytes() {
    let line = claude::JsonlLine {
        bytes: b"{\"secret_prompt\":\"CONFIDENTIAL_PROMPT_CONTENT\"}".to_vec(),
        was_truncated: false,
        start_offset: 0,
        end_offset: 48,
    };
    let dbg = format!("{line:?}");
    assert!(!dbg.contains("CONFIDENTIAL_PROMPT_CONTENT"));
    assert!(dbg.contains("byte_len"));
}

// Rust-added: Claude scanner skips symlinks in walk and file open.
#[cfg(unix)]
#[test]
fn claude_scanner_skips_symlinks_in_walk_and_file_open() {
    use ariadusage_engine::cost::claude::ClaudeScanOptions;
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().expect("tempdir");
    let projects_root = tmp.path().join("claude-projects");
    let proj_dir = projects_root.join("project-a");
    fs::create_dir_all(&proj_dir).unwrap();

    let outside_dir = tmp.path().join("outside");
    fs::create_dir_all(&outside_dir).unwrap();
    let outside_jsonl = outside_dir.join("secret.jsonl");
    write_jsonl(
        &outside_jsonl,
        &[json!({
            "type": "assistant",
            "timestamp": "2025-12-20T12:00:00Z",
            "message": {
                "model": "claude-sonnet-4-20250514",
                "usage": {"input_tokens": 500, "output_tokens": 50}
            }
        })],
    );
    symlink(&outside_jsonl, proj_dir.join("linked.jsonl")).unwrap();
    symlink(&outside_dir, projects_root.join("linked-project")).unwrap();

    let day = utc_ymd_hms(2025, 12, 20, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let rows = claude::scan_claude_projects(
        &range,
        &ClaudeScanOptions {
            claude_projects_roots: vec![projects_root],
            provider_filter: ClaudeLogProviderFilter::All,
            timezone: Some(TimeZone::UTC),
            catalog: None,
        },
    );
    assert!(rows.is_empty());

    let direct = claude::parse_claude_file(
        &proj_dir.join("linked.jsonl"),
        &range,
        ClaudeLogProviderFilter::All,
        0,
        ClaudePathRole::Parent,
        None,
    );
    assert!(direct.rows.is_empty());
}
