// Ported from CodexBar Tests/CodexBarTests/AntigravityLocalFixture.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityBotIDValidationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityLocalBotIDJoinTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityStepUUIDConsistencyTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/AntigravityJSONLNumberTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityLocalWALTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityLocalIntegrityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityLocalScanTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityLocalReaderTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use ariadusage_engine::providers::antigravity::{
    AntigravityJSONLObject, AntigravityJSONLValue, AntigravityLocalReader, AntigravityProtoReader,
    Budget, Context, CostProvenance, CostUsageCoverageCounts, CostUsageTokenMix,
    CostUsageTokenSnapshot, Coverage, DailyReportResult, Limits, ModelsDevCache, ModelsDevCatalog,
    SQLitePayload, ScanFailure, SourceResult,
};
use jiff::Timestamp;
use rusqlite::{Connection, OpenFlags};
use tempfile::{TempDir, tempdir};

// ============================================================================
// AntigravityLocalFixture.swift
// ============================================================================

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
}

impl Fixture {
    const NOW_SECS: i64 = 1_787_832_000; // 2026-08-27 12:00 UTC

    fn now() -> Timestamp {
        Timestamp::from_second(Self::NOW_SECS).unwrap()
    }

    fn new() -> Self {
        let temp = tempdir().unwrap();
        let root = temp.path().to_path_buf();
        Self { _temp: temp, root }
    }

    fn environment(&self) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        env.insert("HOME".to_owned(), self.root.to_string_lossy().into_owned());
        env
    }

    fn context(&self) -> Context {
        Context::new(self.environment(), Vec::new())
    }

    fn report(&self) -> DailyReportResult {
        self.report_with(None, Limits::default(), || 0.0, || Ok(()))
            .unwrap()
    }

    fn report_with_limits(&self, limits: Limits) -> DailyReportResult {
        self.report_with(None, limits, || 0.0, || Ok(())).unwrap()
    }

    fn report_with(
        &self,
        environment: Option<BTreeMap<String, String>>,
        limits: Limits,
        clock: impl Fn() -> f64 + Send + Sync + 'static,
        check_cancellation: impl Fn() -> Result<(), ScanFailure> + Send + Sync + 'static,
    ) -> Result<DailyReportResult, ScanFailure> {
        let ctx = Context::new(
            environment.unwrap_or_else(|| self.environment()),
            Vec::new(),
        );
        AntigravityLocalReader::make_daily_report_with_status(
            &ctx,
            0,
            false,
            None,
            limits,
            clock,
            check_cancellation,
        )
    }

    fn snapshot(&self) -> CostUsageTokenSnapshot {
        self.snapshot_with_env(None)
    }

    fn snapshot_with_env(
        &self,
        environment: Option<BTreeMap<String, String>>,
    ) -> CostUsageTokenSnapshot {
        let ctx = Context::new(
            environment.unwrap_or_else(|| self.environment()),
            Vec::new(),
        );
        let cache_root = self.root.join("scanner-cache");
        AntigravityLocalReader::load_token_snapshot(
            &ctx,
            Self::now(),
            30,
            0,
            Some(&cache_root),
            || Ok(()),
        )
        .unwrap()
    }

    fn database(&self, blobs: &[Vec<u8>]) -> PathBuf {
        self.database_full("session-a", 0, blobs, None)
    }

    fn database_with_steps(&self, blobs: &[Vec<u8>], step_blobs: &[Vec<u8>]) -> PathBuf {
        self.database_full("session-a", 0, blobs, Some(step_blobs))
    }

    fn database_at_root(&self, root_index: usize, blobs: &[Vec<u8>]) -> PathBuf {
        self.database_full("session-a", root_index, blobs, None)
    }

    fn database_named(&self, session: &str, root_index: usize, blobs: &[Vec<u8>]) -> PathBuf {
        self.database_full(session, root_index, blobs, None)
    }

    fn database_full(
        &self,
        session: &str,
        root_index: usize,
        blobs: &[Vec<u8>],
        step_blobs: Option<&[Vec<u8>]>,
    ) -> PathBuf {
        let roots = self.context().database_roots();
        let directory = &roots[root_index];
        fs::create_dir_all(directory).unwrap();
        let url = directory.join(format!("{session}.db"));
        let conn = Self::open(&url);
        Self::execute(
            &conn,
            "CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB)",
        );
        for (index, blob) in blobs.iter().enumerate() {
            Self::insert(&conn, index as i64, blob);
        }
        if let Some(steps) = step_blobs {
            Self::execute(
                &conn,
                "CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB)",
            );
            for (index, blob) in steps.iter().enumerate() {
                Self::insert_step(&conn, index as i64, blob);
            }
        }
        url
    }

    fn jsonl(&self, lines: &[&str]) -> PathBuf {
        self.jsonl_in(lines, "cache", None)
    }

    fn jsonl_strings(&self, lines: &[String], session: &str) -> PathBuf {
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        self.jsonl_in(&refs, session, None)
    }

    fn jsonl_in(&self, lines: &[&str], session: &str, root: Option<&Path>) -> PathBuf {
        let default_root = self.context().cache_root();
        let directory = root.unwrap_or(&default_root);
        fs::create_dir_all(directory).unwrap();
        let url = directory.join(format!("{session}.jsonl"));
        let mut text = lines.join("\n");
        text.push('\n');
        fs::write(&url, text).unwrap();
        url
    }

    fn open(path: &Path) -> Connection {
        Connection::open(path).unwrap()
    }

    fn execute(conn: &Connection, sql: &str) {
        conn.execute_batch(sql).unwrap();
    }

    fn insert(conn: &Connection, row: i64, blob: &[u8]) {
        conn.execute(
            "INSERT INTO gen_metadata (idx, data) VALUES (?1, ?2)",
            rusqlite::params![row, blob],
        )
        .unwrap();
    }

    fn insert_step(conn: &Connection, row: i64, blob: &[u8]) {
        conn.execute(
            "INSERT INTO steps (idx, metadata) VALUES (?1, ?2)",
            rusqlite::params![row, blob],
        )
        .unwrap();
    }

    fn varint(field: u64, value: u64) -> Vec<u8> {
        [Self::unsigned(field << 3), Self::unsigned(value)].concat()
    }

    fn message(field: u64, bytes: &[u8]) -> Vec<u8> {
        [
            Self::unsigned((field << 3) | 2),
            Self::unsigned(bytes.len() as u64),
            bytes.to_vec(),
        ]
        .concat()
    }

    fn unsigned(mut value: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        while value >= 128 {
            bytes.push(((value & 127) as u8) | 128);
            value >>= 7;
        }
        bytes.push(value as u8);
        bytes
    }

    fn blob() -> Vec<u8> {
        Self::blob_custom(
            Some("fixture-model-a"),
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        )
    }

    fn blob_response(response: &str) -> Vec<u8> {
        Self::blob_custom(
            Some("fixture-model-a"),
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            Some(response),
            Some(1_787_832_000),
        )
    }

    #[expect(clippy::too_many_arguments)]
    fn blob_custom(
        model: Option<&str>,
        label: Option<&str>,
        model_id: u64,
        input: u64,
        output: u64,
        cache_read: u64,
        reasoning: u64,
        response: Option<&str>,
        seconds: Option<u64>,
    ) -> Vec<u8> {
        let mut usage = [
            Self::varint(1, model_id),
            Self::varint(2, input),
            Self::varint(5, cache_read),
            Self::varint(9, reasoning),
            Self::varint(10, output),
        ]
        .concat();
        if let Some(resp) = response {
            usage.extend_from_slice(&Self::message(11, resp.as_bytes()));
        }
        let mut chat = Self::message(4, &usage);
        if let Some(secs) = seconds {
            let ts = [Self::varint(1, secs), Self::varint(2, 250_000_000)].concat();
            chat.extend_from_slice(&Self::message(9, &Self::message(4, &ts)));
        }
        if let Some(m) = model {
            chat.extend_from_slice(&Self::message(19, m.as_bytes()));
        }
        if let Some(l) = label {
            chat.extend_from_slice(&Self::message(21, l.as_bytes()));
        }
        Self::message(1, &chat)
    }

    #[expect(clippy::too_many_arguments)]
    fn blob_with_root_envelope(
        step_uuid: Option<&str>,
        bot_id: Option<&str>,
        model: Option<&str>,
        label: Option<&str>,
        model_id: u64,
        input: u64,
        output: u64,
        cache_read: u64,
        reasoning: u64,
        response: Option<&str>,
        seconds: Option<u64>,
    ) -> Vec<u8> {
        let mut root = Self::message(2, &[0x01, 0x02]);
        if let Some(uuid) = step_uuid {
            root.extend_from_slice(&Self::message(4, uuid.as_bytes()));
        }
        let mut usage = [
            Self::varint(1, model_id),
            Self::varint(2, input),
            Self::varint(5, cache_read),
            Self::varint(9, reasoning),
            Self::varint(10, output),
        ]
        .concat();
        if let Some(bid) = bot_id {
            usage.extend_from_slice(&Self::message(7, bid.as_bytes()));
        }
        if let Some(resp) = response {
            usage.extend_from_slice(&Self::message(11, resp.as_bytes()));
        }
        let mut chat = Self::message(4, &usage);
        if let Some(secs) = seconds {
            let ts = [Self::varint(1, secs), Self::varint(2, 250_000_000)].concat();
            chat.extend_from_slice(&Self::message(9, &Self::message(4, &ts)));
        }
        if let Some(m) = model {
            chat.extend_from_slice(&Self::message(19, m.as_bytes()));
        }
        if let Some(l) = label {
            chat.extend_from_slice(&Self::message(21, l.as_bytes()));
        }
        root.extend_from_slice(&Self::message(1, &chat));
        root
    }

    fn root_envelope_simple(
        step_uuid: Option<&str>,
        bot_id: Option<&str>,
        seconds: Option<u64>,
    ) -> Vec<u8> {
        Self::blob_with_root_envelope(
            step_uuid,
            bot_id,
            Some("fixture-model-a"),
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            None,
            seconds,
        )
    }

    fn step_metadata_blob(
        step_uuid: Option<&str>,
        bot_id: Option<&str>,
        seconds: u64,
        nanos: u64,
    ) -> Vec<u8> {
        let mut meta = Self::message(
            1,
            &[Self::varint(1, seconds), Self::varint(2, nanos)].concat(),
        );
        if let Some(bid) = bot_id {
            let usage = Self::message(7, bid.as_bytes());
            meta.extend_from_slice(&Self::message(9, &usage));
        }
        if let Some(uuid) = step_uuid {
            meta.extend_from_slice(&Self::message(12, uuid.as_bytes()));
        }
        meta
    }

    const CACHE_USAGE: &'static str = concat!(
        "{\"type\":\"usage\",\"sessionId\":\"cache-session\",\"input\":100,\"output\":30,\"cacheRead\":50,",
        "\"cacheWrite\":0,\"reasoning\":0,\"timestamp\":1787832000250,\"responseId\":null}"
    );
}

fn read_single_db(url: &Path) -> SourceResult {
    let budget = Budget::with_cancellation(Limits::default(), || Ok(()));
    AntigravityLocalReader::read_databases(&[url.to_path_buf()], &budget).unwrap()
}

// ============================================================================
// AntigravityBotIDValidationTests.swift
// ============================================================================

const EARLY_SECS: u64 = 1_787_875_140;
const LATE_SECS: u64 = 1_787_875_260;

// upstream: AntigravityBotIDValidationTests.swift:10 embedded generations also participate in bot ID uniqueness
#[test]
fn embedded_generations_also_participate_in_bot_id_uniqueness() {
    let fixture = Fixture::new();
    let url = fixture.database_with_steps(
        &[
            Fixture::root_envelope_simple(Some("step-uuid-1"), Some("shared"), Some(EARLY_SECS)),
            Fixture::root_envelope_simple(Some("step-uuid-1"), Some("shared"), None),
        ],
        &[Fixture::step_metadata_blob(
            Some("step-uuid-1"),
            Some("shared"),
            LATE_SECS,
            250_000_000,
        )],
    );
    let source = read_single_db(&url);
    assert!(!source.is_complete);
    assert_eq!(
        source.events.iter().map(|e| e.row).collect::<Vec<_>>(),
        vec![0]
    );
    assert_eq!(
        source
            .events
            .iter()
            .map(|e| e.turn.timestamp_ms)
            .collect::<Vec<_>>(),
        vec![Some(1_787_875_140_250)]
    );
}

// upstream: AntigravityBotIDValidationTests.swift:25 repeated generation IDs retain consistent positional evidence
#[test]
fn repeated_generation_ids_retain_consistent_positional_evidence() {
    for unrelated_uuid in [false, true] {
        let fixture = Fixture::new();
        let url = fixture.database_with_steps(
            &[
                Fixture::root_envelope_simple(
                    if unrelated_uuid {
                        None
                    } else {
                        Some("step-uuid-1")
                    },
                    Some("shared"),
                    Some(EARLY_SECS),
                ),
                Fixture::root_envelope_simple(Some("step-uuid-1"), Some("shared"), None),
            ],
            &[
                Fixture::step_metadata_blob(Some("step-uuid-1"), None, EARLY_SECS, 250_000_000),
                Fixture::step_metadata_blob(
                    Some("step-uuid-1"),
                    Some("shared"),
                    LATE_SECS,
                    250_000_000,
                ),
            ],
        );
        let source = read_single_db(&url);
        assert!(source.is_complete);
        assert_eq!(
            source
                .events
                .iter()
                .map(|e| e.turn.timestamp_ms)
                .collect::<Vec<_>>(),
            vec![
                Some(1_787_875_140_250),
                Some(if unrelated_uuid {
                    1_787_875_140_250
                } else {
                    1_787_875_260_250
                }),
            ]
        );
    }
}

// upstream: AntigravityBotIDValidationTests.swift:45 exact step evidence must agree with embedded timestamps without requiring positional order
#[test]
fn exact_step_evidence_must_agree_with_embedded_timestamps_without_requiring_positional_order() {
    for conflicting in [false, true] {
        let fixture = Fixture::new();
        let url = fixture.database_with_steps(
            &[
                Fixture::root_envelope_simple(
                    Some("step-uuid-1"),
                    Some("embedded"),
                    Some(EARLY_SECS),
                ),
                Fixture::root_envelope_simple(Some("step-uuid-1"), Some("pending"), None),
            ],
            &[
                Fixture::step_metadata_blob(
                    Some("step-uuid-1"),
                    Some("pending"),
                    LATE_SECS,
                    250_000_000,
                ),
                Fixture::step_metadata_blob(
                    Some("step-uuid-1"),
                    Some("embedded"),
                    if conflicting { LATE_SECS } else { EARLY_SECS },
                    250_000_000,
                ),
            ],
        );
        let source = read_single_db(&url);
        assert_eq!(source.is_complete, !conflicting);
        assert_eq!(
            source.events.first().and_then(|e| e.turn.timestamp_ms),
            Some(1_787_875_140_250)
        );
        if conflicting {
            assert_eq!(
                source.events.iter().map(|e| e.row).collect::<Vec<_>>(),
                vec![0]
            );
        } else {
            assert_eq!(
                source.events.iter().map(|e| e.row).collect::<Vec<_>>(),
                vec![0, 1]
            );
            assert_eq!(
                source
                    .events
                    .iter()
                    .map(|e| e.turn.timestamp_ms)
                    .collect::<Vec<_>>(),
                vec![Some(1_787_875_140_250), Some(1_787_875_260_250)]
            );
        }
    }
}

// upstream: AntigravityBotIDValidationTests.swift:70 embedded timestamps without exact identity evidence do not block reordered exact recovery
#[test]
fn embedded_timestamps_without_exact_identity_evidence_do_not_block_reordered_exact_recovery() {
    for has_unmatched_id in [false, true] {
        let fixture = Fixture::new();
        let url = fixture.database_with_steps(
            &[
                Fixture::root_envelope_simple(
                    Some("step-uuid-1"),
                    if has_unmatched_id {
                        Some("absent")
                    } else {
                        None
                    },
                    Some(EARLY_SECS),
                ),
                Fixture::root_envelope_simple(Some("step-uuid-1"), Some("pending"), None),
            ],
            &[
                Fixture::step_metadata_blob(
                    Some("step-uuid-1"),
                    Some("pending"),
                    LATE_SECS,
                    250_000_000,
                ),
                Fixture::step_metadata_blob(Some("step-uuid-1"), None, EARLY_SECS, 250_000_000),
            ],
        );
        let source = read_single_db(&url);
        assert!(source.is_complete);
        assert_eq!(
            source
                .events
                .iter()
                .map(|e| e.turn.timestamp_ms)
                .collect::<Vec<_>>(),
            vec![Some(1_787_875_140_250), Some(1_787_875_260_250)]
        );
    }
}

// upstream: AntigravityBotIDValidationTests.swift:89 an unrelated UUID retains embedded timestamp precedence during exact recovery
#[test]
fn an_unrelated_uuid_retains_embedded_timestamp_precedence_during_exact_recovery() {
    let fixture = Fixture::new();
    let url = fixture.database_with_steps(
        &[
            Fixture::root_envelope_simple(Some("complete"), Some("embedded"), Some(EARLY_SECS)),
            Fixture::root_envelope_simple(Some("recover"), Some("pending"), None),
        ],
        &[
            Fixture::step_metadata_blob(Some("complete"), Some("embedded"), LATE_SECS, 250_000_000),
            Fixture::step_metadata_blob(Some("recover"), Some("pending"), LATE_SECS, 250_000_000),
        ],
    );
    let source = read_single_db(&url);
    assert!(source.is_complete);
    assert_eq!(
        source
            .events
            .iter()
            .map(|e| e.turn.timestamp_ms)
            .collect::<Vec<_>>(),
        vec![Some(1_787_875_140_250), Some(1_787_875_260_250)]
    );
}

// upstream: AntigravityBotIDValidationTests.swift:106 ambiguous steps cannot compress positions or activate single timestamp sharing
#[test]
fn ambiguous_steps_cannot_compress_positions_or_activate_single_timestamp_sharing() {
    for embedded in [false, true] {
        let fixture = Fixture::new();
        let mut steps = vec![
            Fixture::step_metadata_blob(
                Some("step-uuid-1"),
                Some("conflict"),
                EARLY_SECS - 20,
                250_000_000,
            ),
            Fixture::step_metadata_blob(
                Some("step-uuid-1"),
                Some("conflict"),
                EARLY_SECS - 10,
                250_000_000,
            ),
        ];
        if embedded {
            steps.push(Fixture::step_metadata_blob(
                Some("step-uuid-1"),
                None,
                EARLY_SECS,
                250_000_000,
            ));
        }
        steps.push(Fixture::step_metadata_blob(
            Some("step-uuid-1"),
            None,
            LATE_SECS,
            250_000_000,
        ));
        let url = fixture.database_with_steps(
            &[
                Fixture::root_envelope_simple(
                    Some("step-uuid-1"),
                    None,
                    if embedded { Some(EARLY_SECS) } else { None },
                ),
                Fixture::root_envelope_simple(Some("step-uuid-1"), None, None),
            ],
            &steps,
        );
        let source = read_single_db(&url);
        assert!(!source.is_complete);
        assert_eq!(
            source.events.iter().map(|e| e.row).collect::<Vec<_>>(),
            if embedded { vec![0] } else { Vec::new() }
        );
    }
}

// upstream: AntigravityBotIDValidationTests.swift:128 an exact ID owned by another UUID cannot fall back to positional evidence
#[test]
fn an_exact_id_owned_by_another_uuid_cannot_fall_back_to_positional_evidence() {
    let fixture = Fixture::new();
    let url = fixture.database_with_steps(
        &[Fixture::root_envelope_simple(
            Some("local"),
            Some("owned-elsewhere"),
            None,
        )],
        &[
            Fixture::step_metadata_blob(Some("local"), None, EARLY_SECS, 250_000_000),
            Fixture::step_metadata_blob(
                Some("other"),
                Some("owned-elsewhere"),
                LATE_SECS,
                250_000_000,
            ),
        ],
    );
    let source = read_single_db(&url);
    assert!(!source.is_complete);
    assert!(source.events.is_empty());
}

// upstream: AntigravityBotIDValidationTests.swift:144 timestamp less duplicate step IDs invalidate exact evidence in either order
#[test]
fn timestamp_less_duplicate_step_ids_invalidate_exact_evidence_in_either_order() {
    for cross_uuid in [false, true] {
        for missing_first in [false, true] {
            let fixture = Fixture::new();
            let valid = Fixture::step_metadata_blob(
                Some("local"),
                Some("duplicate"),
                EARLY_SECS,
                250_000_000,
            );
            let missing = [
                Fixture::message(9, &Fixture::message(7, b"duplicate")),
                Fixture::message(12, if cross_uuid { b"other" } else { b"local" }),
            ]
            .concat();
            let steps = if missing_first {
                vec![missing, valid]
            } else {
                vec![valid, missing]
            };
            let url = fixture.database_with_steps(
                &[Fixture::root_envelope_simple(
                    Some("local"),
                    Some("duplicate"),
                    None,
                )],
                &steps,
            );
            let source = read_single_db(&url);
            assert!(!source.is_complete);
            assert!(source.events.is_empty());
        }
    }
}

// upstream: AntigravityBotIDValidationTests.swift:175 invalid repeated step IDs stay invalid across envelopes
#[test]
fn invalid_repeated_step_ids_stay_invalid_across_envelopes() {
    let invalid_fields = [Fixture::message(7, &[0xFF]), Fixture::varint(7, 1)];
    for field in invalid_fields {
        for invalid_first in [false, true] {
            let valid = Fixture::message(9, &Fixture::message(7, b"later-key"));
            let malformed = Fixture::message(9, &field);
            let mut bytes =
                Fixture::step_metadata_blob(Some("step-uuid-1"), None, EARLY_SECS, 250_000_000);
            if invalid_first {
                bytes.extend_from_slice(&malformed);
                bytes.extend_from_slice(&valid);
            } else {
                bytes.extend_from_slice(&valid);
                bytes.extend_from_slice(&malformed);
            }
            let parsed =
                AntigravityProtoReader::parse_step_metadata(&bytes, || Ok::<(), ScanFailure>(()))
                    .unwrap()
                    .unwrap();
            assert_eq!(parsed.bot_id, None);
            assert_eq!(parsed.timestamp_ms, Some(1_787_875_140_250));
        }
    }
}

// upstream: AntigravityBotIDValidationTests.swift:188 a malformed step envelope cannot be repaired by a later valid ID
#[test]
fn a_malformed_step_envelope_cannot_be_repaired_by_a_later_valid_id() {
    for invalid_first in [false, true] {
        let valid = Fixture::message(9, &Fixture::message(7, b"later-key"));
        let malformed = Fixture::message(9, &[0x3A, 0x02, 0x61]);
        let mut bytes =
            Fixture::step_metadata_blob(Some("step-uuid-1"), None, EARLY_SECS, 250_000_000);
        if invalid_first {
            bytes.extend_from_slice(&malformed);
            bytes.extend_from_slice(&valid);
        } else {
            bytes.extend_from_slice(&valid);
            bytes.extend_from_slice(&malformed);
        }
        let parsed =
            AntigravityProtoReader::parse_step_metadata(&bytes, || Ok::<(), ScanFailure>(()))
                .unwrap()
                .unwrap();
        assert_eq!(parsed.bot_id, None);
        assert_eq!(parsed.timestamp_ms, Some(1_787_875_140_250));
    }
}

// upstream: AntigravityBotIDValidationTests.swift:201 empty step ID scalars clear earlier values
#[test]
fn empty_step_id_scalars_clear_earlier_values() {
    for empty in ["", " \n"] {
        let mut bytes = Fixture::step_metadata_blob(
            Some("step-uuid-1"),
            Some("earlier"),
            EARLY_SECS,
            250_000_000,
        );
        bytes.extend_from_slice(&Fixture::message(9, &Fixture::message(7, empty.as_bytes())));
        let parsed =
            AntigravityProtoReader::parse_step_metadata(&bytes, || Ok::<(), ScanFailure>(()))
                .unwrap()
                .unwrap();
        assert_eq!(parsed.bot_id, None);
    }
}

