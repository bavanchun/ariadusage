// Ported from CodexBar Tests/CodexBarTests/AntigravityOfflineStoreTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityOfflineFallbackProofTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityProfileScanTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityProfileSettingsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityHubIdentityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use ariadusage_core::config::ProviderConfig;
use ariadusage_core::pipeline::{
    FetchContext, FetchError, FetchInteraction, FetchPhase, FetchRuntime, FetchStrategy,
    TransportClass,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_engine::ConfigStore;
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::providers::antigravity::{
    AntigravityLocalReader, AntigravityOAuthClient, AntigravityOAuthConfig,
    AntigravityOAuthCredentials, AntigravityOAuthCredentialsStore, AntigravityOAuthFetchStrategy,
    AntigravityOfflineFetchStrategy, AntigravityOfflinePriorError, AntigravityOfflineStore,
    AntigravityRemoteFetchError, AntigravityRemoteUsageFetcher, AntigravityStatusProbeError,
    AntigravityStrategyConfig, Context as LocalContext, Coverage as LocalCoverage, Limits,
    RemoteHttpRequest, RemoteHttpResponse,
};
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::{TempDir, tempdir};
use tokio_util::sync::CancellationToken;

fn make_context(
    env: BTreeMap<String, String>,
    selected_token_account: Option<String>,
) -> FetchContext {
    let mut secret_env = BTreeMap::new();
    for (k, v) in env {
        secret_env.insert(k, SecretString::new(v));
    }
    FetchContext {
        provider: ProviderId::new("antigravity").unwrap(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "test-antigravity-offline".to_owned(),
        source_mode: SourceMode::Auto,
        env: secret_env,
        include_credits: false,
        selected_token_account,
        cancel: CancellationToken::new(),
    }
}

// ============================================================================
// AntigravityOfflineStoreTests.swift
// ============================================================================

// upstream: AntigravityOfflineStoreTests.swift:6 resolves gemini home from env override
#[test]
fn resolves_gemini_home_from_env_override() {
    let home = Path::new("/Users/test");
    let env_override = "/tmp/custom-gemini";
    let mut env = BTreeMap::new();
    env.insert("GEMINI_CLI_HOME".to_owned(), env_override.to_owned());
    let resolved = AntigravityOfflineStore::gemini_home_directory(home, &env);
    assert_eq!(resolved, PathBuf::from(env_override));
    let empty_env: BTreeMap<String, String> = BTreeMap::new();
    let fallback = AntigravityOfflineStore::gemini_home_directory(home, &empty_env);
    assert_eq!(fallback, PathBuf::from("/Users/test/.gemini"));
}

// upstream: AntigravityOfflineStoreTests.swift:18 counts db files in conversations directory
#[test]
fn counts_db_files_in_conversations_directory() {
    let tmp = tempdir().unwrap();
    let empty_env: BTreeMap<String, String> = BTreeMap::new();
    let conv = AntigravityOfflineStore::conversations_directory(tmp.path(), &empty_env);
    fs::create_dir_all(&conv).unwrap();
    assert_eq!(
        AntigravityOfflineStore::count_conversations(tmp.path(), &empty_env),
        0
    );
    assert!(!AntigravityOfflineStore::has_offline_data(
        tmp.path(),
        &empty_env
    ));
    fs::write(conv.join("a.db"), b"").unwrap();
    fs::write(conv.join("b.DB"), b"").unwrap();
    fs::write(conv.join("c.txt"), b"").unwrap();
    assert_eq!(
        AntigravityOfflineStore::count_conversations(tmp.path(), &empty_env),
        2
    );
    assert!(AntigravityOfflineStore::has_offline_data(
        tmp.path(),
        &empty_env
    ));
}

// upstream: AntigravityOfflineStoreTests.swift:34 falls back to tokscale cache when no db
#[test]
fn falls_back_to_tokscale_cache_when_no_db() {
    let tmp = tempdir().unwrap();
    let empty_env: BTreeMap<String, String> = BTreeMap::new();
    let cache = AntigravityOfflineStore::tokscale_cache_directory(tmp.path());
    fs::create_dir_all(&cache).unwrap();
    fs::write(cache.join("x.jsonl"), b"").unwrap();
    fs::write(cache.join("y.jsonl"), b"").unwrap();
    assert_eq!(
        AntigravityOfflineStore::count_conversations(tmp.path(), &empty_env),
        2
    );
}

// upstream: AntigravityOfflineStoreTests.swift:46 prefers db count over cache
#[test]
fn prefers_db_count_over_cache() {
    let tmp = tempdir().unwrap();
    let empty_env: BTreeMap<String, String> = BTreeMap::new();
    let conv = AntigravityOfflineStore::conversations_directory(tmp.path(), &empty_env);
    let cache = AntigravityOfflineStore::tokscale_cache_directory(tmp.path());
    fs::create_dir_all(&conv).unwrap();
    fs::create_dir_all(&cache).unwrap();
    fs::write(conv.join("a.db"), b"").unwrap();
    fs::write(cache.join("x.jsonl"), b"").unwrap();
    fs::write(cache.join("y.jsonl"), b"").unwrap();
    assert_eq!(
        AntigravityOfflineStore::count_conversations(tmp.path(), &empty_env),
        1
    );
}

// ============================================================================
// AntigravityOfflineFallbackProofTests.swift
// ============================================================================

// upstream: AntigravityOfflineFallbackProofTests.swift:7 counts db in app-data directory
#[test]
fn counts_db_in_app_data_directory() {
    let tmp = tempdir().unwrap();
    let empty_env: BTreeMap<String, String> = BTreeMap::new();
    let app_data = AntigravityOfflineStore::app_data_directory(tmp.path(), &empty_env);
    fs::create_dir_all(&app_data).unwrap();
    fs::write(app_data.join("a.db"), b"").unwrap();
    fs::write(app_data.join("b.db"), b"").unwrap();
    assert_eq!(
        AntigravityOfflineStore::count_conversations(tmp.path(), &empty_env),
        2
    );
    assert!(AntigravityOfflineStore::has_offline_data(
        tmp.path(),
        &empty_env
    ));
}

// upstream: AntigravityOfflineFallbackProofTests.swift:20 counts db in app-data conversations subdirectory
#[test]
fn counts_db_in_app_data_conversations_subdirectory() {
    let tmp = tempdir().unwrap();
    let empty_env: BTreeMap<String, String> = BTreeMap::new();
    let app_data_conv =
        AntigravityOfflineStore::app_data_directory(tmp.path(), &empty_env).join("conversations");
    fs::create_dir_all(&app_data_conv).unwrap();
    fs::write(app_data_conv.join("c.db"), b"").unwrap();
    assert_eq!(
        AntigravityOfflineStore::count_conversations(tmp.path(), &empty_env),
        1
    );
}

// upstream: AntigravityOfflineFallbackProofTests.swift:32 offline snapshot does not carry selected OAuth email
#[tokio::test]
async fn offline_snapshot_does_not_carry_selected_oauth_email() {
    let tmp = tempdir().unwrap();
    let empty_env: BTreeMap<String, String> = BTreeMap::new();
    let conv = AntigravityOfflineStore::conversations_directory(tmp.path(), &empty_env);
    fs::create_dir_all(&conv).unwrap();
    fs::write(conv.join("x.db"), b"").unwrap();

    let credentials = AntigravityOAuthCredentials::new(
        "ya29.fake",
        Some("1//fake".to_owned()),
        Some(Timestamp::from_second(1_900_000_000).unwrap()),
        None,
        Some("selected@example.com".to_owned()),
        None,
        None,
        None,
    );
    let token_value = AntigravityOAuthCredentialsStore::token_account_value(&credentials).unwrap();
    let mut env = BTreeMap::new();
    env.insert("HOME".to_owned(), tmp.path().to_string_lossy().into_owned());
    env.insert(
        AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.to_owned(),
        token_value,
    );
    let context = make_context(env, Some("account-uuid-1".to_owned()));
    let strategy = AntigravityOfflineFetchStrategy::default();
    let result = strategy.fetch(&context).await.unwrap();
    assert!(
        result
            .usage
            .identity
            .as_ref()
            .and_then(|id| id.account_email.as_ref())
            .is_none()
    );
    assert_eq!(
        result
            .usage
            .extra_rate_windows
            .as_ref()
            .and_then(|w| w.first())
            .map(|w| w.title.as_str()),
        Some("Offline \u{00b7} 1 conversation")
    );
    assert_eq!(result.source_label, "offline");
}

// upstream: AntigravityOfflineFallbackProofTests.swift:58 offline diagnostic only surfaces sanitized reasons
#[test]
fn offline_diagnostic_only_surfaces_sanitized_reasons() {
    let raw = AntigravityOfflineFetchStrategy::diagnostic_for_prior_error(
        &AntigravityOfflinePriorError::ForeignSubprocess {
            code: 1,
            stderr: "raw-secret-stderr".to_owned(),
        },
    )
    .unwrap();
    assert!(!raw.contains("raw-secret-stderr"));
    assert!(raw.contains("Diagnostics"));

    let body = AntigravityOfflineFetchStrategy::diagnostic_for_prior_error(
        &AntigravityOfflinePriorError::Probe(AntigravityStatusProbeError::ApiError(
            "HTTP 500: {\"secret\":\"token-value\"}".to_owned(),
        )),
    )
    .unwrap();
    assert!(!body.contains("token-value"));
    assert!(body.contains("HTTP 500"));

    let expired = AntigravityOfflineFetchStrategy::diagnostic_for_prior_error(
        &AntigravityOfflinePriorError::Probe(AntigravityStatusProbeError::ApiError(
            "HTTP 403: {\"detail\":\"private\"}".to_owned(),
        )),
    )
    .unwrap();
    assert!(!expired.contains("private"));
    assert!(expired.contains("session expired"));

    let classified = AntigravityOfflineFetchStrategy::diagnostic_for_prior_error(
        &AntigravityOfflinePriorError::CliReportFailedExit(1),
    )
    .unwrap();
    assert!(classified.contains("agy exited 1"));

    let remote = AntigravityOfflineFetchStrategy::diagnostic_for_prior_error(
        &AntigravityOfflinePriorError::Remote(AntigravityRemoteFetchError::ApiError(
            "HTTP 429: {\"retry\":\"secret-hint\"}".to_owned(),
        )),
    )
    .unwrap();
    assert!(!remote.contains("secret-hint"));

    let signed_out = AntigravityOfflineFetchStrategy::diagnostic_for_prior_error(
        &AntigravityOfflinePriorError::Remote(AntigravityRemoteFetchError::NotLoggedIn),
    )
    .unwrap();
    assert!(signed_out.contains("Google auth not found"));

    let bare_api = AntigravityOfflineFetchStrategy::diagnostic_for_prior_error(
        &AntigravityOfflinePriorError::Probe(AntigravityStatusProbeError::ApiError(
            "quota service warming up".to_owned(),
        )),
    )
    .unwrap();
    assert!(bare_api.ends_with("the usage request failed"));
    assert!(!bare_api.contains("warming"));

    let mismatch = AntigravityOfflineFetchStrategy::diagnostic_for_prior_error(
        &AntigravityOfflinePriorError::AccountMismatch {
            expected: Some("me@example.com".to_owned()),
            found: Some("other@example.com".to_owned()),
        },
    )
    .unwrap();
    assert!(!mismatch.contains("me@example.com"));
    assert!(!mismatch.contains("other@example.com"));
    assert!(mismatch.contains("does not match the selected account"));

    let parse = AntigravityOfflineFetchStrategy::diagnostic_for_prior_error(
        &AntigravityOfflinePriorError::Probe(AntigravityStatusProbeError::ParseFailed(
            "hypothetical dynamic detail".to_owned(),
        )),
    )
    .unwrap();
    assert!(!parse.contains("hypothetical dynamic detail"));
    assert!(parse.contains("Diagnostics"));

    let offline = AntigravityOfflineFetchStrategy::diagnostic_for_prior_error(
        &AntigravityOfflinePriorError::Transport {
            class: TransportClass::NotConnected,
            caller_detail: None,
        },
    )
    .unwrap();
    assert_eq!(
        offline,
        format!(
            "Live Antigravity usage is unavailable; showing offline data. {}",
            AntigravityOfflineFetchStrategy::transport_class_description(
                TransportClass::NotConnected
            )
        )
    );
}

// upstream: AntigravityOfflineFallbackProofTests.swift:112 offline transport diagnostics discard caller supplied details
#[test]
fn offline_transport_diagnostics_discard_caller_supplied_details() {
    for class in [
        TransportClass::NotConnected,
        TransportClass::Timeout,
        TransportClass::CannotFindHost,
    ] {
        let private_url = "https://synthetic-private.invalid/account@example.com";
        let error = AntigravityOfflinePriorError::Transport {
            class,
            caller_detail: Some(format!(
                "synthetic-private-diagnostic {private_url} synthetic-private-underlying"
            )),
        };
        let diagnostic =
            AntigravityOfflineFetchStrategy::diagnostic_for_prior_error(&error).unwrap();
        assert_eq!(
            diagnostic,
            format!(
                "Live Antigravity usage is unavailable; showing offline data. {}",
                AntigravityOfflineFetchStrategy::transport_class_description(class)
            )
        );
        assert!(!diagnostic.contains("synthetic-private"));
        assert!(!diagnostic.contains("account@example.com"));
    }
}

// upstream: AntigravityOfflineFallbackProofTests.swift:130 oauth shouldFallback when offline data exists
#[test]
fn oauth_should_fallback_when_offline_data_exists() {
    let tmp = tempdir().unwrap();
    let empty_env: BTreeMap<String, String> = BTreeMap::new();
    let conv = AntigravityOfflineStore::conversations_directory(tmp.path(), &empty_env);
    fs::create_dir_all(&conv).unwrap();
    fs::write(conv.join("a.db"), b"").unwrap();

    let mut env_with_data = BTreeMap::new();
    env_with_data.insert("HOME".to_owned(), tmp.path().to_string_lossy().into_owned());
    let ctx_with_data = make_context(env_with_data, None);

    let empty_tmp = tempdir().unwrap();
    let mut env_empty = BTreeMap::new();
    env_empty.insert(
        "HOME".to_owned(),
        empty_tmp.path().to_string_lossy().into_owned(),
    );
    let ctx_empty = make_context(env_empty, None);

    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let oauth = AntigravityOAuthFetchStrategy::new(AntigravityStrategyConfig::new(
        net,
        empty_tmp.path(),
        empty_tmp.path(),
    ));
    let dummy_err =
        FetchError::Classified(AntigravityStatusProbeError::NotRunning.into_classified_error());
    assert!(oauth.should_fallback(&dummy_err, &ctx_with_data));
    assert!(!oauth.should_fallback(&dummy_err, &ctx_empty));
}

// ============================================================================
// AntigravityProfileScanTests.swift
// ============================================================================

struct ProfileFixture {
    _temp: TempDir,
    root: PathBuf,
    environment: BTreeMap<String, String>,
    context: LocalContext,
}

impl ProfileFixture {
    fn new() -> Self {
        let temp = tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let mut environment = BTreeMap::new();
        environment.insert("HOME".to_owned(), root.to_string_lossy().into_owned());
        let context = LocalContext::new(environment.clone(), Vec::new());
        Self {
            _temp: temp,
            root,
            environment,
            context,
        }
    }

    fn cache_usage() -> String {
        concat!(
            "{\"type\":\"usage\",\"sessionId\":\"cache-session\",\"input\":100,\"output\":30,\"cacheRead\":50,",
            "\"cacheWrite\":0,\"reasoning\":0,\"timestamp\":1787832000250,\"responseId\":null}"
        )
        .to_owned()
    }

    fn blob_with(seconds: u64, response: Option<&str>, input: u64) -> Vec<u8> {
        let mut usage = [
            proto_varint(1, 1298),
            proto_varint(2, input),
            proto_varint(5, 50),
            proto_varint(9, 7),
            proto_varint(10, 30),
        ]
        .concat();
        if let Some(resp) = response {
            usage.extend_from_slice(&proto_str(11, resp));
        }
        let mut chat = proto_bytes(4, &usage);
        let ts = [proto_varint(1, seconds), proto_varint(2, 250_000_000)].concat();
        chat.extend_from_slice(&proto_bytes(9, &proto_bytes(4, &ts)));
        chat.extend_from_slice(&proto_str(19, "fixture-model-a"));
        chat.extend_from_slice(&proto_str(21, "Fixture model"));
        proto_bytes(1, &chat)
    }

    fn blob() -> Vec<u8> {
        Self::blob_with(1_787_832_000, None, 100)
    }

    fn database(&self, name: &str, root_index: usize, blobs: &[Vec<u8>]) {
        let roots = self.context.database_roots();
        let dir = &roots[root_index];
        fs::create_dir_all(dir).unwrap();
        let db_path = dir.join(format!("{name}.db"));
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB)")
            .unwrap();
        for (index, blob) in blobs.iter().enumerate() {
            conn.execute(
                "INSERT INTO gen_metadata (idx, data) VALUES (?1, ?2)",
                rusqlite::params![index as i64, blob],
            )
            .unwrap();
        }
    }

    fn jsonl(&self, lines: &[String]) {
        let cache_root = self.context.cache_root();
        fs::create_dir_all(&cache_root).unwrap();
        let path = cache_root.join("cache.jsonl");
        let mut content = lines.join("\n");
        content.push('\n');
        fs::write(path, content).unwrap();
    }
}

fn proto_tag(field: u64, wire: u64) -> Vec<u8> {
    proto_raw_varint((field << 3) | wire)
}

fn proto_varint(field: u64, value: u64) -> Vec<u8> {
    [proto_tag(field, 0), proto_raw_varint(value)].concat()
}

fn proto_bytes(field: u64, data: &[u8]) -> Vec<u8> {
    [
        proto_tag(field, 2),
        proto_raw_varint(data.len() as u64),
        data.to_vec(),
    ]
    .concat()
}

fn proto_str(field: u64, text: &str) -> Vec<u8> {
    proto_bytes(field, text.as_bytes())
}

fn proto_raw_varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    while value >= 0x80 {
        out.push(((value & 0x7f) as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
    out
}

fn profile_report(
    fixture: &ProfileFixture,
    homes: &[String],
    environment: Option<BTreeMap<String, String>>,
) -> ariadusage_engine::providers::antigravity::DailyReportResult {
    let env = environment.unwrap_or_else(|| fixture.environment.clone());
    let ctx = LocalContext::new(env, homes.to_vec());
    AntigravityLocalReader::make_daily_report_with_status(
        &ctx,
        0,
        false,
        None,
        Limits::default(),
        || 0.0,
        || Ok(()),
    )
    .unwrap()
}

// upstream: AntigravityProfileScanTests.swift:17 empty opt in keeps all primary roots and fallback unchanged
#[test]
fn empty_opt_in_keeps_all_primary_roots_and_fallback_unchanged() {
    let fixture = ProfileFixture::new();
    let context = LocalContext::new(fixture.environment.clone(), Vec::new());
    assert_eq!(
        context.database_roots(),
        vec![
            fixture.root.join(".gemini/antigravity-cli/conversations"),
            fixture.root.join(".gemini/antigravity"),
            fixture.root.join(".gemini/antigravity/conversations"),
        ]
    );
    assert_eq!(context.database_roots(), fixture.context.database_roots());
    assert_eq!(context.cache_root(), fixture.context.cache_root());
    fixture.jsonl(&[ProfileFixture::cache_usage()]);
    let normal = AntigravityLocalReader::make_daily_report_with_status(
        &fixture.context,
        0,
        false,
        None,
        Limits::default(),
        || 0.0,
        || Ok(()),
    )
    .unwrap();
    let explicit = profile_report(&fixture, &[], None);
    assert_eq!(
        serde_json::to_value(&normal.report).unwrap(),
        serde_json::to_value(&explicit.report).unwrap()
    );
    assert_eq!(normal.coverage, explicit.coverage);
}

// upstream: AntigravityProfileScanTests.swift:37 selected primary and additional profiles union September and October history
#[test]
fn selected_primary_and_additional_profiles_union_september_and_october_history() {
    let fixture = ProfileFixture::new();
    let primary = ProfileFixture::new();
    let additional = ProfileFixture::new();
    fixture.database("excluded-default", 0, &[ProfileFixture::blob()]);
    primary.database(
        "september-29",
        2,
        &[ProfileFixture::blob_with(1_790_683_200, None, 100)],
    );
    additional.database(
        "september-30",
        0,
        &[ProfileFixture::blob_with(1_790_769_600, None, 100)],
    );
    additional.database(
        "october-1",
        1,
        &[ProfileFixture::blob_with(1_790_856_000, None, 100)],
    );
    let mut environment = fixture.environment.clone();
    environment.insert(
        "GEMINI_CLI_HOME".to_owned(),
        primary.root.join(".gemini").to_string_lossy().into_owned(),
    );
    let report = profile_report(
        &fixture,
        &[additional
            .root
            .join(".gemini")
            .to_string_lossy()
            .into_owned()],
        Some(environment),
    );
    assert_eq!(report.coverage, LocalCoverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-09-29", "2026-09-30", "2026-10-01"]
    );
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(561)
    );
}

// upstream: AntigravityProfileScanTests.swift:55 copied profile conversations preserve new rows and cross conversation response identities
#[test]
fn copied_profile_conversations_preserve_new_rows_and_cross_conversation_response_identities() {
    let primary = ProfileFixture::new();
    let profile = ProfileFixture::new();
    let blobs = vec![
        ProfileFixture::blob(),
        ProfileFixture::blob_with(1_787_832_000, Some("same-response"), 100),
    ];
    primary.database("conversation-a", 0, &blobs);
    let mut profile_blobs = blobs.clone();
    profile_blobs.push(ProfileFixture::blob_with(
        1_787_832_000,
        Some("new-response"),
        100,
    ));
    profile.database("conversation-a", 0, &profile_blobs);
    profile.database(
        "conversation-b",
        2,
        &[ProfileFixture::blob_with(
            1_787_832_000,
            Some("same-response"),
            100,
        )],
    );
    let report = profile_report(
        &primary,
        &[profile.root.join(".gemini").to_string_lossy().into_owned()],
        None,
    );
    assert_eq!(report.coverage, LocalCoverage::Complete);
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(748)
    );
    assert_eq!(
        report.report.data.first().and_then(|d| d.request_count),
        Some(4)
    );
}

// upstream: AntigravityProfileScanTests.swift:69 conflicting profile copies cannot publish a misleading total
#[test]
fn conflicting_profile_copies_cannot_publish_a_misleading_total() {
    let primary = ProfileFixture::new();
    let profile = ProfileFixture::new();
    primary.database(
        "conversation-a",
        0,
        &[ProfileFixture::blob_with(1_787_832_000, None, 100)],
    );
    profile.database(
        "conversation-a",
        0,
        &[ProfileFixture::blob_with(1_787_832_000, None, 200)],
    );
    let report = profile_report(
        &primary,
        &[profile.root.join(".gemini").to_string_lossy().into_owned()],
        None,
    );
    assert_eq!(report.coverage, LocalCoverage::Partial);
    assert!(report.evidence_is_contradicted);
}

// upstream: AntigravityProfileScanTests.swift:80 missing and invalid homes do not hide healthy primary history
#[test]
fn missing_and_invalid_homes_do_not_hide_healthy_primary_history() {
    let fixture = ProfileFixture::new();
    fixture.database("db", 0, &[ProfileFixture::blob()]);
    let missing = fixture.root.join("missing").to_string_lossy().into_owned();
    let healthy = profile_report(
        &fixture,
        &[missing, String::new(), "relative/path".to_owned()],
        None,
    );
    assert_eq!(healthy.coverage, LocalCoverage::Complete);
    assert_eq!(
        healthy.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(187)
    );

    let broken = fixture.root.join("broken").join("antigravity");
    fs::create_dir_all(broken.parent().unwrap()).unwrap();
    fs::write(&broken, b"not a directory").unwrap();
    let partial = profile_report(
        &fixture,
        &[broken.parent().unwrap().to_string_lossy().into_owned()],
        None,
    );
    assert_eq!(partial.coverage, LocalCoverage::Partial);
    assert_eq!(
        partial.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(187)
    );
}

// upstream: AntigravityProfileScanTests.swift:98 tilde and symlink aliases scan once and home set order does not change scope
#[test]
fn tilde_and_symlink_aliases_scan_once_and_home_set_order_does_not_change_scope() {
    let fixture = ProfileFixture::new();
    fixture.database("db", 0, &[ProfileFixture::blob()]);
    let home = fixture.root.join(".gemini");
    let link = fixture.root.join("alias");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&home, &link).unwrap();
    let homes = vec![
        "~/.gemini".to_owned(),
        home.to_string_lossy().into_owned(),
        link.to_string_lossy().into_owned(),
    ];
    let report = profile_report(&fixture, &homes, None);
    assert_eq!(report.statistics.sqlite_handles_opened, 1);
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(187)
    );
    let forward = AntigravityLocalReader::antigravity_history_scope(&fixture.environment, &homes);
    let mut reversed = homes.clone();
    reversed.reverse();
    let backward =
        AntigravityLocalReader::antigravity_history_scope(&fixture.environment, &reversed);
    assert_eq!(forward, backward);
    assert_eq!(
        forward,
        AntigravityLocalReader::antigravity_history_scope(&fixture.environment, &[])
    );
}

