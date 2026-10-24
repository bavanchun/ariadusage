// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageStoreModels.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};

use crate::cost::codex::CodexScanReport;
use crate::cost::ledger::{self, CostUsageDailyReport};
use crate::cost::timestamp::DayRange;

pub const DATABASE_FILENAME: &str = "cost-usage.sqlite";
pub const BASE_SCHEMA_VERSION: i32 = 3;
pub const DEFAULT_PARSER_HASH: &str = "3f88178bf03ad704";

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS scan_metadata (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    payload BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    inode INTEGER,
    mtime_ms INTEGER NOT NULL,
    size INTEGER NOT NULL,
    parsed_bytes INTEGER,
    anchor_indexed_bytes INTEGER,
    anchor_window_start INTEGER,
    anchor_sha256 TEXT,
    scan_state BLOB NOT NULL,
    scan_target_size INTEGER,
    scan_complete INTEGER NOT NULL,
    session_id TEXT,
    coverage_since_day TEXT,
    coverage_until_day TEXT,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS files_path_idx ON files(path);
CREATE INDEX IF NOT EXISTS files_session_idx ON files(session_id);
CREATE INDEX IF NOT EXISTS files_coverage_idx ON files(coverage_since_day, coverage_until_day);
CREATE INDEX IF NOT EXISTS files_updated_idx ON files(updated_at_ms);
CREATE TABLE IF NOT EXISTS token_snapshots (
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    event_index INTEGER NOT NULL,
    timestamp TEXT NOT NULL,
    timestamp_ms INTEGER,
    day TEXT,
    last_input INTEGER,
    last_cached INTEGER,
    last_output INTEGER,
    last_reasoning INTEGER,
    total_input INTEGER,
    total_cached INTEGER,
    total_output INTEGER,
    total_reasoning INTEGER,
    end_offset INTEGER,
    PRIMARY KEY(file_id, event_index)
);
CREATE INDEX IF NOT EXISTS token_snapshots_day_idx ON token_snapshots(day);
CREATE INDEX IF NOT EXISTS token_snapshots_timestamp_idx ON token_snapshots(file_id, timestamp_ms, event_index);
CREATE TABLE IF NOT EXISTS usage_rows (
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    row_index INTEGER NOT NULL,
    payload BLOB NOT NULL,
    PRIMARY KEY(file_id, row_index)
);
CREATE INDEX IF NOT EXISTS usage_rows_file_idx ON usage_rows(file_id, row_index);
CREATE TABLE IF NOT EXISTS file_day_aggregates (
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    day TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL,
    cached_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    reasoning_tokens INTEGER NOT NULL,
    request_count INTEGER NOT NULL,
    authoritative_cost_nanos INTEGER NOT NULL,
    standard_input_tokens INTEGER NOT NULL,
    standard_cached_tokens INTEGER NOT NULL,
    standard_output_tokens INTEGER NOT NULL,
    priority_input_tokens INTEGER NOT NULL,
    priority_cached_tokens INTEGER NOT NULL,
    priority_output_tokens INTEGER NOT NULL,
    standard_tokens INTEGER NOT NULL,
    priority_tokens INTEGER NOT NULL,
    PRIMARY KEY(file_id, day, model)
);
CREATE INDEX IF NOT EXISTS file_day_aggregates_day_idx ON file_day_aggregates(day);
CREATE INDEX IF NOT EXISTS file_day_aggregates_model_day_idx ON file_day_aggregates(model, day);
CREATE TABLE IF NOT EXISTS day_aggregates (
    day TEXT NOT NULL,
    model TEXT NOT NULL,
    input_tokens INTEGER NOT NULL,
    cached_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    reasoning_tokens INTEGER NOT NULL,
    request_count INTEGER NOT NULL,
    authoritative_cost_nanos INTEGER NOT NULL,
    standard_input_tokens INTEGER NOT NULL,
    standard_cached_tokens INTEGER NOT NULL,
    standard_output_tokens INTEGER NOT NULL,
    priority_input_tokens INTEGER NOT NULL,
    priority_cached_tokens INTEGER NOT NULL,
    priority_output_tokens INTEGER NOT NULL,
    standard_tokens INTEGER NOT NULL,
    priority_tokens INTEGER NOT NULL,
    PRIMARY KEY(day, model)
);
CREATE INDEX IF NOT EXISTS day_aggregates_day_idx ON day_aggregates(day);
CREATE INDEX IF NOT EXISTS day_aggregates_model_idx ON day_aggregates(model);
CREATE INDEX IF NOT EXISTS day_aggregates_model_day_idx ON day_aggregates(model, day);
CREATE TABLE IF NOT EXISTS fork_lineage (
    file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    session_id TEXT,
    forked_from_id TEXT,
    fork_timestamp TEXT,
    dependency_key TEXT,
    subagent_state BLOB,
    accounting_state BLOB
);
CREATE INDEX IF NOT EXISTS fork_lineage_parent_idx ON fork_lineage(forked_from_id);
CREATE TABLE IF NOT EXISTS buffered_lines (
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    line_index INTEGER NOT NULL,
    ordinal INTEGER,
    end_offset INTEGER,
    payload BLOB NOT NULL,
    PRIMARY KEY(file_id, kind, line_index)
);
CREATE INDEX IF NOT EXISTS buffered_lines_file_idx ON buffered_lines(file_id, kind, line_index);
CREATE TABLE IF NOT EXISTS discovery_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    payload BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS lookback_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    payload BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS accumulators (
    file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    event_count INTEGER NOT NULL,
    next_usage_row_index INTEGER,
    counted_input INTEGER,
    counted_cached INTEGER,
    counted_output INTEGER,
    counted_reasoning INTEGER,
    baseline_input INTEGER,
    baseline_cached INTEGER,
    baseline_output INTEGER,
    baseline_reasoning INTEGER,
    watermark_input INTEGER,
    watermark_cached INTEGER,
    watermark_output INTEGER,
    watermark_reasoning INTEGER,
    saw_divergent INTEGER NOT NULL,
    saw_interleaved INTEGER NOT NULL,
    seen_raw_totals BLOB NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS accumulators_updated_idx ON accumulators(updated_at_ms);
CREATE TABLE IF NOT EXISTS provider_reports (
    provider TEXT PRIMARY KEY NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    payload BLOB NOT NULL
);
"#;

/// Store error classification mirroring `CostUsageStore.StoreError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostUsageStoreError {
    Sqlite(i32),
    InvalidData,
    IncompatibleSchema,
}

/// Persisted token snapshot row (`CostUsageStoreTokenSnapshot` in CodexBar).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostUsageStoreTokenSnapshot {
    pub path: String,
    pub event_index: i64,
    pub timestamp: String,
    pub timestamp_unix_ms: Option<i64>,
    pub day: Option<String>,
    pub last_input: Option<i64>,
    pub last_cached: Option<i64>,
    pub last_output: Option<i64>,
    pub last_reasoning: Option<i64>,
    pub total_input: Option<i64>,
    pub total_cached: Option<i64>,
    pub total_output: Option<i64>,
    pub total_reasoning: Option<i64>,
    pub end_offset: Option<i64>,
}

