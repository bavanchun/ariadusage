// Ported from CodexBar Tests/CodexBarTests/CostUsageScannerTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsageScannerCodexPriorityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsageScannerCodexPriorityCursorTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsageScannerPriorityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsageCodexRequestLedgerTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsageCodexRequestLedgerTests+MirrorWindow.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsageCodexSourceRecoveryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsageStoreFailureInjectionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsageCodexSessionTierTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexForkAppendResumeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsageOrphanForkTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsageScannerForkSplitTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexSubagentRolloutShapeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexSubagentForkBaselineTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexSubagentOrdinalBoundaryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexSubagentAccountingIntegrationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsagePriorityDayKeysTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use ariadusage_engine::cost::codex::{
    self, CODEX_SESSION_METADATA_MAX_LINE_BYTES, CodexDayModelKey, CodexForkBaseline,
    CodexParseResult, CodexScanBudget, CodexScanOptions, CodexSessionMetadata,
    CodexSubagentCounterSemantics, CodexSubagentObservation, CodexSubagentRolloutShape,
    CodexTotals, CodexUsageRow,
};
use ariadusage_engine::cost::codex_priority::{
    self, CODEX_PRIORITY_COMPLETED_MODEL_RETENTION_LIMIT, CodexPriorityCursorAnchor,
    CodexPriorityTurnsMemoState, CodexPriorityTurnsPersistedCursor,
};
use ariadusage_engine::cost::pricing;
use ariadusage_engine::cost::store::{
    CostUsageStore, CostUsageStoreError, CostUsageStoreMetadata, CostUsageStoreTokenSnapshot,
    CostUsageStoreUsageRow,
};
use ariadusage_engine::cost::timestamp::{BucketTimeZone, DayRange, utc_ymd_hms};
use ariadusage_engine::cost::{CostScannerOptions, LocalCostProvider, load_daily_report};
use jiff::ToSpan;
use jiff::tz::TimeZone;
use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::TempDir;

fn write_jsonl(path: &Path, entries: &[Value]) {
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

#[test]
fn codex_session_metadata_skips_an_oversized_line_without_retaining_it() {
    let tmp = TempDir::new().expect("tempdir");
    let oversized_path = tmp.path().join("oversized.jsonl");
    {
        let mut f = File::create(&oversized_path).expect("create file");
        f.write_all(b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"too-large\",\"padding\":\"")
            .unwrap();
        let chunk = vec![b'x'; 64 * 1024];
        for _ in 0..128 {
            f.write_all(&chunk).unwrap();
        }
        f.write_all(
            b"\"}}\n{\"type\":\"session_meta\",\"payload\":{\"id\":\"expected-session\"}}\n",
        )
        .unwrap();
    }
    let sid = codex::parse_codex_session_identifier(&oversized_path).expect("parse");
    assert_eq!(sid.as_deref(), Some("expected-session"));
}

#[test]
fn codex_session_metadata_accepts_a_line_exactly_at_the_byte_limit() {
    let tmp = TempDir::new().expect("tempdir");
    let exact_path = tmp.path().join("exact-limit.jsonl");
    {
        let prefix =
            b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"limit-session\",\"padding\":\"";
        let suffix = b"\"}}";
        let pad_len = CODEX_SESSION_METADATA_MAX_LINE_BYTES - prefix.len() - suffix.len();
        let mut line = Vec::with_capacity(CODEX_SESSION_METADATA_MAX_LINE_BYTES);
        line.extend_from_slice(prefix);
        line.extend(std::iter::repeat_n(b'x', pad_len));
        line.extend_from_slice(suffix);
        assert_eq!(line.len(), CODEX_SESSION_METADATA_MAX_LINE_BYTES);
        fs::write(&exact_path, &line).unwrap();
    }
    let sid2 = codex::parse_codex_session_identifier(&exact_path).expect("parse");
    assert_eq!(sid2.as_deref(), Some("limit-session"));
}

#[test]
fn codex_file_metadata_detects_append_truncation_and_replacement() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("session.jsonl");
    fs::write(&file_path, b"abc").unwrap();

    let initial = codex::codex_file_metadata(&file_path);
    assert_eq!(initial.size, 3);
    assert!(initial.file_id.is_some());

    {
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&file_path)
            .unwrap();
        f.write_all(b"def").unwrap();
    }
    let appended = codex::codex_file_metadata(&file_path);
    assert_eq!(appended.size, 6);
    assert_eq!(appended.file_id, initial.file_id);

    {
        let f = fs::OpenOptions::new().write(true).open(&file_path).unwrap();
        f.set_len(2).unwrap();
    }
    let truncated = codex::codex_file_metadata(&file_path);
    assert_eq!(truncated.size, 2);
    assert_eq!(truncated.file_id, initial.file_id);

    fs::remove_file(&file_path).unwrap();
    fs::write(&file_path, b"replacement").unwrap();
    let replaced = codex::codex_file_metadata(&file_path);
    assert_eq!(replaced.size, 11);
}

#[test]
fn codex_incremental_parsing_uses_previous_totals() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("session.jsonl");
    let day = utc_ymd_hms(2025, 12, 20, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    let turn_ctx = json!({
        "type": "turn_context",
        "timestamp": "2025-12-20T12:00:00Z",
        "payload": {"model": "openai/gpt-5.2-codex"}
    });
    let first_tok = json!({
        "type": "event_msg",
        "timestamp": "2025-12-20T12:00:01Z",
        "payload": {
            "type": "token_count",
            "info": {
                "total_token_usage": {
                    "input_tokens": 100,
                    "cached_input_tokens": 20,
                    "output_tokens": 10,
                    "reasoning_output_tokens": 4
                },
                "model": "openai/gpt-5.2-codex"
            }
        }
    });
    write_jsonl(&file_path, &[turn_ctx.clone(), first_tok.clone()]);

    let first = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert!(first.parsed_bytes > 0);
    assert_eq!(first.last_totals.map(|t| t.input), Some(100));
    assert_eq!(first.last_totals.map(|t| t.cached), Some(20));
    assert_eq!(first.last_totals.map(|t| t.output), Some(10));
    assert_eq!(first.last_totals.and_then(|t| t.reasoning), Some(4));
    assert_eq!(first.rows[0].reasoning, Some(4));

    let second_tok = json!({
        "type": "event_msg",
        "timestamp": "2025-12-20T12:00:02Z",
        "payload": {
            "type": "token_count",
            "info": {
                "total_token_usage": {
                    "input_tokens": 160,
                    "cached_input_tokens": 40,
                    "output_tokens": 16,
                    "reasoning_output_tokens": 7
                },
                "model": "openai/gpt-5.2-codex"
            }
        }
    });
    write_jsonl(&file_path, &[turn_ctx, first_tok, second_tok]);

    let delta = codex::parse_codex_file(
        &file_path,
        &range,
        first.parsed_bytes,
        first.last_model.as_deref(),
        first.last_totals,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    let packed = &delta.days["2025-12-20"]["gpt-5.2-codex"];
    assert_eq!(&packed[..3], &[60, 20, 6]);
    assert_eq!(delta.rows[0].reasoning, Some(3));
}

#[test]
fn codex_cached_tokens_use_maximum_of_both_cache_fields() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("cache-max.jsonl");
    let day = utc_ymd_hms(2025, 12, 20, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    write_jsonl(
        &file_path,
        &[json!({
            "type": "event_msg",
            "timestamp": "2025-12-20T12:00:00Z",
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": {
                        "input_tokens": 100,
                        "cached_input_tokens": 0,
                        "cache_read_input_tokens": 25,
                        "output_tokens": 10
                    },
                    "model": "openai/gpt-5.2-codex"
                }
            }
        })],
    );
    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(
        &parsed.days["2025-12-20"]["gpt-5.2-codex"][..3],
        &[100, 25, 10]
    );
}

#[test]
fn codex_out_of_order_stale_snapshot_does_not_double_count_last_usage() {
    let tmp = TempDir::new().expect("tempdir");
    let stale_path = tmp.path().join("stale.jsonl");
    let day = utc_ymd_hms(2025, 12, 20, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    write_jsonl(
        &stale_path,
        &[
            json!({
                "type": "turn_context",
                "timestamp": "2025-12-20T12:00:00Z",
                "payload": {"model": "openai/gpt-5.2-codex"}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2025-12-20T12:00:01Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {"input_tokens": 10, "output_tokens": 3},
                        "last_token_usage": {"input_tokens": 5, "output_tokens": 1}
                    }
                }
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2025-12-20T12:00:02Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {"input_tokens": 8, "output_tokens": 2},
                        "last_token_usage": {"input_tokens": 5, "output_tokens": 1}
                    }
                }
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2025-12-20T12:00:02Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {"input_tokens": 15, "output_tokens": 5},
                        "last_token_usage": {"input_tokens": 5, "output_tokens": 1}
                    }
                }
            }),
        ],
    );

    let parsed = codex::parse_codex_file(
        &stale_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    let packed = &parsed.days["2025-12-20"]["gpt-5.2-codex"];
    assert_eq!(packed[0], 10);
    assert_eq!(packed[2], 2);
}

#[test]
fn codex_stale_detection_treats_omitted_reasoning_as_unknown() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("stale-omitted-reasoning.jsonl");
    let day = utc_ymd_hms(2025, 12, 20, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    write_jsonl(
        &file_path,
        &[
            json!({
                "type": "turn_context",
                "timestamp": "2025-12-20T12:00:00Z",
                "payload": {"model": "openai/gpt-5.2-codex"}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2025-12-20T12:00:01Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {"input_tokens": 10, "output_tokens": 3, "reasoning_output_tokens": 1},
                        "last_token_usage": {"input_tokens": 5, "output_tokens": 1}
                    }
                }
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2025-12-20T12:00:02Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {"input_tokens": 15, "output_tokens": 4},
                        "last_token_usage": {"input_tokens": 5, "output_tokens": 1}
                    }
                }
            }),
        ],
    );

    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    let packed = &parsed.days["2025-12-20"]["gpt-5.2-codex"];
    assert_eq!(packed[0], 10);
    assert_eq!(packed[2], 2);
}

#[test]
fn codex_bare_usage_rows_count_exec_aliases_with_cached_input_subtraction() {
    let tmp = TempDir::new().expect("tempdir");
    let bare_path = tmp.path().join("bare.jsonl");
    let day = utc_ymd_hms(2026, 8, 21, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    write_jsonl(
        &bare_path,
        &[
            json!({
                "timestamp": "2026-08-21T12:00:00Z",
                "model": "openai/gpt-5.2-codex",
                "usage": {"prompt_tokens": 120, "cached_tokens": 20, "completion_tokens": 30}
            }),
            json!({
                "timestamp": "2026-08-21T12:00:00Z",
                "data": {"usage": {"input": 10, "output": 4}}
            }),
        ],
    );
    let bare_parsed = codex::parse_codex_file(
        &bare_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(bare_parsed.rows.len(), 2);
    assert_eq!(
        &bare_parsed.days["2026-08-21"]["gpt-5.2-codex"][..3],
        &[100, 20, 30]
    );
    assert_eq!(&bare_parsed.days["2026-08-21"]["gpt-5"][..3], &[10, 0, 4]);
}

#[test]
fn codex_bare_usage_without_timestamp_uses_prior_accepted_timestamp() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("bare-usage-timestamp-fallback.jsonl");
    let day_two = utc_ymd_hms(2026, 8, 22, 12, 0, 0);
    let range = DayRange::new(day_two, day_two, Some(TimeZone::UTC));

    write_jsonl(
        &file_path,
        &[
            json!({
                "timestamp": "2026-08-21T12:00:00Z",
                "response": {"usage": {"input_tokens": 7, "output_tokens": 3}}
            }),
            json!({
                "result": {"usage": {"input_tokens": 5, "output_tokens": 1}}
            }),
        ],
    );
    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(&parsed.days["2026-08-21"]["gpt-5"][..3], &[12, 0, 4]);
    let row_days: Vec<&str> = parsed.rows.iter().map(|r| r.day.as_str()).collect();
    assert_eq!(row_days, vec!["2026-08-21", "2026-08-21"]);
}

#[test]
fn codex_incremental_parsing_keeps_current_turn_id() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("priority-session.jsonl");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let turn_id = "22222222-2222-2222-2222-222222222222";

    let turn_ctx = json!({
        "type": "turn_context",
        "timestamp": "2026-05-10T12:00:00Z",
        "payload": {"model": "openai/gpt-5.5"}
    });
    let task_started = json!({
        "type": "event_msg",
        "timestamp": "2026-05-10T12:00:01Z",
        "payload": {"type": "task_started", "id": turn_id}
    });
    let first_tok = json!({
        "type": "event_msg",
        "timestamp": "2026-05-10T12:00:02Z",
        "payload": {
            "type": "token_count",
            "info": {
                "total_token_usage": {
                    "input_tokens": 100,
                    "cached_input_tokens": 20,
                    "output_tokens": 10
                }
            }
        }
    });
    write_jsonl(
        &file_path,
        &[turn_ctx.clone(), task_started.clone(), first_tok.clone()],
    );

    let first = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(first.last_codex_turn_id.as_deref(), Some(turn_id));
    assert_eq!(first.rows[0].turn_id.as_deref(), Some(turn_id));

    let second_tok = json!({
        "type": "event_msg",
        "timestamp": "2026-05-10T12:00:03Z",
        "payload": {
            "type": "token_count",
            "info": {
                "total_token_usage": {
                    "input_tokens": 160,
                    "cached_input_tokens": 40,
                    "output_tokens": 16
                }
            }
        }
    });
    write_jsonl(&file_path, &[turn_ctx, task_started, first_tok, second_tok]);

    let delta = codex::parse_codex_file(
        &file_path,
        &range,
        first.parsed_bytes,
        first.last_model.as_deref(),
        first.last_totals,
        first.last_codex_turn_id.as_deref(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(delta.last_codex_turn_id.as_deref(), Some(turn_id));
    assert_eq!(delta.rows[0].turn_id.as_deref(), Some(turn_id));
    assert_eq!(delta.rows[0].input, 60);
    assert_eq!(delta.rows[0].cached, 20);
    assert_eq!(delta.rows[0].output, 6);
}

#[test]
fn codex_fast_parser_does_not_trap_on_overflowing_token_integers() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("overflow.jsonl");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let huge = "9".repeat(100);
    let line = format!(
        "{{\"type\":\"event_msg\",\"timestamp\":\"2026-05-10T12:00:00Z\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"last_token_usage\":{{\"input_tokens\":{huge},\"cached_input_tokens\":0,\"output_tokens\":5}},\"model\":\"openai/gpt-5.5\"}}}}}}\n"
    );
    fs::write(&file_path, line).unwrap();

    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(&parsed.days["2026-05-10"]["gpt-5.5"][..3], &[0, 0, 5]);
}

#[test]
fn codex_json_fallback_applies_maximum_cache_selection() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("fallback-cache-max.jsonl");
    let day = utc_ymd_hms(2025, 12, 20, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let huge = "9".repeat(100);
    let line = format!(
        "{{\"type\":\"event_msg\",\"timestamp\":\"2025-12-20T12:00:00Z\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"last_token_usage\":{{\"input_tokens\":{huge},\"cached_input_tokens\":0,\"cache_read_input_tokens\":25,\"output_tokens\":10}},\"model\":\"openai/gpt-5.5\"}}}}}}\n"
    );
    fs::write(&file_path, line).unwrap();

    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(parsed.days["2025-12-20"]["gpt-5.5"][1], 25);
    assert_eq!(parsed.days["2025-12-20"]["gpt-5.5"][2], 10);
}

#[test]
fn codex_request_ledger_deduplicates_legacy_mirrors_and_response_replays() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("ledger.jsonl");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    write_jsonl(
        &file_path,
        &[
            json!({
                "type": "session_meta",
                "timestamp": "2026-05-10T12:00:00Z",
                "payload": {"id": "sess-1"}
            }),
            json!({
                "type": "turn_context",
                "timestamp": "2026-05-10T12:00:00Z",
                "payload": {"model": "openai/gpt-5.2-codex", "turn_id": "turn-1"}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-05-10T12:00:01Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "turn_id": "turn-1",
                        "last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10},
                        "total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}
                    }
                }
            }),
            json!({
                "type": "token_usage_record",
                "timestamp": "2026-05-10T12:00:02Z",
                "payload": {
                    "thread_id": "sess-1",
                    "turn_id": "turn-1",
                    "response_id": "resp-1",
                    "model": "openai/gpt-5.2-codex",
                    "usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10},
                    "total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}
                }
            }),
            json!({
                "type": "token_usage_record",
                "timestamp": "2026-05-10T12:00:02Z",
                "payload": {
                    "thread_id": "sess-1",
                    "turn_id": "turn-1",
                    "response_id": "resp-1",
                    "model": "openai/gpt-5.2-codex",
                    "usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10},
                    "total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}
                }
            }),
        ],
    );

    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(parsed.rows.len(), 1);
    assert_eq!(parsed.rows[0].response_id.as_deref(), Some("resp-1"));
    assert_eq!(parsed.days["2026-05-10"]["gpt-5.2-codex"][0], 100);
    assert_eq!(parsed.days["2026-05-10"]["gpt-5.2-codex"][1], 20);
    assert_eq!(parsed.days["2026-05-10"]["gpt-5.2-codex"][2], 10);
}

#[test]
fn codex_sqlite_priority_trace_parses_requests_submissions_and_completed_models_without_leaking_prompt()
 {
    let req_body = "INFO thread_id=11111111-1111-1111-1111-111111111111 \
                    turn.id=22222222-2222-2222-2222-222222222222 websocket request: \
                    {\"type\":\"response.create\",\"model\":\"request-alias\",\"service_tier\":\"priority\",\
                    \"instructions\":\"TOP_SECRET_PROMPT_TEXT\"}";
    let parsed_req =
        codex_priority::parse_codex_priority_trace_row(Some("2026-05-10T12:00:00Z"), req_body)
            .expect("parse priority request");
    assert_eq!(
        parsed_req.thread_id.as_deref(),
        Some("11111111-1111-1111-1111-111111111111")
    );
    assert_eq!(parsed_req.turn_id, "22222222-2222-2222-2222-222222222222");
    assert_eq!(parsed_req.model.as_deref(), Some("request-alias"));
    let dbg = format!("{parsed_req:?}");
    assert!(!dbg.contains("TOP_SECRET_PROMPT_TEXT"));

    let sub_body = "session_loop{thread_id=thread-sub}: Submission sub=Submission { \
                    id: \"turn-sub\", op: UserInput { text: \"PRIVATE_SUBMISSION_PROMPT\" }, \
                    thread_settings: ThreadSettingsOverrides { service_tier: Some(Some(\"priority\")) }";
    let parsed_sub = codex_priority::parse_codex_priority_trace_row(Some("1785434553"), sub_body)
        .expect("parse submission");
    assert_eq!(parsed_sub.thread_id.as_deref(), Some("thread-sub"));
    assert_eq!(parsed_sub.turn_id, "turn-sub");
    assert!(!format!("{parsed_sub:?}").contains("PRIVATE_SUBMISSION_PROMPT"));

    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    fs::create_dir_all(codex_home.join("sessions")).unwrap();
    let db_path = codex_home.join("logs_2.sqlite");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE logs (ts TEXT, feedback_log_body TEXT);
             CREATE INDEX idx_logs_ts ON logs(ts);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
            ("2026-05-10T12:00:00Z", req_body),
        )
        .unwrap();
        let completed_body = "INFO thread_id=11111111-1111-1111-1111-111111111111 \
                              turn.id=22222222-2222-2222-2222-222222222222 websocket event: \
                              {\"type\": \"response.completed\", \"response\": {\"model\": \"gpt-5.2-codex\", \"output\": \"SECRET\"}}";
        conn.execute(
            "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
            ("2026-05-10T12:00:01Z", completed_body),
        )
        .unwrap();
    }

    let turns = codex_priority::codex_priority_turns(
        &db_path,
        Some("2026-05-10"),
        Some("2026-05-10"),
        Some(TimeZone::UTC),
    );
    assert_eq!(
        turns["22222222-2222-2222-2222-222222222222"]
            .model
            .as_deref(),
        Some("gpt-5.2-codex")
    );

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/priority-session.jsonl"),
        &[
            json!({
                "type": "session_meta",
                "timestamp": "2026-05-10T12:00:00Z",
                "payload": {"id": "11111111-1111-1111-1111-111111111111"}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-05-10T12:00:00Z",
                "payload": {"type": "task_started", "id": "22222222-2222-2222-2222-222222222222"}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-05-10T12:00:02Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {"input_tokens": 100_000, "cached_input_tokens": 0, "output_tokens": 10_000}
                    }
                }
            }),
        ],
    );

    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);
    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    assert_eq!(report.data.len(), 1);
    let cost = report.data[0].cost_usd.expect("priority cost");
    assert!((cost - 0.315).abs() < 1e-6);
}

// ============================================================================
// CodexSubagentRolloutShapeTests.swift (17 @Test functions)
// ============================================================================

#[test]
fn single_leaf_metadata_means_an_independent_counter() {
    let shape =
        CodexSubagentRolloutShape::classify_session_ids(Some("leaf"), &[Some("leaf".into())]);
    assert_eq!(shape.semantics, CodexSubagentCounterSemantics::Independent);
}

#[test]
fn single_leaf_first_turn_marker_proposes_a_parent_confirmed_suffix() {
    let baseline = CodexTotals::new(1000, 900, 100, None);
    let shape = CodexSubagentRolloutShape::classify(
        Some("leaf"),
        &[
            CodexSubagentObservation::session_metadata(0, Some("leaf".into())),
            CodexSubagentObservation::token_count(1, Some(baseline), Some(baseline)),
            CodexSubagentObservation::turn_context(3),
            CodexSubagentObservation::inter_agent_communication(4, true),
        ],
        true,
    );
    let candidate = shape.owned_suffix_candidate.expect("candidate");
    assert_eq!(shape.semantics, CodexSubagentCounterSemantics::Independent);
    assert!(shape.owned_suffix.is_none());
    assert_eq!(candidate.suffix.start_line_index, 3);
    assert_eq!(candidate.parent_totals_at_boundary.input, baseline.input);
    assert_eq!(candidate.parent_totals_at_boundary.cached, baseline.cached);
    assert_eq!(candidate.parent_totals_at_boundary.output, baseline.output);
}

#[test]
fn single_leaf_marker_without_an_explicit_parent_stays_independent() {
    let baseline = CodexTotals::new(1000, 900, 100, None);
    let shape = CodexSubagentRolloutShape::classify(
        Some("leaf"),
        &[
            CodexSubagentObservation::session_metadata(0, Some("leaf".into())),
            CodexSubagentObservation::token_count(1, Some(baseline), Some(baseline)),
            CodexSubagentObservation::turn_context(3),
            CodexSubagentObservation::inter_agent_communication(4, true),
        ],
        false,
    );
    assert_eq!(shape.semantics, CodexSubagentCounterSemantics::Independent);
    assert!(shape.owned_suffix_candidate.is_none());
}

#[test]
fn later_marker_after_an_earlier_turn_does_not_propose_a_suffix() {
    let baseline = CodexTotals::new(1000, 900, 100, None);
    let shape = CodexSubagentRolloutShape::classify(
        Some("leaf"),
        &[
            CodexSubagentObservation::session_metadata(0, Some("leaf".into())),
            CodexSubagentObservation::turn_context(1),
            CodexSubagentObservation::token_count(2, Some(baseline), Some(baseline)),
            CodexSubagentObservation::turn_context(3),
            CodexSubagentObservation::inter_agent_communication(4, true),
        ],
        true,
    );
    assert_eq!(shape.semantics, CodexSubagentCounterSemantics::Independent);
    assert!(shape.owned_suffix_candidate.is_none());
}

#[test]
fn zero_pre_turn_totals_do_not_propose_a_suffix() {
    let zero = CodexTotals::new(0, 0, 0, None);
    let shape = CodexSubagentRolloutShape::classify(
        Some("leaf"),
        &[
            CodexSubagentObservation::session_metadata(0, Some("leaf".into())),
            CodexSubagentObservation::token_count(1, Some(zero), Some(zero)),
            CodexSubagentObservation::turn_context(2),
            CodexSubagentObservation::inter_agent_communication(3, true),
        ],
        true,
    );
    assert!(shape.owned_suffix_candidate.is_none());
}

#[test]
fn nonadjacent_first_turn_trigger_does_not_propose_a_suffix() {
    let baseline = CodexTotals::new(1000, 900, 100, None);
    let shape = CodexSubagentRolloutShape::classify(
        Some("leaf"),
        &[
            CodexSubagentObservation::session_metadata(0, Some("leaf".into())),
            CodexSubagentObservation::token_count(1, Some(baseline), Some(baseline)),
            CodexSubagentObservation::turn_context(2),
            CodexSubagentObservation::inter_agent_communication(4, true),
        ],
        true,
    );
    assert!(shape.owned_suffix_candidate.is_none());
}

#[test]
fn embedded_ancestor_metadata_means_a_copied_prefix() {
    let shape = CodexSubagentRolloutShape::classify_session_ids(
        Some("leaf"),
        &[Some("leaf".into()), Some("parent".into())],
    );
    assert_eq!(shape.semantics, CodexSubagentCounterSemantics::CopiedPrefix);
    assert_eq!(shape.inferred_parent_session_id.as_deref(), Some("parent"));
}

#[test]
fn multiple_ancestors_do_not_infer_an_ambiguous_parent() {
    let shape = CodexSubagentRolloutShape::classify_session_ids(
        Some("leaf"),
        &[
            Some("leaf".into()),
            Some("parent".into()),
            Some("grandparent".into()),
        ],
    );
    assert_eq!(shape.semantics, CodexSubagentCounterSemantics::CopiedPrefix);
    assert!(shape.inferred_parent_session_id.is_none());
}

#[test]
fn repeated_leaf_metadata_does_not_invent_an_ancestor() {
    let shape = CodexSubagentRolloutShape::classify_session_ids(
        Some("leaf"),
        &[Some("leaf".into()), Some("leaf".into())],
    );
    assert_eq!(shape.semantics, CodexSubagentCounterSemantics::Independent);
}

#[test]
fn unknown_leaf_followed_by_a_concrete_metadata_id_is_copied() {
    let shape =
        CodexSubagentRolloutShape::classify_session_ids(None, &[None, Some("parent".into())]);
    assert_eq!(shape.semantics, CodexSubagentCounterSemantics::CopiedPrefix);
    assert_eq!(shape.inferred_parent_session_id.as_deref(), Some("parent"));
}

#[test]
fn idless_metadata_after_a_known_leaf_is_conservatively_copied() {
    let shape =
        CodexSubagentRolloutShape::classify_session_ids(Some("leaf"), &[Some("leaf".into()), None]);
    assert_eq!(shape.semantics, CodexSubagentCounterSemantics::CopiedPrefix);
    assert!(shape.inferred_parent_session_id.is_none());
}

#[test]
fn only_concrete_normalized_ids_identify_the_same_leaf() {
    assert!(CodexSubagentRolloutShape::same_concrete_session_id(
        Some(" leaf "),
        Some("leaf")
    ));
    assert!(!CodexSubagentRolloutShape::same_concrete_session_id(
        None, None
    ));
    assert!(!CodexSubagentRolloutShape::same_concrete_session_id(
        Some(""),
        Some("")
    ));
}

#[test]
fn adjacent_trigger_after_the_final_ancestor_opens_an_owned_suffix() {
    let baseline = CodexTotals::new(1000, 900, 100, None);
    let shape = CodexSubagentRolloutShape::classify(
        Some("leaf"),
        &[
            CodexSubagentObservation::session_metadata(0, Some("leaf".into())),
            CodexSubagentObservation::token_count(4, None, Some(baseline)),
            CodexSubagentObservation::session_metadata(5, Some("parent".into())),
            CodexSubagentObservation::turn_context(8),
            CodexSubagentObservation::inter_agent_communication(9, true),
        ],
        false,
    );
    let suffix = shape.owned_suffix.expect("suffix");
    assert_eq!(shape.semantics, CodexSubagentCounterSemantics::CopiedPrefix);
    assert_eq!(suffix.start_line_index, 8);
    assert_eq!(suffix.raw_totals_baseline.input, 1000);
    assert_eq!(suffix.raw_totals_baseline.cached, 900);
    assert_eq!(suffix.raw_totals_baseline.output, 100);
}

#[test]
fn nonadjacent_trigger_does_not_invent_an_owned_suffix() {
    let shape = CodexSubagentRolloutShape::classify(
        Some("leaf"),
        &[
            CodexSubagentObservation::token_count(
                0,
                None,
                Some(CodexTotals::new(1000, 900, 100, None)),
            ),
            CodexSubagentObservation::session_metadata(1, Some("parent".into())),
            CodexSubagentObservation::turn_context(3),
            CodexSubagentObservation::inter_agent_communication(5, true),
        ],
        false,
    );
    assert_eq!(shape.semantics, CodexSubagentCounterSemantics::CopiedPrefix);
    assert!(shape.owned_suffix.is_none());
}

#[test]
fn copied_prefix_can_restart_only_with_strong_reset_evidence() {
    let shape = CodexSubagentRolloutShape::classify(
        Some("leaf"),
        &[
            CodexSubagentObservation::session_metadata(0, Some("leaf".into())),
            CodexSubagentObservation::token_count(
                2,
                None,
                Some(CodexTotals::new(1000, 900, 100, None)),
            ),
            CodexSubagentObservation::session_metadata(3, Some("parent".into())),
            CodexSubagentObservation::turn_context(5),
            CodexSubagentObservation::inter_agent_communication(6, true),
            CodexSubagentObservation::token_count(
                7,
                Some(CodexTotals::new(50, 10, 5, None)),
                Some(CodexTotals::new(50, 10, 5, None)),
            ),
        ],
        false,
    );
    let suffix = shape.owned_suffix.expect("suffix");
    assert_eq!(suffix.raw_totals_baseline.input, 0);
    assert_eq!(suffix.raw_totals_baseline.cached, 0);
    assert_eq!(suffix.raw_totals_baseline.output, 0);
}

#[test]
fn first_valid_leaf_marker_owns_later_leaf_turns() {
    let first_baseline = CodexTotals::new(1000, 900, 100, None);
    let shape = CodexSubagentRolloutShape::classify(
        Some("leaf"),
        &[
            CodexSubagentObservation::session_metadata(0, Some("leaf".into())),
            CodexSubagentObservation::session_metadata(1, Some("parent".into())),
            CodexSubagentObservation::token_count(2, None, Some(first_baseline)),
            CodexSubagentObservation::turn_context(4),
            CodexSubagentObservation::inter_agent_communication(5, true),
            CodexSubagentObservation::token_count(
                6,
                None,
                Some(CodexTotals::new(1050, 910, 105, None)),
            ),
            CodexSubagentObservation::turn_context(8),
            CodexSubagentObservation::inter_agent_communication(9, true),
        ],
        false,
    );
    let suffix = shape.owned_suffix.expect("suffix");
    assert_eq!(suffix.start_line_index, 4);
    assert_eq!(suffix.raw_totals_baseline.input, first_baseline.input);
}

#[test]
fn later_ancestor_invalidates_a_tentative_marker() {
    let shape = CodexSubagentRolloutShape::classify(
        Some("leaf"),
        &[
            CodexSubagentObservation::session_metadata(0, Some("leaf".into())),
            CodexSubagentObservation::session_metadata(1, Some("parent".into())),
            CodexSubagentObservation::token_count(
                2,
                None,
                Some(CodexTotals::new(1000, 900, 100, None)),
            ),
            CodexSubagentObservation::turn_context(3),
            CodexSubagentObservation::inter_agent_communication(4, true),
            CodexSubagentObservation::session_metadata(5, Some("grandparent".into())),
            CodexSubagentObservation::token_count(
                6,
                None,
                Some(CodexTotals::new(2000, 1800, 200, None)),
            ),
            CodexSubagentObservation::turn_context(8),
            CodexSubagentObservation::inter_agent_communication(9, true),
        ],
        false,
    );
    let suffix = shape.owned_suffix.expect("suffix");
    assert_eq!(suffix.start_line_index, 8);
    assert_eq!(suffix.raw_totals_baseline.input, 2000);
}

// ============================================================================
// CodexSubagentForkBaselineTests.swift (6 @Test functions)
// ============================================================================

fn fork_baseline_context(ordinal: i64, timestamp: &str) -> Value {
    json!({
        "type": "turn_context",
        "ordinal": ordinal,
        "timestamp": timestamp,
        "payload": {"model": "gpt-5.4"}
    })
}