// ============================================================================
// AntigravityProfileSettingsTests.swift
// ============================================================================

// upstream: AntigravityProfileSettingsTests.swift:9 antigravity profile directories default empty and persist add remove edits
#[test]
fn antigravity_profile_directories_default_empty_and_persist_add_remove_edits() {
    let tmp = tempdir().unwrap();
    let config_path = tmp.path().join("config.json");
    let store = ConfigStore::new(config_path.clone());

    let initial = store.load_effective().unwrap();
    assert_eq!(
        initial
            .provider_config("antigravity")
            .and_then(|c| c.antigravity_additional_profile_homes()),
        None
    );

    let homes = vec![
        "/Synthetic/Profiles/Work/.gemini".to_owned(),
        "/Synthetic/Profiles/Personal/.gemini".to_owned(),
    ];
    let mut config = initial;
    let mut ag_cfg = ProviderConfig::new(ProviderId::new("antigravity").unwrap());
    ag_cfg.set_antigravity_additional_profile_homes(Some(homes.clone()));
    config.set_provider_config(ag_cfg);
    store.save(&config).unwrap();

    let reloaded_store = ConfigStore::new(config_path.clone());
    let persisted = reloaded_store.load().unwrap().unwrap();
    assert_eq!(
        persisted
            .provider_config("antigravity")
            .and_then(|c| c.antigravity_additional_profile_homes()),
        Some(homes.clone())
    );
    assert_eq!(
        persisted
            .provider_config("gemini")
            .and_then(|c| c.antigravity_additional_profile_homes()),
        None
    );

    let mut updated = persisted.clone();
    updated
        .provider_config_mut("antigravity")
        .unwrap()
        .set_antigravity_additional_profile_homes(Some(vec![homes[1].clone()]));
    reloaded_store.save(&updated).unwrap();
    assert_eq!(
        ConfigStore::new(config_path.clone())
            .load()
            .unwrap()
            .unwrap()
            .provider_config("antigravity")
            .and_then(|c| c.antigravity_additional_profile_homes()),
        Some(vec![homes[1].clone()])
    );

    updated
        .provider_config_mut("antigravity")
        .unwrap()
        .set_antigravity_additional_profile_homes(Some(Vec::new()));
    reloaded_store.save(&updated).unwrap();
    assert_eq!(
        ConfigStore::new(config_path)
            .load()
            .unwrap()
            .unwrap()
            .provider_config("antigravity")
            .and_then(|c| c.antigravity_additional_profile_homes()),
        Some(Vec::new())
    );
}