// upstream: AntigravityBotIDValidationTests.swift:209 empty generation ID scalars clear earlier values
#[test]
fn empty_generation_id_scalars_clear_earlier_values() {
    for empty in ["", " \n"] {
        let mut bytes =
            Fixture::root_envelope_simple(Some("step-uuid-1"), Some("earlier"), Some(EARLY_SECS));
        bytes.extend_from_slice(&Fixture::message(
            1,
            &Fixture::message(4, &Fixture::message(7, empty.as_bytes())),
        ));
        let turn = AntigravityProtoReader::parse_turn(&bytes, || Ok::<(), ScanFailure>(()))
            .unwrap()
            .unwrap();
        assert_eq!(turn.usage.as_ref().and_then(|u| u.bot_id()), None);
        assert_eq!(turn.timestamp_ms, Some(1_787_875_140_250));
    }
}

// upstream: AntigravityBotIDValidationTests.swift:219 malformed generation IDs preserve valid usage and remain invalid across envelopes
#[test]
fn malformed_generation_ids_preserve_valid_usage_and_remain_invalid_across_envelopes() {
    let invalid_fields = [Fixture::message(7, &[0xFF]), Fixture::varint(7, 1)];
    for field in invalid_fields {
        for invalid_first in [false, true] {
            let valid =
                Fixture::message(1, &Fixture::message(4, &Fixture::message(7, b"later-key")));
            let malformed = Fixture::message(1, &Fixture::message(4, &field));
            let mut blob =
                Fixture::root_envelope_simple(Some("step-uuid-1"), None, Some(EARLY_SECS));
            if invalid_first {
                blob.extend_from_slice(&malformed);
                blob.extend_from_slice(&valid);
            } else {
                blob.extend_from_slice(&valid);
                blob.extend_from_slice(&malformed);
            }
            let fixture = Fixture::new();
            let url = fixture.database(&[blob]);
            let source = read_single_db(&url);
            let turn = &source.events.first().unwrap().turn;
            assert!(source.is_complete);
            assert_eq!(turn.timestamp_ms, Some(1_787_875_140_250));
            assert_eq!(turn.usage.as_ref().and_then(|u| u.bot_id()), None);
            assert_eq!(turn.usage.as_ref().map(|u| u.new_input), Some(100));
        }
    }
}

// upstream: AntigravityBotIDValidationTests.swift:240 optional ID tolerance does not relax counters or protobuf framing
#[test]
fn optional_id_tolerance_does_not_relax_counters_or_protobuf_framing() {
    let base = Fixture::root_envelope_simple(Some("step-uuid-1"), None, Some(EARLY_SECS));
    let invalid_counter = Fixture::message(1, &Fixture::message(4, &Fixture::message(2, &[1])));
    let truncated_usage = Fixture::message(1, &Fixture::message(4, &[0x3A, 0x02, 0x61]));

    let mut b1 = base.clone();
    b1.extend_from_slice(&invalid_counter);
    assert_eq!(
        AntigravityProtoReader::parse_turn(&b1, || Ok::<(), ScanFailure>(())).unwrap(),
        None
    );

    let mut b2 = base;
    b2.extend_from_slice(&truncated_usage);
    assert_eq!(
        AntigravityProtoReader::parse_turn(&b2, || Ok::<(), ScanFailure>(())).unwrap(),
        None
    );
}

// upstream: AntigravityBotIDValidationTests.swift:250 malformed optional IDs do not turn equivalent copies into conflicts
#[test]
fn malformed_optional_ids_do_not_turn_equivalent_copies_into_conflicts() {
    for malformed_first in [false, true] {
        let fixture = Fixture::new();
        let absent = Fixture::root_envelope_simple(Some("step-uuid-1"), None, Some(EARLY_SECS));
        let mut malformed = absent.clone();
        malformed.extend_from_slice(&Fixture::message(
            1,
            &Fixture::message(4, &Fixture::message(7, &[0xFF])),
        ));
        fixture.database_at_root(
            0,
            &[if malformed_first {
                malformed.clone()
            } else {
                absent.clone()
            }],
        );
        fixture.database_at_root(1, &[if malformed_first { absent } else { malformed }]);
        let report = fixture.report();
        assert_eq!(report.coverage, Coverage::Complete);
        assert_eq!(report.report.data.len(), 1);
        assert_eq!(report.report.data[0].total_tokens, Some(187));
    }
}

// ============================================================================
// AntigravityLocalBotIDJoinTests.swift
// ============================================================================

// upstream: AntigravityLocalBotIDJoinTests.swift:13 bot ID exact join recovers turns when step counts mismatch
#[test]
fn bot_id_exact_join_recovers_turns_when_step_counts_mismatch() {
    let fixture = Fixture::new();
    let step_uuid = "shared-session-uuid";
    let first = Fixture::root_envelope_simple(Some(step_uuid), Some("bot-aaa"), None);
    let second = Fixture::root_envelope_simple(Some(step_uuid), Some("bot-bbb"), None);
    let first_step =
        Fixture::step_metadata_blob(Some(step_uuid), Some("bot-aaa"), 1_787_875_140, 250_000_000);
    let extra_step = Fixture::step_metadata_blob(
        Some(step_uuid),
        Some("bot-inflight"),
        1_787_875_200,
        250_000_000,
    );
    let second_step =
        Fixture::step_metadata_blob(Some(step_uuid), Some("bot-bbb"), 1_787_875_260, 250_000_000);
    let url = fixture.database_with_steps(&[first, second], &[first_step, extra_step, second_step]);

    let source = read_single_db(&url);
    let report = fixture.report();

    assert_eq!(
        source.events.iter().map(|e| e.row).collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        source
            .events
            .iter()
            .map(|e| e.turn.timestamp_ms)
            .collect::<Vec<_>>(),
        vec![Some(1_787_875_140_250), Some(1_787_875_260_250)]
    );
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-27", "2026-08-28"]
    );
}

// upstream: AntigravityLocalBotIDJoinTests.swift:38 bot ID exact join ignores step idx order
#[test]
fn bot_id_exact_join_ignores_step_idx_order() {
    let fixture = Fixture::new();
    let step_uuid = "shared-session-uuid";
    let first = Fixture::root_envelope_simple(Some(step_uuid), Some("bot-aaa"), None);
    let second = Fixture::root_envelope_simple(Some(step_uuid), Some("bot-bbb"), None);
    let url = fixture.database(&[first, second]);
    let conn = Fixture::open(&url);
    Fixture::execute(&conn, "CREATE TABLE steps (idx INTEGER, metadata BLOB)");
    Fixture::insert_step(
        &conn,
        20,
        &Fixture::step_metadata_blob(Some(step_uuid), Some("bot-aaa"), 1_787_875_140, 250_000_000),
    );
    Fixture::insert_step(
        &conn,
        10,
        &Fixture::step_metadata_blob(Some(step_uuid), Some("bot-bbb"), 1_787_875_260, 250_000_000),
    );
    drop(conn);

    let source = read_single_db(&url);
    let report = fixture.report();

    assert_eq!(
        source.events.iter().map(|e| e.row).collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        source
            .events
            .iter()
            .map(|e| e.turn.timestamp_ms)
            .collect::<Vec<_>>(),
        vec![Some(1_787_875_140_250), Some(1_787_875_260_250)]
    );
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-27", "2026-08-28"]
    );
}

// upstream: AntigravityLocalBotIDJoinTests.swift:68 conflicting duplicate bot IDs fail closed
#[test]
fn conflicting_duplicate_bot_ids_fail_closed() {
    let fixture = Fixture::new();
    let step_uuid = "shared-session-uuid";
    let turn = Fixture::root_envelope_simple(Some(step_uuid), Some("bot-dup"), None);
    let first =
        Fixture::step_metadata_blob(Some(step_uuid), Some("bot-dup"), 1_787_875_140, 250_000_000);
    let second =
        Fixture::step_metadata_blob(Some(step_uuid), Some("bot-dup"), 1_787_875_260, 250_000_000);
    fixture.database_with_steps(&[turn], &[first, second]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert!(report.report.data.is_empty());
}

// upstream: AntigravityLocalBotIDJoinTests.swift:85 duplicate bot IDs with identical timestamps still recover
#[test]
fn duplicate_bot_ids_with_identical_timestamps_still_recover() {
    let fixture = Fixture::new();
    let step_uuid = "shared-session-uuid";
    let turn = Fixture::root_envelope_simple(Some(step_uuid), Some("bot-retry"), None);
    let first = Fixture::step_metadata_blob(
        Some(step_uuid),
        Some("bot-retry"),
        1_787_875_140,
        250_000_000,
    );
    let second = Fixture::step_metadata_blob(
        Some(step_uuid),
        Some("bot-retry"),
        1_787_875_140,
        250_000_000,
    );
    fixture.database_with_steps(&[turn], &[first, second]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-27"]
    );
}

// upstream: AntigravityLocalBotIDJoinTests.swift:103 embedded timestamp takes precedence over bot ID join
#[test]
fn embedded_timestamp_takes_precedence_over_bot_id_join() {
    let fixture = Fixture::new();
    let step_uuid = "legacy-step-uuid";
    let turn =
        Fixture::root_envelope_simple(Some(step_uuid), Some("bot-legacy"), Some(1_787_875_140));
    let step = Fixture::step_metadata_blob(
        Some(step_uuid),
        Some("bot-legacy"),
        1_787_875_260,
        250_000_000,
    );
    fixture.database_with_steps(&[turn], &[step]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-27"]
    );
}

// upstream: AntigravityLocalBotIDJoinTests.swift:119 bot ID without matching step falls back to positional recovery
#[test]
fn bot_id_without_matching_step_falls_back_to_positional_recovery() {
    let fixture = Fixture::new();
    let step_uuid = "mixed-write-step-uuid";
    let turn = Fixture::root_envelope_simple(Some(step_uuid), Some("bot-new"), None);
    let step = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_140, 250_000_000);
    fixture.database_with_steps(&[turn], &[step]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-27"]
    );
}

// upstream: AntigravityLocalBotIDJoinTests.swift:134 malformed step bot ID falls back without failing the scan
#[test]
fn malformed_step_bot_id_falls_back_without_failing_the_scan() {
    let fixture = Fixture::new();
    let step_uuid = "mixed-bot-step-uuid";
    let turn = Fixture::root_envelope_simple(Some(step_uuid), Some("bot-aaa"), None);
    let malformed = [
        Fixture::message(
            1,
            &[Fixture::varint(1, 1_787_875_140), Fixture::varint(2, 0)].concat(),
        ),
        Fixture::message(9, &Fixture::message(7, &[0xFF, 0xFE])),
        Fixture::message(12, step_uuid.as_bytes()),
    ]
    .concat();
    let step =
        Fixture::step_metadata_blob(Some(step_uuid), Some("bot-aaa"), 1_787_875_260, 250_000_000);
    fixture.database_with_steps(&[turn], &[malformed, step]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-28"]
    );
}

// upstream: AntigravityLocalBotIDJoinTests.swift:152 missing bot ID falls back to positional recovery
#[test]
fn missing_bot_id_falls_back_to_positional_recovery() {
    let fixture = Fixture::new();
    let step_uuid = "fallback-step-uuid";
    let turn = Fixture::root_envelope_simple(Some(step_uuid), None, None);
    let step = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_140, 250_000_000);
    fixture.database_with_steps(&[turn], &[step]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-27"]
    );
}

// upstream: AntigravityLocalBotIDJoinTests.swift:166 interactive session shape with content envelopes recovers via bot ID
#[test]
fn interactive_session_shape_with_content_envelopes_recovers_via_bot_id() {
    let fixture = Fixture::new();
    let step_uuid = "live-session-uuid";
    let bot_id = "bot-live-1";
    let usage = [
        Fixture::varint(1, 1318),
        Fixture::varint(2, 15669),
        Fixture::varint(9, 123),
        Fixture::varint(10, 27),
        Fixture::message(7, bot_id.as_bytes()),
    ]
    .concat();
    let mut chat = Fixture::message(1, &vec![0x61; 24775]);
    chat.extend_from_slice(&Fixture::message(2, &vec![0x62; 433]));
    chat.extend_from_slice(&Fixture::message(4, &usage));
    chat.extend_from_slice(&Fixture::message(
        9,
        &[
            Fixture::varint(2, u64::MAX),
            Fixture::message(10, &Fixture::varint(1, 255_523)),
        ]
        .concat(),
    ));
    chat.extend_from_slice(&Fixture::message(8, &vec![0x63; 2362]));
    let mut root = Fixture::message(2, &[0x01]);
    root.extend_from_slice(&Fixture::message(3, &vec![0x64; 3020]));
    root.extend_from_slice(&Fixture::message(4, step_uuid.as_bytes()));
    root.extend_from_slice(&Fixture::message(8, &vec![0x65; 424]));
    root.extend_from_slice(&Fixture::message(1, &chat));
    let other_step = [
        Fixture::message(
            1,
            &[Fixture::varint(1, 1_787_875_140), Fixture::varint(2, 0)].concat(),
        ),
        Fixture::varint(3, 4),
        Fixture::message(12, step_uuid.as_bytes()),
    ]
    .concat();
    let step =
        Fixture::step_metadata_blob(Some(step_uuid), Some(bot_id), 1_787_875_260, 250_000_000);
    fixture.database_with_steps(&[root], &[other_step, step]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-28"]
    );
}

// upstream: AntigravityLocalBotIDJoinTests.swift:200 structurally malformed step bot ID keeps row positionally usable
#[test]
fn structurally_malformed_step_bot_id_keeps_row_positionally_usable() {
    let fixture = Fixture::new();
    let step_uuid = "mixed-shape-step-uuid";
    let turn = Fixture::root_envelope_simple(Some(step_uuid), Some("bot-aaa"), None);
    let malformed = [
        Fixture::message(
            1,
            &[Fixture::varint(1, 1_787_875_140), Fixture::varint(2, 0)].concat(),
        ),
        Fixture::varint(9, 1),
        Fixture::message(12, step_uuid.as_bytes()),
    ]
    .concat();
    let step =
        Fixture::step_metadata_blob(Some(step_uuid), Some("bot-aaa"), 1_787_875_260, 250_000_000);
    fixture.database_with_steps(&[turn], &[malformed, step]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-28"]
    );
}

// upstream: AntigravityLocalBotIDJoinTests.swift:218 repeated generation bot ID skips exact matching but keeps positional guard
#[test]
fn repeated_generation_bot_id_skips_exact_matching_but_keeps_positional_guard() {
    let fixture = Fixture::new();
    let step_uuid = "reused-bot-step-uuid";
    let first = Fixture::root_envelope_simple(Some(step_uuid), Some("bot-dup"), None);
    let second = Fixture::root_envelope_simple(Some(step_uuid), Some("bot-dup"), None);
    let first_step =
        Fixture::step_metadata_blob(Some(step_uuid), Some("bot-dup"), 1_787_875_140, 250_000_000);
    let second_step =
        Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 250_000_000);
    fixture.database_with_steps(&[first, second], &[first_step, second_step]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-27", "2026-08-28"]
    );
}

// upstream: AntigravityLocalBotIDJoinTests.swift:236 conflicted bot ID rows stay out of positional matching
#[test]
fn conflicted_bot_id_rows_stay_out_of_positional_matching() {
    let fixture = Fixture::new();
    let step_uuid = "leak-step-uuid";
    let conflicted = Fixture::root_envelope_simple(Some(step_uuid), Some("bot-dup"), None);
    let plain = Fixture::root_envelope_simple(Some(step_uuid), None, None);
    let first =
        Fixture::step_metadata_blob(Some(step_uuid), Some("bot-dup"), 1_787_875_140, 250_000_000);
    let second =
        Fixture::step_metadata_blob(Some(step_uuid), Some("bot-dup"), 1_787_875_180, 250_000_000);
    let third = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 250_000_000);
    let fourth = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_320, 250_000_000);
    fixture.database_with_steps(&[conflicted, plain], &[first, second, third, fourth]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert!(report.report.data.is_empty());
}

// ============================================================================
// AntigravityStepUUIDConsistencyTests.swift
// ============================================================================

// upstream: AntigravityStepUUIDConsistencyTests.swift:8 UUID-less duplicate bot IDs cannot establish an exact date
#[test]
fn uuid_less_duplicate_bot_ids_cannot_establish_an_exact_date() {
    for unidentified_first in [false, true] {
        for unidentified_seconds in [Some(1_787_875_140u64), Some(1_787_875_260u64), None] {
            let fixture = Fixture::new();
            let step_uuid = "identified-uuid";
            let bot_id = "shared-bot";
            let turn = Fixture::root_envelope_simple(Some(step_uuid), Some(bot_id), None);
            let matching =
                Fixture::step_metadata_blob(Some(step_uuid), Some(bot_id), 1_787_875_140, 0);
            let unidentified = match unidentified_seconds {
                Some(secs) => Fixture::step_metadata_blob(None, Some(bot_id), secs, 0),
                None => Fixture::message(9, &Fixture::message(7, bot_id.as_bytes())),
            };
            let steps = if unidentified_first {
                vec![unidentified, matching]
            } else {
                vec![matching, unidentified]
            };
            let url = fixture.database_with_steps(&[turn], &steps);
            let source = read_single_db(&url);
            assert!(!source.is_complete);
            assert!(source.events.is_empty());
            assert_eq!(fixture.report().coverage, Coverage::Partial);
        }
    }
}

// upstream: AntigravityStepUUIDConsistencyTests.swift:32 pure UUID evidence must agree with embedded generation timestamps
#[test]
fn pure_uuid_evidence_must_agree_with_embedded_generation_timestamps() {
    for conflicting in [false, true] {
        let fixture = Fixture::new();
        let step_uuid = "pure-uuid-embedded-agreement";
        let generations = vec![
            Fixture::blob_with_root_envelope(
                Some(step_uuid),
                None,
                Some("fixture-model-a"),
                Some("Fixture model"),
                1298,
                100,
                30,
                50,
                7,
                None,
                Some(1_787_875_100),
            ),
            Fixture::blob_with_root_envelope(
                Some(step_uuid),
                None,
                Some("fixture-model-a"),
                Some("Fixture model"),
                1298,
                200,
                30,
                50,
                7,
                None,
                None,
            ),
        ];
        let steps = vec![
            Fixture::step_metadata_blob(
                Some(step_uuid),
                None,
                if conflicting {
                    1_787_875_200
                } else {
                    1_787_875_100
                },
                250_000_000,
            ),
            Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_300, 0),
        ];
        let url = fixture.database_with_steps(&generations, &steps);
        let source = read_single_db(&url);
        let report = fixture.report();

        assert_eq!(source.is_complete, !conflicting);
        assert_eq!(
            source.events.iter().map(|e| e.row).collect::<Vec<_>>(),
            if conflicting { vec![0] } else { vec![0, 1] }
        );
        assert_eq!(
            source
                .events
                .iter()
                .map(|e| e.turn.timestamp_ms)
                .collect::<Vec<_>>(),
            if conflicting {
                vec![Some(1_787_875_100_250)]
            } else {
                vec![Some(1_787_875_100_250), Some(1_787_875_300_000)]
            }
        );
        assert_eq!(
            report.coverage,
            if conflicting {
                Coverage::Partial
            } else {
                Coverage::Complete
            }
        );
    }
}

// upstream: AntigravityStepUUIDConsistencyTests.swift:72 UUID-less step rows do not invalidate timestamp recovery
#[test]
fn uuid_less_step_rows_do_not_invalidate_timestamp_recovery() {
    for uuid_less_row_first in [false, true] {
        let fixture = Fixture::new();
        let step_uuid = "recoverable-step-uuid";
        let generations = vec![Fixture::root_envelope_simple(Some(step_uuid), None, None)];
        let matching_step =
            Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_100, 250_000_000);
        let uuid_less_step = Fixture::step_metadata_blob(None, None, 1_787_874_000, 0);
        let steps = if uuid_less_row_first {
            vec![uuid_less_step, matching_step]
        } else {
            vec![matching_step, uuid_less_step]
        };
        let url = fixture.database_with_steps(&generations, &steps);
        let source = read_single_db(&url);
        let report = fixture.report();

        assert!(source.is_complete);
        assert_eq!(
            source.events.iter().map(|e| e.row).collect::<Vec<_>>(),
            vec![0]
        );
        assert_eq!(
            source
                .events
                .iter()
                .map(|e| e.turn.timestamp_ms)
                .collect::<Vec<_>>(),
            vec![Some(1_787_875_100_250)]
        );
        assert_eq!(report.coverage, Coverage::Complete);
    }
}

// upstream: AntigravityStepUUIDConsistencyTests.swift:105 reused step UUID recovers distinct timestamps despite an unidentified row
#[test]
fn reused_step_uuid_recovers_distinct_timestamps_despite_an_unidentified_row() {
    for unidentified_position in [0usize, 1, 2] {
        let fixture = Fixture::new();
        let step_uuid = "reused-uuid-with-unidentified-row";
        let generations = vec![
            Fixture::root_envelope_simple(Some(step_uuid), None, None),
            Fixture::blob_with_root_envelope(
                Some(step_uuid),
                None,
                Some("fixture-model-a"),
                Some("Fixture model"),
                1298,
                200,
                30,
                50,
                7,
                None,
                None,
            ),
        ];
        let first = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_140, 0);
        let second = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 0);
        let unidentified = Fixture::step_metadata_blob(None, None, 1_787_874_000, 0);
        let mut steps = vec![first, second];
        steps.insert(unidentified_position, unidentified);
        let url = fixture.database_with_steps(&generations, &steps);
        let source = read_single_db(&url);
        let report = fixture.report();

        assert!(source.is_complete);
        assert_eq!(
            source.events.iter().map(|e| e.row).collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(
            source
                .events
                .iter()
                .map(|e| e.turn.timestamp_ms)
                .collect::<Vec<_>>(),
            vec![Some(1_787_875_140_000), Some(1_787_875_260_000)]
        );
        assert_eq!(report.coverage, Coverage::Complete);
    }
}

// upstream: AntigravityStepUUIDConsistencyTests.swift:140 bot ID exact match recovers despite an unidentified step row
#[test]
fn bot_id_exact_match_recovers_despite_an_unidentified_step_row() {
    let fixture = Fixture::new();
    let step_uuid = "bot-id-with-unidentified-row";
    let bot_id = "bot-uuid-less-mix";
    let turn = Fixture::root_envelope_simple(Some(step_uuid), Some(bot_id), None);
    let matching_step =
        Fixture::step_metadata_blob(Some(step_uuid), Some(bot_id), 1_787_875_140, 0);
    let unidentified = Fixture::step_metadata_blob(None, None, 1_787_874_000, 0);
    let url = fixture.database_with_steps(&[turn], &[matching_step, unidentified]);
    let source = read_single_db(&url);
    let report = fixture.report();

    assert!(source.is_complete);
    assert_eq!(
        source.events.iter().map(|e| e.row).collect::<Vec<_>>(),
        vec![0]
    );
    assert_eq!(
        source
            .events
            .iter()
            .map(|e| e.turn.timestamp_ms)
            .collect::<Vec<_>>(),
        vec![Some(1_787_875_140_000)]
    );
    assert_eq!(report.coverage, Coverage::Complete);
}

// upstream: AntigravityStepUUIDConsistencyTests.swift:163 reused step UUID with only one identified step row stays partial alongside an unidentified row
#[test]
fn reused_step_uuid_with_only_one_identified_step_row_stays_partial_alongside_an_unidentified_row()
{
    let fixture = Fixture::new();
    let step_uuid = "reused-uuid-single-identified-row";
    let generations = vec![
        Fixture::root_envelope_simple(Some(step_uuid), None, None),
        Fixture::blob_with_root_envelope(
            Some(step_uuid),
            None,
            Some("fixture-model-a"),
            Some("Fixture model"),
            1298,
            200,
            30,
            50,
            7,
            None,
            None,
        ),
    ];
    let identified = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_140, 0);
    let unidentified = Fixture::step_metadata_blob(None, None, 1_787_874_000, 0);
    let url = fixture.database_with_steps(&generations, &[identified, unidentified]);
    let source = read_single_db(&url);
    let report = fixture.report();

    assert!(!source.is_complete);
    assert!(source.events.is_empty());
    assert_eq!(report.coverage, Coverage::Partial);
}

// ============================================================================
// AntigravityJSONLNumberTests.swift
// ============================================================================