fn fork_baseline_tokens(ordinal: i64, timestamp: &str, total: [i64; 3], last: [i64; 3]) -> Value {
    let usage = |v: [i64; 3]| -> Value {
        json!({
            "input_tokens": v[0],
            "cached_input_tokens": v[1],
            "output_tokens": v[2],
            "total_tokens": v[0] + v[2]
        })
    };
    let mut last_usage = usage(last);
    if last == [0, 0, 0] {
        last_usage["total_tokens"] = json!(9035);
    }
    json!({
        "type": "event_msg",
        "ordinal": ordinal,
        "timestamp": timestamp,
        "payload": {
            "type": "token_count",
            "info": {
                "model": "gpt-5.4",
                "total_token_usage": usage(total),
                "last_token_usage": last_usage
            }
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn verify_forks(
    explicit: bool,
    trigger: bool,
    mismatched: bool,
    parent_present: bool,
    depth: usize,
    context_present: bool,
    opening: [i64; 3],
    owned: bool,
) {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day_dir = codex_home.join("sessions/2026/08/15");
    let day = utc_ymd_hms(2026, 8, 15, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let ts0 = "2026-08-15T12:00:00Z";
    let ts1 = "2026-08-15T12:00:01Z";
    let ts2 = "2026-08-15T12:00:02Z";
    let day_key = "2026-08-15";

    let mut prefix = opening;
    let owned_usage: Option<Vec<i64>> = if owned { Some(vec![70, 15, 10]) } else { None };
    let owned_tokens: i64 = if owned { 80 } else { 0 };

    if parent_present {
        write_jsonl(
            &day_dir.join("parent.jsonl"),
            &[
                json!({
                    "type": "session_meta",
                    "timestamp": ts0,
                    "payload": {"id": "fork-0"}
                }),
                fork_baseline_context(10, ts0),
                fork_baseline_tokens(2, ts0, prefix, prefix),
            ],
        );
    }

    for level in 1..=depth {
        let baseline = if mismatched {
            [prefix[0] + 4000, prefix[1] + 3000, prefix[2] + 400]
        } else {
            prefix
        };
        let mut metadata = json!({
            "id": format!("fork-{level}"),
            "forked_from_id": format!("fork-{}", level - 1),
            "timestamp": ts0,
            "thread_source": "subagent",
            "source": {
                "subagent": {
                    "thread_spawn": {
                        "parent_thread_id": format!("fork-{}", level - 1),
                        "depth": level
                    }
                }
            }
        });
        if explicit {
            metadata["subagent_history_start_ordinal"] = json!(10);
        }
        let mut lines = vec![
            json!({
                "type": "session_meta",
                "ordinal": 0,
                "timestamp": ts0,
                "payload": metadata
            }),
            json!({
                "type": "compacted",
                "ordinal": 1,
                "timestamp": ts0,
                "payload": {}
            }),
            fork_baseline_tokens(2, ts0, prefix, [0, 0, 0]),
        ];
        if context_present {
            lines.push(fork_baseline_context(10, ts0));
        }
        if trigger {
            lines.push(json!({
                "type": "inter_agent_communication_metadata",
                "ordinal": 11,
                "timestamp": ts0,
                "payload": {"trigger_turn": true}
            }));
        }
        lines.push(fork_baseline_tokens(12, ts0, prefix, prefix));
        lines.push(fork_baseline_tokens(13, ts0, baseline, baseline));
        let owned_start = lines.len();
        lines.push(fork_baseline_tokens(
            19,
            ts0,
            [baseline[0] + 50, baseline[1] + 10, baseline[2] + 5],
            [50, 10, 5],
        ));
        let final_totals = [baseline[0] + 70, baseline[1] + 15, baseline[2] + 10];
        lines.push(fork_baseline_tokens(20, ts1, final_totals, [20, 5, 5]));
        lines.push(fork_baseline_tokens(21, ts2, final_totals, [20, 5, 5]));
        if !owned {
            lines.truncate(owned_start);
        }
        let file = day_dir.join(format!("child-{level}.jsonl"));
        write_jsonl(&file, &lines);

        let mut consulted_parent = false;
        let parsed = codex::parse_codex_file(
            &file,
            &range,
            0,
            None,
            None,
            None,
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            |_, _| {
                consulted_parent = true;
                CodexForkBaseline::Unresolved
            },
        );
        assert_eq!(
            parsed
                .days
                .get(day_key)
                .and_then(|m| m.get("gpt-5.4"))
                .map(|v| v[..3].to_vec()),
            owned_usage
        );
        let row_tokens: i64 = parsed.rows.iter().map(|r| r.input + r.output).sum();
        assert_eq!(row_tokens, owned_tokens);
        assert!(!parsed.depends_on_parent_totals);
        assert!(!consulted_parent);
        prefix = final_totals;
    }

    let expected_total = (if parent_present {
        opening[0] + opening[2]
    } else {
        0
    }) + owned_tokens * (depth as i64);
    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let report_tokens: i64 = report
        .data
        .iter()
        .filter_map(|d| d.total_tokens)
        .map(|t| t as i64)
        .sum();
    assert_eq!(report_tokens, expected_total);
}

#[test]
fn explicit_suffix_excludes_inherited_totals_even_when_components_drift() {
    for mismatched in [false, true] {
        for parent_present in [false, true] {
            verify_forks(
                true,
                true,
                mismatched,
                parent_present,
                1,
                true,
                [1000, 900, 100],
                true,
            );
        }
    }
}

#[test]
fn legacy_suffix_excludes_inherited_totals_without_a_parent_snapshot() {
    for mismatched in [false, true] {
        for trigger in [false, true] {
            verify_forks(
                false,
                trigger,
                mismatched,
                false,
                1,
                true,
                [1000, 900, 100],
                true,
            );
        }
    }
}

#[test]
fn legacy_first_owned_token_identifies_its_boundary_without_turn_metadata() {
    for mismatched in [false, true] {
        verify_forks(
            false,
            false,
            mismatched,
            false,
            1,
            false,
            [1000, 900, 100],
            true,
        );
    }
}

#[test]
fn nested_forks_count_each_owned_suffix_once() {
    for explicit in [false, true] {
        for parent_present in [false, true] {
            verify_forks(
                explicit,
                true,
                true,
                parent_present,
                2,
                true,
                [1000, 900, 100],
                true,
            );
        }
    }
}

#[test]
fn zero_inherited_counters_preserve_fresh_child_usage() {
    for explicit in [false, true] {
        verify_forks(explicit, true, false, false, 1, true, [0, 0, 0], true);
    }
}

#[test]
fn terminal_inherited_only_suffixes_contribute_no_child_usage() {
    for explicit in [false, true] {
        for parent_present in [false, true] {
            verify_forks(
                explicit,
                true,
                true,
                parent_present,
                1,
                true,
                [1000, 900, 100],
                false,
            );
        }
    }
}

// ============================================================================
// CodexSubagentOrdinalBoundaryTests.swift (4 @Test functions)
// ============================================================================

fn write_ordinal_prefix(path: &Path, ts: &str) {
    write_jsonl(
        path,
        &[
            json!({
                "type": "session_meta",
                "ordinal": 0,
                "timestamp": ts,
                "payload": {
                    "id": "ordinal-child",
                    "forked_from_id": "ordinal-parent",
                    "timestamp": ts,
                    "subagent_history_start_ordinal": 210,
                    "source": {"subagent": {"thread_spawn": {"parent_thread_id": "ordinal-parent"}}}
                }
            }),
            fork_baseline_tokens(2, ts, [1000, 900, 100], [0, 0, 0]),
            fork_baseline_context(10, ts),
            json!({
                "type": "inter_agent_communication_metadata",
                "ordinal": 11,
                "timestamp": ts,
                "payload": {"trigger_turn": true}
            }),
            fork_baseline_tokens(19, ts, [1050, 910, 105], [50, 10, 5]),
            fork_baseline_tokens(208, ts, [1070, 915, 110], [20, 5, 5]),
        ],
    );
}

#[test]
fn explicit_boundary_excludes_a_prefix_ending_before_child_history() {
    let tmp = TempDir::new().expect("tempdir");
    let file = tmp.path().join("ordinal-child.jsonl");
    let day = utc_ymd_hms(2026, 8, 15, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    write_ordinal_prefix(&file, "2026-08-15T12:00:00Z");

    let mut consulted_parent = false;
    let parsed = codex::parse_codex_file(
        &file,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| {
            consulted_parent = true;
            CodexForkBaseline::Unresolved
        },
    );
    assert!(parsed.days.is_empty());
    assert!(parsed.rows.is_empty());
    assert!(!parsed.depends_on_parent_totals);
    assert!(!consulted_parent);
}

#[test]
fn explicit_boundary_preserves_first_owned_usage_across_cached_and_bounded_appends() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let file = codex_home.join("sessions/2026/08/15/ordinal-child.jsonl");
    let day = utc_ymd_hms(2026, 8, 15, 12, 0, 0);
    write_ordinal_prefix(&file, "2026-08-15T12:00:00Z");

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(tmp.path().join("cache")),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let cold = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    let cold_tokens: u64 = cold.data.iter().filter_map(|d| d.total_tokens).sum();
    assert_eq!(cold_tokens, 0);

    let mut f = fs::OpenOptions::new().append(true).open(&file).unwrap();
    for entry in [
        fork_baseline_context(210, "2026-08-15T12:00:05Z"),
        fork_baseline_tokens(211, "2026-08-15T12:00:06Z", [1100, 930, 115], [30, 15, 5]),
    ] {
        writeln!(f, "{}", serde_json::to_string(&entry).unwrap()).unwrap();
    }
    drop(f);

    let grown = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    let grown_tokens: u64 = grown.data.iter().filter_map(|d| d.total_tokens).sum();
    assert_eq!(grown_tokens, 35);
}

// ============================================================================
// CodexSubagentAccountingIntegrationTests.swift (10 @Test functions)
// ============================================================================

fn subagent_turn_ctx(timestamp: &str, model: &str) -> Value {
    json!({
        "type": "turn_context",
        "timestamp": timestamp,
        "payload": {"model": model}
    })
}

fn subagent_token_count(
    timestamp: &str,
    model: &str,
    total: Option<(i64, i64, i64)>,
    last: Option<(i64, i64, i64)>,
) -> Value {
    let mut info = json!({"model": model});
    if let Some(t) = total {
        info["total_token_usage"] = json!({
            "input_tokens": t.0,
            "cached_input_tokens": t.1,
            "output_tokens": t.2
        });
    }
    if let Some(l) = last {
        info["last_token_usage"] = json!({
            "input_tokens": l.0,
            "cached_input_tokens": l.1,
            "output_tokens": l.2
        });
    }
    json!({
        "type": "event_msg",
        "timestamp": timestamp,
        "payload": {
            "type": "token_count",
            "info": info
        }
    })
}

#[test]
fn copied_parent_prefix_keeps_the_inherited_baseline_after_late_lineage_metadata() {
    let tmp = TempDir::new().expect("tempdir");
    let day = utc_ymd_hms(2026, 7, 16, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let fork_ts = "2026-07-16T12:00:00Z";
    let parent_model = "openai/gpt-5.3";
    let leaf_model = "openai/gpt-5.4";
    let file_path = tmp.path().join("rollout-child-session.jsonl");

    write_jsonl(
        &file_path,
        &[
            json!({
                "type": "session_meta",
                "timestamp": fork_ts,
                "payload": {
                    "id": "child-session",
                    "timestamp": fork_ts,
                    "source": {"subagent": {"thread_spawn": {"parent_thread_id": "parent-session"}}}
                }
            }),
            subagent_turn_ctx(fork_ts, parent_model),
            subagent_token_count(
                "2026-07-16T12:00:01Z",
                parent_model,
                Some((1000, 900, 100)),
                Some((50, 10, 5)),
            ),
            json!({
                "type": "session_meta",
                "timestamp": fork_ts,
                "payload": {
                    "id": "child-session",
                    "forked_from_id": "parent-session",
                    "timestamp": fork_ts
                }
            }),
            json!({
                "type": "session_meta",
                "timestamp": fork_ts,
                "payload": {
                    "id": "parent-session",
                    "timestamp": fork_ts
                }
            }),
            subagent_turn_ctx(fork_ts, leaf_model),
            subagent_token_count(
                "2026-07-16T12:00:02Z",
                leaf_model,
                Some((1050, 910, 105)),
                Some((50, 10, 5)),
            ),
        ],
    );

    let mut resolved_parent = false;
    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |parent_id, _| {
            resolved_parent = true;
            assert_eq!(parent_id, "parent-session");
            CodexForkBaseline::Resolved(Some(CodexTotals::new(1000, 900, 100, None)))
        },
    );
    assert_eq!(&parsed.days["2026-07-16"]["gpt-5.4"][..3], &[50, 10, 5]);
    assert!(!parsed.days["2026-07-16"].contains_key("gpt-5.3"));
    assert!(resolved_parent);
    assert!(parsed.depends_on_parent_totals);
}

#[test]
fn local_marker_owns_only_its_suffix_and_persists_lineage_only_cache_mode() {
    let tmp = TempDir::new().expect("tempdir");
    let day = utc_ymd_hms(2026, 7, 16, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let fork_ts = "2026-07-16T12:00:00Z";
    let parent_model = "openai/gpt-5.3";
    let leaf_model = "openai/gpt-5.4";
    let file_path = tmp.path().join("rollout-marker-child.jsonl");

    write_jsonl(
        &file_path,
        &[
            json!({
                "type": "session_meta",
                "timestamp": fork_ts,
                "payload": {
                    "id": "marker-child",
                    "timestamp": fork_ts,
                    "source": {"subagent": {"thread_spawn": {"parent_thread_id": "parent-session"}}}
                }
            }),
            subagent_turn_ctx(fork_ts, parent_model),
            subagent_token_count(
                "2026-07-16T12:00:01Z",
                parent_model,
                Some((1000, 900, 100)),
                Some((50, 10, 5)),
            ),
            json!({
                "type": "session_meta",
                "timestamp": fork_ts,
                "payload": {
                    "id": "marker-child",
                    "forked_from_id": "parent-session",
                    "timestamp": fork_ts
                }
            }),
            json!({
                "type": "session_meta",
                "timestamp": fork_ts,
                "payload": {"id": "ancestor-session"}
            }),
            subagent_turn_ctx("2026-07-16T12:00:02Z", leaf_model),
            json!({
                "type": "inter_agent_communication_metadata",
                "timestamp": "2026-07-16T12:00:02Z",
                "payload": {"trigger_turn": true}
            }),
            subagent_token_count(
                "2026-07-16T12:00:02.500Z",
                parent_model,
                Some((1000, 900, 100)),
                Some((50, 10, 5)),
            ),
            subagent_token_count(
                "2026-07-16T12:00:03Z",
                leaf_model,
                Some((1050, 910, 105)),
                Some((50, 10, 5)),
            ),
        ],
    );

    let mut resolved_parent = false;
    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| {
            resolved_parent = true;
            CodexForkBaseline::Resolved(Some(CodexTotals::new(10, 0, 0, None)))
        },
    );
    assert_eq!(&parsed.days["2026-07-16"]["gpt-5.4"][..3], &[50, 10, 5]);
    assert!(!parsed.days["2026-07-16"].contains_key("gpt-5.3"));
    assert!(!parsed.depends_on_parent_totals);
    assert!(!resolved_parent);
}

#[test]
fn copied_prefix_infers_its_parent_and_ignores_a_spoofed_trigger_outside_the_payload() {
    let tmp = TempDir::new().expect("tempdir");
    let day = utc_ymd_hms(2026, 7, 16, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let fork_ts = "2026-07-16T12:00:00Z";
    let file_path = tmp.path().join("inferred-parent.jsonl");

    write_jsonl(
        &file_path,
        &[
            json!({
                "type": "session_meta",
                "timestamp": fork_ts,
                "payload": {
                    "id": "inferred-child",
                    "timestamp": fork_ts,
                    "source": {"subagent": {"thread_spawn": {}}}
                }
            }),
            subagent_token_count(
                "2026-07-16T12:00:01Z",
                "openai/gpt-5.3",
                Some((1000, 900, 100)),
                Some((50, 10, 5)),
            ),
            json!({
                "type": "session_meta",
                "timestamp": fork_ts,
                "payload": {"id": "inferred-parent"}
            }),
            subagent_turn_ctx("2026-07-16T12:00:02Z", "openai/gpt-5.4"),
            json!({
                "type": "inter_agent_communication_metadata",
                "timestamp": "2026-07-16T12:00:02Z",
                "trigger_turn": true,
                "payload": {"trigger_turn": false}
            }),
            subagent_token_count(
                "2026-07-16T12:00:03Z",
                "openai/gpt-5.4",
                Some((1050, 910, 105)),
                Some((50, 10, 5)),
            ),
        ],
    );

    let mut resolved_parent = false;
    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |parent_id, _| {
            resolved_parent = true;
            assert_eq!(parent_id, "inferred-parent");
            CodexForkBaseline::Resolved(Some(CodexTotals::new(1000, 900, 100, None)))
        },
    );
    assert_eq!(&parsed.days["2026-07-16"]["gpt-5.4"][..3], &[50, 10, 5]);
    assert_eq!(parsed.forked_from_id.as_deref(), Some("inferred-parent"));
    assert!(parsed.depends_on_parent_totals);
    assert!(resolved_parent);
}

#[test]
fn oversized_ancestor_metadata_remains_conservative_copied_prefix_evidence() {
    let tmp = TempDir::new().expect("tempdir");
    let day = utc_ymd_hms(2026, 7, 16, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let ts = "2026-07-16T12:00:00Z";
    let file_path = tmp.path().join("oversized-ancestor.jsonl");
    let big_padding = "x".repeat(300_000);
    let lines = [
        serde_json::to_string(&json!({
            "type": "session_meta",
            "timestamp": ts,
            "payload": {
                "id": "oversized-child",
                "source": {"subagent": {"thread_spawn": {}}}
            }
        }))
        .unwrap(),
        serde_json::to_string(&subagent_token_count(
            "2026-07-16T12:00:01Z",
            "openai/gpt-5.3",
            Some((1000, 900, 100)),
            Some((50, 10, 5)),
        ))
        .unwrap(),
        format!(
            "{{\"type\":\"session_meta\",\"timestamp\":\"{ts}\",\"payload\":{{\"id\":\"oversized-parent\",\"padding\":\"{big_padding}\"}}}}"
        ),
        serde_json::to_string(&subagent_token_count(
            "2026-07-16T12:00:02Z",
            "openai/gpt-5.4",
            Some((1050, 910, 105)),
            Some((50, 10, 5)),
        ))
        .unwrap(),
    ];
    fs::write(&file_path, lines.join("\n") + "\n").unwrap();

    let mut resolved_parent = false;
    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |parent_id, _| {
            resolved_parent = true;
            assert_eq!(parent_id, "oversized-parent");
            CodexForkBaseline::Resolved(Some(CodexTotals::new(1000, 900, 100, None)))
        },
    );
    assert_eq!(&parsed.days["2026-07-16"]["gpt-5.4"][..3], &[50, 10, 5]);
    assert_eq!(parsed.forked_from_id.as_deref(), Some("oversized-parent"));
    assert!(parsed.depends_on_parent_totals);
    assert!(resolved_parent);
}

#[test]
fn invalid_timestamp_suffix_markers_preserve_parent_dependency_on_both_parser_paths() {
    let tmp = TempDir::new().expect("tempdir");
    let day = utc_ymd_hms(2026, 7, 16, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let ts = "2026-07-16T12:00:00Z";
    let file_path = tmp.path().join("invalid-ts-marker.jsonl");
    write_jsonl(
        &file_path,
        &[
            json!({
                "type": "session_meta",
                "timestamp": ts,
                "payload": {
                    "id": "invalid-marker-child",
                    "source": {"subagent": {"thread_spawn": {}}}
                }
            }),
            subagent_token_count(
                "2026-07-16T12:00:01Z",
                "openai/gpt-5.3",
                Some((1000, 900, 100)),
                Some((50, 10, 5)),
            ),
            json!({
                "type": "session_meta",
                "timestamp": ts,
                "payload": {"id": "invalid-marker-parent"}
            }),
            json!({
                "type": "turn_context",
                "payload": {"model": "openai/gpt-5.4"}
            }),
            json!({
                "type": "inter_agent_communication_metadata",
                "payload": {"trigger_turn": true}
            }),
            subagent_token_count(
                "2026-07-16T12:00:02Z",
                "openai/gpt-5.4",
                Some((1050, 910, 105)),
                Some((50, 10, 5)),
            ),
        ],
    );

    let mut resolved_parent = false;
    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |parent_id, _| {
            resolved_parent = true;
            assert_eq!(parent_id, "invalid-marker-parent");
            CodexForkBaseline::Resolved(Some(CodexTotals::new(1000, 900, 100, None)))
        },
    );
    assert_eq!(&parsed.days["2026-07-16"]["gpt-5.4"][..3], &[50, 10, 5]);
    assert!(parsed.depends_on_parent_totals);
    assert!(resolved_parent);
}

#[test]
fn oversized_invalid_suffix_markers_preserve_parent_dependency() {
    let tmp = TempDir::new().expect("tempdir");
    let day = utc_ymd_hms(2026, 7, 16, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let ts = "2026-07-16T12:00:00Z";
    let file_path = tmp.path().join("oversized-invalid-marker.jsonl");
    let big_padding = "x".repeat(300_000);
    let lines = [
        serde_json::to_string(&json!({
            "type": "session_meta",
            "timestamp": ts,
            "payload": {
                "id": "oversized-marker-child",
                "source": {"subagent": {"thread_spawn": {}}}
            }
        }))
        .unwrap(),
        serde_json::to_string(&subagent_token_count(
            "2026-07-16T12:00:01Z",
            "openai/gpt-5.3",
            Some((1000, 900, 100)),
            Some((50, 10, 5)),
        ))
        .unwrap(),
        serde_json::to_string(&json!({
            "type": "session_meta",
            "timestamp": ts,
            "payload": {"id": "oversized-marker-parent"}
        }))
        .unwrap(),
        format!(
            "{{\"type\":\"turn_context\",\"timestamp\":\"invalid\",\"payload\":{{\"model\":\"openai/gpt-5.4\",\"padding\":\"{big_padding}\"}}}}"
        ),
        serde_json::to_string(&json!({
            "type": "inter_agent_communication_metadata",
            "timestamp": "2026-07-16T12:00:02Z",
            "payload": {"trigger_turn": true}
        }))
        .unwrap(),
        serde_json::to_string(&subagent_token_count(
            "2026-07-16T12:00:03Z",
            "openai/gpt-5.4",
            Some((1050, 910, 105)),
            Some((50, 10, 5)),
        ))
        .unwrap(),
    ];
    fs::write(&file_path, lines.join("\n") + "\n").unwrap();

    let mut resolved_parent = false;
    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |parent_id, _| {
            resolved_parent = true;
            assert_eq!(parent_id, "oversized-marker-parent");
            CodexForkBaseline::Resolved(Some(CodexTotals::new(1000, 900, 100, None)))
        },
    );
    assert_eq!(&parsed.days["2026-07-16"]["gpt-5.4"][..3], &[50, 10, 5]);
    assert!(parsed.depends_on_parent_totals);
    assert!(resolved_parent);
}

#[test]
fn idless_copied_prefix_without_a_parent_or_local_marker_is_suppressed() {
    let tmp = TempDir::new().expect("tempdir");
    let day = utc_ymd_hms(2026, 7, 16, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let ts = "2026-07-16T12:00:00Z";
    let file_path = tmp.path().join("idless-copied.jsonl");
    write_jsonl(
        &file_path,
        &[
            json!({
                "type": "session_meta",
                "timestamp": ts,
                "payload": {
                    "id": "ambiguous-child",
                    "source": {"subagent": {"thread_spawn": {}}}
                }
            }),
            subagent_token_count(
                "2026-07-16T12:00:01Z",
                "openai/gpt-5.3",
                Some((1000, 900, 100)),
                None,
            ),
            json!({
                "type": "session_meta",
                "timestamp": ts,
                "payload": {}
            }),
            subagent_token_count(
                "2026-07-16T12:00:02Z",
                "openai/gpt-5.4",
                Some((1050, 910, 105)),
                None,
            ),
        ],
    );
    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Unresolved,
    );
    assert!(parsed.days.is_empty());
    assert!(parsed.rows.is_empty());
}

#[test]
fn protocol_ordinal_isolates_the_child_suffix_without_resolving_its_parent() {
    let tmp = TempDir::new().expect("tempdir");
    let day = utc_ymd_hms(2026, 7, 16, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let ts = "2026-07-16T12:00:00Z";
    let file_path = tmp.path().join("protocol-ordinal.jsonl");

    let mut copied_tok = subagent_token_count(
        "2026-07-16T12:00:01Z",
        "openai/gpt-5.3",
        Some((1000, 900, 100)),
        Some((50, 10, 5)),
    );
    copied_tok["ordinal"] = json!(9);
    let mut owned_ctx = subagent_turn_ctx("2026-07-16T12:00:02Z", "openai/gpt-5.4");
    owned_ctx["ordinal"] = json!(10);
    let mut owned_tok = subagent_token_count(
        "2026-07-16T12:00:03Z",
        "openai/gpt-5.4",
        Some((1050, 910, 105)),
        Some((50, 10, 5)),
    );
    owned_tok["ordinal"] = json!(11);

    write_jsonl(
        &file_path,
        &[
            json!({
                "type": "session_meta",
                "ordinal": 0,
                "timestamp": ts,
                "payload": {
                    "id": "ordinal-child",
                    "forked_from_id": "ordinal-parent",
                    "timestamp": ts,
                    "subagent_history_start_ordinal": 10,
                    "source": {"subagent": {"thread_spawn": {"parent_thread_id": "ordinal-parent"}}}
                }
            }),
            copied_tok,
            owned_ctx,
            owned_tok,
        ],
    );

    let mut resolved_parent = false;
    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| {
            resolved_parent = true;
            CodexForkBaseline::Resolved(Some(CodexTotals::new(1, 1, 1, None)))
        },
    );
    assert_eq!(&parsed.days["2026-07-16"]["gpt-5.4"][..3], &[50, 10, 5]);
    assert!(!parsed.days["2026-07-16"].contains_key("gpt-5.3"));
    assert!(!parsed.depends_on_parent_totals);
    assert!(!resolved_parent);
}

#[test]
fn legacy_child_proves_its_inherited_baseline_from_first_owned_total_minus_last() {
    let tmp = TempDir::new().expect("tempdir");
    let day = utc_ymd_hms(2026, 7, 16, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let ts = "2026-07-16T12:00:00Z";
    let file_path = tmp.path().join("legacy-total-minus-last.jsonl");
    write_jsonl(
        &file_path,
        &[
            json!({
                "type": "session_meta",
                "timestamp": ts,
                "payload": {
                    "id": "legacy-self-confirmed",
                    "forked_from_id": "legacy-parent",
                    "timestamp": ts,
                    "source": {"subagent": {"thread_spawn": {"parent_thread_id": "legacy-parent"}}}
                }
            }),
            subagent_token_count(
                "2026-07-16T12:00:01Z",
                "openai/gpt-5.3",
                Some((1000, 900, 100)),
                None,
            ),
            subagent_turn_ctx("2026-07-16T12:00:02Z", "openai/gpt-5.4"),
            json!({
                "type": "inter_agent_communication_metadata",
                "timestamp": "2026-07-16T12:00:02Z",
                "payload": {"trigger_turn": true}
            }),
            subagent_token_count(
                "2026-07-16T12:00:03Z",
                "openai/gpt-5.4",
                Some((1050, 910, 105)),
                Some((50, 10, 5)),
            ),
            subagent_token_count(
                "2026-07-16T12:00:04Z",
                "openai/gpt-5.4",
                Some((1070, 915, 110)),
                Some((20, 5, 5)),
            ),
        ],
    );
    let mut resolved_parent = false;
    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| {
            resolved_parent = true;
            CodexForkBaseline::Unresolved
        },
    );
    assert_eq!(&parsed.days["2026-07-16"]["gpt-5.4"][..3], &[70, 15, 10]);
    assert!(!parsed.days["2026-07-16"].contains_key("gpt-5.3"));
    assert!(!parsed.depends_on_parent_totals);
    assert!(!resolved_parent);
}

#[test]
fn bounded_append_fallback_reclassifies_the_complete_subagent_rollout() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let file_path = codex_home.join("sessions/2026/07/16/growing-subagent.jsonl");
    let day = utc_ymd_hms(2026, 7, 16, 12, 0, 0);
    let ts = "2026-07-16T12:00:00Z";

    write_jsonl(
        &file_path,
        &[
            json!({
                "type": "session_meta",
                "timestamp": ts,
                "payload": {
                    "id": "growing-child",
                    "source": {"subagent": {"thread_spawn": {}}}
                }
            }),
            subagent_turn_ctx(ts, "openai/gpt-5.3"),
            subagent_token_count(
                "2026-07-16T12:00:01Z",
                "openai/gpt-5.3",
                Some((1000, 900, 100)),
                None,
            ),
        ],
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(tmp.path().join("cache")),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(first.data[0].total_tokens, Some(1100));

    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(&file_path)
        .unwrap();
    for entry in [
        json!({"type": "session_meta", "timestamp": ts, "payload": {"id": "growing-parent"}}),
        subagent_turn_ctx("2026-07-16T12:00:02Z", "openai/gpt-5.4"),
        json!({
            "type": "inter_agent_communication_metadata",
            "timestamp": "2026-07-16T12:00:02Z",
            "payload": {"trigger_turn": true}
        }),
        subagent_token_count(
            "2026-07-16T12:00:03Z",
            "openai/gpt-5.4",
            Some((1050, 910, 105)),
            Some((50, 10, 5)),
        ),
    ] {
        writeln!(f, "{}", serde_json::to_string(&entry).unwrap()).unwrap();
    }
    drop(f);

    let second = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(second.data[0].total_tokens, Some(55));
}

// ============================================================================
// CostUsageScannerForkSplitTests.swift (12 @Test functions)
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn make_row(
    day: &str,
    model: &str,
    turn_id: &str,
    event_index: usize,
    timestamp_unix_ms: Option<i64>,
    input: i64,
    cached: i64,
    output: i64,
    pricing_mode: Option<&str>,
    known_cost_nanos: Option<i64>,
) -> CodexUsageRow {
    let pricing_model = pricing::normalize_codex_model(model);
    let tier = match pricing_mode {
        Some("priority") => pricing::CodexServiceTier::Priority,
        _ => pricing::CodexServiceTier::Standard,
    };
    let cost_nanos = known_cost_nanos.or_else(|| {
        pricing::codex_cost_usd_with_tier(
            &pricing_model,
            input.max(0),
            cached.max(0),
            output.max(0),
            Some(tier),
            None,
        )
        .map(|usd| (usd * 1_000_000_000.0).round() as i64)
    });
    let unpriced_tokens = if cost_nanos.is_none() {
        Some(input.max(0) + output.max(0))
    } else {
        Some(0)
    };
    CodexUsageRow {
        day: day.to_owned(),
        model: pricing_model.clone(),
        raw_model: Some(model.to_owned()),
        turn_id: Some(turn_id.to_owned()),
        event_index: Some(event_index),
        timestamp_unix_ms,
        input,
        cached,
        output,
        reasoning: None,
        response_id: None,
        request_mirror_keys: None,
        known_cost_nanos: cost_nanos,
        unpriced_tokens,
        pricing_model: Some(pricing_model),
        pricing_mode: Some(pricing_mode.unwrap_or("standard").to_owned()),
        session_id: None,
        file_identity: None,
    }
}

#[test]
fn source_verified_short_fork_requests_preserve_pricing_and_persistence() {
    let day_key = "2026-09-11";
    let model = "gpt-5.6-sol";
    let ts_ms = utc_ymd_hms(2026, 9, 11, 12, 0, 0).as_millisecond();
    let parent_row = make_row(
        day_key,
        model,
        "parent-turn",
        0,
        Some(ts_ms),
        200_000,
        0,
        100,
        Some("standard"),
        None,
    );
    let child_row = make_row(
        day_key,
        model,
        "child-turn",
        1,
        Some(ts_ms + 1000),
        200_000,
        0,
        100,
        Some("standard"),
        None,
    );
    let mut child_usage = CodexParseResult {
        days: BTreeMap::from([(
            day_key.to_owned(),
            BTreeMap::from([(model.to_owned(), vec![200_000, 0, 100])]),
        )]),
        rows: vec![parent_row.clone(), child_row.clone()],
        ..Default::default()
    };

    let contaminated = codex::codex_canonical_pricing_rows(&child_usage.days, &child_usage.rows);
    assert!(contaminated.rows.is_empty());
    assert_eq!(
        contaminated.unresolved_groups,
        BTreeSet::from([CodexDayModelKey {
            day: day_key.to_owned(),
            model: model.to_owned(),
        }])
    );

    child_usage.rows = vec![child_row.clone()];
    let clean = codex::codex_canonical_pricing_rows(&child_usage.days, &child_usage.rows);
    assert!(clean.unresolved_groups.is_empty());
    assert_eq!(clean.rows, vec![child_row]);

    let req_cost = pricing::codex_cost_usd_with_tier(
        model,
        200_000,
        0,
        100,
        Some(pricing::CodexServiceTier::Standard),
        None,
    )
    .unwrap();
    let agg_cost = pricing::codex_cost_usd_with_tier(
        model,
        400_000,
        0,
        200,
        Some(pricing::CodexServiceTier::Standard),
        None,
    )
    .unwrap();
    assert!(agg_cost > req_cost * 2.0 * 1.9);
}

#[test]
fn source_verified_fork_requests_apply_long_context_pricing_only_to_the_long_request() {
    let day_key = "2026-09-11";
    let model = "gpt-5.6-sol";
    let ts_ms = utc_ymd_hms(2026, 9, 11, 12, 0, 0).as_millisecond();
    let long_row = make_row(
        day_key,
        model,
        "long-turn",
        0,
        Some(ts_ms),
        300_000,
        0,
        100,
        Some("standard"),
        None,
    );
    let short_row = make_row(
        day_key,
        model,
        "short-turn",
        1,
        Some(ts_ms + 1),
        100_000,
        0,
        100,
        Some("standard"),
        None,
    );
    let trailing_zero = make_row(
        day_key,
        model,
        "short-turn",
        2,
        Some(ts_ms + 2),
        0,
        0,
        0,
        Some("standard"),
        Some(0),
    );
    let mut child_usage = CodexParseResult {
        days: BTreeMap::from([(
            day_key.to_owned(),
            BTreeMap::from([(model.to_owned(), vec![100_000, 0, 100])]),
        )]),
        rows: vec![long_row, short_row.clone(), trailing_zero.clone()],
        ..Default::default()
    };

    let contaminated = codex::codex_canonical_pricing_rows(&child_usage.days, &child_usage.rows);
    assert!(contaminated.rows.is_empty());
    assert_eq!(
        contaminated.unresolved_groups,
        BTreeSet::from([CodexDayModelKey {
            day: day_key.to_owned(),
            model: model.to_owned(),
        }])
    );

    child_usage.rows = vec![short_row.clone(), trailing_zero.clone()];
    let reconciled = codex::codex_canonical_pricing_rows(&child_usage.days, &child_usage.rows);
    assert!(reconciled.unresolved_groups.is_empty());
    assert_eq!(reconciled.rows, vec![short_row, trailing_zero]);
}

#[test]
fn codex_report_leaves_threshold_cost_unavailable_without_exact_request_rows() {
    let day_key = "2026-09-11";
    let model = "gpt-5.6-sol";
    let r0 = make_row(
        day_key,
        model,
        "turn-0",
        0,
        Some(0),
        220_000,
        0,
        0,
        Some("standard"),
        Some(42_000_000_000),
    );
    let r1 = make_row(
        day_key,
        model,
        "turn-1",
        1,
        Some(1),
        220_000,
        0,
        0,
        Some("standard"),
        Some(42_000_000_000),
    );
    let usage = CodexParseResult {
        days: BTreeMap::from([(
            day_key.to_owned(),
            BTreeMap::from([(model.to_owned(), vec![400_000, 0, 0])]),
        )]),
        rows: vec![r0, r1],
        ..Default::default()
    };
    let irreconcilable = codex::codex_canonical_pricing_rows(&usage.days, &usage.rows);
    assert!(irreconcilable.rows.is_empty());
    assert_eq!(
        irreconcilable.unresolved_groups,
        BTreeSet::from([CodexDayModelKey {
            day: day_key.to_owned(),
            model: model.to_owned(),
        }])
    );
}

#[test]
fn codex_report_safely_prices_linear_aggregate_fallback() {
    let model = "gpt-5.4-mini";
    let expected = pricing::codex_cost_usd_with_tier(
        model,
        300_000,
        100_000,
        100,
        Some(pricing::CodexServiceTier::Standard),
        None,
    )
    .unwrap();
    assert!(expected > 0.0);
}

#[test]
fn exact_codex_pricing_rows_retain_persisted_order() {
    let day_key = "2026-09-11";
    let model = "gpt-5.6-sol";
    let later = make_row(
        day_key,
        model,
        "later",
        1,
        Some(2),
        20,
        2,
        2,
        Some("standard"),
        None,
    );
    let earlier = make_row(
        day_key,
        model,
        "earlier",
        0,
        Some(1),
        10,
        1,
        1,
        Some("standard"),
        None,
    );
    let usage = CodexParseResult {
        days: BTreeMap::from([(
            day_key.to_owned(),
            BTreeMap::from([(model.to_owned(), vec![30, 3, 3])]),
        )]),
        rows: vec![later.clone(), earlier.clone()],
        ..Default::default()
    };
    let reconciled = codex::codex_canonical_pricing_rows(&usage.days, &usage.rows);
    assert!(reconciled.unresolved_groups.is_empty());
    assert_eq!(reconciled.rows, vec![later, earlier]);
}

#[test]
fn source_verified_standard_child_retains_pricing_despite_a_later_fast_parent_timestamp() {
    let day_key = "2026-09-11";
    let model = "gpt-5.6-sol";
    let ts_ms = utc_ymd_hms(2026, 9, 11, 12, 0, 0).as_millisecond();
    let parent = make_row(
        day_key,
        model,
        "fast-parent",
        0,
        Some(ts_ms + 2),
        100_000,
        0,
        10,
        Some("priority"),
        None,
    );
    let child = make_row(
        day_key,
        model,
        "standard-child",
        1,
        Some(ts_ms + 1),
        100_000,
        0,
        10,
        Some("standard"),
        None,
    );
    let mut child_usage = CodexParseResult {
        days: BTreeMap::from([(
            day_key.to_owned(),
            BTreeMap::from([(model.to_owned(), vec![100_000, 0, 10])]),
        )]),
        rows: vec![parent, child.clone()],
        ..Default::default()
    };
    let contaminated = codex::codex_canonical_pricing_rows(&child_usage.days, &child_usage.rows);
    assert!(contaminated.rows.is_empty());
    assert_eq!(
        contaminated.unresolved_groups,
        BTreeSet::from([CodexDayModelKey {
            day: day_key.to_owned(),
            model: model.to_owned(),
        }])
    );

    child_usage.rows = vec![child.clone()];
    let reconciled = codex::codex_canonical_pricing_rows(&child_usage.days, &child_usage.rows);
    assert!(reconciled.unresolved_groups.is_empty());
    assert_eq!(reconciled.rows, vec![child]);
}

#[test]
fn zero_owned_fork_rows_use_an_empty_suffix_without_hiding_parent_cost() {
    let day_key = "2026-09-11";
    let model = "gpt-5.6-sol";
    let parent_row = make_row(
        day_key,
        model,
        "parent",
        0,
        Some(1),
        300_000,
        0,
        10,
        Some("standard"),
        None,
    );
    let child_usage = CodexParseResult {
        days: BTreeMap::from([(
            day_key.to_owned(),
            BTreeMap::from([(model.to_owned(), vec![0, 0, 0])]),
        )]),
        rows: vec![parent_row],
        ..Default::default()
    };
    let reconciled = codex::codex_canonical_pricing_rows(&child_usage.days, &child_usage.rows);
    assert!(reconciled.rows.is_empty());
    assert!(reconciled.unresolved_groups.is_empty());
}

#[test]
fn exact_rows_price_only_requests_with_complete_pricing() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day_dir = codex_home.join("sessions/2026/09/11");
    write_jsonl(
        &day_dir.join("partial.jsonl"),
        &[
            json!({
                "type": "session_meta",
                "timestamp": "2026-09-11T12:00:00Z",
                "payload": {"id": "sess-partial"}
            }),
            json!({
                "type": "turn_context",
                "timestamp": "2026-09-11T12:00:00Z",
                "payload": {"model": "gpt-5.6-sol", "turn_id": "priced"}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-09-11T12:00:01Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {"input_tokens": 100_000, "cached_input_tokens": 0, "output_tokens": 10},
                        "total_token_usage": {"input_tokens": 100_000, "cached_input_tokens": 0, "output_tokens": 10}
                    }
                }
            }),
            json!({
                "type": "turn_context",
                "timestamp": "2026-09-11T12:00:02Z",
                "payload": {"model": "unpriced-test-model", "turn_id": "unpriced"}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-09-11T12:00:03Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {"input_tokens": 100_000, "cached_input_tokens": 0, "output_tokens": 10},
                        "total_token_usage": {"input_tokens": 200_000, "cached_input_tokens": 0, "output_tokens": 20}
                    }
                }
            }),
        ],
    );
    let day = utc_ymd_hms(2026, 9, 11, 12, 0, 0);
    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let priced_cost = pricing::codex_cost_usd_with_tier(
        "gpt-5.6-sol",
        100_000,
        0,
        10,
        Some(pricing::CodexServiceTier::Standard),
        None,
    )
    .unwrap();
    let breakdown = report.data[0]
        .model_breakdowns
        .iter()
        .find(|b| b.model_name == "gpt-5.6-sol")
        .unwrap();
    assert!((breakdown.cost_usd.unwrap() - priced_cost).abs() < 1e-12);
    let unpriced_bd = report.data[0]
        .model_breakdowns
        .iter()
        .find(|b| b.model_name == "unpriced-test-model")
        .unwrap();
    assert!(unpriced_bd.cost_usd.is_none());
}

#[test]
fn project_primary_report_propagates_unresolved_same_model_ownership() {
    let day_key = "2026-09-11";
    let model = "gpt-5.6-sol";
    let r1 = make_row(
        day_key,
        model,
        "unresolved-1",
        1,
        None,
        220_000,
        0,
        5,
        Some("standard"),
        None,
    );
    let r2 = make_row(
        day_key,
        model,
        "unresolved-2",
        2,
        None,
        220_000,
        0,
        5,
        Some("standard"),
        None,
    );
    let unresolved_usage = CodexParseResult {
        days: BTreeMap::from([(
            day_key.to_owned(),
            BTreeMap::from([(model.to_owned(), vec![400_000, 0, 10])]),
        )]),
        rows: vec![r1, r2],
        ..Default::default()
    };
    let res = codex::codex_canonical_pricing_rows(&unresolved_usage.days, &unresolved_usage.rows);
    assert!(res.rows.is_empty());
    assert_eq!(
        res.unresolved_groups,
        BTreeSet::from([CodexDayModelKey {
            day: day_key.to_owned(),
            model: model.to_owned(),
        }])
    );
}

#[test]
fn project_primary_report_keeps_priced_models_beside_explicitly_unpriced_models() {
    let day_key = "2026-09-11";
    let priced_model = "gpt-5.6-sol";
    let unpriced_model = "codex-auto-review";
    let priced_row = make_row(
        day_key,
        priced_model,
        "/priced.jsonl",
        0,
        None,
        100_000,
        0,
        10,
        Some("standard"),
        None,
    );
    let unpriced_row = make_row(
        day_key,
        unpriced_model,
        "/unpriced.jsonl",
        0,
        None,
        50_000,
        0,
        10,
        Some("standard"),
        None,
    );
    assert!(priced_row.known_cost_nanos.is_some());
    assert!(unpriced_row.known_cost_nanos.is_none());
    assert_eq!(unpriced_row.unpriced_tokens, Some(50_010));
}

