// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner+CodexPriority.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

static OBSERVATION_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_observation_id() -> u64 {
    OBSERVATION_COUNTER.fetch_add(1, Ordering::Relaxed)
}

use jiff::Timestamp;
use jiff::tz::TimeZone;
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::cost::pricing::{self, CodexServiceTier};
use crate::cost::timestamp::{self, DayKey, DayRange};
use crate::private_tempdir::{self, PrivateTempDir};

/// Maximum size for streaming a foreign Codex SQLite database or WAL into a private temp copy (`2 GiB`).
pub const MAX_CODEX_SQLITE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Maximum number of `response.completed` turn->model mappings retained in incremental cursor state (`4096`).
pub const CODEX_PRIORITY_COMPLETED_MODEL_RETENTION_LIMIT: usize = 4_096;

const REQUEST_MARKER: &str = "websocket request:";
const COMPLETED_MARKER: &str = "websocket event:";
const SUBMISSION_MARKER: &str = "Submission sub=Submission {";
const SUBMISSION_PRIORITY_MARKER: &str = r#"service_tier: Some(Some("priority"))"#;

/// Metadata extracted from Codex's SQLite priority trace (`logs` table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexPriorityTurnMetadata {
    pub thread_id: Option<String>,
    pub turn_id: String,
    pub model: Option<String>,
    pub timestamp: Option<String>,
}

/// Distributed row anchor for validating incremental `logs` table cursors
/// (`CostUsageScanner.CodexPriorityCursorAnchor`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexPriorityCursorAnchor {
    pub row_id: i64,
    pub digest: String,
}

/// Persisted cursor state for incremental Codex priority trace scanning
/// (`CostUsageScanner.CodexPriorityTurnsPersistedCursor`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexPriorityTurnsPersistedCursor {
    pub path: String,
    pub last_row_id: i64,
    pub row_count: i64,
    pub first_row_id: Option<i64>,
    pub first_row_digest: Option<String>,
    pub last_row_digest: Option<String>,
    #[serde(default)]
    pub anchors: Vec<CodexPriorityCursorAnchor>,
    pub coverage_since_epoch: Option<i64>,
    #[serde(default = "default_true")]
    pub completed_models_complete: bool,
    #[serde(default)]
    pub turns: BTreeMap<String, CodexPriorityTurnMetadata>,
    #[serde(default)]
    pub request_sources_by_turn_id: BTreeMap<String, BTreeMap<i64, CodexPriorityTurnMetadata>>,
    #[serde(default)]
    pub priority_completed_models_by_turn_id: BTreeMap<String, BTreeMap<i64, String>>,
    #[serde(default)]
    pub completed_models_by_turn_id: BTreeMap<String, BTreeMap<i64, String>>,
    #[serde(default)]
    pub completed_turn_id_insertion_order: Vec<String>,
    #[serde(default)]
    pub completed_turn_id_insertion_order_start_index: usize,
    #[serde(default)]
    pub completed_models: BTreeMap<String, String>,
    #[serde(default)]
    pub completed_model_order: Vec<String>,
}

const fn default_true() -> bool {
    true
}

/// Per-source cursor entry inside `CodexPriorityTurnsMemoState`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexPrioritySourceCursorState {
    pub path: String,
    pub file_id: Option<String>,
    pub mtime_unix_ms: i64,
    pub size: u64,
    pub wal_file_id: Option<String>,
    pub wal_mtime_unix_ms: i64,
    pub wal_size: u64,
    #[serde(skip)]
    pub header_bytes: [u8; 32],
    #[serde(skip)]
    pub wal_header_bytes: [u8; 32],
    #[serde(skip)]
    pub observation_id: u64,
    pub validated_against_disk: bool,
    pub last_row_id: i64,
    pub row_count: i64,
    pub first_row_id: Option<i64>,
    pub first_row_digest: Option<String>,
    pub last_row_digest: Option<String>,
    pub anchors: Vec<CodexPriorityCursorAnchor>,
    pub coverage_since_epoch: Option<i64>,
    pub completed_models_complete: bool,
    pub turns: BTreeMap<String, CodexPriorityTurnMetadata>,
    #[serde(default)]
    pub request_sources_by_turn_id: BTreeMap<String, BTreeMap<i64, CodexPriorityTurnMetadata>>,
    #[serde(default)]
    pub priority_completed_models_by_turn_id: BTreeMap<String, BTreeMap<i64, String>>,
    #[serde(default)]
    pub completed_models_by_turn_id: BTreeMap<String, BTreeMap<i64, String>>,
    #[serde(default)]
    pub completed_turn_id_insertion_order: Vec<String>,
    #[serde(default)]
    pub completed_turn_id_insertion_order_start_index: usize,
    pub completed_models: BTreeMap<String, String>,
    pub completed_model_order: Vec<String>,
}

impl CodexPrioritySourceCursorState {
    #[must_use]
    pub fn to_persisted_cursor(&self) -> CodexPriorityTurnsPersistedCursor {
        CodexPriorityTurnsPersistedCursor {
            path: self.path.clone(),
            last_row_id: self.last_row_id,
            row_count: self.row_count,
            first_row_id: self.first_row_id,
            first_row_digest: self.first_row_digest.clone(),
            last_row_digest: self.last_row_digest.clone(),
            anchors: self.anchors.clone(),
            coverage_since_epoch: self.coverage_since_epoch,
            completed_models_complete: self.completed_models_complete,
            turns: self.turns.clone(),
            request_sources_by_turn_id: self.request_sources_by_turn_id.clone(),
            priority_completed_models_by_turn_id: self.priority_completed_models_by_turn_id.clone(),
            completed_models_by_turn_id: self.completed_models_by_turn_id.clone(),
            completed_turn_id_insertion_order: self.completed_turn_id_insertion_order.clone(),
            completed_turn_id_insertion_order_start_index: self
                .completed_turn_id_insertion_order_start_index,
            completed_models: self.completed_models.clone(),
            completed_model_order: self.completed_model_order.clone(),
        }
    }

    #[must_use]
    pub fn from_persisted_cursor(cursor: &CodexPriorityTurnsPersistedCursor) -> Self {
        Self {
            path: cursor.path.clone(),
            file_id: None,
            mtime_unix_ms: 0,
            size: 0,
            wal_file_id: None,
            wal_mtime_unix_ms: 0,
            wal_size: 0,
            header_bytes: [0u8; 32],
            wal_header_bytes: [0u8; 32],
            observation_id: next_observation_id(),
            validated_against_disk: false,
            last_row_id: cursor.last_row_id,
            row_count: cursor.row_count,
            first_row_id: cursor.first_row_id,
            first_row_digest: cursor.first_row_digest.clone(),
            last_row_digest: cursor.last_row_digest.clone(),
            anchors: cursor.anchors.clone(),
            coverage_since_epoch: cursor.coverage_since_epoch,
            completed_models_complete: cursor.completed_models_complete,
            turns: cursor.turns.clone(),
            request_sources_by_turn_id: cursor.request_sources_by_turn_id.clone(),
            priority_completed_models_by_turn_id: cursor
                .priority_completed_models_by_turn_id
                .clone(),
            completed_models_by_turn_id: cursor.completed_models_by_turn_id.clone(),
            completed_turn_id_insertion_order: cursor.completed_turn_id_insertion_order.clone(),
            completed_turn_id_insertion_order_start_index: cursor
                .completed_turn_id_insertion_order_start_index,
            completed_models: cursor.completed_models.clone(),
            completed_model_order: if cursor.completed_model_order.is_empty() {
                cursor.completed_models.keys().cloned().collect()
            } else {
                cursor.completed_model_order.clone()
            },
        }
    }
}

/// In-memory memo state across one or more Codex SQLite sources (`logs_2.sqlite` + `state_*.sqlite`),
/// mirroring `CostUsageScanner.CodexPriorityTurnsMemoState`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexPriorityTurnsMemoState {
    pub sources: BTreeMap<String, CodexPrioritySourceCursorState>,
    pub merged_turns: BTreeMap<String, CodexPriorityTurnMetadata>,
}

impl CodexPriorityTurnsMemoState {
    #[must_use]
    pub fn from_persisted_cursor(cursor: &CodexPriorityTurnsPersistedCursor) -> Self {
        let mut sources = BTreeMap::new();
        let source = CodexPrioritySourceCursorState::from_persisted_cursor(cursor);
        let merged_turns = source.turns.clone();
        sources.insert(cursor.path.clone(), source);
        Self {
            sources,
            merged_turns,
        }
    }

    #[must_use]
    pub fn persisted_cursor_for(
        &self,
        database_path: &Path,
    ) -> Option<CodexPriorityTurnsPersistedCursor> {
        let key = normalize_source_path_key(database_path);
        let source = self.sources.get(&key).or_else(|| {
            self.sources
                .get(&database_path.to_string_lossy().into_owned())
        })?;
        if !source.validated_against_disk {
            return None;
        }
        Some(source.to_persisted_cursor())
    }

    #[must_use]
    pub fn any_persisted_cursor(&self) -> Option<CodexPriorityTurnsPersistedCursor> {
        self.sources
            .values()
            .find(|s| s.validated_against_disk)
            .map(CodexPrioritySourceCursorState::to_persisted_cursor)
    }
}

/// Resolution result for loading priority turns (`CostUsageScanner.CodexPriorityTurnsResolution`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexPriorityTurnsResolution {
    pub turns: BTreeMap<String, CodexPriorityTurnMetadata>,
    pub validation_pending: bool,
    pub database_skipped: bool,
}