fn jsonl_number_report(literal: &str, field: &str) -> DailyReportResult {
    let fixture = Fixture::new();
    let cache_root = fixture.context().cache_root();
    fs::create_dir_all(&cache_root).unwrap();
    let mut fields = BTreeMap::new();
    fields.insert("input", "0");
    fields.insert("output", "0");
    fields.insert("timestamp", "1787832000000");
    fields.insert(field, literal);
    let values = fields
        .into_iter()
        .map(|(k, v)| format!("\"{k}\":{v}"))
        .collect::<Vec<_>>()
        .join(",");
    let line = format!("{{\"type\":\"usage\",\"sessionId\":\"numeric-fixture\",{values}}}\n");
    fs::write(cache_root.join("session.jsonl"), line).unwrap();
    fixture.report()
}

// upstream: AntigravityJSONLNumberTests.swift:6 raw fractional rounded overflowing and nonnumeric usage fields stay unavailable
#[test]
fn raw_fractional_rounded_overflowing_and_nonnumeric_usage_fields_stay_unavailable() {
    for literal in [
        "-1e-400",
        "1e-400",
        "1e-999",
        "9.0071992547409931e15",
        "1.00000000000000001",
        "9223372036854775808",
        "1e400",
        "true",
        "\"1\"",
        "null",
        "0.0000001",
        "1787832000000.0000001",
    ] {
        for field in [
            "input",
            "output",
            "cacheRead",
            "cacheWrite",
            "reasoning",
            "timestamp",
        ] {
            let report = jsonl_number_report(literal, field);
            assert_eq!(
                report.coverage,
                Coverage::Partial,
                "literal={literal}, field={field}"
            );
        }
    }
}

// upstream: AntigravityJSONLNumberTests.swift:17 large exact input literals retain every digit
#[test]
fn large_exact_input_literals_retain_every_digit() {
    for literal in [
        "9007199254740993",
        "9.007199254740993e15",
        "9007199254740993.0",
    ] {
        let report = jsonl_number_report(literal, "input");
        assert_eq!(report.coverage, Coverage::Complete);
        assert_eq!(
            report.report.summary.as_ref().and_then(|s| s.total_tokens),
            Some(9_007_199_254_740_993)
        );
    }
}

// upstream: AntigravityJSONLNumberTests.swift:24 whole finite producer compatible literals remain supported
#[test]
fn whole_finite_producer_compatible_literals_remain_supported() {
    for literal in [
        "0", "-0", "0.0", "0e10", "100", "1e2", "10.0e1", "1000e-1", "1.00e+2",
    ] {
        let report = jsonl_number_report(literal, "input");
        assert_eq!(report.coverage, Coverage::Complete);
        let expected = if literal.starts_with('0') || literal == "-0" {
            0
        } else {
            100
        };
        assert_eq!(
            report.report.summary.as_ref().and_then(|s| s.total_tokens),
            Some(expected)
        );
    }
}

// upstream: AntigravityJSONLNumberTests.swift:31 integer boundary is accepted without a floating point round trip
#[test]
fn integer_boundary_is_accepted_without_a_floating_point_round_trip() {
    for literal in [
        "9223372036854775807",
        "92233720368547758070e-1",
        "9.223372036854775807e18",
    ] {
        let report = jsonl_number_report(literal, "input");
        assert_eq!(report.coverage, Coverage::Complete);
        assert_eq!(
            report.report.summary.as_ref().and_then(|s| s.total_tokens),
            Some(i64::MAX as usize)
        );
    }
}

// upstream: AntigravityJSONLNumberTests.swift:38 numeric masking cannot repair invalid JSON number syntax
#[test]
fn numeric_masking_cannot_repair_invalid_json_number_syntax() {
    for literal in [
        "01", "+1", ".1", "1.", "1e", "1e+", "--1", "NaN", "Infinity", "0x10",
    ] {
        assert_eq!(
            jsonl_number_report(literal, "input").coverage,
            Coverage::Partial
        );
    }
}

// upstream: AntigravityJSONLNumberTests.swift:43 escaped keys strings aliases and null retain their JSON types
#[test]
fn escaped_keys_strings_aliases_and_null_retain_their_json_types() {
    let text = concat!(
        "{\"in\\u0070ut\":1e2,\"cacheRead\":1e1,\"cache_read\":10.00,\"label\":\"1e-999\\\"\",",
        "\"modelId\":null,\"ignored\":[{\"input\":1e999}],\"output\":\"12\"}"
    );
    let object = AntigravityJSONLObject::decode(text.as_bytes(), || Ok::<(), ScanFailure>(()))
        .unwrap()
        .unwrap();
    assert_eq!(
        match object.get("input") {
            Some(AntigravityJSONLValue::Number(n)) => n.integer,
            _ => None,
        },
        Some(100)
    );
    assert_eq!(
        match object.get("cacheRead") {
            Some(AntigravityJSONLValue::Number(n)) => n.integer,
            _ => None,
        },
        Some(10)
    );
    assert_eq!(
        match object.get("cache_read") {
            Some(AntigravityJSONLValue::Number(n)) => n.integer,
            _ => None,
        },
        Some(10)
    );
    assert_eq!(
        object.get("label"),
        Some(&AntigravityJSONLValue::String("1e-999\"".to_owned()))
    );
    assert_eq!(
        object.get("output"),
        Some(&AntigravityJSONLValue::String("12".to_owned()))
    );
    assert_eq!(object.get("modelId"), Some(&AntigravityJSONLValue::Null));
    assert_eq!(
        AntigravityJSONLObject::decode(
            b"{\"input\":1,\"in\\u0070ut\":2}",
            || Ok::<(), ScanFailure>(())
        )
        .unwrap(),
        None
    );
}

// upstream: AntigravityJSONLNumberTests.swift:58 long numeric lexemes and strings propagate a one-shot cancellation unchanged
#[test]
fn long_numeric_lexemes_and_strings_propagate_a_one_shot_cancellation_unchanged() {
    let num_val = format!("1{}", "0".repeat(10_000));
    let str_val = format!("\"{}\"", "a".repeat(10_000));
    for value in [num_val, str_val] {
        let checks = AtomicUsize::new(0);
        let input = format!("{{\"input\":{value}}}");
        let res = AntigravityJSONLObject::decode(input.as_bytes(), || {
            let cur = checks.fetch_add(1, Ordering::SeqCst) + 1;
            if cur == 3 {
                Err(ScanFailure::Cancelled("posix-4321".to_owned()))
            } else {
                Ok(())
            }
        });
        assert_eq!(res, Err(ScanFailure::Cancelled("posix-4321".to_owned())));
        assert_eq!(checks.load(Ordering::SeqCst), 3);
    }
}

// ============================================================================
// AntigravityLocalWALTests.swift
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlQueryStatus {
    Row,
    CantOpen,
    OtherError,
}

fn wal_checkpoint(conn: &Connection) -> i32 {
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
        .unwrap()
}

fn wal_prepare(url: &Path) {
    let writer = Fixture::open(url);
    Fixture::execute(&writer, "PRAGMA journal_mode=WAL");
}

fn wal_add_unreadable(url: &Path) {
    let wal_path = PathBuf::from(format!("{}-wal", url.display()));
    fs::write(&wal_path, b"").unwrap();
    fs::set_permissions(&wal_path, fs::Permissions::from_mode(0o0)).unwrap();
}

fn wal_read_only_query_status(url: &Path) -> ControlQueryStatus {
    let conn = match Connection::open_with_flags(
        url,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(err) => {
            return if err.sqlite_error_code() == Some(rusqlite::ErrorCode::CannotOpen) {
                ControlQueryStatus::CantOpen
            } else {
                ControlQueryStatus::OtherError
            };
        }
    };
    let mut stmt = match conn.prepare("SELECT idx, data FROM gen_metadata") {
        Ok(s) => s,
        Err(err) => {
            return if err.sqlite_error_code() == Some(rusqlite::ErrorCode::CannotOpen) {
                ControlQueryStatus::CantOpen
            } else {
                ControlQueryStatus::OtherError
            };
        }
    };
    let mut rows = stmt.raw_query();
    match rows.next() {
        Ok(Some(_)) => ControlQueryStatus::Row,
        Err(err) if err.sqlite_error_code() == Some(rusqlite::ErrorCode::CannotOpen) => {
            ControlQueryStatus::CantOpen
        }
        _ => ControlQueryStatus::OtherError,
    }
}

fn foreign_wal_database(fixture: &Fixture, name: &str) -> PathBuf {
    let roots = fixture.context().database_roots();
    let root = &roots[0];
    fs::create_dir_all(root).unwrap();
    let url = root.join(format!("{name}.db"));
    let writer = Fixture::open(&url);
    Fixture::execute(
        &writer,
        "CREATE TABLE `conversation_summaries` (`conversation_id` text PRIMARY KEY, `raw_summary` blob)",
    );
    drop(writer);
    wal_prepare(&url);
    url
}

// upstream: AntigravityLocalWALTests.swift:13 schema validation and payload selection retain one snapshot during a writer schema change
#[test]
fn schema_validation_and_payload_selection_retain_one_snapshot_during_a_writer_schema_change() {
    let fixture = Fixture::new();
    let url = fixture.database(&[]);
    let writer = Arc::new(Mutex::new(Fixture::open(&url)));
    {
        let w = writer.lock().unwrap();
        Fixture::execute(&w, "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0");
        Fixture::insert(&w, 0, &Fixture::blob());
    }
    let changed = Arc::new(AtomicBool::new(false));
    let changed_clone = changed.clone();
    let writer_clone = writer.clone();
    let holder: Arc<Mutex<Option<Budget>>> = Arc::new(Mutex::new(None));
    let holder_clone = holder.clone();
    let budget = Budget::with_cancellation(Limits::default(), move || {
        let cols = holder_clone
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |b| b.statistics().schema_columns);
        if !changed_clone.load(Ordering::SeqCst) && cols == 2 {
            changed_clone.store(true, Ordering::SeqCst);
            let w = writer_clone.lock().unwrap();
            Fixture::execute(
                &w,
                "ALTER TABLE gen_metadata RENAME TO backing;
                 CREATE VIEW gen_metadata AS SELECT idx, data FROM backing;",
            );
        }
        Ok(())
    });
    *holder.lock().unwrap() = Some(budget.clone());
    let source = AntigravityLocalReader::read_databases(&[url], &budget).unwrap();
    *holder.lock().unwrap() = None;
    assert!(changed.load(Ordering::SeqCst));
    assert!(source.is_complete);
    assert_eq!(source.events.len(), 1);
    assert_eq!(fixture.report().coverage, Coverage::Partial);
    assert_eq!(wal_checkpoint(&writer.lock().unwrap()), 0);
}

// upstream: AntigravityLocalWALTests.swift:42 read-only live WAL preserves database data while permitting normal SHM coordination
#[test]
fn read_only_live_wal_preserves_database_data_while_permitting_normal_shm_coordination() {
    let fixture = Fixture::new();
    let url = fixture.database(&[]);
    let writer = Fixture::open(&url);
    Fixture::execute(
        &writer,
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0",
    );
    Fixture::insert(&writer, 0, &Fixture::blob());
    let wal = PathBuf::from(format!("{}-wal", url.display()));
    let shm = PathBuf::from(format!("{}-shm", url.display()));
    let before_db = fs::read(&url).unwrap();
    let before_wal = fs::read(&wal).unwrap();
    let before_shm = fs::read(&shm).unwrap();

    assert_eq!(
        fixture
            .report()
            .report
            .summary
            .as_ref()
            .and_then(|s| s.total_tokens),
        Some(187)
    );

    assert_eq!(fs::read(&url).unwrap(), before_db);
    assert_eq!(fs::read(&wal).unwrap(), before_wal);
    let after_shm = fs::read(&shm).unwrap();
    assert_eq!(after_shm.len(), before_shm.len());
    assert_eq!(&after_shm[..96], &before_shm[..96]);
    assert_eq!(wal_checkpoint(&writer), 0);
}

// upstream: AntigravityLocalWALTests.swift:68 single SQL snapshot excludes coordinated later writes and releases handles on cancellation
#[test]
fn single_sql_snapshot_excludes_coordinated_later_writes_and_releases_handles_on_cancellation() {
    let fixture = Fixture::new();
    let url = fixture.database(&[]);
    let writer = Arc::new(Mutex::new(Fixture::open(&url)));
    {
        let w = writer.lock().unwrap();
        Fixture::execute(&w, "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0");
        Fixture::insert(&w, 0, &Fixture::blob());
    }
    let wal = PathBuf::from(format!("{}-wal", url.display()));
    let before = fs::read(&wal).unwrap();
    let inserted = Arc::new(AtomicBool::new(false));
    let inserted_clone = inserted.clone();
    let writer_clone = writer.clone();
    let holder: Arc<Mutex<Option<Budget>>> = Arc::new(Mutex::new(None));
    let holder_clone = holder.clone();
    let budget = Budget::with_cancellation(Limits::default(), move || {
        let rows = holder_clone
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |b| b.statistics().rows);
        if !inserted_clone.load(Ordering::SeqCst) && rows == 1 {
            inserted_clone.store(true, Ordering::SeqCst);
            let w = writer_clone.lock().unwrap();
            Fixture::insert(&w, 1, &Fixture::blob_response("later"));
        }
        Ok(())
    });
    *holder.lock().unwrap() = Some(budget.clone());
    let source =
        AntigravityLocalReader::read_databases(std::slice::from_ref(&url), &budget).unwrap();
    assert_eq!(budget.statistics().sqlite_handles_opened, 1);
    assert_eq!(budget.statistics().sqlite_handles_closed, 1);
    *holder.lock().unwrap() = None;
    assert!(inserted.load(Ordering::SeqCst));
    assert!(source.is_complete);
    assert_eq!(source.events.len(), 1);
    assert_ne!(fs::read(&wal).unwrap(), before);
    assert_eq!(
        fixture
            .report()
            .report
            .summary
            .as_ref()
            .and_then(|s| s.total_tokens),
        Some(374)
    );

    let cancel_holder: Arc<Mutex<Option<Budget>>> = Arc::new(Mutex::new(None));
    let cancel_holder_clone = cancel_holder.clone();
    let cancel_budget = Budget::with_cancellation(Limits::default(), move || {
        let rows = cancel_holder_clone
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |b| b.statistics().rows);
        if rows == 1 {
            return Err(ScanFailure::Cancelled("cancelled".to_owned()));
        }
        Ok(())
    });
    *cancel_holder.lock().unwrap() = Some(cancel_budget.clone());
    let res = AntigravityLocalReader::read_databases(std::slice::from_ref(&url), &cancel_budget);
    assert_eq!(
        res.unwrap_err(),
        ScanFailure::Cancelled("cancelled".to_owned())
    );
    assert_eq!(cancel_budget.statistics().sqlite_handles_opened, 1);
    assert_eq!(cancel_budget.statistics().sqlite_handles_closed, 1);
    *cancel_holder.lock().unwrap() = None;
    {
        let w = writer.lock().unwrap();
        assert_eq!(wal_checkpoint(&w), 0);
        Fixture::execute(&w, "PRAGMA journal_mode=DELETE");
    }
    drop(writer);
    assert!(!wal.exists());
}

// upstream: AntigravityLocalWALTests.swift:119 steps fallback remains on the generation snapshot after a writer update
#[test]
fn steps_fallback_remains_on_the_generation_snapshot_after_a_writer_update() {
    let fixture = Fixture::new();
    let step_uuid = "snapshot-step-uuid";
    let initial_seconds: u64 = 1_787_875_140;
    let later_seconds: u64 = 1_787_875_260;
    let turn = Fixture::root_envelope_simple(Some(step_uuid), None, None);
    let initial_step = Fixture::step_metadata_blob(Some(step_uuid), None, initial_seconds, 0);
    let later_step = Fixture::step_metadata_blob(Some(step_uuid), None, later_seconds, 0);
    let url = fixture.database(&[]);
    let writer = Arc::new(Mutex::new(Fixture::open(&url)));
    {
        let w = writer.lock().unwrap();
        Fixture::execute(&w, "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0");
        Fixture::insert(&w, 0, &turn);
        Fixture::execute(
            &w,
            "CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB)",
        );
        Fixture::insert_step(&w, 0, &initial_step);
    }
    let updated = Arc::new(AtomicBool::new(false));
    let updated_clone = updated.clone();
    let writer_clone = writer.clone();
    let holder: Arc<Mutex<Option<Budget>>> = Arc::new(Mutex::new(None));
    let holder_clone = holder.clone();
    let budget = Budget::with_cancellation(Limits::default(), move || {
        let cols = holder_clone
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |b| b.statistics().schema_columns);
        if !updated_clone.load(Ordering::SeqCst) && cols == 2 {
            updated_clone.store(true, Ordering::SeqCst);
            let w = writer_clone.lock().unwrap();
            Fixture::execute(&w, "BEGIN IMMEDIATE");
            Fixture::execute(&w, "DELETE FROM steps");
            Fixture::insert_step(&w, 0, &later_step);
            Fixture::execute(&w, "COMMIT");
        }
        Ok(())
    });
    *holder.lock().unwrap() = Some(budget.clone());
    let source = AntigravityLocalReader::read_databases(&[url], &budget).unwrap();
    *holder.lock().unwrap() = None;

    assert!(updated.load(Ordering::SeqCst));
    assert!(source.is_complete);
    assert_eq!(source.events.len(), 1);
    assert_eq!(
        source.events[0].turn.timestamp_ms,
        Some((initial_seconds as i64) * 1000)
    );
    assert_eq!(
        fixture
            .report()
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-28"]
    );
    assert_eq!(wal_checkpoint(&writer.lock().unwrap()), 0);
}

// upstream: AntigravityLocalWALTests.swift:165 initially absent WAL sidecars and failed opens leave no reader handles
#[test]
fn initially_absent_wal_sidecars_and_failed_opens_leave_no_reader_handles() {
    let fixture = Fixture::new();
    let url = fixture.database(&[Fixture::blob()]);
    wal_prepare(&url);
    let before = fs::read(&url).unwrap();
    let wal_path = PathBuf::from(format!("{}-wal", url.display()));
    let shm_path = PathBuf::from(format!("{}-shm", url.display()));
    assert!(!wal_path.exists());
    assert!(!shm_path.exists());

    let control_fixture = Fixture::new();
    let control_url = control_fixture.database(&[Fixture::blob()]);
    wal_prepare(&control_url);
    let control = wal_read_only_query_status(&control_url);
    assert!(matches!(
        control,
        ControlQueryStatus::Row | ControlQueryStatus::CantOpen
    ));
    assert!(!wal_path.exists());
    assert!(!shm_path.exists());

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-27"]
    );
    assert_eq!(
        report.statistics.immutable_fallbacks,
        if control == ControlQueryStatus::Row {
            0
        } else {
            1
        }
    );
    assert_eq!(
        report.statistics.sqlite_handles_opened,
        report.statistics.sqlite_handles_closed
    );
    assert_eq!(fs::read(&url).unwrap(), before);
    if control == ControlQueryStatus::CantOpen {
        assert!(!wal_path.exists());
        assert!(!shm_path.exists());
    }

    let reopened = Fixture::open(&url);
    assert_eq!(wal_checkpoint(&reopened), 0);
    Fixture::execute(&reopened, "PRAGMA journal_mode=DELETE");
    drop(reopened);
    assert!(!wal_path.exists());

    let missing = fixture.root.join("missing/absent.db");
    for _ in 0..20 {
        let budget = Budget::with_cancellation(Limits::default(), || Ok(()));
        let result =
            AntigravityLocalReader::read_databases(std::slice::from_ref(&missing), &budget)
                .unwrap();
        assert!(!result.is_complete);
        assert_eq!(
            budget.statistics().sqlite_handles_opened,
            budget.statistics().sqlite_handles_closed
        );
    }
    assert!(!missing.exists());
}

// upstream: AntigravityLocalWALTests.swift:222 immutable fallback reads a sidecar-less WAL database through an escaped URI path
#[test]
fn immutable_fallback_reads_a_sidecar_less_wal_database_through_an_escaped_uri_path() {
    let fixture = Fixture::new();
    let session = "odd %25 ?q #f";
    let url = fixture.database_named(session, 0, &[Fixture::blob()]);
    wal_prepare(&url);
    let wal_path = PathBuf::from(format!("{}-wal", url.display()));
    let shm_path = PathBuf::from(format!("{}-shm", url.display()));
    assert!(!wal_path.exists());

    let control_fixture = Fixture::new();
    let control_url = control_fixture.database_named(session, 0, &[Fixture::blob()]);
    wal_prepare(&control_url);
    let control = wal_read_only_query_status(&control_url);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-27"]
    );
    assert_eq!(
        report.statistics.immutable_fallbacks,
        if control == ControlQueryStatus::Row {
            0
        } else {
            1
        }
    );
    assert_eq!(
        report.statistics.sqlite_handles_opened,
        report.statistics.sqlite_handles_closed
    );
    if control == ControlQueryStatus::CantOpen {
        assert!(!wal_path.exists());
        assert!(!shm_path.exists());
    }
}

// upstream: AntigravityLocalWALTests.swift:248 a sidecar-less WAL foreign database is skipped after the immutable fallback reads its schema
#[test]
fn a_sidecar_less_wal_foreign_database_is_skipped_after_the_immutable_fallback_reads_its_schema() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob()]);
    let summaries = foreign_wal_database(&fixture, "conversation_summaries");
    let wal_path = PathBuf::from(format!("{}-wal", summaries.display()));
    let shm_path = PathBuf::from(format!("{}-shm", summaries.display()));
    assert!(!wal_path.exists());
    assert!(!shm_path.exists());

    let control_fixture = Fixture::new();
    let control_url = foreign_wal_database(&control_fixture, "conversation_summaries");
    let control = wal_read_only_query_status(&control_url);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(187)
    );
    assert_eq!(report.statistics.foreign_databases, 1);
    assert_eq!(
        report.statistics.immutable_fallbacks,
        if control == ControlQueryStatus::CantOpen {
            1
        } else {
            0
        }
    );
    assert_eq!(
        report.statistics.sqlite_handles_opened,
        report.statistics.sqlite_handles_closed
    );
    if control == ControlQueryStatus::CantOpen {
        assert!(!wal_path.exists());
        assert!(!shm_path.exists());
    }
}

// upstream: AntigravityLocalWALTests.swift:292 a present WAL sidecar keeps a declined read-only open unavailable
#[test]
fn a_present_wal_sidecar_keeps_a_declined_read_only_open_unavailable() {
    let fixture = Fixture::new();
    let url = fixture.database(&[Fixture::blob()]);
    wal_prepare(&url);
    wal_add_unreadable(&url);
    let before = fs::read(&url).unwrap();

    let control_fixture = Fixture::new();
    let control_url = control_fixture.database(&[Fixture::blob()]);
    wal_prepare(&control_url);
    wal_add_unreadable(&control_url);
    let control = wal_read_only_query_status(&control_url);

    let report = fixture.report();
    assert_eq!(report.statistics.immutable_fallbacks, 0);
    assert_eq!(
        report.coverage,
        if control == ControlQueryStatus::Row {
            Coverage::Complete
        } else {
            Coverage::Partial
        }
    );
    assert_eq!(
        report.statistics.sqlite_handles_opened,
        report.statistics.sqlite_handles_closed
    );
    assert_eq!(fs::read(&url).unwrap(), before);
}