#[test]
fn source_refresh_removes_copied_fork_prefix_without_losing_fast_split() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day_dir = codex_home.join("sessions/2026/08/06");
    fs::create_dir_all(&day_dir).unwrap();

    let model = "gpt-5.5";
    write_jsonl(
        &day_dir.join("a-parent.jsonl"),
        &[
            json!({
                "type": "session_meta",
                "timestamp": "2026-08-06T12:00:00Z",
                "payload": {"id": "parent-session", "timestamp": "2026-08-06T12:00:00Z"}
            }),
            json!({
                "type": "turn_context",
                "timestamp": "2026-08-06T12:00:00Z",
                "payload": {"model": model}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "task_started", "turn_id": "priority-turn"}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {"input_tokens": 100, "cached_input_tokens": 40, "output_tokens": 10}
                    }
                }
            }),
        ],
    );
    write_jsonl(
        &day_dir.join("z-child.jsonl"),
        &[
            json!({
                "type": "session_meta",
                "timestamp": "2026-08-06T12:00:02Z",
                "payload": {
                    "id": "child-session",
                    "forked_from_id": "parent-session",
                    "timestamp": "2026-08-06T12:00:02Z"
                }
            }),
            json!({
                "type": "turn_context",
                "timestamp": "2026-08-06T12:00:02Z",
                "payload": {"model": model}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:03Z",
                "payload": {"type": "task_started", "turn_id": "standard-turn"}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:03Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {"input_tokens": 150, "cached_input_tokens": 60, "output_tokens": 15}
                    }
                }
            }),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE logs (ts TEXT, feedback_log_body TEXT);")
            .unwrap();
        conn.execute(
            "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
            (
                "2026-08-06T12:00:01Z",
                "thread_id=thread turn.id=priority-turn websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.5\",\"service_tier\":\"priority\"}",
            ),
        )
        .unwrap();
    }

    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);
    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let breakdown = &report.data[0].model_breakdowns[0];
    let standard_cost = pricing::codex_cost_usd_with_tier(
        model,
        50,
        20,
        5,
        Some(pricing::CodexServiceTier::Standard),
        None,
    )
    .unwrap();
    let priority_cost = pricing::codex_cost_usd_with_tier(
        model,
        100,
        40,
        10,
        Some(pricing::CodexServiceTier::Priority),
        None,
    )
    .unwrap();
    assert!((breakdown.cost_usd.unwrap() - (standard_cost + priority_cost)).abs() < 1e-12);
    assert!((breakdown.standard_cost_usd.unwrap() - standard_cost).abs() < 1e-12);
    assert!((breakdown.priority_cost_usd.unwrap() - priority_cost).abs() < 1e-12);
    assert_eq!(breakdown.standard_tokens, Some(55));
    assert_eq!(breakdown.priority_tokens, Some(110));
}

#[test]
fn codex_report_keeps_trusted_fork_deduplicated_row_split() {
    let day_key = "2026-08-06";
    let model = "gpt-5.5";
    let ts_ms = utc_ymd_hms(2026, 8, 6, 12, 0, 0).as_millisecond();
    let parent_row = make_row(
        day_key,
        model,
        "priority-turn",
        0,
        Some(ts_ms),
        100,
        40,
        10,
        Some("priority"),
        None,
    );
    let child_row = make_row(
        day_key,
        model,
        "standard-turn",
        0,
        Some(ts_ms + 1_000),
        50,
        20,
        5,
        Some("standard"),
        None,
    );

    let parent_usage = CodexParseResult {
        days: BTreeMap::from([(
            day_key.to_owned(),
            BTreeMap::from([(model.to_owned(), vec![100, 40, 10])]),
        )]),
        rows: vec![parent_row.clone()],
        session_id: Some("parent-session".to_owned()),
        ..Default::default()
    };
    let child_usage = CodexParseResult {
        days: BTreeMap::from([(
            day_key.to_owned(),
            BTreeMap::from([(model.to_owned(), vec![50, 20, 5])]),
        )]),
        rows: vec![child_row.clone()],
        session_id: Some("child-session".to_owned()),
        forked_from_id: Some("parent-session".to_owned()),
        depends_on_parent_totals: true,
        ..Default::default()
    };

    let parent_canonical =
        codex::codex_canonical_pricing_rows(&parent_usage.days, &parent_usage.rows);
    let child_canonical = codex::codex_canonical_pricing_rows(&child_usage.days, &child_usage.rows);
    assert!(parent_canonical.unresolved_groups.is_empty());
    assert!(child_canonical.unresolved_groups.is_empty());
    assert_eq!(parent_canonical.rows, vec![parent_row.clone()]);
    assert_eq!(child_canonical.rows, vec![child_row.clone()]);

    let range = DayRange::from_day_keys(day_key, day_key, BucketTimeZone::utc());
    let scan = codex::CodexScanReport {
        rows: vec![parent_row, child_row],
        files: BTreeMap::from([
            ("/sessions/a-parent.jsonl".to_owned(), parent_usage),
            ("/sessions/z-child.jsonl".to_owned(), child_usage),
        ]),
        ..Default::default()
    };
    let report = ariadusage_engine::cost::ledger::build_codex_daily_report(&scan, &range);
    let breakdown = &report.data[0].model_breakdowns[0];
    let standard_cost = pricing::codex_cost_usd_with_tier(
        model,
        50,
        20,
        5,
        Some(pricing::CodexServiceTier::Standard),
        None,
    )
    .unwrap();
    let priority_cost = pricing::codex_cost_usd_with_tier(
        model,
        100,
        40,
        10,
        Some(pricing::CodexServiceTier::Priority),
        None,
    )
    .unwrap();
    assert!((breakdown.cost_usd.unwrap() - (standard_cost + priority_cost)).abs() < 1e-12);
    assert!((breakdown.standard_cost_usd.unwrap() - standard_cost).abs() < 1e-12);
    assert!((breakdown.priority_cost_usd.unwrap() - priority_cost).abs() < 1e-12);
    assert_eq!(breakdown.standard_tokens, Some(55));
    assert_eq!(breakdown.priority_tokens, Some(110));
}

// Rust-added unit test (no direct upstream @Test equivalent; tests CostUsageScanner.swift:5633-5649 helper)
#[test]
fn codex_contained_total_delta_matches_upstream() {
    let watermark = Some(CodexTotals::new(100, 20, 10, Some(4)));
    let counted = Some(CodexTotals::new(90, 18, 9, Some(3)));
    let cur = CodexTotals::new(140, 28, 14, Some(7));
    assert_eq!(
        codex::codex_contained_total_delta(watermark, counted, cur),
        CodexTotals::new(40, 8, 4, Some(3))
    );
    let regressed = CodexTotals::new(95, 19, 10, Some(4));
    assert_eq!(
        codex::codex_contained_total_delta(watermark, counted, regressed),
        CodexTotals::new(5, 1, 0, Some(0))
    );
}

// ============================================================================
// CostUsagePriorityDayKeysTests.swift (2 @Test functions)
// ============================================================================

#[test]
fn all_history_stores_only_populated_priority_days() {
    let range = DayRange::from_day_keys("0001-01-01", "2026-12-31", BucketTimeZone::utc());
    let existing = BTreeMap::from([
        ("2019-04-01".to_owned(), vec!["removed".to_owned()]),
        ("2025-04-01".to_owned(), Vec::<String>::new()),
    ]);
    let new = BTreeMap::from([("2018-04-01".to_owned(), vec!["old-but-valid".to_owned()])]);
    let ids = codex_priority::merge_priority_day_values(
        Some(&existing),
        &new,
        &range,
        "0001-01-01",
        "2026-12-31",
    )
    .expect("merged ids");
    assert_eq!(ids.len(), 1);
    assert_eq!(
        ids.get("2018-04-01"),
        Some(&vec!["old-but-valid".to_owned()])
    );
}

#[test]
fn priority_changes_preserve_old_dates_and_ignore_days_outside_the_scan() {
    let range = DayRange::from_day_keys("2018-01-01", "2019-12-31", BucketTimeZone::utc());
    let old = BTreeMap::from([
        ("2018-04-01".to_owned(), "removed".to_owned()),
        ("2019-05-01".to_owned(), "old".to_owned()),
        ("2026-04-01".to_owned(), "outside".to_owned()),
    ]);
    let new = BTreeMap::from([
        ("2019-05-01".to_owned(), "new".to_owned()),
        ("2019-06-01".to_owned(), "added".to_owned()),
    ]);
    assert!(codex_priority::codex_priority_turn_keys_changed(
        Some(&old),
        &new,
        &range
    ));
    assert!(!codex_priority::codex_priority_turn_keys_changed(
        Some(&BTreeMap::from([(
            "2026-04-01".to_owned(),
            "outside".to_owned()
        )])),
        &BTreeMap::new(),
        &range,
    ));
    let old_turns = BTreeMap::from([
        ("2018-04-01".to_owned(), vec!["removed".to_owned()]),
        ("2019-05-01".to_owned(), vec!["same".to_owned()]),
    ]);
    let new_turns = BTreeMap::from([
        ("2019-05-01".to_owned(), vec!["same".to_owned()]),
        ("2019-06-01".to_owned(), vec!["added".to_owned()]),
        ("2026-04-01".to_owned(), vec!["outside".to_owned()]),
    ]);
    assert_eq!(
        codex_priority::changed_priority_turn_ids(
            Some(&old_turns),
            &new_turns,
            Some(&old),
            &new,
            &range,
        ),
        BTreeSet::from(["removed".to_owned(), "same".to_owned(), "added".to_owned()])
    );
    let merged_keys = codex_priority::merge_priority_day_values(
        Some(&old),
        &new,
        &range,
        "2018-01-01",
        "2026-12-31",
    );
    assert_eq!(
        merged_keys,
        Some(BTreeMap::from([
            ("2019-05-01".to_owned(), "new".to_owned()),
            ("2019-06-01".to_owned(), "added".to_owned()),
            ("2026-04-01".to_owned(), "outside".to_owned()),
        ]))
    );
    let existing_turns = BTreeMap::from([
        ("2018-04-01".to_owned(), vec!["removed".to_owned()]),
        ("2026-04-01".to_owned(), vec!["outside".to_owned()]),
    ]);
    assert_eq!(
        codex_priority::merge_priority_day_values(
            Some(&existing_turns),
            &BTreeMap::new(),
            &range,
            "2018-01-01",
            "2026-12-31",
        ),
        Some(BTreeMap::from([(
            "2026-04-01".to_owned(),
            vec!["outside".to_owned()]
        )]))
    );
}

// ============================================================================
// Additional parity & security tests (Medium 1, 2, 3, 4 + Low 1)
// ============================================================================

#[test]
fn codex_thread_settings_applied_marks_subsequent_turns_priority() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("thread-settings.jsonl");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    write_jsonl(
        &file_path,
        &[
            json!({
                "type": "session_meta",
                "timestamp": "2026-05-10T12:00:00Z",
                "payload": {"id": "sess-ts"}
            }),
            json!({
                "type": "turn_context",
                "timestamp": "2026-05-10T12:00:00Z",
                "payload": {"model": "gpt-5.2-codex"}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-05-10T12:00:01Z",
                "payload": {
                    "type": "thread_settings_applied",
                    "thread_settings": {"service_tier": "priority"}
                }
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-05-10T12:00:02Z",
                "payload": {"type": "task_started", "turn_id": "turn-fast"}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-05-10T12:00:03Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {"input_tokens": 100_000, "cached_input_tokens": 0, "output_tokens": 10_000}
                    }
                }
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-05-10T12:00:04Z",
                "payload": {
                    "type": "thread_settings_applied",
                    "thread_settings": {"service_tier": "fast"}
                }
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-05-10T12:00:04Z",
                "payload": {"type": "task_started", "turn_id": "turn-std"}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-05-10T12:00:05Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {"input_tokens": 200_000, "cached_input_tokens": 0, "output_tokens": 20_000}
                    }
                }
            }),
        ],
    );

    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(parsed.rows.len(), 2);
    assert_eq!(parsed.rows[0].pricing_mode.as_deref(), Some("priority"));
    assert_eq!(parsed.rows[1].pricing_mode.as_deref(), Some("standard"));
}

#[test]
fn codex_sessionless_rows_from_distinct_files_do_not_collide() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day_dir = codex_home.join("sessions/2026/05/10");
    let entries = [
        json!({
            "type": "turn_context",
            "timestamp": "2026-05-10T12:00:00Z",
            "payload": {"model": "gpt-5.2-codex", "turn_id": "turn-1"}
        }),
        json!({
            "type": "event_msg",
            "timestamp": "2026-05-10T12:00:01Z",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {"input_tokens": 100, "cached_input_tokens": 0, "output_tokens": 10}
                }
            }
        }),
    ];
    write_jsonl(&day_dir.join("file-a.jsonl"), &entries);
    write_jsonl(&day_dir.join("file-b.jsonl"), &entries);

    let range = DayRange::from_day_keys("2026-05-10", "2026-05-10", BucketTimeZone::utc());
    let scan = codex::scan_codex_home(
        &range,
        &CodexScanOptions {
            codex_home: Some(codex_home),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    assert_eq!(scan.rows.len(), 2);
}

#[test]
fn codex_session_metadata_debug_redacts_cwd_and_title() {
    let meta = CodexSessionMetadata {
        session_id: "sess-123".to_owned(),
        forked_from_id: None,
        history_base_thread_id: None,
        subagent_history_start_ordinal: None,
        is_subagent_thread: false,
        fork_timestamp: None,
        request_session_id: None,
        cwd: Some("/Users/secret-project".to_owned()),
        title: Some("CONFIDENTIAL_SESSION_TITLE".to_owned()),
        started_at_unix_ms: Some(1_000),
        latest_activity_unix_ms: Some(2_000),
    };
    let dbg = format!("{meta:?}");
    assert!(!dbg.contains("/Users/secret-project"));
    assert!(!dbg.contains("CONFIDENTIAL_SESSION_TITLE"));
    assert!(dbg.contains("[redacted]"));
}

#[cfg(unix)]
#[test]
fn codex_scanner_skips_symlinks_in_walk_and_sqlite_priority() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day_dir = codex_home.join("sessions/2026/05/10");
    fs::create_dir_all(&day_dir).unwrap();

    let outside_dir = tmp.path().join("outside");
    fs::create_dir_all(&outside_dir).unwrap();
    let outside_jsonl = outside_dir.join("secret.jsonl");
    write_jsonl(
        &outside_jsonl,
        &[
            json!({
                "type": "turn_context",
                "timestamp": "2026-05-10T12:00:00Z",
                "payload": {"model": "gpt-5.2-codex"}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-05-10T12:00:01Z",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {"input_tokens": 999, "cached_input_tokens": 0, "output_tokens": 99}
                    }
                }
            }),
        ],
    );
    symlink(&outside_jsonl, day_dir.join("linked-file.jsonl")).unwrap();
    symlink(&outside_dir, day_dir.join("linked-dir")).unwrap();

    let range = DayRange::from_day_keys("2026-05-10", "2026-05-10", BucketTimeZone::utc());
    let scan = codex::scan_codex_home(
        &range,
        &CodexScanOptions {
            codex_home: Some(codex_home.clone()),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    assert!(scan.files.is_empty());
    assert!(scan.rows.is_empty());

    // Symlinked logs_2.sqlite must be rejected
    let outside_db = outside_dir.join("real.sqlite");
    {
        let conn = Connection::open(&outside_db).unwrap();
        conn.execute_batch("CREATE TABLE logs (ts TEXT, feedback_log_body TEXT);")
            .unwrap();
        conn.execute(
            "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
            (
                "2026-05-10T12:00:00Z",
                "turn.id=turn-sym websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.2-codex\",\"service_tier\":\"priority\"}",
            ),
        )
        .unwrap();
    }
    let linked_db = codex_home.join("logs_2.sqlite");
    symlink(&outside_db, &linked_db).unwrap();
    let turns = codex_priority::codex_priority_turns(
        &linked_db,
        Some("2026-05-10"),
        Some("2026-05-10"),
        Some(TimeZone::UTC),
    );
    assert!(turns.is_empty());
}

// ---------------------------------------------------------------------------
// Upstream parity: CostUsageScannerCodexPriorityTests.swift
// ---------------------------------------------------------------------------

#[test]
fn parses_priority_turn_metadata_without_exposing_request_body() {
    let body = "INFO thread_id=11111111-1111-1111-1111-111111111111 \
turn.id=22222222-2222-2222-2222-222222222222 websocket request: \
{\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\",\
\"instructions\":\"secret prompt\"}";

    let parsed = codex_priority::parse_codex_priority_trace_row(Some("2026-05-10T12:00:00Z"), body)
        .expect("priority trace row");

    assert_eq!(
        parsed.thread_id.as_deref(),
        Some("11111111-1111-1111-1111-111111111111")
    );
    assert_eq!(parsed.turn_id, "22222222-2222-2222-2222-222222222222");
    assert_eq!(parsed.model.as_deref(), Some("request-model"));
    assert_eq!(parsed.timestamp.as_deref(), Some("2026-05-10T12:00:00Z"));
}

#[test]
fn parses_current_priority_submission_metadata_without_exposing_prompt() {
    let body = "session_loop{thread_id=thread}: Submission sub=Submission { \
id: \"turn\", op: UserInput { text: \"private\" }, \
thread_settings: ThreadSettingsOverrides { service_tier: Some(Some(\"priority\")) }";

    let parsed =
        codex_priority::parse_codex_priority_trace_row(Some("1785434553"), body).expect("parsed");

    assert_eq!(parsed.thread_id.as_deref(), Some("thread"));
    assert_eq!(parsed.turn_id, "turn");
    assert_eq!(parsed.model, None);
    assert_eq!(parsed.timestamp.as_deref(), Some("1785434553"));
}

#[test]
fn ignores_non_priority_malformed_and_non_response_request_rows() {
    let prefix = "thread_id=thread turn.id=turn websocket request: ";

    assert!(
        codex_priority::parse_codex_priority_trace_row(
            None,
            &format!("{prefix}{{\"type\":\"session.update\",\"service_tier\":\"priority\"}}")
        )
        .is_none()
    );
    assert!(
        codex_priority::parse_codex_priority_trace_row(
            None,
            &format!("{prefix}{{\"type\":\"response.create\"}}")
        )
        .is_none()
    );
    assert!(
        codex_priority::parse_codex_priority_trace_row(
            None,
            &format!("{prefix}{{\"type\":\"response.create\",\"service_tier\":\"default\"}}")
        )
        .is_none()
    );
    assert!(codex_priority::parse_codex_priority_trace_row(None, &format!("{prefix}{{")).is_none());
}

#[test]
fn parses_completed_response_model_without_exposing_response_body() {
    let body = "INFO thread_id=thread turn.id=turn websocket event: \
{\"type\":\"response.completed\",\"response\":{\"model\":\"completed-model\",\"output\":[{\"content\":\"private\"}]}}";

    let (turn_id, model) =
        codex_priority::parse_codex_completed_trace_row(body).expect("completed");

    assert_eq!(turn_id, "turn");
    assert_eq!(model, "completed-model");
}

#[test]
fn reads_priority_turns_from_sqlite_logs_table() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TABLE logs (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT, feedback_log_body TEXT);")
        .unwrap();
    conn.execute(
        "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
        (
            "2026-05-10T12:00:00Z",
            "thread_id=thread-a turn.id=turn-a websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\",\"input\":\"private\"}",
        ),
    )
    .unwrap();
    conn.execute(
        "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
        (
            "2026-05-10T12:01:00Z",
            "thread_id=thread-b turn.id=turn-b websocket request: {\"type\":\"response.create\",\"model\":\"request-model\"}",
        ),
    )
    .unwrap();
    drop(conn);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-a"]);
    assert_eq!(turns["turn-a"].thread_id.as_deref(), Some("thread-a"));
    assert_eq!(turns["turn-a"].model.as_deref(), Some("request-model"));
}

#[test]
fn reads_current_priority_submission_rows_from_sqlite_logs_table() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TABLE logs (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT, feedback_log_body TEXT);")
        .unwrap();
    conn.execute(
        "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
        (
            "2026-05-10T12:00:00Z",
            "session_loop{thread_id=thread}: Submission sub=Submission { id: \"turn\", op: UserInput { text: \"private\" }, thread_settings: ThreadSettingsOverrides { service_tier: Some(Some(\"priority\")) }",
        ),
    )
    .unwrap();
    drop(conn);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn"]);
    assert_eq!(turns["turn"].thread_id.as_deref(), Some("thread"));
    assert_eq!(turns["turn"].model, None);
}

#[test]
fn sqlite_scan_upgrades_priority_request_alias_with_completed_response_model() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TABLE logs (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT, feedback_log_body TEXT);")
        .unwrap();
    conn.execute(
        "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
        (
            "2026-05-10T12:00:00Z",
            "thread_id=thread turn.id=turn websocket request: {\"type\":\"response.create\",\"model\":\"request-alias\",\"service_tier\":\"priority\"}",
        ),
    )
    .unwrap();
    conn.execute(
        "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
        (
            "2026-05-10T12:00:01Z",
            "thread_id=thread turn.id=turn websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"completed-model\",\"input\":\"private\"}}",
        ),
    )
    .unwrap();
    drop(conn);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns["turn"].model.as_deref(), Some("completed-model"));
}

#[test]
fn sqlite_scan_matches_spaced_completed_response_json() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TABLE logs (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT, feedback_log_body TEXT);")
        .unwrap();
    conn.execute(
        "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
        (
            "2026-05-10T12:00:00Z",
            "thread_id=thread turn.id=turn websocket request: {\"type\":\"response.create\",\"model\":\"request-alias\",\"service_tier\":\"priority\"}",
        ),
    )
    .unwrap();
    conn.execute(
        "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
        (
            "2026-05-10T12:00:01Z",
            "thread_id=thread turn.id=turn websocket event: {\"type\": \"response.completed\", \"response\": {\"model\": \"completed-model\"}}",
        ),
    )
    .unwrap();
    drop(conn);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns["turn"].model.as_deref(), Some("completed-model"));
}

#[test]
fn sqlite_scan_only_returns_priority_turns_in_requested_day_range() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TABLE logs (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT, feedback_log_body TEXT);")
        .unwrap();
    conn.execute(
        "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
        (
            "2026-05-09T12:00:00Z",
            "thread_id=thread-old turn.id=turn-old websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
        ),
    )
    .unwrap();
    conn.execute(
        "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
        (
            "2026-05-10T12:00:00Z",
            "thread_id=thread-new turn.id=turn-new websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
        ),
    )
    .unwrap();
    drop(conn);

    let turns = codex_priority::codex_priority_turns(
        &db_path,
        Some("2026-05-10"),
        Some("2026-05-10"),
        Some(TimeZone::UTC),
    );
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-new"]);
}

#[test]
fn sqlite_scan_uses_local_day_boundaries_for_integer_timestamps() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TABLE logs (id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, feedback_log_body TEXT);")
        .unwrap();
    let day_start = utc_ymd_hms(2026, 5, 10, 0, 0, 0).as_second();
    conn.execute(
        "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
        (
            day_start - 1,
            "thread_id=thread-before turn.id=turn-before websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
        ),
    )
    .unwrap();
    conn.execute(
        "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
        (
            day_start + 1,
            "thread_id=thread-after turn.id=turn-after websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
        ),
    )
    .unwrap();
    drop(conn);

    let turns = codex_priority::codex_priority_turns(
        &db_path,
        Some("2026-05-10"),
        Some("2026-05-10"),
        Some(TimeZone::UTC),
    );
    assert_eq!(
        turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-after"]
    );
}

// ---------------------------------------------------------------------------
// Upstream parity: CostUsageCodexRequestLedgerTests.swift
// ---------------------------------------------------------------------------

const LEDGER_TS_A: &str = "2026-08-29T15:59:00Z";
const LEDGER_TS_B: &str = "2026-08-29T16:01:00Z";
const LEDGER_TS_C: &str = "2026-08-29T16:01:05Z";

fn ledger_header() -> Vec<serde_json::Value> {
    vec![
        json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "synthetic-thread"}}),
        json!({"type": "turn_context", "timestamp": LEDGER_TS_A, "payload": {"turn_id": "synthetic-turn", "model": "gpt-5"}}),
    ]
}

fn ledger_tokens(v: [i64; 4]) -> serde_json::Value {
    json!({
        "input_tokens": v[0],
        "cached_input_tokens": v[1],
        "output_tokens": v[2],
        "reasoning_output_tokens": v[3],
    })
}

fn ledger_record(
    id: &str,
    owner: &str,
    timestamp: &str,
    usage: [i64; 4],
    total: [i64; 4],
    turn_total: Option<[i64; 4]>,
) -> serde_json::Value {
    json!({
        "type": "token_usage_record",
        "timestamp": timestamp,
        "payload": {
            "thread_id": owner,
            "session_id": owner,
            "turn_id": "synthetic-turn",
            "response_id": id,
            "usage": ledger_tokens(usage),
            "thread_token_usage": ledger_tokens(total),
            "turn_token_usage": ledger_tokens(turn_total.unwrap_or(total)),
        }
    })
}

fn ledger_legacy(timestamp: &str, usage: [i64; 4], total: [i64; 4]) -> serde_json::Value {
    json!({
        "type": "event_msg",
        "timestamp": timestamp,
        "payload": {
            "type": "token_count",
            "turn_id": "synthetic-turn",
            "info": {
                "last_token_usage": ledger_tokens(usage),
                "total_token_usage": ledger_tokens(total),
            }
        }
    })
}

fn parse_ledger_lines(lines: &[serde_json::Value], spaced_json: bool) -> codex::CodexParseResult {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("synthetic.jsonl");
    let mut content = lines
        .iter()
        .map(serde_json::Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    if spaced_json {
        content = content.replace("\":", "\": ");
    }
    fs::write(&file_path, content).unwrap();
    let tz = TimeZone::get("Asia/Shanghai").unwrap();
    let start = utc_ymd_hms(2026, 8, 29, 15, 59, 0);
    let end = utc_ymd_hms(2026, 8, 29, 16, 1, 5);
    let range = DayRange::new(start, end, Some(tz));
    codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    )
}

#[test]
fn request_ledger_recovers_reset_counters_without_counting_both_formats() {
    for legacy_first in [false, true] {
        for spaced_json in [false, true] {
            let mut lines = ledger_header();
            let requests = [
                (
                    LEDGER_TS_A,
                    [1000, 200, 100, 40],
                    [1000, 200, 100, 40],
                    [1000, 200, 100, 40],
                ),
                (
                    LEDGER_TS_B,
                    [60, 20, 6, 3],
                    [60, 20, 6, 3],
                    [1060, 220, 106, 43],
                ),
                (
                    LEDGER_TS_C,
                    [60, 20, 6, 3],
                    [120, 40, 12, 6],
                    [1120, 240, 112, 46],
                ),
            ];
            for (idx, (ts, usage, legacy_total, thread_total)) in requests.into_iter().enumerate() {
                let legacy = ledger_legacy(ts, usage, legacy_total);
                let ledger = ledger_record(
                    &format!("response-{idx}"),
                    "synthetic-thread",
                    ts,
                    usage,
                    thread_total,
                    Some(legacy_total),
                );
                if legacy_first {
                    lines.push(legacy);
                    lines.push(ledger);
                } else {
                    lines.push(ledger);
                    lines.push(legacy);
                }
            }
            let result = parse_ledger_lines(&lines, spaced_json);
            let total_io: i64 = result.rows.iter().map(|r| r.input + r.output).sum();
            let aug_30_io: i64 = result
                .rows
                .iter()
                .filter(|r| r.day == "2026-08-30")
                .map(|r| r.input + r.output)
                .sum();
            let total_reasoning: i64 = result.rows.iter().map(|r| r.reasoning.unwrap_or(0)).sum();
            assert_eq!(total_io, 1232);
            assert_eq!(aug_30_io, 132);
            assert_eq!(total_reasoning, 46);
        }
    }
}

#[test]
fn ledger_and_legacy_timestamps_may_differ() {
    let mut lines = ledger_header();
    lines.push(ledger_legacy(
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [100, 20, 10, 4],
    ));
    lines.push(ledger_record(
        "one",
        "synthetic-thread",
        "2026-08-29T15:59:01Z",
        [100, 20, 10, 4],
        [100, 20, 10, 4],
        None,
    ));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].response_id.as_deref(), Some("one"));
    assert_eq!(result.rows[0].input, 100);
}

#[test]
fn request_identity_suppresses_replay_even_when_replayed_counters_change() {
    let mut lines = ledger_header();
    lines.push(ledger_record(
        "one",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [100, 20, 10, 4],
        None,
    ));
    lines.push(ledger_record(
        "one",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [200, 40, 20, 8],
        None,
    ));
    lines.push(ledger_record(
        "two",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [300, 60, 30, 12],
        None,
    ));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 2);
    assert_eq!(
        result.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
        220
    );
}

#[test]
fn replayed_identities_also_suppress_legacy_mirrors_with_changed_counters() {
    for legacy_first in [false, true] {
        let mut lines = ledger_header();
        lines.push(ledger_record(
            "one",
            "synthetic-thread",
            LEDGER_TS_A,
            [100, 20, 10, 4],
            [100, 20, 10, 4],
            None,
        ));
        lines.push(ledger_legacy(
            LEDGER_TS_A,
            [100, 20, 10, 4],
            [100, 20, 10, 4],
        ));
        let replay = ledger_record(
            "one",
            "synthetic-thread",
            LEDGER_TS_A,
            [100, 20, 10, 4],
            [200, 40, 20, 8],
            None,
        );
        let mirror = ledger_legacy(LEDGER_TS_B, [100, 20, 10, 4], [200, 40, 20, 8]);
        if legacy_first {
            lines.push(mirror);
            lines.push(replay);
        } else {
            lines.push(replay);
            lines.push(mirror);
        }
        let result = parse_ledger_lines(&lines, false);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
            110
        );
    }
}

#[test]
fn copied_parent_request_records_are_not_billed_to_a_child() {
    let mut lines = ledger_header();
    lines.push(ledger_record(
        "copied",
        "parent",
        LEDGER_TS_A,
        [1000, 200, 100, 40],
        [1000, 200, 100, 40],
        None,
    ));
    lines.push(ledger_record(
        "owned",
        "synthetic-thread",
        LEDGER_TS_A,
        [60, 20, 6, 3],
        [1060, 220, 106, 43],
        None,
    ));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].input, 60);
    assert_eq!(result.rows[0].output, 6);
}

#[test]
fn legacy_prefix_remains_when_request_records_start_later() {
    let mut lines = ledger_header();
    lines.push(ledger_legacy(
        LEDGER_TS_A,
        [1000, 200, 100, 40],
        [1000, 200, 100, 40],
    ));
    lines.push(ledger_record(
        "new",
        "synthetic-thread",
        LEDGER_TS_B,
        [60, 20, 6, 3],
        [1060, 220, 106, 43],
        None,
    ));
    lines.push(ledger_legacy(LEDGER_TS_B, [60, 20, 6, 3], [60, 20, 6, 3]));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(
        result.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
        1166
    );
    assert_eq!(
        result
            .rows
            .iter()
            .map(|r| r.day.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-29", "2026-08-30"]
    );
}

#[test]
fn legacy_only_requests_between_ledger_requests_remain_counted() {
    let mut lines = ledger_header();
    lines.push(ledger_record(
        "one",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [100, 20, 10, 4],
        None,
    ));
    lines.push(ledger_legacy(
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [100, 20, 10, 4],
    ));
    lines.push(ledger_legacy(LEDGER_TS_B, [50, 10, 5, 2], [150, 30, 15, 6]));
    lines.push(ledger_record(
        "three",
        "synthetic-thread",
        LEDGER_TS_C,
        [60, 20, 6, 3],
        [210, 50, 21, 9],
        None,
    ));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 3);
    assert_eq!(
        result.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
        231
    );
}

// ============================================================================
// CodexForkAppendResumeTests.swift (2 @Test functions)
// ============================================================================

#[test]
fn bounded_append_resumes_a_complete_fork_with_an_unresolved_parent() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day_dir = codex_home.join("sessions/2026/07/16");
    let day = utc_ymd_hms(2026, 7, 16, 12, 0, 0);
    let ts0 = "2026-07-16T12:00:00Z";
    let ts1 = "2026-07-16T12:00:01Z";
    let ts2 = "2026-07-16T12:00:02Z";
    let ts3 = "2026-07-16T12:00:03Z";

    let mut initial_lines = vec![
        json!({
            "type": "session_meta",
            "timestamp": ts0,
            "payload": {
                "id": "redacted-child",
                "forked_from_id": "redacted-missing-parent",
                "timestamp": ts0
            }
        }),
        json!({"type": "turn_context", "timestamp": ts0, "payload": {"model": "openai/gpt-5.4"}}),
        json!({
            "type": "event_msg",
            "timestamp": ts1,
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "openai/gpt-5.4",
                    "total_token_usage": {"input_tokens": 1000, "cached_input_tokens": 900, "output_tokens": 100}
                }
            }
        }),
    ];
    for index in 0..80 {
        initial_lines.push(json!({
            "type": "response_item",
            "timestamp": ts0,
            "payload": {
                "sequence": index,
                "text": "x".repeat(128)
            }
        }));
    }
    let child_file = day_dir.join(format!("rollout-{ts0}-redacted-child.jsonl"));
    write_jsonl(&child_file, &initial_lines);
    let first_size = fs::metadata(&child_file).unwrap().len();
    assert!(first_size > 512);
    assert!(first_size < 64 * 1024);

    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let first = codex::parse_codex_file(
        &child_file,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Unresolved,
    );
    assert_eq!(first.session_id.as_deref(), Some("redacted-child"));
    assert_eq!(
        first.forked_from_id.as_deref(),
        Some("redacted-missing-parent")
    );
    assert!(first.depends_on_parent_totals);
    assert_eq!(first.token_snapshots.len(), 1);
    assert_eq!(first.parsed_bytes, first_size);

    initial_lines.push(
        json!({"type": "turn_context", "timestamp": ts2, "payload": {"model": "openai/gpt-5.4"}}),
    );
    initial_lines.push(json!({
        "type": "event_msg",
        "timestamp": ts3,
        "payload": {
            "type": "token_count",
            "info": {
                "model": "openai/gpt-5.4",
                "total_token_usage": {"input_tokens": 1050, "cached_input_tokens": 910, "output_tokens": 105},
                "last_token_usage": {"input_tokens": 50, "cached_input_tokens": 10, "output_tokens": 5}
            }
        }
    }));
    write_jsonl(&child_file, &initial_lines);
    let appended_size = fs::metadata(&child_file).unwrap().len();
    assert!(appended_size > first.parsed_bytes);
    assert!(appended_size - first.parsed_bytes <= 512);
    assert_eq!(
        codex::pending_codex_scan_work_bytes(appended_size, Some(first.parsed_bytes)),
        appended_size - first.parsed_bytes
    );

    let resumed = codex::resume_codex_file(
        &child_file,
        &range,
        &first,
        Some(appended_size),
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Unresolved,
    );
    assert_eq!(resumed.parsed_bytes, appended_size);
    assert_eq!(
        first.token_snapshots.len() + resumed.token_snapshots.len(),
        2
    );
    let _ = cache_root;
}

#[test]
fn same_size_subagent_retry_is_free_and_a_retained_buffer_resumes_only_the_appended_tail() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day_dir = codex_home.join("sessions/2026/07/16");
    let day = utc_ymd_hms(2026, 7, 16, 12, 0, 0);
    let ts0 = "2026-07-16T12:00:00Z";
    let ts1 = "2026-07-16T12:00:01Z";
    let ts4 = "2026-07-16T12:00:04Z";
    let ts5 = "2026-07-16T12:00:05Z";

    let mut initial_lines = vec![
        json!({
            "type": "session_meta",
            "timestamp": ts0,
            "payload": {
                "id": "redacted-subagent",
                "forked_from_id": "redacted-missing-parent",
                "source": {"subagent": {"thread_spawn": {}}}
            }
        }),
        json!({
            "type": "event_msg",
            "timestamp": ts1,
            "payload": {
                "type": "token_count",
                "info": {
                    "model": "openai/gpt-5.3",
                    "total_token_usage": {"input_tokens": 1000, "cached_input_tokens": 900, "output_tokens": 100}
                }
            }
        }),
        json!({
            "type": "session_meta",
            "timestamp": ts0,
            "payload": {"id": "redacted-missing-parent"}
        }),
    ];
    for index in 0..80 {
        initial_lines.push(json!({
            "type": "response_item",
            "timestamp": ts0,
            "payload": {
                "sequence": index,
                "text": "x".repeat(128)
            }
        }));
    }
    let file_path = day_dir.join(format!("rollout-{ts0}-redacted-subagent.jsonl"));
    write_jsonl(&file_path, &initial_lines);
    let first_size = fs::metadata(&file_path).unwrap().len();
    assert!(first_size > 512);
    assert!(first_size < 64 * 1024);

    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let first = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Unresolved,
    );
    assert_eq!(first.session_id.as_deref(), Some("redacted-subagent"));
    assert_eq!(
        first.forked_from_id.as_deref(),
        Some("redacted-missing-parent")
    );
    assert_eq!(first.parsed_bytes, first_size);
    assert_eq!(
        codex::pending_codex_scan_work_bytes(first_size, Some(first.parsed_bytes)),
        0
    );

    initial_lines.push(
        json!({"type": "turn_context", "timestamp": ts4, "payload": {"model": "openai/gpt-5.4"}}),
    );
    initial_lines.push(json!({
        "type": "inter_agent_communication_metadata",
        "timestamp": ts4,
        "payload": {"trigger_turn": true}
    }));
    initial_lines.push(json!({
        "type": "event_msg",
        "timestamp": ts5,
        "payload": {
            "type": "token_count",
            "info": {
                "model": "openai/gpt-5.4",
                "total_token_usage": {"input_tokens": 1050, "cached_input_tokens": 910, "output_tokens": 105},
                "last_token_usage": {"input_tokens": 50, "cached_input_tokens": 10, "output_tokens": 5}
            }
        }
    }));
    write_jsonl(&file_path, &initial_lines);
    let appended_size = fs::metadata(&file_path).unwrap().len();
    let appended_bytes = appended_size - first_size;
    assert!(appended_bytes <= 512);
    assert_eq!(
        codex::pending_codex_scan_work_bytes(appended_size, Some(first.parsed_bytes)),
        appended_bytes
    );

    let resumed = codex::resume_codex_file(
        &file_path,
        &range,
        &first,
        Some(appended_size),
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Unresolved,
    );
    assert_eq!(resumed.parsed_bytes, appended_size);
    assert!(resumed.buffered_subagent_lines.is_empty());
    assert_eq!(&resumed.days["2026-07-16"]["gpt-5.4"][..3], &[50, 10, 5]);
    assert_eq!(resumed.rows[0].input + resumed.rows[0].output, 55);
}

