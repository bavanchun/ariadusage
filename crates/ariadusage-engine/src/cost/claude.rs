// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner+Claude.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner+ClaudeRowDecoding.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageClaudeJSON.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageJsonl.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::pricing::{ClaudeCostTokens, ModelsDevCatalog, claude_cost_usd, normalize_claude_model};
use super::timestamp::{CostUsageDayRange, claude_timestamp_and_day_key};

/// Options for scanning Claude Code JSONL session logs.
#[derive(Debug, Clone, Default)]
pub struct ClaudeScanOptions {
    pub claude_projects_roots: Vec<PathBuf>,
    pub provider_filter: ClaudeLogProviderFilter,
    pub timezone: Option<TimeZone>,
    pub catalog: Option<ModelsDevCatalog>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ClaudeLogProviderFilter {
    #[default]
    All,
    VertexAiOnly,
    ExcludeVertexAi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaudePathRole {
    Parent,
    Subagent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeUsageRow {
    pub day_key: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_unix_ms: Option<i64>,
    pub is_sidechain: bool,
    pub path_role: ClaudePathRole,
    pub input: i64,
    pub cache_read: i64,
    pub cache_create: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_create_1h: Option<i64>,
    pub output: i64,
    pub cost_nanos: i64,
    #[serde(default)]
    pub cost_priced: bool,
    #[serde(default)]
    pub is_incomplete: bool,
}

impl ClaudeUsageRow {
    #[must_use]
    pub const fn total_tokens(&self) -> i64 {
        self.input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_create)
            .saturating_add(self.output)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeParseResult {
    pub rows: Vec<ClaudeUsageRow>,
    pub parsed_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClaudeRowKey {
    Request {
        message_id: String,
        request_id: String,
    },
    Session {
        session_id: String,
        message_id: String,
    },
}

const VERTEX_PROVIDER_KEYS: &[&str] = &[
    "provider",
    "platform",
    "backend",
    "api_provider",
    "apiprovider",
    "api_type",
    "apitype",
    "source",
    "vendor",
    "client",
];

#[derive(Clone)]
pub struct JsonlLine {
    pub bytes: Vec<u8>,
    pub was_truncated: bool,
    pub start_offset: u64,
    pub end_offset: u64,
}

impl std::fmt::Debug for JsonlLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonlLine")
            .field("byte_len", &self.bytes.len())
            .field("was_truncated", &self.was_truncated)
            .field("start_offset", &self.start_offset)
            .field("end_offset", &self.end_offset)
            .finish()
    }
}

pub(crate) fn open_regular_file_nofollow(file_path: &Path) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        use rustix::fs::{FileType, Mode, OFlags, fstat, open};
        let fd = open(
            file_path,
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
        Ok(File::from(fd))
    }
    #[cfg(not(unix))]
    {
        let meta = std::fs::symlink_metadata(file_path)?;
        if !meta.file_type().is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        File::open(file_path)
    }
}

/// Scans a JSONL file from `offset`, invoking `on_line` for each newline-delimited or
/// structurally complete EOF-terminated JSON line. Returns the committed byte offset.
pub fn scan_jsonl<F>(
    file_path: &Path,
    offset: u64,
    max_line_bytes: usize,
    prefix_bytes: usize,
    mut on_line: F,
) -> std::io::Result<u64>
where
    F: FnMut(JsonlLine),
{
    let mut file = open_regular_file_nofollow(file_path)?;
    if offset > 0 {
        file.seek(SeekFrom::Start(offset))?;
    }

    let mut buf = vec![0u8; 64 * 1024];
    let mut current = Vec::with_capacity(4 * 1024);
    let mut line_bytes: usize = 0;
    let mut truncated = false;
    let mut bytes_read: u64 = 0;
    let mut line_start_offset: u64 = offset;
    let mut committed_offset: u64 = offset;

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            if line_bytes > 0 && has_complete_json_tail(&current, line_bytes, truncated) {
                let end_offset = offset + bytes_read;
                on_line(JsonlLine {
                    bytes: std::mem::take(&mut current),
                    was_truncated: truncated,
                    start_offset: line_start_offset,
                    end_offset,
                });
                committed_offset = end_offset;
            }
            break;
        }

        let chunk = &buf[..n];
        let chunk_start_offset = offset + bytes_read;
        bytes_read += n as u64;

        let mut segment_start = 0;
        while segment_start < n {
            if let Some(rel_idx) = chunk[segment_start..].iter().position(|&b| b == b'\n') {
                let idx = segment_start + rel_idx;
                let seg = &chunk[segment_start..idx];
                line_bytes += seg.len();
                if current.len() < prefix_bytes {
                    let append_count = (prefix_bytes - current.len()).min(seg.len());
                    current.extend_from_slice(&seg[..append_count]);
                }
                if line_bytes > max_line_bytes || line_bytes > prefix_bytes {
                    truncated = true;
                }
                let line_end_offset = chunk_start_offset + (idx as u64) + 1;
                if line_bytes > 0 {
                    on_line(JsonlLine {
                        bytes: std::mem::take(&mut current),
                        was_truncated: truncated,
                        start_offset: line_start_offset,
                        end_offset: line_end_offset,
                    });
                    line_bytes = 0;
                    truncated = false;
                }
                committed_offset = line_end_offset;
                line_start_offset = committed_offset;
                segment_start = idx + 1;
            } else {
                let seg = &chunk[segment_start..];
                line_bytes += seg.len();
                if current.len() < prefix_bytes {
                    let append_count = (prefix_bytes - current.len()).min(seg.len());
                    current.extend_from_slice(&seg[..append_count]);
                }
                if line_bytes > max_line_bytes || line_bytes > prefix_bytes {
                    truncated = true;
                }
                break;
            }
        }
    }