/// Persisted usage row (`CostUsageStoreUsageRow` in CodexBar).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostUsageStoreUsageRow {
    pub path: String,
    pub row_index: i64,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoreReadyError {
    Untrusted,
    NeedsRebuild,
}

/// Returns true only when `error` represents database corruption or schema drift
/// (`CostUsageStore.shouldRebuild(after:)`). Transient and constraint errors preserve history.
#[must_use]
pub const fn should_rebuild(error: CostUsageStoreError) -> bool {
    match error {
        CostUsageStoreError::InvalidData | CostUsageStoreError::IncompatibleSchema => true,
        CostUsageStoreError::Sqlite(code) => should_rebuild_sqlite_code(code),
    }
}

/// Classifies a raw or extended SQLite result code by its primary 8-bit code.
#[must_use]
pub const fn should_rebuild_sqlite_code(code: i32) -> bool {
    // Primary SQLite result codes that must NOT destroy existing history:
    // SQLITE_PERM(3), SQLITE_BUSY(5), SQLITE_LOCKED(6), SQLITE_NOMEM(7), SQLITE_READONLY(8),
    // SQLITE_INTERRUPT(9), SQLITE_IOERR(10), SQLITE_FULL(13), SQLITE_TOOBIG(18),
    // SQLITE_CONSTRAINT(19), SQLITE_MISUSE(21), SQLITE_AUTH(23), SQLITE_RANGE(25).
    !matches!(
        code & 0xFF,
        3 | 5 | 6 | 7 | 8 | 9 | 10 | 13 | 18 | 19 | 21 | 23 | 25
    )
}

fn should_rebuild_rusqlite_error(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(ffi_err, _) => {
            should_rebuild_sqlite_code(ffi_err.extended_code)
        }
        _ => true,
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostUsageStoreDayAggregate {
    pub day: String,
    pub model: String,
    pub input_tokens: i64,
    pub cached_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub request_count: i64,
    pub authoritative_cost_nanos: i64,
    pub standard_input_tokens: i64,
    pub standard_cached_tokens: i64,
    pub standard_output_tokens: i64,
    pub priority_input_tokens: i64,
    pub priority_cached_tokens: i64,
    pub priority_output_tokens: i64,
    pub standard_tokens: i64,
    pub priority_tokens: i64,
}

impl CostUsageStoreDayAggregate {
    #[must_use]
    pub fn zero(day: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            day: day.into(),
            model: model.into(),
            input_tokens: 0,
            cached_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: 0,
            request_count: 0,
            authoritative_cost_nanos: 0,
            standard_input_tokens: 0,
            standard_cached_tokens: 0,
            standard_output_tokens: 0,
            priority_input_tokens: 0,
            priority_cached_tokens: 0,
            priority_output_tokens: 0,
            standard_tokens: 0,
            priority_tokens: 0,
        }
    }
}

/// Persisted scan metadata (`CostUsageStoreMetadata` in CodexBar).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostUsageStoreMetadata {
    pub last_scan_unix_ms: i64,
    pub scan_since_day: Option<String>,
    pub scan_until_day: Option<String>,
    pub time_zone_identifier: Option<String>,
    pub pricing_key: Option<String>,
    pub priority_metadata_key: Option<String>,
    pub catch_up_pending: bool,
    pub processed_bytes: Option<i64>,
    pub total_bytes: Option<i64>,
    pub completed_files: Option<usize>,
    pub total_files: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority_turns_cursor:
        Option<crate::cost::codex_priority::CodexPriorityTurnsPersistedCursor>,
}

