// Ported from CodexBar Tests/CodexBarTests/CostUsageScannerTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsageScannerCodexPriorityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CostUsageCodexRequestLedgerTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
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
    CodexParseResult, CodexScanOptions, CodexSessionMetadata, CodexSubagentCounterSemantics,
    CodexSubagentObservation, CodexSubagentRolloutShape, CodexTotals, CodexUsageRow,
};
use ariadusage_engine::cost::codex_priority;
use ariadusage_engine::cost::pricing;
use ariadusage_engine::cost::timestamp::{BucketTimeZone, DayRange, utc_ymd_hms};
use ariadusage_engine::cost::{CostScannerOptions, LocalCostProvider, load_daily_report};
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
    assert_eq!(&bare_parsed.days["2026-08-21"]["unknown"][..3], &[10, 0, 4]);
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
    assert_eq!(&parsed.days["2026-08-21"]["unknown"][..3], &[12, 0, 4]);
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
    assert!((cost - 0.63).abs() < 1e-6);
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
    source_refresh_removes_copied_fork_prefix_without_losing_fast_split();
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
                "payload": {"type": "thread_settings_applied", "service_tier": "priority"}
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
    assert_eq!(parsed.rows[0].pricing_mode.as_deref(), Some("priority"));
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
