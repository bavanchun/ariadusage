// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner+CodexPriority.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;

use crate::cost::pricing::{self, CodexServiceTier};
use crate::cost::timestamp::{self, DayKey, DayRange};
use crate::private_tempdir::{self, PrivateTempDir};

const MAX_CODEX_SQLITE_BYTES: u64 = 64 * 1024 * 1024;

const REQUEST_MARKER: &str = "websocket request:";
const COMPLETED_MARKER: &str = "websocket event:";
const SUBMISSION_MARKER: &str = "Submission sub=Submission {";
const SUBMISSION_PRIORITY_MARKER: &str = r#"service_tier: Some(Some("priority"))"#;

/// Metadata extracted from Codex's SQLite priority trace (`logs` table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexPriorityTurnMetadata {
    pub thread_id: Option<String>,
    pub turn_id: String,
    pub model: Option<String>,
    pub timestamp: Option<String>,
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

/// Reads priority turn metadata from Codex's SQLite `logs` table (`logs_2.sqlite` or `state_*.sqlite`).
///
/// Upgrades request aliases with completed response models for the same `turn_id` and filters
/// by local day range (`since_day_key..=until_day_key`).
#[must_use]
pub fn codex_priority_turns(
    database_path: &Path,
    since_day_key: Option<&str>,
    until_day_key: Option<&str>,
    timezone: Option<TimeZone>,
) -> BTreeMap<String, CodexPriorityTurnMetadata> {
    if !fs::symlink_metadata(database_path).is_ok_and(|m| m.file_type().is_file()) {
        return BTreeMap::new();
    }
    read_priority_turns_from_db(database_path, since_day_key, until_day_key, timezone)
}

/// Loads all priority turns across `<codex_home>/logs_2.sqlite` and any `<codex_home>/state_*.sqlite`.
#[must_use]
pub fn load_codex_priority_turns_for_home(
    codex_home: &Path,
    since_day_key: Option<&str>,
    until_day_key: Option<&str>,
    timezone: Option<TimeZone>,
) -> BTreeMap<String, CodexPriorityTurnMetadata> {
    let mut merged = BTreeMap::new();
    for db_path in discover_codex_sqlite_files(codex_home) {
        let turns = codex_priority_turns(&db_path, since_day_key, until_day_key, timezone.clone());
        for (turn_id, meta) in turns {
            merged.insert(turn_id, meta);
        }
    }
    merged
}

/// Loads session-level service tiers from Codex's `threads` table if present.
#[must_use]
pub fn codex_session_service_tiers(codex_home: &Path) -> BTreeMap<String, CodexServiceTier> {
    let mut tiers = BTreeMap::new();
    for db_path in discover_codex_sqlite_files(codex_home) {
        read_session_tiers_from_db(&db_path, &mut tiers);
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
    files.sort();
    files
}

fn copy_regular_file_nofollow(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use rustix::fs::{FileType, Mode, OFlags, fstat, open};
        use rustix::process::geteuid;

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
        if len > MAX_CODEX_SQLITE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "database exceeds max size",
            ));
        }
        let mut reader = std::fs::File::from(src_fd);
        let dst_fd = open(
            dst,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(std::io::Error::from)?;
        let mut writer = std::fs::File::from(dst_fd);
        std::io::copy(&mut reader, &mut writer)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let meta = fs::symlink_metadata(src)?;
        if !meta.file_type().is_file() || meta.len() > MAX_CODEX_SQLITE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid sqlite file",
            ));
        }
        fs::copy(src, dst)?;
        Ok(())
    }
}

/// Copies a foreign Codex SQLite database (`logs_2.sqlite` or `state_*.sqlite`) and its optional
/// `-wal` sidecar (never `-journal` or `-shm`) into a private `0700` temp directory and opens the
/// copy read-only with `SQLITE_DBCONFIG_DEFENSIVE = true` and `trusted_schema = false`.
fn open_copied_codex_sqlite(db_path: &Path) -> Option<(PrivateTempDir, Connection)> {
    let raw_temp = std::env::temp_dir();
    let temp_parent = fs::canonicalize(&raw_temp).unwrap_or(raw_temp);
    let temp_dir = private_tempdir::create_in(&temp_parent).ok()?;
    let dest_db = temp_dir.path().join("snapshot.sqlite");
    copy_regular_file_nofollow(db_path, &dest_db).ok()?;

    let mut wal_os = db_path.as_os_str().to_os_string();
    wal_os.push("-wal");
    let wal_src = PathBuf::from(wal_os);
    if fs::symlink_metadata(&wal_src).is_ok_and(|m| m.file_type().is_file()) {
        let dest_wal = temp_dir.path().join("snapshot.sqlite-wal");
        let _ = copy_regular_file_nofollow(&wal_src, &dest_wal);
    }

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
    Some((temp_dir, conn))
}