/// SQL query plan for incremental or cold-start `logs` scanning
/// (`CostUsageScanner.CodexPriorityAccumulationPlan`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexPriorityAccumulationPlan {
    pub sql: String,
    pub bindings: Vec<i64>,
    pub uses_timestamp_index: bool,
}

/// Fingerprint of a single file `(dev, ino, size, mtime_sec, mtime_nsec, header_bytes)` without following symlinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodexSqliteFileFingerprint {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime_sec: i64,
    pub mtime_nsec: u32,
    pub header_bytes: [u8; 32],
}

impl CodexSqliteFileFingerprint {
    #[must_use]
    pub fn file_id_string(&self) -> String {
        format!("{}:{}", self.dev, self.ino)
    }

    #[must_use]
    pub const fn mtime_unix_ms(&self) -> i64 {
        self.mtime_sec
            .saturating_mul(1_000)
            .saturating_add((self.mtime_nsec / 1_000_000) as i64)
    }
}

/// Combined fingerprint of a SQLite database and its optional `-wal` sidecar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CodexSqliteCopyFingerprint {
    db: CodexSqliteFileFingerprint,
    wal: Option<CodexSqliteFileFingerprint>,
}

struct CachedCodexSqliteCopy {
    fingerprint: CodexSqliteCopyFingerprint,
    temp_dir: Arc<PrivateTempDir>,
    copy_count: usize,
}

static CACHED_SQLITE_COPIES: OnceLock<Mutex<BTreeMap<PathBuf, CachedCodexSqliteCopy>>> =
    OnceLock::new();

fn copy_cache() -> &'static Mutex<BTreeMap<PathBuf, CachedCodexSqliteCopy>> {
    CACHED_SQLITE_COPIES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Returns how many times `db_path` has been copied into a private temp directory.
#[must_use]
pub fn codex_sqlite_copy_count(db_path: &Path) -> usize {
    let key = normalize_cache_path(db_path);
    copy_cache()
        .lock()
        .ok()
        .and_then(|map| map.get(&key).map(|e| e.copy_count))
        .unwrap_or(0)
}

/// Clears the in-process SQLite private copy cache.
pub fn clear_codex_sqlite_copy_cache() {
    if let Ok(mut map) = copy_cache().lock() {
        map.clear();
    }
}

fn normalize_cache_path(path: &Path) -> PathBuf {
    if let Some(parent) = path.parent()
        && let Some(name) = path.file_name()
        && let Ok(canon_parent) = fs::canonicalize(parent)
    {
        return canon_parent.join(name);
    }
    path.to_path_buf()
}

fn normalize_source_path_key(path: &Path) -> String {
    normalize_cache_path(path).to_string_lossy().into_owned()
}

/// Default path for Codex's priority trace database (`<codex_home>/logs_2.sqlite`).
#[must_use]
pub fn default_codex_logs_database_path(codex_home: &Path) -> PathBuf {
    codex_home.join("logs_2.sqlite")
}

/// Parses a single trace row body for a priority request (`websocket request:` or `Submission`).
///
/// Mirrors `CostUsageScanner.parseCodexPriorityTraceRow(timestamp:body:)`.
#[must_use]
pub fn parse_codex_priority_trace_row(
    timestamp: Option<&str>,
    body: &str,
) -> Option<CodexPriorityTurnMetadata> {
    let Some(marker_idx) = body.find(REQUEST_MARKER) else {
        return parse_codex_priority_submission_row(timestamp, body);
    };
    let prefix = &body[..marker_idx];
    let json_text = body[marker_idx + REQUEST_MARKER.len()..].trim();
    let request: Value = serde_json::from_str(json_text).ok()?;
    let obj = request.as_object()?;
    if obj.get("type").and_then(Value::as_str) != Some("response.create") {
        return None;
    }
    if obj.get("service_tier").and_then(Value::as_str) != Some("priority") {
        return None;
    }

    let turn_id = extract_key_value(prefix, "turn.id")
        .or_else(|| extract_key_value(prefix, "turn_id"))
        .or_else(|| obj.get("turn_id").and_then(Value::as_str).map(String::from))?;
    if turn_id.is_empty() {
        return None;
    }

    let thread_id = extract_key_value(prefix, "thread_id")
        .or_else(|| extract_key_value(prefix, "thread.id"))
        .or_else(|| {
            obj.get("thread_id")
                .and_then(Value::as_str)
                .map(String::from)
        });
    let model = obj
        .get("model")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from);

    Some(CodexPriorityTurnMetadata {
        thread_id,
        turn_id,
        model,
        timestamp: timestamp.map(String::from),
    })
}

fn parse_codex_priority_submission_row(
    timestamp: Option<&str>,
    body: &str,
) -> Option<CodexPriorityTurnMetadata> {
    if !body.contains(SUBMISSION_PRIORITY_MARKER) {
        return None;
    }
    let submission_idx = body.find(SUBMISSION_MARKER)?;
    let prefix = &body[..submission_idx];
    let submission = &body[submission_idx + SUBMISSION_MARKER.len()..];
    let turn_id = extract_quoted_value(submission, "id")?;
    let thread_id = extract_key_value(prefix, "thread_id");

    Some(CodexPriorityTurnMetadata {
        thread_id,
        turn_id,
        model: None,
        timestamp: timestamp.map(String::from),
    })
}

/// Parses a completed trace row (`websocket event:` with `type == "response.completed"`)
/// and returns `(turn_id, completed_model)`.
///
/// Mirrors `CostUsageScanner.parseCodexCompletedTraceRow(body:)`.
#[must_use]
pub fn parse_codex_completed_trace_row(body: &str) -> Option<(String, String)> {
    let marker_idx = body.find(COMPLETED_MARKER)?;
    let prefix = &body[..marker_idx];
    let json_text = body[marker_idx + COMPLETED_MARKER.len()..].trim();
    let event: Value = serde_json::from_str(json_text).ok()?;
    let obj = event.as_object()?;
    if obj.get("type").and_then(Value::as_str) != Some("response.completed") {
        return None;
    }
    let response = obj.get("response")?.as_object()?;
    let model = response.get("model")?.as_str()?;
    if model.is_empty() {
        return None;
    }
    let turn_id =
        extract_key_value(prefix, "turn.id").or_else(|| extract_key_value(prefix, "turn_id"))?;
    if turn_id.is_empty() {
        return None;
    }
    Some((turn_id, model.to_owned()))
}

fn extract_key_value(text: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    let start = text.find(&needle)? + needle.len();
    let tail = &text[start..];
    let value: String = tail
        .chars()
        .take_while(|ch| {
            !ch.is_whitespace() && !matches!(ch, ',' | ']' | ')' | '}' | ':' | ';' | '"')
        })
        .collect();
    if value.is_empty() { None } else { Some(value) }
}

fn extract_quoted_value(text: &str, name: &str) -> Option<String> {
    let needle = format!("{name}: \"");
    let start = text.find(&needle)? + needle.len();
    let tail = &text[start..];
    let end = tail.find('"')?;
    let value = &tail[..end];
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

/// Builds the SQL query plan for incremental or cold-start `logs` scanning
/// (`CostUsageScanner.codexPriorityAccumulationPlan`).
#[must_use]
pub fn codex_priority_accumulation_plan(
    timestamp_column: Option<&str>,
    body_column: &str,
    last_row_id: i64,
    coverage_since_epoch: i64,
    has_timestamp_index: bool,
) -> CodexPriorityAccumulationPlan {
    let body_filter = format!(
        "({body_column} LIKE '%websocket request:%' \
         OR {body_column} LIKE '%response.completed%' \
         OR {body_column} LIKE '%service_tier: Some(Some(\"priority\"))%')"
    );
    if let Some(ts_col) = timestamp_column
        && ts_col.eq_ignore_ascii_case("ts")
        && last_row_id == 0
        && coverage_since_epoch > 0
        && has_timestamp_index
    {
        return CodexPriorityAccumulationPlan {
            sql: format!(
                "SELECT rowid, {ts_col}, {body_column} FROM logs INDEXED BY idx_logs_ts \
                 WHERE {ts_col} >= ?1 AND {body_filter} ORDER BY rowid ASC"
            ),
            bindings: vec![coverage_since_epoch],
            uses_timestamp_index: true,
        };
    }

    if let Some(ts_col) = timestamp_column {
        return CodexPriorityAccumulationPlan {
            sql: format!(
                "SELECT rowid, {ts_col}, {body_column} FROM logs \
                 WHERE rowid > ?1 \
                   AND ((typeof({ts_col}) = 'integer' AND {ts_col} >= ?2) OR typeof({ts_col}) != 'integer') \
                   AND {body_filter} \
                 ORDER BY rowid ASC"
            ),
            bindings: vec![last_row_id, coverage_since_epoch],
            uses_timestamp_index: false,
        };
    }

    let ts_expr = timestamp_column.unwrap_or("NULL");
    CodexPriorityAccumulationPlan {
        sql: format!(
            "SELECT rowid, {ts_expr}, {body_column} FROM logs \
             WHERE rowid > ?1 AND {body_filter} ORDER BY rowid ASC"
        ),
        bindings: vec![last_row_id],
        uses_timestamp_index: false,
    }
}

static PROCESS_PRIORITY_MEMO: OnceLock<Mutex<CodexPriorityTurnsMemoState>> = OnceLock::new();

fn process_priority_memo() -> &'static Mutex<CodexPriorityTurnsMemoState> {
    PROCESS_PRIORITY_MEMO.get_or_init(|| Mutex::new(CodexPriorityTurnsMemoState::default()))
}