// upstream: AntigravityLocalWALTests.swift:317 a writer that checkpoints during the immutable fallback keeps the result incomplete
#[test]
fn a_writer_that_checkpoints_during_the_immutable_fallback_keeps_the_result_incomplete() {
    let fixture = Fixture::new();
    let url = fixture.database(&[Fixture::blob()]);
    wal_prepare(&url);
    let wal_path = PathBuf::from(format!("{}-wal", url.display()));
    assert!(!wal_path.exists());

    let control_fixture = Fixture::new();
    let control_url = control_fixture.database(&[Fixture::blob()]);
    wal_prepare(&control_url);
    let control = wal_read_only_query_status(&control_url);

    let written = Arc::new(AtomicBool::new(false));
    let written_clone = written.clone();
    let url_clone = url.clone();
    let holder: Arc<Mutex<Option<Budget>>> = Arc::new(Mutex::new(None));
    let holder_clone = holder.clone();
    let budget = Budget::with_cancellation(Limits::default(), move || {
        let rows = holder_clone
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |b| b.statistics().rows);
        if !written_clone.load(Ordering::SeqCst) && rows == 1 {
            written_clone.store(true, Ordering::SeqCst);
            let writer = Fixture::open(&url_clone);
            Fixture::insert(&writer, 1, &Fixture::blob_response("later"));
            let _ = wal_checkpoint(&writer);
        }
        Ok(())
    });
    *holder.lock().unwrap() = Some(budget.clone());
    let source = AntigravityLocalReader::read_databases(&[url], &budget).unwrap();
    let statistics = budget.statistics();
    *holder.lock().unwrap() = None;

    assert!(written.load(Ordering::SeqCst));
    assert_eq!(
        statistics.sqlite_handles_opened,
        statistics.sqlite_handles_closed
    );
    if control == ControlQueryStatus::CantOpen {
        assert_eq!(statistics.immutable_fallbacks, 1);
        assert!(!source.is_complete);
        assert!(source.evidence_is_unstable);
        let next = fixture.report();
        assert_eq!(next.coverage, Coverage::Complete);
        assert_eq!(
            next.report.summary.as_ref().and_then(|s| s.total_tokens),
            Some(374)
        );
    } else {
        assert_eq!(statistics.immutable_fallbacks, 0);
        assert!(source.is_complete);
        assert!(!source.evidence_is_unstable);
        assert_eq!(source.events.len(), 1);
    }
}

// upstream: AntigravityLocalWALTests.swift:365 an unchanged database keeps the immutable fallback result complete
#[test]
fn an_unchanged_database_keeps_the_immutable_fallback_result_complete() {
    let fixture = Fixture::new();
    let url = fixture.database(&[Fixture::blob(), Fixture::blob_response("second")]);
    wal_prepare(&url);
    let before = fs::read(&url).unwrap();
    let budget = Budget::with_cancellation(Limits::default(), || Ok(()));
    let source =
        AntigravityLocalReader::read_databases(std::slice::from_ref(&url), &budget).unwrap();
    assert!(source.is_complete);
    assert_eq!(source.events.len(), 2);
    assert_eq!(
        budget.statistics().sqlite_handles_opened,
        budget.statistics().sqlite_handles_closed
    );
    assert_eq!(fs::read(&url).unwrap(), before);
}

// ============================================================================
// AntigravityLocalIntegrityTests.swift
// ============================================================================

const CONVERSATION_SUMMARIES_SCHEMA: &str = r#"
CREATE TABLE `conversation_summaries` (`conversation_id` text,`title` text NOT NULL DEFAULT "",
`preview` text NOT NULL DEFAULT "",`step_count` integer NOT NULL DEFAULT 0,
`last_modified_time` datetime NOT NULL,`workspace_uris` text NOT NULL,
`status` text NOT NULL DEFAULT "",`source` text NOT NULL DEFAULT "",
`project_id` text NOT NULL DEFAULT "",`agent_name` text NOT NULL DEFAULT "",
`parent_conversation_id` text NOT NULL DEFAULT "",`nesting_depth` integer NOT NULL DEFAULT 0,
`battle_id` text NOT NULL DEFAULT "",`winning_conversation_id` text NOT NULL DEFAULT "",
`not_fully_idle` numeric NOT NULL DEFAULT false,`killed` numeric NOT NULL DEFAULT false,
`last_user_input_time` datetime NOT NULL,`last_user_input_step_index` integer NOT NULL DEFAULT -1,
`app_data_dir` text NOT NULL DEFAULT "",`raw_summary` blob,PRIMARY KEY (`conversation_id`));
CREATE INDEX `idx_conversation_summaries_last_user_input_time`
    ON `conversation_summaries`(`last_user_input_time`);
CREATE INDEX `idx_conversation_summaries_last_modified_time`
    ON `conversation_summaries`(`last_modified_time`);
"#;

fn foreign_database(fixture: &Fixture, name: &str, schema: Option<&str>) -> PathBuf {
    let roots = fixture.context().database_roots();
    let root = &roots[0];
    fs::create_dir_all(root).unwrap();
    let url = root.join(format!("{name}.db"));
    let conn = Fixture::open(&url);
    if let Some(sql) = schema {
        Fixture::execute(&conn, sql);
    }
    url
}

// upstream: AntigravityLocalIntegrityTests.swift:13 copied rows with missing metadata leave valid evidence explicitly partial in either order
#[test]
fn copied_rows_with_missing_metadata_leave_valid_evidence_explicitly_partial_in_either_order() {
    for malformed_first in [false, true] {
        let fixture = Fixture::new();
        let valid = Fixture::blob_response("copied-response");
        let malformed = Fixture::blob_custom(
            Some("fixture-model-a"),
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            Some("copied-response"),
            None,
        );
        fixture.database_at_root(
            0,
            &[if malformed_first {
                malformed.clone()
            } else {
                valid.clone()
            }],
        );
        fixture.database_at_root(1, &[if malformed_first { valid } else { malformed }]);
        let report = fixture.report();
        assert_eq!(report.coverage, Coverage::Partial);
        assert!(!report.evidence_is_contradicted);
        let snapshot = fixture.snapshot();
        assert_eq!(snapshot.last_30_days_tokens, Some(187));
        assert!(snapshot.history_scan_is_partial);
        assert!(!snapshot.history_coverage_is_established);
    }
}

// upstream: AntigravityLocalIntegrityTests.swift:31 duplicate responses retain contradictory copied row evidence in either source order
#[test]
fn duplicate_responses_retain_contradictory_copied_row_evidence_in_either_source_order() {
    for reversed in [false, true] {
        for contradictory in [false, true] {
            for sqlite in [true, false] {
                let fixture = Fixture::new();
                let mut responses = [
                    vec!["r", "r"],
                    vec!["r", if contradictory { "s" } else { "r" }],
                ];
                if reversed {
                    responses.reverse();
                }
                for (index, ids) in responses.iter().enumerate() {
                    if sqlite {
                        let blobs: Vec<Vec<u8>> =
                            ids.iter().map(|id| Fixture::blob_response(id)).collect();
                        fixture.database_at_root(index, &blobs);
                    } else {
                        let lines: Vec<String> = ids
                            .iter()
                            .map(|id| {
                                format!(
                                    "{{\"type\":\"usage\",\"sessionId\":\"copied\",\"responseId\":\"{id}\",\"input\":10,\"output\":2,\"timestamp\":1787832000000}}"
                                )
                            })
                            .collect();
                        fixture.jsonl_strings(&lines, if index == 0 { "a" } else { "z" });
                    }
                }
                let report = fixture.report();
                assert_eq!(
                    report.coverage,
                    if contradictory {
                        Coverage::Partial
                    } else {
                        Coverage::Complete
                    }
                );
                let snapshot = fixture.snapshot();
                assert_eq!(snapshot.history_coverage_is_established, !contradictory);
                if contradictory {
                    assert!(snapshot.daily.is_empty());
                    assert_eq!(snapshot.last_30_days_tokens, None);
                } else {
                    assert_eq!(
                        snapshot.last_30_days_tokens,
                        Some(if sqlite { 187 } else { 12 })
                    );
                    assert_eq!(snapshot.daily[0].request_count, Some(1));
                }
            }
        }
    }
}

// upstream: AntigravityLocalIntegrityTests.swift:63 copied rows differing only in model enum ID conflict like any other unequal copy
#[test]
fn copied_rows_differing_only_in_model_enum_id_conflict_like_any_other_unequal_copy() {
    for conflicting in [false, true] {
        let fixture = Fixture::new();
        let first = Fixture::blob_custom(
            Some("fixture-model-a"),
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        );
        let second = Fixture::blob_custom(
            Some("fixture-model-a"),
            Some("Fixture model"),
            if conflicting { 1318 } else { 1298 },
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        );
        fixture.database_at_root(0, &[first]);
        fixture.database_at_root(1, &[second]);
        let report = fixture.report();
        assert_eq!(
            report.coverage,
            if conflicting {
                Coverage::Partial
            } else {
                Coverage::Complete
            }
        );
        assert_eq!(report.evidence_is_contradicted, conflicting);
        let snapshot = fixture.snapshot();
        assert_eq!(snapshot.history_coverage_is_established, !conflicting);
        if conflicting {
            assert!(snapshot.daily.is_empty());
            assert_eq!(snapshot.last_30_days_tokens, None);
        } else {
            assert_eq!(snapshot.last_30_days_tokens, Some(187));
        }
    }
}

// upstream: AntigravityLocalIntegrityTests.swift:85 computed SQLite layouts are rejected before any payload row is selected
#[test]
fn computed_sqlite_layouts_are_rejected_before_any_payload_row_is_selected() {
    for layout in [
        "view",
        "expression",
        "ordered",
        "grouped",
        "generated-data",
        "generated-idx",
        "stored",
        "virtual",
    ] {
        let fixture = Fixture::new();
        let url = fixture.database(&[Fixture::blob(), Fixture::blob()]);
        let conn = Fixture::open(&url);
        Fixture::execute(&conn, "ALTER TABLE gen_metadata RENAME TO backing");
        let sql = match layout {
            "view" => "CREATE VIEW gen_metadata AS SELECT idx, data FROM backing",
            "expression" => {
                "CREATE VIEW gen_metadata AS SELECT idx, substr(data, 1, length(data)) AS data FROM backing"
            }
            "ordered" => "CREATE VIEW gen_metadata AS SELECT idx, data FROM backing ORDER BY data",
            "grouped" => {
                "CREATE VIEW gen_metadata AS SELECT idx, data FROM backing GROUP BY idx, data"
            }
            "generated-data" => {
                "CREATE TABLE gen_metadata (idx INTEGER, payload BLOB, data BLOB AS (payload) VIRTUAL);
                 INSERT INTO gen_metadata (idx, payload) SELECT idx, data FROM backing;"
            }
            "stored" => {
                "CREATE TABLE gen_metadata (idx INTEGER, payload BLOB, data BLOB AS (payload) STORED);
                 INSERT INTO gen_metadata (idx, payload) SELECT idx, data FROM backing;"
            }
            "generated-idx" => {
                "CREATE TABLE gen_metadata (original INTEGER, idx INTEGER AS (original), data BLOB);
                 INSERT INTO gen_metadata (original, data) SELECT idx, data FROM backing;"
            }
            _ => {
                "CREATE VIRTUAL TABLE gen_metadata USING fts5(idx, data); INSERT INTO gen_metadata VALUES (0, 'invalid')"
            }
        };
        Fixture::execute(&conn, sql);
        drop(conn);
        fixture.jsonl(&[Fixture::CACHE_USAGE]);
        let report = fixture.report();
        assert_eq!(report.coverage, Coverage::Partial, "layout={layout}");
        assert_eq!(report.statistics.rows, 0);
        assert_eq!(report.statistics.attempted_bytes, 0);
        assert_eq!(report.statistics.materialized_payload_bytes, 0);
        assert_eq!(
            report.statistics.sqlite_handles_opened,
            report.statistics.sqlite_handles_closed
        );
        let snapshot = fixture.snapshot();
        assert!(!snapshot.history_coverage_is_established);
        assert!(snapshot.daily.is_empty());
    }
}

// upstream: AntigravityLocalIntegrityTests.swift:135 stored ordinary SQLite tables remain supported
#[test]
fn stored_ordinary_sqlite_tables_remain_supported() {
    for layout in ["ordinary", "extra-column", "without-rowid"] {
        let fixture = Fixture::new();
        let url = fixture.database(&[]);
        let conn = Fixture::open(&url);
        Fixture::execute(&conn, "DROP TABLE gen_metadata");
        let definition = if layout == "extra-column" {
            "idx INTEGER, data BLOB, size INTEGER"
        } else {
            "idx INTEGER PRIMARY KEY, data BLOB"
        };
        let suffix = if layout == "without-rowid" {
            "WITHOUT ROWID"
        } else {
            ""
        };
        Fixture::execute(
            &conn,
            &format!("CREATE TABLE gen_metadata ({definition}) {suffix}"),
        );
        Fixture::insert(&conn, 0, &Fixture::blob());
        drop(conn);
        let report = fixture.report();
        assert_eq!(report.coverage, Coverage::Complete);
        assert_eq!(
            report.report.summary.as_ref().and_then(|s| s.total_tokens),
            Some(187)
        );
        assert_eq!(report.statistics.rows, 1);
    }
}

// upstream: AntigravityLocalIntegrityTests.swift:186 a foreign database beside the history is skipped without withholding any day
#[test]
fn a_foreign_database_beside_the_history_is_skipped_without_withholding_any_day() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob()]);
    foreign_database(
        &fixture,
        "conversation_summaries",
        Some(CONVERSATION_SUMMARIES_SCHEMA),
    );
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(187)
    );
    assert_eq!(report.statistics.foreign_databases, 1);
    assert_eq!(
        report.statistics.sqlite_handles_opened,
        report.statistics.sqlite_handles_closed
    );
}

// upstream: AntigravityLocalIntegrityTests.swift:200 foreign databases alone do not establish empty history or select the JSONL cache
#[test]
fn foreign_databases_alone_do_not_establish_empty_history_or_select_the_jsonl_cache() {
    let fixture = Fixture::new();
    foreign_database(
        &fixture,
        "conversation_summaries",
        Some(CONVERSATION_SUMMARIES_SCHEMA),
    );
    fixture.jsonl(&[Fixture::CACHE_USAGE]);

    let report = fixture.report();
    let snapshot = fixture.snapshot();

    assert_eq!(report.coverage, Coverage::Unavailable);
    assert!(!report.is_available());
    assert_eq!(report.statistics.foreign_databases, 1);
    assert_eq!(report.statistics.rows, 0);
    assert_eq!(
        report.statistics.sqlite_handles_opened,
        report.statistics.sqlite_handles_closed
    );
    assert!(!snapshot.history_coverage_is_established);
    assert!(snapshot.daily.is_empty());
    assert_eq!(snapshot.last_30_days_tokens, None);
}

// upstream: AntigravityLocalIntegrityTests.swift:219 supported empty history stays complete beside foreign databases
#[test]
fn supported_empty_history_stays_complete_beside_foreign_databases() {
    for name in ["a-summaries", "z-summaries"] {
        let fixture = Fixture::new();
        fixture.database(&[]);
        foreign_database(&fixture, name, Some(CONVERSATION_SUMMARIES_SCHEMA));

        let report = fixture.report();
        let snapshot = fixture.snapshot();

        assert_eq!(report.coverage, Coverage::Complete);
        assert!(report.is_available());
        assert_eq!(report.statistics.foreign_databases, 1);
        assert_eq!(
            report.statistics.sqlite_handles_opened,
            report.statistics.sqlite_handles_closed
        );
        assert!(snapshot.history_coverage_is_established);
        assert!(snapshot.daily.is_empty());
    }
}

// upstream: AntigravityLocalIntegrityTests.swift:236 undecodable SQLite schema names do not prove a database is foreign
#[test]
fn undecodable_sqlite_schema_names_do_not_prove_a_database_is_foreign() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob()]);
    let url = foreign_database(
        &fixture,
        "undecodable",
        Some("CREATE TABLE t (value INTEGER)"),
    );
    let conn = Fixture::open(&url);
    Fixture::execute(
        &conn,
        "PRAGMA writable_schema = ON;
         UPDATE sqlite_master SET name = CAST(X'FF' AS TEXT), tbl_name = CAST(X'FF' AS TEXT) WHERE name = 't';
         PRAGMA writable_schema = OFF;",
    );
    drop(conn);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert_eq!(report.statistics.foreign_databases, 0);
    assert_eq!(
        report.statistics.sqlite_handles_opened,
        report.statistics.sqlite_handles_closed
    );
}

// upstream: AntigravityLocalIntegrityTests.swift:256 a gen_metadata table with unknown columns stays incomplete instead of foreign
#[test]
fn a_gen_metadata_table_with_unknown_columns_stays_incomplete_instead_of_foreign() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob()]);
    foreign_database(
        &fixture,
        "drifted",
        Some("CREATE TABLE gen_metadata (idx INTEGER, wrong TEXT)"),
    );

    let report = fixture.report();
    assert_ne!(report.coverage, Coverage::Complete);
    assert_eq!(report.statistics.foreign_databases, 0);
}

// upstream: AntigravityLocalIntegrityTests.swift:271 a database that describes no schema at all remains unsupported rather than foreign
#[test]
fn a_database_that_describes_no_schema_at_all_remains_unsupported_rather_than_foreign() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob()]);
    let empty = foreign_database(
        &fixture,
        "empty",
        Some("CREATE TABLE tmp (v INT); DROP TABLE tmp;"),
    );
    assert!(empty.exists());

    let report = fixture.report();
    assert_ne!(report.coverage, Coverage::Complete);
    assert_eq!(report.statistics.foreign_databases, 0);
}

// upstream: AntigravityLocalIntegrityTests.swift:284 a schema walk that the entry limit truncated never counts as foreign
#[test]
fn a_schema_walk_that_the_entry_limit_truncated_never_counts_as_foreign() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob()]);
    let tables = (0..130)
        .map(|i| format!("CREATE TABLE unrelated_{i} (value INTEGER);"))
        .collect::<String>();
    foreign_database(&fixture, "wide", Some(&tables));
    let limits = Limits {
        schema_entries: 200,
        ..Limits::default()
    };
    let report = fixture.report_with_limits(limits);
    assert_ne!(report.coverage, Coverage::Complete);
    assert_eq!(report.statistics.foreign_databases, 0);
}

// upstream: AntigravityLocalIntegrityTests.swift:301 embedded timestamps do not spend schema budget discovering optional steps
#[test]
fn embedded_timestamps_do_not_spend_schema_budget_discovering_optional_steps() {
    let fixture = Fixture::new();
    let url = fixture.database(&[Fixture::blob()]);
    let conn = Fixture::open(&url);
    Fixture::execute(&conn, "CREATE TABLE unrelated (value INTEGER)");
    drop(conn);
    let limits = Limits {
        schema_entries: 1,
        ..Limits::default()
    };
    let report = fixture.report_with_limits(limits);
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(report.statistics.schema_entries, 1);
    assert_eq!(report.statistics.rows, 1);
}

// upstream: AntigravityLocalIntegrityTests.swift:318 copy guard derives length from the selected BLOB and rejects inconsistent declarations
#[test]
fn copy_guard_derives_length_from_the_selected_blob_and_rejects_inconsistent_declarations() {
    let fixture = Fixture::new();
    let conn = Fixture::open(&fixture.database(&[]));
    let mut stmt = conn.prepare("SELECT 0, 50000, x'0102'").unwrap();
    let mut rows = stmt.raw_query();
    let row = rows.next().unwrap().unwrap();
    let declared_from_col: i64 = row.get(1).unwrap();
    let payload = SQLitePayload::from_row(row);
    assert_eq!(payload.byte_count, 2);
    assert_eq!(payload.copy(declared_from_col as usize, 65536), None);
    assert_eq!(payload.copy(1, 65536), None);
    assert_eq!(payload.copy(2, 1), None);
    assert_eq!(payload.copy(2, 2), Some(vec![1, 2]));
}

// upstream: AntigravityLocalIntegrityTests.swift:337 schema entry column and byte limits reject before payload reads
#[test]
fn schema_entry_column_and_byte_limits_reject_before_payload_reads() {
    let fixture = Fixture::new();
    let url = fixture.database(&[]);
    let conn = Fixture::open(&url);
    Fixture::execute(
        &conn,
        "DROP TABLE gen_metadata;
         CREATE TABLE first (value);
         CREATE TABLE second (value);
         CREATE TABLE gen_metadata (idx INTEGER, data BLOB);",
    );
    Fixture::insert(&conn, 0, &Fixture::blob());
    drop(conn);

    let mut limits = Limits {
        schema_entries: 1,
        ..Limits::default()
    };
    let entries = fixture.report_with_limits(limits);
    assert_eq!(entries.coverage, Coverage::Partial);
    assert_eq!(entries.statistics.schema_entries, 2);
    assert_eq!(entries.statistics.rows, 0);

    limits.schema_entries = 128;
    limits.schema_columns = 1;
    let columns = fixture.report_with_limits(limits);
    assert_eq!(columns.coverage, Coverage::Partial);
    assert_eq!(columns.statistics.schema_columns, 2);
    assert_eq!(columns.statistics.rows, 0);

    limits.schema_columns = 64;
    let complete = fixture.report_with_limits(limits);
    assert_eq!(complete.coverage, Coverage::Complete);

    limits.schema_bytes = complete.statistics.schema_bytes - 1;
    let bytes = fixture.report_with_limits(limits);
    assert_eq!(bytes.coverage, Coverage::Partial);
    assert_eq!(bytes.statistics.rows, 0);
    assert!(bytes.statistics.schema_bytes > limits.schema_bytes);
    assert_eq!(
        bytes.statistics.sqlite_handles_opened,
        bytes.statistics.sqlite_handles_closed
    );
}

// upstream: AntigravityLocalIntegrityTests.swift:373 schema inspection honors cancellation and a pinned deadline before payload selection
#[test]
fn schema_inspection_honors_cancellation_and_a_pinned_deadline_before_payload_selection() {
    for deadline in [false, true] {
        let fixture = Fixture::new();
        let url = fixture.database(&[Fixture::blob()]);
        let now_bits = Arc::new(AtomicUsize::new(0));
        let now_bits_clock = now_bits.clone();
        let now_bits_cancel = now_bits.clone();
        let holder: Arc<Mutex<Option<Budget>>> = Arc::new(Mutex::new(None));
        let holder_clone = holder.clone();
        let limits = Limits {
            duration: 5.0,
            ..Limits::default()
        };
        let budget = Budget::new(
            limits,
            move || now_bits_clock.load(Ordering::SeqCst) as f64,
            move || {
                let cols = holder_clone
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map_or(0, |b| b.statistics().schema_columns);
                if cols == 1 {
                    if deadline {
                        now_bits_cancel.store(5, Ordering::SeqCst);
                    } else {
                        return Err(ScanFailure::Cancelled("posix-4321".to_owned()));
                    }
                }
                Ok(())
            },
        );
        *holder.lock().unwrap() = Some(budget.clone());
        let res = AntigravityLocalReader::read_databases(&[url], &budget);
        *holder.lock().unwrap() = None;
        if deadline {
            assert_eq!(res.unwrap_err(), ScanFailure::Exhausted);
        } else {
            assert_eq!(
                res.unwrap_err(),
                ScanFailure::Cancelled("posix-4321".to_owned())
            );
        }
        assert_eq!(budget.statistics().schema_columns, 1);
        assert_eq!(budget.statistics().rows, 0);
        assert_eq!(
            budget.statistics().sqlite_handles_opened,
            budget.statistics().sqlite_handles_closed
        );
    }
}

// ============================================================================
// AntigravityLocalScanTests.swift
// ============================================================================

struct ScanCancellationGate {
    state: Mutex<bool>,
    cvar: Condvar,
    release_state: Mutex<bool>,
    release_cvar: Condvar,
}

impl ScanCancellationGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(false),
            cvar: Condvar::new(),
            release_state: Mutex::new(false),
            release_cvar: Condvar::new(),
        })
    }

    fn enter(&self) {
        let mut entered = self.state.lock().unwrap();
        let first = !*entered;
        *entered = true;
        self.cvar.notify_all();
        drop(entered);
        if first {
            let mut released = self.release_state.lock().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !*released {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let (guard, _) = self
                    .release_cvar
                    .wait_timeout(released, deadline - now)
                    .unwrap();
                released = guard;
            }
        }
    }

    fn wait_until_inside(&self) {
        let mut entered = self.state.lock().unwrap();
        while !*entered {
            entered = self.cvar.wait(entered).unwrap();
        }
    }

    fn release(&self) {
        let mut released = self.release_state.lock().unwrap();
        *released = true;
        self.release_cvar.notify_all();
    }
}

// upstream: AntigravityLocalScanTests.swift:13 database cap distinguishes below exactly and above limit
#[test]
fn database_cap_distinguishes_below_exactly_and_above_limit() {
    for count in [499usize, 500, 501] {
        let fixture = Fixture::new();
        let seed = fixture.database_named("session-0", 0, &[]);
        let parent = seed.parent().unwrap().to_path_buf();
        for index in 1..count {
            fs::copy(&seed, parent.join(format!("session-{index}.db"))).unwrap();
        }
        let limits = Limits {
            duration: 60.0,
            ..Limits::default()
        };
        let report = fixture.report_with_limits(limits);
        assert_eq!(
            report.coverage,
            if count <= 500 {
                Coverage::Complete
            } else {
                Coverage::Partial
            }
        );
        assert_eq!(
            report.statistics.files,
            if count <= 500 { count } else { 0 }
        );
        assert_eq!(report.statistics.rows, 0);
        assert_eq!(
            report.statistics.sqlite_handles_opened,
            report.statistics.files
        );
        assert_eq!(
            report.statistics.sqlite_handles_closed,
            report.statistics.sqlite_handles_opened
        );
    }
}