// ============================================================================
// AntigravityHubIdentityTests.swift
// ============================================================================

fn decode_form_body(bytes: &[u8]) -> BTreeMap<String, String> {
    let text = std::str::from_utf8(bytes).expect("utf8 form body");
    let mut map = BTreeMap::new();
    for pair in text.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').expect("key=value");
        map.insert(percent_decode(k), percent_decode(v));
    }
    map
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
            let byte = u8::from_str_radix(hex, 16).unwrap();
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn sample_quota_response() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "buckets": [
            {
                "modelId": "gemini-2.5-pro",
                "remainingFraction": 0.75,
                "resetTime": "2026-06-01T12:00:00Z"
            },
            {
                "modelId": "gemini-2.5-flash",
                "remainingFraction": 0.5,
                "resetTime": "2026-06-01T12:00:00Z"
            },
            {
                "modelId": "claude-sonnet-4-5",
                "remainingFraction": 0.25,
                "resetTime": "2026-06-01T12:00:00Z"
            }
        ]
    }))
    .unwrap()
}

fn quota_summary_json(gemini_session: f64, gemini_weekly: f64) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "groups": [
            {
                "displayName": "Gemini Models",
                "buckets": [
                    {
                        "bucketId": "gemini-5h",
                        "window": "5h",
                        "remainingFraction": gemini_session,
                        "resetTime": "2026-06-01T05:00:00Z"
                    },
                    {
                        "bucketId": "gemini-weekly",
                        "window": "weekly",
                        "remainingFraction": gemini_weekly,
                        "resetTime": "2026-06-07T00:00:00Z"
                    }
                ]
            }
        ]
    }))
    .unwrap()
}