/// Returns the in-process memo state for `database_path`, if present.
#[must_use]
pub fn codex_priority_turns_memo_state(
    database_path: &Path,
) -> Option<CodexPrioritySourceCursorState> {
    let key = normalize_source_path_key(database_path);
    let raw_key = database_path.to_string_lossy().into_owned();
    process_priority_memo().lock().ok().and_then(|memo| {
        memo.sources
            .get(&key)
            .or_else(|| memo.sources.get(&raw_key))
            .cloned()
    })
}

/// Returns a persisted cursor snapshot from the in-process memo for `database_path`.
#[must_use]
pub fn codex_priority_turns_persisted_cursor(
    database_path: &Path,
) -> Option<CodexPriorityTurnsPersistedCursor> {
    process_priority_memo()
        .lock()
        .ok()
        .and_then(|memo| memo.persisted_cursor_for(database_path))
}

/// Drops the in-process memo state for `database_path`.
pub fn reset_codex_priority_turns_memo(database_path: &Path) {
    let key = normalize_source_path_key(database_path);
    let raw_key = database_path.to_string_lossy().into_owned();
    if let Ok(mut memo) = process_priority_memo().lock() {
        memo.sources.remove(&key);
        memo.sources.remove(&raw_key);
        rebuild_merged_turns(&mut memo);
    }
}

/// Seeds the in-process memo for `database_path` from `persisted` if not already populated.
pub fn seed_codex_priority_turns_memo_if_empty_for_path(
    database_path: &Path,
    mut persisted: CodexPriorityTurnsPersistedCursor,
) {
    let key = normalize_source_path_key(database_path);
    let raw_key = database_path.to_string_lossy().into_owned();
    if let Ok(mut memo) = process_priority_memo().lock() {
        if memo.sources.contains_key(&key) || memo.sources.contains_key(&raw_key) {
            return;
        }
        persisted.path = key.clone();
        let source = CodexPrioritySourceCursorState::from_persisted_cursor(&persisted);
        memo.sources.insert(key, source);
        rebuild_merged_turns(&mut memo);
    }
}

/// Stores `candidate` into the in-process memo for `database_path` if it is newer or covers an earlier window.
pub fn store_codex_priority_turns_memo_if_newer_for_path(
    database_path: &Path,
    mut candidate: CodexPrioritySourceCursorState,
) {
    let key = normalize_source_path_key(database_path);
    let raw_key = database_path.to_string_lossy().into_owned();
    candidate.path = key.clone();
    if let Ok(mut memo) = process_priority_memo().lock() {
        if let Some(existing) = memo
            .sources
            .get(&key)
            .or_else(|| memo.sources.get(&raw_key))
            && !is_source_cursor_newer_or_equal(&candidate, existing)
        {
            return;
        }
        memo.sources.remove(&raw_key);
        memo.sources.insert(key, candidate);
        rebuild_merged_turns(&mut memo);
    }
}

/// Seeds `target` from `persisted` if `target` has no sources yet
/// (`CostUsageScanner.seedCodexPriorityTurnsMemoIfEmpty`).
pub fn seed_codex_priority_turns_memo_if_empty(
    target: &mut Option<CodexPriorityTurnsMemoState>,
    persisted: &CodexPriorityTurnsPersistedCursor,
) {
    if target.as_ref().is_none_or(|m| m.sources.is_empty()) {
        *target = Some(CodexPriorityTurnsMemoState::from_persisted_cursor(
            persisted,
        ));
    }
}

/// Stores `candidate` into `target` only when `candidate` is at least as new as `target`
/// (`CostUsageScanner.storeCodexPriorityTurnsMemoIfNewer`).
pub fn store_codex_priority_turns_memo_if_newer(
    target: &mut Option<CodexPriorityTurnsMemoState>,
    candidate: CodexPriorityTurnsMemoState,
) {
    let Some(ref existing) = *target else {
        *target = Some(candidate);
        return;
    };
    if is_codex_priority_memo_newer_or_equal(&candidate, existing) {
        *target = Some(candidate);
    }
}

fn is_source_cursor_newer_or_equal(
    cand_source: &CodexPrioritySourceCursorState,
    existing_source: &CodexPrioritySourceCursorState,
) -> bool {
    if existing_source.validated_against_disk && !cand_source.validated_against_disk {
        return false;
    }
    if cand_source.observation_id != 0
        && existing_source.observation_id != 0
        && cand_source.observation_id != existing_source.observation_id
    {
        return cand_source.observation_id > existing_source.observation_id;
    }
    if cand_source.file_id == existing_source.file_id {
        if let (Some(cand_cov), Some(exist_cov)) = (
            cand_source.coverage_since_epoch,
            existing_source.coverage_since_epoch,
        ) {
            if cand_cov < exist_cov {
                return true;
            }
            if cand_cov > exist_cov {
                return false;
            }
        }
        if cand_source.last_row_id < existing_source.last_row_id {
            return false;
        }
    }
    true
}

fn is_codex_priority_memo_newer_or_equal(
    candidate: &CodexPriorityTurnsMemoState,
    existing: &CodexPriorityTurnsMemoState,
) -> bool {
    for (path, existing_source) in &existing.sources {
        let Some(cand_source) = candidate.sources.get(path) else {
            continue;
        };
        if !is_source_cursor_newer_or_equal(cand_source, existing_source) {
            return false;
        }
    }
    true
}

/// Reads priority turn metadata from Codex's SQLite `logs` table (`logs_2.sqlite` or `state_*.sqlite`).
#[must_use]
pub fn codex_priority_turns(
    database_path: &Path,
    since_day_key: Option<&str>,
    until_day_key: Option<&str>,
    timezone: Option<TimeZone>,
) -> BTreeMap<String, CodexPriorityTurnMetadata> {
    let key = normalize_source_path_key(database_path);
    let raw_key = database_path.to_string_lossy().into_owned();
    let mut memo = CodexPriorityTurnsMemoState::default();
    if let Ok(guard) = process_priority_memo().lock()
        && let Some(src) = guard
            .sources
            .get(&key)
            .or_else(|| guard.sources.get(&raw_key))
            .cloned()
    {
        memo.sources.insert(key.clone(), src);
        rebuild_merged_turns(&mut memo);
    }
    let res = resolve_codex_priority_turns(
        database_path,
        since_day_key,
        until_day_key,
        timezone,
        &mut memo,
        None,
        None,
        MAX_CODEX_SQLITE_BYTES,
    );
    if !res.validation_pending {
        if let Some(updated_source) = memo.sources.get(&key).cloned() {
            store_codex_priority_turns_memo_if_newer_for_path(database_path, updated_source);
        } else {
            reset_codex_priority_turns_memo(database_path);
        }
    }
    res.turns
}

/// Resolves priority turns for a single SQLite database using an incremental memo state and
/// optional deadline, returning `(turns, validation_pending, database_skipped)`.
#[allow(clippy::too_many_arguments)]
pub fn resolve_codex_priority_turns(
    database_path: &Path,
    since_day_key: Option<&str>,
    until_day_key: Option<&str>,
    timezone: Option<TimeZone>,
    memo: &mut CodexPriorityTurnsMemoState,
    deadline: Option<Instant>,
    temp_dir: Option<&Path>,
    max_sqlite_bytes: u64,
) -> CodexPriorityTurnsResolution {
    let source_key = normalize_source_path_key(database_path);
    let raw_key = database_path.to_string_lossy().into_owned();

    // Check if the database exists on disk (without following symlinks).
    let meta_res = fs::symlink_metadata(database_path);
    match meta_res {
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {
            memo.sources.remove(&source_key);
            memo.sources.remove(&raw_key);
            rebuild_merged_turns(memo);
            return CodexPriorityTurnsResolution {
                turns: BTreeMap::new(),
                validation_pending: false,
                database_skipped: false,
            };
        }
        Err(_) => {
            return CodexPriorityTurnsResolution {
                turns: BTreeMap::new(),
                validation_pending: true,
                database_skipped: true,
            };
        }
        Ok(ref m) if !m.file_type().is_file() => {
            return CodexPriorityTurnsResolution {
                turns: BTreeMap::new(),
                validation_pending: true,
                database_skipped: true,
            };
        }
        Ok(_) => {}
    }

    let mut validation_pending = false;
    let mut database_skipped = false;
    update_source_cursor(
        database_path,
        &source_key,
        since_day_key,
        timezone.clone(),
        memo,
        deadline,
        temp_dir,
        max_sqlite_bytes,
        &mut validation_pending,
        &mut database_skipped,
    );
    rebuild_merged_turns(memo);

    let empty_turns = BTreeMap::new();
    let source_turns = memo
        .sources
        .get(&source_key)
        .map_or(&empty_turns, |s| &s.turns);
    CodexPriorityTurnsResolution {
        turns: filter_turns_by_day_range(source_turns, since_day_key, until_day_key, timezone),
        validation_pending,
        database_skipped,
    }
}

/// Loads all priority turns across `<codex_home>/logs_2.sqlite` and any `<codex_home>/state_*.sqlite`.
#[must_use]
pub fn load_codex_priority_turns_for_home(
    codex_home: &Path,
    since_day_key: Option<&str>,
    until_day_key: Option<&str>,
    timezone: Option<TimeZone>,
) -> BTreeMap<String, CodexPriorityTurnMetadata> {
    let mut memo = CodexPriorityTurnsMemoState::default();
    resolve_codex_priority_turns_for_home(
        codex_home,
        since_day_key,
        until_day_key,
        timezone,
        &mut memo,
        None,
        None,
        MAX_CODEX_SQLITE_BYTES,
    )
    .turns
}