// upstream: AntigravityLocalScanTests.swift:33 exceeding the database cap with valid history withholds the truncated report
#[test]
fn exceeding_the_database_cap_with_valid_history_withholds_the_truncated_report() {
    let fixture = Fixture::new();
    fixture.database_named("first", 0, &[Fixture::blob()]);
    let limits = Limits {
        databases: 1,
        ..Limits::default()
    };
    let exact = fixture.report_with_limits(limits);
    assert_eq!(exact.coverage, Coverage::Complete);
    assert_eq!(
        exact.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(187)
    );

    fixture.database_named("second", 0, &[Fixture::blob()]);
    let exceeded = fixture.report_with_limits(limits);
    assert_eq!(exceeded.coverage, Coverage::Partial);
    assert!(exceeded.report.data.is_empty());
    assert!(exceeded.report.summary.is_none());
    assert_eq!(exceeded.statistics.files, 0);
    assert_eq!(exceeded.statistics.sqlite_handles_opened, 0);
}

// upstream: AntigravityLocalScanTests.swift:52 history beyond the former row cap produces a complete report
#[test]
fn history_beyond_the_former_row_cap_produces_a_complete_report() {
    let fixture = Fixture::new();
    let blob_hex: String = Fixture::blob().iter().map(|b| format!("{b:02x}")).collect();
    for file in 0..6 {
        let url = fixture.database_named(&format!("session-{file}"), 0, &[]);
        let conn = Fixture::open(&url);
        Fixture::execute(
            &conn,
            &format!(
                "WITH RECURSIVE rows(idx) AS (
                    SELECT 1
                    UNION ALL SELECT idx + 1 FROM rows WHERE idx < 10000
                )
                INSERT INTO gen_metadata (idx, data)
                SELECT idx, X'{blob_hex}' FROM rows"
            ),
        );
    }

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(report.statistics.rows, 60_000);
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(60_000 * 187)
    );
    assert_eq!(
        report.statistics.sqlite_handles_opened,
        report.statistics.sqlite_handles_closed
    );
    let snapshot = fixture.snapshot();
    assert!(snapshot.history_coverage_is_established);
    assert_eq!(snapshot.last_30_days_tokens, Some(60_000 * 187));
}

// upstream: AntigravityLocalScanTests.swift:79 expanded payload and time allowances still stop at their boundaries
#[test]
fn expanded_payload_and_time_allowances_still_stop_at_their_boundaries() {
    let elapsed_bits = Arc::new(AtomicUsize::new(0));
    let elapsed_clock = elapsed_bits.clone();
    let limits = Limits::default();
    let budget = Budget::new(
        limits,
        move || f64::from_bits(elapsed_clock.load(Ordering::SeqCst) as u64),
        || Ok(()),
    );
    budget.charge_bytes(256 * 1024 * 1024).unwrap();
    elapsed_bits.store(10.0f64.to_bits() as usize, Ordering::SeqCst);
    budget.check().unwrap();
    let remaining = limits.bytes - budget.statistics().attempted_bytes;
    budget.charge_bytes(remaining).unwrap();
    assert_eq!(budget.charge_bytes(1), Err(ScanFailure::Exhausted));
    elapsed_bits.store(limits.duration.to_bits() as usize, Ordering::SeqCst);
    assert_eq!(budget.check(), Err(ScanFailure::Exhausted));
}

// upstream: AntigravityLocalScanTests.swift:93 the schema byte allowance applies to each database rather than the whole history
#[test]
fn the_schema_byte_allowance_applies_to_each_database_rather_than_the_whole_history() {
    let fixture = Fixture::new();
    fixture.database_named("session-1", 0, &[Fixture::blob()]);
    let initial = fixture.report();
    assert_eq!(initial.coverage, Coverage::Complete);
    assert_eq!(
        initial.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(187)
    );

    fixture.database_named("session-2", 0, &[Fixture::blob()]);
    fixture.database_named("session-3", 0, &[Fixture::blob()]);
    let limits = Limits {
        schema_bytes: initial.statistics.schema_bytes,
        ..Limits::default()
    };
    let report = fixture.report_with_limits(limits);
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(561)
    );
    assert_eq!(report.statistics.files, 3);
    assert!(report.statistics.schema_bytes > limits.schema_bytes);
}

// upstream: AntigravityLocalScanTests.swift:113 an oversized schema withholds only its own database and keeps the other rows as partial history
#[test]
fn an_oversized_schema_withholds_only_its_own_database_and_keeps_the_other_rows_as_partial_history()
{
    for oversized_session in ["session-0", "session-2", "session-4"] {
        let fixture = Fixture::new();
        fixture.database_named("session-1", 0, &[Fixture::blob()]);
        let initial = fixture.report();
        assert_eq!(initial.coverage, Coverage::Complete);

        let url = fixture.database_named(oversized_session, 0, &[]);
        let conn = Fixture::open(&url);
        Fixture::execute(
            &conn,
            "DROP TABLE gen_metadata;
             CREATE TABLE an_unrelated_table_with_a_long_name (value);
             CREATE TABLE gen_metadata (idx INTEGER, data BLOB);",
        );
        Fixture::insert(&conn, 0, &Fixture::blob());
        drop(conn);
        fixture.database_named("session-3", 0, &[Fixture::blob()]);
        let limits = Limits {
            schema_bytes: initial.statistics.schema_bytes,
            ..Limits::default()
        };
        let partial = fixture.report_with_limits(limits);
        assert_eq!(partial.coverage, Coverage::Partial);
        assert_eq!(
            partial.report.summary.as_ref().and_then(|s| s.total_tokens),
            Some(374)
        );
        assert_eq!(partial.statistics.files, 3);
        assert_eq!(partial.statistics.rows, 2);
        assert_eq!(
            partial.statistics.sqlite_handles_opened,
            partial.statistics.sqlite_handles_closed
        );
    }
}

// upstream: AntigravityLocalScanTests.swift:143 generation and step table inspection share one database schema allowance
#[test]
fn generation_and_step_table_inspection_share_one_database_schema_allowance() {
    let fixture = Fixture::new();
    let step_uuid = "schema-budget-step";
    fixture.database_with_steps(
        &[Fixture::root_envelope_simple(Some(step_uuid), None, None)],
        &[Fixture::step_metadata_blob(
            Some(step_uuid),
            None,
            1_787_832_000,
            250_000_000,
        )],
    );
    let complete = fixture.report();
    assert_eq!(complete.coverage, Coverage::Complete);
    assert_eq!(
        complete
            .report
            .summary
            .as_ref()
            .and_then(|s| s.total_tokens),
        Some(187)
    );

    let limits = Limits {
        schema_bytes: complete.statistics.schema_bytes - 1,
        ..Limits::default()
    };
    let partial = fixture.report_with_limits(limits);
    assert_eq!(partial.coverage, Coverage::Partial);
    assert!(partial.report.data.is_empty());
    assert_eq!(partial.statistics.rows, 1);
    assert!(partial.statistics.schema_bytes > limits.schema_bytes);
    assert_eq!(
        partial.statistics.sqlite_handles_opened,
        partial.statistics.sqlite_handles_closed
    );
}

// upstream: AntigravityLocalScanTests.swift:164 hard row exhaustion across databases withholds the truncated report
#[test]
fn hard_row_exhaustion_across_databases_withholds_the_truncated_report() {
    let fixture = Fixture::new();
    fixture.database_named("first", 0, &[Fixture::blob()]);
    fixture.database_named("second", 0, &[Fixture::blob()]);
    let limits = Limits {
        rows: 1,
        ..Limits::default()
    };
    let report = fixture.report_with_limits(limits);
    assert_eq!(report.coverage, Coverage::Partial);
    assert!(report.report.data.is_empty());
    assert!(report.report.summary.is_none());
    assert_eq!(report.statistics.files, 2);
}

// upstream: AntigravityLocalScanTests.swift:180 hard byte exhaustion across databases withholds the truncated report
#[test]
fn hard_byte_exhaustion_across_databases_withholds_the_truncated_report() {
    let fixture = Fixture::new();
    let blob = Fixture::blob();
    fixture.database_named("first", 0, std::slice::from_ref(&blob));
    fixture.database_named("second", 0, std::slice::from_ref(&blob));
    let limits = Limits {
        bytes: blob.len(),
        ..Limits::default()
    };
    let report = fixture.report_with_limits(limits);
    assert_eq!(report.coverage, Coverage::Partial);
    assert!(report.report.data.is_empty());
    assert!(report.report.summary.is_none());
    assert_eq!(report.statistics.files, 2);
}

// upstream: AntigravityLocalScanTests.swift:197 discovery stops before collecting every irrelevant filename
#[test]
fn discovery_stops_before_collecting_every_irrelevant_filename() {
    let fixture = Fixture::new();
    let roots = fixture.context().database_roots();
    let root = &roots[0];
    fs::create_dir_all(root).unwrap();
    for index in 0..20 {
        fs::write(root.join(format!("ignored-{index}.txt")), b"").unwrap();
    }
    let limits = Limits {
        directory_entries: 3,
        ..Limits::default()
    };
    let report = fixture.report_with_limits(limits);
    assert_eq!(report.coverage, Coverage::Partial);
    assert_eq!(report.statistics.directory_entries, 4);
    assert_eq!(report.statistics.files, 0);
}

// upstream: AntigravityLocalScanTests.swift:213 NULL and empty SQL rows count toward truncation and cumulative row limits
#[test]
fn null_and_empty_sql_rows_count_toward_truncation_and_cumulative_row_limits() {
    let fixture = Fixture::new();
    let url = fixture.database(&[]);
    let conn = Fixture::open(&url);
    Fixture::execute(
        &conn,
        "INSERT INTO gen_metadata VALUES (0, NULL), (1, x''), (2, NULL), (3, NULL)",
    );
    drop(conn);
    let mut limits = Limits {
        rows_per_database: 2,
        ..Limits::default()
    };
    let per_database = fixture.report_with_limits(limits);
    assert_eq!(per_database.coverage, Coverage::Partial);
    assert_eq!(per_database.statistics.rows, 3);
    limits.rows_per_database = 100;
    limits.rows = 1;
    let cumulative = fixture.report_with_limits(limits);
    assert_eq!(cumulative.coverage, Coverage::Partial);
    assert_eq!(cumulative.statistics.rows, 2);
}

// upstream: AntigravityLocalScanTests.swift:232 rejected oversized SQL rows exhaust attempted bytes before another row
#[test]
fn rejected_oversized_sql_rows_exhaust_attempted_bytes_before_another_row() {
    let fixture = Fixture::new();
    let url = fixture.database(&[]);
    let conn = Fixture::open(&url);
    Fixture::execute(
        &conn,
        "INSERT INTO gen_metadata VALUES (0, zeroblob(40)), (1, zeroblob(40)),
            (2, zeroblob(40)), (3, zeroblob(40))",
    );
    drop(conn);
    let mut limits = Limits {
        blob_bytes: 10,
        bytes: 70,
        ..Limits::default()
    };
    let report = fixture.report_with_limits(limits);
    assert_eq!(report.coverage, Coverage::Partial);
    assert_eq!(report.statistics.rows, 2);
    assert_eq!(report.statistics.attempted_bytes, 80);
    assert_eq!(report.statistics.materialized_payload_bytes, 0);
    limits.bytes = 1000;
    limits.database_bytes = 70;
    let per_database = fixture.report_with_limits(limits);
    assert_eq!(per_database.statistics.rows, 2);
    assert_eq!(per_database.coverage, Coverage::Partial);
    assert_eq!(per_database.statistics.materialized_payload_bytes, 0);
    limits.blob_bytes = 100;
    let projected = fixture.report_with_limits(limits);
    assert_eq!(projected.statistics.attempted_bytes, 80);
    assert_eq!(projected.statistics.materialized_payload_bytes, 40);
}

// upstream: AntigravityLocalScanTests.swift:262 byte budget is cumulative across separate databases
#[test]
fn byte_budget_is_cumulative_across_separate_databases() {
    let fixture = Fixture::new();
    let blob = Fixture::blob();
    fixture.database_named("a", 0, std::slice::from_ref(&blob));
    fixture.database_named("b", 0, std::slice::from_ref(&blob));
    let limits = Limits {
        bytes: blob.len(),
        ..Limits::default()
    };
    let report = fixture.report_with_limits(limits);
    assert_eq!(report.coverage, Coverage::Partial);
    assert_eq!(report.statistics.files, 2);
    assert_eq!(report.statistics.rows, 2);
    assert_eq!(report.statistics.attempted_bytes, blob.len() * 2);
    assert_eq!(report.statistics.materialized_payload_bytes, blob.len());
}

// upstream: AntigravityLocalScanTests.swift:278 expression view is rejected before evaluating its intermediate value
#[test]
fn expression_view_is_rejected_before_evaluating_its_intermediate_value() {
    let fixture = Fixture::new();
    let url = fixture.database(&[]);
    let conn = Fixture::open(&url);
    Fixture::execute(
        &conn,
        "DROP TABLE gen_metadata;
         CREATE VIEW gen_metadata AS SELECT 0 AS idx, zeroblob(2048) AS data;",
    );
    drop(conn);
    let limits = Limits {
        blob_bytes: 10,
        ..Limits::default()
    };
    let report = fixture.report_with_limits(limits);
    assert_eq!(report.coverage, Coverage::Partial);
    assert_eq!(report.statistics.rows, 0);
    assert_eq!(report.statistics.materialized_payload_bytes, 0);
    assert_eq!(
        report.statistics.sqlite_handles_opened,
        report.statistics.sqlite_handles_closed
    );
}

// upstream: AntigravityLocalScanTests.swift:297 row truncation sentinel does not materialize a rejected payload
#[test]
fn row_truncation_sentinel_does_not_materialize_a_rejected_payload() {
    let fixture = Fixture::new();
    let blob = Fixture::blob();
    fixture.database(&[blob.clone(), blob.clone()]);
    let mut limits = Limits {
        rows_per_database: 1,
        ..Limits::default()
    };
    let per_database = fixture.report_with_limits(limits);
    assert_eq!(per_database.coverage, Coverage::Partial);
    assert_eq!(per_database.statistics.rows, 2);
    assert_eq!(
        per_database.statistics.materialized_payload_bytes,
        blob.len()
    );
    limits.rows_per_database = 10;
    limits.rows = 1;
    let cumulative = fixture.report_with_limits(limits);
    assert_eq!(cumulative.coverage, Coverage::Partial);
    assert_eq!(cumulative.statistics.rows, 2);
    assert_eq!(cumulative.statistics.materialized_payload_bytes, blob.len());
}

// upstream: AntigravityLocalScanTests.swift:316 optional steps scan does not exhaust embedded timestamp usage rows
#[test]
fn optional_steps_scan_does_not_exhaust_embedded_timestamp_usage_rows() {
    let fixture = Fixture::new();
    let blob = Fixture::root_envelope_simple(None, None, Some(1_787_832_000));
    let url = fixture.database_with_steps(std::slice::from_ref(&blob), &[]);
    let conn = Fixture::open(&url);
    Fixture::execute(
        &conn,
        "INSERT INTO steps VALUES (0, zeroblob(40)), (1, zeroblob(40))",
    );
    drop(conn);
    let limits = Limits {
        rows_per_database: 1,
        ..Limits::default()
    };
    let report = fixture.report_with_limits(limits);
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(report.statistics.rows, 1);
    assert_eq!(report.statistics.attempted_bytes, blob.len());
}

// upstream: AntigravityLocalScanTests.swift:334 optional steps scan charges its rows and bytes against the shared job budget
#[test]
fn optional_steps_scan_charges_its_rows_and_bytes_against_the_shared_job_budget() {
    let fixture = Fixture::new();
    let step_uuid = "budgeted-step-uuid";
    let gen_blob = Fixture::root_envelope_simple(Some(step_uuid), None, None);
    let step_blob = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_832_000, 250_000_000);
    fixture.database_with_steps(
        std::slice::from_ref(&gen_blob),
        std::slice::from_ref(&step_blob),
    );

    assert_eq!(fixture.report().coverage, Coverage::Complete);

    let row_bound = fixture.report_with_limits(Limits {
        rows: 1,
        ..Limits::default()
    });
    assert_eq!(row_bound.coverage, Coverage::Partial);
    assert_eq!(row_bound.statistics.rows, 2);

    let byte_bound = fixture.report_with_limits(Limits {
        bytes: gen_blob.len(),
        ..Limits::default()
    });
    assert_eq!(byte_bound.coverage, Coverage::Partial);
    assert_eq!(
        byte_bound.statistics.attempted_bytes,
        gen_blob.len() + step_blob.len()
    );

    let db_byte_bound = fixture.report_with_limits(Limits {
        database_bytes: gen_blob.len() + step_blob.len() - 1,
        ..Limits::default()
    });
    assert_eq!(db_byte_bound.coverage, Coverage::Partial);
    assert_eq!(
        db_byte_bound.statistics.attempted_bytes,
        gen_blob.len() + step_blob.len()
    );
}

// upstream: AntigravityLocalScanTests.swift:363 step scan truncation cannot publish already recovered timestamps as complete
#[test]
fn step_scan_truncation_cannot_publish_already_recovered_timestamps_as_complete() {
    let fixture = Fixture::new();
    let step_uuid = "bounded-step-uuid";
    let gen_blob = Fixture::root_envelope_simple(Some(step_uuid), None, None);
    let matching_step =
        Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_832_000, 250_000_000);
    let trailing_step =
        Fixture::step_metadata_blob(Some("unrelated-step"), None, 1_787_832_000, 250_000_000);
    fixture.database_with_steps(
        std::slice::from_ref(&gen_blob),
        &[matching_step.clone(), trailing_step.clone()],
    );
    let limits = Limits {
        database_bytes: gen_blob.len() + matching_step.len(),
        ..Limits::default()
    };
    let report = fixture.report_with_limits(limits);
    assert_eq!(report.coverage, Coverage::Partial);
    assert_eq!(report.statistics.rows, 3);
    assert_eq!(
        report.statistics.attempted_bytes,
        gen_blob.len() + matching_step.len() + trailing_step.len()
    );
}

// upstream: AntigravityLocalScanTests.swift:381 secondary byte exhaustion cannot publish primary rows as partial history
#[test]
fn secondary_byte_exhaustion_cannot_publish_primary_rows_as_partial_history() {
    let fixture = Fixture::new();
    let step_uuid = "bounded-secondary-history";
    let primary = Fixture::blob();
    let pending = Fixture::root_envelope_simple(Some(step_uuid), None, None);
    let step = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_832_000, 250_000_000);
    fixture.database_with_steps(&[primary.clone(), pending.clone()], &[step]);
    assert_eq!(fixture.report().coverage, Coverage::Complete);

    let limits = Limits {
        database_bytes: primary.len() + pending.len(),
        ..Limits::default()
    };
    let report = fixture.report_with_limits(limits);
    assert_eq!(report.coverage, Coverage::Partial);
    assert!(report.report.data.is_empty());
    assert!(report.report.summary.is_none());
    assert_eq!(
        report.statistics.sqlite_handles_opened,
        report.statistics.sqlite_handles_closed
    );
}

// upstream: AntigravityLocalScanTests.swift:400 step lookup reaches beyond the primary row cap within the shared budget
#[test]
fn step_lookup_reaches_beyond_the_primary_row_cap_within_the_shared_budget() {
    let fixture = Fixture::new();
    let step_uuid = "sparse-step-uuid";
    let gen_blob = Fixture::root_envelope_simple(Some(step_uuid), None, None);
    let url = fixture.database_with_steps(&[gen_blob], &[]);
    let conn = Fixture::open(&url);
    let unrelated_count = 10_001usize;
    let unrelated =
        Fixture::step_metadata_blob(Some("unrelated-step"), None, 1_787_832_000, 250_000_000);
    Fixture::execute(&conn, "BEGIN");
    for index in 0..unrelated_count {
        Fixture::insert_step(&conn, index as i64, &unrelated);
    }
    Fixture::insert_step(
        &conn,
        unrelated_count as i64,
        &Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_832_000, 250_000_000),
    );
    Fixture::insert_step(&conn, (unrelated_count + 1) as i64, &unrelated);
    Fixture::execute(&conn, "COMMIT");
    drop(conn);

    let limits = Limits {
        duration: 30.0,
        ..Limits::default()
    };
    let report = fixture.report_with_limits(limits);
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report.report.data.first().map(|d| d.date.as_str()),
        Some("2026-08-27")
    );
    assert_eq!(report.statistics.rows, unrelated_count + 3);
}

// upstream: AntigravityLocalScanTests.swift:435 recursive aggregate view is rejected without executing its payload query
#[test]
fn recursive_aggregate_view_is_rejected_without_executing_its_payload_query() {
    let fixture = Fixture::new();
    let url = fixture.database(&[]);
    let conn = Fixture::open(&url);
    Fixture::execute(
        &conn,
        "DROP TABLE gen_metadata;
         CREATE VIEW gen_metadata AS
         WITH RECURSIVE rows(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM rows WHERE x<4999)
         SELECT sum(x) AS idx, NULL AS data FROM rows;",
    );
    let result = fixture.report();
    assert_eq!(result.coverage, Coverage::Partial);
    assert_eq!(result.statistics.rows, 0);
    assert_eq!(result.statistics.materialized_payload_bytes, 0);
    Fixture::execute(&conn, "BEGIN EXCLUSIVE; ROLLBACK");
}

// upstream: AntigravityLocalScanTests.swift:454 JSONL byte and line limits apply before whole file allocation
#[test]
fn jsonl_byte_and_line_limits_apply_before_whole_file_allocation() {
    let fixture = Fixture::new();
    let url = fixture.jsonl(&[Fixture::CACHE_USAGE]);
    let file = fs::OpenOptions::new().write(true).open(&url).unwrap();
    file.set_len(129 * 1024 * 1024).unwrap();
    drop(file);
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert_eq!(report.statistics.rows, 0);
    assert_eq!(report.statistics.attempted_bytes, 129 * 1024 * 1024);
    assert!(!fixture.snapshot().history_coverage_is_established);

    fs::write(&url, Fixture::CACHE_USAGE).unwrap();
    let mut limits = Limits {
        blob_bytes: 10,
        ..Limits::default()
    };
    assert_eq!(
        fixture.report_with_limits(limits).coverage,
        Coverage::Partial
    );
    fixture.jsonl(&[Fixture::CACHE_USAGE, Fixture::CACHE_USAGE]);
    limits.blob_bytes = 1000;
    limits.rows = 1;
    assert_eq!(fixture.report_with_limits(limits).statistics.rows, 2);
}

// upstream: AntigravityLocalScanTests.swift:476 pinned monotonic clock bounds the whole discovery and parsing job
#[test]
fn pinned_monotonic_clock_bounds_the_whole_discovery_and_parsing_job() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob()]);
    let tick = Arc::new(AtomicUsize::new(0));
    let tick_clock = tick.clone();
    let mut limits = Limits {
        duration: 3.0,
        ..Limits::default()
    };
    let report = fixture
        .report_with(
            None,
            limits,
            move || {
                let current = tick_clock.fetch_add(1, Ordering::SeqCst);
                current as f64
            },
            || Ok(()),
        )
        .unwrap();
    assert_eq!(report.coverage, Coverage::Partial);
    assert_eq!(tick.load(Ordering::SeqCst), 4);

    tick.store(0, Ordering::SeqCst);
    let tick_clock2 = tick.clone();
    limits.duration = 20.0;
    let parsing = fixture
        .report_with(
            None,
            limits,
            move || {
                let current = tick_clock2.fetch_add(1, Ordering::SeqCst);
                current as f64
            },
            || Ok(()),
        )
        .unwrap();
    assert_eq!(parsing.coverage, Coverage::Partial);
    assert_eq!(parsing.statistics.rows, 1);
    assert_eq!(
        parsing.statistics.sqlite_handles_opened,
        parsing.statistics.sqlite_handles_closed
    );
}