// upstream: AntigravityHubIdentityTests.swift:18 quota requests use the Hub identity for every credential source
#[tokio::test]
async fn quota_requests_use_the_hub_identity_for_every_credential_source() {
    for account in ["shared", "selected-a", "selected-b"] {
        for summary_available in [true, false] {
            let tmp = tempdir().unwrap();
            let shared_creds = AntigravityOAuthCredentials::new(
                "shared-token",
                Some("shared-refresh".to_owned()),
                Some(Timestamp::from_second(1).unwrap()),
                None,
                Some("shared@example.com".to_owned()),
                None,
                Some("synthetic-client".to_owned()),
                Some("synthetic-secret".to_owned()),
            );
            let shared_url = AntigravityOAuthCredentialsStore::credentials_url(tmp.path());
            AntigravityOAuthCredentialsStore::save(&shared_creds, &shared_url).unwrap();

            let selected = AntigravityOAuthCredentials::new(
                format!("{account}-token"),
                Some(format!("{account}-refresh")),
                Some(Timestamp::from_second(1).unwrap()),
                None,
                Some(format!("{account}@example.com")),
                None,
                Some("synthetic-client".to_owned()),
                Some("synthetic-secret".to_owned()),
            );
            let mut environment = BTreeMap::new();
            if account != "shared" {
                environment.insert(
                    AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.to_owned(),
                    AntigravityOAuthCredentialsStore::token_account_value(&selected).unwrap(),
                );
            }

            let captured: Arc<Mutex<Vec<RemoteHttpRequest>>> = Arc::new(Mutex::new(Vec::new()));
            let captured_clone = captured.clone();
            let account_str = account.to_owned();

            let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(10), tmp.path())
                .with_environment(environment)
                .with_oauth_client_resolver(|| None)
                .with_data_loader(move |request| {
                    let captured_clone = captured_clone.clone();
                    let account_str = account_str.clone();
                    Box::pin(async move {
                        captured_clone.lock().unwrap().push(request.clone());
                        let host = request.url.host_str().unwrap_or("");
                        let path = request.url.path();
                        let mut status = 200;
                        let body = if host == "oauth2.googleapis.com" {
                            assert!(request.header("User-Agent").is_none());
                            let form = decode_form_body(request.body.as_deref().unwrap());
                            assert_eq!(
                                form.get("refresh_token").map(String::as_str),
                                Some(format!("{account_str}-refresh").as_str())
                            );
                            serde_json::to_vec(&json!({
                                "access_token": format!("{account_str}-refreshed"),
                                "expires_in": 3600
                            }))
                            .unwrap()
                        } else {
                            assert!(
                                host == "cloudcode-pa.googleapis.com"
                                    || host == "daily-cloudcode-pa.googleapis.com"
                            );
                            assert_eq!(
                                request.header("Authorization"),
                                Some(format!("Bearer {account_str}-refreshed").as_str())
                            );
                            if [
                                "retrieveUserQuotaSummary",
                                "fetchAvailableModels",
                                "retrieveUserQuota",
                            ]
                            .iter()
                            .any(|suffix| path.ends_with(suffix))
                            {
                                let req_json: Value =
                                    serde_json::from_slice(request.body.as_deref().unwrap())
                                        .unwrap();
                                assert_eq!(
                                    req_json.get("project").and_then(Value::as_str),
                                    Some(format!("{account_str}-project").as_str())
                                );
                            }
                            match path {
                                "/v1internal:loadCodeAssist" => {
                                    let req_json: Value =
                                        serde_json::from_slice(request.body.as_deref().unwrap())
                                            .unwrap();
                                    assert_eq!(
                                        req_json.get("metadata"),
                                        Some(&json!({
                                            "ideType": "ANTIGRAVITY",
                                            "platform": "PLATFORM_UNSPECIFIED",
                                            "pluginType": "GEMINI"
                                        }))
                                    );
                                    serde_json::to_vec(&json!({
                                        "allowedTiers": [{"id": "standard-tier", "isDefault": true}]
                                    }))
                                    .unwrap()
                                }
                                "/v1internal:onboardUser" => {
                                    let req_json: Value =
                                        serde_json::from_slice(request.body.as_deref().unwrap())
                                            .unwrap();
                                    assert_eq!(
                                        req_json.get("metadata"),
                                        Some(&json!({
                                            "ideType": "ANTIGRAVITY",
                                            "platform": "PLATFORM_UNSPECIFIED",
                                            "pluginType": "GEMINI"
                                        }))
                                    );
                                    assert_eq!(
                                        req_json.get("tierId").and_then(Value::as_str),
                                        Some("standard-tier")
                                    );
                                    serde_json::to_vec(&json!({
                                        "response": {
                                            "cloudaicompanionProject": {
                                                "id": format!(" {account_str}-project \n")
                                            }
                                        }
                                    }))
                                    .unwrap()
                                }
                                "/v1internal:retrieveUserQuotaSummary" if summary_available => {
                                    let session = if host == "daily-cloudcode-pa.googleapis.com" {
                                        0.88
                                    } else {
                                        1.0
                                    };
                                    let weekly = if host == "daily-cloudcode-pa.googleapis.com" {
                                        0.0
                                    } else {
                                        0.69
                                    };
                                    quota_summary_json(session, weekly)
                                }
                                "/v1internal:retrieveUserQuota" => sample_quota_response(),
                                _ => {
                                    status = 403;
                                    Vec::new()
                                }
                            }
                        };
                        Ok(RemoteHttpResponse { status, body })
                    })
                });

            let snapshot = fetcher.fetch().await.unwrap();
            assert_eq!(
                snapshot.account_email.as_deref(),
                Some(format!("{account}@example.com").as_str())
            );
            assert_eq!(snapshot.has_known_quota_summary(), summary_available);
            assert!(summary_available || snapshot.model_quotas.len() == 3);
            if summary_available {
                let usage = snapshot.to_usage_snapshot().unwrap();
                let first_two: Vec<i64> = usage
                    .extra_rate_windows
                    .unwrap()
                    .iter()
                    .take(2)
                    .map(|w| w.window.remaining_percent().round() as i64)
                    .collect();
                assert_eq!(first_two, vec![88, 0]);
            }

            let requests = captured.lock().unwrap().clone();
            let cloud_requests: Vec<&RemoteHttpRequest> = requests
                .iter()
                .filter(|r| r.url.host_str() == Some("cloudcode-pa.googleapis.com"))
                .collect();
            let mut expected_cloud_paths =
                vec!["/v1internal:loadCodeAssist", "/v1internal:onboardUser"];
            if !summary_available {
                expected_cloud_paths.push("/v1internal:fetchAvailableModels");
                expected_cloud_paths.push("/v1internal:retrieveUserQuota");
            }
            assert_eq!(
                cloud_requests
                    .iter()
                    .map(|r| r.url.path())
                    .collect::<Vec<_>>(),
                expected_cloud_paths
            );

            let daily_requests: Vec<&RemoteHttpRequest> = requests
                .iter()
                .filter(|r| r.url.host_str() == Some("daily-cloudcode-pa.googleapis.com"))
                .collect();
            assert_eq!(
                daily_requests
                    .iter()
                    .map(|r| r.url.path())
                    .collect::<Vec<_>>(),
                vec!["/v1internal:retrieveUserQuotaSummary"]
            );

            let expected_ua = AntigravityRemoteUsageFetcher::user_agent();
            for req in cloud_requests.into_iter().chain(daily_requests) {
                assert_eq!(req.header("User-Agent"), Some(expected_ua));
            }

            if account != "shared" {
                let shared_reloaded = AntigravityOAuthCredentialsStore::load(&shared_url).unwrap();
                assert_eq!(shared_reloaded.access_token, "shared-token");
            }
        }
    }
}