    Ok(committed_offset)
}

fn has_complete_json_tail(bytes: &[u8], line_bytes: usize, truncated: bool) -> bool {
    if truncated {
        return false;
    }
    if line_bytes != bytes.len() {
        return false;
    }
    serde_json::from_slice::<Value>(bytes).is_ok()
}

pub fn contains_ascii(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn could_contain_vertex_ai_metadata(line: &[u8]) -> bool {
    const MARKERS: &[&[u8]] = &[
        b"vertex", b"Vertex", b"gcp", b"Gcp", b"\\u004", b"\\u005", b"\\u006", b"\\u007",
    ];
    MARKERS.iter().any(|m| contains_ascii(line, m))
}

fn contains_claude_vertex_marker(value: &str, include_gcp: bool) -> bool {
    let lower = value.to_lowercase();
    lower.contains("vertex") || (include_gcp && lower.contains("gcp"))
}

fn contains_vertex_ai_metadata(dict: &Map<String, Value>) -> bool {
    dict.iter().any(|(key, value)| {
        if contains_claude_vertex_marker(key, true) {
            return true;
        }
        if VERTEX_PROVIDER_KEYS.contains(&key.to_lowercase().as_str())
            && let Some(text) = value.as_str()
            && contains_claude_vertex_marker(text, false)
        {
            return true;
        }
        if let Some(nested) = value.as_object() {
            return contains_vertex_ai_metadata(nested);
        }
        // Array elements descend into dictionaries only, never into another array.
        if let Some(arr) = value.as_array() {
            return arr
                .iter()
                .any(|entry| entry.as_object().is_some_and(contains_vertex_ai_metadata));
        }
        false
    })
}

pub fn is_vertex_ai_usage_entry(
    obj: &Map<String, Value>,
    message: Option<&Map<String, Value>>,
    scan_metadata: bool,
) -> bool {
    if let Some(msg_id) = message.and_then(|m| m.get("id")).and_then(Value::as_str)
        && msg_id.contains("_vrtx_")
    {
        return true;
    }
    if let Some(req_id) = obj.get("requestId").and_then(Value::as_str)
        && req_id.contains("_vrtx_")
    {
        return true;
    }
    if let Some(model) = message.and_then(|m| m.get("model")).and_then(Value::as_str)
        && model.starts_with("claude-")
        && model.contains('@')
    {
        return true;
    }
    scan_metadata && contains_vertex_ai_metadata(obj)
}

pub fn is_vertex_ai_usage_value(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };
    let message = obj.get("message").and_then(Value::as_object);
    is_vertex_ai_usage_entry(obj, message, true)
}