// upstream: AntigravityLocalScanTests.swift:500 cancellation reaches discovery SQL protobuf and JSONL loops
#[test]
fn cancellation_reaches_discovery_sql_protobuf_and_jsonl_loops() {
    let fixture = Fixture::new();
    let url = fixture.database(&[Fixture::blob(), Fixture::blob()]);
    assert_eq!(
        fixture
            .report_with(
                None,
                Limits::default(),
                || 0.0,
                || { Err(ScanFailure::Cancelled("cancelled".to_owned())) }
            )
            .unwrap_err(),
        ScanFailure::Cancelled("cancelled".to_owned())
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_clone = calls.clone();
    let holder: Arc<Mutex<Option<Budget>>> = Arc::new(Mutex::new(None));
    let holder_clone = holder.clone();
    let budget = Budget::with_cancellation(Limits::default(), move || {
        let rows = holder_clone
            .lock()
            .unwrap()
            .as_ref()
            .map_or(0, |b| b.statistics().rows);
        if rows == 1 {
            let cur = calls_clone.fetch_add(1, Ordering::SeqCst) + 1;
            if cur == 2 {
                return Err(ScanFailure::Cancelled("cancelled".to_owned()));
            }
        }
        Ok(())
    });
    *holder.lock().unwrap() = Some(budget.clone());
    assert_eq!(
        AntigravityLocalReader::read_databases(std::slice::from_ref(&url), &budget).unwrap_err(),
        ScanFailure::Cancelled("cancelled".to_owned())
    );
    assert_eq!(budget.statistics().rows, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    *holder.lock().unwrap() = None;

    let mut fields = 0usize;
    assert_eq!(
        AntigravityProtoReader::parse_turn(&Fixture::blob(), || {
            fields += 1;
            if fields == 6 {
                Err(ScanFailure::Cancelled("cancelled".to_owned()))
            } else {
                Ok(())
            }
        })
        .unwrap_err(),
        ScanFailure::Cancelled("cancelled".to_owned())
    );

    let cache = fixture.jsonl(&[Fixture::CACHE_USAGE]);
    let json_calls = Arc::new(AtomicUsize::new(0));
    let json_calls_clone = json_calls.clone();
    let json_budget = Budget::with_cancellation(Limits::default(), move || {
        let cur = json_calls_clone.fetch_add(1, Ordering::SeqCst) + 1;
        if cur == 6 {
            Err(ScanFailure::Cancelled("cancelled".to_owned()))
        } else {
            Ok(())
        }
    });
    assert_eq!(
        AntigravityLocalReader::read_jsonl(&[cache], &json_budget).unwrap_err(),
        ScanFailure::Cancelled("cancelled".to_owned())
    );
}

// upstream: AntigravityLocalScanTests.swift:539 executor cancellation drains a running single database reader
#[test]
fn executor_cancellation_drains_a_running_single_database_reader() {
    let fixture = Fixture::new();
    let url = fixture.database(&[Fixture::blob()]);
    let gate = ScanCancellationGate::new();
    let cancelled = Arc::new(AtomicBool::new(false));
    let gate_worker = gate.clone();
    let cancelled_worker = cancelled.clone();
    let url_worker = url.clone();

    let handle = std::thread::spawn(move || {
        let holder: Arc<Mutex<Option<Budget>>> = Arc::new(Mutex::new(None));
        let holder_clone = holder.clone();
        let budget = Budget::with_cancellation(Limits::default(), move || {
            let rows = holder_clone
                .lock()
                .unwrap()
                .as_ref()
                .map_or(0, |b| b.statistics().rows);
            if rows == 1 {
                gate_worker.enter();
            }
            if cancelled_worker.load(Ordering::SeqCst) {
                return Err(ScanFailure::Cancelled("cancelled".to_owned()));
            }
            Ok(())
        });
        *holder.lock().unwrap() = Some(budget.clone());
        let res =
            AntigravityLocalReader::read_databases(&[url_worker], &budget).map(|s| s.is_complete);
        *holder.lock().unwrap() = None;
        res
    });

    gate.wait_until_inside();
    cancelled.store(true, Ordering::SeqCst);
    gate.release();
    assert_eq!(
        handle.join().unwrap(),
        Err(ScanFailure::Cancelled("cancelled".to_owned()))
    );
    let conn = Fixture::open(&url);
    Fixture::execute(&conn, "BEGIN EXCLUSIVE; ROLLBACK");
    drop(conn);
    assert!(fixture.snapshot().history_coverage_is_established);
}

// upstream: AntigravityLocalScanTests.swift:568 real fetcher cancellation is bridged to the scan executor
#[test]
fn real_fetcher_cancellation_is_bridged_to_the_scan_executor() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob()]);
    let gate = ScanCancellationGate::new();
    let cancelled = Arc::new(AtomicBool::new(false));
    let gate_worker = gate.clone();
    let cancelled_worker = cancelled.clone();
    let ctx = fixture.context();
    let cache_root = fixture.root.join("scanner-cache");

    let handle = std::thread::spawn(move || {
        AntigravityLocalReader::load_token_snapshot(
            &ctx,
            Fixture::now(),
            30,
            0,
            Some(&cache_root),
            move || {
                gate_worker.enter();
                if cancelled_worker.load(Ordering::SeqCst) {
                    return Err(ScanFailure::Cancelled("cancelled".to_owned()));
                }
                Ok(())
            },
        )
    });

    gate.wait_until_inside();
    cancelled.store(true, Ordering::SeqCst);
    gate.release();
    assert_eq!(
        handle.join().unwrap(),
        Err(ScanFailure::Cancelled("cancelled".to_owned()))
    );
}

// ============================================================================
// AntigravityLocalReaderTests.swift
// ============================================================================

const VERTEX_CATALOG_JSON: &str = r#"{
    "google-vertex": {
        "id": "google-vertex",
        "name": "Vertex",
        "models": {
            "openai/gpt-oss-120b-maas": {
                "id": "openai/gpt-oss-120b-maas",
                "cost": {"input": 0.09, "output": 0.36}
            }
        }
    }
}"#;

const RESELLER_CATALOG_JSON: &str = r#"{
    "fixture-reseller": {
        "id": "fixture-reseller",
        "name": "Fixture Reseller",
        "models": {
            "gpt-oss-120b": {
                "id": "gpt-oss-120b",
                "cost": {"input": 5, "output": 6}
            }
        }
    }
}"#;

fn save_catalog(json: &str, fixture: &Fixture) {
    let catalog: ModelsDevCatalog = serde_json::from_str(json).unwrap();
    let cache_root = fixture.root.join("scanner-cache");
    assert!(ModelsDevCache::save(&catalog, Fixture::now(), &cache_root));
}

fn malformed_blobs() -> Vec<Vec<u8>> {
    let valid = Fixture::blob();
    let timestamp = Fixture::message(9, &Fixture::message(4, &[0x10, 0x80]));
    vec![
        [Fixture::message(1, &[0x22, 0x80]), valid.clone()].concat(),
        [Fixture::message(1, &timestamp), valid.clone()].concat(),
        [
            Fixture::message(
                1,
                &Fixture::message(9, &Fixture::message(4, &Fixture::varint(2, 1_000_000_000))),
            ),
            valid.clone(),
        ]
        .concat(),
        [
            Fixture::message(1, &Fixture::message(19, &[0xFF])),
            valid.clone(),
        ]
        .concat(),
        [
            Fixture::message(1, &Fixture::message(4, &Fixture::varint(1, u64::MAX))),
            valid.clone(),
        ]
        .concat(),
        Fixture::message(
            1,
            &Fixture::message(4, &[Fixture::varint(2, 4), vec![0x5A, 0x80]].concat()),
        ),
        Fixture::message(
            1,
            &Fixture::message(4, &[vec![0x08], vec![0xFF; 10]].concat()),
        ),
        [Fixture::message(1, &Fixture::varint(4, 1)), valid.clone()].concat(),
        [Fixture::message(1, &[0]), valid].concat(),
        Fixture::blob_custom(
            Some("fixture-model-a"),
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            None,
            None,
        ),
        [
            Fixture::blob_custom(
                Some("fixture-model-a"),
                Some("Fixture model"),
                1298,
                100,
                30,
                50,
                7,
                None,
                None,
            ),
            Fixture::message(
                1,
                &Fixture::message(9, &Fixture::message(10, &[1, 2, 3, 4, 5, 6, 7, 8])),
            ),
        ]
        .concat(),
        Fixture::blob_custom(
            Some("fixture-model-a"),
            Some("Fixture model"),
            1298,
            i64::MAX as u64,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        ),
    ]
}

// upstream: AntigravityLocalReaderTests.swift:13 literal synthetic schema example has independently calculated counts and time
#[test]
fn literal_synthetic_schema_example_has_independently_calculated_counts_and_time() {
    let bytes: Vec<u8> = vec![
        0x0A, 0x1B, 0x22, 0x0A, 0x08, 0x0B, 0x10, 0x64, 0x28, 0x32, 0x48, 0x1E, 0x50, 0x07, 0x4A,
        0x0D, 0x22, 0x0B, 0x08, 0xC0, 0xCD, 0xC0, 0xD4, 0x06, 0x10, 0x80, 0xE5, 0x9A, 0x77,
    ];
    let fixture = Fixture::new();
    fixture.database(&[bytes]);
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    let first = &report.report.data[0];
    assert_eq!(first.date, "2026-08-27");
    assert_eq!(first.input_tokens, Some(100));
    assert_eq!(first.output_tokens, Some(7));
    assert_eq!(first.reasoning_tokens, Some(30));
    assert_eq!(
        first.model_breakdowns.as_ref().unwrap()[0].model_name,
        "unknown"
    );
    assert_eq!(fixture.snapshot().last_30_days_tokens, Some(187));
}

// upstream: AntigravityLocalReaderTests.swift:33 empty optional model metadata remains absent
#[test]
fn empty_optional_model_metadata_remains_absent() {
    let fixture = Fixture::new();
    let blob = Fixture::blob_custom(
        Some(""),
        Some("  "),
        1298,
        100,
        30,
        50,
        7,
        None,
        Some(1_787_832_000),
    );
    let turn = AntigravityProtoReader::parse_turn(&blob, || Ok::<(), ScanFailure>(()))
        .unwrap()
        .unwrap();
    assert_eq!(turn.model, None);
    assert_eq!(turn.label, None);
    let second = Fixture::blob_custom(
        Some("fixture-model-b"),
        Some(""),
        1298,
        100,
        30,
        50,
        7,
        None,
        Some(1_787_832_000),
    );
    fixture.database(&[blob, second]);
    let snapshot = fixture.snapshot();
    assert!(snapshot.history_coverage_is_established);
    assert_eq!(
        snapshot.daily[0]
            .model_breakdowns
            .as_ref()
            .unwrap()
            .iter()
            .map(|b| b.model_name.as_str())
            .collect::<Vec<_>>(),
        vec!["unknown", "fixture-model-b"]
    );
}

// upstream: AntigravityLocalReaderTests.swift:46 malformed earlier embedded envelope cannot be hidden by a valid later envelope
#[test]
fn malformed_earlier_embedded_envelope_cannot_be_hidden_by_a_valid_later_envelope() {
    let malformed = [Fixture::message(1, &[0x22, 0x80]), Fixture::blob()].concat();
    assert_eq!(
        AntigravityProtoReader::parse_turn(&malformed, || Ok::<(), ScanFailure>(())).unwrap(),
        None
    );
}

// upstream: AntigravityLocalReaderTests.swift:52 shared token mix overflow cannot revive on a third day
#[test]
fn shared_token_mix_overflow_cannot_revive_on_a_third_day() {
    let mut mix = CostUsageTokenMix::new(Some((i64::MAX as usize) - 1), Some(2), None, None, None);
    mix.merge(CostUsageTokenMix::new(Some(2), Some(3), None, None, None));
    mix.merge(CostUsageTokenMix::new(Some(7), Some(4), None, None, None));
    assert_eq!(mix.input_tokens, None);
    assert_eq!(mix.output_tokens, Some(9));
    let mut receiver = CostUsageTokenMix::new(Some(4), None, None, None, None);
    receiver.merge(mix.clone());
    assert_eq!(receiver.input_tokens, None);
    let json_val: serde_json::Value = serde_json::to_value(&mix).unwrap();
    assert!(json_val.get("overflowedClasses").is_none());
    let mut absent = CostUsageTokenMix::default();
    absent.merge(CostUsageTokenMix::new(Some(8), None, None, None, None));
    absent.merge(CostUsageTokenMix::default());
    assert_eq!(absent.input_tokens, Some(8));
}

// upstream: AntigravityLocalReaderTests.swift:70 production discovery report and fetcher retain independent token counts without pricing
#[test]
fn production_discovery_report_and_fetcher_retain_independent_token_counts_without_pricing() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob_response("a"), Fixture::blob_response("b")]);
    fixture.database_named(
        "session-b",
        1,
        &[
            Fixture::blob_custom(
                None,
                Some("Fixture model"),
                1298,
                100,
                30,
                50,
                7,
                Some("a"),
                Some(1_787_832_000),
            ),
            Fixture::blob_response("b"),
        ],
    );
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(748)
    );
    assert_eq!(report.report.data[0].request_count, Some(4));
    assert_eq!(
        report.report.data[0].model_breakdowns.as_ref().unwrap()[0].request_count,
        Some(4)
    );
    let snapshot = fixture.snapshot();
    assert_eq!(snapshot.last_30_days_tokens, Some(748));
    assert_eq!(snapshot.session_tokens, Some(748));
    assert!(snapshot.history_coverage_is_established);
    assert_eq!(snapshot.last_30_days_cost_usd, None);
    assert_eq!(snapshot.session_cost_usd, None);
    assert_eq!(snapshot.cost_provenance, CostProvenance::Unknown);
    assert!(snapshot.daily.iter().all(|d| d.cost_usd.is_none()));
    assert!(
        snapshot
            .daily
            .iter()
            .flat_map(|d| d.model_breakdowns.as_deref().unwrap_or(&[]))
            .all(|b| b.cost_usd.is_none())
    );
    let window = snapshot.summary(30, 0);
    assert_eq!(window.coverage, CostUsageCoverageCounts::new(0, 4, 0, 0));
    assert_eq!(window.total_requests, Some(4));
}

// upstream: AntigravityLocalReaderTests.swift:96 known model gets list price estimate while unknown model stays unpriced
#[test]
fn known_model_gets_list_price_estimate_while_unknown_model_stays_unpriced() {
    let fixture = Fixture::new();
    save_catalog(
        r#"{
            "google": {
                "id": "google",
                "name": "Google",
                "models": {
                    "gemini-fixture-a": {
                        "id": "gemini-fixture-a",
                        "cost": {"input": 1, "output": 2, "cache_read": 0.2}
                    }
                }
            }
        }"#,
        &fixture,
    );
    fixture.database(&[
        Fixture::blob_custom(
            Some("gemini-fixture-a"),
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        ),
        Fixture::blob_custom(
            Some("fixture-unpriced"),
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        ),
    ]);

    let snapshot = fixture.snapshot();
    let expected = 100e-6 + 50.0 * 0.2e-6 + 37.0 * 2e-6;
    assert!((snapshot.last_30_days_cost_usd.unwrap() - expected).abs() < 1e-9);
    assert!((snapshot.session_cost_usd.unwrap() - expected).abs() < 1e-9);
    assert_eq!(snapshot.cost_provenance, CostProvenance::ListPriceEstimate);
    let breakdowns = snapshot.daily[0].model_breakdowns.as_ref().unwrap();
    assert!((breakdowns[0].cost_usd.unwrap() - expected).abs() < 1e-9);
    assert_eq!(breakdowns.last().unwrap().cost_usd, None);
    assert_eq!(
        snapshot.summary(30, 0).coverage,
        CostUsageCoverageCounts::new(0, 1, 0, 1)
    );
}

// upstream: AntigravityLocalReaderTests.swift:131 routing variants price from their base model without widening shared Claude pricing
#[test]
fn routing_variants_price_from_their_base_model_without_widening_shared_claude_pricing() {
    let fixture = Fixture::new();
    save_catalog(
        r#"{
            "google": {
                "id": "google",
                "name": "Google",
                "models": {
                    "gemini-fixture-a": {
                        "id": "gemini-fixture-a",
                        "cost": {"input": 1, "output": 2, "cache_read": 0.2}
                    }
                }
            }
        }"#,
        &fixture,
    );
    fixture.database(&[Fixture::blob_custom(
        Some("gemini-fixture-a-tiered"),
        Some("Fixture model"),
        1298,
        100,
        30,
        50,
        7,
        None,
        Some(1_787_832_000),
    )]);

    let snapshot = fixture.snapshot();
    let expected = 100e-6 + 50.0 * 0.2e-6 + 37.0 * 2e-6;
    assert!((snapshot.last_30_days_cost_usd.unwrap() - expected).abs() < 1e-9);
    assert_eq!(
        snapshot.daily[0].model_breakdowns.as_ref().unwrap()[0].model_name,
        "gemini-fixture-a-tiered"
    );

    assert_eq!(
        AntigravityLocalReader::pricing_base_model_id("gemini-3.8-flash-tiered").as_deref(),
        Some("gemini-3.8-flash")
    );
    assert_eq!(
        AntigravityLocalReader::pricing_base_model_id("gemini-3.1-pro-low").as_deref(),
        Some("gemini-3.1-pro-preview")
    );
    assert_eq!(
        AntigravityLocalReader::pricing_base_model_id("claude-opus-4-6-thinking").as_deref(),
        Some("claude-opus-4-6")
    );
    assert_eq!(
        AntigravityLocalReader::pricing_base_model_id("gemini-3.8-flash"),
        None
    );
    assert_eq!(AntigravityLocalReader::pricing_base_model_id("-low"), None);

    let claude_targets =
        AntigravityLocalReader::claude_models_dev_pricing_targets("claude-fixture-9-thinking");
    assert!(!claude_targets.is_empty());
    assert!(!claude_targets.iter().any(|(_, m)| m == "claude-fixture-9"));
}

// upstream: AntigravityLocalReaderTests.swift:176 historical long context prices use event time for exact and routed models
#[test]
fn historical_long_context_prices_use_event_time_for_exact_and_routed_models() {
    for model in ["claude-sonnet-4-6", "claude-sonnet-4-6-thinking"] {
        let fixture = Fixture::new();
        fixture.database(&[
            Fixture::blob_custom(
                Some(model),
                Some("Fixture model"),
                0,
                250_000,
                0,
                0,
                0,
                None,
                Some(1_771_588_800),
            ),
            Fixture::blob_custom(
                Some(model),
                Some("Fixture model"),
                0,
                250_000,
                0,
                0,
                0,
                None,
                Some(1_773_446_400),
            ),
        ]);
        let pricing_root = fixture.root.join("pricing");
        let result = AntigravityLocalReader::make_daily_report_with_status(
            &fixture.context(),
            0,
            true,
            Some(&pricing_root),
            Limits::default(),
            || 0.0,
            || Ok(()),
        )
        .unwrap();
        let mut entries = result.report.data;
        entries.sort_by(|a, b| a.date.cmp(&b.date));
        let costs: Vec<Option<f64>> = entries.iter().map(|e| e.cost_usd).collect();
        assert_eq!(costs, vec![Some(1.5), Some(0.75)]);
    }
}

// upstream: AntigravityLocalReaderTests.swift:206 a partial scan keeps its rows and marks the snapshot as partially scanned
#[test]
fn a_partial_scan_keeps_its_rows_and_marks_the_snapshot_as_partially_scanned() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob(), vec![0x08, 0xFF]]);

    let result = fixture.report();
    assert_eq!(result.coverage, Coverage::Partial);
    assert!(!result.report.data.is_empty());

    let snapshot = fixture.snapshot();
    assert_eq!(snapshot.last_30_days_tokens, Some(187));
    assert!(!snapshot.history_coverage_is_established);
    assert!(snapshot.history_scan_is_partial);
    assert!(!snapshot.history_is_fully_scanned());
}

// upstream: AntigravityLocalReaderTests.swift:223 complete empty out of window absent and corrupt sources remain distinct
#[test]
fn complete_empty_out_of_window_absent_and_corrupt_sources_remain_distinct() {
    let fixture = Fixture::new();
    assert_eq!(fixture.report().coverage, Coverage::Unavailable);
    assert!(!fixture.snapshot().history_coverage_is_established);
    let url = fixture.database(&[]);
    assert_eq!(fixture.report().coverage, Coverage::Complete);
    assert_eq!(fixture.snapshot().last_30_days_tokens, Some(0));
    let conn = Fixture::open(&url);
    Fixture::insert(
        &conn,
        0,
        &Fixture::blob_custom(
            Some("fixture-model-a"),
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_600_000_000),
        ),
    );
    drop(conn);
    let outside = fixture.snapshot();
    assert!(outside.daily.is_empty());
    assert!(outside.history_coverage_is_established);
    assert_eq!(outside.last_30_days_tokens, Some(0));
    fs::write(&url, b"not a database").unwrap();
    assert_eq!(fixture.report().coverage, Coverage::Partial);
    let corrupt = fixture.snapshot();
    assert!(!corrupt.history_coverage_is_established);
    assert_eq!(corrupt.session_tokens, None);
    assert_eq!(corrupt.last_30_days_tokens, None);
}

// upstream: AntigravityLocalReaderTests.swift:246 SQLite authority never permits smaller stale JSONL replacement
#[test]
fn sqlite_authority_never_permits_smaller_stale_jsonl_replacement() {
    let fixture = Fixture::new();
    fixture.jsonl(&[Fixture::CACHE_USAGE]);
    assert_eq!(fixture.snapshot().last_30_days_tokens, Some(180));
    fixture.database(&[Fixture::blob(), Fixture::blob()]);
    assert_eq!(fixture.snapshot().last_30_days_tokens, Some(374));
    let invalid = fixture.database_named("broken", 0, &[]);
    fs::write(&invalid, b"broken").unwrap();
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(374)
    );
    let snapshot = fixture.snapshot();
    assert!(!snapshot.history_coverage_is_established);
    assert!(snapshot.history_scan_is_partial);
    assert!(!snapshot.daily.is_empty());
    assert_eq!(snapshot.last_30_days_tokens, Some(374));
}

// upstream: AntigravityLocalReaderTests.swift:267 one explicit environment preserves both recognized overrides
#[test]
fn one_explicit_environment_preserves_both_recognized_overrides() {
    let fixture = Fixture::new();
    let relocated = fixture.root.join("relocated-tokscale");
    fixture.jsonl_in(
        &[Fixture::CACHE_USAGE],
        "cache",
        Some(&relocated.join("antigravity-cache/sessions")),
    );
    let mut env = fixture.environment();
    env.insert(
        "TOKSCALE_CONFIG_DIR".to_owned(),
        relocated.to_string_lossy().into_owned(),
    );
    assert_eq!(
        fixture
            .snapshot_with_env(Some(env.clone()))
            .last_30_days_tokens,
        Some(180)
    );
    let original = fixture.database(&[Fixture::blob()]);
    let gemini = fixture.root.join("relocated-gemini");
    let conversations = gemini.join("antigravity-cli/conversations");
    fs::create_dir_all(&conversations).unwrap();
    fs::rename(&original, conversations.join("session-a.db")).unwrap();
    env.insert(
        "GEMINI_CLI_HOME".to_owned(),
        gemini.to_string_lossy().into_owned(),
    );
    assert_eq!(
        fixture.snapshot_with_env(Some(env)).last_30_days_tokens,
        Some(187)
    );
}

// upstream: AntigravityLocalReaderTests.swift:285 discovery errors are partial and block fallback even alongside a healthy root
#[test]
fn discovery_errors_are_partial_and_block_fallback_even_alongside_a_healthy_root() {
    let fixture = Fixture::new();
    let roots = fixture.context().database_roots();
    let root = &roots[0];
    fs::create_dir_all(root.parent().unwrap()).unwrap();
    fs::write(root, b"directory replaced by file").unwrap();
    fixture.jsonl(&[Fixture::CACHE_USAGE]);
    assert_eq!(fixture.report().coverage, Coverage::Partial);
    assert!(!fixture.snapshot().history_coverage_is_established);
    fixture.database_named("healthy", 1, &[Fixture::blob()]);
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(187)
    );
}