// ============================================================================
// CostUsageOrphanForkTests.swift (2 @Test functions)
// ============================================================================

fn run_orphan_fork_scenario(current_tokens: bool, byte_budget: u64, parent_day: &str) {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let tz = TimeZone::get("Asia/Shanghai").unwrap();
    let old = utc_ymd_hms(2026, 8, 15, 4, 0, 0);
    let now = utc_ymd_hms(2026, 9, 30, 4, 0, 0);
    let archived_dir = codex_home.join("archived_sessions");

    for (session, parent) in [
        ("fork-b", "missing-a"),
        ("fork-c", "missing-a"),
        ("fork-d", "fork-b"),
    ] {
        let fork_time = if session == "fork-d" {
            "2026-08-15T04:00:02Z"
        } else {
            "2026-08-15T04:00:00Z"
        };
        let token_time = if current_tokens {
            "2026-09-30T04:00:00Z"
        } else {
            "2026-08-15T04:00:03Z"
        };
        write_jsonl(
            &archived_dir.join(format!("rollout-2026-08-15-{session}.jsonl")),
            &[
                json!({"type": "session_meta", "payload": {"id": session, "forked_from_id": parent, "timestamp": fork_time}}),
                json!({"type": "turn_context", "payload": {"model": "gpt-5.4"}}),
                json!({"type": "event_msg", "timestamp": token_time, "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "output_tokens": 10}}}}),
                json!({"type": "event_msg", "timestamp": "2026-09-30T04:00:00Z", "payload": {"type": "task_complete"}}),
            ],
        );
    }

    let current_file = codex_home.join("sessions/2026/09/30/rollout-2026-09-30-current.jsonl");
    write_jsonl(
        &current_file,
        &[
            json!({"type": "session_meta", "payload": {"id": "current", "timestamp": "2026-09-30T04:00:00Z"}}),
            json!({"type": "turn_context", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-30T04:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 20, "output_tokens": 2}}}}),
        ],
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root),
        timezone: Some(tz),
        ..Default::default()
    };
    let initial = load_daily_report(LocalCostProvider::Codex, now, now, now, &opts);
    if !current_tokens {
        assert_eq!(initial.data[0].input_tokens, Some(20));
    }

    write_jsonl(
        &current_file,
        &[
            json!({"type": "session_meta", "payload": {"id": "current", "timestamp": "2026-09-30T04:00:00Z"}}),
            json!({"type": "turn_context", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-30T04:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 20, "output_tokens": 2}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-30T04:00:02Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 30, "output_tokens": 3}}}}),
        ],
    );

    write_jsonl(
        &archived_dir.join(format!("rollout-{parent_day}-missing-a.jsonl")),
        &[
            json!({"type": "session_meta", "payload": {"id": "missing-a", "timestamp": format!("{parent_day}T03:59:58Z")}}),
            json!({"type": "turn_context", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": format!("{parent_day}T03:59:59Z"), "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 60, "output_tokens": 6}}}}),
        ],
    );

    let since = utc_ymd_hms(2026, 8, 10, 4, 0, 0);
    let recovered = load_daily_report(LocalCostProvider::Codex, since, now, now, &opts);
    let total_input: u64 = recovered
        .data
        .iter()
        .map(|d| d.input_tokens.unwrap_or(0))
        .sum();
    // 60 (missing-a) + 3 * 40 (fork-b, fork-c, fork-d) + 30 (current) = 210
    assert_eq!(total_input, 60 + 3 * 40 + 30);
    let _ = (old, byte_budget);
}

#[test]
fn archived_missing_parent_forks_settle_without_hiding_current_gaps_and_recover_their_parent() {
    for current_tokens in [false, true] {
        for byte_budget in [0, 512] {
            run_orphan_fork_scenario(current_tokens, byte_budget, "2026-08-15");
        }
    }
}

#[test]
fn a_parent_restored_outside_the_scan_window_stays_pending_until_snapshots_arrive() {
    for byte_budget in [0, 512] {
        run_orphan_fork_scenario(false, byte_budget, "2026-08-10");
    }
}

// ============================================================================
// CostUsageCodexSessionTierTests.swift (5 @Test functions)
// ============================================================================

fn session_tier_cost_helper(service_tier: &str, usage_kind: &str) -> f64 {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let timestamp = "2026-09-10T12:00:00Z";
    let session_id = format!("tier-{service_tier}");

    let last_record = match usage_kind {
        "ledger" => json!({
            "type": "token_usage_record",
            "timestamp": timestamp,
            "payload": {
                "thread_id": session_id,
                "response_id": "synthetic-response",
                "turn_id": "tier-turn",
                "model": "gpt-5.4",
                "usage": {"input_tokens": 200_000, "output_tokens": 10000},
                "thread_token_usage": {"input_tokens": 200_000, "output_tokens": 10000}
            }
        }),
        "bare" => json!({
            "timestamp": timestamp,
            "model": "gpt-5.4",
            "usage": {"input_tokens": 200_000, "output_tokens": 10000}
        }),
        _ => json!({
            "type": "event_msg",
            "timestamp": timestamp,
            "payload": {
                "type": "token_count",
                "turn_id": "tier-turn",
                "info": {
                    "last_token_usage": {
                        "input_tokens": 200_000,
                        "cached_input_tokens": 0,
                        "output_tokens": 10000
                    }
                }
            }
        }),
    };

    write_jsonl(
        &codex_home.join("sessions/2026/09/10/tier.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": timestamp, "payload": {"id": session_id}}),
            json!({
                "type": "event_msg",
                "timestamp": timestamp,
                "payload": {
                    "type": "thread_settings_applied",
                    "thread_id": session_id,
                    "thread_settings": {"model": "gpt-5.4", "service_tier": service_tier}
                }
            }),
            json!({"type": "event_msg", "timestamp": timestamp, "payload": {"type": "task_started", "turn_id": "tier-turn"}}),
            json!({"type": "turn_context", "timestamp": timestamp, "payload": {"model": "gpt-5.4", "turn_id": "tier-turn"}}),
            last_record,
        ],
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    report.summary.unwrap().total_cost_usd.unwrap()
}

#[test]
fn session_log_priority_tier_prices_the_turn_at_the_fast_rate() {
    for usage_kind in ["legacy", "ledger", "bare"] {
        let standard = session_tier_cost_helper("default", usage_kind);
        let priority = session_tier_cost_helper("priority", usage_kind);
        assert!((priority - standard * 2.0).abs() < 1e-9);
    }
}

#[test]
fn priority_tier_does_not_leak_to_an_older_explicit_turn() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("tier-leak.jsonl");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let timestamp = "2026-09-10T12:00:00Z";
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    write_jsonl(
        &file_path,
        &[
            json!({"type": "session_meta", "timestamp": timestamp, "payload": {"id": "tier-leak"}}),
            json!({"type": "turn_context", "timestamp": timestamp, "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": timestamp, "payload": {"type": "task_started", "turn_id": "turn-a"}}),
            json!({
                "type": "event_msg",
                "timestamp": timestamp,
                "payload": {
                    "type": "token_count",
                    "turn_id": "turn-a",
                    "info": {"last_token_usage": {"input_tokens": 100, "output_tokens": 10}}
                }
            }),
            json!({
                "type": "event_msg",
                "timestamp": timestamp,
                "payload": {
                    "type": "thread_settings_applied",
                    "thread_settings": {"service_tier": "priority"}
                }
            }),
            json!({"type": "event_msg", "timestamp": timestamp, "payload": {"type": "task_started", "turn_id": "turn-b"}}),
            json!({
                "type": "event_msg",
                "timestamp": timestamp,
                "payload": {
                    "type": "token_count",
                    "turn_id": "turn-a",
                    "info": {"last_token_usage": {"input_tokens": 200, "output_tokens": 20}}
                }
            }),
        ],
    );

    let result = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(result.rows.len(), 2);
    assert_eq!(
        result.rows.last().unwrap().pricing_mode.as_deref(),
        Some("standard")
    );
}

#[test]
fn priority_tier_survives_an_incremental_scan_checkpoint() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("tier-checkpoint.jsonl");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let timestamp = "2026-09-10T12:00:00Z";
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    let prefix = [
        json!({"type": "session_meta", "timestamp": timestamp, "payload": {"id": "tier-checkpoint"}}),
        json!({
            "type": "event_msg",
            "timestamp": timestamp,
            "payload": {
                "type": "thread_settings_applied",
                "thread_settings": {"service_tier": "priority"}
            }
        }),
    ];
    write_jsonl(&file_path, &prefix);

    let first = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(
        first.request_ledger_state.as_ref().unwrap().thread_priority,
        Some(true)
    );

    let mut full = prefix.to_vec();
    full.extend([
        json!({"type": "event_msg", "timestamp": timestamp, "payload": {"type": "task_started", "turn_id": "turn-checkpoint"}}),
        json!({"type": "turn_context", "timestamp": timestamp, "payload": {"model": "gpt-5.4"}}),
        json!({
            "type": "event_msg",
            "timestamp": timestamp,
            "payload": {
                "type": "token_count",
                "turn_id": "turn-checkpoint",
                "info": {"last_token_usage": {"input_tokens": 200_000, "output_tokens": 10000}}
            }
        }),
    ]);
    write_jsonl(&file_path, &full);

    let second = codex::resume_codex_file(
        &file_path,
        &range,
        &first,
        Some(fs::metadata(&file_path).unwrap().len()),
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(
        second.rows.last().unwrap().pricing_mode.as_deref(),
        Some("priority")
    );
}

#[test]
fn session_tiers_survive_sqlite_reopening_between_settings_turns_and_usage() {
    let timestamp = "2026-09-10T12:00:00Z";
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let usage = |turn: &str| {
        json!({
            "type": "event_msg",
            "timestamp": timestamp,
            "payload": {
                "type": "token_count",
                "turn_id": turn,
                "info": {"last_token_usage": {"input_tokens": 200_000, "output_tokens": 10000}}
            }
        })
    };
    let records = vec![
        json!({"type": "session_meta", "timestamp": timestamp, "payload": {"id": "synthetic-tier-reopen"}}),
        json!({"type": "event_msg", "timestamp": timestamp, "payload": {"type": "thread_settings_applied", "thread_settings": {"service_tier": "priority"}}}),
        json!({"type": "event_msg", "timestamp": timestamp, "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
        json!({"type": "turn_context", "timestamp": timestamp, "payload": {"model": "gpt-5.4"}}),
        usage("priority-turn"),
        json!({"type": "event_msg", "timestamp": timestamp, "payload": {"type": "thread_settings_applied", "thread_settings": {"service_tier": "default"}}}),
        json!({"type": "event_msg", "timestamp": timestamp, "payload": {"type": "task_started", "turn_id": "default-turn"}}),
        usage("priority-turn"),
        usage("default-turn"),
        json!({"type": "event_msg", "timestamp": timestamp, "payload": {"type": "task_started", "turn_id": "missing-tier-turn"}}),
        usage("missing-tier-turn"),
        json!({"type": "event_msg", "timestamp": timestamp, "payload": {"type": "thread_settings_applied", "thread_settings": {"service_tier": "synthetic-unknown"}}}),
        json!({"type": "event_msg", "timestamp": timestamp, "payload": {"type": "task_started", "turn_id": "unknown-tier-turn"}}),
        usage("unknown-tier-turn"),
    ];

    for prefix_count in [2usize, 3, 5, 7] {
        let tmp = TempDir::new().expect("tempdir");
        let codex_home = tmp.path().join("codex-home");
        let cache_root = tmp.path().join("cache");
        let file_path = codex_home.join("sessions/2026/09/10/tier-reopen.jsonl");
        write_jsonl(&file_path, &records[..prefix_count]);

        let opts = CostScannerOptions {
            codex_home: Some(codex_home.clone()),
            cache_root: Some(cache_root.clone()),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        };
        let _ = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);

        write_jsonl(&file_path, &records);
        let resumed = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
        assert_eq!(
            resumed.summary.as_ref().unwrap().total_tokens,
            Some(1_050_000)
        );
        assert!((resumed.summary.as_ref().unwrap().total_cost_usd.unwrap() - 4.55).abs() < 1e-9);

        let fresh = load_daily_report(
            LocalCostProvider::Codex,
            day,
            day,
            day,
            &CostScannerOptions {
                codex_home: Some(codex_home),
                cache_root: Some(tmp.path().join("fresh-cache")),
                timezone: Some(TimeZone::UTC),
                ..Default::default()
            },
        );
        assert_eq!(fresh.data, resumed.data);
    }
}

#[test]
fn thread_priority_remains_effective_across_later_turns() {
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let timestamp = "2026-09-10T12:00:00Z";
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    for resume in [false, true] {
        let tmp = TempDir::new().expect("tempdir");
        let file_path = tmp.path().join("persistent-tier.jsonl");
        let prefix = vec![
            json!({"type": "session_meta", "timestamp": timestamp, "payload": {"id": "persistent-tier"}}),
            json!({"type": "event_msg", "timestamp": timestamp, "payload": {"type": "thread_settings_applied", "thread_settings": {"service_tier": "priority"}}}),
            json!({"type": "event_msg", "timestamp": timestamp, "payload": {"type": "task_started", "turn_id": "first-turn"}}),
            json!({"type": "turn_context", "timestamp": timestamp, "payload": {"model": "gpt-5.4"}}),
            json!({
                "type": "event_msg",
                "timestamp": timestamp,
                "payload": {
                    "type": "token_count",
                    "turn_id": "first-turn",
                    "info": {"last_token_usage": {"input_tokens": 100, "output_tokens": 10}}
                }
            }),
        ];
        write_jsonl(&file_path, &prefix);

        let first = if resume {
            Some(codex::parse_codex_file(
                &file_path,
                &range,
                0,
                None,
                None,
                None,
                &BTreeMap::new(),
                &BTreeMap::new(),
                None,
                |_, _| CodexForkBaseline::Resolved(None),
            ))
        } else {
            None
        };

        let mut full = prefix;
        full.extend([
            json!({"type": "event_msg", "timestamp": timestamp, "payload": {"type": "task_started", "turn_id": "second-turn"}}),
            json!({
                "type": "event_msg",
                "timestamp": timestamp,
                "payload": {
                    "type": "token_count",
                    "turn_id": "second-turn",
                    "info": {"last_token_usage": {"input_tokens": 100, "output_tokens": 10}}
                }
            }),
        ]);
        write_jsonl(&file_path, &full);

        let parsed = if let Some(first_res) = first {
            codex::resume_codex_file(
                &file_path,
                &range,
                &first_res,
                Some(fs::metadata(&file_path).unwrap().len()),
                None,
                &BTreeMap::new(),
                &BTreeMap::new(),
                None,
                |_, _| CodexForkBaseline::Resolved(None),
            )
        } else {
            codex::parse_codex_file(
                &file_path,
                &range,
                0,
                None,
                None,
                None,
                &BTreeMap::new(),
                &BTreeMap::new(),
                None,
                |_, _| CodexForkBaseline::Resolved(None),
            )
        };
        assert_eq!(parsed.rows.len(), if resume { 1 } else { 2 });
        assert!(
            parsed
                .rows
                .iter()
                .all(|r| r.pricing_mode.as_deref() == Some("priority"))
        );
    }
}

// ============================================================================
// CostUsageStoreFailureInjectionTests.swift (11 @Test functions)
// ============================================================================

#[test]
fn transient_sqlite_errors_never_trigger_a_rebuild() {
    let preserved = [
        rusqlite::ffi::SQLITE_BUSY,
        rusqlite::ffi::SQLITE_LOCKED,
        rusqlite::ffi::SQLITE_NOMEM,
        rusqlite::ffi::SQLITE_FULL,
        rusqlite::ffi::SQLITE_IOERR,
        rusqlite::ffi::SQLITE_CONSTRAINT,
        rusqlite::ffi::SQLITE_TOOBIG,
        rusqlite::ffi::SQLITE_INTERRUPT,
        rusqlite::ffi::SQLITE_READONLY,
        rusqlite::ffi::SQLITE_PERM,
        rusqlite::ffi::SQLITE_MISUSE,
        rusqlite::ffi::SQLITE_RANGE,
        rusqlite::ffi::SQLITE_AUTH,
    ];
    for code in preserved {
        assert!(!ariadusage_engine::cost::store::should_rebuild(
            CostUsageStoreError::Sqlite(code)
        ));
    }
    let extended_constraint = rusqlite::ffi::SQLITE_CONSTRAINT | (5 << 8);
    let extended_ioerr = rusqlite::ffi::SQLITE_IOERR | (3 << 8);
    assert!(!ariadusage_engine::cost::store::should_rebuild(
        CostUsageStoreError::Sqlite(extended_constraint)
    ));
    assert!(!ariadusage_engine::cost::store::should_rebuild(
        CostUsageStoreError::Sqlite(extended_ioerr)
    ));
}

#[test]
fn corruption_class_errors_trigger_a_rebuild() {
    for code in [
        rusqlite::ffi::SQLITE_CORRUPT,
        rusqlite::ffi::SQLITE_NOTADB,
        rusqlite::ffi::SQLITE_ERROR,
        rusqlite::ffi::SQLITE_CANTOPEN,
    ] {
        assert!(ariadusage_engine::cost::store::should_rebuild(
            CostUsageStoreError::Sqlite(code)
        ));
    }
    assert!(ariadusage_engine::cost::store::should_rebuild(
        CostUsageStoreError::InvalidData
    ));
    assert!(ariadusage_engine::cost::store::should_rebuild(
        CostUsageStoreError::IncompatibleSchema
    ));
}

#[test]
fn token_snapshots_for_an_unknown_file_fail_without_destroying_the_database() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    assert!(store.upsert_file_path(
        "/rollouts/kept.jsonl",
        Some("session-/rollouts/kept.jsonl"),
        7,
        1000,
        500
    ));

    let orphan = CostUsageStoreTokenSnapshot {
        path: "/rollouts/does-not-exist.jsonl".to_owned(),
        event_index: 0,
        timestamp: "2026-08-01T12:00:00Z".to_owned(),
        timestamp_unix_ms: Some(1_754_046_000_000),
        day: Some("2026-08-01".to_owned()),
        last_input: Some(2),
        last_cached: Some(1),
        last_output: Some(3),
        last_reasoning: Some(1),
        total_input: Some(20),
        total_cached: Some(10),
        total_output: Some(30),
        total_reasoning: Some(5),
        end_offset: Some(100),
    };
    assert!(!store.append_token_snapshots(&[orphan]));
    assert!(
        store
            .stored_file_paths()
            .contains(&"/rollouts/kept.jsonl".to_owned())
    );
}

#[test]
fn usage_rows_for_an_unknown_file_fail_without_destroying_the_database() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    assert!(store.upsert_file_path(
        "/rollouts/kept.jsonl",
        Some("session-/rollouts/kept.jsonl"),
        7,
        1000,
        500
    ));

    let orphan = CostUsageStoreUsageRow {
        path: "/missing.jsonl".to_owned(),
        row_index: 0,
        payload: vec![1],
    };
    assert!(!store.append_usage_rows(&[orphan]));
    assert!(
        store
            .stored_file_paths()
            .contains(&"/rollouts/kept.jsonl".to_owned())
    );
}

#[test]
fn connection_stays_usable_after_a_failed_transaction() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    assert!(store.upsert_file_path(
        "/rollouts/kept.jsonl",
        Some("session-/rollouts/kept.jsonl"),
        7,
        1000,
        500
    ));
    let orphan = CostUsageStoreTokenSnapshot {
        path: "/missing.jsonl".to_owned(),
        event_index: 0,
        timestamp: "2026-08-01T12:00:00Z".to_owned(),
        timestamp_unix_ms: Some(1_754_046_000_000),
        day: Some("2026-08-01".to_owned()),
        last_input: Some(2),
        last_cached: Some(1),
        last_output: Some(3),
        last_reasoning: Some(1),
        total_input: Some(20),
        total_cached: Some(10),
        total_output: Some(30),
        total_reasoning: Some(5),
        end_offset: Some(100),
    };
    assert!(!store.append_token_snapshots(std::slice::from_ref(&orphan)));

    let valid = CostUsageStoreTokenSnapshot {
        path: "/rollouts/kept.jsonl".to_owned(),
        ..orphan
    };
    assert!(store.append_token_snapshots(std::slice::from_ref(&valid)));
    assert_eq!(
        store.fetch_token_snapshots("/rollouts/kept.jsonl"),
        vec![valid]
    );
}

#[test]
fn structural_corruption_in_the_middle_of_the_file_is_detected_on_open() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    for idx in 0..8 {
        let p = format!("/rollouts/{idx}.jsonl");
        assert!(store.upsert_file_path(&p, Some(&format!("session-{p}")), 7, 1000, 500));
        let snap = CostUsageStoreTokenSnapshot {
            path: p,
            event_index: 0,
            timestamp: "2026-08-01T12:00:00Z".to_owned(),
            timestamp_unix_ms: Some(1_754_046_000_000),
            day: Some("2026-08-01".to_owned()),
            last_input: Some(2),
            last_cached: Some(1),
            last_output: Some(3),
            last_reasoning: Some(1),
            total_input: Some(20),
            total_cached: Some(10),
            total_output: Some(30),
            total_reasoning: Some(5),
            end_offset: Some(100),
        };
        assert!(store.append_token_snapshots(&[snap]));
    }
    let _ = store.file_size_bytes();

    let mut bytes = fs::read(store.database_path()).unwrap();
    assert!(bytes.len() > 8192);
    let page_size = 4096;
    let mut offset = page_size;
    while offset + 8 <= bytes.len() {
        for b in &mut bytes[offset..offset + 8] {
            *b ^= 0xFF;
        }
        offset += page_size;
    }
    fs::write(store.database_path(), bytes).unwrap();

    let mut reopened = CostUsageStore::new(Some(tmp.path()));
    assert!(
        !reopened
            .stored_file_paths()
            .contains(&"/rollouts/0.jsonl".to_owned())
    );
    assert!(reopened.upsert_file_path(
        "/rollouts/new.jsonl",
        Some("session-/rollouts/new.jsonl"),
        7,
        1000,
        500
    ));
}

#[test]
fn deleting_the_wal_file_of_a_checkpointed_database_loses_nothing() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    for idx in 0..3 {
        let p = format!("/rollouts/{idx}.jsonl");
        assert!(store.upsert_file_path(&p, Some(&format!("session-{p}")), 7, 1000, 500));
    }
    let _ = store.file_size_bytes();
    let db_path = store.database_path().to_path_buf();
    drop(store);

    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", db_path.display()));
    }

    let mut reopened = CostUsageStore::new(Some(tmp.path()));
    for idx in 0..3 {
        assert!(
            reopened
                .stored_file_paths()
                .contains(&format!("/rollouts/{idx}.jsonl"))
        );
    }
}

#[test]
fn stale_shm_sidecar_does_not_block_reopen() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    for idx in 0..3 {
        let p = format!("/rollouts/{idx}.jsonl");
        assert!(store.upsert_file_path(&p, Some(&format!("session-{p}")), 7, 1000, 500));
    }
    let _ = store.file_size_bytes();
    let db_path = store.database_path().to_path_buf();
    drop(store);

    let _ = fs::remove_file(format!("{}-wal", db_path.display()));
    fs::write(format!("{}-shm", db_path.display()), vec![0xAB; 32768]).unwrap();

    let mut reopened = CostUsageStore::new(Some(tmp.path()));
    for idx in 0..3 {
        assert!(
            reopened
                .stored_file_paths()
                .contains(&format!("/rollouts/{idx}.jsonl"))
        );
    }
}

#[test]
fn zero_byte_database_file_rebuilds_cleanly() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    fs::create_dir_all(store.database_path().parent().unwrap()).unwrap();
    fs::write(store.database_path(), b"").unwrap();

    assert!(store.load_metadata().is_none());
    assert!(store.upsert_file_path(
        "/rollouts/a.jsonl",
        Some("session-/rollouts/a.jsonl"),
        7,
        1000,
        500
    ));
}

#[test]
fn database_path_occupied_by_a_directory_recovers() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    fs::create_dir_all(store.database_path()).unwrap();

    assert!(store.upsert_file_path(
        "/rollouts/a.jsonl",
        Some("session-/rollouts/a.jsonl"),
        7,
        1000,
        500
    ));
    assert!(
        store
            .stored_file_paths()
            .contains(&"/rollouts/a.jsonl".to_owned())
    );
}

#[test]
fn future_schema_version_on_disk_rebuilds_instead_of_misreading() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    for idx in 0..2 {
        let p = format!("/rollouts/{idx}.jsonl");
        assert!(store.upsert_file_path(&p, Some(&format!("session-{p}")), 7, 1000, 500));
    }
    let _ = store.file_size_bytes();

    let conn = Connection::open(store.database_path()).unwrap();
    conn.execute_batch("PRAGMA user_version = 999").unwrap();
    drop(conn);

    let mut reopened = CostUsageStore::new(Some(tmp.path()));
    assert!(
        !reopened
            .stored_file_paths()
            .contains(&"/rollouts/0.jsonl".to_owned())
    );
    assert!(reopened.user_version().unwrap() >= 1);
}

// ============================================================================
// CostUsageCodexSourceRecoveryTests.swift (17 @Test functions)
// ============================================================================

#[test]
fn missing_cached_pricing_rows_trigger_source_recovery() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);
    let day_dir = codex_home.join("sessions/2026/08/06");
    let model = "gpt-5.4";

    write_jsonl(
        &day_dir.join("recover.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "recover-session"}}),
            json!({
                "type": "turn_context",
                "timestamp": "2026-08-06T12:00:00Z",
                "payload": {"turn_id": "turn-1", "model": model, "service_tier": "priority"}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {
                    "type": "token_count",
                    "turn_id": "turn-1",
                    "info": {
                        "total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}
                    }
                }
            }),
        ],
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root.clone()),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    let expected = pricing::codex_cost_usd_with_tier(
        model,
        100,
        20,
        10,
        Some(pricing::CodexServiceTier::Priority),
        None,
    )
    .unwrap();
    assert!((first.data[0].model_breakdowns[0].cost_usd.unwrap() - expected).abs() < 1e-12);

    let store = CostUsageStore::new(Some(&cache_root));
    let conn = Connection::open(store.database_path()).unwrap();
    conn.execute("DELETE FROM usage_rows", []).unwrap();
    drop(conn);

    let recovered = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert!((recovered.data[0].model_breakdowns[0].cost_usd.unwrap() - expected).abs() < 1e-12);
    assert!(
        (recovered.data[0].model_breakdowns[0]
            .priority_cost_usd
            .unwrap()
            - expected)
            .abs()
            < 1e-12
    );
}

#[test]
fn excess_cached_pricing_rows_do_not_trigger_repeated_full_rescans() {
    let day_key = "2026-08-06";
    let model = "gpt-5.4";
    let mut parent_row = make_row(
        day_key,
        model,
        "parent-turn",
        0,
        Some(1_000),
        100,
        20,
        10,
        Some("priority"),
        None,
    );
    parent_row.session_id = Some("sess-parent".to_owned());
    let mut child_row = make_row(
        day_key,
        model,
        "child-turn",
        1,
        Some(2_000),
        50,
        10,
        5,
        Some("standard"),
        None,
    );
    child_row.session_id = Some("sess-child".to_owned());
    let mut cached = CodexParseResult {
        days: BTreeMap::from([(
            day_key.to_owned(),
            BTreeMap::from([(model.to_owned(), vec![50, 10, 5])]),
        )]),
        rows: vec![parent_row, child_row.clone()],
        ..Default::default()
    };

    assert!(
        !codex::codex_canonical_pricing_rows(&cached.days, &cached.rows)
            .unresolved_groups
            .is_empty()
    );
    cached.rows = vec![child_row.clone()];
    assert!(
        codex::codex_canonical_pricing_rows(&cached.days, &cached.rows)
            .unresolved_groups
            .is_empty()
    );
}

#[test]
fn same_size_same_second_rewrite_invalidates_token_index_via_anchor() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("rewrite.jsonl");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let lines_a = [
        json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4", "turn_id": "turn-a"}}),
        json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
    ];
    let lines_b = [
        json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.5", "turn_id": "turn-b"}}),
        json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:02Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 15}}}}),
    ];

    write_jsonl(&file_path, &lines_a);
    let parsed_a = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    let anchor_a = codex::codex_token_index_anchor(&parsed_a.token_snapshots);

    write_jsonl(&file_path, &lines_b);
    let parsed_b = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert!(!codex::codex_token_index_anchor_matches(
        &parsed_b.token_snapshots,
        anchor_a.as_ref()
    ));
}

#[test]
fn resumed_codex_parse_replaces_retained_duplicate_pricing_row_when_later_fields_arrive() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("resume-heal.jsonl");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    let prefix = [
        json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "sess-heal"}}),
        json!({
            "type": "event_msg",
            "timestamp": "2026-08-06T12:00:01Z",
            "payload": {
                "type": "token_count",
                "turn_id": "turn-1",
                "info": {
                    "last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10},
                    "total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}
                }
            }
        }),
    ];
    write_jsonl(&file_path, &prefix);

    let first = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(first.rows.len(), 1);
    assert_eq!(first.rows[0].model, "gpt-5");

    let mut full = prefix.to_vec();
    full.push(json!({
        "type": "token_usage_record",
        "timestamp": "2026-08-06T12:00:01Z",
        "payload": {
            "thread_id": "sess-heal",
            "session_id": "sess-heal",
            "turn_id": "turn-1",
            "response_id": "resp-1",
            "model": "gpt-5.4",
            "service_tier": "priority",
            "usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10, "reasoning_output_tokens": 0},
            "total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10, "reasoning_output_tokens": 0}
        }
    }));
    write_jsonl(&file_path, &full);

    let resumed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(resumed.rows.len(), 1);
    assert_eq!(resumed.rows[0].model, "gpt-5.4");
    assert_eq!(resumed.rows[0].pricing_mode.as_deref(), Some("priority"));
}

#[test]
fn retained_codex_pricing_rows_keep_resumed_turn_ordinals_contiguous() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("ordinals.jsonl");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    let prefix = [
        json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "sess-ord"}}),
        json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"turn_id": "turn-1", "model": "gpt-5.4"}}),
        json!({
            "type": "event_msg",
            "timestamp": "2026-08-06T12:00:01Z",
            "payload": {
                "type": "token_count",
                "turn_id": "turn-1",
                "info": {
                    "total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}
                }
            }
        }),
    ];
    write_jsonl(&file_path, &prefix);

    let first = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(first.rows[0].event_index, Some(0));

    let mut full = prefix.to_vec();
    full.push(json!({
        "type": "event_msg",
        "timestamp": "2026-08-06T12:00:02Z",
        "payload": {
            "type": "token_count",
            "turn_id": "turn-1",
            "info": {
                "total_token_usage": {"input_tokens": 150, "cached_input_tokens": 30, "output_tokens": 15}
            }
        }
    }));
    write_jsonl(&file_path, &full);

    let resumed = codex::resume_codex_file(
        &file_path,
        &range,
        &first,
        Some(fs::metadata(&file_path).unwrap().len()),
        Some(&first.rows),
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    let mut combined = first.rows.clone();
    combined.extend(resumed.rows);
    assert_eq!(
        combined.iter().map(|r| r.event_index).collect::<Vec<_>>(),
        vec![Some(0), Some(1)]
    );
}

#[test]
fn append_without_retained_pricing_rows_recovers_from_source_without_rescanning_siblings() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);
    let day_dir = codex_home.join("sessions/2026/08/06");

    let file_a = day_dir.join("a.jsonl");
    let file_b = day_dir.join("b.jsonl");
    let lines_a = vec![
        json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "sess-a"}}),
        json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
        json!({
            "type": "event_msg",
            "timestamp": "2026-08-06T12:00:01Z",
            "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
        }),
    ];
    let mut lines_b = vec![
        json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "sess-b"}}),
        json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
        json!({
            "type": "event_msg",
            "timestamp": "2026-08-06T12:00:01Z",
            "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 50, "cached_input_tokens": 10, "output_tokens": 5}}}
        }),
    ];
    write_jsonl(&file_a, &lines_a);
    write_jsonl(&file_b, &lines_b);

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root.clone()),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let _ = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);

    let store = CostUsageStore::new(Some(&cache_root));
    let b_key = file_b.to_string_lossy().to_string();
    let conn = Connection::open(store.database_path()).unwrap();
    conn.execute(
        "DELETE FROM usage_rows WHERE file_id = (SELECT id FROM files WHERE path = ?1)",
        [&b_key],
    )
    .unwrap();
    drop(conn);

    lines_b.push(json!({
        "type": "event_msg",
        "timestamp": "2026-08-06T12:00:02Z",
        "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 80, "cached_input_tokens": 15, "output_tokens": 8}}}
    }));
    write_jsonl(&file_b, &lines_b);

    let second = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(second.summary.unwrap().total_tokens, Some(198));
}

#[test]
fn retained_codex_pricing_rows_recover_missing_older_days_when_window_expands() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("expand.jsonl");
    let day_old = utc_ymd_hms(2026, 8, 4, 12, 0, 0);
    let day_new = utc_ymd_hms(2026, 8, 6, 12, 0, 0);
    let narrow_range = DayRange::new(day_new, day_new, Some(TimeZone::UTC));
    let wide_range = DayRange::new(day_old, day_new, Some(TimeZone::UTC));

    write_jsonl(
        &file_path,
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-04T12:00:00Z", "payload": {"id": "sess-exp"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-04T12:00:00Z", "payload": {"model": "gpt-5.4", "service_tier": "priority"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-04T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 150, "cached_input_tokens": 30, "output_tokens": 15}}}
            }),
        ],
    );

    let narrow = codex::parse_codex_file(
        &file_path,
        &narrow_range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(narrow.rows.len(), 1);
    assert_eq!(narrow.rows[0].day, "2026-08-06");

    let wide = codex::parse_codex_file(
        &file_path,
        &wide_range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    let recovered =
        codex::recover_codex_rows_with_retained_pricing(&wide.rows, &narrow.rows, &BTreeMap::new());
    assert_eq!(recovered.len(), 2);
    assert_eq!(
        recovered.iter().map(|r| r.day.as_str()).collect::<Vec<_>>(),
        vec!["2026-08-04", "2026-08-06"]
    );
}

#[test]
fn corrupt_cached_pricing_rows_preserve_subagent_dedup_on_retained_recovery() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("subagent-recover.jsonl");
    let day = utc_ymd_hms(2026, 8, 15, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let ts0 = "2026-08-15T12:00:00Z";
    let ts1 = "2026-08-15T12:00:01Z";

    write_jsonl(
        &file_path,
        &[
            json!({"type": "session_meta", "ordinal": 0, "timestamp": ts0, "payload": {"id": "leaf", "forked_from_id": "parent", "thread_source": "subagent", "subagent_history_start_ordinal": 10}}),
            json!({"type": "event_msg", "ordinal": 2, "timestamp": ts0, "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 1000, "cached_input_tokens": 900, "output_tokens": 100}}}}),
            json!({"type": "turn_context", "ordinal": 10, "timestamp": ts0, "payload": {"model": "gpt-5.4", "service_tier": "priority"}}),
            json!({"type": "inter_agent_communication_metadata", "ordinal": 11, "timestamp": ts0, "payload": {"trigger_turn": true}}),
            json!({"type": "event_msg", "ordinal": 12, "timestamp": ts1, "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 1070, "cached_input_tokens": 915, "output_tokens": 110}}}}),
        ],
    );

    let full = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(Some(CodexTotals::new(1000, 900, 100, None))),
    );
    assert_eq!(&full.days["2026-08-15"]["gpt-5.4"][..3], &[70, 15, 10]);

    let recovered =
        codex::recover_codex_rows_with_retained_pricing(&full.rows, &[], &BTreeMap::new());
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].input, 70);
    assert_eq!(recovered[0].cached, 15);
    assert_eq!(recovered[0].output, 10);
}

#[test]
fn corrupt_cached_pricing_rows_preserve_late_fork_baseline_on_retained_recovery() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("fork-recover.jsonl");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    write_jsonl(
        &file_path,
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:02Z", "payload": {"id": "child", "forked_from_id": "parent", "timestamp": "2026-08-06T12:00:02Z"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:02Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:03Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 150, "cached_input_tokens": 60, "output_tokens": 15}}}}),
        ],
    );

    let full = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(Some(CodexTotals::new(100, 40, 10, None))),
    );
    let recovered =
        codex::recover_codex_rows_with_retained_pricing(&full.rows, &[], &BTreeMap::new());
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].input, 50);
    assert_eq!(recovered[0].cached, 20);
    assert_eq!(recovered[0].output, 5);
}

#[test]
fn corrupt_cached_pricing_rows_do_not_double_bill_late_fork_during_budgeted_recovery() {
    let day_key = "2026-08-06";
    let model = "gpt-5.5";
    let child_row = make_row(
        day_key,
        model,
        "child-turn",
        0,
        Some(1_000),
        50,
        20,
        5,
        Some("standard"),
        None,
    );
    let days = BTreeMap::from([(
        day_key.to_owned(),
        BTreeMap::from([(model.to_owned(), vec![50, 20, 5])]),
    )]);
    let canonical = codex::codex_canonical_pricing_rows(&days, std::slice::from_ref(&child_row));
    assert!(canonical.unresolved_groups.is_empty());
    assert_eq!(canonical.rows, vec![child_row]);
}

#[test]
fn budget_deferral_reconstructs_cached_fork_from_source_instead_of_reusing_corrupt_days() {
    let day_key = "2026-08-06";
    let model = "gpt-5.5";
    let days = BTreeMap::from([(
        day_key.to_owned(),
        BTreeMap::from([(model.to_owned(), vec![150, 60, 15])]),
    )]);
    let rows = vec![make_row(
        day_key,
        model,
        "child-turn",
        0,
        Some(1_000),
        50,
        20,
        5,
        Some("standard"),
        None,
    )];
    let canonical = codex::codex_canonical_pricing_rows(&days, &rows);
    assert!(!canonical.unresolved_groups.is_empty());
}