/// Resolves priority turns across all SQLite databases in `codex_home` (`logs_2.sqlite` and `state_*.sqlite`),
/// pruning deleted sources from `memo` and tracking `validation_pending` / `database_skipped`.
#[allow(clippy::too_many_arguments)]
pub fn resolve_codex_priority_turns_for_home(
    codex_home: &Path,
    since_day_key: Option<&str>,
    until_day_key: Option<&str>,
    timezone: Option<TimeZone>,
    memo: &mut CodexPriorityTurnsMemoState,
    deadline: Option<Instant>,
    temp_dir: Option<&Path>,
    max_sqlite_bytes: u64,
) -> CodexPriorityTurnsResolution {
    let db_files = discover_codex_sqlite_files(codex_home);
    let active_keys: BTreeSet<String> = db_files
        .iter()
        .map(|p| normalize_source_path_key(p))
        .collect();
    let active_raw: BTreeSet<String> = db_files
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    memo.sources
        .retain(|k, _| active_keys.contains(k) || active_raw.contains(k));

    let mut validation_pending = false;
    let mut database_skipped = false;

    for db_path in &db_files {
        let source_key = normalize_source_path_key(db_path);
        update_source_cursor(
            db_path,
            &source_key,
            since_day_key,
            timezone.clone(),
            memo,
            deadline,
            temp_dir,
            max_sqlite_bytes,
            &mut validation_pending,
            &mut database_skipped,
        );
    }

    rebuild_merged_turns(memo);
    CodexPriorityTurnsResolution {
        turns: filter_turns_by_day_range(
            &memo.merged_turns,
            since_day_key,
            until_day_key,
            timezone,
        ),
        validation_pending,
        database_skipped,
    }
}

fn rebuild_merged_turns(memo: &mut CodexPriorityTurnsMemoState) {
    let mut merged = BTreeMap::new();
    for source in memo.sources.values() {
        for (turn_id, meta) in &source.turns {
            let entry = merged
                .entry(turn_id.clone())
                .or_insert_with(|| meta.clone());
            if entry.model.is_none() && meta.model.is_some() {
                entry.model = meta.model.clone();
            }
            if entry.thread_id.is_none() && meta.thread_id.is_some() {
                entry.thread_id = meta.thread_id.clone();
            }
            if entry.timestamp.is_none() && meta.timestamp.is_some() {
                entry.timestamp = meta.timestamp.clone();
            }
        }
    }
    memo.merged_turns = merged;
}

fn filter_turns_by_day_range(
    turns: &BTreeMap<String, CodexPriorityTurnMetadata>,
    since_day_key: Option<&str>,
    until_day_key: Option<&str>,
    timezone: Option<TimeZone>,
) -> BTreeMap<String, CodexPriorityTurnMetadata> {
    turns
        .iter()
        .filter(|(_, meta)| {
            timestamp_is_in_range(
                meta.timestamp.as_deref(),
                since_day_key,
                until_day_key,
                timezone.clone(),
            )
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Loads session-level service tiers from Codex's `threads` table if present.
#[must_use]
pub fn codex_session_service_tiers(codex_home: &Path) -> BTreeMap<String, CodexServiceTier> {
    codex_session_service_tiers_with_temp(codex_home, None)
}

/// Loads session-level service tiers from Codex's `threads` table using an injected `temp_dir`.
#[must_use]
pub fn codex_session_service_tiers_with_temp(
    codex_home: &Path,
    temp_dir: Option<&Path>,
) -> BTreeMap<String, CodexServiceTier> {
    let mut tiers = BTreeMap::new();
    for db_path in discover_codex_sqlite_files(codex_home) {
        read_session_tiers_from_db(&db_path, temp_dir, &mut tiers);
    }
    tiers
}

/// Returns true if any priority turn key in `range` differs between `old` and `new`
/// (`CostUsageScanner.codexPriorityTurnKeysChanged`).
#[must_use]
pub fn codex_priority_turn_keys_changed(
    old: Option<&BTreeMap<String, String>>,
    new: &BTreeMap<String, String>,
    range: &DayRange,
) -> bool {
    let since = range.scan_since_key.as_str();
    let until = range.scan_until_key.as_str();
    let mut keys: BTreeSet<&str> = new.keys().map(String::as_str).collect();
    if let Some(old_map) = old {
        keys.extend(old_map.keys().map(String::as_str));
    }
    keys.into_iter().any(|day_key| {
        DayRange::is_in_range(day_key, since, until)
            && old.and_then(|m| m.get(day_key)) != new.get(day_key)
    })
}

/// Returns the set of priority turn IDs on days within `range` whose turn ID set or key changed
/// (`CostUsageScanner.changedPriorityTurnIDs`).
#[must_use]
pub fn changed_priority_turn_ids(
    old: Option<&BTreeMap<String, Vec<String>>>,
    new: &BTreeMap<String, Vec<String>>,
    old_keys: Option<&BTreeMap<String, String>>,
    new_keys: &BTreeMap<String, String>,
    range: &DayRange,
) -> BTreeSet<String> {
    let since = range.scan_since_key.as_str();
    let until = range.scan_until_key.as_str();
    let mut days: BTreeSet<&str> = new.keys().map(String::as_str).collect();
    if let Some(old_map) = old {
        days.extend(old_map.keys().map(String::as_str));
    }
    let mut out = BTreeSet::new();
    for day_key in days {
        if !DayRange::is_in_range(day_key, since, until) {
            continue;
        }
        let old_ids: BTreeSet<&str> = old
            .and_then(|m| m.get(day_key))
            .map(|v| v.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let new_ids: BTreeSet<&str> = new
            .get(day_key)
            .map(|v| v.iter().map(String::as_str).collect())
            .unwrap_or_default();
        if old_ids != new_ids || old_keys.and_then(|m| m.get(day_key)) != new_keys.get(day_key) {
            out.extend(old_ids.into_iter().map(str::to_owned));
            out.extend(new_ids.into_iter().map(str::to_owned));
        }
    }
    out
}

/// Merges sparse per-day priority metadata over `range` without enumerating empty calendar days
/// (`CostUsageScanner.mergePriorityDayValues`).
#[must_use]
pub fn merge_priority_day_values<V: Clone>(
    existing: Option<&BTreeMap<String, V>>,
    new: &BTreeMap<String, V>,
    range: &DayRange,
    retained_since_key: &str,
    retained_until_key: &str,
) -> Option<BTreeMap<String, V>> {
    let mut out = existing.cloned().unwrap_or_default();
    let since = range.scan_since_key.as_str();
    let until = range.scan_until_key.as_str();
    let candidate_days: BTreeSet<String> = out.keys().chain(new.keys()).cloned().collect();
    for day_key in candidate_days {
        if DayRange::is_in_range(&day_key, since, until) {
            if let Some(val) = new.get(&day_key) {
                out.insert(day_key, val.clone());
            } else {
                out.remove(&day_key);
            }
        }
    }
    out.retain(|key, _| DayRange::is_in_range(key, retained_since_key, retained_until_key));
    if out.is_empty() { None } else { Some(out) }
}

fn discover_codex_sqlite_files(codex_home: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(codex_home) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        let is_sqlite = std::path::Path::new(&lower)
            .extension()
            .is_some_and(|ext| ext == "sqlite" || ext == "db" || ext == "sqlite3");
        if is_sqlite && (lower.starts_with("logs") || lower.starts_with("state")) {
            files.push(path);
        }
    }
    files.sort_by(|a, b| {
        let a_is_logs = a
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("logs_2.sqlite"));
        let b_is_logs = b
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("logs_2.sqlite"));
        b_is_logs.cmp(&a_is_logs).then_with(|| a.cmp(b))
    });
    files
}

fn read_sqlite_header_fingerprint(file: &mut fs::File) -> [u8; 32] {
    use std::io::{Read, Seek, SeekFrom};
    let mut header = [0u8; 32];
    if file.seek(SeekFrom::Start(24)).is_ok() {
        let _ = file.read(&mut header);
    }
    header
}

fn inspect_regular_file_fingerprint(path: &Path) -> std::io::Result<CodexSqliteFileFingerprint> {
    #[cfg(unix)]
    {
        use rustix::fs::{FileType, Mode, OFlags, fstat, open};
        use rustix::process::geteuid;

        let fd = open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let st = fstat(&fd).map_err(std::io::Error::from)?;
        if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        if st.st_uid != geteuid().as_raw() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "foreign owner",
            ));
        }
        let mut file = fs::File::from(fd);
        let header_bytes = read_sqlite_header_fingerprint(&mut file);
        Ok(CodexSqliteFileFingerprint {
            dev: st.st_dev as u64,
            ino: st.st_ino as u64,
            size: u64::try_from(st.st_size).unwrap_or(0),
            mtime_sec: st.st_mtime as i64,
            mtime_nsec: u32::try_from(st.st_mtime_nsec).unwrap_or(0),
            header_bytes,
        })
    }
    #[cfg(not(unix))]
    {
        let meta = fs::symlink_metadata(path)?;
        if !meta.file_type().is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        let dur = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .unwrap_or_default();
        let header_bytes = fs::File::open(path)
            .ok()
            .map(|mut f| read_sqlite_header_fingerprint(&mut f))
            .unwrap_or([0u8; 32]);
        Ok(CodexSqliteFileFingerprint {
            dev: 0,
            ino: 0,
            size: meta.len(),
            mtime_sec: dur.as_secs() as i64,
            mtime_nsec: dur.subsec_nanos(),
            header_bytes,
        })
    }
}