// ============================================================================
// Installed OAuth Client Negative Cache (PR #52 Review Low 4)
// ============================================================================

#[cfg(feature = "test-hooks")]
#[test]
fn installed_client_negative_cache_hits_and_invalidates_on_artifact_fingerprint_change() {
    let tmp = tempdir().unwrap();
    let app_root = tmp.path().join("Apps");
    let main_js = app_root
        .join("Antigravity.app")
        .join("Contents/Resources/app/out/main.js");
    fs::create_dir_all(main_js.parent().unwrap()).unwrap();
    fs::write(&main_js, b"// placeholder without oauth client").unwrap();

    // 1. First discovery reads the artifact and caches the negative result (`None`).
    let reads_before_first = AntigravityOAuthConfig::installed_client_artifact_read_count();
    assert_eq!(
        AntigravityOAuthConfig::discover_client_from_installed_app(&[&app_root]),
        None
    );
    let reads_after_first = AntigravityOAuthConfig::installed_client_artifact_read_count();
    assert_eq!(reads_after_first, reads_before_first + 1);

    // 2. Repeated discovery with unchanged `(path, mtime, length)` hits the negative cache
    // without re-reading the artifact.
    assert_eq!(
        AntigravityOAuthConfig::discover_client_from_installed_app(&[&app_root]),
        None
    );
    let reads_after_second = AntigravityOAuthConfig::installed_client_artifact_read_count();
    assert_eq!(reads_after_second, reads_after_first);

    // 3. Changing `mtime` while keeping the same byte length invalidates the negative cache.
    let file = fs::File::options().write(true).open(&main_js).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000))
        .unwrap();
    drop(file);
    assert_eq!(
        AntigravityOAuthConfig::discover_client_from_installed_app(&[&app_root]),
        None
    );
    let reads_after_mtime_change = AntigravityOAuthConfig::installed_client_artifact_read_count();
    assert_eq!(reads_after_mtime_change, reads_after_second + 1);

    // 4. Updating the artifact with valid OAuth client credentials changes `(mtime, length)`
    // and invalidates the negative cache, returning the newly discovered client.
    fs::write(
        &main_js,
        concat!(
            "/* vs/platform/cloudCode/common/oauthClient.js */ ",
            "{clientId:\"123456789-testclient.apps.googleusercontent.com\",",
            "clientSecret:\"GOCSPX-testsecretvalue\"}"
        ),
    )
    .unwrap();
    let discovered = AntigravityOAuthConfig::discover_client_from_installed_app(&[&app_root]);
    let reads_after_update = AntigravityOAuthConfig::installed_client_artifact_read_count();
    assert_eq!(reads_after_update, reads_after_mtime_change + 1);
    assert_eq!(
        discovered,
        Some(AntigravityOAuthClient::new(
            "123456789-testclient.apps.googleusercontent.com",
            "GOCSPX-testsecretvalue"
        ))
    );
}