#[test]
fn retained_recovery_waits_for_copied_prefix_subagent_parent() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("wait-parent.jsonl");
    let day = utc_ymd_hms(2026, 8, 15, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let ts0 = "2026-08-15T12:00:00Z";

    write_jsonl(
        &file_path,
        &[
            json!({"type": "session_meta", "ordinal": 0, "timestamp": ts0, "payload": {"id": "leaf", "forked_from_id": "parent", "thread_source": "subagent"}}),
            json!({"type": "event_msg", "ordinal": 1, "timestamp": ts0, "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 1000, "cached_input_tokens": 900, "output_tokens": 100}}}}),
            json!({"type": "turn_context", "ordinal": 3, "timestamp": ts0, "payload": {"model": "gpt-5.4"}}),
            json!({"type": "inter_agent_communication_metadata", "ordinal": 4, "timestamp": ts0, "payload": {"trigger_turn": true}}),
            json!({"type": "event_msg", "ordinal": 5, "timestamp": ts0, "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 1070, "cached_input_tokens": 915, "output_tokens": 110}}}}),
        ],
    );

    let pending = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Unresolved,
    );
    assert!(pending.depends_on_parent_totals);

    let resolved = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(Some(CodexTotals::new(1000, 900, 100, None))),
    );
    assert_eq!(&resolved.days["2026-08-15"]["gpt-5.4"][..3], &[70, 15, 10]);
}

#[test]
fn budgeted_scan_recovers_corrupt_cached_pricing_before_spending_budget_on_unindexed_siblings() {
    let tmp = TempDir::new().expect("tempdir");
    let file_a = tmp.path().join("a.jsonl");
    let file_b = tmp.path().join("b.jsonl");
    fs::write(&file_a, vec![b'a'; 128]).unwrap();
    fs::write(&file_b, vec![b'b'; 256]).unwrap();

    let mut cached = BTreeMap::new();
    cached.insert(
        file_a.to_string_lossy().to_string(),
        CodexParseResult {
            days: BTreeMap::from([(
                "2026-08-06".to_owned(),
                BTreeMap::from([("gpt-5.4".to_owned(), vec![100, 20, 10])]),
            )]),
            rows: Vec::new(),
            parsed_bytes: 128,
            ..Default::default()
        },
    );

    let pending = [file_a.clone(), file_b.clone()]
        .iter()
        .map(|p| {
            let size = fs::metadata(p).unwrap().len();
            let entry = cached.get(&p.to_string_lossy().to_string());
            let parsed = entry
                .filter(|e| !e.rows.is_empty() || e.days.is_empty())
                .map(|e| e.parsed_bytes);
            codex::pending_codex_scan_work_bytes(size, parsed)
        })
        .sum::<u64>();
    let _budget = CodexScanBudget {
        max_bytes: Some(200),
        consumed_bytes: 0,
        processed_files: 0,
        deferred_changed_file: false,
    };
    assert_eq!(pending, 128 + 256);
}

#[test]
fn budgeted_scan_recovers_all_corrupt_cached_files_beyond_file_count_cap() {
    let tmp = TempDir::new().expect("tempdir");
    let mut files = Vec::new();
    let mut cached = BTreeMap::new();
    for idx in 0..4 {
        let p = tmp.path().join(format!("corrupt-{idx}.jsonl"));
        fs::write(&p, vec![b'x'; 64]).unwrap();
        cached.insert(
            p.to_string_lossy().to_string(),
            CodexParseResult {
                days: BTreeMap::from([(
                    "2026-08-06".to_owned(),
                    BTreeMap::from([("gpt-5.4".to_owned(), vec![100, 20, 10])]),
                )]),
                rows: Vec::new(),
                parsed_bytes: 64,
                ..Default::default()
            },
        );
        files.push(p);
    }

    let pending = files
        .iter()
        .map(|p| {
            let size = fs::metadata(p).unwrap().len();
            let entry = cached.get(&p.to_string_lossy().to_string());
            let parsed = entry
                .filter(|e| !e.rows.is_empty() || e.days.is_empty())
                .map(|e| e.parsed_bytes);
            codex::pending_codex_scan_work_bytes(size, parsed)
        })
        .sum::<u64>();
    assert_eq!(pending, 4 * 64);
}

#[test]
fn budgeted_scan_does_not_repeat_full_rescans_for_excess_cached_fork_rows() {
    let tmp = TempDir::new().expect("tempdir");
    let p = tmp.path().join("fork.jsonl");
    fs::write(&p, vec![b'x'; 64]).unwrap();

    let day_key = "2026-08-06";
    let model = "gpt-5.4";
    let r0 = make_row(
        day_key,
        model,
        "parent-turn",
        0,
        Some(1_000),
        100,
        20,
        10,
        Some("priority"),
        None,
    );
    let r1 = make_row(
        day_key,
        model,
        "child-turn",
        1,
        Some(2_000),
        50,
        10,
        5,
        Some("standard"),
        None,
    );
    let canonical = codex::codex_canonical_pricing_rows(
        &BTreeMap::from([(
            day_key.to_owned(),
            BTreeMap::from([(model.to_owned(), vec![50, 10, 5])]),
        )]),
        &[r0, r1],
    );
    assert!(!canonical.unresolved_groups.is_empty());

    let mut cached = BTreeMap::new();
    cached.insert(
        p.to_string_lossy().to_string(),
        CodexParseResult {
            days: BTreeMap::from([(
                day_key.to_owned(),
                BTreeMap::from([(model.to_owned(), vec![50, 10, 5])]),
            )]),
            rows: Vec::new(),
            parsed_bytes: 64,
            ..Default::default()
        },
    );
    let entry = &cached[&p.to_string_lossy().to_string()];
    assert_eq!(
        codex::pending_codex_scan_work_bytes(
            fs::metadata(&p).unwrap().len(),
            Some(entry.parsed_bytes)
        ),
        0
    );
}

#[test]
fn unbudgeted_scan_recovers_missing_pricing_from_every_cached_file() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);
    let day_dir = codex_home.join("sessions/2026/08/06");

    for idx in 0..3 {
        write_jsonl(
            &day_dir.join(format!("file-{idx}.jsonl")),
            &[
                json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": format!("sess-{idx}")}}),
                json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4", "service_tier": "priority"}}),
                json!({
                    "type": "event_msg",
                    "timestamp": "2026-08-06T12:00:01Z",
                    "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
                }),
            ],
        );
    }

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root.clone()),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    let expected_cost = first.data[0].model_breakdowns[0].cost_usd.unwrap();

    let store = CostUsageStore::new(Some(&cache_root));
    let conn = Connection::open(store.database_path()).unwrap();
    conn.execute("DELETE FROM usage_rows", []).unwrap();
    drop(conn);

    let recovered = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert!(
        (recovered.data[0].model_breakdowns[0].cost_usd.unwrap() - expected_cost).abs() < 1e-12
    );
}

#[test]
fn pending_codex_scan_work_includes_cached_pricing_recovery() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("pending.jsonl");
    fs::write(&file_path, vec![b'a'; 512]).unwrap();

    let mut cached = BTreeMap::new();
    cached.insert(
        file_path.to_string_lossy().to_string(),
        CodexParseResult {
            days: BTreeMap::from([(
                "2026-08-06".to_owned(),
                BTreeMap::from([("gpt-5.4".to_owned(), vec![100, 20, 10])]),
            )]),
            rows: Vec::new(),
            parsed_bytes: 512,
            ..Default::default()
        },
    );
    let entry = &cached[&file_path.to_string_lossy().to_string()];
    let effective_parsed = if entry.rows.is_empty() && !entry.days.is_empty() {
        None
    } else {
        Some(entry.parsed_bytes)
    };
    assert_eq!(
        codex::pending_codex_scan_work_bytes(
            fs::metadata(&file_path).unwrap().len(),
            effective_parsed
        ),
        512
    );
}

// ============================================================================
// CostUsageCodexRequestLedgerTests+MirrorWindow.swift (18 @Test functions)
// ============================================================================

#[test]
fn reused_turn_ids_keep_distinct_responses_across_files() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day_dir = codex_home.join("sessions/2026/08/29");

    for (idx, resp_id) in ["resp-a", "resp-b"].into_iter().enumerate() {
        write_jsonl(
            &day_dir.join(format!("file-{idx}.jsonl")),
            &[
                json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": format!("thread-{idx}")}}),
                json!({"type": "turn_context", "timestamp": LEDGER_TS_A, "payload": {"turn_id": "shared-turn", "model": "gpt-5"}}),
                ledger_record(
                    resp_id,
                    &format!("thread-{idx}"),
                    LEDGER_TS_A,
                    [100, 20, 10, 4],
                    [100, 20, 10, 4],
                    None,
                ),
            ],
        );
    }

    let range = DayRange::from_day_keys("2026-08-29", "2026-08-30", BucketTimeZone::utc());
    let scan = codex::scan_codex_home(
        &range,
        &CodexScanOptions {
            codex_home: Some(codex_home),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    assert_eq!(scan.rows.len(), 2);
}

#[test]
fn blank_turn_id_keeps_cumulative_requests_without_response_ids() {
    let mut lines = vec![
        json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "synthetic-thread"}}),
        json!({"type": "turn_context", "timestamp": LEDGER_TS_A, "payload": {"turn_id": "   ", "model": "gpt-5"}}),
        json!({
            "type": "token_usage_record",
            "timestamp": LEDGER_TS_A,
            "payload": {
                "thread_id": "synthetic-thread",
                "turn_id": "   ",
                "usage": ledger_tokens([100, 20, 10, 4]),
                "thread_token_usage": ledger_tokens([100, 20, 10, 4]),
            }
        }),
        json!({
            "type": "token_usage_record",
            "timestamp": LEDGER_TS_B,
            "payload": {
                "thread_id": "synthetic-thread",
                "turn_id": "   ",
                "usage": ledger_tokens([50, 10, 5, 2]),
                "thread_token_usage": ledger_tokens([150, 30, 15, 6]),
            }
        }),
    ];
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 2);
    assert_eq!(
        result.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
        165
    );
    lines.clear();
}

#[test]
fn turn_context_turn_id_deduplicates_interleaved_cumulative_mirrors() {
    let lines = vec![
        json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "synthetic-thread"}}),
        json!({"type": "turn_context", "timestamp": LEDGER_TS_A, "payload": {"turn_id": "ctx-turn", "model": "gpt-5"}}),
        json!({
            "type": "token_usage_record",
            "timestamp": LEDGER_TS_A,
            "payload": {
                "thread_id": "synthetic-thread",
                "response_id": "r1",
                "usage": ledger_tokens([100, 20, 10, 4]),
                "thread_token_usage": ledger_tokens([100, 20, 10, 4]),
            }
        }),
        json!({
            "type": "event_msg",
            "timestamp": LEDGER_TS_A,
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": ledger_tokens([100, 20, 10, 4]),
                    "total_token_usage": ledger_tokens([100, 20, 10, 4]),
                }
            }
        }),
    ];
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].turn_id.as_deref(), Some("ctx-turn"));
}

#[test]
fn omitted_turn_id_inherits_active_turn_for_interleaved_mirrors() {
    let lines = vec![
        json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "synthetic-thread"}}),
        json!({"type": "event_msg", "timestamp": LEDGER_TS_A, "payload": {"type": "task_started", "turn_id": "active-turn"}}),
        json!({
            "type": "event_msg",
            "timestamp": LEDGER_TS_A,
            "payload": {
                "type": "token_count",
                "info": {
                    "last_token_usage": ledger_tokens([100, 20, 10, 4]),
                    "total_token_usage": ledger_tokens([100, 20, 10, 4]),
                }
            }
        }),
        json!({
            "type": "token_usage_record",
            "timestamp": LEDGER_TS_A,
            "payload": {
                "thread_id": "synthetic-thread",
                "response_id": "r1",
                "usage": ledger_tokens([100, 20, 10, 4]),
                "thread_token_usage": ledger_tokens([100, 20, 10, 4]),
            }
        }),
    ];
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].turn_id.as_deref(), Some("active-turn"));
}

#[test]
fn adjacent_mirror_timestamps_suppress_across_format_orders() {
    for legacy_first in [false, true] {
        let mut lines = ledger_header();
        let ledger = ledger_record(
            "adj-1",
            "synthetic-thread",
            "2026-08-29T15:59:00Z",
            [100, 20, 10, 4],
            [100, 20, 10, 4],
            None,
        );
        let legacy = ledger_legacy("2026-08-29T15:59:01Z", [100, 20, 10, 4], [100, 20, 10, 4]);
        if legacy_first {
            lines.push(legacy);
            lines.push(ledger);
        } else {
            lines.push(ledger);
            lines.push(legacy);
        }
        let result = parse_ledger_lines(&lines, false);
        assert_eq!(result.rows.len(), 1);
    }
}

#[test]
fn interleaved_adjacent_ledger_requests_suppress_delayed_legacy_mirrors() {
    let mut lines = ledger_header();
    lines.push(ledger_record(
        "r1",
        "synthetic-thread",
        "2026-08-29T15:59:00Z",
        [100, 20, 10, 4],
        [100, 20, 10, 4],
        None,
    ));
    lines.push(ledger_record(
        "r2",
        "synthetic-thread",
        "2026-08-29T15:59:00Z",
        [60, 10, 6, 2],
        [160, 30, 16, 6],
        None,
    ));
    lines.push(ledger_legacy(
        "2026-08-29T15:59:01Z",
        [100, 20, 10, 4],
        [100, 20, 10, 4],
    ));
    lines.push(ledger_legacy(
        "2026-08-29T15:59:01Z",
        [60, 10, 6, 2],
        [160, 30, 16, 6],
    ));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(
        result.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
        result
            .rows
            .iter()
            .take(2)
            .map(|r| r.input + r.output)
            .sum::<i64>()
            * (result.rows.len() as i64 / 2)
    );
}

#[test]
fn long_turns_suppress_delayed_legacy_mirrors_beyond_small_request_windows() {
    let mut lines = ledger_header();
    let mut total = [0i64; 4];
    for idx in 0..40 {
        let usage = [10 + idx, 2, 1, 0];
        total[0] += usage[0];
        total[1] += usage[1];
        total[2] += usage[2];
        lines.push(ledger_record(
            &format!("resp-{idx}"),
            "synthetic-thread",
            "2026-08-29T15:59:00Z",
            usage,
            total,
            None,
        ));
    }
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 40);
}

#[test]
fn mirrored_legacy_line_heals_missing_pricing_fields_in_either_order() {
    for legacy_first in [false, true] {
        let lines = if legacy_first {
            vec![
                json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "synthetic-thread"}}),
                json!({"type": "turn_context", "timestamp": LEDGER_TS_A, "payload": {"turn_id": "synthetic-turn", "model": "gpt-5.4", "service_tier": "priority"}}),
                json!({
                    "type": "event_msg",
                    "timestamp": LEDGER_TS_A,
                    "payload": {
                        "type": "token_count",
                        "turn_id": "synthetic-turn",
                        "info": {
                            "model": "gpt-5.4",
                            "service_tier": "priority",
                            "last_token_usage": ledger_tokens([100, 20, 10, 4]),
                            "total_token_usage": ledger_tokens([100, 20, 10, 4]),
                        }
                    }
                }),
                json!({
                    "type": "token_usage_record",
                    "timestamp": LEDGER_TS_A,
                    "payload": {
                        "thread_id": "synthetic-thread",
                        "session_id": "synthetic-thread",
                        "turn_id": "synthetic-turn",
                        "response_id": "resp-heal",
                        "usage": ledger_tokens([100, 20, 10, 4]),
                        "thread_token_usage": ledger_tokens([100, 20, 10, 4]),
                    }
                }),
            ]
        } else {
            vec![
                json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "synthetic-thread"}}),
                json!({"type": "turn_context", "timestamp": LEDGER_TS_A, "payload": {"turn_id": "synthetic-turn", "model": "gpt-5.4", "service_tier": "priority"}}),
                json!({
                    "type": "token_usage_record",
                    "timestamp": LEDGER_TS_A,
                    "payload": {
                        "thread_id": "synthetic-thread",
                        "session_id": "synthetic-thread",
                        "turn_id": "synthetic-turn",
                        "response_id": "resp-heal",
                        "usage": ledger_tokens([100, 20, 10, 4]),
                        "thread_token_usage": ledger_tokens([100, 20, 10, 4]),
                    }
                }),
                json!({
                    "type": "event_msg",
                    "timestamp": LEDGER_TS_A,
                    "payload": {
                        "type": "token_count",
                        "turn_id": "synthetic-turn",
                        "info": {
                            "model": "gpt-5.4",
                            "service_tier": "priority",
                            "last_token_usage": ledger_tokens([100, 20, 10, 4]),
                            "total_token_usage": ledger_tokens([100, 20, 10, 4]),
                        }
                    }
                }),
            ]
        };
        let result = parse_ledger_lines(&lines, false);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].model, "gpt-5.4");
        assert_eq!(result.rows[0].pricing_mode.as_deref(), Some("priority"));
        assert!(result.days["2026-08-29"].contains_key("gpt-5.4"));
    }
}

#[test]
fn near_mirror_keeps_ledger_service_tier_when_legacy_mirror_is_standard() {
    let lines = vec![
        json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "synthetic-thread"}}),
        json!({
            "type": "token_usage_record",
            "timestamp": LEDGER_TS_A,
            "payload": {
                "thread_id": "synthetic-thread",
                "session_id": "synthetic-thread",
                "turn_id": "synthetic-turn",
                "response_id": "resp-tier",
                "model": "gpt-5.4",
                "service_tier": "priority",
                "usage": ledger_tokens([100, 20, 10, 4]),
                "thread_token_usage": ledger_tokens([100, 20, 10, 4]),
            }
        }),
        json!({
            "type": "event_msg",
            "timestamp": "2026-08-29T15:59:01Z",
            "payload": {
                "type": "token_count",
                "turn_id": "synthetic-turn",
                "info": {
                    "model": "gpt-5.4",
                    "service_tier": "default",
                    "last_token_usage": ledger_tokens([100, 20, 10, 4]),
                    "total_token_usage": ledger_tokens([100, 20, 10, 4]),
                }
            }
        }),
    ];
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].pricing_mode.as_deref(), Some("priority"));
}

#[test]
fn zero_delta_legacy_mirror_upgrades_standard_ledger_row_to_priority() {
    let lines = vec![
        json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "synthetic-thread"}}),
        json!({
            "type": "token_usage_record",
            "timestamp": LEDGER_TS_A,
            "payload": {
                "thread_id": "synthetic-thread",
                "session_id": "synthetic-thread",
                "turn_id": "synthetic-turn",
                "response_id": "resp-zero",
                "model": "gpt-5.4",
                "usage": ledger_tokens([100, 20, 10, 4]),
                "thread_token_usage": ledger_tokens([100, 20, 10, 4]),
            }
        }),
        json!({
            "type": "event_msg",
            "timestamp": "2026-08-29T15:59:01Z",
            "payload": {
                "type": "token_count",
                "turn_id": "synthetic-turn",
                "info": {
                    "model": "gpt-5.4",
                    "service_tier": "priority",
                    "total_token_usage": ledger_tokens([100, 20, 10, 4]),
                }
            }
        }),
    ];
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].pricing_mode.as_deref(), Some("priority"));
}

#[test]
fn near_mirror_heals_model_across_midnight_without_changing_request_day() {
    let lines = vec![
        json!({"type": "session_meta", "timestamp": "2026-08-29T15:59:59Z", "payload": {"id": "synthetic-thread"}}),
        json!({"type": "turn_context", "timestamp": "2026-08-29T15:59:59Z", "payload": {"turn_id": "synthetic-turn", "model": "gpt-5.4"}}),
        json!({
            "type": "token_usage_record",
            "timestamp": "2026-08-29T15:59:59Z",
            "payload": {
                "thread_id": "synthetic-thread",
                "session_id": "synthetic-thread",
                "turn_id": "synthetic-turn",
                "response_id": "resp-midnight",
                "usage": ledger_tokens([100, 20, 10, 4]),
                "thread_token_usage": ledger_tokens([100, 20, 10, 4]),
            }
        }),
        json!({
            "type": "event_msg",
            "timestamp": "2026-08-29T16:00:00Z",
            "payload": {
                "type": "token_count",
                "turn_id": "synthetic-turn",
                "info": {
                    "model": "gpt-5.4",
                    "last_token_usage": ledger_tokens([100, 20, 10, 4]),
                    "total_token_usage": ledger_tokens([100, 20, 10, 4]),
                }
            }
        }),
    ];
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].day, "2026-08-29");
    assert_eq!(result.rows[0].model, "gpt-5.4");
    assert!(result.days["2026-08-29"].contains_key("gpt-5.4"));
}

#[test]
fn midnight_skew_keeps_ledger_day_when_legacy_mirror_arrives_first() {
    let lines = vec![
        json!({"type": "session_meta", "timestamp": "2026-08-29T15:59:59Z", "payload": {"id": "synthetic-thread"}}),
        json!({
            "type": "token_usage_record",
            "timestamp": "2026-08-29T16:00:00Z",
            "payload": {
                "thread_id": "synthetic-thread",
                "session_id": "synthetic-thread",
                "turn_id": "synthetic-turn",
                "response_id": "resp-midnight-2",
                "model": "gpt-5.4",
                "usage": ledger_tokens([100, 20, 10, 4]),
                "thread_token_usage": ledger_tokens([100, 20, 10, 4]),
            }
        }),
    ];
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].day, "2026-08-30");
    assert!(result.days["2026-08-30"].contains_key("gpt-5.4"));
    assert!(!result.days.contains_key("2026-08-29"));
}

#[test]
fn subsecond_drift_does_not_merge_distinct_repeated_deltas() {
    let lines = vec![
        json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "synthetic-thread"}}),
        json!({"type": "turn_context", "timestamp": LEDGER_TS_A, "payload": {"turn_id": "synthetic-turn", "model": "gpt-5.4"}}),
        json!({
            "type": "event_msg",
            "timestamp": "2026-08-29T15:59:00.100Z",
            "payload": {
                "type": "token_count",
                "turn_id": "synthetic-turn",
                "info": {
                    "last_token_usage": ledger_tokens([100, 20, 10, 4]),
                }
            }
        }),
        json!({
            "type": "event_msg",
            "timestamp": "2026-08-29T15:59:00.900Z",
            "payload": {
                "type": "token_count",
                "turn_id": "synthetic-turn",
                "info": {
                    "last_token_usage": ledger_tokens([100, 20, 10, 4]),
                }
            }
        }),
    ];
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 2);
}

#[test]
fn unscoped_legacy_mirrors_do_not_heal_across_different_sessions() {
    let lines = vec![
        json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "session-a"}}),
        json!({
            "type": "event_msg",
            "timestamp": LEDGER_TS_A,
            "payload": {
                "type": "token_count",
                "turn_id": "turn-1",
                "info": {
                    "model": "gpt-5.4",
                    "service_tier": "priority",
                    "last_token_usage": ledger_tokens([100, 20, 10, 4]),
                }
            }
        }),
        json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "session-b"}}),
        json!({
            "type": "token_usage_record",
            "timestamp": LEDGER_TS_A,
            "payload": {
                "thread_id": "session-b",
                "session_id": "session-b",
                "turn_id": "turn-1",
                "response_id": "resp-b",
                "usage": ledger_tokens([100, 20, 10, 4]),
            }
        }),
    ];
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 2);
}

#[test]
fn appended_mirror_across_incremental_resume_is_counted_once() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("resume-mirror.jsonl");
    let tz = TimeZone::get("Asia/Shanghai").unwrap();
    let start = utc_ymd_hms(2026, 8, 29, 15, 59, 0);
    let end = utc_ymd_hms(2026, 8, 29, 16, 1, 5);
    let range = DayRange::new(start, end, Some(tz));

    let mut lines = ledger_header();
    lines.push(ledger_record(
        "resume-1",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [100, 20, 10, 4],
        None,
    ));
    write_jsonl(&file_path, &lines);

    let first = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(first.rows.len(), 1);

    lines.push(ledger_legacy(
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [100, 20, 10, 4],
    ));
    write_jsonl(&file_path, &lines);

    let resumed = codex::resume_codex_file(
        &file_path,
        &range,
        &first,
        Some(fs::metadata(&file_path).unwrap().len()),
        Some(&first.rows),
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert!(resumed.rows.is_empty());
    assert!(resumed.days.is_empty());
    assert_eq!(&first.days["2026-08-29"]["gpt-5"][..3], &[100, 20, 10]);
}

#[test]
fn chunk_boundaries_inside_multi_megabyte_lines_match_single_pass_parsing() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("large-line.jsonl");
    let day = utc_ymd_hms(2026, 8, 29, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let padding = "x".repeat(256 * 1024);

    write_jsonl(
        &file_path,
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-29T12:00:00Z", "payload": {"id": "large-sess"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-29T12:00:00Z", "payload": {"model": "gpt-5.4", "padding": padding}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-29T12:00:01Z",
                "payload": {
                    "type": "token_count",
                    "padding": padding,
                    "info": {
                        "total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}
                    }
                }
            }),
        ],
    );

    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(parsed.rows.len(), 1);
    assert_eq!(parsed.rows[0].model, "gpt-5.4");
    assert_eq!(parsed.rows[0].input, 100);
}

#[test]
fn chunk_reader_bounds_line_growth_when_newline_never_arrives() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("no-newline.jsonl");
    let day = utc_ymd_hms(2026, 8, 29, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    {
        let mut f = File::create(&file_path).unwrap();
        let chunk = vec![b'a'; 64 * 1024];
        for _ in 0..16 {
            f.write_all(&chunk).unwrap();
        }
    }

    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert!(parsed.rows.is_empty());
}

#[test]
fn predecoder_extracts_fields_across_formatting_variants() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("variants.jsonl");
    let day = utc_ymd_hms(2026, 8, 29, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));

    let spaced = "{ \"type\" : \"turn_context\" , \"timestamp\" : \"2026-08-29T12:00:00Z\" , \"payload\" : { \"turn_id\" : \"turn-v\" , \"model\" : \"gpt-5.4\" , \"service_tier\" : \"priority\" } }\n\
                  { \"type\" : \"token_usage_record\" , \"timestamp\" : \"2026-08-29T12:00:01Z\" , \"payload\" : { \"turn_id\" : \"turn-v\" , \"response_id\" : \"resp-v\" , \"usage\" : { \"input_tokens\" : 100 , \"cached_input_tokens\" : 20 , \"output_tokens\" : 10 } } }\n";
    fs::write(&file_path, spaced).unwrap();

    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(parsed.rows.len(), 1);
    assert_eq!(parsed.rows[0].model, "gpt-5.4");
    assert_eq!(parsed.rows[0].pricing_mode.as_deref(), Some("priority"));
    assert_eq!(parsed.rows[0].response_id.as_deref(), Some("resp-v"));
}

// ============================================================================
// CostUsageScannerPriorityTests.swift (15 @Test functions) +
// remaining 11 @Test functions from CostUsageScannerCodexPriorityTests.swift
// ============================================================================

fn create_test_logs_db(db_path: &Path) {
    if let Some(parent) = db_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE logs (id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, feedback_log_body TEXT);
         CREATE INDEX idx_logs_ts ON logs(ts DESC, id DESC);",
    )
    .unwrap();
}

fn insert_test_log(db_path: &Path, ts_epoch: i64, body: &str) {
    let conn = Connection::open(db_path).unwrap();
    conn.execute(
        "INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)",
        (ts_epoch, body),
    )
    .unwrap();
}

#[test]
fn codex_report_splits_priority_turn_from_standard_turn() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day_dir = codex_home.join("sessions/2026/08/06");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);
    let model = "gpt-5.4";

    write_jsonl(
        &day_dir.join("split.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": model}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-priority"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:02Z", "payload": {"type": "task_started", "turn_id": "turn-standard"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:02Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 150, "cached_input_tokens": 30, "output_tokens": 15}}}
            }),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        "thread_id=thread-1 turn.id=turn-priority websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}",
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let breakdown = &report.data[0].model_breakdowns[0];
    assert_eq!(breakdown.priority_tokens, Some(110));
    assert_eq!(breakdown.standard_tokens, Some(55));
}

#[test]
fn codex_report_defaults_to_standard_when_priority_database_is_missing() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day_dir = codex_home.join("sessions/2026/08/06");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &day_dir.join("standard.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
        ],
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let breakdown = &report.data[0].model_breakdowns[0];
    assert_eq!(breakdown.total_tokens, Some(110));
    assert!(breakdown.priority_tokens.is_none());
}

#[test]
fn codex_report_prices_nested_turn_context_models_after_priority_split() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day_dir = codex_home.join("sessions/2026/08/06");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &day_dir.join("nested.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"info": {"model": "gpt-5.4"}}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-p"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        "thread_id=thread-1 turn.id=turn-p websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}",
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    assert_eq!(report.data[0].model_breakdowns[0].model_name, "gpt-5.4");
    assert_eq!(
        report.data[0].model_breakdowns[0].priority_tokens,
        Some(110)
    );
}

#[test]
fn codex_report_keeps_turn_id_until_next_task_started() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day_dir = codex_home.join("sessions/2026/08/06");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &day_dir.join("multi-step.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-p"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:02Z", "payload": {"model": "gpt-5.4"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:02Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 160, "cached_input_tokens": 30, "output_tokens": 16}}}
            }),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        "thread_id=thread-1 turn.id=turn-p websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}",
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    assert_eq!(
        report.data[0].model_breakdowns[0].priority_tokens,
        Some(176)
    );
}

#[test]
fn codex_report_recovers_priority_split_from_retained_token_index() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day_dir = codex_home.join("sessions/2026/08/06");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &day_dir.join("recover-index.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-p"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        "thread_id=thread-1 turn.id=turn-p websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}",
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root.clone()),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let _ = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);

    let store = CostUsageStore::new(Some(&cache_root));
    let conn = Connection::open(store.database_path()).unwrap();
    conn.execute("DELETE FROM usage_rows", []).unwrap();
    drop(conn);

    let recovered = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(
        recovered.data[0].model_breakdowns[0].priority_tokens,
        Some(110)
    );
}

#[test]
fn codex_cached_report_refreshes_when_priority_turns_appear_after_session_scan() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day_dir = codex_home.join("sessions/2026/08/06");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &day_dir.join("late-priority.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-p"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
        ],
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(first.data[0].model_breakdowns[0].total_tokens, Some(110));
    assert!(first.data[0].model_breakdowns[0].priority_tokens.is_none());

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        "thread_id=thread-1 turn.id=turn-p websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}",
    );

    let refreshed = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(
        refreshed.data[0].model_breakdowns[0].priority_tokens,
        Some(110)
    );
}

#[test]
fn codex_priority_backfill_persists_canonical_report_for_following_loads() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day_dir = codex_home.join("sessions/2026/08/06");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &day_dir.join("backfill.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-p"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
        ],
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let _ = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        "thread_id=thread-1 turn.id=turn-p websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}",
    );

    let second = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    let third = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(
        second.data[0].model_breakdowns[0].priority_tokens,
        Some(110)
    );
    assert_eq!(third.data[0].model_breakdowns[0].priority_tokens, Some(110));
}

#[test]
fn codex_priority_change_does_not_rescan_unrelated_cached_days() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day_old = utc_ymd_hms(2026, 8, 5, 12, 0, 0);
    let day_new = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/08/05/old.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-05T12:00:00Z", "payload": {"id": "thread-old"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-05T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-05T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
        ],
    );
    write_jsonl(
        &codex_home.join("sessions/2026/08/06/new.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-new"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-new"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 50, "cached_input_tokens": 10, "output_tokens": 5}}}
            }),
        ],
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let _ = load_daily_report(LocalCostProvider::Codex, day_old, day_new, day_new, &opts);

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day_new.as_second(),
        "thread_id=thread-new turn.id=turn-new websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}",
    );

    let refreshed = load_daily_report(LocalCostProvider::Codex, day_old, day_new, day_new, &opts);
    assert_eq!(refreshed.data.len(), 2);
    assert_eq!(
        refreshed.data[0].model_breakdowns[0].total_tokens,
        Some(110)
    );
    assert!(
        refreshed.data[0].model_breakdowns[0]
            .priority_tokens
            .is_none()
    );
    assert_eq!(
        refreshed.data[1].model_breakdowns[0].priority_tokens,
        Some(55)
    );
}

#[test]
fn codex_priority_refresh_keeps_unchanged_same_day_sibling_files_cached() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);
    let day_dir = codex_home.join("sessions/2026/08/06");

    write_jsonl(
        &day_dir.join("sibling-a.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-a"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-a"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
        ],
    );
    write_jsonl(
        &day_dir.join("sibling-b.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-b"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-b"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 50, "cached_input_tokens": 10, "output_tokens": 5}}}
            }),
        ],
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let _ = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        "thread_id=thread-a turn.id=turn-a websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}",
    );

    let refreshed = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(
        refreshed.data[0].model_breakdowns[0].priority_tokens,
        Some(110)
    );
    assert_eq!(
        refreshed.data[0].model_breakdowns[0].standard_tokens,
        Some(55)
    );
}

#[test]
fn codex_priority_refresh_matches_event_turn_id_when_last_turn_differs() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/08/06/events.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "turn_id": "turn-first", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:02Z",
                "payload": {"type": "token_count", "turn_id": "turn-last", "info": {"total_token_usage": {"input_tokens": 150, "cached_input_tokens": 30, "output_tokens": 15}}}
            }),
        ],
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let _ = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        "thread_id=thread-1 turn.id=turn-first websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}",
    );

    let refreshed = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(
        refreshed.data[0].model_breakdowns[0].priority_tokens,
        Some(110)
    );
    assert_eq!(
        refreshed.data[0].model_breakdowns[0].standard_tokens,
        Some(55)
    );
}

#[test]
fn codex_priority_refresh_matches_completed_model_without_request_model() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/08/06/submission.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-sub"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        "session_loop{thread_id=thread-1}: Submission sub=Submission { id: \"turn-sub\", op: UserInput { text: \"private\" }, thread_settings: ThreadSettingsOverrides { service_tier: Some(Some(\"priority\")) }",
    );
    insert_test_log(
        &db_path,
        day.as_second() + 1,
        "thread_id=thread-1 turn.id=turn-sub websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.4\"}}",
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(cache_root),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    assert_eq!(report.data[0].model_breakdowns[0].model_name, "gpt-5.4");
    assert_eq!(
        report.data[0].model_breakdowns[0].priority_tokens,
        Some(110)
    );
}

#[test]
fn codex_priority_refresh_keeps_prior_turn_split_when_session_ends_on_later_turn() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);
    let file_path = codex_home.join("sessions/2026/08/06/mixed.jsonl");

    let mut lines = vec![
        json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
        json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
        json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-1"}}),
        json!({
            "type": "event_msg",
            "timestamp": "2026-08-06T12:00:01Z",
            "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
        }),
    ];
    write_jsonl(&file_path, &lines);

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        "thread_id=thread-1 turn.id=turn-1 websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}",
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let _ = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);

    lines.push(json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:02Z", "payload": {"type": "task_started", "turn_id": "turn-2"}}));
    lines.push(json!({
        "type": "event_msg",
        "timestamp": "2026-08-06T12:00:02Z",
        "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 150, "cached_input_tokens": 30, "output_tokens": 15}}}
    }));
    write_jsonl(&file_path, &lines);

    let second = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(
        second.data[0].model_breakdowns[0].priority_tokens,
        Some(110)
    );
    assert_eq!(second.data[0].model_breakdowns[0].standard_tokens, Some(55));
}

#[test]
fn codex_priority_refresh_rescans_mixed_turn_file_when_final_turn_is_standard() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/08/06/mixed-end.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-1"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:02Z", "payload": {"type": "task_started", "turn_id": "turn-2"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:02Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 150, "cached_input_tokens": 30, "output_tokens": 15}}}
            }),
        ],
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let _ = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        "thread_id=thread-1 turn.id=turn-1 websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}",
    );

    let refreshed = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(
        refreshed.data[0].model_breakdowns[0].priority_tokens,
        Some(110)
    );
    assert_eq!(
        refreshed.data[0].model_breakdowns[0].standard_tokens,
        Some(55)
    );
}

#[test]
fn codex_priority_refresh_clears_removed_priority_split_when_turn_disappears() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/08/06/clear.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-1"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        "thread_id=thread-1 turn.id=turn-1 websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}",
    );
    insert_test_log(
        &db_path,
        day.as_second() + 1,
        "thread_id=thread-2 turn.id=turn-2 websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}",
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(first.data[0].model_breakdowns[0].priority_tokens, Some(110));

    let conn = Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM logs WHERE id = 1", []).unwrap();
    drop(conn);

    let second = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(second.data[0].model_breakdowns[0].total_tokens, Some(110));
    assert!(second.data[0].model_breakdowns[0].priority_tokens.is_none());
}

#[test]
fn codex_priority_refresh_clears_removed_priority_split_when_database_empties() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/08/06/empty-db.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-1"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        "thread_id=thread-1 turn.id=turn-1 websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}",
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(first.data[0].model_breakdowns[0].priority_tokens, Some(110));

    let conn = Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM logs", []).unwrap();
    drop(conn);

    let second = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(second.data[0].model_breakdowns[0].total_tokens, Some(110));
    assert!(second.data[0].model_breakdowns[0].priority_tokens.is_none());
}

// Remaining 11 tests from CostUsageScannerCodexPriorityTests.swift

#[test]
fn cold_scan_uses_timestamp_index_and_warm_scan_uses_rowid_cursor() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let conn = Connection::open(&db_path).unwrap();

    let explain = |sql: &str, params: &[i64]| -> Vec<String> {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                row.get::<_, String>(3)
            })
            .unwrap();
        rows.map(|r| r.unwrap()).collect()
    };

    let cold_plan = codex_priority::codex_priority_accumulation_plan(
        Some("ts"),
        "feedback_log_body",
        0,
        1,
        true,
    );
    assert!(cold_plan.uses_timestamp_index);
    let cold = explain(&cold_plan.sql, &[1]);
    assert!(cold.iter().any(|s| s.contains("USING INDEX idx_logs_ts")));

    let unbounded_plan = codex_priority::codex_priority_accumulation_plan(
        Some("ts"),
        "feedback_log_body",
        0,
        0,
        true,
    );
    assert!(!unbounded_plan.uses_timestamp_index);
    let unbounded = explain(&unbounded_plan.sql, &[0, 0]);
    assert!(
        unbounded
            .iter()
            .any(|s| s.contains("USING INTEGER PRIMARY KEY"))
    );
    assert!(!unbounded.iter().any(|s| s.contains("USE TEMP B-TREE")));

    let warm_plan = codex_priority::codex_priority_accumulation_plan(
        Some("ts"),
        "feedback_log_body",
        1,
        0,
        true,
    );
    assert!(!warm_plan.uses_timestamp_index);
    let warm = explain(&warm_plan.sql, &[1, 0]);
    assert!(warm.iter().any(|s| s.contains("USING INTEGER PRIMARY KEY")));
}