fn inspect_sqlite_source_fingerprint(
    db_path: &Path,
) -> std::io::Result<CodexSqliteCopyFingerprint> {
    let db = inspect_regular_file_fingerprint(db_path)?;
    let mut wal_os = db_path.as_os_str().to_os_string();
    wal_os.push("-wal");
    let wal_path = PathBuf::from(wal_os);
    let wal = match fs::symlink_metadata(&wal_path) {
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
        Ok(_) => Some(inspect_regular_file_fingerprint(&wal_path)?),
    };
    Ok(CodexSqliteCopyFingerprint { db, wal })
}

fn copy_regular_file_nofollow(src: &Path, dst: &Path, max_bytes: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use rustix::fs::{FileType, Mode, OFlags, fstat, open};
        use rustix::process::geteuid;
        use std::io::Read;

        let src_fd = open(
            src,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let st = fstat(&src_fd).map_err(std::io::Error::from)?;
        if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        if st.st_uid != geteuid().as_raw() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "foreign owner",
            ));
        }
        let len = u64::try_from(st.st_size).unwrap_or(u64::MAX);
        if len > max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "database exceeds max size",
            ));
        }
        let reader = std::fs::File::from(src_fd);
        let mut bounded_reader = reader.take(max_bytes.saturating_add(1));
        let dst_fd = open(
            dst,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(std::io::Error::from)?;
        let mut writer = std::fs::File::from(dst_fd);
        let copied = std::io::copy(&mut bounded_reader, &mut writer)?;
        if copied > max_bytes {
            let _ = fs::remove_file(dst);
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "database grew past max size during copy",
            ));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        use std::io::Read;

        let meta = fs::symlink_metadata(src)?;
        if !meta.file_type().is_file() || meta.len() > max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid sqlite file",
            ));
        }
        let reader = fs::File::open(src)?;
        let mut bounded_reader = reader.take(max_bytes.saturating_add(1));
        let mut writer = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dst)?;
        let copied = std::io::copy(&mut bounded_reader, &mut writer)?;
        if copied > max_bytes {
            let _ = fs::remove_file(dst);
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "database grew past max size during copy",
            ));
        }
        Ok(())
    }
}

#[cfg(unix)]
fn sweep_stale_copy_children(parent: &Path) {
    let active_paths: BTreeSet<PathBuf> = copy_cache()
        .lock()
        .map(|guard| {
            guard
                .values()
                .map(|entry| {
                    fs::canonicalize(entry.temp_dir.path())
                        .unwrap_or_else(|_| entry.temp_dir.path().to_path_buf())
                })
                .collect()
        })
        .unwrap_or_default();

    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_tmp_dir = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.starts_with("ariadusage-tmp-"))
            && entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        if !is_tmp_dir {
            continue;
        }
        let canon = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !active_paths.contains(&canon) && !active_paths.contains(&path) {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

#[cfg(unix)]
fn ensure_private_0700_dir(dir: &Path) -> Option<PathBuf> {
    use rustix::fs::{FileType, Mode, OFlags, chmod, fchmod, fstat, lstat, open};
    use rustix::process::geteuid;
    use std::os::unix::fs::DirBuilderExt;

    if let Some(parent) = dir.parent()
        && !parent.as_os_str().is_empty()
    {
        let mut parent_builder = fs::DirBuilder::new();
        parent_builder.recursive(true).mode(0o700);
        let _ = parent_builder.create(parent);
    }

    if let Ok(existing_stat) = lstat(dir)
        && FileType::from_raw_mode(existing_stat.st_mode) != FileType::Directory
    {
        return None;
    }

    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(dir).ok()?;

    let dir_fd = open(
        dir,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()?;
    let dir_stat = fstat(&dir_fd).ok()?;
    if FileType::from_raw_mode(dir_stat.st_mode) != FileType::Directory
        || dir_stat.st_uid != geteuid().as_raw()
    {
        return None;
    }
    if (dir_stat.st_mode & 0o777) != 0o700 {
        let _ = fchmod(&dir_fd, Mode::from_raw_mode(0o700));
        let _ = chmod(dir, Mode::from_raw_mode(0o700));
        let recheck = fstat(&dir_fd).ok()?;
        if (recheck.st_mode & 0o777) != 0o700 {
            return None;
        }
    }
    let canon = fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    sweep_stale_copy_children(&canon);
    Some(canon)
}

fn resolve_temp_parent(injected_temp_dir: Option<&Path>) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        if let Some(dir) = injected_temp_dir {
            return ensure_private_0700_dir(dir);
        }
        let default_parent = crate::cost::store::default_sqlite_copy_parent(None);
        ensure_private_0700_dir(&default_parent)
    }
    #[cfg(not(unix))]
    {
        let _ = injected_temp_dir;
        None
    }
}

/// Copies a foreign Codex SQLite database (`logs_2.sqlite` or `state_*.sqlite`) and its optional
/// `-wal` sidecar (never `-journal` or `-shm`) into a private `0700` temp directory cached by
/// `(dev, ino, size, mtime)` of the DB and WAL, and opens the copy read-only with
/// `SQLITE_DBCONFIG_DEFENSIVE = true` and `trusted_schema = false`.
fn open_copied_codex_sqlite(
    db_path: &Path,
    injected_temp_dir: Option<&Path>,
    max_bytes: u64,
) -> Option<(Arc<PrivateTempDir>, Connection, CodexSqliteCopyFingerprint)> {
    let fingerprint = inspect_sqlite_source_fingerprint(db_path).ok()?;
    if fingerprint.db.size > max_bytes {
        return None;
    }
    if let Some(wal_fp) = fingerprint.wal
        && wal_fp.size > max_bytes
    {
        return None;
    }

    let cache_key = normalize_cache_path(db_path);
    let mut cached_dir: Option<Arc<PrivateTempDir>> = None;
    let mut prev_count = 0usize;

    if let Ok(guard) = copy_cache().lock()
        && let Some(entry) = guard.get(&cache_key)
    {
        prev_count = entry.copy_count;
        let snap_db = entry.temp_dir.path().join("snapshot.sqlite");
        if entry.fingerprint == fingerprint && fs::symlink_metadata(&snap_db).is_ok() {
            cached_dir = Some(Arc::clone(&entry.temp_dir));
        }
    }

    let temp_dir = if let Some(dir) = cached_dir {
        dir
    } else {
        let temp_parent = resolve_temp_parent(injected_temp_dir)?;
        let new_dir = Arc::new(private_tempdir::create_in(&temp_parent).ok()?);
        let dest_db = new_dir.path().join("snapshot.sqlite");
        copy_regular_file_nofollow(db_path, &dest_db, max_bytes).ok()?;

        if fingerprint.wal.is_some() {
            let mut wal_os = db_path.as_os_str().to_os_string();
            wal_os.push("-wal");
            let wal_src = PathBuf::from(wal_os);
            let dest_wal = new_dir.path().join("snapshot.sqlite-wal");
            copy_regular_file_nofollow(&wal_src, &dest_wal, max_bytes).ok()?;
        }

        if let Ok(mut guard) = copy_cache().lock() {
            guard.insert(
                cache_key,
                CachedCodexSqliteCopy {
                    fingerprint,
                    temp_dir: Arc::clone(&new_dir),
                    copy_count: prev_count + 1,
                },
            );
        }
        new_dir
    };

    let dest_db = temp_dir.path().join("snapshot.sqlite");
    #[cfg(unix)]
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    #[cfg(not(unix))]
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;

    let conn = Connection::open_with_flags(&dest_db, flags).ok()?;
    let _ = conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true);
    let _ = conn.pragma_update(None, "trusted_schema", false);
    let _ = conn.busy_timeout(Duration::from_millis(250));
    Some((temp_dir, conn, fingerprint))
}