/// Computes the combined SQLite `user_version` from `base` and `parser_hash`
/// using FNV-1a, matching `CostUsageStore.combinedSchemaVersion`.
#[must_use]
pub fn combined_schema_version(base: i32, parser_hash: &str) -> i32 {
    let mut hash: u32 = 2_166_136_261;
    for byte in parser_hash.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    let combined = (((base as u32) & 0x7F) << 24) | (hash & 0x00FF_FFFF);
    combined as i32
}

fn expand_tilde_path(raw: &str, home: Option<&Path>) -> PathBuf {
    if raw == "~" {
        if let Some(h) = home {
            return h.to_path_buf();
        }
        return PathBuf::from(raw);
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(h) = home {
            return h.join(rest);
        }
        return PathBuf::from(raw);
    }
    #[cfg(windows)]
    if let Some(rest) = raw.strip_prefix(r"~\") {
        if let Some(h) = home {
            return h.join(rest);
        }
        return PathBuf::from(raw);
    }
    PathBuf::from(raw)
}

/// Resolves the cache root directory from injected environment, home directory, and optional temp directory,
/// honoring the engine path policy (`XDG_CACHE_HOME` trimmed, tilde-expanded, and required to be absolute;
/// fallback `$HOME/.cache/ariadusage`).
#[must_use]
pub fn resolve_cache_root_with_temp(
    env: &BTreeMap<String, String>,
    home: Option<&Path>,
    temp_dir: Option<&Path>,
) -> PathBuf {
    if let Some(xdg) = env.get("XDG_CACHE_HOME").map(|s| s.trim())
        && !xdg.is_empty()
    {
        let expanded = expand_tilde_path(xdg, home);
        if expanded.is_absolute() {
            return expanded.join("ariadusage");
        }
    }
    if let Some(home) = home.filter(|h| h.is_absolute()) {
        return home.join(".cache").join("ariadusage");
    }
    if let Some(temp_dir) = temp_dir {
        return temp_dir.join("ariadusage");
    }
    if let Some(home) = etcetera::home_dir().ok().filter(|p| p.is_absolute()) {
        return home.join(".cache").join("ariadusage");
    }
    PathBuf::new()
}

/// Resolves the cache root directory from injected environment and home directory.
#[must_use]
pub fn resolve_cache_root(env: &BTreeMap<String, String>, home: Option<&Path>) -> PathBuf {
    resolve_cache_root_with_temp(env, home, None)
}

/// Resolves the default cache root directory (`$XDG_CACHE_HOME/ariadusage` or `~/.cache/ariadusage`)
/// using the engine path policy when no explicit root is provided.
#[must_use]
pub fn default_cache_root() -> PathBuf {
    let home = etcetera::home_dir().ok().filter(|p| p.is_absolute());
    let mut env = BTreeMap::new();
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME").and_then(|v| v.into_string().ok()) {
        env.insert("XDG_CACHE_HOME".to_owned(), xdg);
    }
    resolve_cache_root(&env, home.as_deref())
}

/// Resolves the disk-backed `0700` parent directory beside the cost cache root
/// (`<cache_root>/codex-sqlite-copies`, sibling to `<cache_root>/cost-usage`) for private copies
/// of foreign Codex SQLite databases.
#[must_use]
pub fn default_sqlite_copy_parent(cache_root: Option<&Path>) -> PathBuf {
    let root = cache_root
        .map(Path::to_path_buf)
        .unwrap_or_else(default_cache_root);
    root.join("codex-sqlite-copies")
}

/// Live SQLite connection configuration snapshot (`CostUsageStore.Configuration` in CodexBar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreConfiguration {
    pub busy_timeout_milliseconds: i64,
    pub foreign_keys_enabled: bool,
}

/// SQLite persistence store for local cost scanning (`CostUsageStore` in CodexBar).
#[derive(Debug, Clone)]
pub struct CostUsageStore {
    database_path: PathBuf,
    expected_schema_version: i32,
    expected_parser_hash: String,
    busy_timeout_ms: u64,
    rebuild_count: usize,
}

impl CostUsageStore {
    #[must_use]
    pub fn new(cache_root: Option<&Path>) -> Self {
        Self::with_versions(
            cache_root,
            combined_schema_version(BASE_SCHEMA_VERSION, DEFAULT_PARSER_HASH),
            DEFAULT_PARSER_HASH,
            5_000,
        )
    }

    #[must_use]
    pub fn with_versions(
        cache_root: Option<&Path>,
        schema_version: i32,
        parser_hash: impl Into<String>,
        busy_timeout_ms: u64,
    ) -> Self {
        let root = cache_root
            .map(Path::to_path_buf)
            .unwrap_or_else(default_cache_root);
        let database_path = root.join("cost-usage").join(DATABASE_FILENAME);
        Self {
            database_path,
            expected_schema_version: schema_version,
            expected_parser_hash: parser_hash.into(),
            busy_timeout_ms,
            rebuild_count: 0,
        }
    }

    #[must_use]
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    #[must_use]
    pub const fn rebuild_count(&self) -> usize {
        self.rebuild_count
    }