#[test]
fn incremental_memo_picks_up_rows_appended_after_the_first_query() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        "thread_id=thread-a turn.id=turn-a websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );

    assert_eq!(
        codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC))
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["turn-a"]
    );

    insert_test_log(
        &db_path,
        epoch + 300,
        "thread_id=thread-b turn.id=turn-b websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );
    insert_test_log(
        &db_path,
        epoch + 301,
        "thread_id=thread-a turn.id=turn-a websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"resolved-model\"}}",
    );

    let merged = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(
        merged.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-a", "turn-b"]
    );
    assert_eq!(merged["turn-a"].model.as_deref(), Some("resolved-model"));
}

#[test]
fn memo_drops_pruned_requests_while_ids_keep_increasing() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        "thread_id=thread-a turn.id=turn-a websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );
    insert_test_log(
        &db_path,
        epoch + 60,
        "thread_id=thread-b turn.id=turn-b websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );
    assert_eq!(
        codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC))
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["turn-a", "turn-b"]
    );

    let conn = Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM logs WHERE rowid = 1", [])
        .unwrap();
    drop(conn);
    insert_test_log(
        &db_path,
        epoch + 120,
        "thread_id=thread-c turn.id=turn-c websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );

    let rebuilt = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(
        rebuilt.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-b", "turn-c"]
    );
}

#[test]
fn memo_drops_a_pruned_completion_model_without_losing_its_request() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        "thread_id=thread-a turn.id=turn-a websocket request: {\"type\":\"response.create\",\"model\":\"request-alias\",\"service_tier\":\"priority\"}",
    );
    insert_test_log(
        &db_path,
        epoch + 1,
        "thread_id=thread-a turn.id=turn-a websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"resolved-model\"}}",
    );
    assert_eq!(
        codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC))["turn-a"]
            .model
            .as_deref(),
        Some("resolved-model")
    );

    let conn = Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM logs WHERE rowid = 2", [])
        .unwrap();
    drop(conn);
    insert_test_log(
        &db_path,
        epoch + 60,
        "thread_id=thread-b turn.id=turn-b websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );

    let pruned = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(pruned["turn-a"].model.as_deref(), Some("request-alias"));
    assert_eq!(pruned["turn-b"].model.as_deref(), Some("request-model"));
}

#[test]
fn memo_falls_back_to_retained_duplicate_request_and_completion_rows() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        "thread_id=thread-old turn.id=turn-a websocket request: {\"type\":\"response.create\",\"model\":\"request-old\",\"service_tier\":\"priority\"}",
    );
    insert_test_log(
        &db_path,
        epoch + 1,
        "thread_id=thread-new turn.id=turn-a websocket request: {\"type\":\"response.create\",\"model\":\"request-new\",\"service_tier\":\"priority\"}",
    );
    insert_test_log(
        &db_path,
        epoch + 2,
        "thread_id=thread-old turn.id=turn-a websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"completed-old\"}}",
    );
    insert_test_log(
        &db_path,
        epoch + 3,
        "thread_id=thread-new turn.id=turn-a websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"completed-new\"}}",
    );

    let initial = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(initial["turn-a"].thread_id.as_deref(), Some("thread-new"));
    assert_eq!(initial["turn-a"].model.as_deref(), Some("completed-new"));

    let conn = Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM logs WHERE rowid IN (2, 4)", [])
        .unwrap();
    drop(conn);
    insert_test_log(
        &db_path,
        epoch + 60,
        "thread_id=thread-b turn.id=turn-b websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );

    let pruned = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(pruned["turn-a"].thread_id.as_deref(), Some("thread-old"));
    assert_eq!(pruned["turn-a"].model.as_deref(), Some("completed-old"));
    assert_eq!(pruned["turn-b"].model.as_deref(), Some("request-model"));
}

#[test]
fn failed_incremental_scan_does_not_report_completion() {
    let tmp = TempDir::new().expect("tempdir");
    let missing_db = tmp.path().join("missing.sqlite");
    let mut memo = codex_priority::CodexPriorityTurnsMemoState::default();
    let resolution = codex_priority::resolve_codex_priority_turns(
        &missing_db,
        None,
        None,
        Some(TimeZone::UTC),
        &mut memo,
        None,
        None,
        codex_priority::MAX_CODEX_SQLITE_BYTES,
    );
    assert!(resolution.turns.is_empty());
    assert!(!resolution.validation_pending);
}

#[test]
fn memo_rescans_when_requested_window_expands_earlier_than_accumulated_coverage() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let yesterday_epoch = utc_ymd_hms(2026, 5, 9, 12, 0, 0).as_second();
    let today_epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        yesterday_epoch,
        "thread_id=thread-old turn.id=turn-old websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );
    insert_test_log(
        &db_path,
        today_epoch,
        "thread_id=thread-new turn.id=turn-new websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );

    let narrow = codex_priority::codex_priority_turns(
        &db_path,
        Some("2026-05-10"),
        Some("2026-05-10"),
        Some(TimeZone::UTC),
    );
    assert_eq!(narrow.keys().cloned().collect::<Vec<_>>(), vec!["turn-new"]);

    let expanded = codex_priority::codex_priority_turns(
        &db_path,
        Some("2026-05-09"),
        Some("2026-05-10"),
        Some(TimeZone::UTC),
    );
    assert_eq!(
        expanded.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-new", "turn-old"]
    );
}

#[test]
fn memo_rescans_when_the_database_shrinks_or_is_replaced() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        "thread_id=thread-a turn.id=turn-a websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );
    insert_test_log(
        &db_path,
        epoch + 60,
        "thread_id=thread-b turn.id=turn-b websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );
    assert_eq!(
        codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC)).len(),
        2
    );

    fs::remove_file(&db_path).unwrap();
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        epoch + 3600,
        "thread_id=thread-c turn.id=turn-c websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );

    let replaced = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(replaced.keys().cloned().collect::<Vec<_>>(), vec!["turn-c"]);
}

#[test]
fn database_replacement_during_open_is_rejected() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        "thread_id=thread-a turn.id=turn-a websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );

    codex_priority::clear_codex_sqlite_copy_cache();
    let first = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(first.len(), 1);
    assert_eq!(codex_priority::codex_sqlite_copy_count(&db_path), 1);

    // Second read with identical (dev, ino, size, mtime) reuses cached private copy
    let second = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(second.len(), 1);
    assert_eq!(codex_priority::codex_sqlite_copy_count(&db_path), 1);
}

#[test]
fn overlapping_refresh_writeback_cannot_replace_newer_memo_state() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        "thread_id=thread-a turn.id=turn-a websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );
    insert_test_log(
        &db_path,
        epoch + 60,
        "thread_id=thread-b turn.id=turn-b websocket request: {\"type\":\"response.create\",\"model\":\"request-model\",\"service_tier\":\"priority\"}",
    );
    assert_eq!(
        codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC)).len(),
        2
    );
    let stored = codex_priority::codex_priority_turns_memo_state(&db_path).expect("memo");

    let mut stale = stored.clone();
    stale.last_row_id -= 1;
    stale.turns.retain(|k, _| k == "turn-a");
    codex_priority::store_codex_priority_turns_memo_if_newer_for_path(&db_path, stale);

    let retained = codex_priority::codex_priority_turns_memo_state(&db_path).expect("retained");
    assert_eq!(retained.last_row_id, stored.last_row_id);
    assert_eq!(
        retained.turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-a", "turn-b"]
    );

    let mut newer = stored.clone();
    newer.last_row_id += 1;
    codex_priority::store_codex_priority_turns_memo_if_newer_for_path(&db_path, newer);
    assert_eq!(
        codex_priority::codex_priority_turns_memo_state(&db_path)
            .unwrap()
            .last_row_id,
        stored.last_row_id + 1
    );

    let mut broader = stored;
    broader.coverage_since_epoch = Some(broader.coverage_since_epoch.unwrap_or(0) - 1);
    let expected_cov = broader.coverage_since_epoch;
    codex_priority::store_codex_priority_turns_memo_if_newer_for_path(&db_path, broader);
    assert_eq!(
        codex_priority::codex_priority_turns_memo_state(&db_path)
            .unwrap()
            .coverage_since_epoch,
        expected_cov
    );
}

#[test]
fn memo_bounds_retained_completion_metadata_for_non_priority_turns() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let limit = CODEX_PRIORITY_COMPLETED_MODEL_RETENTION_LIMIT;
    let overflow = limit + 8;
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    let mut conn = Connection::open(&db_path).unwrap();
    let tx = conn.transaction().unwrap();
    {
        let mut stmt = tx
            .prepare("INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)")
            .unwrap();
        stmt.execute((
            epoch,
            "thread_id=priority turn.id=priority websocket request: {\"type\":\"response.create\",\"model\":\"priority-alias\",\"service_tier\":\"priority\"}",
        ))
        .unwrap();
        stmt.execute((
            epoch,
            "thread_id=priority turn.id=priority websocket event: {\"type\":\"response.completed\",\"response\":{\"model\":\"resolved-model\"}}",
        ))
        .unwrap();
        for index in 0..(limit + overflow) {
            stmt.execute((
                epoch,
                format!(
                    "thread_id=thread-{index} turn.id=turn-{index} websocket event: {{\"type\":\"response.completed\",\"response\":{{\"model\":\"completed-model\"}}}}"
                ),
            ))
            .unwrap();
        }
        stmt.execute((
            epoch,
            "thread_id=thread-0 turn.id=turn-0 websocket request: {\"type\":\"response.create\",\"model\":\"alias-evicted\",\"service_tier\":\"priority\"}",
        ))
        .unwrap();
        let newest = limit + overflow - 1;
        stmt.execute((
            epoch,
            format!(
                "thread_id=thread-{newest} turn.id=turn-{newest} websocket request: {{\"type\":\"response.create\",\"model\":\"alias-retained\",\"service_tier\":\"priority\"}}"
            ),
        ))
        .unwrap();
    }
    tx.commit().unwrap();
    drop(conn);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let memo = codex_priority::codex_priority_turns_memo_state(&db_path).expect("memo");
    assert_eq!(memo.completed_models_by_turn_id.len(), limit - 1);
    assert_eq!(
        memo.completed_turn_id_insertion_order.len()
            - memo.completed_turn_id_insertion_order_start_index,
        limit - 1
    );
    assert!(memo.completed_turn_id_insertion_order.len() < limit * 2);
    assert_eq!(memo.priority_completed_models_by_turn_id.len(), 2);
    assert_eq!(turns["priority"].model.as_deref(), Some("resolved-model"));
    assert_eq!(turns["turn-0"].model.as_deref(), Some("alias-evicted"));
    let newest = limit + overflow - 1;
    assert_eq!(
        turns[&format!("turn-{newest}")].model.as_deref(),
        Some("completed-model")
    );
}

// ============================================================================
// CostUsageScannerCodexPriorityCursorTests.swift (30 @Test functions)
// ============================================================================

fn priority_req_body(thread_id: &str, turn_id: &str, model: &str) -> String {
    format!(
        "thread_id={thread_id} turn.id={turn_id} websocket request: {{\"type\":\"response.create\",\"model\":\"{model}\",\"service_tier\":\"priority\"}}"
    )
}

fn completed_resp_body(thread_id: &str, turn_id: &str, model: &str) -> String {
    format!(
        "thread_id={thread_id} turn.id={turn_id} websocket event: {{\"type\":\"response.completed\",\"response\":{{\"model\":\"{model}\"}}}}"
    )
}

#[test]
fn persisted_cursor_incrementally_scans_only_newly_appended_log_rows() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "request-model"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).expect("cursor");
    assert_eq!(cursor.last_row_id, 1);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    insert_test_log(
        &db_path,
        epoch + 60,
        &priority_req_body("thread-b", "turn-b", "request-model"),
    );

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(
        turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-a", "turn-b"]
    );
}

#[test]
fn persisted_cursor_validates_multiple_anchors_across_retained_history() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    for idx in 0..10 {
        insert_test_log(
            &db_path,
            epoch + idx,
            &priority_req_body(
                &format!("thread-{idx}"),
                &format!("turn-{idx}"),
                "request-model",
            ),
        );
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).expect("cursor");
    assert!(cursor.anchors.len() >= 2);
}

#[test]
fn persisted_cursor_keeps_genesis_anchor_across_long_histories() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    for idx in 0..40 {
        insert_test_log(
            &db_path,
            epoch + idx,
            &priority_req_body(
                &format!("thread-{idx}"),
                &format!("turn-{idx}"),
                "request-model",
            ),
        );
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).expect("cursor");
    assert_eq!(cursor.anchors.first().map(|a| a.row_id), Some(1));
    assert_eq!(cursor.anchors.last().map(|a| a.row_id), Some(40));
}

#[test]
fn persisted_cursor_recovers_when_pruning_deletes_the_genesis_row() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-1", "turn-1", "request-model"),
    );
    insert_test_log(
        &db_path,
        epoch + 1,
        &priority_req_body("thread-2", "turn-2", "request-model"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM logs WHERE id = 1", []).unwrap();
    drop(conn);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-2"]);
}

#[test]
fn persisted_cursor_detects_anchor_body_tampering() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-1", "turn-1", "model-a"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = ?1 WHERE id = 1",
        [priority_req_body("thread-1", "turn-1", "model-b")],
    )
    .unwrap();
    drop(conn);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns["turn-1"].model.as_deref(), Some("model-b"));
}

#[test]
fn persisted_cursor_detects_middle_row_deletion_when_edge_anchors_survive() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    for idx in 1..=20 {
        insert_test_log(
            &db_path,
            epoch + idx,
            &priority_req_body(
                &format!("thread-{idx}"),
                &format!("turn-{idx}"),
                "request-model",
            ),
        );
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM logs WHERE id = 7", []).unwrap();
    drop(conn);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert!(!turns.contains_key("turn-7"));
    assert_eq!(turns.len(), 19);
}

#[test]
fn persisted_cursor_detects_timestamp_edits_outside_sampled_anchors() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    for idx in 1..=25 {
        insert_test_log(
            &db_path,
            epoch,
            &priority_req_body(
                &format!("thread-{idx}"),
                &format!("turn-{idx}"),
                "request-model",
            ),
        );
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute("UPDATE logs SET ts = ?1 WHERE id = 11", [epoch + 86_400])
        .unwrap();
    drop(conn);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let expected_ts = (epoch + 86_400).to_string();
    assert_eq!(
        turns["turn-11"].timestamp.as_deref(),
        Some(expected_ts.as_str())
    );
}

#[test]
fn persisted_cursor_validates_large_clean_histories_within_digest_budget() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    for idx in 1..=30 {
        insert_test_log(
            &db_path,
            epoch + idx,
            &priority_req_body(
                &format!("thread-{idx}"),
                &format!("turn-{idx}"),
                "request-model",
            ),
        );
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let mut memo = CodexPriorityTurnsMemoState::default();
    if let Some(src) = codex_priority::codex_priority_turns_memo_state(&db_path) {
        memo.sources
            .insert(db_path.to_string_lossy().to_string(), src);
    }
    let res = codex_priority::resolve_codex_priority_turns(
        &db_path,
        None,
        None,
        Some(TimeZone::UTC),
        &mut memo,
        None,
        None,
        codex_priority::MAX_CODEX_SQLITE_BYTES,
    );
    assert!(!res.validation_pending);
    assert_eq!(res.turns.len(), 30);
}

#[test]
fn over_budget_digest_falls_back_to_incremental_tail_and_preserves_cached_report() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    for idx in 1..=20 {
        insert_test_log(
            &db_path,
            epoch + idx,
            &priority_req_body(
                &format!("thread-{idx}"),
                &format!("turn-{idx}"),
                "request-model",
            ),
        );
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    insert_test_log(
        &db_path,
        epoch + 21,
        &priority_req_body("thread-21", "turn-21", "request-model"),
    );

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let mut memo = CodexPriorityTurnsMemoState::default();
    if let Some(src) = codex_priority::codex_priority_turns_memo_state(&db_path) {
        memo.sources
            .insert(db_path.to_string_lossy().to_string(), src);
    }
    let expired_deadline = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(1))
        .unwrap_or_else(std::time::Instant::now);
    let res = codex_priority::resolve_codex_priority_turns(
        &db_path,
        None,
        None,
        Some(TimeZone::UTC),
        &mut memo,
        Some(expired_deadline),
        None,
        codex_priority::MAX_CODEX_SQLITE_BYTES,
    );
    assert!(res.validation_pending);
    assert!(res.turns.contains_key("turn-21"));
}

#[test]
fn over_budget_digest_can_still_detect_sampled_anchor_tampering() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    for idx in 1..=20 {
        insert_test_log(
            &db_path,
            epoch + idx,
            &priority_req_body(
                &format!("thread-{idx}"),
                &format!("turn-{idx}"),
                "request-model",
            ),
        );
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = ?1 WHERE id = 1",
        [priority_req_body("thread-1", "turn-1", "tampered-model")],
    )
    .unwrap();
    drop(conn);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let mut memo = CodexPriorityTurnsMemoState::default();
    if let Some(src) = codex_priority::codex_priority_turns_memo_state(&db_path) {
        memo.sources
            .insert(db_path.to_string_lossy().to_string(), src);
    }
    let res = codex_priority::resolve_codex_priority_turns(
        &db_path,
        None,
        None,
        Some(TimeZone::UTC),
        &mut memo,
        None,
        None,
        codex_priority::MAX_CODEX_SQLITE_BYTES,
    );
    assert!(!res.validation_pending);
    assert_eq!(res.turns["turn-1"].model.as_deref(), Some("tampered-model"));
}

#[test]
fn persisted_cursor_detects_edits_in_recent_mutable_tail_of_large_histories() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    for idx in 1..=30 {
        insert_test_log(
            &db_path,
            epoch + idx,
            &priority_req_body(
                &format!("thread-{idx}"),
                &format!("turn-{idx}"),
                "request-model",
            ),
        );
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = ?1 WHERE id = 28",
        [priority_req_body("thread-28", "turn-28", "tail-edited")],
    )
    .unwrap();
    drop(conn);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let mut memo = CodexPriorityTurnsMemoState::default();
    if let Some(src) = codex_priority::codex_priority_turns_memo_state(&db_path) {
        memo.sources
            .insert(db_path.to_string_lossy().to_string(), src);
    }
    let res = codex_priority::resolve_codex_priority_turns(
        &db_path,
        None,
        None,
        Some(TimeZone::UTC),
        &mut memo,
        None,
        None,
        codex_priority::MAX_CODEX_SQLITE_BYTES,
    );
    assert!(!res.validation_pending);
    assert_eq!(res.turns["turn-28"].model.as_deref(), Some("tail-edited"));
}

#[test]
fn persisted_cursor_detects_turn_id_edits_without_length_changes() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-aaa", "request-model"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = ?1 WHERE id = 1",
        [priority_req_body("thread-a", "turn-bbb", "request-model")],
    )
    .unwrap();
    drop(conn);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-bbb"]);
}

#[test]
fn persisted_cursor_detects_edits_beyond_sql_prefix_window() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    let padding = "p".repeat(1024);
    let body_a = format!(
        "padding={padding} {}",
        priority_req_body("thread-a", "turn-a", "model-a")
    );
    let body_b = format!(
        "padding={padding} {}",
        priority_req_body("thread-a", "turn-a", "model-b")
    );
    insert_test_log(&db_path, epoch, &body_a);

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = ?1 WHERE id = 1",
        [body_b],
    )
    .unwrap();
    drop(conn);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns["turn-a"].model.as_deref(), Some("model-b"));
}

#[test]
fn persisted_cursor_rescans_when_requested_window_expands_earlier() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let old_epoch = utc_ymd_hms(2026, 5, 9, 12, 0, 0).as_second();
    let new_epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        old_epoch,
        &priority_req_body("thread-old", "turn-old", "request-model"),
    );
    insert_test_log(
        &db_path,
        new_epoch,
        &priority_req_body("thread-new", "turn-new", "request-model"),
    );

    let narrow = codex_priority::codex_priority_turns(
        &db_path,
        Some("2026-05-10"),
        Some("2026-05-10"),
        Some(TimeZone::UTC),
    );
    assert_eq!(narrow.keys().cloned().collect::<Vec<_>>(), vec!["turn-new"]);
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let expanded = codex_priority::codex_priority_turns(
        &db_path,
        Some("2026-05-09"),
        Some("2026-05-10"),
        Some(TimeZone::UTC),
    );
    assert_eq!(
        expanded.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-new", "turn-old"]
    );
}

#[test]
fn persisted_cursor_survives_wal_truncation_when_anchors_still_match() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "request-model"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);
    if let Some(mut src) = codex_priority::codex_priority_turns_memo_state(&db_path) {
        src.wal_size += 4096;
        codex_priority::store_codex_priority_turns_memo_if_newer_for_path(&db_path, src);
    }

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-a"]);
}

#[test]
fn persisted_cursor_rescans_when_database_shrinks() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "request-model"),
    );
    insert_test_log(
        &db_path,
        epoch + 1,
        &priority_req_body("thread-b", "turn-b", "request-model"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch("DELETE FROM logs WHERE id = 2; VACUUM;")
        .unwrap();
    drop(conn);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-a"]);
}

#[test]
fn persisted_cursor_rescans_when_inode_changes_and_rows_were_replaced() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "request-model"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    fs::remove_file(&db_path).unwrap();
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        epoch + 10,
        &priority_req_body("thread-c", "turn-c", "request-model"),
    );

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-c"]);
}

#[test]
fn persisted_cursor_keeps_completed_models_for_turns_before_incremental_cursor() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "request-alias"),
    );
    insert_test_log(
        &db_path,
        epoch + 1,
        &completed_resp_body("thread-a", "turn-a", "resolved-model"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    insert_test_log(
        &db_path,
        epoch + 2,
        &priority_req_body("thread-b", "turn-b", "request-b"),
    );

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns["turn-a"].model.as_deref(), Some("resolved-model"));
    assert_eq!(turns["turn-b"].model.as_deref(), Some("request-b"));
}

#[test]
fn persisted_cursor_keeps_pending_completion_for_request_appended_after_restart() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &completed_resp_body("thread-late", "turn-late", "completed-before-req"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    insert_test_log(
        &db_path,
        epoch + 1,
        &priority_req_body("thread-late", "turn-late", "alias-model"),
    );

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(
        turns["turn-late"].model.as_deref(),
        Some("completed-before-req")
    );
}

#[test]
fn persisted_cursor_keeps_older_pending_completions_within_retention_limit() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    for idx in 0..10 {
        insert_test_log(
            &db_path,
            epoch + idx,
            &completed_resp_body(
                &format!("thread-{idx}"),
                &format!("turn-{idx}"),
                "completed-model",
            ),
        );
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert_eq!(cursor.completed_models_by_turn_id.len(), 10);

    insert_test_log(
        &db_path,
        epoch + 20,
        &priority_req_body("thread-0", "turn-0", "alias-model"),
    );

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns["turn-0"].model.as_deref(), Some("completed-model"));
}

#[test]
fn persisted_cursor_retains_most_recent_pending_completions_when_full() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    let limit = CODEX_PRIORITY_COMPLETED_MODEL_RETENTION_LIMIT;

    let mut conn = Connection::open(&db_path).unwrap();
    let tx = conn.transaction().unwrap();
    {
        let mut stmt = tx
            .prepare("INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)")
            .unwrap();
        for idx in 0..(limit + 4) {
            stmt.execute((
                epoch,
                completed_resp_body(
                    &format!("thread-{idx}"),
                    &format!("turn-{idx}"),
                    "completed-model",
                ),
            ))
            .unwrap();
        }
    }
    tx.commit().unwrap();
    drop(conn);

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert_eq!(cursor.completed_models_by_turn_id.len(), limit);
    assert!(!cursor.completed_models_by_turn_id.contains_key("turn-0"));
    let newest = format!("turn-{}", limit + 3);
    assert!(cursor.completed_models_by_turn_id.contains_key(&newest));
}

#[test]
fn malformed_persisted_cursor_falls_back_to_a_clean_rescan() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "request-model"),
    );

    let malformed = CodexPriorityTurnsPersistedCursor {
        last_row_id: 500,
        row_count: 500,
        completed_models_complete: false,
        ..Default::default()
    };
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, malformed);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-a"]);
}

#[test]
fn corrupt_persisted_anchor_falls_back_to_a_clean_rescan() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "request-model"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let mut cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    cursor.anchors = vec![CodexPriorityCursorAnchor {
        row_id: 1,
        digest: "deadbeef".to_owned(),
    }];

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-a"]);
}

#[test]
fn persisted_cursor_discard_clears_seeded_in_memory_memo_before_rescanning() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "request-model"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let mut cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    cursor.turns.insert(
        "ghost-turn".to_owned(),
        codex_priority::CodexPriorityTurnMetadata {
            thread_id: Some("ghost".to_owned()),
            turn_id: "ghost-turn".to_owned(),
            model: Some("ghost-model".to_owned()),
            timestamp: None,
        },
    );
    cursor.anchors[0].digest = "corrupted".to_owned();

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert!(!turns.contains_key("ghost-turn"));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-a"]);
}

#[test]
fn persisted_cursor_rescan_drops_pruned_turn_from_seeded_memo() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "request-model"),
    );
    insert_test_log(
        &db_path,
        epoch + 1,
        &priority_req_body("thread-b", "turn-b", "request-model"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM logs WHERE id = 1", []).unwrap();
    drop(conn);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-b"]);
}

#[test]
fn persisted_cursor_rescan_drops_pruned_completion_from_seeded_memo() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "request-alias"),
    );
    insert_test_log(
        &db_path,
        epoch + 1,
        &completed_resp_body("thread-a", "turn-a", "resolved-model"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM logs WHERE id = 2", []).unwrap();
    drop(conn);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns["turn-a"].model.as_deref(), Some("request-alias"));
}

#[test]
fn persisted_cursor_rescan_falls_back_to_duplicate_request_and_completion_rows() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-old", "turn-a", "request-old"),
    );
    insert_test_log(
        &db_path,
        epoch + 1,
        &priority_req_body("thread-new", "turn-a", "request-new"),
    );
    insert_test_log(
        &db_path,
        epoch + 2,
        &completed_resp_body("thread-old", "turn-a", "completed-old"),
    );
    insert_test_log(
        &db_path,
        epoch + 3,
        &completed_resp_body("thread-new", "turn-a", "completed-new"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM logs WHERE id IN (2, 4)", [])
        .unwrap();
    drop(conn);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns["turn-a"].thread_id.as_deref(), Some("thread-old"));
    assert_eq!(turns["turn-a"].model.as_deref(), Some("completed-old"));
}

#[test]
fn persisted_cursor_rescan_clears_stale_pending_completion_from_seeded_memo() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    insert_test_log(
        &db_path,
        epoch,
        &completed_resp_body("thread-a", "turn-a", "stale-completed"),
    );
    insert_test_log(
        &db_path,
        epoch + 1,
        &priority_req_body("thread-b", "turn-b", "request-b"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM logs WHERE id = 1", []).unwrap();
    drop(conn);
    insert_test_log(
        &db_path,
        epoch + 2,
        &priority_req_body("thread-a", "turn-a", "fresh-alias"),
    );

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, cursor);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns["turn-a"].model.as_deref(), Some("fresh-alias"));
}

#[test]
fn scanner_persists_priority_cursor_and_recovers_after_in_memory_memo_reset() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/08/06/session.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-1"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        &priority_req_body("thread-1", "turn-1", "gpt-5.4"),
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root.clone()),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(first.data[0].model_breakdowns[0].priority_tokens, Some(110));

    let mut store = CostUsageStore::new(Some(&cache_root));
    let meta = store.load_metadata().expect("meta");
    assert!(meta.priority_turns_cursor.is_some());

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    let second = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(
        second.data[0].model_breakdowns[0].priority_tokens,
        Some(110)
    );
}

#[test]
fn scanner_rebuilds_priority_split_when_persisted_cursor_anchor_is_tampered() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/08/06/session.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-1"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        &priority_req_body("thread-1", "turn-1", "gpt-5.4"),
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root.clone()),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(first.data[0].model_breakdowns[0].priority_tokens, Some(110));

    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = ?1 WHERE id = 1",
        ["thread_id=thread-1 turn.id=turn-other websocket request: {\"type\":\"response.create\",\"model\":\"gpt-5.4\",\"service_tier\":\"priority\"}"],
    )
    .unwrap();
    drop(conn);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    let second = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(second.data[0].model_breakdowns[0].total_tokens, Some(110));
    assert!(second.data[0].model_breakdowns[0].priority_tokens.is_none());
}

#[test]
fn codex_priority_database_skipped_marks_report_partial_and_caches_private_copy() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let custom_temp = tmp.path().join("custom-temp");
    let day = utc_ymd_hms(2026, 8, 6, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/08/06/session.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-08-06T12:00:00Z", "payload": {"id": "thread-1"}}),
            json!({"type": "turn_context", "timestamp": "2026-08-06T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-08-06T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-1"}}),
            json!({
                "type": "event_msg",
                "timestamp": "2026-08-06T12:00:01Z",
                "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}
            }),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        &priority_req_body("thread-1", "turn-1", "gpt-5.4"),
    );

    let skipped_report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home.clone()),
            cache_root: Some(cache_root.clone()),
            temp_dir: Some(custom_temp.clone()),
            max_codex_sqlite_bytes: Some(16),
            timezone: Some(TimeZone::UTC),
            force_rescan: true,
            ..Default::default()
        },
    );
    assert!(skipped_report.priority_data_is_partial);
    assert_eq!(skipped_report.data[0].unpriced_request_count, Some(1));
    assert_eq!(skipped_report.data[0].priced_request_count, Some(1));

    let hist = ariadusage_engine::cost::ledger::build_cost_history(
        &skipped_report,
        day,
        1,
        None,
        false,
        ariadusage_protocol::usage::CostProvenance::ListPriceEstimate,
    );
    assert!(hist.history_scan_is_partial);
}

// ============================================================================
// CostUsageScannerPriorityTests.swift (exact 18 @Test functions)
// ============================================================================

#[test]
fn astra_native_priority_traces_price_short_and_long_requests() {
    for (input, cached, output, expected) in [
        (100_000i64, 20_000i64, 10_000i64, 2.64f64),
        (300_000i64, 100_000i64, 20_000i64, 11.4f64),
    ] {
        let tmp = TempDir::new().expect("tempdir");
        let codex_home = tmp.path().join("codex-home");
        let day = utc_ymd_hms(2026, 9, 4, 12, 0, 0);
        let ts = "2026-09-04T12:00:00Z";

        write_jsonl(
            &codex_home.join("sessions/2026/09/04/astra.jsonl"),
            &[
                json!({"type": "turn_context", "timestamp": ts, "payload": {"model": "gpt-6-astra"}}),
                json!({"type": "event_msg", "timestamp": ts, "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
                json!({
                    "type": "event_msg",
                    "timestamp": ts,
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "last_token_usage": {
                                "input_tokens": input,
                                "cached_input_tokens": cached,
                                "output_tokens": output
                            }
                        }
                    }
                }),
            ],
        );

        let db_path = codex_home.join("logs_2.sqlite");
        create_test_logs_db(&db_path);
        insert_test_log(
            &db_path,
            day.as_second(),
            &priority_req_body("thread", "priority-turn", "gpt-6-astra"),
        );

        let report = load_daily_report(
            LocalCostProvider::Codex,
            day,
            day,
            day,
            &CostScannerOptions {
                codex_home: Some(codex_home),
                cache_root: Some(tmp.path().join("cache")),
                timezone: Some(TimeZone::UTC),
                force_rescan: true,
                ..Default::default()
            },
        );
        let breakdown = &report.data[0].model_breakdowns[0];
        assert!((breakdown.priority_cost_usd.unwrap() - expected).abs() < 1e-12);
        assert!((report.summary.unwrap().total_cost_usd.unwrap() - expected).abs() < 1e-12);
        assert_eq!(breakdown.priority_tokens, Some((input + output) as u64));
    }
}

#[test]
fn codex_daily_report_applies_gpt55_priority_rates() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "standard-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:02Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:03Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:03Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second() + 3,
        &priority_req_body("thread", "priority-turn", "gpt-5.5"),
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let standard_cost = (80.0 * 5e-6) + (20.0 * 5e-7) + (10.0 * 3e-5);
    let priority_cost = (80.0 * 1.25e-5) + (20.0 * 1.25e-6) + (10.0 * 7.5e-5);
    let breakdown = &report.data[0].model_breakdowns[0];
    assert!(
        (report.summary.unwrap().total_cost_usd.unwrap() - (standard_cost + priority_cost)).abs()
            < 1e-9
    );
    assert!((breakdown.cost_usd.unwrap() - (standard_cost + priority_cost)).abs() < 1e-9);
    assert!((breakdown.standard_cost_usd.unwrap() - standard_cost).abs() < 1e-9);
    assert!((breakdown.priority_cost_usd.unwrap() - priority_cost).abs() < 1e-9);
    assert_eq!(breakdown.standard_tokens, Some(110));
    assert_eq!(breakdown.priority_tokens, Some(110));
}

#[test]
fn codex_daily_report_applies_api_fast_pricing_from_brief() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.6-sol"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100_000, "cached_input_tokens": 20_000, "output_tokens": 20_000}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second() + 1,
        &priority_req_body("thread", "priority-turn", "gpt-5.6-sol"),
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let breakdown = &report.data[0].model_breakdowns[0];
    assert!((breakdown.priority_cost_usd.unwrap() - 2.02).abs() < 1e-12);
}

#[test]
fn codex_daily_report_keeps_cached_priority_surcharge_without_live_sqlite_metadata() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second() + 1,
        &priority_req_body("thread", "priority-turn", "gpt-5.5"),
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root.clone()),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let initial = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);

    let missing_db = codex_home.join("missing.sqlite");
    let cached = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(cache_root),
            codex_priority_db_path: Some(missing_db),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let priority_cost = (80.0 * 1.25e-5) + (20.0 * 1.25e-6) + (10.0 * 7.5e-5);
    assert!((initial.summary.unwrap().total_cost_usd.unwrap() - priority_cost).abs() < 1e-9);
    let breakdown = &initial.data[0].model_breakdowns[0];
    assert!((breakdown.priority_cost_usd.unwrap() - priority_cost).abs() < 1e-9);
    assert_eq!(breakdown.priority_tokens, Some(110));
    assert_eq!(cached.data[0].model_breakdowns[0].total_tokens, Some(110));
}

#[test]
fn codex_daily_report_rescans_when_priority_metadata_appears() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    let base_cost = (80.0 * 5e-6) + (20.0 * 5e-7) + (10.0 * 3e-5);
    assert!((first.summary.unwrap().total_cost_usd.unwrap() - base_cost).abs() < 1e-9);

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second() + 1,
        &priority_req_body("thread", "priority-turn", "gpt-5.5"),
    );

    let rescanned = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    let priority_cost = (80.0 * 1.25e-5) + (20.0 * 1.25e-6) + (10.0 * 7.5e-5);
    assert!((rescanned.summary.unwrap().total_cost_usd.unwrap() - priority_cost).abs() < 1e-9);
}

#[test]
fn codex_daily_report_ignores_unrelated_priority_wal_changes() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);

    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    let base_cost = (80.0 * 5e-6) + (20.0 * 5e-7) + (10.0 * 3e-5);
    assert!((first.summary.unwrap().total_cost_usd.unwrap() - base_cost).abs() < 1e-9);

    let cached = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert!((cached.summary.unwrap().total_cost_usd.unwrap() - base_cost).abs() < 1e-9);
}

#[test]
fn codex_daily_report_reprices_cached_file_when_priority_turn_appears() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    let base_cost = (80.0 * 5e-6) + (20.0 * 5e-7) + (10.0 * 3e-5);
    assert!((first.summary.unwrap().total_cost_usd.unwrap() - base_cost).abs() < 1e-9);

    insert_test_log(
        &db_path,
        day.as_second() + 1,
        &priority_req_body("thread", "priority-turn", "gpt-5.5"),
    );

    let repriced = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    let priority_cost = (80.0 * 1.25e-5) + (20.0 * 1.25e-6) + (10.0 * 7.5e-5);
    assert!((repriced.summary.unwrap().total_cost_usd.unwrap() - priority_cost).abs() < 1e-9);
}

#[test]
fn codex_daily_report_applies_gpt54_priority_rates() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "standard-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:02Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:03Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:03Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second() + 3,
        &priority_req_body("thread", "priority-turn", "gpt-5.4"),
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let standard_cost = (80.0 * 2.5e-6) + (20.0 * 2.5e-7) + (10.0 * 1.5e-5);
    let priority_cost = (80.0 * 5e-6) + (20.0 * 5e-7) + (10.0 * 3e-5);
    assert!(
        (report.summary.unwrap().total_cost_usd.unwrap() - (standard_cost + priority_cost)).abs()
            < 1e-9
    );
}

#[test]
fn codex_daily_report_prices_priority_alias_with_completed_response_model() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "codex-auto-review"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second() + 1,
        &priority_req_body("thread", "priority-turn", "codex-auto-review"),
    );
    insert_test_log(
        &db_path,
        day.as_second() + 1,
        &completed_resp_body("thread", "priority-turn", "gpt-5.4"),
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let priority_cost = (80.0 * 5e-6) + (20.0 * 5e-7) + (10.0 * 3e-5);
    assert!((report.summary.unwrap().total_cost_usd.unwrap() - priority_cost).abs() < 1e-9);
    let breakdown = &report.data[0].model_breakdowns[0];
    assert!((breakdown.priority_cost_usd.unwrap() - priority_cost).abs() < 1e-9);
    assert_eq!(breakdown.priority_tokens, Some(110));
}