#[allow(clippy::too_many_arguments)]
fn update_source_cursor(
    db_path: &Path,
    source_key: &str,
    since_day_key: Option<&str>,
    timezone: Option<TimeZone>,
    memo: &mut CodexPriorityTurnsMemoState,
    deadline: Option<Instant>,
    temp_dir: Option<&Path>,
    max_sqlite_bytes: u64,
    validation_pending: &mut bool,
    database_skipped: &mut bool,
) {
    let raw_key = db_path.to_string_lossy().into_owned();
    let existing = memo
        .sources
        .remove(source_key)
        .or_else(|| memo.sources.remove(&raw_key));

    let requested_since_epoch = since_day_key
        .map(|dk| epoch_seconds_for_day_key(dk, timezone.clone(), -86_400).max(0))
        .unwrap_or(0);

    let Ok(fp) = inspect_sqlite_source_fingerprint(db_path) else {
        *validation_pending = true;
        *database_skipped = true;
        if let Some(prev) = existing {
            memo.sources.insert(source_key.to_owned(), prev);
        }
        return;
    };

    let db_file_id = Some(fp.db.file_id_string());
    let db_mtime_ms = fp.db.mtime_unix_ms();
    let db_size = fp.db.size;
    let db_header = fp.db.header_bytes;
    let wal_file_id = fp.wal.map(|w| w.file_id_string());
    let wal_mtime_ms = fp.wal.map_or(0, |w| w.mtime_unix_ms());
    let wal_size = fp.wal.map_or(0, |w| w.size);
    let wal_header = fp.wal.map_or([0u8; 32], |w| w.header_bytes);

    let unchanged_on_disk = existing.as_ref().is_some_and(|prev| {
        prev.validated_against_disk
            && prev.file_id == db_file_id
            && prev.mtime_unix_ms == db_mtime_ms
            && prev.size == db_size
            && prev.header_bytes == db_header
            && prev.wal_file_id == wal_file_id
            && prev.wal_mtime_unix_ms == wal_mtime_ms
            && prev.wal_size == wal_size
            && prev.wal_header_bytes == wal_header
    });

    let range_covered = existing.as_ref().is_some_and(|prev| {
        prev.completed_models_complete
            && prev
                .coverage_since_epoch
                .is_some_and(|cov| cov <= requested_since_epoch)
    });

    let deadline_expired = deadline.is_some_and(|d| Instant::now() >= d);

    if unchanged_on_disk
        && range_covered
        && let Some(prev) = existing
    {
        memo.sources.insert(source_key.to_owned(), prev);
        return;
    }

    // If deadline is already expired and we have no seeded cursor to append onto:
    if deadline_expired && existing.is_none() {
        *validation_pending = true;
        *database_skipped = true;
        return;
    }

    let Some((_temp_dir, conn, _)) = open_copied_codex_sqlite(db_path, temp_dir, max_sqlite_bytes)
    else {
        *validation_pending = true;
        *database_skipped = true;
        if let Some(mut prev) = existing {
            prev.validated_against_disk = false;
            memo.sources.insert(source_key.to_owned(), prev);
        }
        return;
    };

    if !table_exists(&conn, "logs") {
        // Valid SQLite database without a `logs` table (e.g., `state_5.sqlite` with only `threads`).
        let empty_state = CodexPrioritySourceCursorState {
            path: source_key.to_owned(),
            file_id: db_file_id,
            mtime_unix_ms: db_mtime_ms,
            size: db_size,
            wal_file_id,
            wal_mtime_unix_ms: wal_mtime_ms,
            wal_size,
            observation_id: next_observation_id(),
            validated_against_disk: true,
            coverage_since_epoch: Some(requested_since_epoch),
            completed_models_complete: true,
            ..Default::default()
        };
        memo.sources.insert(source_key.to_owned(), empty_state);
        return;
    }

    let columns = table_columns(&conn, "logs");
    let body_col = ["feedback_log_body", "body", "message", "log_body", "text"]
        .into_iter()
        .find(|col| columns.iter().any(|c| c.eq_ignore_ascii_case(col)));
    let Some(body_col) = body_col else {
        return;
    };
    let ts_col = ["ts", "timestamp", "created_at"]
        .into_iter()
        .find(|col| columns.iter().any(|c| c.eq_ignore_ascii_case(col)));
    let has_ts_index = has_codex_logs_timestamp_index(&conn);

    let Ok((table_row_count, table_min_rowid, table_max_rowid)) = query_logs_bounds(&conn) else {
        // Corrupt database: drop invalid cursor so subsequent loads after repair are not poisoned.
        *validation_pending = true;
        *database_skipped = true;
        return;
    };

    let fresh_state = || CodexPrioritySourceCursorState {
        path: source_key.to_owned(),
        file_id: db_file_id.clone(),
        mtime_unix_ms: db_mtime_ms,
        size: db_size,
        wal_file_id: wal_file_id.clone(),
        wal_mtime_unix_ms: wal_mtime_ms,
        wal_size,
        observation_id: next_observation_id(),
        validated_against_disk: false,
        coverage_since_epoch: Some(requested_since_epoch),
        completed_models_complete: true,
        ..Default::default()
    };

    let mut can_append = existing.as_ref().is_some_and(|prev| {
        can_reuse_cursor_for_append(
            &conn,
            prev,
            ts_col,
            body_col,
            requested_since_epoch,
            table_row_count,
            table_min_rowid,
            table_max_rowid,
        )
    });

    if deadline_expired && !can_append {
        *validation_pending = true;
        if let Some(mut prev) = existing {
            prev.validated_against_disk = false;
            memo.sources.insert(source_key.to_owned(), prev);
        }
        return;
    }

    let mut base_state = if can_append {
        let mut candidate = existing.clone().unwrap();
        if deadline_expired {
            *validation_pending = true;
            candidate
        } else if prune_deleted_codex_priority_sources(&conn, ts_col, body_col, &mut candidate)
            .is_some()
        {
            candidate
        } else {
            can_append = false;
            fresh_state()
        }
    } else {
        fresh_state()
    };

    let plan_since_epoch = if can_append {
        base_state
            .coverage_since_epoch
            .unwrap_or(requested_since_epoch)
    } else {
        requested_since_epoch
    };

    let max_rowid_now = table_max_rowid.unwrap_or(0);
    if !can_append || max_rowid_now > base_state.last_row_id {
        let plan = codex_priority_accumulation_plan(
            ts_col,
            body_col,
            base_state.last_row_id,
            plan_since_epoch,
            has_ts_index,
        );
        let tail_deadline = if deadline_expired && can_append {
            None
        } else {
            deadline
        };

        if !accumulate_logs_rows(&conn, &plan, &mut base_state, tail_deadline) {
            // Mid-scan failure or deadline expiry: preserve previous validated state if available without advancing coverage.
            *validation_pending = true;
            if let Some(mut prev) = existing {
                prev.validated_against_disk = false;
                memo.sources.insert(source_key.to_owned(), prev);
            } else {
                *database_skipped = true;
            }
            return;
        }
    }

    let effective_cov = base_state
        .coverage_since_epoch
        .map_or(requested_since_epoch, |prev_cov| {
            if can_append {
                prev_cov.min(requested_since_epoch)
            } else {
                requested_since_epoch
            }
        });

    let anchor_bound_rowid = if can_append {
        Some(max_rowid_now.max(base_state.last_row_id))
    } else {
        table_max_rowid
    };
    let Ok(anchors) =
        compute_cursor_anchors(&conn, ts_col, body_col, effective_cov, anchor_bound_rowid)
    else {
        *validation_pending = true;
        if let Some(prev) = existing {
            memo.sources.insert(source_key.to_owned(), prev);
        }
        return;
    };

    base_state.path = source_key.to_owned();
    base_state.file_id = db_file_id;
    base_state.mtime_unix_ms = db_mtime_ms;
    base_state.size = db_size;
    base_state.header_bytes = db_header;
    base_state.wal_file_id = wal_file_id;
    base_state.wal_mtime_unix_ms = wal_mtime_ms;
    base_state.wal_size = wal_size;
    base_state.wal_header_bytes = wal_header;
    base_state.observation_id = next_observation_id();
    base_state.validated_against_disk = !*validation_pending;
    base_state.last_row_id = anchor_bound_rowid.unwrap_or(0);
    base_state.row_count = table_row_count;
    base_state.first_row_id = anchors.first().map(|a| a.row_id);
    base_state.first_row_digest = anchors.first().map(|a| a.digest.clone());
    base_state.last_row_digest = anchors.last().map(|a| a.digest.clone());
    base_state.anchors = anchors;
    base_state.coverage_since_epoch = Some(effective_cov);
    base_state.completed_models_complete = true;

    memo.sources.insert(source_key.to_owned(), base_state);
}

#[allow(clippy::too_many_arguments)]
fn can_reuse_cursor_for_append(
    conn: &Connection,
    prev: &CodexPrioritySourceCursorState,
    ts_col: Option<&str>,
    body_col: &str,
    requested_since_epoch: i64,
    table_row_count: i64,
    table_min_rowid: Option<i64>,
    table_max_rowid: Option<i64>,
) -> bool {
    if !prev.completed_models_complete {
        return false;
    }
    let Some(prev_cov) = prev.coverage_since_epoch else {
        return false;
    };
    if requested_since_epoch < prev_cov {
        return false;
    }
    if prev.last_row_id == 0 && prev.row_count == 0 {
        return table_row_count == 0;
    }
    let Some(max_rowid) = table_max_rowid else {
        return false;
    };
    if max_rowid < prev.last_row_id {
        return false;
    }
    if table_min_rowid.is_none() {
        return false;
    }
    anchors_match_in_db(conn, ts_col, body_col, prev)
}

fn anchors_match_in_db(
    conn: &Connection,
    ts_col: Option<&str>,
    body_col: &str,
    prev: &CodexPrioritySourceCursorState,
) -> bool {
    if !prev.anchors.is_empty() {
        let required = if prev.anchors.len() < 4 {
            prev.anchors.len()
        } else {
            2
        };
        let mut matched = 0usize;
        for anchor in &prev.anchors {
            match query_row_digest(conn, ts_col, body_col, anchor.row_id) {
                Ok(Some(dig)) => {
                    if dig == anchor.digest {
                        matched += 1;
                    } else {
                        return false;
                    }
                }
                Ok(None) => {}
                Err(_) => return false,
            }
        }
        return matched >= required;
    }

    let (Some(first_id), Some(first_dig), Some(last_dig)) = (
        prev.first_row_id,
        prev.first_row_digest.as_deref(),
        prev.last_row_digest.as_deref(),
    ) else {
        return false;
    };
    let cur_first = query_row_digest(conn, ts_col, body_col, first_id)
        .ok()
        .flatten();
    let cur_last = query_row_digest(conn, ts_col, body_col, prev.last_row_id)
        .ok()
        .flatten();
    cur_first.as_deref() == Some(first_dig) && cur_last.as_deref() == Some(last_dig)
}