#[tokio::test]
async fn offline_strategy_wires_local_reader_and_additional_profile_homes() {
    let primary = ProfileFixture::new();
    let additional = ProfileFixture::new();
    primary.database(
        "september-29",
        2,
        &[ProfileFixture::blob_with(1_790_683_200, None, 100)],
    );
    additional.database(
        "september-30",
        0,
        &[ProfileFixture::blob_with(1_790_769_600, None, 100)],
    );

    let mut provider_config = ProviderConfig::new(ProviderId::new("antigravity").unwrap());
    provider_config.set_antigravity_additional_profile_homes(Some(vec![
        additional
            .root
            .join(".gemini")
            .to_string_lossy()
            .into_owned(),
    ]));

    let strategy = AntigravityOfflineFetchStrategy::with_home_directory(&primary.root)
        .with_provider_config(&provider_config);
    let cx = make_context(primary.environment.clone(), None);

    assert!(strategy.is_available(&cx).await);
    let fetched = strategy.fetch(&cx).await.unwrap();
    assert_eq!(fetched.source_label, "offline");
    assert_eq!(
        fetched
            .usage
            .extra_rate_windows
            .as_ref()
            .and_then(|w| w.first())
            .map(|w| w.title.as_str()),
        Some("Offline \u{00b7} 2 conversations")
    );

    let report = strategy.load_local_daily_report(&cx).unwrap();
    assert_eq!(report.coverage, LocalCoverage::Complete);
    assert_eq!(
        report
            .report
            .data
            .iter()
            .map(|d| d.date.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-09-29", "2026-09-30"]
    );
    assert_eq!(
        report.report.summary.as_ref().and_then(|s| s.total_tokens),
        Some(374)
    );

    let token_snapshot = strategy.load_local_token_snapshot(&cx).unwrap();
    assert!(token_snapshot.history_coverage_is_established);
    assert_eq!(token_snapshot.daily.len(), 2);
}