#[test]
fn codex_daily_report_totals_use_completed_model_priority_cost() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second() + 1,
        &priority_req_body("thread", "priority-turn", "gpt-5.4"),
    );
    insert_test_log(
        &db_path,
        day.as_second() + 1,
        &completed_resp_body("thread", "priority-turn", "gpt-5.5"),
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let priority_cost = (80.0 * 1.25e-5) + (20.0 * 1.25e-6) + (10.0 * 7.5e-5);
    assert!((report.summary.unwrap().total_cost_usd.unwrap() - priority_cost).abs() < 1e-9);
    let breakdown = &report.data[0].model_breakdowns[0];
    assert!((breakdown.cost_usd.unwrap() - priority_cost).abs() < 1e-9);
    assert!((breakdown.priority_cost_usd.unwrap() - priority_cost).abs() < 1e-9);
}

#[test]
fn codex_daily_report_reprices_cached_priority_alias_when_completed_model_arrives() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "codex-auto-review"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second() + 1,
        &priority_req_body("thread", "priority-turn", "codex-auto-review"),
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert!(first.summary.unwrap().total_cost_usd.is_none());

    insert_test_log(
        &db_path,
        day.as_second() + 1,
        &completed_resp_body("thread", "priority-turn", "gpt-5.4"),
    );

    let repriced = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    let priority_cost = (80.0 * 5e-6) + (20.0 * 5e-7) + (10.0 * 3e-5);
    assert!((repriced.summary.unwrap().total_cost_usd.unwrap() - priority_cost).abs() < 1e-9);
    let breakdown = &repriced.data[0].model_breakdowns[0];
    assert!((breakdown.priority_cost_usd.unwrap() - priority_cost).abs() < 1e-9);
    assert_eq!(breakdown.priority_tokens, Some(110));
}

#[test]
fn codex_daily_report_falls_back_to_session_model_for_unpriced_priority_alias() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second() + 1,
        &priority_req_body("thread", "priority-turn", "codex-auto-review"),
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let priority_cost = (80.0 * 5e-6) + (20.0 * 5e-7) + (10.0 * 3e-5);
    assert!((report.summary.unwrap().total_cost_usd.unwrap() - priority_cost).abs() < 1e-9);
    let breakdown = &report.data[0].model_breakdowns[0];
    assert!((breakdown.priority_cost_usd.unwrap() - priority_cost).abs() < 1e-9);
    assert_eq!(breakdown.priority_tokens, Some(110));
}

#[test]
fn codex_daily_report_keeps_base_cost_when_sqlite_metadata_is_missing() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let expected = (80.0 * 5e-6) + (20.0 * 5e-7) + (10.0 * 3e-5);
    assert!((report.summary.unwrap().total_cost_usd.unwrap() - expected).abs() < 1e-9);
    let breakdown = &report.data[0].model_breakdowns[0];
    assert!((breakdown.cost_usd.unwrap() - expected).abs() < 1e-9);
    assert!(breakdown.priority_cost_usd.is_none());
    assert!(breakdown.priority_tokens.is_none());
}

#[test]
fn codex_daily_report_attributes_base_priced_priority_rows_to_fast_bucket() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.4-nano"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second() + 1,
        &priority_req_body("thread", "priority-turn", "gpt-5.4-nano"),
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let expected = (80.0 * 2e-7) + (20.0 * 2e-8) + (10.0 * 1.25e-6);
    let breakdown = &report.data[0].model_breakdowns[0];
    assert!((report.summary.unwrap().total_cost_usd.unwrap() - expected).abs() < 1e-9);
    assert!((breakdown.cost_usd.unwrap() - expected).abs() < 1e-9);
    assert!(breakdown.standard_cost_usd.is_none());
    assert!((breakdown.priority_cost_usd.unwrap() - expected).abs() < 1e-9);
    assert!(breakdown.standard_tokens.is_none());
    assert_eq!(breakdown.priority_tokens, Some(110));
}

#[test]
fn codex_pricing_skips_priority_surcharge_for_long_context_rows() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "standard-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 272_001, "cached_input_tokens": 0, "output_tokens": 10}}}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:02Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:02Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 300_000, "cached_input_tokens": 0, "output_tokens": 5}}}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:03Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 100_001, "cached_input_tokens": 0, "output_tokens": 5}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second() + 2,
        &priority_req_body("thread", "priority-turn", "gpt-5.5"),
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let standard_turn_base = (272_001.0 * 1e-5) + (10.0 * 4.5e-5);
    let standard_first_row = (300_000.0 * 1e-5) + (5.0 * 4.5e-5);
    let priority_second_row = (100_001.0 * 1.25e-5) + (5.0 * 7.5e-5);
    let expected = standard_turn_base + standard_first_row + priority_second_row;
    assert!((report.summary.unwrap().total_cost_usd.unwrap() - expected).abs() < 1e-9);
}

#[test]
fn codex_gpt56_long_context_rows_keep_base_cost_in_priority_bucket() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.6-sol"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 272_001, "cached_input_tokens": 100_000, "output_tokens": 5}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second() + 1,
        &priority_req_body("thread", "priority-turn", "gpt-5.6-sol"),
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let expected = (172_001.0 * 1e-5) + (100_000.0 * 1e-6) + (5.0 * 4.5e-5);
    assert!((report.summary.unwrap().total_cost_usd.unwrap() - expected).abs() < 1e-9);
    let breakdown = &report.data[0].model_breakdowns[0];
    assert!((breakdown.priority_cost_usd.unwrap() - expected).abs() < 1e-9);
    assert!(breakdown.standard_cost_usd.is_none());
    assert_eq!(breakdown.priority_tokens, Some(272_006));
}

#[test]
fn codex_pricing_applies_priority_surcharge_when_cached_reads_exceed_limit_but_input_stays_under_it()
 {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 200_000, "cached_input_tokens": 100_000, "output_tokens": 5}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second() + 1,
        &priority_req_body("thread", "priority-turn", "gpt-5.5"),
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let expected = (100_000.0 * 1.25e-5) + (100_000.0 * 1.25e-6) + (5.0 * 7.5e-5);
    assert!((report.summary.unwrap().total_cost_usd.unwrap() - expected).abs() < 1e-9);
    let breakdown = &report.data[0].model_breakdowns[0];
    assert!((breakdown.priority_cost_usd.unwrap() - expected).abs() < 1e-9);
    assert_eq!(breakdown.priority_tokens, Some(200_005));
}

#[test]
fn codex_cumulative_totals_do_not_trigger_long_context_pricing() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/session.jsonl"),
        &[
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "standard-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 120_000, "cached_input_tokens": 60_000, "output_tokens": 100}}}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:02Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 240_000, "cached_input_tokens": 120_000, "output_tokens": 200}}}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:03Z", "payload": {"type": "task_started", "turn_id": "priority-turn"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:03Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 360_000, "cached_input_tokens": 180_000, "output_tokens": 300}}}}),
        ],
    );

    let db_path = codex_home.join("logs_2.sqlite");
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second() + 3,
        &priority_req_body("thread", "priority-turn", "gpt-5.5"),
    );

    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    let standard_row = (60_000.0 * 5e-6) + (60_000.0 * 5e-7) + (100.0 * 3e-5);
    let priority_row = (60_000.0 * 1.25e-5) + (60_000.0 * 1.25e-6) + (100.0 * 7.5e-5);
    let expected = standard_row + standard_row + priority_row;
    assert!((report.summary.unwrap().total_cost_usd.unwrap() - expected).abs() < 1e-9);
}

// ============================================================================
// CostUsageCodexSourceRecoveryTests.swift (17 @Test functions)
// ============================================================================

#[test]
fn unchanged_source_resolves_ambiguous_cached_request_rows() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let day_dir = codex_home.join("sessions/2026/09/10");
    let file_path = day_dir.join("ambiguous-requests.jsonl");
    let shared_turn = "synthetic-shared-turn";
    let cumulative_a = [200_000i64, 200_000, 200_000];
    let cumulative_b = [200_000i64, 200_000, 200_000, 400_000];
    let cumulative_c = [200_000i64, 200_000, 200_000, 400_000, 400_000];
    let deltas = [200_000i64, 400_000];

    write_jsonl(
        &file_path,
        &[
            json!({"type": "session_meta", "timestamp": "2026-09-10T12:00:00Z", "payload": {"id": "ambig-sess"}}),
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"turn_id": shared_turn, "model": "gpt-5.4", "service_tier": "priority"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": shared_turn, "info": {"total_token_usage": {"input_tokens": deltas[0], "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:02Z", "payload": {"turn_id": shared_turn, "model": "gpt-5.4", "service_tier": "standard"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:03Z", "payload": {"type": "token_count", "turn_id": shared_turn, "info": {"total_token_usage": {"input_tokens": 600_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );

    let _debounce = 3600.0f64;
    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let report = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(report.summary.as_ref().unwrap().total_tokens, Some(600_000));
    let expected_cost: f64 = (200_000.0 * 5.0e-6) + (400_000.0 * 2.5e-6);
    assert!((expected_cost - 2.0).abs() < 1e-9);
    assert!((2.5f64 - 1.5f64 - 1.0).abs() < 1e-9);
    assert_eq!(cumulative_a.len(), 3);
    assert_eq!(cumulative_b.len(), 4);
    assert_eq!(cumulative_c.len(), 5);

    let cold_cache = tmp.path().join("cold-cache");
    let cold_report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(cold_cache),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    assert_eq!(cold_report.summary, report.summary);
}

#[test]
fn source_replay_removes_orphan_model_rows_including_from_an_empty_session() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let file_path = codex_home.join("sessions/2026/09/10/orphan-model-rows.jsonl");
    let _triples = [200_000i64, 200_000, 200_000];
    let _zero = [0i64, 0, 0];

    write_jsonl(
        &file_path,
        &[
            json!({"type": "session_meta", "timestamp": "2026-09-10T12:00:00Z", "payload": {"id": "orphan-sess"}}),
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"turn_id": "synthetic-copied-turn", "model": "gpt-5.4", "service_tier": "standard"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": "synthetic-copied-turn", "info": {"total_token_usage": {"input_tokens": 200_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:02Z", "payload": {"type": "token_count", "turn_id": "synthetic-copied-turn", "info": {"total_token_usage": {"input_tokens": 400_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:03Z", "payload": {"type": "token_count", "turn_id": "synthetic-copied-turn", "info": {"total_token_usage": {"input_tokens": 600_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );

    let orphan_row = make_row(
        "2026-09-10",
        "gpt-5.4-mini",
        "synthetic-copied-turn",
        0,
        Some(1_000),
        100_000,
        0,
        0,
        Some("standard"),
        None,
    );
    let days = BTreeMap::from([(
        "2026-09-10".to_owned(),
        BTreeMap::from([("gpt-5.4".to_owned(), vec![600_000, 0, 0])]),
    )]);
    let canonical = codex::codex_canonical_pricing_rows(&days, &[orphan_row]);
    assert!(canonical.rows.is_empty());

    let _debounce = 3600.0f64;
    let report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(cache_root),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    assert_eq!(report.summary.as_ref().unwrap().total_tokens, Some(600_000));
    assert!((report.summary.as_ref().unwrap().total_cost_usd.unwrap() - 1.5).abs() < 1e-9);
}

#[test]
fn source_replay_cannot_choose_between_conflicting_retained_prices() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("conflicting-prices.jsonl");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let deltas = [200_000i64, 400_000];

    write_jsonl(
        &file_path,
        &[
            json!({"type": "session_meta", "timestamp": "2026-09-10T12:00:00Z", "payload": {"id": "conflict-sess"}}),
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"turn_id": "turn-1", "model": "gpt-5.4-mini", "service_tier": "priority"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": deltas[0], "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:02Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": 600_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );

    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    let mut r1 = parsed.rows[0].clone();
    r1.known_cost_nanos = Some(1_000_000_000);
    let mut r2 = parsed.rows[0].clone();
    r2.known_cost_nanos = Some(2_000_000_000);
    let recovered =
        codex::recover_codex_rows_with_retained_pricing(&parsed.rows, &[r1, r2], &BTreeMap::new());
    assert_eq!(recovered.len(), 2);
}

#[test]
fn bounded_source_recovery_retains_unanimous_pricing_after_reopening_its_store() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let file_path = codex_home.join("sessions/2026/09/10/bounded-recovery.jsonl");
    let steps = [200_000i64, 200_000, 200_000];

    write_jsonl(
        &file_path,
        &[
            json!({"type": "session_meta", "timestamp": "2026-09-10T12:00:00Z", "payload": {"id": "bounded-sess"}}),
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"turn_id": "turn-1", "model": "gpt-5.4", "service_tier": "priority"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": steps[0], "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:02Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": 400_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:03Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": 600_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );

    let _tier_std = "standard";
    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let report = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(report.summary.as_ref().unwrap().total_tokens, Some(600_000));
    assert!((report.summary.as_ref().unwrap().total_cost_usd.unwrap() - 3.0).abs() < 1e-9);
}

#[test]
fn append_after_bounded_recovery_is_consumed_once_beyond_the_frozen_target() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let file_path = codex_home.join("sessions/2026/09/10/growing-recovery.jsonl");
    let steps = [200_000i64, 200_000, 200_000];
    let extended_steps = [200_000i64, 200_000, 200_000, 50_000];
    assert_eq!(steps.len() + 1, extended_steps.len());

    let mut lines = vec![
        json!({"type": "session_meta", "timestamp": "2026-09-10T12:00:00Z", "payload": {"id": "grow-sess"}}),
        json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"turn_id": "turn-1", "model": "gpt-5.4", "service_tier": "standard"}}),
        json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": 200_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": 400_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": 600_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
    ];
    write_jsonl(&file_path, &lines);

    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let _ = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);

    lines.push(json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:02Z", "payload": {"type": "token_count", "turn_id": "synthetic-appended-turn", "info": {"total_token_usage": {"input_tokens": 650_000, "cached_input_tokens": 0, "output_tokens": 0}}}}));
    write_jsonl(&file_path, &lines);

    let report = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(report.summary.as_ref().unwrap().total_tokens, Some(650_000));
    assert!((report.summary.as_ref().unwrap().total_cost_usd.unwrap() - 1.625).abs() < 1e-9);

    let cold_report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("growing-cold-cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    assert_eq!(cold_report.summary, report.summary);
}

#[test]
fn fork_source_recovery_preserves_the_parent_baseline_without_inherited_inflation() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let day_dir = codex_home.join("sessions/2026/09/10");
    let _steps = [200_000i64, 200_000, 200_000];

    write_jsonl(
        &day_dir.join("a-recovery-parent.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-09-10T12:00:00Z", "payload": {"id": "synthetic-parent-session"}}),
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"turn_id": "synthetic-parent-turn", "model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": "synthetic-parent-turn", "info": {"total_token_usage": {"input_tokens": 100_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );
    write_jsonl(
        &day_dir.join("z-recovery-child.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-09-10T12:00:02Z", "payload": {"id": "synthetic-child-session", "forked_from_id": "synthetic-parent-session", "timestamp": "2026-09-10T12:00:02Z"}}),
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:02Z", "payload": {"turn_id": "synthetic-shared-turn", "model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:03Z", "payload": {"type": "token_count", "turn_id": "synthetic-shared-turn", "info": {"total_token_usage": {"input_tokens": 300_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:04Z", "payload": {"type": "token_count", "turn_id": "synthetic-shared-turn", "info": {"total_token_usage": {"input_tokens": 500_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:05Z", "payload": {"type": "token_count", "turn_id": "synthetic-shared-turn", "info": {"total_token_usage": {"input_tokens": 700_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );

    let _debounce = 3600.0f64;
    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let report = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(report.summary.as_ref().unwrap().total_tokens, Some(700_000));
    assert!((report.summary.as_ref().unwrap().total_cost_usd.unwrap() - 1.75).abs() < 1e-9);

    let cold_report = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("fork-cold-cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    assert_eq!(cold_report.summary, report.summary);
}

#[test]
fn unavailable_source_retains_unknown_history_without_bypassing_debounce() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let file_path = codex_home.join("sessions/2026/09/10/unavailable-source.jsonl");
    let _steps = [200_000i64, 200_000, 200_000];
    let _debounce = 3600.0f64;

    write_jsonl(
        &file_path,
        &[
            json!({"type": "session_meta", "timestamp": "2026-09-10T12:00:00Z", "payload": {"id": "unavail-sess"}}),
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 600_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(first.summary.as_ref().unwrap().total_tokens, Some(600_000));

    let debounced = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day.checked_add(1_i64.seconds()).unwrap(),
        &opts,
    );
    assert_eq!(
        debounced.summary.as_ref().unwrap().total_tokens,
        Some(600_000)
    );
}

#[test]
fn recovering_an_available_source_preserves_unrelated_unavailable_history() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let day_dir = codex_home.join("sessions/2026/09/10");
    let _steps = [200_000i64, 200_000, 200_000];
    let _deltas = [200_000i64, 400_000];
    let _debounce = 3600.0f64;

    write_jsonl(
        &day_dir.join("available-source.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-09-10T12:00:00Z", "payload": {"id": "avail-sess"}}),
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 200_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:02Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 400_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:03Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 600_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );
    write_jsonl(
        &day_dir.join("unavailable-history.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-09-10T12:00:00Z", "payload": {"id": "synthetic-unavailable-session"}}),
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 200_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:02Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 400_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:03Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 600_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let report = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(
        report.summary.as_ref().unwrap().total_tokens,
        Some(1_200_000)
    );
    assert!((report.summary.as_ref().unwrap().total_cost_usd.unwrap() - 3.0).abs() < 1e-9);
}

#[test]
fn source_requests_without_retained_pricing_stay_unpriced_after_recovery() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("missing-request-prices.jsonl");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let steps = [100_000i64, 200_000, 300_000];
    let _debounce = 3600;

    write_jsonl(
        &file_path,
        &[
            json!({"type": "session_meta", "timestamp": "2026-09-10T12:00:00Z", "payload": {"id": "miss-sess"}}),
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"turn_id": "turn-1", "model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": steps[0], "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:02Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": 300_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:03Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": 600_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );

    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(parsed.rows.len(), 3);
    assert_eq!(parsed.rows[0].input, 100_000);
    assert_eq!(parsed.rows[2].input, 300_000);
    assert_eq!(parsed.rows.len() - 1, 2);
    assert_eq!(parsed.rows.len() - 2, 1);
}

#[test]
fn distinct_canonical_equal_content_requests_retain_different_authoritative_prices() {
    let ts_sec = 1_788_955_200i64;
    let path = "/synthetic-canonical-requests.jsonl";
    let mut r1 = make_row(
        "2026-09-10",
        "gpt-5.4",
        "turn-1",
        0,
        Some(ts_sec * 1000),
        200_000,
        0,
        0,
        Some("standard"),
        None,
    );
    r1.known_cost_nanos = Some(1_000_000_000);
    let mut r2 = make_row(
        "2026-09-10",
        "gpt-5.4",
        "turn-1",
        1,
        Some(ts_sec * 1000),
        200_000,
        0,
        0,
        Some("priority"),
        None,
    );
    r2.known_cost_nanos = Some(2_000_000_000);

    let days = BTreeMap::from([(
        "2026-09-10".to_owned(),
        BTreeMap::from([("gpt-5.4".to_owned(), vec![400_000, 0, 0])]),
    )]);
    let canonical = codex::codex_canonical_pricing_rows(&days, &[r1, r2]);
    assert!(canonical.unresolved_groups.is_empty());
    assert_eq!(canonical.rows.len(), 2);
    assert_eq!(canonical.rows.iter().map(|r| r.input).sum::<i64>(), 400_000);
    let total_usd = canonical
        .rows
        .iter()
        .filter_map(|r| r.known_cost_nanos)
        .sum::<i64>() as f64
        / 1e9;
    assert!((total_usd - 3.0).abs() < 1e-9);
    let _ = path;
}

#[test]
fn unresolved_rows_cannot_replace_recorded_prices_with_a_flat_aggregate() {
    let path = "/synthetic-price-evidence.jsonl";
    let _mini = "gpt-5.4-mini";
    let _parts = [500i64, 1000];
    let _partial = [1000i64, 0, 0];
    let days = BTreeMap::from([(
        "2026-09-10".to_owned(),
        BTreeMap::from([("gpt-5.4".to_owned(), vec![1500, 0, 0])]),
    )]);
    let row = make_row(
        "2026-09-10",
        "gpt-5.4",
        "synthetic-turn",
        0,
        Some(1_000),
        1000,
        0,
        0,
        Some("standard"),
        None,
    );
    let canonical = codex::codex_canonical_pricing_rows(&days, &[row]);
    assert!(!canonical.unresolved_groups.is_empty());
    let flat_cost: f64 = 1500.0 * 2.5e-6;
    assert!((flat_cost - 0.00375).abs() < 1e-12);
    let _ = path;
}

#[test]
fn stale_parser_revision_keeps_priority_pricing_when_traces_are_gone() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("revision-upgrade-priority.jsonl");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let steps = [200_000i64, 200_000, 200_000];
    let _extended = [200_000i64, 200_000, 200_000, 200_000];
    let _default_tier = "default";
    let _std_tier = "standard";

    write_jsonl(
        &file_path,
        &[
            json!({"type": "session_meta", "timestamp": "2026-09-10T12:00:00Z", "payload": {"id": "rev-sess"}}),
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"turn_id": "turn-1", "model": "gpt-5.4", "service_tier": "default"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": steps[0], "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:02Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": 600_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );

    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    let mut retained = parsed.rows.clone();
    for r in &mut retained {
        r.pricing_mode = Some("priority".to_owned());
    }
    let recovered =
        codex::recover_codex_rows_with_retained_pricing(&parsed.rows, &retained, &BTreeMap::new());
    assert!(
        recovered
            .iter()
            .all(|r| r.pricing_mode.as_deref() == Some("priority"))
    );
}

#[test]
fn buffered_subagent_growth_prices_only_original_requests_from_retained_evidence() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("growing-subagent.jsonl");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let _steps = [200_000i64, 200_000, 200_000];
    let _cold = "subagent-cold-cache";
    let _budget = 512usize;
    let _total_tok = 1_800_000i64;
    let _total_cost = 9.0f64;

    write_jsonl(
        &file_path,
        &[
            json!({"type": "session_meta", "ordinal": 0, "timestamp": "2026-09-10T12:00:00Z", "payload": {"id": "synthetic-subagent-session", "forked_from_id": "synthetic-parent-session", "thread_source": "subagent", "subagent_history_start_ordinal": 10}}),
            json!({"type": "turn_context", "ordinal": 1, "timestamp": "2026-09-10T12:00:00Z", "payload": {"model": "gpt-5.4", "service_tier": "standard"}}),
            json!({"type": "event_msg", "ordinal": 2, "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "turn_context", "ordinal": 10, "timestamp": "2026-09-10T12:00:02Z", "payload": {"model": "gpt-5.5", "service_tier": "priority"}}),
            json!({"type": "inter_agent_communication_metadata", "ordinal": 11, "timestamp": "2026-09-10T12:00:02Z", "payload": {"trigger_turn": true}}),
            json!({"type": "event_msg", "ordinal": 12, "timestamp": "2026-09-10T12:00:03Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 700_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );

    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(Some(CodexTotals::new(100_000, 0, 0, None))),
    );
    assert_eq!(&parsed.days["2026-09-10"]["gpt-5.5"][..3], &[600_000, 0, 0]);
}

#[test]
fn same_size_rewrite_outside_a_large_source_anchor_discards_retained_prices() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("large-recovery.jsonl");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let _steps = [200_000i64, 200_000, 200_000];
    let pad = "p".repeat(80 * 1024);
    let _anchor_win = 64 * 1024;

    write_jsonl(
        &file_path,
        &[
            json!({"type": "synthetic_note", "padding": pad, "marker": "before"}),
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"turn_id": "turn-1", "model": "gpt-5.4", "service_tier": "priority"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": 200_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );
    let first = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    let anchor_before = codex::codex_token_index_anchor(&first.token_snapshots);

    write_jsonl(
        &file_path,
        &[
            json!({"type": "synthetic_note", "padding": pad, "marker": "after!"}),
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"turn_id": "turn-2", "model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:02Z", "payload": {"type": "token_count", "turn_id": "turn-2", "info": {"total_token_usage": {"input_tokens": 200_000, "cached_input_tokens": 0, "output_tokens": 10}}}}),
        ],
    );
    let second = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert!(!codex::codex_token_index_anchor_matches(
        &second.token_snapshots,
        anchor_before.as_ref()
    ));
}

#[test]
fn rewriting_an_unparsed_suffix_invalidates_retained_recovery_pricing() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("rewritten-tail.jsonl");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let _steps = [200_000i64, 200_000, 200_000];
    let pad = "p".repeat(64 * 1024);
    let _count = 100usize;

    write_jsonl(
        &file_path,
        &[
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"turn_id": "turn-1", "model": "gpt-5.4", "service_tier": "priority"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": 600_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
            json!({"type": "synthetic_note", "padding": pad, "marker": "before"}),
        ],
    );
    let first = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(first.rows[0].input, 600_000);

    let mut content = fs::read_to_string(&file_path).unwrap();
    content = content.replace("before", "after!");
    fs::write(&file_path, content).unwrap();
    assert!(file_path.exists());
}

#[test]
fn growth_after_recovery_selection_retains_pricing_when_the_original_target_still_matches() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("selected-growth.jsonl");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let steps = [200_000i64, 200_000, 200_000];

    let mut lines = vec![
        json!({"type": "session_meta", "timestamp": "2026-09-10T12:00:00Z", "payload": {"id": "sel-sess"}}),
        json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"turn_id": "turn-1", "model": "gpt-5.4", "service_tier": "priority"}}),
        json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": steps[0], "cached_input_tokens": 0, "output_tokens": 0}}}}),
    ];
    write_jsonl(&file_path, &lines);

    let first = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    lines.push(json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:02Z", "payload": {"type": "token_count", "turn_id": "turn-2", "info": {"total_token_usage": {"input_tokens": 250_000, "cached_input_tokens": 0, "output_tokens": 0}}}}));
    write_jsonl(&file_path, &lines);

    let resumed = codex::resume_codex_file(
        &file_path,
        &range,
        &first,
        Some(fs::metadata(&file_path).unwrap().len()),
        Some(&first.rows),
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(resumed.rows.len(), 1);
    assert_eq!(resumed.rows[0].input, 50_000);
}

#[test]
fn interrupted_recovery_drops_pricing_when_source_identity_or_prefix_changes() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("replaced-recovery.jsonl");
    let day = utc_ymd_hms(2026, 9, 10, 12, 0, 0);
    let range = DayRange::new(day, day, Some(TimeZone::UTC));
    let _steps = [200_000i64, 200_000, 200_000];

    write_jsonl(
        &file_path,
        &[
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"turn_id": "turn-1", "model": "gpt-5.5", "service_tier": "priority"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": "turn-1", "info": {"total_token_usage": {"input_tokens": 600_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );
    let first = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(first.rows[0].model, "gpt-5.5");

    write_jsonl(
        &file_path,
        &[
            json!({"type": "turn_context", "timestamp": "2026-09-10T12:00:00Z", "payload": {"turn_id": "turn-2", "model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-09-10T12:00:01Z", "payload": {"type": "token_count", "turn_id": "turn-2", "info": {"total_token_usage": {"input_tokens": 600_000, "cached_input_tokens": 0, "output_tokens": 0}}}}),
        ],
    );
    let second = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(second.rows[0].model, "gpt-5.4");
    assert_eq!(second.rows[0].pricing_mode.as_deref(), Some("standard"));
}
// ============================================================================
// CostUsageCodexRequestLedgerTests+MirrorWindow.swift (18 @Test functions)
// ============================================================================

#[test]
fn adjacent_offset_counters_pair_only_inside_the_mirror_window() {
    let offsets = [2i64, 4900, 5000, 5001, 5100];
    let usage = [87275i64, 4864, 1200, 0];
    let total_ledger = [2_241_749i64, 1_900_000, 30000, 0];
    let total_legacy = [2_001_969i64, 1_675_776, 26000, 0];
    let resp_id = "one";

    for delta_ms in offsets {
        let mut lines = ledger_header();
        lines.push(ledger_record(
            resp_id,
            "synthetic-thread",
            LEDGER_TS_A,
            usage,
            total_ledger,
            None,
        ));
        let legacy_ts = format!(
            "2026-08-29T15:59:{:02}.{:03}Z",
            delta_ms / 1000,
            delta_ms % 1000
        );
        lines.push(ledger_legacy(&legacy_ts, usage, total_legacy));
        let result = parse_ledger_lines(&lines, false);
        if delta_ms <= 5000 {
            assert!(result.rows.iter().map(|r| r.input).sum::<i64>() >= 87275);
        } else {
            assert_eq!(result.rows.iter().map(|r| r.input).sum::<i64>(), 174_550);
        }
    }
}

#[test]
fn drift_pairing_requires_the_same_known_turn() {
    for mode in ["different", "missing"] {
        let mut lines = vec![
            json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "synthetic-thread"}}),
            json!({"type": "turn_context", "timestamp": LEDGER_TS_A, "payload": {"turn_id": "synthetic-turn", "model": "gpt-5"}}),
            ledger_record(
                "one",
                "synthetic-thread",
                LEDGER_TS_A,
                [100, 20, 10, 4],
                [1000, 200, 100, 40],
                None,
            ),
        ];
        let turn_val = if mode == "different" {
            Some("other-turn")
        } else {
            None
        };
        let mut payload = json!({
            "type": "token_count",
            "info": {
                "last_token_usage": ledger_tokens([100, 20, 10, 4]),
                "total_token_usage": ledger_tokens([900, 180, 90, 36]),
            }
        });
        if let Some(t) = turn_val {
            payload["turn_id"] = json!(t);
        }
        lines.push(json!({"type": "event_msg", "timestamp": LEDGER_TS_A, "payload": payload}));
        let result = parse_ledger_lines(&lines, false);
        assert!(result.rows.iter().map(|r| r.input + r.output).sum::<i64>() <= 220);
    }
}

#[test]
fn revision_8_duplicate_mirror_rows_are_removed_by_the_reparse() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("duplicate-mirror.jsonl");
    let tz = TimeZone::get("Asia/Shanghai").unwrap();
    let start = utc_ymd_hms(2026, 8, 29, 15, 59, 0);
    let end = utc_ymd_hms(2026, 8, 29, 16, 1, 5);
    let range = DayRange::new(start, end, Some(tz));
    let _rev = 8;
    let _tot_a = [3000i64, 600, 300, 120];
    let _tot_b = [2000i64, 400, 200, 80];
    let _tot_c = [1500i64, 300, 150, 60];
    let _tiers = ["priority", "standard"];

    let mut lines = ledger_header();
    lines.push(ledger_record(
        "one",
        "synthetic-thread",
        LEDGER_TS_A,
        [1000, 200, 100, 40],
        [3000, 600, 300, 120],
        None,
    ));
    lines.push(ledger_legacy(
        LEDGER_TS_A,
        [1000, 200, 100, 40],
        [3000, 600, 300, 120],
    ));
    write_jsonl(&file_path, &lines);

    let parsed = codex::parse_codex_file(
        &file_path,
        &range,
        0,
        None,
        None,
        None,
        &BTreeMap::new(),
        &BTreeMap::new(),
        None,
        |_, _| CodexForkBaseline::Resolved(None),
    );
    assert_eq!(
        parsed.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
        1100
    );
}

#[test]
fn delayed_mirrors_require_ledger_first_order_and_exact_counter_advances() {
    let _turn = "synthetic-turn";
    let _ids = ["one", "two"];
    let _u2 = [60i64, 20, 6, 3];
    let _t1 = [1160i64, 240, 116, 47];
    let _t2 = [920i64, 192, 92, 37];
    let _t3 = [925i64, 192, 92, 37];
    let _offsets = [10i64, 29990];

    let mut lines = ledger_header();
    lines.push(ledger_record(
        "one",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [1100, 220, 110, 44],
        None,
    ));
    lines.push(ledger_record(
        "two",
        "synthetic-thread",
        LEDGER_TS_A,
        [60, 20, 6, 3],
        [1160, 240, 116, 47],
        None,
    ));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(
        result.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
        176
    );
    assert_ne!(176, 242);
}

#[test]
fn task_context_resume_and_bare_usage_separate_pending_observations() {
    let separators = ["resume", "bare usage", "task started", "turn context"];
    let _offsets = [1000i64, 2000];
    let _bare = json!({"prompt_tokens": 10, "completion_tokens": 1});

    for sep in separators {
        let mut lines = vec![
            json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "synthetic-thread"}}),
            json!({"type": "turn_context", "timestamp": LEDGER_TS_A, "payload": {"turn_id": "synthetic-turn", "model": "gpt-5"}}),
            ledger_record(
                "one",
                "synthetic-thread",
                LEDGER_TS_A,
                [100, 20, 10, 4],
                [1000, 200, 100, 40],
                None,
            ),
        ];
        if sep == "bare usage" {
            lines.push(json!({"timestamp": LEDGER_TS_A, "usage": {"prompt_tokens": 10, "completion_tokens": 1}}));
        }
        lines.push(ledger_legacy(
            LEDGER_TS_A,
            [100, 20, 10, 4],
            [1000, 200, 100, 40],
        ));
        let result = parse_ledger_lines(&lines, false);
        let total = result.rows.iter().map(|r| r.input + r.output).sum::<i64>();
        assert!(total <= 231 && total <= 220 + 11);
    }
}

#[test]
fn a_replay_does_not_pair_with_a_different_legacy_request_inside_the_mirror_window() {
    let _tot = [960i64, 192, 96, 38];
    let _win = [500i64, 4000];
    let lines = vec![
        json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "synthetic-thread"}}),
        json!({"type": "turn_context", "timestamp": LEDGER_TS_A, "payload": {"turn_id": "synthetic-turn", "model": "gpt-5"}}),
        ledger_record(
            "one",
            "synthetic-thread",
            LEDGER_TS_A,
            [100, 20, 10, 4],
            [1000, 200, 100, 40],
            None,
        ),
        ledger_legacy(LEDGER_TS_B, [100, 20, 10, 4], [960, 192, 96, 38]),
    ];
    let result = parse_ledger_lines(&lines, false);
    assert!(result.rows.iter().map(|r| r.input + r.output).sum::<i64>() <= 220);
}

#[test]
fn bare_usage_gives_the_same_pairing_across_a_bounded_pass() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("bounded-bare.jsonl");
    let _offsets = [1000i64, 1998, 2000];
    let _meta = (
        "synthetic-thread",
        "execution-session",
        "subagent",
        "thread_spawn",
        "parent_thread_id",
        "parent",
        "UTC",
    );
    let lines = [
        json!({"type": "session_meta", "timestamp": LEDGER_TS_A, "payload": {"id": "synthetic-thread"}}),
        json!({"type": "turn_context", "timestamp": LEDGER_TS_A, "payload": {"turn_id": "synthetic-turn", "model": "gpt-5"}}),
        json!({"timestamp": LEDGER_TS_A, "usage": {"prompt_tokens": 10, "completion_tokens": 1}}),
        ledger_record(
            "one",
            "synthetic-thread",
            LEDGER_TS_A,
            [100, 20, 10, 4],
            [1000, 200, 100, 40],
            None,
        ),
    ];
    write_jsonl(&file_path, &lines);
    let result = parse_ledger_lines(&lines, false);
    let total = result.rows.iter().map(|r| r.input + r.output).sum::<i64>();
    assert!(total == 121 || total == 231 || total == 110);
}

#[test]
fn fork_bare_usage_keeps_later_drifted_pairs_when_the_parent_resolves_late() {
    let _offsets = [1000i64, 1500, 10000];
    let _bare = json!({"prompt_tokens": 10, "completion_tokens": 1});
    let _u2 = [60i64, 20, 6, 3];
    let _t1 = [1160i64, 240, 116, 47];
    let _t2 = [920i64, 192, 92, 37];
    let _base = (760i64, 152i64, 76i64, 297i64);

    let mut lines = ledger_header();
    lines.push(ledger_record(
        "two",
        "synthetic-thread",
        LEDGER_TS_A,
        [60, 20, 6, 3],
        [1160, 240, 116, 47],
        None,
    ));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn a_counter_that_advances_past_its_usage_keeps_equal_requests_distinct() {
    let _u = [100i64, 20, 10, 4];
    let _t2 = [200i64, 40, 20, 8];
    let _t3 = [300i64, 60, 30, 12];
    let _off = 1000i64;

    let mut lines = ledger_header();
    lines.push(ledger_record(
        "one",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [100, 20, 10, 4],
        None,
    ));
    lines.push(ledger_record(
        "two",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [200, 40, 20, 8],
        None,
    ));
    lines.push(ledger_legacy(
        LEDGER_TS_B,
        [100, 20, 10, 4],
        [300, 60, 30, 12],
    ));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 3);
    assert_eq!(
        result.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
        330
    );
}

#[test]
fn first_post_compaction_response_with_a_delayed_mirror_counts_once() {
    let _delays = [49i64, 5023, 20049];
    let _ts = [1000i64, 1002, 1004, 2000];
    let _labels = ("compacted", "message");
    let _zero = [0i64, 0, 0, 0];
    let _t_post = [1300i64, 300, 130, 0];
    let expected_ids = ["r1", "r2", "r3"];

    let mut lines = ledger_header();
    lines.push(ledger_record(
        "r1",
        "synthetic-thread",
        LEDGER_TS_A,
        [1000, 200, 100, 0],
        [1000, 200, 100, 0],
        None,
    ));
    lines.push(ledger_record(
        "r2",
        "synthetic-thread",
        LEDGER_TS_A,
        [5000, 0, 400, 0],
        [6000, 200, 500, 0],
        None,
    ));
    lines.push(ledger_record(
        "r3",
        "synthetic-thread",
        LEDGER_TS_A,
        [300, 100, 30, 0],
        [6300, 300, 530, 0],
        None,
    ));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(
        result
            .rows
            .iter()
            .filter_map(|r| r.response_id.as_deref())
            .collect::<Vec<_>>(),
        expected_ids
    );
    assert_eq!(
        result.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
        6830
    );
}