fn prune_deleted_codex_priority_sources(
    conn: &Connection,
    ts_col: Option<&str>,
    body_col: &str,
    state: &mut CodexPrioritySourceCursorState,
) -> Option<bool> {
    let retained_row_ids = retained_codex_priority_source_row_ids(conn, ts_col, body_col, state)?;

    let mut did_prune = false;
    let turn_ids: Vec<String> = state.request_sources_by_turn_id.keys().cloned().collect();
    for turn_id in turn_ids {
        let Some(sources) = state.request_sources_by_turn_id.get(&turn_id).cloned() else {
            continue;
        };
        let retained_sources: BTreeMap<i64, CodexPriorityTurnMetadata> = sources
            .into_iter()
            .filter(|(row_id, _)| retained_row_ids.contains(row_id))
            .collect();
        let prev_len = state
            .request_sources_by_turn_id
            .get(&turn_id)
            .map_or(0, BTreeMap::len);
        if retained_sources.len() == prev_len {
            continue;
        }
        did_prune = true;
        if retained_sources.is_empty() {
            state.request_sources_by_turn_id.remove(&turn_id);
            state.turns.remove(&turn_id);
            if let Some(completed_models) =
                state.priority_completed_models_by_turn_id.remove(&turn_id)
            {
                store_pending_codex_completed_models(state, &turn_id, completed_models);
            }
        } else {
            if let Some((_, latest)) = retained_sources.iter().max_by_key(|(id, _)| *id) {
                state.turns.insert(turn_id.clone(), latest.clone());
            }
            state
                .request_sources_by_turn_id
                .insert(turn_id, retained_sources);
        }
    }

    if prune_deleted_codex_completed_models(
        &retained_row_ids,
        &mut state.priority_completed_models_by_turn_id,
    ) {
        did_prune = true;
    }
    if prune_deleted_codex_completed_models(
        &retained_row_ids,
        &mut state.completed_models_by_turn_id,
    ) {
        did_prune = true;
    }
    compact_codex_pending_completion_order_prefix(state);
    state
        .completed_turn_id_insertion_order
        .retain(|turn_id| state.completed_models_by_turn_id.contains_key(turn_id));

    sync_turn_resolved_models(state);
    Some(did_prune)
}

fn prune_deleted_codex_completed_models(
    retained_row_ids: &BTreeSet<i64>,
    models_by_turn_id: &mut BTreeMap<String, BTreeMap<i64, String>>,
) -> bool {
    let mut did_prune = false;
    let turn_ids: Vec<String> = models_by_turn_id.keys().cloned().collect();
    for turn_id in turn_ids {
        let Some(models_by_row_id) = models_by_turn_id.get(&turn_id).cloned() else {
            continue;
        };
        let orig_len = models_by_row_id.len();
        let retained: BTreeMap<i64, String> = models_by_row_id
            .into_iter()
            .filter(|(row_id, _)| retained_row_ids.contains(row_id))
            .collect();
        if retained.len() == orig_len {
            continue;
        }
        did_prune = true;
        if retained.is_empty() {
            models_by_turn_id.remove(&turn_id);
        } else {
            models_by_turn_id.insert(turn_id, retained);
        }
    }
    did_prune
}

fn retained_codex_priority_source_row_ids(
    conn: &Connection,
    ts_col: Option<&str>,
    body_col: &str,
    state: &CodexPrioritySourceCursorState,
) -> Option<BTreeSet<i64>> {
    let mut request_sources: BTreeMap<i64, CodexPriorityTurnMetadata> = BTreeMap::new();
    for sources in state.request_sources_by_turn_id.values() {
        for (row_id, source) in sources {
            if let Some(existing) = request_sources.get(row_id)
                && existing != source
            {
                return None;
            }
            request_sources.insert(*row_id, source.clone());
        }
    }

    let mut completed_sources: BTreeMap<i64, (String, String)> = BTreeMap::new();
    for models_by_turn_id in [
        &state.priority_completed_models_by_turn_id,
        &state.completed_models_by_turn_id,
    ] {
        for (turn_id, models_by_row_id) in models_by_turn_id {
            for (row_id, model) in models_by_row_id {
                if let Some((ex_turn, ex_model)) = completed_sources.get(row_id)
                    && (ex_turn != turn_id || ex_model != model)
                {
                    return None;
                }
                completed_sources.insert(*row_id, (turn_id.clone(), model.clone()));
            }
        }
    }

    if request_sources
        .keys()
        .any(|row_id| completed_sources.contains_key(row_id))
    {
        return None;
    }

    let row_ids: Vec<i64> = request_sources
        .keys()
        .chain(completed_sources.keys())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if row_ids.is_empty() {
        return Some(BTreeSet::new());
    }

    let ts_expr = ts_col.unwrap_or("NULL");
    let mut retained = BTreeSet::new();
    for chunk in row_ids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT rowid, {ts_expr}, {body_col} FROM logs WHERE rowid IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql).ok()?;
        let params_iter = rusqlite::params_from_iter(chunk.iter());
        let mut rows = stmt.query(params_iter).ok()?;
        while let Some(row) = rows.next().ok()? {
            let row_id: i64 = row.get(0).ok()?;
            let ts_val: rusqlite::types::Value = row.get(1).ok()?;
            let ts_str = match ts_val {
                rusqlite::types::Value::Integer(i) => Some(i.to_string()),
                rusqlite::types::Value::Text(s) => Some(s),
                _ => None,
            };
            let body: String = row.get(2).ok()?;
            if let Some(expected) = request_sources.get(&row_id) {
                let parsed = parse_codex_priority_trace_row(ts_str.as_deref(), &body)?;
                if &parsed != expected {
                    return None;
                }
            } else if let Some((exp_turn, exp_model)) = completed_sources.get(&row_id) {
                let (turn_id, model) = parse_codex_completed_trace_row(&body)?;
                if &turn_id != exp_turn || &model != exp_model {
                    return None;
                }
            } else {
                return None;
            }
            retained.insert(row_id);
        }
    }
    Some(retained)
}

fn store_pending_codex_completed_models(
    state: &mut CodexPrioritySourceCursorState,
    turn_id: &str,
    completed_models: BTreeMap<i64, String>,
) {
    if !state.completed_models_by_turn_id.contains_key(turn_id) {
        state
            .completed_turn_id_insertion_order
            .push(turn_id.to_owned());
        let retained_count = state
            .completed_turn_id_insertion_order
            .len()
            .saturating_sub(state.completed_turn_id_insertion_order_start_index);
        if retained_count > CODEX_PRIORITY_COMPLETED_MODEL_RETENTION_LIMIT {
            let evicted = state.completed_turn_id_insertion_order
                [state.completed_turn_id_insertion_order_start_index]
                .clone();
            state.completed_turn_id_insertion_order_start_index += 1;
            state.completed_models_by_turn_id.remove(&evicted);
            if state.completed_turn_id_insertion_order_start_index
                >= CODEX_PRIORITY_COMPLETED_MODEL_RETENTION_LIMIT
            {
                compact_codex_pending_completion_order_prefix(state);
            }
        }
    }
    state
        .completed_models_by_turn_id
        .entry(turn_id.to_owned())
        .or_default()
        .extend(completed_models);
}

fn compact_codex_pending_completion_order_prefix(state: &mut CodexPrioritySourceCursorState) {
    if state.completed_turn_id_insertion_order_start_index == 0 {
        return;
    }
    state
        .completed_turn_id_insertion_order
        .drain(0..state.completed_turn_id_insertion_order_start_index);
    state.completed_turn_id_insertion_order_start_index = 0;
}

fn sync_turn_resolved_models(state: &mut CodexPrioritySourceCursorState) {
    for (turn_id, turn) in &mut state.turns {
        let base_model = state
            .request_sources_by_turn_id
            .get(turn_id)
            .and_then(|sources| sources.iter().max_by_key(|(id, _)| *id))
            .and_then(|(_, meta)| meta.model.clone());
        let completed_model = state
            .priority_completed_models_by_turn_id
            .get(turn_id)
            .and_then(|models| models.iter().max_by_key(|(id, _)| *id))
            .map(|(_, m)| m.clone());
        if completed_model.is_some() {
            turn.model = completed_model;
        } else if state.request_sources_by_turn_id.contains_key(turn_id) {
            turn.model = base_model;
        }
    }
}