pub fn claude_path_role(file_path: &Path) -> ClaudePathRole {
    let path_str = file_path.to_string_lossy();
    if path_str.contains("/subagents/") || path_str.contains("\\subagents\\") {
        ClaudePathRole::Subagent
    } else {
        ClaudePathRole::Parent
    }
}

pub fn claude_canonical_row_key(row: &ClaudeUsageRow) -> Option<ClaudeRowKey> {
    let message_id = row.message_id.as_ref()?;
    if let Some(request_id) = &row.request_id {
        return Some(ClaudeRowKey::Request {
            message_id: message_id.clone(),
            request_id: request_id.clone(),
        });
    }
    if message_id.trim().is_empty() {
        return None;
    }
    let session_id = row.session_id.as_ref()?;
    if session_id.trim().is_empty() {
        return None;
    }
    Some(ClaudeRowKey::Session {
        session_id: session_id.clone(),
        message_id: message_id.clone(),
    })
}

pub fn should_replace_claude_row(existing: Option<&ClaudeUsageRow>, row: &ClaudeUsageRow) -> bool {
    !row.is_incomplete || existing.is_none() || existing.is_some_and(|r| r.is_incomplete)
}

pub fn merge_claude_rows(
    existing: &[ClaudeUsageRow],
    delta: &[ClaudeUsageRow],
) -> Vec<ClaudeUsageRow> {
    let mut keyed_rows: BTreeMap<ClaudeRowKey, ClaudeUsageRow> = BTreeMap::new();
    let mut unkeyed_rows: Vec<ClaudeUsageRow> = Vec::new();

    for row in existing {
        if let Some(key) = claude_canonical_row_key(row) {
            keyed_rows.insert(key, row.clone());
        } else {
            unkeyed_rows.push(row.clone());
        }
    }
    for row in delta {
        if let Some(key) = claude_canonical_row_key(row) {
            if should_replace_claude_row(keyed_rows.get(&key), row) {
                keyed_rows.insert(key, row.clone());
            }
        } else {
            unkeyed_rows.push(row.clone());
        }
    }

    ordered_claude_rows(keyed_rows, unkeyed_rows)
}

fn ordered_claude_rows(
    keyed: BTreeMap<ClaudeRowKey, ClaudeUsageRow>,
    mut unkeyed: Vec<ClaudeUsageRow>,
) -> Vec<ClaudeUsageRow> {
    let mut rows: Vec<ClaudeUsageRow> = keyed.into_values().collect();
    rows.append(&mut unkeyed);
    rows
}

pub fn claude_row_wins(
    lhs_path: &str,
    lhs_row: &ClaudeUsageRow,
    rhs_path: &str,
    rhs_row: &ClaudeUsageRow,
) -> bool {
    if lhs_row.is_incomplete != rhs_row.is_incomplete {
        return !lhs_row.is_incomplete;
    }
    if lhs_row.is_sidechain != rhs_row.is_sidechain {
        return rhs_row.is_sidechain;
    }
    if lhs_row.path_role != rhs_row.path_role {
        return rhs_row.path_role == ClaudePathRole::Subagent;
    }
    lhs_path < rhs_path
}

pub fn reconciled_claude_rows(
    files: &BTreeMap<String, Vec<ClaudeUsageRow>>,
) -> Vec<ClaudeUsageRow> {
    let mut rows: Vec<ClaudeUsageRow> = Vec::new();
    let mut winners: BTreeMap<ClaudeRowKey, (&str, &ClaudeUsageRow)> = BTreeMap::new();

    for (path, file_rows) in files {
        for row in file_rows {
            let Some(canonical_key) = claude_canonical_row_key(row) else {
                rows.push(row.clone());
                continue;
            };
            if let Some(&(existing_path, existing_row)) = winners.get(&canonical_key)
                && !claude_row_wins(path, row, existing_path, existing_row)
            {
                continue;
            }
            winners.insert(canonical_key, (path.as_str(), row));
        }
    }

    rows.reserve(winners.len());
    for (_, row) in winners.into_values() {
        rows.push(row.clone());
    }
    rows
}

fn to_int(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_u64().and_then(|u| i64::try_from(u).ok()))
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        Some(Value::Bool(b)) => i64::from(*b),
        _ => 0,
    }
}

fn to_bool(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(u) = n.as_u64() {
                u != 0
            } else if let Some(f) = n.as_f64() {
                f != 0.0
            } else {
                false
            }
        }
        _ => false,
    }
}