// upstream: AntigravityLocalReaderTests.swift:300 copied session rows deduplicate without dropping legitimate ID-less rows or cross-session IDs
#[test]
fn copied_session_rows_deduplicate_without_dropping_legitimate_id_less_rows_or_cross_session_ids() {
    let fixture = Fixture::new();
    let blobs = vec![
        Fixture::blob(),
        Fixture::blob(),
        Fixture::blob_response("same"),
        Fixture::blob_response("same"),
    ];
    let original = fixture.database(&blobs);
    let roots = fixture.context().database_roots();
    let root1 = &roots[1];
    fs::create_dir_all(root1).unwrap();
    fs::copy(&original, root1.join(original.file_name().unwrap())).unwrap();
    fixture.database_named("session-b", 2, &[Fixture::blob_response("same")]);
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(748)
    );
    assert_eq!(report.report.data[0].request_count, Some(4));
}

// upstream: AntigravityLocalReaderTests.swift:315 invalid first copy cannot reserve identity and conflicting copies are partial
#[test]
fn invalid_first_copy_cannot_reserve_identity_and_conflicting_copies_are_partial() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob_custom(
        Some("fixture-model-a"),
        Some("Fixture model"),
        1298,
        100,
        30,
        50,
        7,
        Some("response"),
        None,
    )]);
    fixture.database_at_root(1, &[Fixture::blob_response("response")]);
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(187)
    );
    fixture.database_at_root(
        2,
        &[Fixture::blob_custom(
            Some("fixture-model-a"),
            Some("Fixture model"),
            1298,
            200,
            30,
            50,
            7,
            Some("response"),
            Some(1_787_832_000),
        )],
    );
    assert_eq!(fixture.report().coverage, Coverage::Partial);
}

// upstream: AntigravityLocalReaderTests.swift:327 failed daily aggregation does not reserve a response needed by a later valid row
#[test]
fn failed_daily_aggregation_does_not_reserve_a_response_needed_by_a_later_valid_row() {
    let fixture = Fixture::new();
    fixture.database(&[
        Fixture::blob_custom(
            Some("fixture-model-a"),
            Some("Fixture model"),
            0,
            (i64::MAX - 1) as u64,
            0,
            0,
            0,
            None,
            Some(1_787_745_600),
        ),
        Fixture::blob_custom(
            Some("fixture-model-a"),
            Some("Fixture model"),
            0,
            2,
            0,
            0,
            0,
            Some("retry"),
            Some(1_787_745_600),
        ),
        Fixture::blob_custom(
            Some("fixture-model-a"),
            Some("Fixture model"),
            0,
            7,
            0,
            0,
            0,
            Some("retry"),
            Some(1_787_832_000),
        ),
    ]);
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert_eq!(report.report.data.len(), 2);
    assert_eq!(report.report.data.last().unwrap().input_tokens, Some(7));
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        None
    );
}

// upstream: AntigravityLocalReaderTests.swift:355 label conflicts remain sticky and raw historical identities are not canonicalized
#[test]
fn label_conflicts_remain_sticky_and_raw_historical_identities_are_not_canonicalized() {
    let fixture = Fixture::new();
    fixture.database(&[
        Fixture::blob_custom(
            Some("fixture-historical-a"),
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        ),
        Fixture::blob_custom(
            Some("fixture-historical-b"),
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        ),
        Fixture::blob_custom(
            Some("fixture-historical-a"),
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        ),
        Fixture::blob_custom(
            None,
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        ),
    ]);
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    let breakdowns = report.report.data[0].model_breakdowns.as_ref().unwrap();
    assert_eq!(
        breakdowns
            .iter()
            .map(|b| b.model_name.as_str())
            .collect::<Vec<_>>(),
        vec!["fixture-historical-a", "fixture-historical-b", "unknown"]
    );
    assert_eq!(
        breakdowns
            .iter()
            .map(|b| b.request_count)
            .collect::<Vec<_>>(),
        vec![Some(2), Some(1), Some(1)]
    );
}

// upstream: AntigravityLocalReaderTests.swift:369 raw model conflicts are detected before display whitespace normalization
#[test]
fn raw_model_conflicts_are_detected_before_display_whitespace_normalization() {
    let fixture = Fixture::new();
    fixture.database(&[
        Fixture::blob_custom(
            Some("fixture-a"),
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        ),
        Fixture::blob_custom(
            Some(" fixture-a "),
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        ),
        Fixture::blob_custom(
            None,
            Some("Fixture model"),
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        ),
    ]);
    let report = fixture.report();
    let breakdowns = report.report.data[0].model_breakdowns.as_ref().unwrap();
    assert_eq!(
        breakdowns
            .iter()
            .map(|b| b.model_name.as_str())
            .collect::<Vec<_>>(),
        vec!["fixture-a", "unknown"]
    );
    assert_eq!(
        breakdowns
            .iter()
            .map(|b| b.request_count)
            .collect::<Vec<_>>(),
        vec![Some(2), Some(1)]
    );
}

// upstream: AntigravityLocalReaderTests.swift:381 unreadable recognized directory does not disappear from coverage
#[test]
fn unreadable_recognized_directory_does_not_disappear_from_coverage() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob()]);
    fixture.database_named("healthy", 1, &[Fixture::blob()]);
    fixture.jsonl(&[Fixture::CACHE_USAGE]);
    let roots = fixture.context().database_roots();
    let root = &roots[0];
    fs::set_permissions(root, fs::Permissions::from_mode(0o0)).unwrap();
    let report = fixture.report();
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(report.coverage, Coverage::Partial);
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(187)
    );
}

// upstream: AntigravityLocalReaderTests.swift:395 copied JSONL uses explicit session identity and preserves separate ID-less turns
#[test]
fn copied_jsonl_uses_explicit_session_identity_and_preserves_separate_id_less_turns() {
    let fixture = Fixture::new();
    let source = fixture.jsonl(&[Fixture::CACHE_USAGE, Fixture::CACHE_USAGE]);
    fs::copy(&source, source.parent().unwrap().join("copy.jsonl")).unwrap();
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(360)
    );
    assert_eq!(report.report.data[0].request_count, Some(2));
    let distinct = Fixture::CACHE_USAGE.replace("cache-session", "second-session");
    fixture.jsonl_in(&[distinct.as_str()], "second", None);
    assert_eq!(
        fixture
            .report()
            .report
            .summary
            .as_ref()
            .and_then(|s| s.total_tokens),
        Some(540)
    );
}

// upstream: AntigravityLocalReaderTests.swift:411 repeated known messages merge scalar fields after validating every occurrence
#[test]
fn repeated_known_messages_merge_scalar_fields_after_validating_every_occurrence() {
    let usage = [
        Fixture::message(
            4,
            &[Fixture::varint(1, 1298), Fixture::varint(2, 100)].concat(),
        ),
        Fixture::message(4, &Fixture::varint(5, 50)),
    ]
    .concat();
    let time = [
        Fixture::message(9, &Fixture::message(4, &Fixture::varint(1, 1_787_832_000))),
        Fixture::message(9, &Fixture::message(4, &Fixture::varint(2, 123_000_000))),
    ]
    .concat();
    let bytes = [Fixture::message(1, &usage), Fixture::message(1, &time)].concat();
    let turn = AntigravityProtoReader::parse_turn(&bytes, || Ok::<(), ScanFailure>(()))
        .unwrap()
        .unwrap();
    let parsed_usage = turn.usage.as_ref().unwrap();
    assert_eq!(parsed_usage.model_id, Some(1298));
    assert_eq!(parsed_usage.new_input, 100);
    assert_eq!(parsed_usage.cache_read, 50);
    assert_eq!(turn.timestamp_ms, Some(1_787_832_000_123));
    let fixture = Fixture::new();
    fixture.database(&[bytes]);
    assert_eq!(
        fixture
            .report()
            .report
            .summary
            .as_ref()
            .and_then(|s| s.total_tokens),
        Some(150)
    );
}

// upstream: AntigravityLocalReaderTests.swift:428 malformed or unsupported event layouts cannot establish coverage
#[test]
fn malformed_or_unsupported_event_layouts_cannot_establish_coverage() {
    for blob in malformed_blobs() {
        let fixture = Fixture::new();
        fixture.database(&[Fixture::blob(), blob]);
        assert_eq!(fixture.report().coverage, Coverage::Partial);
        assert!(!fixture.snapshot().history_coverage_is_established);
    }
}

// upstream: AntigravityLocalReaderTests.swift:457 present invalid JSONL optional counter makes coverage incomplete
#[test]
fn present_invalid_jsonl_optional_counter_makes_coverage_incomplete() {
    for key in ["cacheRead", "cacheWrite", "reasoning"] {
        for invalid in ["\"invalid\"", "null", "true", "-1", "1.5", "1e300"] {
            let fixture = Fixture::new();
            let line = format!(
                "{{\"type\":\"usage\",\"sessionId\":\"cache-session\",\"input\":10,\"output\":2,\"timestamp\":1787832000250,\"{key}\":{invalid}}}"
            );
            fixture.jsonl(&[Fixture::CACHE_USAGE, line.as_str()]);
            assert_eq!(fixture.report().coverage, Coverage::Partial);
            assert!(!fixture.snapshot().history_coverage_is_established);
        }
    }
}

// upstream: AntigravityLocalReaderTests.swift:471 invalid JSONL timestamps cannot trap or round into event dates
#[test]
fn invalid_jsonl_timestamps_cannot_trap_or_round_into_event_dates() {
    for timestamp in ["1e300", "1787832000250.5", "true", "null", "-1"] {
        let fixture = Fixture::new();
        let line = format!(
            "{{\"type\":\"usage\",\"sessionId\":\"cache-session\",\"input\":10,\"output\":2,\"timestamp\":{timestamp}}}"
        );
        fixture.jsonl(&[line.as_str()]);
        assert_eq!(fixture.report().coverage, Coverage::Partial);
        assert!(!fixture.snapshot().history_coverage_is_established);
    }
}

// upstream: AntigravityLocalReaderTests.swift:480 JSONL producer metadata supplies session and model but never event time
#[test]
fn jsonl_producer_metadata_supplies_session_and_model_but_never_event_time() {
    let fixture = Fixture::new();
    fixture.jsonl(&[
        "{\"type\":\"session_meta\",\"sessionId\":\"cache-session\",\"modelId\":\"fixture-cache\",\"timestamp\":1700000000000}",
        "{\"type\":\"usage\",\"sessionId\":\"cache-session\",\"input\":10,\"output\":2,\"timestamp\":1787832000250,\"modelId\":\"\"}",
    ]);
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(12)
    );
    assert_eq!(
        report.report.data[0].model_breakdowns.as_ref().unwrap()[0].model_name,
        "fixture-cache"
    );
    fixture.jsonl(&[
        "{\"type\":\"session_meta\",\"sessionId\":\"cache-session\",\"timestamp\":1787832000250}",
        Fixture::CACHE_USAGE,
    ]);
    let same_timestamp = fixture.report();
    assert_eq!(same_timestamp.coverage, Coverage::Complete);
    assert_eq!(
        same_timestamp
            .report
            .summary
            .as_ref()
            .and_then(|s| s.total_tokens),
        Some(180)
    );
    let snapshot = fixture.snapshot();
    assert!(snapshot.history_coverage_is_established);
    assert_eq!(snapshot.last_30_days_tokens, Some(180));
    fixture.jsonl(&[
        "{\"type\":\"session_meta\",\"sessionId\":\"cache-session\",\"timestamp\":1787832000250}",
        "{\"type\":\"usage\",\"sessionId\":\"cache-session\",\"input\":10,\"output\":2}",
    ]);
    assert_eq!(fixture.report().coverage, Coverage::Partial);
    assert!(!fixture.snapshot().history_coverage_is_established);
}

// upstream: AntigravityLocalReaderTests.swift:510 declared linked roots overrides and session files retain normal path semantics
#[test]
fn declared_linked_roots_overrides_and_session_files_retain_normal_path_semantics() {
    let fixture = Fixture::new();
    let original = fixture.database(&[Fixture::blob()]);
    let external = fixture.root.join("owned-source");
    let orig_parent = original.parent().unwrap().to_path_buf();
    fs::rename(&orig_parent, &external).unwrap();
    std::os::unix::fs::symlink(&external, &orig_parent).unwrap();
    assert_eq!(fixture.snapshot().last_30_days_tokens, Some(187));

    let linked_gemini = fixture.root.join("linked-gemini");
    std::os::unix::fs::symlink(fixture.root.join(".gemini"), &linked_gemini).unwrap();
    let mut linked_env = fixture.environment();
    linked_env.insert(
        "GEMINI_CLI_HOME".to_owned(),
        linked_gemini.to_string_lossy().into_owned(),
    );
    assert_eq!(
        fixture
            .snapshot_with_env(Some(linked_env.clone()))
            .last_30_days_tokens,
        Some(187)
    );

    let linked_file = fixture.root.join("owned-session-data");
    fs::rename(external.join("session-a.db"), &linked_file).unwrap();
    std::os::unix::fs::symlink(&linked_file, external.join("session-a.db")).unwrap();
    assert_eq!(fixture.snapshot().last_30_days_tokens, None);
    assert!(!fixture.snapshot().history_coverage_is_established);

    let cache = Fixture::new();
    let cache_file = cache.jsonl(&[Fixture::CACHE_USAGE]);
    let linked_config = cache.root.join("linked-config");
    std::os::unix::fs::symlink(cache.root.join(".config/tokscale"), &linked_config).unwrap();
    let mut env = cache.environment();
    env.insert(
        "TOKSCALE_CONFIG_DIR".to_owned(),
        linked_config.to_string_lossy().into_owned(),
    );
    assert_eq!(
        cache
            .snapshot_with_env(Some(env.clone()))
            .last_30_days_tokens,
        Some(180)
    );
    let data_file = cache.root.join("owned-session-data");
    fs::rename(&cache_file, &data_file).unwrap();
    std::os::unix::fs::symlink(&data_file, &cache_file).unwrap();
    assert_eq!(cache.snapshot_with_env(Some(env)).last_30_days_tokens, None);
}

// upstream: AntigravityLocalReaderTests.swift:544 one-shot caller Cocoa and POSIX cancellation errors preserve their original cause
#[test]
fn one_shot_caller_cocoa_and_posix_cancellation_errors_preserve_their_original_cause() {
    for domain in ["NSCocoaErrorDomain", "NSPOSIXErrorDomain"] {
        let fixture = Fixture::new();
        let path = fixture.jsonl(&[Fixture::CACHE_USAGE]);
        let expected = format!("{domain}:9876");
        let expected_clone = expected.clone();
        let checks = Arc::new(AtomicUsize::new(0));
        let checks_clone = checks.clone();
        let budget = Budget::with_cancellation(Limits::default(), move || {
            let cur = checks_clone.fetch_add(1, Ordering::SeqCst) + 1;
            if cur == 2 {
                Err(ScanFailure::Cancelled(expected_clone.clone()))
            } else {
                Ok(())
            }
        });
        let res = AntigravityLocalReader::read_jsonl(&[path], &budget);
        assert_eq!(res.unwrap_err(), ScanFailure::Cancelled(expected));
        assert_eq!(checks.load(Ordering::SeqCst), 2);
    }
}

// upstream: AntigravityLocalReaderTests.swift:567 empty JSONL requires recognized session metadata to establish empty history
#[test]
fn empty_jsonl_requires_recognized_session_metadata_to_establish_empty_history() {
    let fixture = Fixture::new();
    fixture.jsonl(&[]);
    assert!(!fixture.snapshot().history_coverage_is_established);
    fixture.jsonl(&[
        "{\"type\":\"session_meta\",\"sessionId\":\"empty\",\"modelId\":null,\"timestamp\":null}",
    ]);
    let snapshot = fixture.snapshot();
    assert!(snapshot.history_coverage_is_established);
    assert!(snapshot.daily.is_empty());
    assert_eq!(snapshot.last_30_days_tokens, Some(0));
}

// upstream: AntigravityLocalReaderTests.swift:579 JSONL output and thinking relationship is unsupported without independent schema evidence
#[test]
fn jsonl_output_and_thinking_relationship_is_unsupported_without_independent_schema_evidence() {
    let fixture = Fixture::new();
    let usage = "{\"type\":\"usage\",\"sessionId\":\"producer-example\",\"input\":10,\"output\":5,\"cacheRead\":2,\"reasoning\":1,\"timestamp\":1787832000250}";
    fixture.jsonl(&[usage]);
    assert_eq!(fixture.report().coverage, Coverage::Partial);
    let snapshot = fixture.snapshot();
    assert!(!snapshot.history_coverage_is_established);
    assert_eq!(snapshot.last_30_days_tokens, None);
}

// upstream: AntigravityLocalReaderTests.swift:593 field 2 root envelope with step table timestamps aggregates complete coverage
#[test]
fn field_2_root_envelope_with_step_table_timestamps_aggregates_complete_coverage() {
    let fixture = Fixture::new();
    let step_uuid = "step-abc-123";
    let gen_blob = Fixture::blob_with_root_envelope(
        Some(step_uuid),
        None,
        Some("gemini-3.7-flash"),
        Some("Fixture model"),
        1298,
        100,
        30,
        50,
        7,
        None,
        None,
    );
    let step_blob = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_832_000, 250_000_000);
    fixture.database_with_steps(&[gen_blob], &[step_blob]);
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    let first = &report.report.data[0];
    assert_eq!(first.date, "2026-08-27");
    assert_eq!(first.input_tokens, Some(100));
    assert_eq!(first.output_tokens, Some(30));
    assert_eq!(first.reasoning_tokens, Some(7));
    assert_eq!(
        first.model_breakdowns.as_ref().unwrap()[0].model_name,
        "gemini-3.7-flash"
    );
    assert_eq!(fixture.snapshot().last_30_days_tokens, Some(187));
}

// upstream: AntigravityLocalReaderTests.swift:610 field 2 root envelope with reused step UUID follows stored idx order
#[test]
fn field_2_root_envelope_with_reused_step_uuid_follows_stored_idx_order() {
    let fixture = Fixture::new();
    let step_uuid = "reused-step-uuid";
    let first = Fixture::root_envelope_simple(Some(step_uuid), None, None);
    let second = Fixture::blob_with_root_envelope(
        Some(step_uuid),
        None,
        Some("fixture-model-a"),
        Some("Fixture model"),
        1298,
        200,
        30,
        50,
        7,
        None,
        None,
    );
    let before_midnight =
        Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_140, 250_000_000);
    let after_midnight =
        Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 250_000_000);
    let url = fixture.database(&[first, second]);
    let conn = Fixture::open(&url);
    Fixture::execute(&conn, "CREATE TABLE steps (idx INTEGER, metadata BLOB)");
    Fixture::insert_step(&conn, 20, &after_midnight);
    Fixture::insert_step(&conn, 10, &before_midnight);
    drop(conn);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-27", "2026-08-28"]
    );
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.request_count)
            .collect::<Vec<_>>(),
        vec![Some(1), Some(1)]
    );
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.input_tokens)
            .collect::<Vec<_>>(),
        vec![Some(100), Some(200)]
    );
}

// upstream: AntigravityLocalReaderTests.swift:633 step lookup selects the lowest idx even when it was inserted later
#[test]
fn step_lookup_selects_the_lowest_idx_even_when_it_was_inserted_later() {
    let fixture = Fixture::new();
    let step_uuid = "extra-step-uuid";
    let turn = Fixture::root_envelope_simple(Some(step_uuid), None, None);
    let before_midnight =
        Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_140, 250_000_000);
    let after_midnight =
        Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 250_000_000);
    let url = fixture.database(&[turn]);
    let conn = Fixture::open(&url);
    Fixture::execute(&conn, "CREATE TABLE steps (idx INTEGER, metadata BLOB)");
    Fixture::insert_step(&conn, 20, &after_midnight);
    Fixture::insert_step(&conn, 10, &before_midnight);
    drop(conn);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-27"]
    );
}

// upstream: AntigravityLocalReaderTests.swift:653 duplicate step indices fail closed instead of choosing scan order
#[test]
fn duplicate_step_indices_fail_closed_instead_of_choosing_scan_order() {
    let fixture = Fixture::new();
    let step_uuid = "duplicate-step-uuid";
    let turn = Fixture::root_envelope_simple(Some(step_uuid), None, None);
    let first = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_140, 250_000_000);
    let second = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 250_000_000);
    let url = fixture.database(&[turn]);
    let conn = Fixture::open(&url);
    Fixture::execute(&conn, "CREATE TABLE steps (idx INTEGER, metadata BLOB)");
    Fixture::insert_step(&conn, 10, &first);
    Fixture::insert_step(&conn, 10, &second);
    drop(conn);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert!(report.report.data.is_empty());
}

// upstream: AntigravityLocalReaderTests.swift:673 missing timestamp at the lowest matching step idx fails closed
#[test]
fn missing_timestamp_at_the_lowest_matching_step_idx_fails_closed() {
    let fixture = Fixture::new();
    let step_uuid = "missing-lowest-step-uuid";
    let turn = Fixture::root_envelope_simple(Some(step_uuid), None, None);
    let missing_timestamp = Fixture::message(12, step_uuid.as_bytes());
    let later_timestamp =
        Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 250_000_000);
    let url = fixture.database(&[turn]);
    let conn = Fixture::open(&url);
    Fixture::execute(&conn, "CREATE TABLE steps (idx INTEGER, metadata BLOB)");
    Fixture::insert_step(&conn, 10, &missing_timestamp);
    Fixture::insert_step(&conn, 20, &later_timestamp);
    drop(conn);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert!(report.report.data.is_empty());
}

// upstream: AntigravityLocalReaderTests.swift:693 malformed matching step cannot turn one timestamp into shared coverage
#[test]
fn malformed_matching_step_cannot_turn_one_timestamp_into_shared_coverage() {
    let fixture = Fixture::new();
    let step_uuid = "malformed-shared-step-uuid";
    let turns = vec![
        Fixture::root_envelope_simple(Some(step_uuid), None, None),
        Fixture::root_envelope_simple(Some(step_uuid), None, None),
    ];
    let valid = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_140, 250_000_000);
    let invalid = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 1_000_000_000);
    fixture.database_with_steps(&turns, &[valid, invalid]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert!(report.report.data.is_empty());
}

// upstream: AntigravityLocalReaderTests.swift:711 NULL step occurrence cannot turn one timestamp into shared coverage
#[test]
fn null_step_occurrence_cannot_turn_one_timestamp_into_shared_coverage() {
    let fixture = Fixture::new();
    let step_uuid = "null-shared-step-uuid";
    let turns = vec![
        Fixture::root_envelope_simple(Some(step_uuid), None, None),
        Fixture::root_envelope_simple(Some(step_uuid), None, None),
    ];
    let valid = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 250_000_000);
    let url = fixture.database(&turns);
    let conn = Fixture::open(&url);
    Fixture::execute(&conn, "CREATE TABLE steps (idx INTEGER, metadata BLOB)");
    Fixture::execute(&conn, "INSERT INTO steps (idx, metadata) VALUES (10, NULL)");
    Fixture::insert_step(&conn, 20, &valid);
    drop(conn);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert!(report.report.data.is_empty());
}

// upstream: AntigravityLocalReaderTests.swift:730 unidentified step occurrence cannot turn one timestamp into shared coverage
#[test]
fn unidentified_step_occurrence_cannot_turn_one_timestamp_into_shared_coverage() {
    for unidentified_step_uuid in [None, Some("")] {
        let fixture = Fixture::new();
        let step_uuid = "unidentified-shared-step-uuid";
        let turns = vec![
            Fixture::root_envelope_simple(Some(step_uuid), None, None),
            Fixture::root_envelope_simple(Some(step_uuid), None, None),
        ];
        let unidentified =
            Fixture::step_metadata_blob(unidentified_step_uuid, None, 1_787_875_140, 250_000_000);
        let valid = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 250_000_000);
        fixture.database_with_steps(&turns, &[unidentified, valid]);

        let report = fixture.report();
        assert_eq!(report.coverage, Coverage::Partial);
        assert!(report.report.data.is_empty());
    }
}