fn query_logs_bounds(conn: &Connection) -> rusqlite::Result<(i64, Option<i64>, Option<i64>)> {
    conn.query_row(
        "SELECT COUNT(*), MIN(rowid), MAX(rowid) FROM logs",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
}

fn compute_cursor_anchors(
    conn: &Connection,
    ts_col: Option<&str>,
    body_col: &str,
    coverage_since_epoch: i64,
    table_max_rowid: Option<i64>,
) -> rusqlite::Result<Vec<CodexPriorityCursorAnchor>> {
    let Some(last_row_id) = table_max_rowid.filter(|&id| id > 0) else {
        return Ok(Vec::new());
    };
    let min_row_id: Option<i64> = if let Some(ts) = ts_col
        && coverage_since_epoch > 0
    {
        let sql = format!(
            "SELECT MIN(rowid) FROM logs \
             WHERE (typeof({ts}) = 'integer' AND {ts} >= ?1) OR typeof({ts}) != 'integer'"
        );
        conn.query_row(&sql, params![coverage_since_epoch], |r| r.get(0))?
    } else {
        conn.query_row("SELECT MIN(rowid) FROM logs", [], |r| r.get(0))?
    };
    let Some(min_id) = min_row_id.filter(|&id| id > 0 && id <= last_row_id) else {
        if let Some(dig) = query_row_digest(conn, ts_col, body_col, last_row_id)? {
            return Ok(vec![CodexPriorityCursorAnchor {
                row_id: last_row_id,
                digest: dig,
            }]);
        }
        return Ok(Vec::new());
    };

    let span = (last_row_id - min_id).max(0);
    let candidates = [
        min_id,
        min_id + (span / 3),
        min_id + ((span / 3) * 2) + (((span % 3) * 2) / 3),
        last_row_id,
    ];
    let mut seen = BTreeSet::new();
    let mut anchors = Vec::new();
    for cand in candidates {
        if !seen.insert(cand) {
            continue;
        }
        let target_id = if cand == min_id || cand == last_row_id {
            Some(cand)
        } else {
            conn.query_row(
                "SELECT rowid FROM logs WHERE rowid >= ?1 AND rowid <= ?2 ORDER BY rowid ASC LIMIT 1",
                params![cand, last_row_id],
                |r| r.get::<_, i64>(0),
            )
            .ok()
        };
        if let Some(row_id) = target_id
            && (anchors
                .iter()
                .all(|a: &CodexPriorityCursorAnchor| a.row_id != row_id))
            && let Some(digest) = query_row_digest(conn, ts_col, body_col, row_id)?
        {
            anchors.push(CodexPriorityCursorAnchor { row_id, digest });
        }
    }
    Ok(anchors)
}

fn query_row_digest(
    conn: &Connection,
    ts_col: Option<&str>,
    body_col: &str,
    row_id: i64,
) -> rusqlite::Result<Option<String>> {
    let ts_expr = ts_col.unwrap_or("NULL");
    let sql =
        format!("SELECT {ts_expr}, CAST({body_col} AS BLOB) FROM logs WHERE rowid = ?1 LIMIT 1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params![row_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let ts_val: rusqlite::types::Value = row.get(0)?;
    let ts_str = match ts_val {
        rusqlite::types::Value::Integer(i) => i.to_string(),
        rusqlite::types::Value::Text(s) => s,
        _ => String::new(),
    };
    let body_bytes: Vec<u8> = match row.get_ref(1)? {
        rusqlite::types::ValueRef::Blob(b) => b.to_vec(),
        rusqlite::types::ValueRef::Text(t) => t.to_vec(),
        _ => Vec::new(),
    };
    let mut hasher = Sha256::new();
    hasher.update(ts_str.as_bytes());
    hasher.update(b"\n");
    hasher.update(&body_bytes);
    let digest = hasher.finalize();
    let mut hex_str = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut hex_str, "{byte:02x}");
    }
    Ok(Some(hex_str))
}

fn accumulate_logs_rows(
    conn: &Connection,
    plan: &CodexPriorityAccumulationPlan,
    state: &mut CodexPrioritySourceCursorState,
    deadline: Option<Instant>,
) -> bool {
    let Ok(mut stmt) = conn.prepare(&plan.sql) else {
        return false;
    };

    let map_row = |row: &rusqlite::Row<'_>| {
        let row_id: i64 = row.get(0)?;
        let ts_val: rusqlite::types::Value = row.get(1)?;
        let ts_str = match ts_val {
            rusqlite::types::Value::Integer(i) => Some(i.to_string()),
            rusqlite::types::Value::Text(s) => Some(s),
            _ => None,
        };
        let body: Option<String> = row.get(2).ok();
        Ok((row_id, ts_str, body))
    };

    let rows_res = match plan.bindings.as_slice() {
        [b0] => stmt.query_map(params![b0], map_row),
        [b0, b1] => stmt.query_map(params![b0, b1], map_row),
        _ => stmt.query_map([], map_row),
    };
    let Ok(rows) = rows_res else {
        return false;
    };

    for (idx, row_res) in rows.enumerate() {
        if idx % 64 == 0
            && let Some(d) = deadline
            && Instant::now() >= d
        {
            return false;
        }
        let Ok((row_id, ts, body_opt)) = row_res else {
            return false;
        };
        let Some(body) = body_opt else {
            continue;
        };
        if let Some((turn_id, completed_model)) = parse_codex_completed_trace_row(&body) {
            record_completed_model(state, row_id, &turn_id, completed_model);
            continue;
        }
        if let Some(parsed) = parse_codex_priority_trace_row(ts.as_deref(), &body) {
            let turn_id = parsed.turn_id.clone();
            state
                .request_sources_by_turn_id
                .entry(turn_id.clone())
                .or_default()
                .insert(row_id, parsed.clone());
            if let Some(completed_models) = state.completed_models_by_turn_id.remove(&turn_id) {
                compact_codex_pending_completion_order_prefix(state);
                state
                    .completed_turn_id_insertion_order
                    .retain(|id| id != &turn_id);
                state
                    .priority_completed_models_by_turn_id
                    .entry(turn_id.clone())
                    .or_default()
                    .extend(completed_models);
            }
            let mut resolved_turn = parsed;
            if let Some(latest_completed) = state
                .priority_completed_models_by_turn_id
                .get(&turn_id)
                .and_then(|m| m.iter().max_by_key(|(id, _)| *id))
                .map(|(_, model)| model.clone())
                .or_else(|| state.completed_models.get(&turn_id).cloned())
            {
                resolved_turn.model = Some(latest_completed);
            }
            state.turns.insert(turn_id, resolved_turn);
        }
    }
    true
}

fn record_completed_model(
    state: &mut CodexPrioritySourceCursorState,
    row_id: i64,
    turn_id: &str,
    completed_model: String,
) {
    if state.turns.contains_key(turn_id) {
        state
            .priority_completed_models_by_turn_id
            .entry(turn_id.to_owned())
            .or_default()
            .insert(row_id, completed_model.clone());
        if let Some(turn) = state.turns.get_mut(turn_id) {
            turn.model = Some(completed_model.clone());
        }
    } else {
        let mut map = BTreeMap::new();
        map.insert(row_id, completed_model.clone());
        store_pending_codex_completed_models(state, turn_id, map);
    }
    state
        .completed_model_order
        .retain(|existing| existing != turn_id);
    state.completed_model_order.push(turn_id.to_owned());
    state
        .completed_models
        .insert(turn_id.to_owned(), completed_model);
    while state.completed_model_order.len() > CODEX_PRIORITY_COMPLETED_MODEL_RETENTION_LIMIT {
        let evicted = state.completed_model_order.remove(0);
        state.completed_models.remove(&evicted);
    }
}

fn has_codex_logs_timestamp_index(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'index' AND tbl_name = 'logs' AND name = 'idx_logs_ts' LIMIT 1",
        [],
        |_| Ok(()),
    )
    .is_ok()
}

fn epoch_seconds_for_day_key(
    day_key: &str,
    timezone: Option<TimeZone>,
    safety_margin_secs: i64,
) -> i64 {
    let Some(date) = timestamp::parse_day_key_date(day_key) else {
        return if safety_margin_secs < 0 {
            i64::MIN / 2
        } else {
            i64::MAX / 2
        };
    };
    let tz = timezone.unwrap_or_else(TimeZone::system);
    let Ok(zdt) = date.to_zoned(tz) else {
        return if safety_margin_secs < 0 {
            i64::MIN / 2
        } else {
            i64::MAX / 2
        };
    };
    zdt.timestamp()
        .as_second()
        .saturating_add(safety_margin_secs)
}

fn timestamp_is_in_range(
    timestamp: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
    timezone: Option<TimeZone>,
) -> bool {
    if since.is_none() && until.is_none() {
        return true;
    }
    let Some(day_key) = day_key_from_trace_timestamp(timestamp, timezone) else {
        return false;
    };
    let key_str = day_key.as_str();
    if let Some(s) = since
        && key_str < s
    {
        return false;
    }
    if let Some(u) = until
        && key_str > u
    {
        return false;
    }
    true
}

fn day_key_from_trace_timestamp(
    timestamp: Option<&str>,
    timezone: Option<TimeZone>,
) -> Option<DayKey> {
    let ts = timestamp?;
    if let Ok(seconds) = ts.parse::<i64>() {
        let dt = Timestamp::from_second(seconds).ok()?;
        return Some(timestamp::day_key_from_utc(dt, timezone));
    }
    ts.get(..10).and_then(DayKey::parse)
}

fn read_session_tiers_from_db(
    db_path: &Path,
    temp_dir: Option<&Path>,
    tiers: &mut BTreeMap<String, CodexServiceTier>,
) {
    let Some((_temp_dir, conn, _)) =
        open_copied_codex_sqlite(db_path, temp_dir, MAX_CODEX_SQLITE_BYTES)
    else {
        return;
    };

    if !table_exists(&conn, "threads") {
        return;
    }
    let columns = table_columns(&conn, "threads");
    let has_id = columns.iter().any(|c| c.eq_ignore_ascii_case("id"));
    let tier_col = ["service_tier", "tier"]
        .into_iter()
        .find(|col| columns.iter().any(|c| c.eq_ignore_ascii_case(col)));
    let (true, Some(tier_col)) = (has_id, tier_col) else {
        return;
    };

    let sql = format!("SELECT id, {tier_col} FROM threads WHERE {tier_col} IS NOT NULL");
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return;
    };
    let Ok(rows) = stmt.query_map([], |row| {
        let id: Option<String> = row.get(0).ok();
        let tier: Option<String> = row.get(1).ok();
        Ok((id, tier))
    }) else {
        return;
    };

    for (id_opt, tier_opt) in rows.flatten() {
        let (Some(id), Some(raw_tier)) = (id_opt, tier_opt) else {
            continue;
        };
        if let Some(tier) = pricing::normalize_codex_service_tier(Some(&raw_tier)) {
            tiers.insert(id, tier);
        }
    }
}

fn table_exists(conn: &Connection, table_name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
        [table_name],
        |_| Ok(()),
    )
    .is_ok()
}

fn table_columns(conn: &Connection, table_name: &str) -> Vec<String> {
    let sql = format!("PRAGMA table_info({table_name})");
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return Vec::new();
    };
    rows.flatten().collect()
}