fn claude_one_hour_cache_creation_tokens(usage: &Map<String, Value>, total: i64) -> i64 {
    let Some(cache_creation) = usage.get("cache_creation").and_then(Value::as_object) else {
        return 0;
    };
    let tokens = to_int(cache_creation.get("ephemeral_1h_input_tokens"));
    total.min(tokens.max(0))
}

pub fn parse_claude_file(
    file_path: &Path,
    range: &CostUsageDayRange,
    provider_filter: ClaudeLogProviderFilter,
    start_offset: u64,
    path_role: ClaudePathRole,
    catalog: Option<&ModelsDevCatalog>,
) -> ClaudeParseResult {
    let mut keyed_rows: BTreeMap<ClaudeRowKey, ClaudeUsageRow> = BTreeMap::new();
    let mut unkeyed_rows: Vec<ClaudeUsageRow> = Vec::new();

    let max_line_bytes = 512 * 1024;
    let prefix_bytes = max_line_bytes;
    let cost_scale = 1_000_000_000.0_f64;

    let parsed_bytes = scan_jsonl(
        file_path,
        start_offset,
        max_line_bytes,
        prefix_bytes,
        |line| {
            if line.bytes.is_empty() || line.was_truncated {
                return;
            }
            if !contains_ascii(&line.bytes, b"\"type\":\"assistant\"")
                && !contains_ascii(&line.bytes, b"\"type\": \"assistant\"")
                && !contains_ascii(&line.bytes, b"\"assistant\"")
            {
                return;
            }
            if !contains_ascii(&line.bytes, b"\"usage\"") {
                return;
            }

            let could_contain_metadata = provider_filter != ClaudeLogProviderFilter::All
                && could_contain_vertex_ai_metadata(&line.bytes);
            if provider_filter == ClaudeLogProviderFilter::VertexAiOnly
                && !could_contain_metadata
                && !contains_ascii(&line.bytes, b"@")
                && !contains_ascii(&line.bytes, b"_vrtx_")
            {
                return;
            }

            let Ok(Value::Object(obj)) = serde_json::from_slice::<Value>(&line.bytes) else {
                return;
            };
            let Some(ty) = obj.get("type").and_then(Value::as_str) else {
                return;
            };
            if ty != "assistant" {
                return;
            }
            let message = obj.get("message").and_then(Value::as_object);
            if provider_filter != ClaudeLogProviderFilter::All {
                let is_vertex = is_vertex_ai_usage_entry(&obj, message, could_contain_metadata);
                if is_vertex != (provider_filter == ClaudeLogProviderFilter::VertexAiOnly) {
                    return;
                }
            }

            let Some(ts_text) = obj.get("timestamp").and_then(Value::as_str) else {
                return;
            };
            let Some((timestamp, day_key)) =
                claude_timestamp_and_day_key(ts_text, range.timezone())
            else {
                return;
            };
            if !CostUsageDayRange::is_in_range(
                &day_key,
                range.scan_since_key.as_str(),
                range.scan_until_key.as_str(),
            ) {
                return;
            }

            let Some(message) = message else {
                return;
            };
            let Some(model) = message.get("model").and_then(Value::as_str) else {
                return;
            };
            let Some(usage) = message.get("usage").and_then(Value::as_object) else {
                return;
            };

            let input = to_int(usage.get("input_tokens")).max(0);
            let cache_create = to_int(usage.get("cache_creation_input_tokens")).max(0);
            let cache_read = to_int(usage.get("cache_read_input_tokens")).max(0);
            let output = to_int(usage.get("output_tokens")).max(0);
            if input == 0 && cache_create == 0 && cache_read == 0 && output == 0 {
                return;
            }
            let cache_create_1h = claude_one_hour_cache_creation_tokens(usage, cache_create);

            let is_incomplete = message.get("stop_reason").is_some_and(Value::is_null)
                && input > 0
                && output == 0
                && !usage.contains_key("cache_read_input_tokens")
                && !usage.contains_key("cache_creation_input_tokens");

            let cost = if is_incomplete {
                None
            } else {
                claude_cost_usd(
                    model,
                    ClaudeCostTokens {
                        input,
                        cache_read,
                        cache_creation: cache_create,
                        cache_creation_1h: cache_create_1h,
                        output,
                    },
                    Some(timestamp.as_second()),
                    catalog,
                    None,
                )
            };
            let cost_nanos = cost.and_then(|c| {
                let scaled = (c * cost_scale).round();
                if scaled.is_finite() && scaled >= (i64::MIN as f64) && scaled <= (i64::MAX as f64)
                {
                    Some(scaled as i64)
                } else {
                    None
                }
            });

            let message_id = message.get("id").and_then(Value::as_str).map(String::from);
            let request_id = obj
                .get("requestId")
                .and_then(Value::as_str)
                .map(String::from);
            let session_id = obj
                .get("sessionId")
                .and_then(Value::as_str)
                .or_else(|| obj.get("session_id").and_then(Value::as_str))
                .or_else(|| {
                    obj.get("metadata")
                        .and_then(Value::as_object)
                        .and_then(|m| m.get("sessionId"))
                        .and_then(Value::as_str)
                })
                .or_else(|| {
                    message
                        .get("metadata")
                        .and_then(Value::as_object)
                        .and_then(|m| m.get("sessionId"))
                        .and_then(Value::as_str)
                })
                .map(String::from);

            let normalized_model = normalize_claude_model(model);
            let row = ClaudeUsageRow {
                day_key,
                model: normalized_model,
                session_id,
                message_id,
                request_id,
                timestamp_unix_ms: Some(timestamp.as_millisecond()),
                is_sidechain: to_bool(obj.get("isSidechain")),
                path_role,
                input,
                cache_read,
                cache_create,
                cache_create_1h: Some(cache_create_1h),
                output,
                cost_nanos: cost_nanos.unwrap_or(0),
                cost_priced: cost_nanos.is_some(),
                is_incomplete,
            };

            if let Some(key) = claude_canonical_row_key(&row) {
                if should_replace_claude_row(keyed_rows.get(&key), &row) {
                    keyed_rows.insert(key, row);
                }
            } else {
                unkeyed_rows.push(row);
            }
        },
    )
    .unwrap_or(start_offset);

    let rows = ordered_claude_rows(keyed_rows, unkeyed_rows);
    ClaudeParseResult { rows, parsed_bytes }
}