fn read_priority_turns_from_db(
    db_path: &Path,
    since_day_key: Option<&str>,
    until_day_key: Option<&str>,
    timezone: Option<TimeZone>,
) -> BTreeMap<String, CodexPriorityTurnMetadata> {
    let Some((_temp_dir, conn)) = open_copied_codex_sqlite(db_path) else {
        return BTreeMap::new();
    };

    if !table_exists(&conn, "logs") {
        return BTreeMap::new();
    }

    let columns = table_columns(&conn, "logs");
    let body_col = ["feedback_log_body", "body", "message", "log_body", "text"]
        .into_iter()
        .find(|col| columns.iter().any(|c| c.eq_ignore_ascii_case(col)));
    let Some(body_col) = body_col else {
        return BTreeMap::new();
    };
    let ts_col = ["ts", "timestamp", "created_at"]
        .into_iter()
        .find(|col| columns.iter().any(|c| c.eq_ignore_ascii_case(col)));

    let start_epoch = epoch_seconds_for_day_key(
        since_day_key.unwrap_or("0000-01-01"),
        timezone.clone(),
        -86_400,
    );
    let until_next = next_day_key_str(until_day_key.unwrap_or("9999-12-30"));
    let end_epoch = epoch_seconds_for_day_key(&until_next, timezone.clone(), 86_400);

    let sql = match ts_col {
        Some(ts) => format!(
            "SELECT {ts}, {body_col} FROM logs \
             WHERE ((typeof({ts}) = 'integer' AND {ts} >= ?1 AND {ts} < ?2) OR typeof({ts}) != 'integer') \
               AND ({body_col} LIKE '%websocket request:%' \
                    OR {body_col} LIKE '%response.completed%' \
                    OR {body_col} LIKE '%service_tier: Some(Some(\"priority\"))%') \
             ORDER BY rowid ASC"
        ),
        None => format!(
            "SELECT NULL, {body_col} FROM logs \
             WHERE {body_col} LIKE '%websocket request:%' \
                OR {body_col} LIKE '%response.completed%' \
                OR {body_col} LIKE '%service_tier: Some(Some(\"priority\"))%' \
             ORDER BY rowid ASC"
        ),
    };

    let Ok(mut stmt) = conn.prepare(&sql) else {
        return BTreeMap::new();
    };

    let mut turns: BTreeMap<String, CodexPriorityTurnMetadata> = BTreeMap::new();
    let mut completed_models: BTreeMap<String, String> = BTreeMap::new();

    let map_row = |row: &rusqlite::Row<'_>| {
        let ts_val: rusqlite::types::Value = row.get(0)?;
        let ts_str = match ts_val {
            rusqlite::types::Value::Integer(i) => Some(i.to_string()),
            rusqlite::types::Value::Text(s) => Some(s),
            _ => None,
        };
        let body: Option<String> = row.get(1).ok();
        Ok((ts_str, body))
    };

    let rows_result = if ts_col.is_some() {
        stmt.query_map(params![start_epoch, end_epoch], map_row)
    } else {
        stmt.query_map([], map_row)
    };
    let Ok(rows) = rows_result else {
        return BTreeMap::new();
    };

    for row in rows.flatten() {
        let (ts, Some(body)) = row else {
            continue;
        };
        if let Some((turn_id, completed_model)) = parse_codex_completed_trace_row(&body) {
            completed_models.insert(turn_id, completed_model);
            continue;
        }
        if let Some(parsed) = parse_codex_priority_trace_row(ts.as_deref(), &body) {
            turns.insert(parsed.turn_id.clone(), parsed);
        }
    }

    turns
        .into_iter()
        .filter_map(|(turn_id, mut meta)| {
            if !timestamp_is_in_range(
                meta.timestamp.as_deref(),
                since_day_key,
                until_day_key,
                timezone.clone(),
            ) {
                return None;
            }
            if let Some(completed_model) = completed_models.get(&turn_id) {
                meta.model = Some(completed_model.clone());
            }
            Some((turn_id, meta))
        })
        .collect()
}

fn next_day_key_str(day_key: &str) -> String {
    timestamp::add_days_to_day_key(day_key, 1).unwrap_or_else(|| day_key.to_owned())
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

fn read_session_tiers_from_db(db_path: &Path, tiers: &mut BTreeMap<String, CodexServiceTier>) {
    let Some((_temp_dir, conn)) = open_copied_codex_sqlite(db_path) else {
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