    /// Reads `PRAGMA busy_timeout` and `PRAGMA foreign_keys` from the store's live SQLite connection.
    pub fn configuration(&mut self) -> rusqlite::Result<StoreConfiguration> {
        let (open_path, existed) = match self.ensure_private_store_ready() {
            Ok(pair) => pair,
            Err(StoreReadyError::Untrusted) => {
                return Err(rusqlite::Error::InvalidPath(self.database_path.clone()));
            }
            Err(StoreReadyError::NeedsRebuild) => {
                self.rebuild_database();
                self.ensure_private_store_ready()
                    .map_err(|_| rusqlite::Error::InvalidPath(self.database_path.clone()))?
            }
        };
        let conn = match self.open_and_validate(&open_path, existed) {
            Ok(c) => c,
            Err(err) => {
                if !should_rebuild_rusqlite_error(&err) {
                    return Err(err);
                }
                self.rebuild_database();
                let (open_path2, existed2) = self
                    .ensure_private_store_ready()
                    .map_err(|_| rusqlite::Error::InvalidPath(self.database_path.clone()))?;
                self.open_and_validate(&open_path2, existed2)?
            }
        };
        let busy_timeout_milliseconds: i64 =
            conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
        let foreign_keys_raw: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        drop(conn);
        self.repair_store_permissions(&open_path);
        Ok(StoreConfiguration {
            busy_timeout_milliseconds,
            foreign_keys_enabled: foreign_keys_raw != 0,
        })
    }