pub fn discover_claude_jsonl_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in roots {
        let Ok(meta) = std::fs::symlink_metadata(root) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        let Ok(canon_root) = std::fs::canonicalize(root) else {
            continue;
        };
        let mut visited = BTreeSet::new();
        visited.insert(canon_root.clone());
        collect_jsonl_files_recursive(root, &canon_root, &mut visited, &mut files);
    }
    files.sort();
    files.dedup();
    files
}

fn collect_jsonl_files_recursive(
    dir: &Path,
    canon_root: &Path,
    visited: &mut BTreeSet<PathBuf>,
    out: &mut Vec<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            let Ok(canon_dir) = std::fs::canonicalize(&path) else {
                continue;
            };
            if !canon_dir.starts_with(canon_root) || !visited.insert(canon_dir) {
                continue;
            }
            collect_jsonl_files_recursive(&path, canon_root, visited, out);
        } else if meta.is_file() && path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
            let Ok(canon_file) = std::fs::canonicalize(&path) else {
                continue;
            };
            if canon_file.starts_with(canon_root) {
                out.push(path);
            }
        }
    }
}

/// Discovers and scans all Claude JSONL files under `options.claude_projects_roots`,
/// reconciling parent vs subagent duplicates across files.
#[must_use]
pub fn scan_claude_projects(
    range: &CostUsageDayRange,
    options: &ClaudeScanOptions,
) -> Vec<ClaudeUsageRow> {
    let files = discover_claude_jsonl_files(&options.claude_projects_roots);
    let mut file_rows: BTreeMap<String, Vec<ClaudeUsageRow>> = BTreeMap::new();
    for file in files {
        let role = claude_path_role(&file);
        let parsed = parse_claude_file(
            &file,
            range,
            options.provider_filter,
            0,
            role,
            options.catalog.as_ref(),
        );
        if !parsed.rows.is_empty() {
            file_rows.insert(file.to_string_lossy().into_owned(), parsed.rows);
        }
    }
    reconciled_claude_rows(&file_rows)
}