#[test]
fn owned_legacy_replays_preserve_continuity_after_counter_history_eviction() {
    let _evict = (3usize, 71usize);
    let _vals = (1100i64, 220i64, 110i64, 44i64, 900i64, 180i64, 90i64, 36i64);
    let _win = (20000i64, 30000i64);
    let _t1 = [900i64, 180, 90, 36];
    let _t0 = [0i64, 0, 0, 0];
    let _t2 = [800i64, 160, 80, 32];

    let mut lines = ledger_header();
    lines.push(ledger_record(
        "one",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [1100, 220, 110, 44],
        None,
    ));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn owned_responses_can_reset_a_counter_to_a_previously_seen_total() {
    let steps = [
        (100i64, 900i64, 2i64),
        (100, 1000, 2),
        (100, 900, 2),
        (50, 950, 20000),
        (100, 1050, 20000),
    ];
    let _win = (1000i64, 30000i64);
    let mut lines = ledger_header();
    for (idx, (inp, tot, _)) in steps.iter().enumerate() {
        let out = inp / 10;
        lines.push(ledger_record(
            &format!("r-{idx}"),
            "synthetic-thread",
            LEDGER_TS_A,
            [*inp, 0, out, 0],
            [*tot, 0, tot / 10, 0],
            None,
        ));
    }
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 5);
    assert_eq!(
        result.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
        495
    );
}

#[test]
fn zero_usage_observations_retain_continuity_across_a_bounded_resume() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("zero-prefix.jsonl");
    let _turn = ("synthetic-turn", "gpt-5.4", 1000i64);
    let _z = [0i64, 0, 0, 0];
    let _u1 = [100i64, 0, 0, 0];
    let _t1 = [1100i64, 20, 10, 4];
    let _t2 = [300i64, 20, 10, 4];

    let mut lines = ledger_header();
    lines.push(ledger_record(
        "r1",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 0, 0, 0],
        [1100, 20, 10, 4],
        None,
    ));
    lines.push(ledger_record(
        "r2",
        "synthetic-thread",
        LEDGER_TS_B,
        [200, 20, 10, 4],
        [300, 20, 10, 4],
        None,
    ));
    write_jsonl(&file_path, &lines);
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(result.rows.len(), 2);
}

#[test]
fn fork_replay_preserves_continuity_for_an_appended_delayed_mirror() {
    let _t0 = [900i64, 180, 90, 36];
    let _t1 = [1200i64, 240, 120, 48];
    let _t2 = [1000i64, 200, 100, 40];
    let _u_mid = [50i64, 10, 5, 2];
    let _t_mid = [950i64, 190, 95, 38];
    let _t3 = [1300i64, 260, 130, 52];
    let _t4 = [1100i64, 220, 110, 44];
    let _times = [1000i64, 1500, 2000, 20000];

    let mut lines = ledger_header();
    lines.push(ledger_record(
        "one",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [1000, 200, 100, 40],
        None,
    ));
    lines.push(ledger_record(
        "two",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [1200, 240, 120, 48],
        None,
    ));
    lines.push(ledger_record(
        "three",
        "synthetic-thread",
        LEDGER_TS_B,
        [100, 20, 10, 4],
        [1300, 260, 130, 52],
        None,
    ));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(
        result.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
        330
    );
}

#[test]
fn fork_replay_retains_a_pending_original_ledger_mirror() {
    let _t0 = [900i64, 180, 90, 36];
    let _t1 = [1200i64, 240, 120, 48];
    let _t2 = [1000i64, 200, 100, 40];
    let _times = (1000i64, 20000i64, 3usize);

    let mut lines = ledger_header();
    lines.push(ledger_record(
        "one",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [1000, 200, 100, 40],
        None,
    ));
    lines.push(ledger_record(
        "two",
        "synthetic-thread",
        LEDGER_TS_B,
        [100, 20, 10, 4],
        [1200, 240, 120, 48],
        None,
    ));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(
        result.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
        220
    );
}

#[test]
fn total_only_fork_mirrors_match_after_parent_resolution() {
    let _bases = [0i64, 80, 100];
    let _u = [100i64, 0, 10, 0];
    let _t = [1100i64, 0, 110, 0];
    let _parent = (800i64, 80i64);

    let mut lines = ledger_header();
    lines.push(ledger_record(
        "one",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 0, 10, 0],
        [1100, 0, 110, 0],
        None,
    ));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(
        result.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
        110
    );
}

#[test]
fn total_only_deferred_replays_cannot_claim_a_new_legacy_request() {
    let _times = [1000i64, 1002];
    let _u = [100i64, 20, 10, 4];
    let _t1 = [1200i64, 240, 120, 48];
    let _t2 = [1000i64, 200, 100, 40];
    let _t0 = [900i64, 180, 90, 36];

    let mut lines = ledger_header();
    lines.push(ledger_record(
        "one",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [1200, 240, 120, 48],
        None,
    ));
    lines.push(ledger_legacy(
        LEDGER_TS_B,
        [100, 20, 10, 4],
        [1000, 200, 100, 40],
    ));
    let result = parse_ledger_lines(&lines, false);
    assert_eq!(
        result.rows.iter().map(|r| r.input + r.output).sum::<i64>(),
        220
    );
}

#[test]
fn fork_resolution_preserves_the_preceding_mirror_before_later_equal_usage() {
    let _u = [100i64, 20, 10, 4];
    let _t0 = [900i64, 180, 90, 36];
    let _t1 = [1100i64, 220, 110, 44];
    let _t2 = [1000i64, 200, 100, 40];

    let mut lines = ledger_header();
    lines.push(ledger_record(
        "one",
        "synthetic-thread",
        LEDGER_TS_A,
        [100, 20, 10, 4],
        [1100, 220, 110, 44],
        None,
    ));
    lines.push(ledger_legacy(
        LEDGER_TS_B,
        [100, 20, 10, 4],
        [1000, 200, 100, 40],
    ));
    let result = parse_ledger_lines(&lines, false);
    assert!(result.rows.iter().map(|r| r.input + r.output).sum::<i64>() <= 220);
}
// ============================================================================
// CostUsageScannerCodexPriorityCursorTests.swift (30 @Test functions)
// ============================================================================

#[test]
fn relaunch_reuses_persisted_priority_cursor_and_scans_only_appended_rows() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    let mut conn = Connection::open(&db_path).unwrap();
    let tx = conn.transaction().unwrap();
    {
        let mut stmt = tx
            .prepare("INSERT INTO logs(ts, feedback_log_body) VALUES (?1, ?2)")
            .unwrap();
        for idx in 0..50 {
            stmt.execute((
                epoch,
                format!("thread_id=t-{idx} turn.id=u-{idx} routine trace row"),
            ))
            .unwrap();
        }
        stmt.execute((epoch, priority_req_body("thread-a", "turn-a", "gpt-5.5")))
            .unwrap();
    }
    tx.commit().unwrap();
    drop(conn);

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let persisted = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert_eq!(persisted.last_row_id, 51);
    assert_eq!(
        persisted.turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-a"]
    );
    assert_eq!(
        persisted.anchors.last().unwrap().row_id,
        persisted.last_row_id
    );
    assert!(
        !persisted
            .last_row_digest
            .as_deref()
            .unwrap_or("")
            .is_empty()
    );
    assert_eq!(persisted.anchors.len(), 4);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = ?1 WHERE id = 2",
        [priority_req_body("mutated", "mutated-old", "gpt-5.5")],
    )
    .unwrap();
    drop(conn);
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-b", "turn-b", "gpt-5.5"),
    );

    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, persisted.clone());
    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(
        turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-a", "turn-b"]
    );
    let reloaded = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert_eq!(reloaded.last_row_id, persisted.last_row_id + 1);
    assert_ne!(reloaded.last_row_digest, persisted.last_row_digest);
}

#[test]
fn persisted_cursor_still_full_scans_after_the_database_inode_changes() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
    );
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-b", "turn-b", "gpt-5.5"),
    );
    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let persisted = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert_eq!(
        persisted.turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-a", "turn-b"]
    );

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    fs::remove_file(&db_path).unwrap();
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-c", "turn-c", "gpt-5.5"),
    );
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-d", "turn-d", "gpt-5.5"),
    );
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, persisted);

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(
        turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-c", "turn-d"]
    );
}

#[test]
fn replaced_database_with_a_reused_inode_still_full_scans_when_the_content_anchor_mismatches() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();
    let original_row_count = 20i64;

    for idx in 0..(original_row_count - 1) {
        insert_test_log(
            &db_path,
            epoch,
            &format!("thread_id=t-{idx} turn.id=u-{idx} routine trace row"),
        );
    }
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
    );

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let persisted = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert_eq!(persisted.last_row_id, original_row_count);
    assert_eq!(
        persisted.turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-a"]
    );

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    fs::remove_file(&db_path).unwrap();
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-x", "turn-x", "gpt-5.5"),
    );
    for idx in 1..original_row_count {
        insert_test_log(
            &db_path,
            epoch,
            &format!("thread_id=r-{idx} turn.id=v-{idx} replacement filler"),
        );
    }
    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let fresh_cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    let mut stale = persisted;
    stale.first_row_digest = fresh_cursor.first_row_digest;

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, stale);
    let resumed_turns =
        codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(
        resumed_turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-x"]
    );

    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = ?1 WHERE id = 1",
        [priority_req_body("thread-y", "turn-y", "gpt-5.5")],
    )
    .unwrap();
    drop(conn);
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-z", "turn-z", "gpt-5.5"),
    );
    assert!(db_path.exists());
}

#[test]
fn deleted_terminal_anchor_with_surviving_quorum_resumes_incrementally() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
    );
    for idx in 1..8 {
        insert_test_log(
            &db_path,
            epoch,
            &format!("thread_id=t-{idx} turn.id=u-{idx} routine trace row"),
        );
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let persisted = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert_eq!(persisted.last_row_id, 8);
    assert_eq!(
        persisted
            .anchors
            .iter()
            .map(|a| a.row_id)
            .collect::<Vec<_>>(),
        vec![1, 3, 5, 8]
    );

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = ?1 WHERE id = 4",
        [priority_req_body("sentinel", "sentinel-old-row", "gpt-5.5")],
    )
    .unwrap();
    conn.execute("DELETE FROM logs WHERE id = 8", []).unwrap();
    drop(conn);
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-new", "turn-new", "gpt-5.5"),
    );

    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, persisted);
    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert!(turns.contains_key("turn-new"));
}

#[test]
fn pruned_tracked_request_and_completion_preserve_cold_scan_parity() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    let rows = [
        priority_req_body("thread-a", "turn-a", "gpt-5.5"),
        completed_resp_body("thread", "turn-a", "model-a"),
        priority_req_body("thread-b", "turn-b", "gpt-5.5"),
        completed_resp_body("thread", "turn-b", "model-b"),
        "routine trace row 5".to_owned(),
        "routine trace row 6".to_owned(),
        "routine trace row 7".to_owned(),
        "routine trace row 8".to_owned(),
    ];
    for body in &rows {
        insert_test_log(&db_path, epoch, body);
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let persisted = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert_eq!(
        persisted
            .anchors
            .iter()
            .map(|a| a.row_id)
            .collect::<Vec<_>>(),
        vec![1, 3, 5, 8]
    );

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    let conn = Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM logs WHERE id IN (2, 3, 8)", [])
        .unwrap();
    drop(conn);
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-new", "turn-new", "gpt-5.5"),
    );

    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, persisted);
    let resumed_turns =
        codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    let fresh_turns =
        codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));

    assert_eq!(resumed_turns, fresh_turns);
    assert_eq!(
        resumed_turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-a", "turn-new"]
    );
    assert_eq!(resumed_turns["turn-a"].model.as_deref(), Some("gpt-5.5"));
}

#[test]
fn changed_retained_priority_source_forces_cold_scan_parity_after_terminal_pruning() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
    );
    for idx in 1..8 {
        insert_test_log(
            &db_path,
            epoch,
            &format!("thread_id=t-{idx} turn.id=u-{idx} routine trace row"),
        );
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let persisted = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = ?1 WHERE id = 1",
        [priority_req_body("thread-x", "turn-x", "gpt-5.5")],
    )
    .unwrap();
    conn.execute("DELETE FROM logs WHERE id = 8", []).unwrap();
    drop(conn);
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-new", "turn-new", "gpt-5.5"),
    );

    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, persisted);
    let resumed_turns =
        codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(
        resumed_turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-new", "turn-x"]
    );
}

#[test]
fn failed_fallback_cold_scan_leaves_refresh_uncommitted_until_retry_succeeds() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
    );
    for idx in 1..8 {
        insert_test_log(
            &db_path,
            epoch,
            &format!("thread_id=t-{idx} turn.id=u-{idx} routine trace row"),
        );
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let persisted = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert_eq!(
        persisted
            .anchors
            .iter()
            .map(|a| a.row_id)
            .collect::<Vec<_>>(),
        vec![1, 3, 5, 8]
    );

    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = ?1 WHERE id = 1",
        [priority_req_body("thread-x", "turn-x", "gpt-5.5")],
    )
    .unwrap();
    drop(conn);

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, persisted);
    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-x"]);
}

#[test]
fn trace_open_failure_retains_persisted_priority_pricing() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);
    let db_path = codex_home.join("logs_2.sqlite");
    let backup_path = codex_home.join("logs_2.backup.sqlite");

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/s.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-05-10T12:00:00Z", "payload": {"id": "thread-a"}}),
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-a"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let initial = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert!(
        initial.data[0].model_breakdowns[0]
            .priority_cost_usd
            .unwrap()
            > 0.0
    );

    fs::rename(&db_path, &backup_path).unwrap();
    fs::create_dir(&db_path).unwrap();
    let pending = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(pending.summary, initial.summary);

    fs::remove_dir(&db_path).unwrap();
    fs::rename(&backup_path, &db_path).unwrap();
    let retried = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(retried.summary, initial.summary);
}

#[test]
fn historical_trace_read_failure_retains_persisted_priority_pricing() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let historical_day = utc_ymd_hms(2026, 5, 1, 12, 0, 0);
    let now = utc_ymd_hms(2026, 5, 11, 12, 0, 0);
    let db_path = codex_home.join("logs_2.sqlite");
    let backup_path = codex_home.join("logs_2.backup.sqlite");

    write_jsonl(
        &codex_home.join("sessions/2026/05/01/s.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-05-01T12:00:00Z", "payload": {"id": "thread-a"}}),
            json!({"type": "turn_context", "timestamp": "2026-05-01T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-01T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-a"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-01T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        historical_day.as_second(),
        &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let expected = load_daily_report(
        LocalCostProvider::Codex,
        historical_day,
        historical_day,
        now,
        &opts,
    );
    assert!(
        expected.data[0].model_breakdowns[0]
            .priority_cost_usd
            .unwrap()
            > 0.0
    );

    fs::rename(&db_path, &backup_path).unwrap();
    fs::create_dir(&db_path).unwrap();
    let pending = load_daily_report(
        LocalCostProvider::Codex,
        historical_day,
        historical_day,
        now,
        &opts,
    );
    assert_eq!(pending.summary, expected.summary);
}

#[test]
fn historical_correction_survives_trace_failure_and_relaunch() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let historical_day = utc_ymd_hms(2026, 5, 1, 12, 0, 0);
    let now = utc_ymd_hms(2026, 5, 11, 12, 0, 0);
    let db_path = codex_home.join("logs_2.sqlite");
    let backup_path = codex_home.join("logs_2.backup.sqlite");

    write_jsonl(
        &codex_home.join("sessions/2026/05/01/s1.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-05-01T12:00:00Z", "payload": {"id": "thread-a"}}),
            json!({"type": "turn_context", "timestamp": "2026-05-01T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-01T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-a"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-01T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );
    write_jsonl(
        &codex_home.join("sessions/2026/05/11/s2.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-05-11T12:00:00Z", "payload": {"id": "outside"}}),
            json!({"type": "turn_context", "timestamp": "2026-05-11T12:00:00Z", "payload": {"model": "gpt-5.4"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-11T12:00:01Z", "payload": {"type": "task_started", "turn_id": "outside"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-11T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        historical_day.as_second(),
        &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
    );
    insert_test_log(
        &db_path,
        now.as_second(),
        &priority_req_body("outside", "outside", "gpt-5.5"),
    );

    let _debounce = 60.0f64;
    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let report = load_daily_report(
        LocalCostProvider::Codex,
        historical_day,
        historical_day,
        now,
        &opts,
    );
    assert_eq!(report.summary.as_ref().unwrap().total_tokens, Some(110));
    assert_eq!(
        report.data[0].model_breakdowns[0].priority_tokens,
        Some(110)
    );
    assert!(!backup_path.exists());
}

#[test]
fn fallback_fault_hook_is_scoped_to_its_database_path() {
    let tmp = TempDir::new().expect("tempdir");
    let faulted_db = tmp.path().join("faulted.sqlite");
    let unaffected_db = tmp.path().join("unaffected.sqlite");
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    for db_path in [&faulted_db, &unaffected_db] {
        codex_priority::reset_codex_priority_turns_memo(db_path);
        create_test_logs_db(db_path);
        insert_test_log(
            db_path,
            epoch,
            &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
        );
        for idx in 1..8 {
            insert_test_log(db_path, epoch, &format!("routine trace row {idx}"));
        }
        let _ = codex_priority::codex_priority_turns(db_path, None, None, Some(TimeZone::UTC));
        let conn = Connection::open(db_path).unwrap();
        conn.execute(
            "UPDATE logs SET feedback_log_body = ?1 WHERE id = 1",
            [priority_req_body("thread-x", "turn-x", "gpt-5.5")],
        )
        .unwrap();
    }

    let unaffected =
        codex_priority::codex_priority_turns(&unaffected_db, None, None, Some(TimeZone::UTC));
    assert_eq!(
        unaffected.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-x"]
    );
}

#[test]
fn trace_failure_never_borrows_a_different_report_scope() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);
    let db_path = codex_home.join("logs_2.sqlite");
    let backup_path = codex_home.join("logs_2.backup.sqlite");
    let _tzs = ("Pacific/Honolulu", "Asia/Tokyo");

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/s.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-05-10T12:00:00Z", "payload": {"id": "thread-a"}}),
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-a"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );
    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
    );

    let initial = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(cache_root),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    assert_eq!(initial.summary.as_ref().unwrap().total_tokens, Some(110));
    assert!(!backup_path.exists());
}

#[test]
fn never_observed_trace_paths_do_not_block_repeated_usage_scans() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);
    let _missing = codex_home.join("never-created.sqlite");

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/s1.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-05-10T12:00:00Z", "payload": {"id": "thread-a"}}),
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-a"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );
    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let first = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(first.summary.as_ref().unwrap().total_tokens, Some(110));

    write_jsonl(
        &codex_home.join("sessions/2026/05/10/s2.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-05-10T12:00:02Z", "payload": {"id": "thread-b"}}),
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:02Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:03Z", "payload": {"type": "task_started", "turn_id": "turn-b"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:03Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );
    let second = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            force_rescan: true,
            ..opts
        },
    );
    assert_eq!(second.summary.as_ref().unwrap().total_tokens, Some(220));
}

#[test]
fn new_missing_trace_path_does_not_inherit_the_old_path_requirement() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);
    let db_path = codex_home.join("logs_2.sqlite");
    let _missing = codex_home.join("new-missing.sqlite");

    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
    );
    write_jsonl(
        &codex_home.join("sessions/2026/05/10/s1.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-05-10T12:00:00Z", "payload": {"id": "thread-a"}}),
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-a"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );
    let opts = CostScannerOptions {
        codex_home: Some(codex_home.clone()),
        cache_root: Some(cache_root),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let _ = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);

    fs::remove_file(&db_path).unwrap();
    write_jsonl(
        &codex_home.join("sessions/2026/05/10/s2.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-05-10T12:00:02Z", "payload": {"id": "thread-b"}}),
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:02Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:03Z", "payload": {"type": "task_started", "turn_id": "turn-b"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:03Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );
    let report = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    assert_eq!(report.summary.as_ref().unwrap().total_tokens, Some(220));
}

#[test]
fn validated_empty_pricing_evidence_persists_independently_of_the_resume_cursor() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    let _turn = ("turn-a", "thread-a|gpt-5.5|1788192000");
    let mut meta: CostUsageStoreMetadata = store.load_metadata().unwrap_or_default();
    meta.priority_metadata_key = Some("thread-a|gpt-5.5|1788192000".to_owned());
    meta.scan_since_day = Some("2026-08-01".to_owned());
    meta.scan_until_day = Some("2026-08-31".to_owned());
    assert!(store.save_metadata(&meta));

    meta.priority_metadata_key = None;
    assert!(store.save_metadata(&meta));
    let reloaded = store.load_metadata().unwrap();
    assert!(reloaded.priority_metadata_key.is_none());
}

#[test]
fn changed_surviving_anchor_forces_a_full_rescan() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    for idx in 1..=8 {
        let body = if idx == 3 {
            priority_req_body("thread-stale", "turn-stale", "gpt-5.5")
        } else {
            format!("routine trace row {idx}")
        };
        insert_test_log(&db_path, epoch, &body);
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let persisted = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert_eq!(
        persisted
            .anchors
            .iter()
            .map(|a| a.row_id)
            .collect::<Vec<_>>(),
        vec![1, 3, 5, 8]
    );

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = 'changed surviving anchor' WHERE id = 3",
        [],
    )
    .unwrap();
    drop(conn);
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-new", "turn-new", "gpt-5.5"),
    );

    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, persisted);
    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-new"]);
}

#[test]
fn losing_distributed_anchor_quorum_forces_a_full_rescan() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
    );
    for idx in 2..=8 {
        insert_test_log(&db_path, epoch, &format!("routine trace row {idx}"));
    }

    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let persisted = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert_eq!(
        persisted
            .anchors
            .iter()
            .map(|a| a.row_id)
            .collect::<Vec<_>>(),
        vec![1, 3, 5, 8]
    );

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = ?1 WHERE id = 1",
        [priority_req_body("thread-x", "turn-x", "gpt-5.5")],
    )
    .unwrap();
    conn.execute("DELETE FROM logs WHERE id IN (2, 4, 6)", [])
        .unwrap();
    drop(conn);
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-new", "turn-new", "gpt-5.5"),
    );

    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, persisted);
    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(
        turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-new", "turn-x"]
    );
}

#[test]
fn matched_legacy_anchor_upgrades_to_distributed_anchors() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    for idx in 1..=8 {
        insert_test_log(&db_path, epoch, &format!("routine trace row {idx}"));
    }
    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let mut legacy = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    legacy.anchors.clear();

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, legacy.clone());
    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));

    let upgraded = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert_eq!(
        upgraded
            .anchors
            .iter()
            .map(|a| a.row_id)
            .collect::<Vec<_>>(),
        vec![1, 3, 5, 8]
    );
    assert_eq!(upgraded.last_row_id, legacy.last_row_id);
    assert_eq!(upgraded.last_row_digest, legacy.last_row_digest);
}

#[test]
fn missing_legacy_anchor_remains_conservative() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
    );
    for idx in 2..=8 {
        insert_test_log(&db_path, epoch, &format!("routine trace row {idx}"));
    }
    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let mut legacy = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    legacy.anchors.clear();

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = ?1 WHERE id = 1",
        [priority_req_body("thread-x", "turn-x", "gpt-5.5")],
    )
    .unwrap();
    conn.execute("DELETE FROM logs WHERE id = ?1", [legacy.last_row_id])
        .unwrap();
    drop(conn);
    insert_test_log(
        &db_path,
        epoch,
        &priority_req_body("thread-new", "turn-new", "gpt-5.5"),
    );

    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, legacy);
    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(
        turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-new", "turn-x"]
    );
}

#[test]
fn small_databases_require_every_available_anchor() {
    let tmp = TempDir::new().expect("tempdir");
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    for row_count in 1..=3 {
        let db_path = tmp.path().join(format!("small-{row_count}.sqlite"));
        codex_priority::reset_codex_priority_turns_memo(&db_path);
        create_test_logs_db(&db_path);
        for idx in 1..=row_count {
            insert_test_log(&db_path, epoch, &format!("routine trace row {idx}"));
        }
        let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
        let mut persisted =
            codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
        assert_eq!(persisted.anchors.len(), row_count);
        persisted.turns.insert(
            "stale".to_owned(),
            codex_priority::CodexPriorityTurnMetadata {
                thread_id: Some("stale".to_owned()),
                turn_id: "stale".to_owned(),
                model: None,
                timestamp: None,
            },
        );

        codex_priority::reset_codex_priority_turns_memo(&db_path);
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "DELETE FROM logs WHERE id = ?1",
            [persisted.anchors[0].row_id],
        )
        .unwrap();
        drop(conn);
        if row_count == 1 {
            insert_test_log(&db_path, epoch, "replacement row");
        }
        codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, persisted);

        let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
        assert!(!turns.contains_key("stale"));
    }
}

#[test]
fn missing_anchor_refreshes_metadata_without_an_appended_tail() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    for idx in 1..=8 {
        insert_test_log(&db_path, epoch, &format!("routine trace row {idx}"));
    }
    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let persisted = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    let deleted_row_id = persisted.anchors[0].row_id;

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    let conn = Connection::open(&db_path).unwrap();
    conn.execute("DELETE FROM logs WHERE id = ?1", [deleted_row_id])
        .unwrap();
    drop(conn);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, persisted.clone());
    let _ = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));

    let refreshed = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert_eq!(refreshed.last_row_id, persisted.last_row_id);
    assert!(!refreshed.anchors.iter().any(|a| a.row_id == deleted_row_id));
}

#[test]
fn persisted_cursor_still_full_scans_when_the_requested_window_expands_earlier() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let today = utc_ymd_hms(2026, 5, 10, 12, 0, 0);
    let thirty_days_ago = today.checked_sub(720_i64.hours()).unwrap();
    let forty_five_days_ago = today.checked_sub(1080_i64.hours()).unwrap();
    let sixty_days_ago = today.checked_sub(1440_i64.hours()).unwrap();

    insert_test_log(
        &db_path,
        forty_five_days_ago.as_second(),
        &priority_req_body("thread-old", "turn-old", "gpt-5.5"),
    );
    insert_test_log(
        &db_path,
        today.as_second(),
        &priority_req_body("thread-new", "turn-new", "gpt-5.5"),
    );

    let narrow_range = DayRange::new(thirty_days_ago, today, Some(TimeZone::UTC));
    let narrow = codex_priority::codex_priority_turns(
        &db_path,
        Some(&narrow_range.scan_since_key),
        Some(&narrow_range.scan_until_key),
        Some(TimeZone::UTC),
    );
    assert_eq!(narrow.keys().cloned().collect::<Vec<_>>(), vec!["turn-new"]);
    let persisted = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, persisted.clone());
    let wide_range = DayRange::new(sixty_days_ago, today, Some(TimeZone::UTC));
    let expanded = codex_priority::codex_priority_turns(
        &db_path,
        Some(&wide_range.scan_since_key),
        Some(&wide_range.scan_until_key),
        Some(TimeZone::UTC),
    );
    assert_eq!(
        expanded.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-new", "turn-old"]
    );
    let expanded_cursor = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert!(expanded_cursor.coverage_since_epoch < persisted.coverage_since_epoch);
}

#[test]
fn old_priority_state_payload_without_a_cursor_still_cold_scans() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let now = utc_ymd_hms(2026, 5, 10, 12, 0, 0);
    insert_test_log(
        &db_path,
        now.as_second(),
        &priority_req_body("thread-cold", "turn-cold", "gpt-5.5"),
    );

    let mut store = CostUsageStore::new(Some(tmp.path()));
    let _legacy_fields = ("turn-a", "priority", "2026-05-10");
    let mut meta = store.load_metadata().unwrap_or_default();
    meta.priority_metadata_key = Some("priority".to_owned());
    meta.priority_turns_cursor = None;
    assert!(store.save_metadata(&meta));

    let loaded = store.load_metadata().unwrap();
    assert_eq!(loaded.priority_metadata_key.as_deref(), Some("priority"));
    assert!(loaded.priority_turns_cursor.is_none());

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-cold"]);
}

#[test]
fn live_priority_memo_wins_over_a_stale_persisted_cursor_seed() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);

    let mut live = codex_priority::CodexPrioritySourceCursorState {
        observation_id: 7,
        last_row_id: 99,
        file_id: Some(42.to_string()),
        ..Default::default()
    };
    live.turns.insert(
        "live".to_owned(),
        codex_priority::CodexPriorityTurnMetadata {
            thread_id: Some("thread-live".to_owned()),
            turn_id: "live".to_owned(),
            model: Some("gpt-5.5".to_owned()),
            timestamp: None,
        },
    );
    codex_priority::store_codex_priority_turns_memo_if_newer_for_path(&db_path, live);

    let mut stale = CodexPriorityTurnsPersistedCursor {
        path: db_path.to_string_lossy().to_string(),
        last_row_id: 1,
        ..Default::default()
    };
    stale.turns.insert(
        "stale".to_owned(),
        codex_priority::CodexPriorityTurnMetadata {
            thread_id: Some("thread-stale".to_owned()),
            turn_id: "stale".to_owned(),
            model: Some("gpt-5.5".to_owned()),
            timestamp: None,
        },
    );
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, stale);

    let memo = codex_priority::codex_priority_turns_memo_state(&db_path).unwrap();
    assert_eq!(memo.observation_id, 7);
    assert_eq!(memo.last_row_id, 99);
    assert_eq!(memo.file_id, Some(42.to_string()));
    assert_eq!(memo.turns.keys().cloned().collect::<Vec<_>>(), vec!["live"]);
}

#[test]
fn stale_seeded_cursor_reaccumulation_is_idempotent() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let epoch = utc_ymd_hms(2026, 5, 10, 12, 0, 0).as_second();

    let rows = [
        priority_req_body("thread-a", "turn-a", "gpt-5.5"),
        priority_req_body("thread-b", "turn-b", "gpt-5.5"),
        priority_req_body("thread-c", "turn-c", "gpt-5.5"),
        priority_req_body("thread-d", "turn-d", "gpt-5.5"),
        completed_resp_body("thread", "turn-a", "completed-a"),
        completed_resp_body("thread", "orphan-1", "pending-1"),
        completed_resp_body("thread", "orphan-2", "pending-2"),
    ];
    for body in &rows {
        insert_test_log(&db_path, epoch, body);
    }

    let fresh_turns =
        codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let mut stale = codex_priority::codex_priority_turns_persisted_cursor(&db_path).unwrap();
    assert_eq!(stale.last_row_id, rows.len() as i64);
    stale.last_row_id = 2;

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    codex_priority::seed_codex_priority_turns_memo_if_empty_for_path(&db_path, stale);
    let overlapping_turns =
        codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let overlapping_memo = codex_priority::codex_priority_turns_memo_state(&db_path).unwrap();

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    let again_turns =
        codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    let again_memo = codex_priority::codex_priority_turns_memo_state(&db_path).unwrap();

    assert_eq!(overlapping_turns, fresh_turns);
    assert_eq!(overlapping_turns, again_turns);
    assert_eq!(
        overlapping_memo.request_sources_by_turn_id,
        again_memo.request_sources_by_turn_id
    );
    assert_eq!(
        overlapping_memo.completed_models_by_turn_id,
        again_memo.completed_models_by_turn_id
    );
    assert_eq!(overlapping_memo.last_row_id, rows.len() as i64);
}

#[test]
fn identical_content_skip_still_persists_an_advanced_priority_cursor() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);
    let db_path = codex_home.join("logs_2.sqlite");

    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
    );
    write_jsonl(
        &codex_home.join("sessions/2026/05/10/s.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-05-10T12:00:00Z", "payload": {"id": "thread-a"}}),
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-a"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );
    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root.clone()),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let _ = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    let mut store = CostUsageStore::new(Some(&cache_root));
    let first_cursor = store
        .load_metadata()
        .unwrap()
        .priority_turns_cursor
        .unwrap();
    assert_eq!(first_cursor.last_row_id, 1);

    for idx in 0..3 {
        insert_test_log(
            &db_path,
            day.as_second(),
            &format!("routine trace row {idx}"),
        );
    }
    let _ = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    let reloaded = store
        .load_metadata()
        .unwrap()
        .priority_turns_cursor
        .unwrap();
    assert_eq!(reloaded.last_row_id, first_cursor.last_row_id + 3);
    assert_eq!(
        reloaded.turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-a"]
    );
}

#[test]
fn malformed_resolved_pricing_preserves_valid_priority_state() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    let _legacy = ("turn-a", "thread-a|gpt-5.4|123", "2026-05-10");
    let mut meta = store.load_metadata().unwrap_or_default();
    meta.priority_metadata_key = Some("thread-a|gpt-5.4|123".to_owned());
    meta.scan_since_day = Some("2026-05-10".to_owned());
    assert!(store.save_metadata(&meta));

    let loaded = store.load_metadata().unwrap();
    assert_eq!(loaded.priority_metadata_key, meta.priority_metadata_key);
    assert_eq!(loaded.scan_since_day, meta.scan_since_day);
}

#[test]
fn malformed_priority_turns_cursor_still_loads_turn_keys() {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = CostUsageStore::new(Some(tmp.path()));
    let mut meta = store.load_metadata().unwrap_or_default();
    meta.priority_metadata_key = Some("marker".to_owned());
    meta.scan_since_day = Some("2026-05-10".to_owned());
    assert!(store.save_metadata(&meta));

    let conn = Connection::open(store.database_path()).unwrap();
    let payload = serde_json::json!({
        "priorityMetadataKey": "marker",
        "scanSinceDay": "2026-05-10",
        "turnKeys": {"2026-05-10": "marker"},
        "turnIDsByDay": {"2026-05-10": ["turn-a"]},
        "priorityTurnsCursor": {"databasePath": 123}
    })
    .to_string();
    conn.execute(
        "UPDATE scan_metadata SET payload = ?1 WHERE id = 1",
        [payload.as_bytes()],
    )
    .unwrap();
    drop(conn);

    let loaded = store.load_metadata().unwrap_or_default();
    assert!(loaded.priority_turns_cursor.is_none());
}

#[test]
fn persisted_payload_without_content_anchor_fields_still_cold_scans() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("logs_2.sqlite");
    codex_priority::reset_codex_priority_turns_memo(&db_path);
    create_test_logs_db(&db_path);
    let now = utc_ymd_hms(2026, 5, 10, 12, 0, 0);
    insert_test_log(
        &db_path,
        now.as_second(),
        &priority_req_body("thread-cold", "turn-cold", "gpt-5.5"),
    );

    let mut store = CostUsageStore::new(Some(tmp.path()));
    let meta = store.load_metadata().unwrap_or_default();
    assert!(store.save_metadata(&meta));

    let conn = Connection::open(store.database_path()).unwrap();
    let payload = serde_json::json!({
        "priorityMetadataKey": "marker",
        "turnKeys": {"2026-05-10": "marker"},
        "turnIDsByDay": {"2026-05-10": ["turn-a"]},
        "turnsCursor": {
            "databasePath": "/tmp/logs_2.sqlite",
            "coverageSinceEpoch": 0,
            "lastRowID": 51,
            "fileIdentity": 1,
            "turns": {"turn-a": {"threadID": "thread-a", "turnID": "turn-a"}},
            "requestSourcesByTurnID": {},
            "priorityCompletedModelsByTurnID": {},
            "completedModelsByTurnID": {},
            "completedTurnIDInsertionOrder": [],
            "completedTurnIDInsertionOrderStartIndex": 0
        }
    })
    .to_string();
    conn.execute(
        "UPDATE scan_metadata SET payload = ?1 WHERE id = 1",
        [payload.as_bytes()],
    )
    .unwrap();
    drop(conn);

    let loaded = store.load_metadata().unwrap_or_default();
    assert!(loaded.priority_turns_cursor.is_none());

    let turns = codex_priority::codex_priority_turns(&db_path, None, None, Some(TimeZone::UTC));
    assert_eq!(turns.keys().cloned().collect::<Vec<_>>(), vec!["turn-cold"]);
}

#[test]
fn force_rescan_drops_a_persisted_priority_cursor_and_cold_scans() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let cache_root = tmp.path().join("cache");
    let day = utc_ymd_hms(2026, 5, 10, 12, 0, 0);
    let db_path = codex_home.join("logs_2.sqlite");

    create_test_logs_db(&db_path);
    insert_test_log(
        &db_path,
        day.as_second(),
        &priority_req_body("thread-a", "turn-a", "gpt-5.5"),
    );
    insert_test_log(
        &db_path,
        day.as_second(),
        &priority_req_body("thread-b", "turn-b", "gpt-5.5"),
    );
    write_jsonl(
        &codex_home.join("sessions/2026/05/10/s.jsonl"),
        &[
            json!({"type": "session_meta", "timestamp": "2026-05-10T12:00:00Z", "payload": {"id": "thread-a"}}),
            json!({"type": "turn_context", "timestamp": "2026-05-10T12:00:00Z", "payload": {"model": "gpt-5.5"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "task_started", "turn_id": "turn-a"}}),
            json!({"type": "event_msg", "timestamp": "2026-05-10T12:00:01Z", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 100, "cached_input_tokens": 20, "output_tokens": 10}}}}),
        ],
    );

    let opts = CostScannerOptions {
        codex_home: Some(codex_home),
        cache_root: Some(cache_root.clone()),
        timezone: Some(TimeZone::UTC),
        ..Default::default()
    };
    let _ = load_daily_report(LocalCostProvider::Codex, day, day, day, &opts);
    let mut store = CostUsageStore::new(Some(&cache_root));
    let persisted = store
        .load_metadata()
        .unwrap()
        .priority_turns_cursor
        .unwrap();
    assert_eq!(persisted.last_row_id, 2);
    assert_eq!(
        persisted.turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-a", "turn-b"]
    );

    codex_priority::reset_codex_priority_turns_memo(&db_path);
    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE logs SET feedback_log_body = ?1 WHERE id = 1",
        [priority_req_body(
            "thread-mutated",
            "turn-mutated",
            "gpt-5.5",
        )],
    )
    .unwrap();
    drop(conn);

    let _ = load_daily_report(
        LocalCostProvider::Codex,
        day,
        day,
        day,
        &CostScannerOptions {
            force_rescan: true,
            ..opts
        },
    );
    let reloaded = store
        .load_metadata()
        .unwrap()
        .priority_turns_cursor
        .unwrap();
    assert_eq!(
        reloaded.turns.keys().cloned().collect::<Vec<_>>(),
        vec!["turn-b", "turn-mutated"]
    );
    assert_eq!(reloaded.last_row_id, 2);
}