    /// Removes legacy `codex-v11.json` and any temporary siblings (`codex-v11.json*`, `.codex-v11.json*`)
    /// from the store directory and rebuilds the SQLite database (`CostUsageStore.removeLegacyCodexArtifactIfPresent`).
    pub fn remove_legacy_codex_artifact_if_present(&mut self) -> bool {
        let Some(directory) = self.database_path.parent() else {
            return false;
        };
        let legacy_filename = "codex-v11.json";
        let legacy_path = directory.join(legacy_filename);
        if fs::symlink_metadata(&legacy_path).is_err() {
            return false;
        }
        let mut to_remove = vec![legacy_path];
        if let Ok(entries) = fs::read_dir(directory) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str()
                    && (name == legacy_filename
                        || name.starts_with(".codex-v11.json.")
                        || name.starts_with("codex-v11.json.")
                        || name.starts_with("codex-v11.json-"))
                {
                    to_remove.push(entry.path());
                }
            }
        }
        for path in to_remove {
            let _ = fs::remove_file(path);
        }
        self.rebuild_database();
        true
    }

    /// Saves scan metadata into `scan_metadata`.
    pub fn save_metadata(&mut self, metadata: &CostUsageStoreMetadata) -> bool {
        let Ok(payload) = serde_json::to_vec(metadata) else {
            return false;
        };
        self.with_database(false, |conn| {
            conn.execute(
                "INSERT INTO scan_metadata(id, payload) VALUES (1, ?1) \
                 ON CONFLICT(id) DO UPDATE SET payload = excluded.payload",
                params![payload],
            )?;
            Ok(true)
        })
    }

    /// Loads scan metadata from `scan_metadata`.
    #[must_use]
    pub fn load_metadata(&mut self) -> Option<CostUsageStoreMetadata> {
        self.with_database(None, |conn| {
            let mut stmt = conn.prepare("SELECT payload FROM scan_metadata WHERE id = 1")?;
            let mut rows = stmt.query([])?;
            let Some(row) = rows.next()? else {
                return Ok(None);
            };
            let payload: Vec<u8> = row.get(0)?;
            let decoded = serde_json::from_slice(&payload).ok();
            Ok(decoded)
        })
    }

    /// Upserts a scanned file entry into `files`.
    pub fn upsert_file_path(
        &mut self,
        path: &str,
        session_id: Option<&str>,
        mtime_ms: i64,
        size: i64,
        updated_at_ms: i64,
    ) -> bool {
        self.with_database(false, |conn| {
            conn.execute(
                "INSERT INTO files(path, mtime_ms, size, scan_state, scan_complete, session_id, updated_at_ms) \
                 VALUES (?1, ?2, ?3, X'', 1, ?4, ?5) \
                 ON CONFLICT(path) DO UPDATE SET \
                     mtime_ms = excluded.mtime_ms, \
                     size = excluded.size, \
                     scan_complete = 1, \
                     session_id = COALESCE(excluded.session_id, files.session_id), \
                     updated_at_ms = excluded.updated_at_ms",
                params![path, mtime_ms, size, session_id, updated_at_ms],
            )?;
            Ok(true)
        })
    }

    /// Loads stored file paths from `files` ordered by path.
    #[must_use]
    pub fn stored_file_paths(&mut self) -> Vec<String> {
        self.with_database(Vec::new(), |conn| {
            let mut stmt = conn.prepare("SELECT path FROM files ORDER BY path ASC")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            Ok(rows.flatten().collect())
        })
    }

    /// Appends token snapshots for existing files in a single transaction
    /// (`CostUsageStore.appendTokenSnapshots`). Constraint failures on unknown files roll back
    /// without triggering a store rebuild.
    pub fn append_token_snapshots(&mut self, snapshots: &[CostUsageStoreTokenSnapshot]) -> bool {
        if snapshots.is_empty() {
            return true;
        }
        self.with_database(false, |conn| {
            let tx = conn.unchecked_transaction()?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO token_snapshots (
                        file_id, event_index, timestamp, timestamp_ms, day,
                        last_input, last_cached, last_output, last_reasoning,
                        total_input, total_cached, total_output, total_reasoning, end_offset
                    ) VALUES (
                        (SELECT id FROM files WHERE path = ?1),
                        ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
                    )
                    ON CONFLICT(file_id, event_index) DO UPDATE SET
                        timestamp = excluded.timestamp,
                        timestamp_ms = excluded.timestamp_ms,
                        day = excluded.day,
                        last_input = excluded.last_input,
                        last_cached = excluded.last_cached,
                        last_output = excluded.last_output,
                        last_reasoning = excluded.last_reasoning,
                        total_input = excluded.total_input,
                        total_cached = excluded.total_cached,
                        total_output = excluded.total_output,
                        total_reasoning = excluded.total_reasoning,
                        end_offset = excluded.end_offset",
                )?;
                for snap in snapshots {
                    stmt.execute(params![
                        snap.path,
                        snap.event_index,
                        snap.timestamp,
                        snap.timestamp_unix_ms,
                        snap.day,
                        snap.last_input,
                        snap.last_cached,
                        snap.last_output,
                        snap.last_reasoning,
                        snap.total_input,
                        snap.total_cached,
                        snap.total_output,
                        snap.total_reasoning,
                        snap.end_offset,
                    ])?;
                }
            }
            tx.commit()?;
            Ok(true)
        })
    }

    /// Loads token snapshots for `path` ordered by `event_index`.
    #[must_use]
    pub fn fetch_token_snapshots(&mut self, path: &str) -> Vec<CostUsageStoreTokenSnapshot> {
        self.with_database(Vec::new(), |conn| {
            let mut stmt = conn.prepare(
                "SELECT f.path, s.event_index, s.timestamp, s.timestamp_ms, s.day,
                        s.last_input, s.last_cached, s.last_output, s.last_reasoning,
                        s.total_input, s.total_cached, s.total_output, s.total_reasoning, s.end_offset
                 FROM token_snapshots s
                 JOIN files f ON f.id = s.file_id
                 WHERE f.path = ?1
                 ORDER BY s.event_index ASC",
            )?;
            let rows = stmt.query_map(params![path], |row| {
                Ok(CostUsageStoreTokenSnapshot {
                    path: row.get(0)?,
                    event_index: row.get(1)?,
                    timestamp: row.get(2)?,
                    timestamp_unix_ms: row.get(3)?,
                    day: row.get(4)?,
                    last_input: row.get(5)?,
                    last_cached: row.get(6)?,
                    last_output: row.get(7)?,
                    last_reasoning: row.get(8)?,
                    total_input: row.get(9)?,
                    total_cached: row.get(10)?,
                    total_output: row.get(11)?,
                    total_reasoning: row.get(12)?,
                    end_offset: row.get(13)?,
                })
            })?;
            Ok(rows.flatten().collect())
        })
    }

    /// Appends usage rows for existing files in a single transaction
    /// (`CostUsageStore.appendUsageRows`). Constraint failures on unknown files roll back
    /// without triggering a store rebuild.
    pub fn append_usage_rows(&mut self, usage_rows: &[CostUsageStoreUsageRow]) -> bool {
        if usage_rows.is_empty() {
            return true;
        }
        self.with_database(false, |conn| {
            let tx = conn.unchecked_transaction()?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO usage_rows (
                        file_id, row_index, payload
                    ) VALUES (
                        (SELECT id FROM files WHERE path = ?1),
                        ?2, ?3
                    )
                    ON CONFLICT(file_id, row_index) DO UPDATE SET
                        payload = excluded.payload",
                )?;
                for row in usage_rows {
                    stmt.execute(params![row.path, row.row_index, row.payload])?;
                }
            }
            tx.commit()?;
            Ok(true)
        })
    }

    /// Checkpoints the WAL into the main database file and returns the on-disk byte size
    /// (`CostUsageStore.fileSizeBytes`).
    pub fn file_size_bytes(&mut self) -> u64 {
        if !self.database_path.exists() {
            return 0;
        }
        self.with_database(0, |conn| {
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
            let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
            Ok((page_count.max(0) as u64).saturating_mul(page_size.max(0) as u64))
        })
    }

    /// Queries the active SQLite `user_version` (`CostUsageStore.configuration().userVersion`).
    #[must_use]
    pub fn user_version(&mut self) -> Option<i32> {
        self.with_database(None, |conn| {
            let v: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
            Ok(Some(v))
        })
    }

    /// Replaces `day_aggregates` in a single transaction.
    pub fn save_day_aggregates(&mut self, aggregates: &[CostUsageStoreDayAggregate]) -> bool {
        self.with_database(false, |conn| {
            let tx = conn.unchecked_transaction()?;
            tx.execute("DELETE FROM day_aggregates", [])?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO day_aggregates (
                        day, model, input_tokens, cached_tokens, output_tokens, reasoning_tokens,
                        request_count, authoritative_cost_nanos,
                        standard_input_tokens, standard_cached_tokens, standard_output_tokens,
                        priority_input_tokens, priority_cached_tokens, priority_output_tokens,
                        standard_tokens, priority_tokens
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                )?;
                for agg in aggregates {
                    stmt.execute(params![
                        agg.day,
                        agg.model,
                        agg.input_tokens,
                        agg.cached_tokens,
                        agg.output_tokens,
                        agg.reasoning_tokens,
                        agg.request_count,
                        agg.authoritative_cost_nanos,
                        agg.standard_input_tokens,
                        agg.standard_cached_tokens,
                        agg.standard_output_tokens,
                        agg.priority_input_tokens,
                        agg.priority_cached_tokens,
                        agg.priority_output_tokens,
                        agg.standard_tokens,
                        agg.priority_tokens,
                    ])?;
                }
            }
            tx.commit()?;
            Ok(true)
        })
    }

    /// Loads `day_aggregates` optionally filtered by `since_day..=until_day`.
    #[must_use]
    pub fn load_day_aggregates(
        &mut self,
        since_day: Option<&str>,
        until_day: Option<&str>,
    ) -> Vec<CostUsageStoreDayAggregate> {
        self.with_database(Vec::new(), |conn| {
            let mut stmt = conn.prepare(
                "SELECT day, model, input_tokens, cached_tokens, output_tokens, reasoning_tokens,
                        request_count, authoritative_cost_nanos,
                        standard_input_tokens, standard_cached_tokens, standard_output_tokens,
                        priority_input_tokens, priority_cached_tokens, priority_output_tokens,
                        standard_tokens, priority_tokens
                 FROM day_aggregates
                 ORDER BY day ASC, model ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(CostUsageStoreDayAggregate {
                    day: row.get(0)?,
                    model: row.get(1)?,
                    input_tokens: row.get(2)?,
                    cached_tokens: row.get(3)?,
                    output_tokens: row.get(4)?,
                    reasoning_tokens: row.get(5)?,
                    request_count: row.get(6)?,
                    authoritative_cost_nanos: row.get(7)?,
                    standard_input_tokens: row.get(8)?,
                    standard_cached_tokens: row.get(9)?,
                    standard_output_tokens: row.get(10)?,
                    priority_input_tokens: row.get(11)?,
                    priority_cached_tokens: row.get(12)?,
                    priority_output_tokens: row.get(13)?,
                    standard_tokens: row.get(14)?,
                    priority_tokens: row.get(15)?,
                })
            })?;
            let mut out = Vec::new();
            for agg in rows.flatten() {
                if let Some(s) = since_day
                    && agg.day.as_str() < s
                {
                    continue;
                }
                if let Some(u) = until_day
                    && agg.day.as_str() > u
                {
                    continue;
                }
                out.push(agg);
            }
            Ok(out)
        })
    }

    /// Saves a `CostUsageDailyReport` for `provider` (`"claude"`, `"codex"`, `"vertexai"`).
    pub fn save_provider_report(
        &mut self,
        provider: &str,
        report: &CostUsageDailyReport,
        updated_at_unix_ms: i64,
    ) -> bool {
        let Ok(payload) = serde_json::to_vec(report) else {
            return false;
        };
        self.with_database(false, |conn| {
            conn.execute(
                "INSERT INTO provider_reports(provider, updated_at_ms, payload) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(provider) DO UPDATE SET updated_at_ms = excluded.updated_at_ms, payload = excluded.payload",
                params![provider, updated_at_unix_ms, payload],
            )?;
            Ok(true)
        })
    }

    /// Loads a persisted `(CostUsageDailyReport, updated_at_unix_ms)` for `provider`.
    #[must_use]
    pub fn load_provider_report(&mut self, provider: &str) -> Option<(CostUsageDailyReport, i64)> {
        self.with_database(None, |conn| {
            let mut stmt = conn.prepare(
                "SELECT updated_at_ms, payload FROM provider_reports WHERE provider = ?1",
            )?;
            let mut rows = stmt.query(params![provider])?;
            let Some(row) = rows.next()? else {
                return Ok(None);
            };
            let updated_at_ms: i64 = row.get(0)?;
            let payload: Vec<u8> = row.get(1)?;
            let report: CostUsageDailyReport = match serde_json::from_slice(&payload) {
                Ok(r) => r,
                Err(_) => return Ok(None),
            };
            Ok(Some((report, updated_at_ms)))
        })
    }

    /// Persists a `CodexScanReport`'s day/model aggregates, metadata, scanned files, and daily report.
    pub fn save_codex_scan(
        &mut self,
        scan: &CodexScanReport,
        range: &DayRange,
        updated_at_unix_ms: i64,
    ) -> CostUsageDailyReport {
        let _ = self.remove_legacy_codex_artifact_if_present();
        if scan.validation_pending
            && let Some((cached_report, _)) = self.load_provider_report("codex")
        {
            return cached_report;
        }
        let report = ledger::build_codex_daily_report(scan, range);
        let mut agg_map: BTreeMap<(String, String), CostUsageStoreDayAggregate> = BTreeMap::new();

        for row in &scan.rows {
            let entry = agg_map
                .entry((row.day.clone(), row.model.clone()))
                .or_insert_with(|| CostUsageStoreDayAggregate::zero(&row.day, &row.model));
            let is_priority = row.pricing_mode.as_deref() == Some("priority");
            let tok = row.input + row.cached + row.output;
            entry.input_tokens += row.input;
            entry.cached_tokens += row.cached;
            entry.output_tokens += row.output;
            entry.reasoning_tokens += row.reasoning.unwrap_or(0);
            entry.request_count += 1;
            entry.authoritative_cost_nanos += row.known_cost_nanos.unwrap_or(0);
            if is_priority {
                entry.priority_input_tokens += row.input;
                entry.priority_cached_tokens += row.cached;
                entry.priority_output_tokens += row.output;
                entry.priority_tokens += tok;
            } else {
                entry.standard_input_tokens += row.input;
                entry.standard_cached_tokens += row.cached;
                entry.standard_output_tokens += row.output;
                entry.standard_tokens += tok;
            }
        }

        for (path_str, file) in &scan.files {
            let (mtime_ms, size) = fs::symlink_metadata(path_str)
                .ok()
                .map(|meta| {
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_millis() as i64);
                    (mtime, meta.len() as i64)
                })
                .unwrap_or((0, 0));
            let _ = self.upsert_file_path(
                path_str,
                file.session_id.as_deref(),
                mtime_ms,
                size,
                updated_at_unix_ms,
            );
        }

        let aggregates: Vec<CostUsageStoreDayAggregate> = agg_map.into_values().collect();
        let _ = self.save_day_aggregates(&aggregates);
        let _ = self.save_metadata(&CostUsageStoreMetadata {
            last_scan_unix_ms: updated_at_unix_ms,
            scan_since_day: Some(range.since_key.as_str().to_owned()),
            scan_until_day: Some(range.until_key.as_str().to_owned()),
            time_zone_identifier: range
                .timezone
                .as_ref()
                .and_then(|tz| tz.iana_name().map(str::to_owned)),
            pricing_key: None,
            priority_metadata_key: None,
            catch_up_pending: false,
            processed_bytes: None,
            total_bytes: None,
            completed_files: Some(scan.files.len()),
            total_files: Some(scan.files.len()),
            priority_turns_cursor: scan.persisted_priority_cursor.clone(),
        });
        let _ = self.save_provider_report("codex", &report, updated_at_unix_ms);
        report
    }

    fn with_database<T, F>(&mut self, fallback: T, mut operation: F) -> T
    where
        F: FnMut(&Connection) -> rusqlite::Result<T>,
    {
        let (open_path, existed) = match self.ensure_private_store_ready() {
            Ok(pair) => pair,
            Err(StoreReadyError::Untrusted) => return fallback,
            Err(StoreReadyError::NeedsRebuild) => {
                self.rebuild_database();
                let Ok(pair) = self.ensure_private_store_ready() else {
                    return fallback;
                };
                pair
            }
        };
        match self.open_and_validate(&open_path, existed) {
            Ok(conn) => match operation(&conn) {
                Ok(val) => {
                    drop(conn);
                    self.repair_store_permissions(&open_path);
                    val
                }
                Err(err) => {
                    drop(conn);
                    if !should_rebuild_rusqlite_error(&err) {
                        self.repair_store_permissions(&open_path);
                        return fallback;
                    }
                    self.rebuild_database();
                    let Ok((open_path2, existed2)) = self.ensure_private_store_ready() else {
                        return fallback;
                    };
                    match self.open_and_validate(&open_path2, existed2) {
                        Ok(conn2) => {
                            let res = operation(&conn2).unwrap_or(fallback);
                            drop(conn2);
                            self.repair_store_permissions(&open_path2);
                            res
                        }
                        Err(_) => fallback,
                    }
                }
            },
            Err(err) => {
                if !should_rebuild_rusqlite_error(&err) {
                    return fallback;
                }
                self.rebuild_database();
                let Ok((open_path2, existed2)) = self.ensure_private_store_ready() else {
                    return fallback;
                };
                match self.open_and_validate(&open_path2, existed2) {
                    Ok(conn2) => {
                        let res = operation(&conn2).unwrap_or(fallback);
                        drop(conn2);
                        self.repair_store_permissions(&open_path2);
                        res
                    }
                    Err(_) => fallback,
                }
            }
        }
    }

    /// Creates/verifies the `0700` parent directory and `0600` database file (and any sidecars)
    /// without following symlinks. Returns `Ok((canonical_parent_db_path, existed))` if ready,
    /// `Err(StoreReadyError::NeedsRebuild)` if the database path is occupied by an owned directory,
    /// or `Err(StoreReadyError::Untrusted)` if a security/ownership check fails.
    fn ensure_private_store_ready(&self) -> Result<(PathBuf, bool), StoreReadyError> {
        let Some(parent) = self.database_path.parent() else {
            return Err(StoreReadyError::Untrusted);
        };
        let Some(file_name) = self.database_path.file_name() else {
            return Err(StoreReadyError::Untrusted);
        };
        #[cfg(unix)]
        {
            use rustix::fs::{FileType, Mode, OFlags, chmod, fchmod, fstat, lstat, open};
            use rustix::process::geteuid;
            use std::os::unix::fs::DirBuilderExt;

            if let Ok(existing_parent_stat) = lstat(parent)
                && FileType::from_raw_mode(existing_parent_stat.st_mode) != FileType::Directory
            {
                return Err(StoreReadyError::Untrusted);
            }

            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700);
            builder
                .create(parent)
                .map_err(|_| StoreReadyError::Untrusted)?;

            let dir_stat = lstat(parent).map_err(|_| StoreReadyError::Untrusted)?;
            if FileType::from_raw_mode(dir_stat.st_mode) != FileType::Directory {
                return Err(StoreReadyError::Untrusted);
            }
            let euid = geteuid().as_raw();
            if dir_stat.st_uid != euid {
                return Err(StoreReadyError::Untrusted);
            }
            if (dir_stat.st_mode & 0o777) != 0o700 {
                let _ = chmod(parent, Mode::from_raw_mode(0o700));
            }

            let canon_parent = fs::canonicalize(parent).map_err(|_| StoreReadyError::Untrusted)?;
            let open_path = canon_parent.join(file_name);

            let pre_existed = match lstat(&open_path) {
                Ok(st) => {
                    if st.st_uid != euid {
                        return Err(StoreReadyError::Untrusted);
                    }
                    let ft = FileType::from_raw_mode(st.st_mode);
                    if ft == FileType::Directory {
                        return Err(StoreReadyError::NeedsRebuild);
                    }
                    if ft != FileType::RegularFile {
                        return Err(StoreReadyError::Untrusted);
                    }
                    true
                }
                Err(rustix::io::Errno::NOENT) => false,
                Err(_) => return Err(StoreReadyError::Untrusted),
            };

            let base = open_path.to_string_lossy().into_owned();
            for suffix in ["-wal", "-shm", "-journal"] {
                let sidecar = PathBuf::from(format!("{base}{suffix}"));
                if let Ok(st) = lstat(&sidecar)
                    && (FileType::from_raw_mode(st.st_mode) != FileType::RegularFile
                        || st.st_uid != euid)
                {
                    return Err(StoreReadyError::Untrusted);
                }
            }

            let fd = open(
                &open_path,
                OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )
            .map_err(|_| StoreReadyError::Untrusted)?;
            let st = fstat(&fd).map_err(|_| StoreReadyError::Untrusted)?;
            if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile || st.st_uid != euid {
                return Err(StoreReadyError::Untrusted);
            }
            fchmod(&fd, Mode::from_raw_mode(0o600)).map_err(|_| StoreReadyError::Untrusted)?;
            Ok((open_path, pre_existed))
        }
        #[cfg(not(unix))]
        {
            fs::create_dir_all(parent).map_err(|_| StoreReadyError::Untrusted)?;
            let meta = fs::symlink_metadata(&self.database_path);
            let pre_existed = match meta {
                Ok(ref m) => {
                    if m.file_type().is_symlink() {
                        return Err(StoreReadyError::Untrusted);
                    }
                    if m.file_type().is_dir() {
                        return Err(StoreReadyError::NeedsRebuild);
                    }
                    true
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => false,
                Err(_) => return Err(StoreReadyError::Untrusted),
            };
            let canon_parent = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
            Ok((canon_parent.join(file_name), pre_existed))
        }
    }

    #[cfg(unix)]
    fn repair_store_permissions(&self, open_path: &Path) {
        use rustix::fs::{FileType, Mode, OFlags, fchmod, fstat, open};
        use rustix::process::geteuid;

        let euid = geteuid().as_raw();
        let base = open_path.to_string_lossy().into_owned();
        for suffix in ["", "-wal", "-shm"] {
            let candidate = PathBuf::from(format!("{base}{suffix}"));
            if let Ok(fd) = open(
                &candidate,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) && let Ok(st) = fstat(&fd)
                && FileType::from_raw_mode(st.st_mode) == FileType::RegularFile
                && st.st_uid == euid
                && (st.st_mode & 0o777) != 0o600
            {
                let _ = fchmod(&fd, Mode::from_raw_mode(0o600));
            }
        }
    }

    #[cfg(not(unix))]
    fn repair_store_permissions(&self, _open_path: &Path) {}

    fn open_and_validate(&self, open_path: &Path, existed: bool) -> rusqlite::Result<Connection> {
        #[cfg(unix)]
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_FULL_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        #[cfg(not(unix))]
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_FULL_MUTEX;

        let conn = Connection::open_with_flags(open_path, flags)?;
        let _ = conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true);
        let _ = conn.pragma_update(None, "trusted_schema", false);
        conn.busy_timeout(Duration::from_millis(self.busy_timeout_ms))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;

        if existed {
            self.validate_existing(&conn)?;
        } else {
            conn.execute_batch("PRAGMA auto_vacuum=INCREMENTAL; VACUUM;")?;
            self.create_schema(&conn)?;
        }
        Ok(conn)
    }

    fn validate_existing(&self, conn: &Connection) -> rusqlite::Result<()> {
        let user_version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let stored_hash: String = conn.query_row(
            "SELECT value FROM meta WHERE key = 'parser_hash'",
            [],
            |r| r.get(0),
        )?;
        if user_version != self.expected_schema_version || stored_hash != self.expected_parser_hash
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let quick_check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if quick_check != "ok" {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let auto_vacuum: i32 = conn.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
        if auto_vacuum != 2 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        Ok(())
    }

    fn create_schema(&self, conn: &Connection) -> rusqlite::Result<()> {
        conn.execute_batch(SCHEMA_SQL)?;
        conn.execute_batch(&format!(
            "PRAGMA user_version = {};",
            self.expected_schema_version
        ))?;
        conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES ('parser_hash', ?1)",
            params![self.expected_parser_hash],
        )?;
        Ok(())
    }

    fn rebuild_database(&mut self) {
        let base = self.database_path.to_string_lossy().into_owned();
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let candidate = PathBuf::from(format!("{base}{suffix}"));
            if let Ok(meta) = fs::symlink_metadata(&candidate) {
                if meta.file_type().is_symlink() {
                    continue;
                }
                if meta.file_type().is_dir() {
                    let _ = fs::remove_dir_all(&candidate);
                } else {
                    let _ = fs::remove_file(&candidate);
                }
            }
        }
        self.rebuild_count += 1;
    }
}