// upstream: AntigravityLocalReaderTests.swift:749 unidentified timestamp-less step occurrence cannot turn one timestamp into shared coverage
#[test]
fn unidentified_timestamp_less_step_occurrence_cannot_turn_one_timestamp_into_shared_coverage() {
    let fixture = Fixture::new();
    let step_uuid = "unidentified-timestamp-less-step-uuid";
    let turns = vec![
        Fixture::root_envelope_simple(Some(step_uuid), None, None),
        Fixture::root_envelope_simple(Some(step_uuid), None, None),
    ];
    let unidentified = Fixture::varint(99, 1);
    let valid = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 250_000_000);
    fixture.database_with_steps(&turns, &[unidentified, valid]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert!(report.report.data.is_empty());
}

// upstream: AntigravityLocalReaderTests.swift:764 multiple timestamps cannot be guessed across more reused turns
#[test]
fn multiple_timestamps_cannot_be_guessed_across_more_reused_turns() {
    let fixture = Fixture::new();
    let step_uuid = "ambiguous-reused-step-uuid";
    let turns = vec![
        Fixture::root_envelope_simple(Some(step_uuid), None, None),
        Fixture::root_envelope_simple(Some(step_uuid), None, None),
        Fixture::root_envelope_simple(Some(step_uuid), None, None),
    ];
    let first = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_140, 250_000_000);
    let second = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 250_000_000);
    fixture.database_with_steps(&turns, &[first, second]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert!(report.report.data.is_empty());
}

// upstream: AntigravityLocalReaderTests.swift:779 embedded reused step occurrence keeps later pending timestamp aligned
#[test]
fn embedded_reused_step_occurrence_keeps_later_pending_timestamp_aligned() {
    let fixture = Fixture::new();
    let step_uuid = "mixed-timestamp-step-uuid";
    let first = Fixture::root_envelope_simple(Some(step_uuid), None, Some(1_787_875_140));
    let second = Fixture::blob_with_root_envelope(
        Some(step_uuid),
        None,
        Some("fixture-model-a"),
        Some("Fixture model"),
        1298,
        200,
        30,
        50,
        7,
        None,
        None,
    );
    let before_midnight =
        Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_140, 250_000_000);
    let after_midnight =
        Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 250_000_000);
    fixture.database_with_steps(&[first, second], &[before_midnight, after_midnight]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-27", "2026-08-28"]
    );
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.input_tokens)
            .collect::<Vec<_>>(),
        vec![Some(100), Some(200)]
    );
}

// upstream: AntigravityLocalReaderTests.swift:796 recovered rows retain generation order beside embedded rows
#[test]
fn recovered_rows_retain_generation_order_beside_embedded_rows() {
    let fixture = Fixture::new();
    let response = "shared-order-response";
    let recovered = Fixture::blob_with_root_envelope(
        Some("recovered-step"),
        None,
        Some("fixture-model-a"),
        Some("Fixture model"),
        1298,
        100,
        30,
        50,
        7,
        Some(response),
        None,
    );
    let embedded = Fixture::blob_with_root_envelope(
        Some("embedded-step"),
        None,
        Some("fixture-model-a"),
        Some("Fixture model"),
        1298,
        200,
        30,
        50,
        7,
        Some(response),
        Some(1_787_875_140),
    );
    let step = Fixture::step_metadata_blob(Some("recovered-step"), None, 1_787_875_260, 0);
    let url = fixture.database_with_steps(&[recovered, embedded], &[step]);
    let source = read_single_db(&url);
    let report = fixture.report();

    assert!(source.is_complete);
    assert_eq!(
        source.events.iter().map(|e| e.row).collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(
        source
            .events
            .iter()
            .map(|e| e.turn.timestamp_ms)
            .collect::<Vec<_>>(),
        vec![Some(1_787_875_260_000), Some(1_787_875_140_250)]
    );
    assert_eq!(report.coverage, Coverage::Partial);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-08-28"]
    );
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.input_tokens)
            .collect::<Vec<_>>(),
        vec![Some(100)]
    );
}

// upstream: AntigravityLocalReaderTests.swift:824 unparseable generation occurrence cannot shift a later reused step timestamp
#[test]
fn unparsable_generation_occurrence_cannot_shift_a_later_reused_step_timestamp() {
    let fixture = Fixture::new();
    let step_uuid = "unparsable-generation-step-uuid";
    let invalid_chat = Fixture::message(4, &Fixture::varint(2, u64::MAX));
    let malformed = [
        Fixture::message(4, step_uuid.as_bytes()),
        Fixture::message(1, &invalid_chat),
    ]
    .concat();
    let valid_pending = Fixture::blob_with_root_envelope(
        Some(step_uuid),
        None,
        Some("fixture-model-a"),
        Some("Fixture model"),
        1298,
        200,
        30,
        50,
        7,
        None,
        None,
    );
    let before_midnight =
        Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_140, 250_000_000);
    let after_midnight =
        Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 250_000_000);
    fixture.database_with_steps(
        &[malformed, valid_pending],
        &[before_midnight, after_midnight],
    );

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert!(report.report.data.is_empty());
}

// upstream: AntigravityLocalReaderTests.swift:843 unparseable step occurrence cannot shift later timestamps
#[test]
fn unparsable_step_occurrence_cannot_shift_later_timestamps() {
    let fixture = Fixture::new();
    let step_uuid = "unparsable-step-occurrence-uuid";
    let turns = vec![
        Fixture::root_envelope_simple(Some(step_uuid), None, None),
        Fixture::root_envelope_simple(Some(step_uuid), None, None),
    ];
    let malformed = [Fixture::message(12, step_uuid.as_bytes()), vec![0x0A, 0x80]].concat();
    let valid = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_875_260, 250_000_000);
    fixture.database_with_steps(&turns, &[malformed, valid]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    assert!(report.report.data.is_empty());
}

// upstream: AntigravityLocalReaderTests.swift:858 field 2 root envelope with multi-turn steps sharing step timestamp aggregates complete coverage
#[test]
fn field_2_root_envelope_with_multi_turn_steps_sharing_step_timestamp_aggregates_complete_coverage()
{
    let fixture = Fixture::new();
    let step_uuid = "shared-step-uuid";
    let turn1 = Fixture::root_envelope_simple(Some(step_uuid), None, None);
    let turn2 = Fixture::root_envelope_simple(Some(step_uuid), None, None);
    let turn3 = Fixture::root_envelope_simple(Some(step_uuid), None, None);
    let step_blob = Fixture::step_metadata_blob(Some(step_uuid), None, 1_787_832_000, 250_000_000);
    fixture.database_with_steps(&[turn1, turn2, turn3], &[step_blob]);

    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(report.report.data[0].request_count, Some(3));
    assert_eq!(report.report.data[0].date, "2026-08-27");
}

// upstream: AntigravityLocalReaderTests.swift:875 field 2 root envelope with embedded timestamp in field 9 aggregates complete coverage
#[test]
fn field_2_root_envelope_with_embedded_timestamp_in_field_9_aggregates_complete_coverage() {
    let fixture = Fixture::new();
    let gen_blob = Fixture::blob_with_root_envelope(
        Some("step-embedded"),
        None,
        Some("claude-opus-4-6-thinking"),
        Some("Fixture model"),
        1298,
        100,
        30,
        50,
        7,
        None,
        Some(1_787_832_000),
    );
    fixture.database(&[gen_blob]);
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(report.report.data[0].date, "2026-08-27");
    assert_eq!(
        report.report.data[0].model_breakdowns.as_ref().unwrap()[0].model_name,
        "claude-opus-4-6-thinking"
    );
    assert_eq!(fixture.snapshot().last_30_days_tokens, Some(187));
}

// upstream: AntigravityLocalReaderTests.swift:890 field 2 root envelope without timestamp in either gen_metadata or steps fails closed with partial coverage
#[test]
fn field_2_root_envelope_without_timestamp_in_either_gen_metadata_or_steps_fails_closed_with_partial_coverage()
 {
    let fixture = Fixture::new();
    let gen_blob = Fixture::root_envelope_simple(Some("missing-step-uuid"), None, None);
    fixture.database(&[gen_blob]);
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Partial);
    let snapshot = fixture.snapshot();
    assert!(!snapshot.history_coverage_is_established);
    assert_eq!(snapshot.last_30_days_tokens, None);
}

// upstream: AntigravityLocalReaderTests.swift:906 gemini pro product aliases price from the catalogued preview model
#[test]
fn gemini_pro_product_aliases_price_from_the_catalogued_preview_model() {
    let fixture = Fixture::new();
    save_catalog(
        r#"{
            "google": {
                "id": "google",
                "name": "Google",
                "models": {
                    "gemini-3.1-pro-preview": {
                        "id": "gemini-3.1-pro-preview",
                        "cost": {"input": 1, "output": 2, "cache_read": 0.2}
                    }
                }
            }
        }"#,
        &fixture,
    );
    fixture.database(&[Fixture::blob_custom(
        Some("gemini-pro-default"),
        Some("Fixture model"),
        1298,
        100,
        30,
        50,
        7,
        None,
        Some(1_787_832_000),
    )]);

    let snapshot = fixture.snapshot();
    let expected = 100e-6 + 50.0 * 0.2e-6 + 37.0 * 2e-6;
    assert!((snapshot.last_30_days_cost_usd.unwrap() - expected).abs() < 1e-9);
    assert_eq!(
        snapshot.daily[0].model_breakdowns.as_ref().unwrap()[0].model_name,
        "gemini-pro-default"
    );

    for alias in [
        "gemini-pro-default",
        "gemini-pro-agent",
        "gemini-3.1-pro",
        "gemini-3.1-pro-high",
        "gemini-3.1-pro-thinking",
    ] {
        assert_eq!(
            AntigravityLocalReader::pricing_base_model_id(alias).as_deref(),
            Some("gemini-3.1-pro-preview")
        );
    }
    assert_eq!(
        AntigravityLocalReader::pricing_base_model_id("Gemini-Pro-Default").as_deref(),
        Some("gemini-3.1-pro-preview")
    );
}

// upstream: AntigravityLocalReaderTests.swift:945 model enum ID in usage field 1 does not count as input tokens
#[test]
fn model_enum_id_in_usage_field_1_does_not_count_as_input_tokens() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob_custom(
        Some("fixture-model-a"),
        Some("Fixture model"),
        1318,
        40,
        0,
        0,
        0,
        None,
        Some(1_787_832_000),
    )]);
    let report = fixture.report();
    assert_eq!(report.coverage, Coverage::Complete);
    assert_eq!(report.report.data[0].input_tokens, Some(40));
    assert_eq!(report.report.data[0].total_tokens, Some(40));

    let zero_fixture = Fixture::new();
    zero_fixture.database(&[Fixture::blob_custom(
        Some("fixture-model-a"),
        Some("Fixture model"),
        1318,
        0,
        0,
        0,
        0,
        None,
        Some(1_787_832_000),
    )]);
    let zero_report = zero_fixture.report();
    assert_eq!(zero_report.coverage, Coverage::Complete);
    assert_eq!(zero_report.report.data[0].input_tokens, Some(0));
    assert_eq!(zero_report.report.data[0].total_tokens, Some(0));
    assert_eq!(zero_report.report.data[0].request_count, Some(1));
}

// upstream: AntigravityLocalReaderTests.swift:969 zero token request without a model counts but adds no unknown row
#[test]
fn zero_token_request_without_a_model_counts_but_adds_no_unknown_row() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob_custom(
        None,
        None,
        1298,
        0,
        0,
        0,
        0,
        None,
        Some(1_787_832_000),
    )]);
    let result = fixture.report();
    let entry = &result.report.data[0];
    assert_eq!(result.coverage, Coverage::Complete);
    assert_eq!(entry.request_count, Some(1));
    assert_eq!(entry.total_tokens, Some(0));
    assert!(entry.model_breakdowns.as_ref().unwrap().is_empty());
    assert_eq!(entry.cost_usd, None);
    assert_eq!(entry.unpriced_request_count, Some(1));
    assert_eq!(entry.estimated_request_count, Some(0));
}

// upstream: AntigravityLocalReaderTests.swift:986 zero token request with a named unpriced model keeps its row
#[test]
fn zero_token_request_with_a_named_unpriced_model_keeps_its_row() {
    let fixture = Fixture::new();
    fixture.database(&[Fixture::blob_custom(
        Some("fixture-unpriced"),
        None,
        1298,
        0,
        0,
        0,
        0,
        None,
        Some(1_787_832_000),
    )]);
    let snapshot = fixture.snapshot();
    let entry = &snapshot.daily[0];
    assert!(snapshot.history_coverage_is_established);
    assert_eq!(entry.request_count, Some(1));
    assert_eq!(entry.total_tokens, Some(0));
    assert_eq!(
        entry
            .model_breakdowns
            .as_ref()
            .unwrap()
            .iter()
            .map(|b| b.model_name.as_str())
            .collect::<Vec<_>>(),
        vec!["fixture-unpriced"]
    );
    assert_eq!(entry.cost_usd, None);
    assert_eq!(entry.unpriced_request_count, Some(1));
    assert_eq!(entry.estimated_request_count, Some(0));
}

// upstream: AntigravityLocalReaderTests.swift:1003 zero token request without a model folds into a named day in either order
#[test]
fn zero_token_request_without_a_model_folds_into_a_named_day_in_either_order() {
    for zero_first in [false, true] {
        let fixture = Fixture::new();
        let named = Fixture::blob_custom(
            Some("fixture-model-a"),
            None,
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        );
        let zero = Fixture::blob_custom(None, None, 1298, 0, 0, 0, 0, None, Some(1_787_832_000));
        fixture.database(&if zero_first {
            vec![zero, named]
        } else {
            vec![named, zero]
        });
        let result = fixture.report();
        let entry = &result.report.data[0];
        assert_eq!(result.coverage, Coverage::Complete);
        assert_eq!(entry.request_count, Some(2));
        assert_eq!(entry.total_tokens, Some(187));
        let breakdowns = entry.model_breakdowns.as_ref().unwrap();
        assert_eq!(
            breakdowns
                .iter()
                .map(|b| b.model_name.as_str())
                .collect::<Vec<_>>(),
            vec!["fixture-model-a"]
        );
        assert_eq!(breakdowns[0].request_count, Some(1));
        assert_eq!(breakdowns[0].total_tokens, Some(187));
    }
}

// upstream: AntigravityLocalReaderTests.swift:1019 zero token request without a model costs nothing when pricing is present
#[test]
fn zero_token_request_without_a_model_costs_nothing_when_pricing_is_present() {
    let fixture = Fixture::new();
    save_catalog(
        r#"{
            "google": {
                "id": "google",
                "name": "Google",
                "models": {
                    "gemini-fixture-a": {
                        "id": "gemini-fixture-a",
                        "cost": {"input": 1, "output": 2, "cache_read": 0.2}
                    }
                }
            }
        }"#,
        &fixture,
    );
    fixture.database(&[
        Fixture::blob_custom(
            Some("gemini-fixture-a"),
            None,
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        ),
        Fixture::blob_custom(None, None, 1298, 0, 0, 0, 0, None, Some(1_787_832_000)),
        Fixture::blob_custom(None, None, 1298, 0, 0, 0, 0, None, Some(1_787_745_600)),
    ]);

    let snapshot = fixture.snapshot();
    let expected = 100e-6 + 50.0 * 0.2e-6 + 37.0 * 2e-6;
    let entry = snapshot
        .daily
        .iter()
        .find(|d| d.date == "2026-08-27")
        .unwrap();
    assert_eq!(entry.request_count, Some(2));
    assert!((entry.cost_usd.unwrap() - expected).abs() < 1e-9);
    assert_eq!(entry.unpriced_request_count, Some(0));
    assert_eq!(entry.estimated_request_count, Some(2));
    assert_eq!(
        entry
            .model_breakdowns
            .as_ref()
            .unwrap()
            .iter()
            .map(|b| b.model_name.as_str())
            .collect::<Vec<_>>(),
        vec!["gemini-fixture-a"]
    );
    let zero_day = snapshot
        .daily
        .iter()
        .find(|d| d.date == "2026-08-26")
        .unwrap();
    assert_eq!(zero_day.request_count, Some(1));
    assert_eq!(zero_day.cost_usd, Some(0.0));
    assert_eq!(zero_day.unpriced_request_count, Some(0));
    assert_eq!(zero_day.estimated_request_count, Some(1));
    assert!(zero_day.model_breakdowns.as_ref().unwrap().is_empty());
    assert!(snapshot.history_coverage_is_established);
    assert_eq!(
        snapshot.summary(30, 0).coverage,
        CostUsageCoverageCounts::new(0, 0, 0, 3)
    );
}

// upstream: AntigravityLocalReaderTests.swift:1071 gpt oss medium prices at the Google Vertex list price
#[test]
fn gpt_oss_medium_prices_at_the_google_vertex_list_price() {
    let fixture = Fixture::new();
    save_catalog(VERTEX_CATALOG_JSON, &fixture);
    fixture.database(&[Fixture::blob_custom(
        Some("gpt-oss-120b-medium"),
        None,
        1298,
        100,
        30,
        50,
        7,
        None,
        Some(1_787_832_000),
    )]);

    let snapshot = fixture.snapshot();
    let expected = 100.0 * 0.09e-6 + 50.0 * 0.09e-6 + 37.0 * 0.36e-6;
    let entry = &snapshot.daily[0];
    assert!((entry.cost_usd.unwrap() - expected).abs() < 1e-12);
    assert_eq!(entry.unpriced_request_count, Some(0));
    assert_eq!(entry.estimated_request_count, Some(1));
    let breakdowns = entry.model_breakdowns.as_ref().unwrap();
    assert_eq!(breakdowns[0].model_name, "gpt-oss-120b-medium");
    assert!((breakdowns[0].cost_usd.unwrap() - expected).abs() < 1e-12);
}

// upstream: AntigravityLocalReaderTests.swift:1089 gpt oss medium stays unpriced without the Google Vertex entry
#[test]
fn gpt_oss_medium_stays_unpriced_without_the_google_vertex_entry() {
    let fixture = Fixture::new();
    save_catalog(RESELLER_CATALOG_JSON, &fixture);
    fixture.database(&[Fixture::blob_custom(
        Some("gpt-oss-120b-medium"),
        None,
        1298,
        100,
        30,
        50,
        7,
        None,
        Some(1_787_832_000),
    )]);

    let snapshot = fixture.snapshot();
    let entry = &snapshot.daily[0];
    assert_eq!(entry.cost_usd, None);
    assert_eq!(entry.unpriced_request_count, Some(1));
    assert_eq!(entry.estimated_request_count, Some(0));
    assert_eq!(
        entry.model_breakdowns.as_ref().unwrap()[0].model_name,
        "gpt-oss-120b-medium"
    );
}

// upstream: AntigravityLocalReaderTests.swift:1103 other gpt oss names do not get the Google Vertex price
#[test]
fn other_gpt_oss_names_do_not_get_the_google_vertex_price() {
    for model in [
        "gpt-oss-120b",
        "gpt-oss-120b-high",
        "gpt-oss-120b-medium-thinking",
    ] {
        let fixture = Fixture::new();
        save_catalog(VERTEX_CATALOG_JSON, &fixture);
        fixture.database(&[Fixture::blob_custom(
            Some(model),
            None,
            1298,
            100,
            30,
            50,
            7,
            None,
            Some(1_787_832_000),
        )]);

        let snapshot = fixture.snapshot();
        let entry = &snapshot.daily[0];
        assert_eq!(entry.cost_usd, None);
        assert_eq!(entry.unpriced_request_count, Some(1));
        assert_eq!(entry.estimated_request_count, Some(0));
        assert_eq!(
            entry.model_breakdowns.as_ref().unwrap()[0].model_name,
            model
        );
    }
}

// upstream: AntigravityLocalReaderTests.swift:1153 usage field 9 is reasoning and field 10 is visible output
#[test]
fn usage_field_9_is_reasoning_and_field_10_is_visible_output() {
    let usage = [
        Fixture::varint(2, 10),
        Fixture::varint(9, 5),
        Fixture::varint(10, 3),
    ]
    .concat();
    let chat = [
        Fixture::message(4, &usage),
        Fixture::message(9, &Fixture::message(4, &Fixture::varint(1, 1_787_832_000))),
    ]
    .concat();
    let bytes = Fixture::message(1, &chat);
    let turn = AntigravityProtoReader::parse_turn(&bytes, || Ok::<(), ScanFailure>(()))
        .unwrap()
        .unwrap();
    let parsed_usage = turn.usage.as_ref().unwrap();
    assert_eq!(parsed_usage.reasoning, 5);
    assert_eq!(parsed_usage.output, 3);
}

// upstream: AntigravityLocalReaderTests.swift:1164 safety routed Gemini Flash turns price from the base Flash model
#[test]
fn safety_routed_gemini_flash_turns_price_from_the_base_flash_model() {
    let fixture = Fixture::new();
    save_catalog(
        r#"{
            "google": {
                "id": "google",
                "name": "Google",
                "models": {
                    "gemini-3.7-flash": {
                        "id": "gemini-3.7-flash",
                        "cost": {"input": 1, "output": 2, "cache_read": 0.2}
                    }
                }
            }
        }"#,
        &fixture,
    );
    fixture.database(&[Fixture::blob_custom(
        Some("gemini-3.7-flash-safety-le"),
        Some("Fixture model"),
        1298,
        100,
        30,
        50,
        7,
        None,
        Some(1_787_832_000),
    )]);

    let snapshot = fixture.snapshot();
    let expected = 100e-6 + 50.0 * 0.2e-6 + 37.0 * 2e-6;
    assert!((snapshot.last_30_days_cost_usd.unwrap() - expected).abs() < 1e-9);
    assert_eq!(
        snapshot.daily[0].model_breakdowns.as_ref().unwrap()[0].model_name,
        "gemini-3.7-flash-safety-le"
    );
    assert_eq!(
        AntigravityLocalReader::pricing_base_model_id("gemini-3.7-flash-safety-le").as_deref(),
        Some("gemini-3.7-flash")
    );
}

#[test]
fn entry_symlinks_pointing_outside_roots_are_rejected_in_discovery_and_jsonl_reader() {
    let fixture = Fixture::new();
    let outside_dir = tempfile::tempdir().unwrap();
    let outside_db = outside_dir.path().join("outside-session.db");
    let conn = Fixture::open(&outside_db);
    Fixture::execute(
        &conn,
        "CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB)",
    );
    Fixture::insert(&conn, 0, &Fixture::blob());
    drop(conn);

    let roots = fixture.context().database_roots();
    fs::create_dir_all(&roots[0]).unwrap();
    let symlink_db = roots[0].join("escaped-session.db");
    std::os::unix::fs::symlink(&outside_db, &symlink_db).unwrap();

    let budget = Budget::with_cancellation(Limits::default(), || Ok(()));
    let discovery = AntigravityLocalReader::discover(&roots, "db", &budget).unwrap();
    assert!(discovery.paths.is_empty());
    assert!(!discovery.is_complete);

    let db_budget = Budget::with_cancellation(Limits::default(), || Ok(()));
    let db_source =
        AntigravityLocalReader::read_databases(std::slice::from_ref(&symlink_db), &db_budget)
            .unwrap();
    assert!(!db_source.is_complete);
    assert!(db_source.events.is_empty());
    assert_eq!(db_budget.statistics().sqlite_handles_opened, 0);

    let outside_jsonl = outside_dir.path().join("outside-cache.jsonl");
    fs::write(&outside_jsonl, format!("{}\n", Fixture::CACHE_USAGE)).unwrap();
    let cache_root = fixture.context().cache_root();
    fs::create_dir_all(&cache_root).unwrap();
    let symlink_jsonl = cache_root.join("escaped-cache.jsonl");
    std::os::unix::fs::symlink(&outside_jsonl, &symlink_jsonl).unwrap();

    let cache_discovery = AntigravityLocalReader::discover(
        std::slice::from_ref(&cache_root),
        "jsonl",
        &Budget::with_cancellation(Limits::default(), || Ok(())),
    )
    .unwrap();
    assert!(cache_discovery.paths.is_empty());
    assert!(!cache_discovery.is_complete);

    let jsonl_budget = Budget::with_cancellation(Limits::default(), || Ok(()));
    let jsonl_source =
        AntigravityLocalReader::read_jsonl(std::slice::from_ref(&symlink_jsonl), &jsonl_budget)
            .unwrap();
    assert!(!jsonl_source.is_complete);
    assert!(jsonl_source.events.is_empty());
}
