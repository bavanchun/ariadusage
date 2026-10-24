// Ported from CodexBar Tests/CodexBarTests/Issue2037FixtureSupport.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/Issue2037FixtureHarnessTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/Issue2037ProvenanceFixtureTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/Issue2037ScannerIntegrationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use ariadusage_engine::cost::codex::{
    self, CodexScanOptions, CodexSnapshotAccumulator, CodexTotals,
};
use ariadusage_engine::cost::timestamp::{BucketTimeZone, DayRange, utc_ymd_hms};
use ariadusage_engine::cost::{CostScannerOptions, LocalCostProvider, load_daily_report};
use jiff::tz::TimeZone;
use serde::Deserialize;
use serde_json::Value;
use tempfile::TempDir;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureManifest {
    schema_version: u32,
    redaction_version: u32,
    family_alias: String,
    files: Vec<FixtureFile>,
    copied_prefixes: Vec<CopiedPrefix>,
    oracle: Option<Oracle>,
    scanner_oracle: Option<ScannerOracle>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureFile {
    alias: String,
    relative_path: String,
    #[allow(dead_code)]
    source_role: Option<String>,
    #[allow(dead_code)]
    leaf_session_alias: Option<String>,
    #[allow(dead_code)]
    parent_session_alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopiedPrefix {
    parent_alias: String,
    child_alias: String,
    length: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Oracle {
    parent_event_count: usize,
    child_event_count: usize,
    copied_prefix_length: usize,
    parent_last_tokens: u64,
    child_last_tokens: u64,
    copied_prefix_last_tokens: u64,
    naive_last_tokens: u64,
    deduped_last_tokens: u64,
    copied_prefix_timestamp_mismatches: usize,
    parent_has_total_token_usage_drop: bool,
    child_has_total_token_usage_drop: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScannerOracle {
    naive_scanner_units: u64,
    deduped_scanner_units: u64,
    prefix_scanner_units: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionMetadata {
    id: String,
    forked_from_id: Option<String>,
    timestamp: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TokenUsage {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    total_tokens: u64,
}

impl TokenUsage {
    fn from_value(val: &Value) -> Self {
        Self {
            input_tokens: val.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            cached_input_tokens: val
                .get("cached_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: val
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            reasoning_output_tokens: val
                .get("reasoning_output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            total_tokens: val.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
        }
    }

    fn scanner_units(&self) -> u64 {
        self.input_tokens + self.cached_input_tokens + self.output_tokens
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TokenEvent {
    timestamp: String,
    last: TokenUsage,
    total: TokenUsage,
}

impl TokenEvent {
    fn fingerprint(&self) -> (TokenUsage, TokenUsage) {
        (self.last.clone(), self.total.clone())
    }
}

struct SanitizedForkFamilyFixture {
    root_dir: PathBuf,
    manifest: FixtureManifest,
}

impl SanitizedForkFamilyFixture {
    fn load(name: &str) -> Self {
        let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("fixtures/codexbar/cost-usage/issue-2037")
            .join(name);
        let manifest_bytes = fs::read(root_dir.join("manifest.json")).expect("read manifest.json");
        let manifest: FixtureManifest =
            serde_json::from_slice(&manifest_bytes).expect("decode manifest.json");
        assert_eq!(manifest.schema_version, 1);
        assert_eq!(manifest.redaction_version, 1);
        Self { root_dir, manifest }
    }

    fn install_in(&self, dest_root: &Path) -> PathBuf {
        for file in &self.manifest.files {
            assert!(
                !file.relative_path.starts_with('/') && !file.relative_path.contains(".."),
                "unsafe relative path: {}",
                file.relative_path
            );
            let src = self.root_dir.join(&file.relative_path);
            let dst = dest_root.join(&file.relative_path);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent).expect("create parent dir");
            }
            fs::copy(&src, &dst).expect("copy fixture file");
        }
        dest_root.join("codex-home")
    }

    fn file_path(&self, alias: &str) -> PathBuf {
        let entry = self
            .manifest
            .files
            .iter()
            .find(|f| f.alias == alias)
            .unwrap_or_else(|| panic!("missing file alias {alias}"));
        self.root_dir.join(&entry.relative_path)
    }

    fn json_objects(&self, alias: &str) -> Vec<Value> {
        let content = fs::read_to_string(self.file_path(alias)).expect("read jsonl");
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("parse jsonl line"))
            .collect()
    }

    fn session_metadata(&self, alias: &str) -> SessionMetadata {
        for obj in self.json_objects(alias) {
            if obj.get("type").and_then(Value::as_str) == Some("session_meta") {
                let payload = obj.get("payload").expect("payload");
                return SessionMetadata {
                    id: payload
                        .get("id")
                        .and_then(Value::as_str)
                        .expect("id")
                        .to_owned(),
                    forked_from_id: payload
                        .get("forked_from_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    timestamp: payload
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .expect("timestamp")
                        .to_owned(),
                };
            }
        }
        panic!("missing session_meta for {alias}");
    }

    fn events(&self, alias: &str) -> Vec<TokenEvent> {
        let mut out = Vec::new();
        for obj in self.json_objects(alias) {
            if obj.get("type").and_then(Value::as_str) != Some("event_msg") {
                continue;
            }
            if obj.pointer("/payload/type").and_then(Value::as_str) != Some("token_count") {
                continue;
            }
            let info = obj.pointer("/payload/info").expect("payload.info");
            let last = TokenUsage::from_value(info.get("last_token_usage").expect("last"));
            let total = TokenUsage::from_value(info.get("total_token_usage").expect("total"));
            let timestamp = obj
                .get("timestamp")
                .and_then(Value::as_str)
                .expect("timestamp")
                .to_owned();
            out.push(TokenEvent {
                timestamp,
                last,
                total,
            });
        }
        out
    }
}

fn longest_common_normalized_prefix(parent: &[TokenEvent], child: &[TokenEvent]) -> usize {
    parent
        .iter()
        .zip(child.iter())
        .take_while(|(l, r)| l.fingerprint() == r.fingerprint())
        .count()
}

fn has_total_token_usage_drop(events: &[TokenEvent]) -> bool {
    events
        .windows(2)
        .any(|w| w[1].total.total_tokens < w[0].total.total_tokens)
}

fn assert_provenance_safe_fields(name: &str) {
    let fixture = SanitizedForkFamilyFixture::load(name);
    let usage_keys = BTreeSet::from([
        "cached_input_tokens",
        "input_tokens",
        "output_tokens",
        "reasoning_output_tokens",
        "total_tokens",
    ]);

    for alias in ["parent", "child"] {
        for record in fixture.json_objects(alias) {
            let record_obj = record.as_object().expect("record object");
            let top_keys: BTreeSet<&str> = record_obj.keys().map(String::as_str).collect();
            assert_eq!(top_keys, BTreeSet::from(["payload", "timestamp", "type"]));
            let record_type = record.get("type").and_then(Value::as_str).unwrap();
            let payload = record.get("payload").and_then(Value::as_object).unwrap();
            let payload_keys: BTreeSet<&str> = payload.keys().map(String::as_str).collect();

            match record_type {
                "session_meta" => {
                    assert_eq!(
                        payload_keys,
                        BTreeSet::from(["forked_from_id", "id", "timestamp"])
                    );
                }
                "turn_context" => {
                    assert!(
                        payload_keys.is_subset(&BTreeSet::from([
                            "model",
                            "multi_agent_mode",
                            "multi_agent_version"
                        ])),
                        "unexpected turn_context keys: {payload_keys:?}"
                    );
                    assert_eq!(
                        payload.get("model").and_then(Value::as_str),
                        Some("fixture-model")
                    );
                }
                "event_msg" => {
                    assert_eq!(payload_keys, BTreeSet::from(["info", "type"]));
                    assert_eq!(
                        payload.get("type").and_then(Value::as_str),
                        Some("token_count")
                    );
                    let info = payload.get("info").and_then(Value::as_object).unwrap();
                    let info_keys: BTreeSet<&str> = info.keys().map(String::as_str).collect();
                    assert_eq!(
                        info_keys,
                        BTreeSet::from(["last_token_usage", "total_token_usage"])
                    );
                    for usage_name in ["last_token_usage", "total_token_usage"] {
                        let usage = info.get(usage_name).and_then(Value::as_object).unwrap();
                        let u_keys: BTreeSet<&str> = usage.keys().map(String::as_str).collect();
                        assert_eq!(u_keys, usage_keys);
                    }
                }
                other => panic!("unexpected record type: {other}"),
            }
        }
    }
}

fn scanned_units_from_report(
    report: &ariadusage_engine::cost::ledger::CostUsageDailyReport,
) -> u64 {
    report
        .data
        .iter()
        .map(|d| {
            d.input_tokens.unwrap_or(0)
                + d.cache_read_tokens.unwrap_or(0)
                + d.output_tokens.unwrap_or(0)
        })
        .sum()
}

// Upstream test ports (each marker identifies one @Test declaration).

// upstream: Issue2037FixtureHarnessTests.swift:6 issue 2037 fixture harness installs a sanitized family into an isolated codex home
#[test]
fn issue_2037_fixture_harness_installs_a_sanitized_family_into_an_isolated_codex_home() {
    let fixture = SanitizedForkFamilyFixture::load("harness-smoke");
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = fixture.install_in(tmp.path());

    assert_eq!(fixture.manifest.schema_version, 1);
    assert_eq!(fixture.manifest.redaction_version, 1);
    assert_eq!(fixture.manifest.family_alias, "harness-smoke");
    let aliases: Vec<&str> = fixture
        .manifest
        .files
        .iter()
        .map(|f| f.alias.as_str())
        .collect();
    assert_eq!(aliases, vec!["parent", "child"]);
    assert_eq!(
        fixture.manifest.copied_prefixes,
        vec![CopiedPrefix {
            parent_alias: "parent".to_owned(),
            child_alias: "child".to_owned(),
            length: 1,
        }]
    );

    for file in &fixture.manifest.files {
        let installed = tmp.path().join(&file.relative_path);
        assert!(installed.is_file(), "missing {}", file.alias);
        let contents = fs::read_to_string(&installed).unwrap();
        assert!(contents.contains("\"session_meta\""));
        assert!(contents.contains("\"token_count\""));
    }

    let day = utc_ymd_hms(2026, 7, 11, 12, 0, 0);
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

    assert_eq!(report.data.len(), 1);
    let entry = &report.data[0];
    assert_eq!(entry.input_tokens, Some(15));
    assert_eq!(entry.cache_read_tokens, Some(0));
    assert_eq!(entry.output_tokens, Some(2));
    assert_eq!(entry.total_tokens, Some(17));
}

// upstream: Issue2037ProvenanceFixtureTests.swift:5 archived fork fixture preserves the copied normalized prefix and hand oracle
#[test]
fn archived_fork_fixture_preserves_the_copied_normalized_prefix_and_hand_oracle() {
    let fixture = SanitizedForkFamilyFixture::load("archived-fork-33ce-3869");
    let oracle = fixture.manifest.oracle.as_ref().expect("oracle");
    let parent_meta = fixture.session_metadata("parent");
    let child_meta = fixture.session_metadata("child");
    let parent = fixture.events("parent");
    let child = fixture.events("child");
    let prefix = fixture
        .manifest
        .copied_prefixes
        .iter()
        .find(|p| p.parent_alias == "parent" && p.child_alias == "child")
        .expect("prefix");

    let actual_prefix_len = longest_common_normalized_prefix(&parent, &child);
    let copied_parent = &parent[..prefix.length];
    let copied_child = &child[..prefix.length];
    let parent_last_tokens: u64 = parent.iter().map(|e| e.last.total_tokens).sum();
    let child_last_tokens: u64 = child.iter().map(|e| e.last.total_tokens).sum();
    let copied_last_tokens: u64 = copied_child.iter().map(|e| e.last.total_tokens).sum();
    let naive_last_tokens = parent_last_tokens + child_last_tokens;
    let deduped_last_tokens = parent_last_tokens
        + child[prefix.length..]
            .iter()
            .map(|e| e.last.total_tokens)
            .sum::<u64>();
    let timestamp_mismatches = copied_parent
        .iter()
        .zip(copied_child.iter())
        .filter(|(l, r)| l.timestamp != r.timestamp)
        .count();

    assert_eq!(parent.len(), oracle.parent_event_count);
    assert_eq!(child.len(), oracle.child_event_count);
    assert_eq!(parent_meta.id, "parent-session");
    assert_eq!(parent_meta.forked_from_id, None);
    assert!(!parent_meta.timestamp.is_empty());
    assert_eq!(child_meta.id, "child-session");
    assert_eq!(child_meta.forked_from_id.as_deref(), Some("parent-session"));
    assert!(!child_meta.timestamp.is_empty());
    assert_eq!(actual_prefix_len, prefix.length);
    assert_eq!(prefix.length, oracle.copied_prefix_length);
    let p_fp: Vec<_> = copied_parent.iter().map(TokenEvent::fingerprint).collect();
    let c_fp: Vec<_> = copied_child.iter().map(TokenEvent::fingerprint).collect();
    assert_eq!(p_fp, c_fp);

    assert_eq!(parent_last_tokens, oracle.parent_last_tokens);
    assert_eq!(child_last_tokens, oracle.child_last_tokens);
    assert_eq!(copied_last_tokens, oracle.copied_prefix_last_tokens);
    assert_eq!(naive_last_tokens, oracle.naive_last_tokens);
    assert_eq!(deduped_last_tokens, oracle.deduped_last_tokens);
    assert!(naive_last_tokens > deduped_last_tokens);
    assert_eq!(
        timestamp_mismatches,
        oracle.copied_prefix_timestamp_mismatches
    );
    assert!(timestamp_mismatches > 0);
    assert_eq!(
        has_total_token_usage_drop(&parent),
        oracle.parent_has_total_token_usage_drop
    );
    assert_eq!(
        has_total_token_usage_drop(&child),
        oracle.child_has_total_token_usage_drop
    );
}

// upstream: Issue2037ProvenanceFixtureTests.swift:54 live fork 4d90 fixture preserves the copied normalized prefix and hand oracle
#[test]
fn live_fork_4d90_fixture_preserves_the_copied_normalized_prefix_and_hand_oracle() {
    let fixture = SanitizedForkFamilyFixture::load("live-fork-4d90-52bf");
    let oracle = fixture.manifest.oracle.as_ref().expect("oracle");
    let parent_meta = fixture.session_metadata("parent");
    let child_meta = fixture.session_metadata("child");
    let parent = fixture.events("parent");
    let child = fixture.events("child");
    let prefix = fixture
        .manifest
        .copied_prefixes
        .iter()
        .find(|p| p.parent_alias == "parent" && p.child_alias == "child")
        .expect("prefix");

    let actual_prefix_len = longest_common_normalized_prefix(&parent, &child);
    let copied_parent = &parent[..prefix.length];
    let copied_child = &child[..prefix.length];
    let parent_last_tokens: u64 = parent.iter().map(|e| e.last.total_tokens).sum();
    let child_last_tokens: u64 = child.iter().map(|e| e.last.total_tokens).sum();
    let copied_last_tokens: u64 = copied_child.iter().map(|e| e.last.total_tokens).sum();
    let naive_last_tokens = parent_last_tokens + child_last_tokens;
    let deduped_last_tokens = parent_last_tokens
        + child[prefix.length..]
            .iter()
            .map(|e| e.last.total_tokens)
            .sum::<u64>();
    let timestamp_mismatches = copied_parent
        .iter()
        .zip(copied_child.iter())
        .filter(|(l, r)| l.timestamp != r.timestamp)
        .count();

    assert_eq!(parent.len(), oracle.parent_event_count);
    assert_eq!(child.len(), oracle.child_event_count);
    assert_eq!(parent_meta.id, "parent-session");
    assert_eq!(parent_meta.forked_from_id, None);
    assert_eq!(child_meta.id, "child-session");
    assert_eq!(child_meta.forked_from_id.as_deref(), Some("parent-session"));
    assert_eq!(actual_prefix_len, prefix.length);
    assert_eq!(prefix.length, oracle.copied_prefix_length);
    let p_fp: Vec<_> = copied_parent.iter().map(TokenEvent::fingerprint).collect();
    let c_fp: Vec<_> = copied_child.iter().map(TokenEvent::fingerprint).collect();
    assert_eq!(p_fp, c_fp);

    assert_eq!(parent_last_tokens, oracle.parent_last_tokens);
    assert_eq!(child_last_tokens, oracle.child_last_tokens);
    assert_eq!(copied_last_tokens, oracle.copied_prefix_last_tokens);
    assert_eq!(naive_last_tokens, oracle.naive_last_tokens);
    assert_eq!(deduped_last_tokens, oracle.deduped_last_tokens);
    assert!(naive_last_tokens > deduped_last_tokens);
    assert_eq!(
        timestamp_mismatches,
        oracle.copied_prefix_timestamp_mismatches
    );
    assert!(timestamp_mismatches > 0);
    assert!(!has_total_token_usage_drop(&parent));
    assert!(!has_total_token_usage_drop(&child));
}

// upstream: Issue2037ProvenanceFixtureTests.swift:100 archived fork fixture admits only provenance safe fields
#[test]
fn archived_fork_fixture_admits_only_provenance_safe_fields() {
    assert_provenance_safe_fields("archived-fork-33ce-3869");
}

// upstream: Issue2037ProvenanceFixtureTests.swift:105 live fork 4d90 fixture admits only provenance safe fields
#[test]
fn live_fork_4d90_fixture_admits_only_provenance_safe_fields() {
    assert_provenance_safe_fields("live-fork-4d90-52bf");
}

// upstream: Issue2037ScannerIntegrationTests.swift:10 archived fork family scanner matches parent-owns-prefix oracle
#[test]
fn archived_fork_family_scanner_matches_parent_owns_prefix_oracle() {
    let fixture = SanitizedForkFamilyFixture::load("archived-fork-33ce-3869");
    let oracle = fixture.manifest.oracle.as_ref().expect("oracle");
    let prefix_len = fixture.manifest.copied_prefixes[0].length;

    let parent_events = fixture.events("parent");
    let child_events = fixture.events("child");
    let expected_scanner_units: u64 = parent_events
        .iter()
        .map(|e| e.last.scanner_units())
        .sum::<u64>()
        + child_events[prefix_len..]
            .iter()
            .map(|e| e.last.scanner_units())
            .sum::<u64>();
    let naive_scanner_units: u64 = parent_events
        .iter()
        .map(|e| e.last.scanner_units())
        .sum::<u64>()
        + child_events
            .iter()
            .map(|e| e.last.scanner_units())
            .sum::<u64>();

    let tmp = TempDir::new().expect("tempdir");
    let codex_home = fixture.install_in(tmp.path());
    let since = utc_ymd_hms(2030, 1, 1, 12, 0, 0);
    let until = utc_ymd_hms(2030, 1, 2, 12, 0, 0);
    let report = load_daily_report(
        LocalCostProvider::Codex,
        since,
        until,
        until,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );

    assert!(!report.data.is_empty());
    assert!(naive_scanner_units > expected_scanner_units);
    assert!(oracle.naive_last_tokens > oracle.deduped_last_tokens);
    assert_eq!(scanned_units_from_report(&report), expected_scanner_units);
}

// upstream: Issue2037ScannerIntegrationTests.swift:69 live fork 4d90 family scanner matches parent-owns-prefix oracle
#[test]
fn live_fork_4d90_family_scanner_matches_parent_owns_prefix_oracle() {
    let fixture = SanitizedForkFamilyFixture::load("live-fork-4d90-52bf");
    let scanner_oracle = fixture
        .manifest
        .scanner_oracle
        .as_ref()
        .expect("scanner_oracle");
    let prefix_len = fixture.manifest.copied_prefixes[0].length;

    let parent_events = fixture.events("parent");
    let child_events = fixture.events("child");
    let parent_total_units = parent_events.last().unwrap().total.scanner_units();
    let prefix_end_total_units = child_events[prefix_len - 1].total.scanner_units();
    let child_end_total_units = child_events.last().unwrap().total.scanner_units();
    let expected_scanner_units =
        parent_total_units + child_end_total_units.saturating_sub(prefix_end_total_units);

    assert_eq!(parent_total_units, prefix_end_total_units);
    assert_eq!(parent_total_units, scanner_oracle.prefix_scanner_units);
    assert_eq!(expected_scanner_units, child_end_total_units);
    assert_eq!(expected_scanner_units, scanner_oracle.deduped_scanner_units);
    assert!(scanner_oracle.naive_scanner_units > scanner_oracle.deduped_scanner_units);
    assert!(
        parent_events
            .iter()
            .map(|e| e.last.scanner_units())
            .sum::<u64>()
            > parent_total_units
    );

    let tmp = TempDir::new().expect("tempdir");
    let codex_home = fixture.install_in(tmp.path());
    let since = utc_ymd_hms(2030, 1, 1, 12, 0, 0);
    let until = utc_ymd_hms(2030, 1, 2, 12, 0, 0);
    let report = load_daily_report(
        LocalCostProvider::Codex,
        since,
        until,
        until,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );

    assert!(!report.data.is_empty());
    assert_eq!(
        scanned_units_from_report(&report),
        scanner_oracle.deduped_scanner_units
    );
}

// upstream: Issue2037ScannerIntegrationTests.swift:127 missing parent equal counter siblings fail closed
#[test]
fn missing_parent_equal_counter_siblings_fail_closed() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = tmp.path().join("codex-home");
    let archived_dir = codex_home.join("archived_sessions");
    fs::create_dir_all(&archived_dir).unwrap();

    let sibling_contents = |id: &str, model: &str, timestamp: &str| -> String {
        let metadata = format!(
            "{{\"type\":\"session_meta\",\"timestamp\":\"{timestamp}\",\"payload\":{{\"id\":\"{id}\",\"forked_from_id\":\"missing-parent\",\"timestamp\":\"{timestamp}\"}}}}"
        );
        let context = format!(
            "{{\"type\":\"turn_context\",\"timestamp\":\"{timestamp}\",\"payload\":{{\"model\":\"{model}\"}}}}"
        );
        let first = format!(
            "{{\"type\":\"event_msg\",\"timestamp\":\"{timestamp}\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"last_token_usage\":{{\"input_tokens\":10,\"cached_input_tokens\":0,\"output_tokens\":0}},\"total_token_usage\":{{\"input_tokens\":10,\"cached_input_tokens\":0,\"output_tokens\":0}}}}}}}}"
        );
        let second = format!(
            "{{\"type\":\"event_msg\",\"timestamp\":\"{timestamp}\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"last_token_usage\":{{\"input_tokens\":5,\"cached_input_tokens\":0,\"output_tokens\":0}},\"total_token_usage\":{{\"input_tokens\":15,\"cached_input_tokens\":0,\"output_tokens\":0}}}}}}}}"
        );
        format!("{metadata}\n{context}\n{first}\n{second}\n")
    };

    fs::write(
        archived_dir.join("sibling-a.jsonl"),
        sibling_contents("sibling-a", "fixture-model-a", "2030-07-01T12:00:00Z"),
    )
    .unwrap();
    fs::write(
        archived_dir.join("sibling-b.jsonl"),
        sibling_contents("sibling-b", "fixture-model-b", "2030-07-01T13:00:00Z"),
    )
    .unwrap();

    let day = utc_ymd_hms(2030, 7, 1, 12, 0, 0);
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

    assert_eq!(scanned_units_from_report(&report), 0);
    let unmetered: u64 = report
        .data
        .iter()
        .map(|d| d.unmetered_request_count.unwrap_or(0))
        .sum();
    assert_eq!(unmetered, 2);
    assert!(report.data.iter().all(|d| d.cost_usd.is_none()));
}

// upstream: Issue2037ScannerIntegrationTests.swift:186 missing parent forks stay in the scan window without billed days
#[test]
fn missing_parent_forks_stay_in_the_scan_window_without_billed_days() {
    let fixture = SanitizedForkFamilyFixture::load("missing-parent-siblings");
    let tmp = TempDir::new().expect("tempdir");
    let codex_home = fixture.install_in(tmp.path());

    let range = DayRange::from_day_keys("2030-01-01", "2030-01-02", BucketTimeZone::utc());
    let scan = codex::scan_codex_home(
        &range,
        &CodexScanOptions {
            codex_home: Some(codex_home.clone()),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );
    assert_eq!(scan.files.len(), 2);
    for file in scan.files.values() {
        assert!(file.days.is_empty());
        assert!(file.rows.is_empty());
        assert!(file.has_unresolved_fork_baseline);
    }
    assert_eq!(scan.unmetered_requests_by_day.values().sum::<u64>(), 2);

    let since = utc_ymd_hms(2030, 1, 1, 12, 0, 0);
    let until = utc_ymd_hms(2030, 1, 2, 12, 0, 0);
    let report = load_daily_report(
        LocalCostProvider::Codex,
        since,
        until,
        until,
        &CostScannerOptions {
            codex_home: Some(codex_home),
            cache_root: Some(tmp.path().join("cache")),
            timezone: Some(TimeZone::UTC),
            ..Default::default()
        },
    );

    assert_eq!(scanned_units_from_report(&report), 0);
    assert!(report.data.iter().all(|d| d.cost_usd.is_none()));
    assert_eq!(
        report
            .data
            .iter()
            .map(|d| d.unmetered_request_count.unwrap_or(0))
            .sum::<u64>(),
        2
    );
}

// Rust-added tests (no direct upstream @Test equivalent; excluded from parity totals).

// Rust-added: Issue 2037 interleaved cumulative counters contain watermark gaps.
#[test]
fn issue_2037_interleaved_cumulative_counters_contain_watermark_gaps() {
    let mut acc = CodexSnapshotAccumulator::default();
    let (d1, _) = acc.apply_event(
        Some(CodexTotals::new(1000, 0, 100, None)),
        Some(CodexTotals::new(1000, 0, 100, None)),
        false,
    );
    assert_eq!(d1.input, 1000);
    let (d2, _) = acc.apply_event(
        Some(CodexTotals::new(50, 0, 5, None)),
        Some(CodexTotals::new(200, 0, 20, None)),
        false,
    );
    assert_eq!(d2.input, 0);
    let (d3, _) = acc.apply_event(
        Some(CodexTotals::new(50, 0, 5, None)),
        Some(CodexTotals::new(1050, 0, 105, None)),
        false,
    );
    assert_eq!(d3.input, 50);
    assert_eq!(d3.output, 5);
    assert_eq!(
        codex::codex_contained_total_delta(
            Some(CodexTotals::new(1000, 0, 100, None)),
            Some(CodexTotals::new(100, 0, 10, None)),
            CodexTotals::new(1050, 0, 105, None),
        )
        .input,
        50
    );
}
