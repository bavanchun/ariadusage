// Ported from CodexBar Tests/CodexBarTests/CLIServeAccountsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIServeAuthTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIServeDashboardIdentityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIServeRawHTTPTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIServeRouterTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIServeTestSupport.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIServeTimeoutTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIServeWebUITests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/CLIServeRequestDeadlineLinuxTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ariadusage_core::config::{
    Config, ProviderConfig, ProviderEntry, TokenAccountMeta, TokenAccountsMeta,
};
use ariadusage_core::model::{ProviderIdentitySnapshot, UsageSnapshot};
use ariadusage_engine::config_store::ConfigStore;
use ariadusage_engine::serve::accounts::{
    CliServeAccountDiscovery, CliServeAccountsPayload, CodexActiveSource,
    FileManagedCodexAccountStore, ManagedCodexAccount,
};
use ariadusage_engine::serve::auth::{
    CliServeDashboardAuth, CliServeSecurity, CliServeStartupError, serve_unauthorized_response,
};
use ariadusage_engine::serve::coordinator::{
    CliServeOperationCoordinator, CoordinatorCancelled, CoordinatorSnapshot,
};
use ariadusage_engine::serve::dashboard::{
    CodexVisibleAccount, CostPayload, DashboardIdentityMode, ProviderPayload, UsageCommandOutput,
    codex_usage_cache_account_key,
};
use ariadusage_engine::serve::http_server::{
    CliLocalHttpAllowedHosts, CliLocalHttpConnectionGate, CliLocalHttpRequest,
    CliLocalHttpRequestError, CliLocalHttpResponse, CliLocalHttpServer,
};
use ariadusage_engine::serve::provider_icons;
use ariadusage_engine::serve::web_ui::{self, PROVIDER_ICON_BY_PROVIDER_ID};
use ariadusage_engine::serve::{
    CachePolicy, CliServeArgumentError, CliServeCoordinatedResponse,
    CliServeDashboardTokenResolution, CliServeResponseCache, CliServeRoute, CliServeRouteError,
    CliServeRouter, CliTerminationSignalMonitor, DASHBOARD_COST_REFRESHES_PRICING_IN_BACKGROUND,
    SERVE_COST_REFRESHES_PRICING_IN_BACKGROUND, ServeCostCollectionContext, ServeRuntime,
    adding_no_store, cached_serve_response, clamped_serve_request_timeout,
    dashboard_identity_flag_present, decode_dashboard_identity_mode, decode_serve_host,
    decode_serve_port, decode_serve_refresh_interval, decode_serve_request_timeout,
    handle_serve_request, load_serve_config_snapshot, resolve_dashboard_identity_mode,
    resolve_dashboard_token, root_help, serve_cache_key, serve_cli_session_idle_window,
    serve_collect_cost_payloads, serve_collect_usage_outputs, serve_cost_provider_deadline,
    serve_dashboard_operation_key, serve_health_response, serve_help, serve_operation_key,
    serve_provider_timeout, serve_request_deadline, serve_stale_ttl, serve_timeout_response,
    serve_usage_operation_fingerprint, should_cache_serve_response,
};
use ariadusage_protocol::ids::ProviderId;
use ariadusage_protocol::metric::Confidence;
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Notify, watch};
use uuid::Uuid;

// ============================================================================
// Shared test helpers (`CLIServeTestSupport.swift` + router/raw HTTP helpers)
// ============================================================================

struct ServeTestClock {
    current: Mutex<Timestamp>,
}

impl ServeTestClock {
    fn new(initial: Timestamp) -> Arc<Self> {
        Arc::new(Self {
            current: Mutex::new(initial),
        })
    }

    fn now(&self) -> Timestamp {
        *self.current.lock().unwrap()
    }

    fn advance(&self, seconds: i64) {
        let mut guard = self.current.lock().unwrap();
        *guard = guard
            .checked_add(jiff::SignedDuration::from_secs(seconds))
            .unwrap();
    }
}

fn make_serve_test_cache() -> (Arc<ServeTestClock>, CliServeResponseCache) {
    let clock = ServeTestClock::new(Timestamp::from_second(1_000).unwrap());
    let c = Arc::clone(&clock);
    let cache = CliServeResponseCache::with_clock(
        CliServeOperationCoordinator::new(),
        Arc::new(move || c.now()),
    );
    (clock, cache)
}

#[derive(Clone, Default)]
struct TestGate {
    open: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl TestGate {
    fn new() -> Self {
        Self::default()
    }

    async fn wait(&self) {
        while !self.open.load(Ordering::SeqCst) {
            self.notify.notified().await;
        }
    }

    fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }
}

fn default_config() -> Config {
    Config::new(1, Vec::new())
}

fn parse_config(value: serde_json::Value) -> Config {
    ariadusage_core::config::decode(&serde_json::to_vec(&value).unwrap())
        .unwrap()
        .unwrap()
}

fn make_usage_with_email(provider: ProviderId, email: &str) -> UsageSnapshot {
    let identity =
        ProviderIdentitySnapshot::new(Some(provider), Some(email.to_string()), None, None, None);
    UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        Timestamp::now(),
        Some(identity),
        Confidence::Exact,
    )
    .unwrap()
}

async fn send_raw_http(port: u16, raw: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(raw).await.unwrap();
    let _ = stream.shutdown().await;
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut buf)).await;
    buf
}

fn make_test_runtime(
    config: &Config,
    token: Option<&str>,
    bind_host: &str,
    identity_mode: Option<DashboardIdentityMode>,
    all_accounts: bool,
) -> (tempfile::TempDir, ServeRuntime) {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    let store = ConfigStore::new(config_path);
    store.save(config).unwrap();
    let secret_token = token.map(|value| SecretString::new(value.to_string()));
    let auth = CliServeDashboardAuth::new(secret_token.as_ref());
    let mut rt = ServeRuntime::new(
        store,
        30.0,
        8.0,
        Some("0.79.0".to_string()),
        auth,
        identity_mode,
        all_accounts,
        bind_host,
    );
    rt.managed_accounts_store =
        FileManagedCodexAccountStore::new(dir.path().join("codex-accounts.json"));
    rt.usage_fetcher = Arc::new(|provider, _cfg, _sink| {
        Box::pin(async move {
            let mut payload = ProviderPayload::new(provider.clone(), "cli");
            payload.usage = Some(make_usage_with_email(
                ProviderId::new(&provider).unwrap_or_else(|_| ProviderId::new("codex").unwrap()),
                "test@example.com",
            ));
            UsageCommandOutput {
                sections: vec![format!("{provider}: ok")],
                payload: vec![payload],
                exit_code: 0,
            }
        })
    });
    rt.cost_fetcher = Arc::new(|provider, _cfg, _sink| {
        Box::pin(async move { CostPayload::from_token_snapshot(provider, None, None) })
    });
    (dir, rt)
}

fn synthetic_usage_cache_keys(data: &[u8]) -> Option<Vec<Option<String>>> {
    let items: Vec<serde_json::Value> = serde_json::from_slice(data).ok()?;
    Some(
        items
            .into_iter()
            .map(|item| {
                let provider = item.get("provider")?.as_str()?;
                let account = item
                    .get("account")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                Some(format!("test:{provider}:{account}"))
            })
            .collect(),
    )
}

fn router_test_response(
    body: &str,
    status_code: u16,
    usage_cache_keys: Option<Vec<Option<String>>>,
) -> CliLocalHttpResponse {
    let data = body.as_bytes().to_vec();
    let keys = usage_cache_keys.or_else(|| synthetic_usage_cache_keys(&data));
    let reason = if status_code == 200 {
        "OK"
    } else if status_code == 503 {
        "Service Unavailable"
    } else {
        "Error"
    };
    CliLocalHttpResponse::json(status_code, reason, data).with_usage_cache_keys(keys)
}

// ============================================================================
// 1. CLIServeAccountsTests.swift (7 tests)
// ============================================================================

fn make_token_account(
    id: Uuid,
    label: &str,
    external_identifier: Option<&str>,
    organization_id: Option<&str>,
    workspace_id: Option<&str>,
    seat_credit_entitlement: Option<&str>,
) -> TokenAccountMeta {
    TokenAccountMeta {
        id: id.hyphenated().to_string().to_ascii_uppercase(),
        label: label.to_string(),
        added_at: 1.0,
        last_used: Some(2.0),
        external_identifier: external_identifier.map(ToString::to_string),
        usage_scope: None,
        organization_id: organization_id.map(ToString::to_string),
        workspace_id: workspace_id.map(ToString::to_string),
        claude_config_directory: None,
        seat_credit_entitlement: seat_credit_entitlement.map(ToString::to_string),
        model_rate_limits: None,
        extra: BTreeMap::new(),
    }
}

fn make_managed_account(
    id: Uuid,
    workspace_label: Option<&str>,
    auth_fingerprint: Option<&str>,
    managed_home_path: &str,
) -> ManagedCodexAccount {
    ManagedCodexAccount {
        id,
        email: "managed@example.com".to_string(),
        provider_account_id: Some("provider-account".to_string()),
        workspace_label: workspace_label.map(ToString::to_string),
        workspace_account_id: Some("workspace-account".to_string()),
        auth_fingerprint: auth_fingerprint.map(ToString::to_string),
        managed_home_path: managed_home_path.to_string(),
        created_at: 1,
        updated_at: 2,
        last_authenticated_at: 3,
    }
}

fn value_contains_key(key: &str, value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.contains_key(key) || map.values().any(|v| value_contains_key(key, v))
        }
        serde_json::Value::Array(arr) => arr.iter().any(|v| value_contains_key(key, v)),
        _ => false,
    }
}

// upstream: CLIServeAccountsTests.swift:7 empty account set encodes as an empty list
#[test]
fn empty_account_set_encodes_as_an_empty_list() {
    let payload =
        CliServeAccountDiscovery::make_payload(&default_config(), &[], DashboardIdentityMode::Full);
    assert_eq!(payload.schema_version, 1);
    assert!(payload.accounts.is_empty());
    let obj: serde_json::Value = serde_json::to_value(&payload).unwrap();
    assert_eq!(obj["accounts"].as_array().unwrap().len(), 0);
}

// upstream: CLIServeAccountsTests.swift:21 projects managed and token accounts across providers with stable opaque ids
#[test]
fn projects_managed_and_token_accounts_across_providers_with_stable_opaque_ids() {
    let managed_id = Uuid::parse_str("AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE").unwrap();
    let first_claude_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
    let second_claude_id = Uuid::parse_str("66666666-7777-8888-9999-AAAAAAAAAAAA").unwrap();
    let openai_id = Uuid::parse_str("BBBBBBBB-CCCC-DDDD-EEEE-FFFFFFFFFFFF").unwrap();

    let mut codex = ProviderConfig::new(ProviderId::new("codex").unwrap());
    codex.enabled = Some(true);
    CodexActiveSource::ManagedAccount { id: managed_id }.apply_to_provider_config(&mut codex);

    let mut claude = ProviderConfig::new(ProviderId::new("claude").unwrap());
    claude.enabled = Some(true);
    claude.token_accounts = Some(TokenAccountsMeta {
        version: 1,
        active_index: 1,
        accounts: vec![
            make_token_account(first_claude_id, "Claude One", None, None, None, None),
            make_token_account(second_claude_id, "Claude Two", None, None, None, None),
        ],
    });

    let mut config = parse_config(serde_json::json!({
        "version": 1,
        "providers": [
            {
                "id": "openai",
                "enabled": true,
                "tokenAccounts": {
                    "version": 1,
                    "activeIndex": 0,
                    "accounts": [{
                        "id": openai_id.hyphenated().to_string().to_ascii_uppercase(),
                        "label": "OpenAI",
                        "token": "openai-secret",
                        "addedAt": 1.0
                    }]
                }
            }
        ]
    }));
    config.providers.insert(0, ProviderEntry::Typed(codex));
    config.providers.insert(0, ProviderEntry::Typed(claude));
    let managed = make_managed_account(
        managed_id,
        Some("Example Workspace"),
        Some("fingerprint"),
        "/synthetic/managed-home",
    );

    let payload =
        CliServeAccountDiscovery::make_payload(&config, &[managed], DashboardIdentityMode::Full);

    assert_eq!(payload.accounts.len(), 4);
    let providers: Vec<&str> = payload
        .accounts
        .iter()
        .map(|a| a.provider.as_str())
        .collect();
    assert_eq!(providers, vec!["claude", "claude", "codex", "openai"]);
    let ids: Vec<String> = payload.accounts.iter().map(|a| a.id.clone()).collect();
    assert_eq!(
        ids,
        vec![
            format!("token-account:claude:{}", first_claude_id.hyphenated()),
            format!("token-account:claude:{}", second_claude_id.hyphenated()),
            format!("codex-managed:{}", managed_id.hyphenated()),
            format!("token-account:openai:{}", openai_id.hyphenated()),
        ]
    );
    let actives: Vec<bool> = payload.accounts.iter().map(|a| a.active).collect();
    assert_eq!(actives, vec![false, true, true, true]);
    assert_eq!(payload.accounts[2].label, "Example Workspace");
    assert_eq!(
        payload.accounts[2]
            .identity
            .as_ref()
            .and_then(|i| i.account_email.as_deref()),
        Some("managed@example.com")
    );
}

// upstream: CLIServeAccountsTests.swift:69 token account ids remain distinct across providers sharing a stored uuid
#[test]
fn token_account_ids_remain_distinct_across_providers_sharing_a_stored_uuid() {
    let shared_id = Uuid::new_v4();
    let config = parse_config(serde_json::json!({
        "version": 1,
        "providers": [
            {
                "id": "claude",
                "tokenAccounts": {
                    "version": 1,
                    "activeIndex": 0,
                    "accounts": [{
                        "id": shared_id.hyphenated().to_string().to_ascii_uppercase(),
                        "label": "Shared label",
                        "token": "synthetic-token",
                        "addedAt": 1.0
                    }]
                }
            },
            {
                "id": "openai",
                "tokenAccounts": {
                    "version": 1,
                    "activeIndex": 0,
                    "accounts": [{
                        "id": shared_id.hyphenated().to_string().to_ascii_uppercase(),
                        "label": "Shared label",
                        "token": "synthetic-token",
                        "addedAt": 1.0
                    }]
                }
            }
        ]
    }));
    let full = CliServeAccountDiscovery::make_payload(&config, &[], DashboardIdentityMode::Full);
    let redacted =
        CliServeAccountDiscovery::make_payload(&config, &[], DashboardIdentityMode::Redacted);

    let full_ids: BTreeSet<&str> = full.accounts.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(full_ids.len(), 2);
    let full_vec: Vec<&str> = full.accounts.iter().map(|a| a.id.as_str()).collect();
    let redacted_vec: Vec<&str> = redacted.accounts.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(full_vec, redacted_vec);
    for account in &full.accounts {
        let resp = CliServeAccountDiscovery::response(&full, Some(&account.id));
        let obj: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
        assert_eq!(obj["provider"].as_str(), Some(account.provider.as_str()));
    }
}

// upstream: CLIServeAccountsTests.swift:92 wire payload excludes credential and internal storage fields and values
#[test]
fn wire_payload_excludes_credential_and_internal_storage_fields_and_values() {
    let token_id = Uuid::new_v4();
    let managed_id = Uuid::new_v4();
    let config = parse_config(serde_json::json!({
        "version": 1,
        "providers": [{
            "id": "claude",
            "apiKey": "TOP-SECRET-API-KEY",
            "secretKey": "TOP-SECRET-SECRET-KEY",
            "cookieHeader": "TOP-SECRET-COOKIE",
            "tokenAccounts": {
                "version": 1,
                "activeIndex": 0,
                "accounts": [{
                    "id": token_id.hyphenated().to_string().to_ascii_uppercase(),
                    "label": "safe label",
                    "token": "TOP-SECRET-TOKEN",
                    "addedAt": 1.0,
                    "lastUsed": 2.0,
                    "externalIdentifier": "private-login",
                    "organizationID": "private-org",
                    "workspaceID": "private-workspace",
                    "seatCreditEntitlement": "private-entitlement"
                }]
            }
        }]
    }));
    let managed = make_managed_account(
        managed_id,
        Some("Example Workspace"),
        Some("credential-derived-fingerprint"),
        "/Users/private/managed-home",
    );

    let payload =
        CliServeAccountDiscovery::make_payload(&config, &[managed], DashboardIdentityMode::Full);
    let json = serde_json::to_string(&payload).unwrap();
    let obj: serde_json::Value = serde_json::from_str(&json).unwrap();

    for forbidden_key in [
        "token",
        "apiKey",
        "secretKey",
        "cookieHeader",
        "authFingerprint",
        "managedHomePath",
        "externalIdentifier",
        "organizationID",
        "organizationId",
        "workspaceID",
        "seatCreditEntitlement",
    ] {
        assert!(
            !value_contains_key(forbidden_key, &obj),
            "found {forbidden_key}"
        );
    }
    for forbidden_value in [
        "TOP-SECRET",
        "private-login",
        "private-org",
        "private-workspace",
        "private-entitlement",
        "credential-derived-fingerprint",
        "/Users/private/managed-home",
    ] {
        assert!(!json.contains(forbidden_value), "found {forbidden_value}");
    }
}

// upstream: CLIServeAccountsTests.swift:139 redacted identity hides email local parts in identity and fallback labels
#[test]
fn redacted_identity_hides_email_local_parts_in_identity_and_fallback_labels() {
    let token_id = Uuid::new_v4();
    let managed_id = Uuid::new_v4();
    let config = parse_config(serde_json::json!({
        "version": 1,
        "providers": [{
            "id": "claude",
            "tokenAccounts": {
                "version": 1,
                "activeIndex": 0,
                "accounts": [{
                    "id": token_id.hyphenated().to_string().to_ascii_uppercase(),
                    "label": "token.user@example.com",
                    "token": "secret",
                    "addedAt": 1.0
                }]
            }
        }]
    }));
    let managed = make_managed_account(managed_id, None, Some("fingerprint"), "/synthetic/home");

    let payload = CliServeAccountDiscovery::make_payload(
        &config,
        &[managed],
        DashboardIdentityMode::Redacted,
    );

    assert_eq!(
        payload
            .accounts
            .iter()
            .find(|a| a.provider == "claude")
            .unwrap()
            .label,
        format!("Account {}", token_id.hyphenated())
    );
    let codex = payload
        .accounts
        .iter()
        .find(|a| a.provider == "codex")
        .unwrap();
    assert_eq!(codex.label, format!("Account {}", managed_id.hyphenated()));
    assert_eq!(
        codex
            .identity
            .as_ref()
            .and_then(|i| i.account_email.as_deref()),
        Some("redacted@example.com")
    );
    let json = serde_json::to_string(&payload).unwrap();
    assert!(!json.contains("token.user"));
    assert!(!json.contains("managed@"));
}

// upstream: CLIServeAccountsTests.swift:168 redacted labels use opaque account placeholders while full labels remain unchanged
#[test]
fn redacted_labels_use_opaque_account_placeholders_while_full_labels_remain_unchanged() {
    let token_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
    let managed_id = Uuid::parse_str("AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE").unwrap();
    let config = parse_config(serde_json::json!({
        "version": 1,
        "providers": [{
            "id": "claude",
            "tokenAccounts": {
                "version": 1,
                "activeIndex": 0,
                "accounts": [{
                    "id": token_id.hyphenated().to_string().to_ascii_uppercase(),
                    "label": "Alice Smith",
                    "token": "secret",
                    "addedAt": 1.0
                }]
            }
        }]
    }));
    let managed = make_managed_account(
        managed_id,
        Some("Personal Workspace"),
        Some("fingerprint"),
        "/synthetic/home",
    );

    let redacted = CliServeAccountDiscovery::make_payload(
        &config,
        std::slice::from_ref(&managed),
        DashboardIdentityMode::Redacted,
    );
    let full =
        CliServeAccountDiscovery::make_payload(&config, &[managed], DashboardIdentityMode::Full);

    let token_wire_id = format!("token-account:claude:{}", token_id.hyphenated());
    let managed_wire_id = format!("codex-managed:{}", managed_id.hyphenated());

    assert_eq!(
        redacted
            .accounts
            .iter()
            .find(|a| a.id == token_wire_id)
            .unwrap()
            .label,
        format!("Account {}", token_id.hyphenated())
    );
    assert_eq!(
        redacted
            .accounts
            .iter()
            .find(|a| a.id == managed_wire_id)
            .unwrap()
            .label,
        format!("Account {}", managed_id.hyphenated())
    );
    assert!(
        !redacted
            .accounts
            .iter()
            .any(|a| { a.label.contains("Alice Smith") || a.label.contains("Personal Workspace") })
    );
    assert_eq!(
        full.accounts
            .iter()
            .find(|a| a.id == token_wire_id)
            .unwrap()
            .label,
        "Alice Smith"
    );
    assert_eq!(
        full.accounts
            .iter()
            .find(|a| a.id == managed_wire_id)
            .unwrap()
            .label,
        "Personal Workspace"
    );
}

// upstream: CLIServeAccountsTests.swift:205 single account response returns matching account and unknown id returns 404
#[test]
fn single_account_response_returns_matching_account_and_unknown_id_returns_404() {
    let account_id = Uuid::new_v4();
    let config = parse_config(serde_json::json!({
        "version": 1,
        "providers": [{
            "id": "claude",
            "tokenAccounts": {
                "version": 1,
                "activeIndex": 0,
                "accounts": [{
                    "id": account_id.hyphenated().to_string().to_ascii_uppercase(),
                    "label": "Claude",
                    "token": "secret",
                    "addedAt": 1.0
                }]
            }
        }]
    }));
    let payload = CliServeAccountDiscovery::make_payload(&config, &[], DashboardIdentityMode::Full);
    let wire_id = format!("token-account:claude:{}", account_id.hyphenated());

    let found = CliServeAccountDiscovery::response(&payload, Some(&wire_id));
    assert_eq!(found.status_code, 200);
    let found_obj: serde_json::Value = serde_json::from_slice(&found.body).unwrap();
    assert_eq!(found_obj["id"].as_str(), Some(wire_id.as_str()));

    let missing = CliServeAccountDiscovery::response(&payload, Some("unknown"));
    assert_eq!(missing.status_code, 404);
    let missing_obj: serde_json::Value = serde_json::from_slice(&missing.body).unwrap();
    assert_eq!(missing_obj["error"].as_str(), Some("account not found"));
}

// ============================================================================
// 2. CLIServeAuthTests.swift (8 tests)
// ============================================================================

// partial-upstream: CLIServeAuthTests.swift:10 serve help documents dashboard token transport honestly
#[test]
fn serve_help_documents_dashboard_token_transport_honestly() {
    let serve = serve_help("0.0.0");
    let root = root_help("0.0.0");

    assert!(serve.contains("--host <host>"));
    assert!(!serve.contains("--dashboard-token <token>"));
    assert!(serve.contains("ariadusage secret set --id serve.dashboardToken"));
    assert!(serve.contains("--allow-plain-http"));
    assert!(serve.contains("GET /dashboard/v1/snapshot"));
    assert!(serve.contains("ARIADUSAGE_DASHBOARD_TOKEN"));
    assert!(serve.contains("cleartext"));
    assert!(!serve.contains("never traverses the network"));
    assert!(!root.contains("--dashboard-token <token>"));
    assert!(root.contains("--allow-plain-http"));
}

// upstream: CLIServeAuthTests.swift:26 serve host option parses and normalizes
#[test]
fn serve_host_option_parses_and_normalizes() {
    assert_eq!(decode_serve_host(None).as_deref(), Some("127.0.0.1"));
    assert_eq!(
        decode_serve_host(Some("0.0.0.0")).as_deref(),
        Some("0.0.0.0")
    );
    assert_eq!(decode_serve_host(Some("  ")), None);
    assert_eq!(decode_serve_host(Some("::1")), None);
    assert_eq!(decode_serve_host(Some("dashboard.local")), None);
    assert_eq!(decode_serve_host(Some("256.1.1.1")), None);

    assert_eq!(CliServeSecurity::bind_host(Some("localhost")), "127.0.0.1");
    assert_eq!(
        CliServeSecurity::bind_host(Some(" LOCALHOST ")),
        "127.0.0.1"
    );
    assert_eq!(CliServeSecurity::bind_host(Some("0.0.0.0")), "0.0.0.0");
    assert_eq!(
        CliServeSecurity::bind_host(Some("192.168.1.10")),
        "192.168.1.10"
    );
    assert!(CliServeSecurity::is_supported_ipv4_bind_host("127.0.0.1"));
    assert!(CliServeSecurity::is_supported_ipv4_bind_host("0.0.0.0"));
    assert!(!CliServeSecurity::is_supported_ipv4_bind_host("::1"));
    assert!(!CliServeSecurity::is_supported_ipv4_bind_host("01.2.3.4"));

    assert!(CliServeSecurity::is_loopback_host("127.0.0.1"));
    assert!(CliServeSecurity::is_loopback_host("127.1.2.3"));
    assert!(CliServeSecurity::is_loopback_host("localhost"));
    assert!(CliServeSecurity::is_loopback_host("::1"));
    assert!(!CliServeSecurity::is_loopback_host("0.0.0.0"));
    assert!(!CliServeSecurity::is_loopback_host("192.168.1.10"));

    assert_eq!(
        CliServeSecurity::allowed_hosts("127.0.0.1"),
        CliLocalHttpAllowedHosts::LoopbackOnly
    );
    assert_eq!(
        CliServeSecurity::allowed_hosts("127.0.0.2"),
        CliLocalHttpAllowedHosts::loopback_and(["127.0.0.2"])
    );
    assert_eq!(
        CliServeSecurity::allowed_hosts("0.0.0.0"),
        CliLocalHttpAllowedHosts::Any
    );
    assert_eq!(
        CliServeSecurity::allowed_hosts("192.168.1.10"),
        CliLocalHttpAllowedHosts::loopback_and(["192.168.1.10"])
    );
}

// partial-upstream: CLIServeAuthTests.swift:76 serve flags parse through the real commander signature
#[test]
fn serve_flags_parse_through_the_real_commander_signature() {
    assert_eq!(
        decode_serve_host(Some("192.168.1.42")).as_deref(),
        Some("192.168.1.42")
    );
    assert_eq!(decode_serve_port(Some("9090")), Some(9090));
    assert_eq!(
        decode_dashboard_identity_mode(Some("full")),
        Some(DashboardIdentityMode::Full)
    );
    let empty_env = BTreeMap::new();
    let secret = SecretString::new("store-secret".to_string());
    assert_eq!(
        resolve_dashboard_token(Some(&secret), &empty_env),
        CliServeDashboardTokenResolution::Token(SecretString::new("store-secret".to_string()))
    );
    assert!(
        CliServeSecurity::validate_startup(
            "192.168.1.42",
            &CliServeDashboardAuth::new(Some(&secret)),
            true
        )
        .is_ok()
    );
    assert_eq!(
        CliServeArgumentError::UnsupportedDashboardTokenFlag.to_string(),
        "--dashboard-token is not supported because secrets must not be passed on argv; set serve.dashboardToken with `ariadusage secret set --id serve.dashboardToken` or set ARIADUSAGE_DASHBOARD_TOKEN."
    );
}

// partial-upstream: CLIServeAuthTests.swift:97 dashboard token resolution prefers the environment and rejects blanks
#[test]
fn dashboard_token_resolution_prefers_the_environment_and_rejects_blanks() {
    let mut env = BTreeMap::new();
    let store_secret = SecretString::new("  store-secret ".to_string());
    assert_eq!(
        resolve_dashboard_token(None, &env),
        CliServeDashboardTokenResolution::Absent
    );
    assert_eq!(
        resolve_dashboard_token(Some(&store_secret), &env),
        CliServeDashboardTokenResolution::Token(SecretString::new("store-secret".to_string()))
    );
    env.insert(
        "ARIADUSAGE_DASHBOARD_TOKEN".to_string(),
        " env-secret ".to_string(),
    );
    assert_eq!(
        resolve_dashboard_token(Some(&store_secret), &env),
        CliServeDashboardTokenResolution::Token(SecretString::new("store-secret".to_string()))
    );
    assert_eq!(
        resolve_dashboard_token(None, &env),
        CliServeDashboardTokenResolution::Token(SecretString::new("env-secret".to_string()))
    );
    let mut blank_env = BTreeMap::new();
    blank_env.insert("ARIADUSAGE_DASHBOARD_TOKEN".to_string(), "   ".to_string());
    assert_eq!(
        resolve_dashboard_token(None, &blank_env),
        CliServeDashboardTokenResolution::Empty {
            source: "ARIADUSAGE_DASHBOARD_TOKEN".to_string()
        }
    );
    let empty_env = BTreeMap::new();
    let blank_store = SecretString::new("   ".to_string());
    assert_eq!(
        resolve_dashboard_token(Some(&blank_store), &empty_env),
        CliServeDashboardTokenResolution::Empty {
            source: "serve.dashboardToken".to_string()
        }
    );
}

// upstream: CLIServeAuthTests.swift:126 serve startup validation enforces the token and plain-http matrix
#[test]
fn serve_startup_validation_enforces_the_token_and_plain_http_matrix() {
    let unauthenticated = CliServeDashboardAuth::new(None);
    let secret = SecretString::new("secret-token".to_string());
    let authenticated = CliServeDashboardAuth::new(Some(&secret));

    assert_eq!(
        CliServeSecurity::validate_startup("127.0.0.1", &unauthenticated, false),
        Ok(())
    );
    assert_eq!(
        CliServeSecurity::validate_startup("localhost", &unauthenticated, false),
        Ok(())
    );
    assert_eq!(
        CliServeSecurity::validate_startup("127.0.0.1", &authenticated, false),
        Ok(())
    );
    let missing_err = CliServeStartupError::MissingDashboardToken {
        host: "192.168.1.42".to_string(),
    };
    assert!(
        missing_err
            .to_string()
            .contains("ariadusage secret set --id serve.dashboardToken")
    );
    assert!(
        missing_err
            .to_string()
            .contains("ARIADUSAGE_DASHBOARD_TOKEN")
    );
    assert_eq!(
        CliServeSecurity::validate_startup("192.168.1.42", &unauthenticated, false),
        Err(missing_err.clone())
    );
    assert_eq!(
        CliServeSecurity::validate_startup("192.168.1.42", &unauthenticated, true),
        Err(missing_err)
    );
    assert_eq!(
        CliServeSecurity::validate_startup("192.168.1.42", &authenticated, false),
        Err(CliServeStartupError::PlainHttpNotAccepted {
            host: "192.168.1.42".to_string()
        })
    );
    assert_eq!(
        CliServeSecurity::validate_startup("192.168.1.42", &authenticated, true),
        Ok(())
    );
}

// upstream: CLIServeAuthTests.swift:159 dashboard auth compares constant-time digests and fails closed
#[test]
fn dashboard_auth_compares_constant_time_digests_and_fails_closed() {
    let unconfigured = CliServeDashboardAuth::new(None);
    assert!(!unconfigured.is_configured());
    assert!(!unconfigured.authorizes(Some("Bearer anything")));
    assert!(!unconfigured.authorizes(None));

    let blank_secret = SecretString::new("   ".to_string());
    let blank = CliServeDashboardAuth::new(Some(&blank_secret));
    assert!(!blank.is_configured());
    assert!(!blank.authorizes(Some("Bearer ")));

    let top_secret = SecretString::new("top-secret-token".to_string());
    let auth = CliServeDashboardAuth::new(Some(&top_secret));
    assert!(auth.is_configured());
    assert!(auth.authorizes(Some("Bearer top-secret-token")));
    assert!(auth.authorizes(Some("bearer   top-secret-token  ")));
    assert!(!auth.authorizes(Some("Bearer top-secret-token-2")));
    assert!(!auth.authorizes(Some("Bearer short")));
    assert!(!auth.authorizes(Some("Basic top-secret-token")));
    assert!(!auth.authorizes(Some("Bearer")));
    assert!(!auth.authorizes(Some("Bearer    ")));
    assert!(!auth.authorizes(None));

    assert!(CliServeDashboardAuth::constant_time_equals(
        b"same", b"same"
    ));
    assert!(!CliServeDashboardAuth::constant_time_equals(
        b"same", b"diff"
    ));
    assert!(!CliServeDashboardAuth::constant_time_equals(
        b"short", b"longer"
    ));
}

// upstream: CLIServeAuthTests.swift:193 adding no-store preserves an existing cache-control header
#[test]
fn adding_no_store_preserves_an_existing_cache_control_header() {
    let plain = CliLocalHttpResponse::json(200, "OK", b"[]".to_vec());
    let mut declared_headers = BTreeMap::new();
    declared_headers.insert("Cache-Control".to_string(), "no-store".to_string());
    let declared =
        CliLocalHttpResponse::json(200, "OK", b"[]".to_vec()).with_headers(declared_headers);

    let annotated = adding_no_store(plain);
    let untouched = adding_no_store(declared);

    assert_eq!(
        annotated.headers.get("Cache-Control").map(String::as_str),
        Some("no-store")
    );
    assert_eq!(untouched.headers.len(), 1);
    assert_eq!(adding_no_store(annotated).headers.len(), 1);
}

// Rust-added: adding_no_store preserves a pre-existing non-no-store Cache-Control value
#[test]
fn adding_no_store_preserves_custom_cache_control_value() {
    let mut existing = BTreeMap::new();
    existing.insert(
        "Cache-Control".to_string(),
        "private, max-age=0".to_string(),
    );
    let response = CliLocalHttpResponse::json(200, "OK", Vec::new()).with_headers(existing);
    let updated = adding_no_store(response);
    assert_eq!(
        updated.headers.get("Cache-Control").map(String::as_str),
        Some("private, max-age=0")
    );
}

// upstream: CLIServeAuthTests.swift:209 unauthorized response advertises bearer challenge and no-store
#[test]
fn unauthorized_response_advertises_bearer_challenge_and_no_store() {
    let response = serve_unauthorized_response();
    assert_eq!(response.status_code, 401);
    assert_eq!(
        response.headers.get("WWW-Authenticate").map(String::as_str),
        Some("Bearer")
    );
    assert_eq!(
        response.headers.get("Cache-Control").map(String::as_str),
        Some("no-store")
    );
    let raw = String::from_utf8(response.serialized()).unwrap();
    assert!(raw.contains("HTTP/1.1 401 Unauthorized\r\n"));
    assert!(raw.contains("WWW-Authenticate: Bearer\r\n"));
    assert!(raw.contains("Cache-Control: no-store\r\n"));
    assert!(raw.contains("{\"error\":\"unauthorized\"}"));
}

// ============================================================================
// 3. CLIServeDashboardIdentityTests.swift (7 tests)
// ============================================================================

// upstream: CLIServeDashboardIdentityTests.swift:10 dashboard identity follows the app privacy setting without a flag
#[test]
fn dashboard_identity_follows_the_app_privacy_setting_without_a_flag() {
    assert_eq!(
        resolve_dashboard_identity_mode(None, true, false),
        DashboardIdentityMode::Redacted
    );
    assert_eq!(
        resolve_dashboard_identity_mode(None, false, false),
        DashboardIdentityMode::Full
    );
}

// upstream: CLIServeDashboardIdentityTests.swift:20 dashboard identity flag overrides the app privacy setting
#[test]
fn dashboard_identity_flag_overrides_the_app_privacy_setting() {
    assert_eq!(
        resolve_dashboard_identity_mode(Some(DashboardIdentityMode::Full), true, false),
        DashboardIdentityMode::Full
    );
    assert_eq!(
        resolve_dashboard_identity_mode(Some(DashboardIdentityMode::Redacted), false, false),
        DashboardIdentityMode::Redacted
    );
}

// upstream: CLIServeDashboardIdentityTests.swift:30 dashboard identity flag presence separates an explicit full from an absent flag
#[test]
fn dashboard_identity_flag_presence_separates_an_explicit_full_from_an_absent_flag() {
    assert!(dashboard_identity_flag_present(Some("full")));
    assert!(!dashboard_identity_flag_present(None));
}

// upstream: CLIServeDashboardIdentityTests.swift:42 an absent identity flag still decodes to the full default
#[test]
fn an_absent_identity_flag_still_decodes_to_the_full_default() {
    assert_eq!(
        decode_dashboard_identity_mode(None),
        Some(DashboardIdentityMode::Full)
    );
}

// upstream: CLIServeDashboardIdentityTests.swift:50 dashboard operation key separates identity modes
#[test]
fn dashboard_operation_key_separates_identity_modes() {
    let redacted =
        serve_dashboard_operation_key(DashboardIdentityMode::Redacted, false, None, false).unwrap();
    let full =
        serve_dashboard_operation_key(DashboardIdentityMode::Full, false, None, false).unwrap();
    assert_ne!(redacted, full);
}

// upstream: CLIServeDashboardIdentityTests.swift:64 dashboard operation key separates usage bars fill preferences
#[test]
fn dashboard_operation_key_separates_usage_bars_fill_preferences() {
    let used =
        serve_dashboard_operation_key(DashboardIdentityMode::Full, true, None, false).unwrap();
    let remaining =
        serve_dashboard_operation_key(DashboardIdentityMode::Full, false, None, false).unwrap();
    assert_ne!(used, remaining);
}

// upstream: CLIServeDashboardIdentityTests.swift:78 warm dashboard responses retain the requested fill mode through toggles
#[tokio::test]
async fn warm_dashboard_responses_retain_the_requested_fill_mode_through_toggles() {
    let (_clock, cache) = make_serve_test_cache();
    let counter = Arc::new(AtomicUsize::new(0));
    for show_used in [false, true, false, true] {
        let key = serve_dashboard_operation_key(
            DashboardIdentityMode::Redacted,
            show_used,
            Some("codex"),
            false,
        )
        .unwrap();
        let c = Arc::clone(&counter);
        let response = cached_serve_response(
            &key,
            &cache,
            60.0,
            30.0,
            "synthetic-config",
            false,
            move || {
                let c = Arc::clone(&c);
                async move {
                    let build = c.fetch_add(1, Ordering::SeqCst) + 1;
                    CliLocalHttpResponse::json(
                        200,
                        "OK",
                        format!(r#"{{"usageBarsShowUsed":{show_used},"build":{build}}}"#)
                            .into_bytes(),
                    )
                }
            },
        )
        .await;
        let payload: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(payload["usageBarsShowUsed"].as_bool(), Some(show_used));
    }
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

// ============================================================================
// 4. CLIServeRawHTTPTests.swift (25 tests)
// ============================================================================

#[allow(clippy::type_complexity)]
async fn with_running_test_server<F, Fut>(
    allowed_hosts: CliLocalHttpAllowedHosts,
    handler: Arc<
        dyn Fn(
                CliLocalHttpRequest,
            )
                -> std::pin::Pin<Box<dyn std::future::Future<Output = CliLocalHttpResponse> + Send>>
            + Send
            + Sync,
    >,
    body: F,
) where
    F: FnOnce(u16) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let server = CliLocalHttpServer::bind("127.0.0.1", 0, 5_000, 10_000, 16)
        .await
        .unwrap()
        .with_allowed_hosts(allowed_hosts);
    let port = server.port;
    let (tx, rx) = watch::channel(false);
    let task = tokio::spawn(async move {
        server.serve(rx, handler).await;
    });
    body(port).await;
    let _ = tx.send(true);
    let _ = task.await;
}

async fn send_raw_request(port: u16, raw: &str) -> String {
    let bytes = send_raw_http(port, raw.as_bytes()).await;
    String::from_utf8(bytes).unwrap()
}

// upstream: CLIServeRawHTTPTests.swift:20 raw server serializes status body and extra headers on the wire
#[tokio::test]
async fn raw_server_serializes_status_body_and_extra_headers_on_the_wire() {
    with_running_test_server(
        CliLocalHttpAllowedHosts::LoopbackOnly,
        Arc::new(|_req| {
            Box::pin(async {
                let mut headers = BTreeMap::new();
                headers.insert("Cache-Control".to_string(), "no-store".to_string());
                headers.insert("WWW-Authenticate".to_string(), "Bearer".to_string());
                CliLocalHttpResponse::json(
                    401,
                    "Unauthorized",
                    br#"{"error":"unauthorized"}"#.to_vec(),
                )
                .with_headers(headers)
            })
        }),
        |port| async move {
            let raw = send_raw_request(
                port,
                &format!("GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
            )
            .await;
            assert!(raw.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
            assert!(raw.contains("Content-Type: application/json; charset=utf-8\r\n"));
            assert!(raw.contains("Cache-Control: no-store\r\n"));
            assert!(raw.contains("WWW-Authenticate: Bearer\r\n"));
            assert!(raw.ends_with("{\"error\":\"unauthorized\"}"));
        },
    )
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:41 raw server rejects non loopback host headers by default
#[tokio::test]
async fn raw_server_rejects_non_loopback_host_headers_by_default() {
    with_running_test_server(
        CliLocalHttpAllowedHosts::LoopbackOnly,
        Arc::new(|_req| Box::pin(async { CliLocalHttpResponse::json(200, "OK", b"{}".to_vec()) })),
        |port| async move {
            let raw = send_raw_request(
                port,
                &format!("GET /health HTTP/1.1\r\nHost: 192.168.1.42:{port}\r\n\r\n"),
            )
            .await;
            assert!(raw.starts_with("HTTP/1.1 403 Forbidden\r\n"));
            assert!(raw.contains("Cache-Control: no-store\r\n"));
            assert!(raw.ends_with("{\"error\":\"forbidden host\"}"));
        },
    )
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:53 raw server accepts a configured non loopback host alongside loopback
#[tokio::test]
async fn raw_server_accepts_a_configured_non_loopback_host_alongside_loopback() {
    with_running_test_server(
        CliServeSecurity::allowed_hosts("192.168.1.42"),
        Arc::new(|_req| {
            Box::pin(async {
                CliLocalHttpResponse::json(200, "OK", br#"{"status":"ok"}"#.to_vec())
            })
        }),
        |port| async move {
            let lan_raw = send_raw_request(
                port,
                &format!("GET /health HTTP/1.1\r\nHost: 192.168.1.42:{port}\r\n\r\n"),
            )
            .await;
            assert!(lan_raw.starts_with("HTTP/1.1 200 OK\r\n"));
            assert!(lan_raw.ends_with("{\"status\":\"ok\"}"));

            let loopback_raw = send_raw_request(
                port,
                &format!("GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
            )
            .await;
            assert!(loopback_raw.starts_with("HTTP/1.1 200 OK\r\n"));
            assert!(loopback_raw.ends_with("{\"status\":\"ok\"}"));
        },
    )
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:75 raw server rejects duplicate host headers
#[tokio::test]
async fn raw_server_rejects_duplicate_host_headers() {
    with_running_test_server(
        CliLocalHttpAllowedHosts::LoopbackOnly,
        Arc::new(|_req| Box::pin(async { CliLocalHttpResponse::json(200, "OK", b"{}".to_vec()) })),
        |port| async move {
            let raw = send_raw_request(
                port,
                &format!(
                    "GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nHost: evil.example\r\n\r\n"
                ),
            )
            .await;
            assert!(raw.starts_with("HTTP/1.1 400 Bad Request\r\n"));
            assert!(raw.contains("Cache-Control: no-store\r\n"));
            assert!(raw.ends_with("{\"error\":\"invalid request\"}"));
        },
    )
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:87 raw server rejects duplicate authorization headers
#[tokio::test]
async fn raw_server_rejects_duplicate_authorization_headers() {
    let invoked = Arc::new(AtomicBool::new(false));
    let inv = Arc::clone(&invoked);
    with_running_test_server(
        CliLocalHttpAllowedHosts::LoopbackOnly,
        Arc::new(move |_req| {
            inv.store(true, Ordering::SeqCst);
            Box::pin(async { CliLocalHttpResponse::json(200, "OK", b"{}".to_vec()) })
        }),
        |port| async move {
            let raw = send_raw_request(
                port,
                &format!(
                    "GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer first\r\nAuthorization: Bearer second\r\n\r\n"
                ),
            )
            .await;
            assert!(raw.starts_with("HTTP/1.1 400 Bad Request\r\n"));
            assert!(raw.contains("Cache-Control: no-store\r\n"));
            assert!(raw.ends_with("{\"error\":\"invalid request\"}"));
        },
    )
    .await;
    assert!(!invoked.load(Ordering::SeqCst));
}

async fn with_running_runtime_server<F, Fut>(rt: ServeRuntime, body: F)
where
    F: FnOnce(u16) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let allowed_hosts = if rt.data_routes_require_auth {
        CliLocalHttpAllowedHosts::Any
    } else {
        CliLocalHttpAllowedHosts::LoopbackOnly
    };
    let rt = Arc::new(rt);
    with_running_test_server(
        allowed_hosts,
        Arc::new(move |req| {
            let rt = Arc::clone(&rt);
            Box::pin(async move { handle_serve_request(req, &rt).await })
        }),
        body,
    )
    .await;
}

fn split_http_response(raw: &str) -> (&str, &str) {
    raw.split_once("\r\n\r\n")
        .expect("valid HTTP response with header/body separator")
}

// upstream: CLIServeRawHTTPTests.swift:107 raw server passes the authorization header to the handler
#[tokio::test]
async fn raw_server_passes_the_authorization_header_to_the_handler() {
    let secret = SecretString::new("socket-secret".to_string());
    let auth = CliServeDashboardAuth::new(Some(&secret));
    with_running_test_server(
        CliLocalHttpAllowedHosts::LoopbackOnly,
        Arc::new(move |req| {
            let auth = auth.clone();
            Box::pin(async move {
                if auth.authorizes(req.authorization()) {
                    adding_no_store(CliLocalHttpResponse::json(
                        200,
                        "OK",
                        br#"{"authorized":true}"#.to_vec(),
                    ))
                } else {
                    serve_unauthorized_response()
                }
            })
        }),
        |port| async move {
            let raw = send_raw_request(
                port,
                &format!(
                    "GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer socket-secret\r\n\r\n"
                ),
            )
            .await;
            assert!(raw.starts_with("HTTP/1.1 200 OK\r\n"));
            assert!(raw.contains("Cache-Control: no-store\r\n"));
            assert!(raw.ends_with("{\"authorized\":true}"));
        },
    )
    .await;
}

// partial-upstream: CLIServeRawHTTPTests.swift:129 web UI returns HTML with no-store
#[tokio::test]
async fn web_ui_returns_html_with_no_store() {
    let cfg = default_config();
    let (_dir, rt) = make_test_runtime(&cfg, None, "127.0.0.1", None, false);
    with_running_runtime_server(rt, |port| async move {
        let raw = send_raw_request(
            port,
            &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
        .await;
        let (headers, body) = split_http_response(&raw);
        assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(headers.contains("Content-Type: text/html; charset=utf-8"));
        assert!(headers.contains("Cache-Control: no-store"));
        assert!(headers.contains("Content-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self'; connect-src 'self'"));
        assert!(body.contains("AriadUsage Dashboard"));
        assert!(body.contains("/dashboard/v1/snapshot"));
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:144 web UI stays open when a dashboard token is configured
#[tokio::test]
async fn web_ui_stays_open_when_a_dashboard_token_is_configured() {
    let cfg = default_config();
    let (_dir, rt) = make_test_runtime(&cfg, Some("secret-token"), "127.0.0.1", None, false);
    with_running_runtime_server(rt, |port| async move {
        let raw = send_raw_request(
            port,
            &format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
        .await;
        assert!(raw.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(raw.contains("Content-Type: text/html; charset=utf-8\r\n"));
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:156 snapshot without credentials returns 401 with challenge and no-store
#[tokio::test]
async fn snapshot_without_credentials_returns_401_with_challenge_and_no_store() {
    let cfg = default_config();
    let (_dir, mut rt) = make_test_runtime(&cfg, Some("secret-token"), "127.0.0.1", None, false);
    let called = Arc::new(AtomicBool::new(false));
    let c = Arc::clone(&called);
    rt.usage_fetcher = Arc::new(move |prov, _cfg, _sink| {
        c.store(true, Ordering::SeqCst);
        Box::pin(async move {
            UsageCommandOutput {
                sections: Vec::new(),
                payload: vec![ProviderPayload::new(prov, "cli")],
                exit_code: 0,
            }
        })
    });
    with_running_runtime_server(rt, |port| async move {
        let raw = send_raw_request(
            port,
            &format!("GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
        .await;
        assert!(raw.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
        assert!(raw.contains("WWW-Authenticate: Bearer\r\n"));
        assert!(raw.contains("Cache-Control: no-store\r\n"));
        assert!(raw.ends_with("{\"error\":\"unauthorized\"}"));
    })
    .await;
    assert!(!called.load(Ordering::SeqCst));
}

// upstream: CLIServeRawHTTPTests.swift:170 snapshot with wrong token returns 401
#[tokio::test]
async fn snapshot_with_wrong_token_returns_401() {
    let cfg = default_config();
    let (_dir, mut rt) = make_test_runtime(&cfg, Some("secret-token"), "127.0.0.1", None, false);
    let called = Arc::new(AtomicBool::new(false));
    let c = Arc::clone(&called);
    rt.usage_fetcher = Arc::new(move |prov, _cfg, _sink| {
        c.store(true, Ordering::SeqCst);
        Box::pin(async move {
            UsageCommandOutput {
                sections: Vec::new(),
                payload: vec![ProviderPayload::new(prov, "cli")],
                exit_code: 0,
            }
        })
    });
    with_running_runtime_server(rt, |port| async move {
        let raw = send_raw_request(
            port,
            &format!(
                "GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer wrong-token\r\n\r\n"
            ),
        )
        .await;
        assert!(raw.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
        assert!(raw.contains("WWW-Authenticate: Bearer\r\n"));
        assert!(raw.contains("Cache-Control: no-store\r\n"));
    })
    .await;
    assert!(!called.load(Ordering::SeqCst));
}

// upstream: CLIServeRawHTTPTests.swift:184 snapshot with correct token returns decodable JSON with no-store
#[tokio::test]
async fn snapshot_with_correct_token_returns_decodable_json_with_no_store() {
    let cfg = parse_config(serde_json::json!({
        "version": 1,
        "providers": [{ "id": "codex", "enabled": true }]
    }));
    let (_dir, mut rt) = make_test_runtime(&cfg, Some("secret-token"), "127.0.0.1", None, false);
    rt.usage_fetcher = Arc::new(|prov, _cfg, _sink| {
        Box::pin(async move {
            let mut p = ProviderPayload::new(prov, "cli");
            p.usage = Some(make_usage_with_email(
                ProviderId::new("codex").unwrap(),
                "person@example.com",
            ));
            UsageCommandOutput {
                sections: Vec::new(),
                payload: vec![p],
                exit_code: 0,
            }
        })
    });
    with_running_runtime_server(rt, |port| async move {
        let raw = send_raw_request(
            port,
            &format!(
                "GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer secret-token\r\n\r\n"
            ),
        )
        .await;
        let (headers, body) = split_http_response(&raw);
        assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(headers.contains("Cache-Control: no-store"));
        let obj: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(obj["schemaVersion"], 1);
        assert_eq!(obj["providers"][0]["id"], "codex");
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:203 snapshot shell returns minimal provider rows without invoking fetches
#[tokio::test]
async fn snapshot_shell_returns_minimal_provider_rows_without_invoking_fetches() {
    let cfg = parse_config(serde_json::json!({
        "version": 1,
        "providers": [
            { "id": "codex", "enabled": true },
            { "id": "claude", "enabled": true }
        ]
    }));
    let (_dir, mut rt) = make_test_runtime(&cfg, Some("secret-token"), "127.0.0.1", None, false);
    let called = Arc::new(AtomicBool::new(false));
    let c = Arc::clone(&called);
    rt.usage_fetcher = Arc::new(move |prov, _cfg, _sink| {
        c.store(true, Ordering::SeqCst);
        Box::pin(async move {
            UsageCommandOutput {
                sections: Vec::new(),
                payload: vec![ProviderPayload::new(prov, "cli")],
                exit_code: 0,
            }
        })
    });
    with_running_runtime_server(rt, |port| async move {
        let raw = send_raw_request(
            port,
            &format!(
                "GET /dashboard/v1/snapshot?detail=shell HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer secret-token\r\n\r\n"
            ),
        )
        .await;
        let (headers, body) = split_http_response(&raw);
        assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(headers.contains("Cache-Control: no-store"));
        let obj: serde_json::Value = serde_json::from_str(body).unwrap();
        let providers = obj["providers"].as_array().unwrap();
        assert_eq!(providers.len(), 2);
        let first = providers[0].as_object().unwrap();
        let mut keys: Vec<&str> = first.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["display", "enabled", "id", "name"]);
    })
    .await;
    assert!(!called.load(Ordering::SeqCst));
}

// upstream: CLIServeRawHTTPTests.swift:237 snapshot shell supports provider filter and rejects invalid query values
#[tokio::test]
async fn snapshot_shell_supports_provider_filter_and_rejects_invalid_query_values() {
    let cfg = parse_config(serde_json::json!({
        "version": 1,
        "providers": [
            { "id": "codex", "enabled": true },
            { "id": "claude", "enabled": true }
        ]
    }));
    let (_dir, rt) = make_test_runtime(&cfg, Some("secret-token"), "127.0.0.1", None, false);
    with_running_runtime_server(rt, |port| async move {
        let filtered_raw = send_raw_request(
            port,
            &format!(
                "GET /dashboard/v1/snapshot?detail=shell&provider=claude HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer secret-token\r\n\r\n"
            ),
        )
        .await;
        let (filtered_headers, filtered_body) = split_http_response(&filtered_raw);
        assert!(filtered_headers.starts_with("HTTP/1.1 200 OK\r\n"));
        let obj: serde_json::Value = serde_json::from_str(filtered_body).unwrap();
        let providers = obj["providers"].as_array().unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0]["id"], "claude");

        let invalid_provider_raw = send_raw_request(
            port,
            &format!(
                "GET /dashboard/v1/snapshot?provider=missing HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer secret-token\r\n\r\n"
            ),
        )
        .await;
        let (inv_prov_headers, inv_prov_body) = split_http_response(&invalid_provider_raw);
        assert!(inv_prov_headers.starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert_eq!(inv_prov_body, r#"{"error":"Unknown provider 'missing'."}"#);

        let invalid_detail_raw = send_raw_request(
            port,
            &format!(
                "GET /dashboard/v1/snapshot?detail=summary HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer secret-token\r\n\r\n"
            ),
        )
        .await;
        let (inv_det_headers, inv_det_body) = split_http_response(&invalid_detail_raw);
        assert!(inv_det_headers.starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert_eq!(
            inv_det_body,
            r#"{"error":"Unknown dashboard detail 'summary'."}"#
        );
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:269 snapshot never accepts the token from the query string
#[tokio::test]
async fn snapshot_never_accepts_the_token_from_the_query_string() {
    let cfg = default_config();
    let (_dir, rt) = make_test_runtime(&cfg, Some("secret-token"), "127.0.0.1", None, false);
    with_running_runtime_server(rt, |port| async move {
        let raw = send_raw_request(
            port,
            &format!(
                "GET /dashboard/v1/snapshot?token=secret-token HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
            ),
        )
        .await;
        assert!(raw.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
        assert!(raw.contains("Cache-Control: no-store\r\n"));
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:281 snapshot with duplicate authorization headers returns 400
#[tokio::test]
async fn snapshot_with_duplicate_authorization_headers_returns_400() {
    let cfg = default_config();
    let (_dir, rt) = make_test_runtime(&cfg, Some("secret-token"), "127.0.0.1", None, false);
    with_running_runtime_server(rt, |port| async move {
        let raw = send_raw_request(
            port,
            &format!(
                "GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer secret-token\r\nAuthorization: Bearer secret-token\r\n\r\n"
            ),
        )
        .await;
        assert!(raw.starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert!(raw.contains("Cache-Control: no-store\r\n"));
        assert!(raw.ends_with("{\"error\":\"invalid request\"}"));
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:300 snapshot rejects non get methods with 405
#[tokio::test]
async fn snapshot_rejects_non_get_methods_with_405() {
    let cfg = default_config();
    let (_dir, rt) = make_test_runtime(&cfg, Some("secret-token"), "127.0.0.1", None, false);
    with_running_runtime_server(rt, |port| async move {
        let raw = send_raw_request(
            port,
            &format!("POST /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
        .await;
        assert!(raw.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
        assert!(raw.contains("Cache-Control: no-store\r\n"));
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:313 dashboard missing routes return no-store
#[tokio::test]
async fn dashboard_missing_routes_return_no_store() {
    let cfg = default_config();
    let (_dir, rt) = make_test_runtime(&cfg, Some("secret-token"), "127.0.0.1", None, false);
    with_running_runtime_server(rt, |port| async move {
        let raw = send_raw_request(
            port,
            &format!("GET /dashboard/v1/unknown HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
        .await;
        assert!(raw.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(raw.contains("Cache-Control: no-store\r\n"));
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:325 snapshot fails closed when no token is configured
#[tokio::test]
async fn snapshot_fails_closed_when_no_token_is_configured() {
    let cfg = default_config();
    let (_dir, rt) = make_test_runtime(&cfg, None, "127.0.0.1", None, false);
    with_running_runtime_server(rt, |port| async move {
        let raw = send_raw_request(
            port,
            &format!(
                "GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer anything\r\n\r\n"
            ),
        )
        .await;
        assert!(raw.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
        assert!(raw.contains("WWW-Authenticate: Bearer\r\n"));
        assert!(raw.contains("Cache-Control: no-store\r\n"));
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:338 usage and cost responses carry no-store on the wire
#[tokio::test]
async fn usage_and_cost_responses_carry_no_store_on_the_wire() {
    let cfg = parse_config(serde_json::json!({
        "version": 1,
        "providers": [{ "id": "codex", "enabled": true }]
    }));
    let (_dir, rt) = make_test_runtime(&cfg, None, "127.0.0.1", None, false);
    with_running_runtime_server(rt, |port| async move {
        for path in ["/usage", "/cost"] {
            let raw = send_raw_request(
                port,
                &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
            )
            .await;
            assert!(raw.starts_with("HTTP/1.1 200 OK\r\n"));
            assert!(raw.contains("Cache-Control: no-store\r\n"));
        }
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:358 accounts routes return metadata only with no-store and unknown ids return 404
#[tokio::test]
async fn accounts_routes_return_metadata_only_with_no_store_and_unknown_ids_return_404() {
    let acc_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
    let cfg = parse_config(serde_json::json!({
        "version": 1,
        "providers": [{
            "id": "claude",
            "enabled": true,
            "tokenAccounts": {
                "version": 1,
                "activeIndex": 0,
                "accounts": [{
                    "id": acc_id.hyphenated().to_string().to_ascii_uppercase(),
                    "label": "Work Claude",
                    "token": "sk-ant-secret",
                    "addedAt": 1.0
                }]
            }
        }]
    }));
    let (_dir, rt) = make_test_runtime(
        &cfg,
        None,
        "127.0.0.1",
        Some(DashboardIdentityMode::Full),
        false,
    );

    with_running_runtime_server(rt, |port| async move {
        let list_raw = send_raw_request(
            port,
            &format!("GET /accounts HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
        .await;
        let (list_headers, list_body) = split_http_response(&list_raw);
        assert!(list_headers.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(list_headers.contains("Cache-Control: no-store"));
        assert!(!list_body.contains("sk-ant-secret"));
        let list_payload: CliServeAccountsPayload = serde_json::from_str(list_body).unwrap();
        assert_eq!(list_payload.accounts.len(), 1);
        let wire_id = &list_payload.accounts[0].id;

        let item_raw = send_raw_request(
            port,
            &format!("GET /accounts/{wire_id} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
        .await;
        assert!(item_raw.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(item_raw.contains("Cache-Control: no-store\r\n"));

        let missing_raw = send_raw_request(
            port,
            &format!("GET /accounts/unknown-id HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"),
        )
        .await;
        assert!(missing_raw.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(missing_raw.contains("Cache-Control: no-store\r\n"));
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:414 non-loopback binds gate account usage and cost data behind the token
#[tokio::test]
async fn non_loopback_binds_gate_account_usage_and_cost_data_behind_the_token() {
    let cfg = parse_config(serde_json::json!({
        "version": 1,
        "providers": [{ "id": "codex", "enabled": true }]
    }));
    let (_dir, rt) = make_test_runtime(&cfg, Some("lan-secret"), "192.168.1.42", None, false);

    with_running_runtime_server(rt, |port| async move {
        for path in ["/accounts", "/usage", "/cost"] {
            let unauth_raw = send_raw_request(
                port,
                &format!("GET {path} HTTP/1.1\r\nHost: 192.168.1.42:{port}\r\n\r\n"),
            )
            .await;
            assert!(unauth_raw.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
            assert!(unauth_raw.contains("WWW-Authenticate: Bearer\r\n"));
            assert!(unauth_raw.contains("Cache-Control: no-store\r\n"));

            let auth_raw = send_raw_request(
                port,
                &format!(
                    "GET {path} HTTP/1.1\r\nHost: 192.168.1.42:{port}\r\nAuthorization: Bearer lan-secret\r\n\r\n"
                ),
            )
            .await;
            assert!(auth_raw.starts_with("HTTP/1.1 200 OK\r\n"));
            assert!(auth_raw.contains("Cache-Control: no-store\r\n"));
        }

        let health_raw = send_raw_request(
            port,
            &format!("GET /health HTTP/1.1\r\nHost: 192.168.1.42:{port}\r\n\r\n"),
        )
        .await;
        let (health_headers, health_body) = split_http_response(&health_raw);
        assert!(health_headers.starts_with("HTTP/1.1 200 OK\r\n"));
        let health_obj: serde_json::Value = serde_json::from_str(health_body).unwrap();
        assert_eq!(health_obj["status"], "ok");
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:454 account authentication precedes config reads and errors do not expose storage details
#[tokio::test]
async fn account_authentication_precedes_config_reads_and_errors_do_not_expose_storage_details() {
    let cfg = default_config();
    let (dir, rt) = make_test_runtime(&cfg, Some("lan-secret"), "192.168.1.42", None, false);
    std::fs::write(
        dir.path().join("config.json"),
        b"{broken /Users/private/secret",
    )
    .unwrap();

    with_running_runtime_server(rt, |port| async move {
        let unauth_raw = send_raw_request(
            port,
            &format!("GET /accounts HTTP/1.1\r\nHost: 192.168.1.42:{port}\r\n\r\n"),
        )
        .await;
        assert!(unauth_raw.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
        assert!(unauth_raw.contains("Cache-Control: no-store\r\n"));

        let auth_raw = send_raw_request(
            port,
            &format!(
                "GET /accounts HTTP/1.1\r\nHost: 192.168.1.42:{port}\r\nAuthorization: Bearer lan-secret\r\n\r\n"
            ),
        )
        .await;
        let (auth_headers, auth_body) = split_http_response(&auth_raw);
        assert!(auth_headers.starts_with("HTTP/1.1 500 Internal Server Error\r\n"));
        assert!(auth_headers.contains("Cache-Control: no-store"));
        assert_eq!(auth_body, r#"{"error":"could not load accounts"}"#);
        assert!(!auth_body.contains("/Users/private"));
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:476 dashboard error responses carry no-store
#[tokio::test]
async fn dashboard_error_responses_carry_no_store() {
    let cfg = default_config();
    let (_dir, rt) = make_test_runtime(&cfg, Some("secret-token"), "127.0.0.1", None, false);
    with_running_runtime_server(rt, |port| async move {
        let raw = send_raw_request(
            port,
            &format!(
                "GET /dashboard/v1/snapshot?provider=not-a-provider HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer secret-token\r\n\r\n"
            ),
        )
        .await;
        assert!(raw.starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert!(raw.contains("Cache-Control: no-store\r\n"));
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:490 health stays open when a dashboard token is configured
#[tokio::test]
async fn health_stays_open_when_a_dashboard_token_is_configured() {
    let cfg = default_config();
    let (_dir, rt) = make_test_runtime(&cfg, Some("secret-token"), "192.168.1.42", None, false);
    with_running_runtime_server(rt, |port| async move {
        let raw = send_raw_request(
            port,
            &format!("GET /health HTTP/1.1\r\nHost: 192.168.1.42:{port}\r\n\r\n"),
        )
        .await;
        let (headers, body) = split_http_response(&raw);
        assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
        let obj: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(obj["status"], "ok");
    })
    .await;
}

// upstream: CLIServeRawHTTPTests.swift:504 snapshot response preserves usage cache metadata
#[tokio::test]
async fn snapshot_response_preserves_usage_cache_metadata() {
    let cfg = parse_config(serde_json::json!({
        "version": 1,
        "providers": [{ "id": "codex", "enabled": true }]
    }));
    let (_dir, mut rt) = make_test_runtime(&cfg, Some("secret-token"), "127.0.0.1", None, false);
    rt.usage_fetcher = Arc::new(|prov, _cfg, _sink| {
        Box::pin(async move {
            let mut item = ProviderPayload::new(prov, "cli");
            item.cache_account_key = Some("codex:acct-1".to_string());
            item.usage = Some(make_usage_with_email(
                ProviderId::new("codex").unwrap(),
                "alice@example.com",
            ));
            UsageCommandOutput {
                sections: Vec::new(),
                payload: vec![item],
                exit_code: 0,
            }
        })
    });
    let req = CliLocalHttpRequest::parse(
        b"GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nAuthorization: Bearer secret-token\r\n\r\n",
        &CliLocalHttpAllowedHosts::LoopbackOnly,
    )
    .unwrap();
    let response = handle_serve_request(req, &rt).await;
    assert_eq!(response.status_code, 200);
    assert_eq!(
        response.usage_cache_keys,
        Some(vec![Some("codex:acct-1".to_string())])
    );

    with_running_runtime_server(rt, |port| async move {
        let raw = send_raw_request(
            port,
            &format!(
                "GET /dashboard/v1/snapshot HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer secret-token\r\n\r\n"
            ),
        )
        .await;
        assert!(raw.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(raw.contains("Cache-Control: no-store\r\n"));
    })
    .await;
}

// ============================================================================
// 5. CLIServeRouterTests.swift (48 tests)
// ============================================================================

fn json_rows(response: &CliLocalHttpResponse) -> Vec<serde_json::Value> {
    serde_json::from_slice(&response.body).unwrap()
}

fn find_row<'a>(
    rows: &'a [serde_json::Value],
    provider: &str,
    account: Option<&str>,
) -> Option<&'a serde_json::Value> {
    rows.iter().find(|r| {
        r.get("provider").and_then(serde_json::Value::as_str) == Some(provider)
            && match account {
                Some(acc) => r.get("account").and_then(serde_json::Value::as_str) == Some(acc),
                None => true,
            }
    })
}

fn make_codex_visible_account(
    id: &str,
    email: &str,
    workspace_account_id: Option<&str>,
    auth_fingerprint: Option<&str>,
    stored_account_id: Option<Uuid>,
) -> CodexVisibleAccount {
    CodexVisibleAccount {
        id: id.to_string(),
        email: email.to_string(),
        workspace_label: None,
        workspace_account_id: workspace_account_id.map(ToString::to_string),
        auth_fingerprint: auth_fingerprint.map(ToString::to_string),
        stored_account_id,
        selection_source: CodexActiveSource::LiveSystem,
        is_active: true,
        is_live: true,
        can_reauthenticate: true,
        can_remove: false,
    }
}

// upstream: CLIServeRouterTests.swift:15 local HTTP connection gate caps pre-auth clients
#[test]
fn local_http_connection_gate_caps_pre_auth_clients() {
    let gate = CliLocalHttpConnectionGate::new(2);
    assert!(gate.try_acquire());
    assert!(gate.try_acquire());
    assert!(!gate.try_acquire());
    assert_eq!(gate.active_count(), 2);
    gate.release();
    assert!(gate.try_acquire());
    assert_eq!(gate.active_count(), 2);
    gate.release();
    gate.release();
    assert_eq!(gate.active_count(), 0);
}

// upstream: CLIServeRouterTests.swift:31 usage operation fingerprint separates dashboard account mode
#[test]
fn usage_operation_fingerprint_separates_dashboard_account_mode() {
    let all_accounts = serve_usage_operation_fingerprint("config", true, false);
    let selected_account = serve_usage_operation_fingerprint("config", false, false);
    assert_ne!(all_accounts, selected_account);
    assert_eq!(
        all_accounts,
        serve_usage_operation_fingerprint("config", true, false)
    );
}

// upstream: CLIServeRouterTests.swift:46 termination monitor handles interactive and hangup signals
#[test]
fn termination_monitor_handles_interactive_and_hangup_signals() {
    assert_eq!(CliTerminationSignalMonitor::SIGNAL_NUMBERS, [2, 15, 1]);
}

// upstream: CLIServeRouterTests.swift:51 local http parser accepts only loopback host headers
#[test]
fn local_http_parser_accepts_only_loopback_host_headers() {
    let allowed_hosts = [
        "localhost",
        "localhost.",
        "localhost:8080",
        "127.0.0.1",
        "127.0.0.1:8080",
        "[::1]",
        "[::1]:8080",
    ];
    for host in allowed_hosts {
        let raw = format!("GET /usage?provider=claude HTTP/1.1\r\nHost: {host}\r\n\r\n");
        let req =
            CliLocalHttpRequest::parse(raw.as_bytes(), &CliLocalHttpAllowedHosts::LoopbackOnly)
                .unwrap();
        assert_eq!(req.host(), host);
        assert_eq!(req.path, "/usage");
    }
}

// upstream: CLIServeRouterTests.swift:70 local http parser rejects hostile missing and duplicate hosts
#[test]
fn local_http_parser_rejects_hostile_missing_and_duplicate_hosts() {
    assert_eq!(
        CliLocalHttpRequest::parse(
            b"GET /usage HTTP/1.1\r\n\r\n",
            &CliLocalHttpAllowedHosts::LoopbackOnly
        ),
        Err(CliLocalHttpRequestError::MissingHost)
    );
    assert_eq!(
        CliLocalHttpRequest::parse(
            b"GET /usage HTTP/1.1\r\nHost: evil.test\r\n\r\n",
            &CliLocalHttpAllowedHosts::LoopbackOnly
        ),
        Err(CliLocalHttpRequestError::DisallowedHost)
    );
    assert_eq!(
        CliLocalHttpRequest::parse(
            b"GET /usage HTTP/1.1\r\nHost: localhost, evil.test\r\n\r\n",
            &CliLocalHttpAllowedHosts::LoopbackOnly
        ),
        Err(CliLocalHttpRequestError::DisallowedHost)
    );
    assert_eq!(
        CliLocalHttpRequest::parse(
            b"GET /usage HTTP/1.1\r\nHost: localhost:abc\r\n\r\n",
            &CliLocalHttpAllowedHosts::LoopbackOnly
        ),
        Err(CliLocalHttpRequestError::DisallowedHost)
    );
    assert_eq!(
        CliLocalHttpRequest::parse(
            b"GET /usage HTTP/1.1\r\nHost: localhost\r\nHost: 127.0.0.1\r\n\r\n",
            &CliLocalHttpAllowedHosts::LoopbackOnly
        ),
        Err(CliLocalHttpRequestError::DuplicateHost)
    );
}

// upstream: CLIServeRouterTests.swift:81 local http parser captures a single authorization header
#[test]
fn local_http_parser_captures_a_single_authorization_header() {
    let raw = [
        "GET /usage HTTP/1.1",
        "Host: localhost",
        "authorization: Bearer token",
        "",
        "",
    ]
    .join("\r\n");
    let req = CliLocalHttpRequest::parse(raw.as_bytes(), &CliLocalHttpAllowedHosts::LoopbackOnly)
        .unwrap();
    assert_eq!(req.authorization(), Some("Bearer token"));

    let no_auth = CliLocalHttpRequest::parse(
        b"GET /usage?provider=claude HTTP/1.1\r\nHost: localhost\r\n\r\n",
        &CliLocalHttpAllowedHosts::LoopbackOnly,
    )
    .unwrap();
    assert_eq!(no_auth.authorization(), None);

    assert_eq!(
        CliLocalHttpRequest::parse(
            b"GET /usage HTTP/1.1\r\nHost: localhost\r\nAuthorization: a\r\nAuthorization: b\r\n\r\n",
            &CliLocalHttpAllowedHosts::LoopbackOnly
        ),
        Err(CliLocalHttpRequestError::DuplicateAuthorization)
    );
}

// upstream: CLIServeRouterTests.swift:99 local http parser extends the allowed host set without replacing loopback
#[test]
fn local_http_parser_extends_the_allowed_host_set_without_replacing_loopback() {
    let raw = b"GET /usage HTTP/1.1\r\nHost: 192.168.1.10:8080\r\n\r\n";
    assert_eq!(
        CliLocalHttpRequest::parse(raw, &CliLocalHttpAllowedHosts::LoopbackOnly),
        Err(CliLocalHttpRequestError::DisallowedHost)
    );

    let allowed = CliLocalHttpAllowedHosts::loopback_and(["192.168.1.10"]);
    let req = CliLocalHttpRequest::parse(raw, &allowed).unwrap();
    assert_eq!(req.host(), "192.168.1.10:8080");
    assert_eq!(req.path, "/usage");

    let loopback =
        CliLocalHttpRequest::parse(b"GET /usage HTTP/1.1\r\nHost: localhost\r\n\r\n", &allowed)
            .unwrap();
    assert_eq!(loopback.host(), "localhost");

    assert_eq!(
        CliLocalHttpRequest::parse(b"GET /usage HTTP/1.1\r\nHost: evil.test\r\n\r\n", &allowed),
        Err(CliLocalHttpRequestError::DisallowedHost)
    );

    let wildcard = CliLocalHttpRequest::parse(raw, &CliLocalHttpAllowedHosts::Any).unwrap();
    assert_eq!(wildcard.host(), "192.168.1.10:8080");

    let alt_loopback = CliLocalHttpRequest::parse(
        b"GET /usage HTTP/1.1\r\nHost: 127.0.0.2\r\n\r\n",
        &CliServeSecurity::allowed_hosts("127.0.0.2"),
    )
    .unwrap();
    assert_eq!(alt_loopback.host(), "127.0.0.2");

    assert_eq!(
        CliLocalHttpRequest::parse(
            b"GET /usage HTTP/1.1\r\nHost: 192.168.1.10, evil.test\r\n\r\n",
            &CliLocalHttpAllowedHosts::Any
        ),
        Err(CliLocalHttpRequestError::DisallowedHost)
    );
}

// upstream: CLIServeRouterTests.swift:130 routes web UI health usage cost and dashboard endpoints
#[test]
fn routes_web_ui_health_usage_cost_and_dashboard_endpoints() {
    assert_eq!(
        CliServeRouter::route("GET", "/", &[]).unwrap(),
        CliServeRoute::WebUi
    );
    assert_eq!(
        CliServeRouter::route("GET", "/health", &[]).unwrap(),
        CliServeRoute::Health
    );
    assert_eq!(
        CliServeRouter::route("GET", "/accounts", &[]).unwrap(),
        CliServeRoute::Accounts { id: None }
    );
    assert_eq!(
        CliServeRouter::route(
            "GET",
            "/accounts/token-account:claude:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            &[]
        )
        .unwrap(),
        CliServeRoute::Accounts {
            id: Some("token-account:claude:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string())
        }
    );
    assert_eq!(
        CliServeRouter::route("GET", "/usage", &[]).unwrap(),
        CliServeRoute::Usage { provider: None }
    );
    assert_eq!(
        CliServeRouter::route(
            "GET",
            "/usage",
            &[("provider".to_string(), Some("claude".to_string()))]
        )
        .unwrap(),
        CliServeRoute::Usage {
            provider: Some("claude".to_string())
        }
    );
    assert_eq!(
        CliServeRouter::route(
            "GET",
            "/cost",
            &[("provider".to_string(), Some("codex".to_string()))]
        )
        .unwrap(),
        CliServeRoute::Cost {
            provider: Some("codex".to_string())
        }
    );
    assert_eq!(
        CliServeRouter::route("GET", "/dashboard/v1/snapshot", &[]).unwrap(),
        CliServeRoute::DashboardSnapshot {
            provider: None,
            detail: None
        }
    );
    assert_eq!(
        CliServeRouter::route(
            "GET",
            "/dashboard/v1/snapshot",
            &[("provider".to_string(), Some("claude".to_string()))]
        )
        .unwrap(),
        CliServeRoute::DashboardSnapshot {
            provider: Some("claude".to_string()),
            detail: None
        }
    );
    assert_eq!(
        CliServeRouter::route(
            "GET",
            "/dashboard/v1/snapshot",
            &[("detail".to_string(), Some("shell".to_string()))]
        )
        .unwrap(),
        CliServeRoute::DashboardSnapshot {
            provider: None,
            detail: Some("shell".to_string())
        }
    );
    assert_eq!(
        CliServeRouter::route(
            "GET",
            "/dashboard/v1/snapshot",
            &[("detail".to_string(), Some("full".to_string()))]
        )
        .unwrap(),
        CliServeRoute::DashboardSnapshot {
            provider: None,
            detail: Some("full".to_string())
        }
    );
    assert_eq!(
        CliServeRouter::route(
            "GET",
            "/dashboard/v1/snapshot",
            &[
                ("provider".to_string(), Some("codex".to_string())),
                ("detail".to_string(), Some("shell".to_string()))
            ]
        )
        .unwrap(),
        CliServeRoute::DashboardSnapshot {
            provider: Some("codex".to_string()),
            detail: Some("shell".to_string())
        }
    );
}

// upstream: CLIServeRouterTests.swift:180 rejects non get methods
#[test]
fn rejects_non_get_methods() {
    assert_eq!(
        CliServeRouter::route("POST", "/usage", &[]),
        Err(CliServeRouteError::MethodNotAllowed)
    );
}

// upstream: CLIServeRouterTests.swift:192 rejects post to web UI
#[test]
fn rejects_post_to_web_ui() {
    assert_eq!(
        CliServeRouter::route("POST", "/", &[]),
        Err(CliServeRouteError::MethodNotAllowed)
    );
}

// upstream: CLIServeRouterTests.swift:204 rejects unknown paths
#[test]
fn rejects_unknown_paths() {
    assert_eq!(
        CliServeRouter::route("GET", "/missing", &[]),
        Err(CliServeRouteError::NotFound)
    );
}

// upstream: CLIServeRouterTests.swift:216 health response reports ok status and build version
#[test]
fn health_response_reports_ok_status_and_build_version() {
    let response = serve_health_response(Some("1.2.3"));
    assert_eq!(response.status_code, 200);
    let obj: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(obj["status"], "ok");
    assert_eq!(obj["version"], "1.2.3");
}

// upstream: CLIServeRouterTests.swift:225 health response omits version detail when unavailable
#[test]
fn health_response_omits_version_detail_when_unavailable() {
    let response = serve_health_response(None);
    assert_eq!(response.status_code, 200);
    let obj: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(obj["status"], "ok");
    assert!(obj.get("version").is_none());
}

// upstream: CLIServeRouterTests.swift:234 serve numeric options reject malformed values
#[test]
fn serve_numeric_options_reject_malformed_values() {
    assert_eq!(decode_serve_port(Some("abc")), None);
    assert_eq!(decode_serve_port(Some("0")), None);
    assert_eq!(decode_serve_port(Some("65536")), None);
    assert_eq!(decode_serve_port(None), Some(8080));

    assert_eq!(decode_serve_refresh_interval(Some("later")), None);
    assert_eq!(decode_serve_refresh_interval(Some("-1")), None);
    assert_eq!(decode_serve_refresh_interval(Some("inf")), None);
    assert_eq!(decode_serve_refresh_interval(Some("86401")), Some(86401.0));
    assert_eq!(decode_serve_refresh_interval(Some("86400")), Some(86400.0));
    assert_eq!(decode_serve_refresh_interval(None), Some(60.0));

    assert_eq!(decode_serve_request_timeout(Some("soon")), None);
    assert_eq!(decode_serve_request_timeout(Some("-0.5")), None);
    assert_eq!(decode_serve_request_timeout(Some("inf")), None);
    assert_eq!(decode_serve_request_timeout(Some("0")), Some(0.0));
    assert_eq!(decode_serve_request_timeout(Some("12.5")), Some(12.5));
    assert_eq!(decode_serve_request_timeout(None), Some(30.0));
}

// upstream: CLIServeRouterTests.swift:304 serve help documents request timeout option
#[test]
fn serve_help_documents_request_timeout_option() {
    let serve = serve_help("0.0.0");
    let root = root_help("0.0.0");

    assert!(serve.contains("--request-timeout <seconds>"));
    assert!(
        serve.contains("ariadusage serve --port 8080 --refresh-interval 60 --request-timeout 30")
    );
    assert!(serve.contains("GET /accounts"));
    assert!(serve.contains("GET /accounts/<id>"));
    assert!(serve.contains("Account IDs are stable opaque identifiers"));
    assert!(serve.contains("never returns credentials or refreshes usage"));
    assert!(root.contains("--request-timeout <seconds>"));
}

// upstream: CLIServeRouterTests.swift:318 serve config snapshot reflects provider changes
#[test]
fn serve_config_snapshot_reflects_provider_changes() {
    let dir = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(dir.path().join("config.json"));
    let first_cfg = parse_config(serde_json::json!({
        "version": 1,
        "providers": [{ "id": "opencodego", "enabled": false }]
    }));
    store.save(&first_cfg).unwrap();
    let first_snapshot = load_serve_config_snapshot(&store).unwrap();

    let second_cfg = parse_config(serde_json::json!({
        "version": 1,
        "providers": [{ "id": "opencodego", "enabled": true }]
    }));
    store.save(&second_cfg).unwrap();
    let second_snapshot = load_serve_config_snapshot(&store).unwrap();

    assert!(
        !first_snapshot
            .config
            .enabled_providers()
            .iter()
            .any(|p| p == "opencodego")
    );
    assert!(
        second_snapshot
            .config
            .enabled_providers()
            .iter()
            .any(|p| p == "opencodego")
    );
    assert_ne!(first_snapshot.cache_token, second_snapshot.cache_token);

    let op_key = serve_operation_key("usage", None).unwrap();
    assert_eq!(op_key, serve_operation_key("usage", None).unwrap());
    assert_ne!(
        serve_cache_key(&op_key, &first_snapshot.cache_token),
        serve_cache_key(&op_key, &second_snapshot.cache_token)
    );
}

// upstream: CLIServeRouterTests.swift:343 serve cache skips provider error payloads
#[test]
fn serve_cache_skips_provider_error_payloads() {
    let success = CliLocalHttpResponse::json(
        200,
        "OK",
        br#"[{"provider":"codex","source":"local"}]"#.to_vec(),
    );
    let provider_error = CliLocalHttpResponse::json(
        200,
        "OK",
        br#"[{"provider":"codex","source":"local","error":{"message":"temporary"}}]"#.to_vec(),
    );
    let route_error =
        CliLocalHttpResponse::json(400, "Bad Request", br#"{"error":"bad request"}"#.to_vec());

    assert!(should_cache_serve_response(&success));
    assert!(!should_cache_serve_response(&provider_error));
    assert!(!should_cache_serve_response(&route_error));
}

// upstream: CLIServeRouterTests.swift:360 serve provider timeout stays below the request deadline
#[test]
fn serve_provider_timeout_stays_below_the_request_deadline() {
    let thirty = serve_provider_timeout(30.0).unwrap();
    let ten = serve_provider_timeout(10.0).unwrap();
    assert!((thirty - 24.0).abs() < 1e-9);
    assert!((ten - 8.0).abs() < 1e-9);
    assert_eq!(serve_provider_timeout(0.0), None);
    assert_eq!(serve_provider_timeout(f64::INFINITY), None);

    let one = serve_provider_timeout(1.0).unwrap();
    let half = serve_provider_timeout(0.5).unwrap();
    assert!(one < 1.0);
    assert!((half - 0.4).abs() < 1e-9);

    let oversized = serve_provider_timeout(f64::MAX).unwrap();
    assert!((oversized - 69_120.0).abs() < 1e-9);
    assert!(oversized < 86_400.0);
}

// upstream: CLIServeRouterTests.swift:383 serve usage collection bounds a hung provider without blocking others
#[tokio::test]
async fn serve_usage_collection_bounds_a_hung_provider_without_blocking_others() {
    let providers = vec![
        "codex".to_string(),
        "claude".to_string(),
        "gemini".to_string(),
    ];
    let release_provider = TestGate::new();
    let provider_started = TestGate::new();
    let provider_finished = TestGate::new();
    let fast_providers_returned = TestGate::new();
    let fast_count = Arc::new(AtomicUsize::new(0));
    let deadline_gate = TestGate::new();

    let now = Instant::now();
    let dg = deadline_gate.clone();
    let operations = CliServeOperationCoordinator::with_clock(
        Arc::new(move || now),
        Arc::new(move |_d| {
            let dg = dg.clone();
            Box::pin(async move {
                dg.wait().await;
                Ok(())
            })
        }),
    );

    let rp = release_provider.clone();
    let ps = provider_started.clone();
    let pf = provider_finished.clone();
    let fast_returned_gate = fast_providers_returned.clone();
    let fc = Arc::clone(&fast_count);
    let ops_for_task = operations.clone();

    let collection = tokio::spawn(async move {
        serve_collect_usage_outputs(
            &providers,
            "fixture",
            Some(now + Duration::from_secs(30)),
            &ops_for_task,
            move |provider, _sink| {
                let rp = rp.clone();
                let ps = ps.clone();
                let pf = pf.clone();
                let fast_returned_gate = fast_returned_gate.clone();
                let fc = Arc::clone(&fc);
                async move {
                    if provider == "claude" {
                        ps.open();
                        rp.wait().await;
                        pf.open();
                        return UsageCommandOutput::with_sections(vec![format!("late:{provider}")]);
                    }
                    if fc.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                        fast_returned_gate.open();
                    }
                    UsageCommandOutput::with_sections(vec![format!("ok:{provider}")])
                }
            },
        )
        .await
    });

    provider_started.wait().await;
    fast_providers_returned.wait().await;
    while operations.snapshot().await.operation_count != 1 {
        tokio::task::yield_now().await;
    }
    assert_eq!(operations.snapshot().await.operation_count, 1);
    deadline_gate.open();

    let output = collection.await.unwrap();
    assert!(!release_provider.is_open());
    release_provider.open();
    provider_finished.wait().await;

    assert_eq!(output.sections, vec!["ok:codex", "ok:gemini"]);
    assert_eq!(output.payload.len(), 1);
    assert_eq!(output.payload[0].provider, "claude");
    assert!(output.payload[0].error.is_some());
    assert_eq!(
        output.payload[0].error.as_ref().unwrap().kind.as_deref(),
        Some("provider")
    );
    assert_eq!(output.payload[0].cache_account_key, None);
    assert_eq!(output.payload[0].account, None);
    assert_eq!(output.exit_code, 1);
}

// upstream: CLIServeRouterTests.swift:442 serve usage collection adds no join bound when request deadline is disabled
#[tokio::test]
async fn serve_usage_collection_adds_no_join_bound_when_request_deadline_is_disabled() {
    let providers = vec!["codex".to_string(), "claude".to_string()];
    let operations = CliServeOperationCoordinator::new();
    let output = serve_collect_usage_outputs(
        &providers,
        "",
        None,
        &operations,
        |provider, _sink| async move {
            if provider == "codex" {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            UsageCommandOutput::with_sections(vec![format!("ok:{provider}")])
        },
    )
    .await;

    assert_eq!(output.sections, vec!["ok:codex", "ok:claude"]);
    assert!(output.payload.is_empty());
    assert_eq!(output.exit_code, 0);
}

// upstream: CLIServeRouterTests.swift:459 serve cache uses stable Codex account identities
#[test]
fn serve_cache_uses_stable_codex_account_identities() {
    let stored_id = Uuid::new_v4();
    let first_projection = make_codex_visible_account(
        "email-shaped-id",
        "user@example.com",
        Some("workspace-1"),
        Some("auth-1"),
        Some(stored_id),
    );
    let reshaped_projection = make_codex_visible_account(
        &format!("managed:{}", stored_id.hyphenated()),
        "user@example.com",
        Some("workspace-1"),
        Some("auth-1"),
        Some(stored_id),
    );
    let replacement = make_codex_visible_account(
        "email-shaped-id",
        "user@example.com",
        Some("workspace-2"),
        Some("auth-2"),
        Some(Uuid::new_v4()),
    );
    let workspace_peer = make_codex_visible_account(
        "workspace-peer",
        "other@example.com",
        Some("workspace-1"),
        Some("auth-3"),
        Some(Uuid::new_v4()),
    );
    let ambiguous = make_codex_visible_account("email-only", "user@example.com", None, None, None);
    let stored_before_refresh = make_codex_visible_account(
        "stored-before",
        "user@example.com",
        None,
        Some("old-auth"),
        Some(stored_id),
    );
    let stored_after_refresh = make_codex_visible_account(
        "stored-after",
        "user@example.com",
        None,
        Some("new-auth"),
        Some(stored_id),
    );

    let first_key = codex_usage_cache_account_key("codex", None, Some(&first_projection));
    let reshaped_key = codex_usage_cache_account_key("codex", None, Some(&reshaped_projection));
    let replacement_key = codex_usage_cache_account_key("codex", None, Some(&replacement));
    let workspace_peer_key = codex_usage_cache_account_key("codex", None, Some(&workspace_peer));
    let stored_before_key =
        codex_usage_cache_account_key("codex", None, Some(&stored_before_refresh));
    let stored_after_key =
        codex_usage_cache_account_key("codex", None, Some(&stored_after_refresh));

    assert_eq!(first_key, reshaped_key);
    assert_ne!(first_key, replacement_key);
    assert_ne!(first_key, workspace_peer_key);
    assert_eq!(stored_before_key, stored_after_key);
    assert_eq!(
        codex_usage_cache_account_key("codex", None, Some(&ambiguous)),
        None
    );
    assert_eq!(
        codex_usage_cache_account_key("antigravity", None, None),
        None
    );
}

// upstream: CLIServeRouterTests.swift:538 serve cache coalesces concurrent cache misses
#[tokio::test]
async fn serve_cache_coalesces_concurrent_cache_misses() {
    let cache = CliServeResponseCache::new();
    let counter = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for _ in 0..5 {
        let cache = cache.clone();
        let counter = Arc::clone(&counter);
        handles.push(tokio::spawn(async move {
            cached_serve_response("usage:", &cache, 60.0, 1.0, "", false, move || {
                let counter = Arc::clone(&counter);
                async move {
                    let call = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    router_test_response(
                        &format!(r#"[{{"provider":"codex","call":{call}}}]"#),
                        200,
                        None,
                    )
                }
            })
            .await
        }));
    }

    let mut responses = Vec::new();
    for h in handles {
        responses.push(h.await.unwrap());
    }

    assert_eq!(counter.load(Ordering::SeqCst), 1);
    let bodies: BTreeSet<String> = responses
        .iter()
        .map(|r| String::from_utf8(r.body.clone()).unwrap())
        .collect();
    assert_eq!(bodies.len(), 1);
    assert!(responses.iter().all(|r| r.status_code == 200));
    assert!(bodies.iter().next().unwrap().contains("\"call\":1"));
}

// upstream: CLIServeRouterTests.swift:572 serve cache prunes expired config token entries
#[tokio::test]
async fn serve_cache_prunes_expired_config_token_entries() {
    let (clock, cache) = make_serve_test_cache();

    let _ = cached_serve_response(
        "usage::old-config",
        &cache,
        1.0,
        30.0,
        "",
        false,
        || async { router_test_response(r#"[{"provider":"codex","config":"old"}]"#, 200, None) },
    )
    .await;
    assert_eq!(cache.cached_entry_count(), 1);

    clock.advance(1);
    let _ = cached_serve_response(
        "usage::new-config",
        &cache,
        60.0,
        30.0,
        "",
        false,
        || async { router_test_response(r#"[{"provider":"codex","config":"new"}]"#, 200, None) },
    )
    .await;

    assert_eq!(cache.cached_entry_count(), 1);
}

// upstream: CLIServeRouterTests.swift:597 serve cache does not cache timeouts and recovers on next success
#[tokio::test]
async fn serve_cache_does_not_cache_timeouts_and_recovers_on_next_success() {
    let cache = CliServeResponseCache::new();
    let counter = Arc::new(AtomicUsize::new(0));
    let cancel_gate = TestGate::new();

    let cg = cancel_gate.clone();
    let c1 = Arc::clone(&counter);
    let timeout = cached_serve_response("usage:", &cache, 60.0, 0.01, "", false, move || {
        let cg = cg.clone();
        let c1 = Arc::clone(&c1);
        async move {
            c1.fetch_add(1, Ordering::SeqCst);
            cg.wait().await;
            serve_timeout_response()
        }
    })
    .await;

    assert_eq!(timeout.status_code, 504);
    assert!(String::from_utf8_lossy(&timeout.body).contains("request timed out"));

    cancel_gate.open();
    for _ in 0..1000 {
        if cache.operations.snapshot().await.operation_count == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(cache.operations.snapshot().await.operation_count, 0);

    let c2 = Arc::clone(&counter);
    let success = cached_serve_response("usage:", &cache, 60.0, 1.0, "", false, move || {
        let c2 = Arc::clone(&c2);
        async move {
            let call = c2.fetch_add(1, Ordering::SeqCst) + 1;
            router_test_response(
                &format!(r#"[{{"provider":"codex","call":{call}}}]"#),
                200,
                None,
            )
        }
    })
    .await;

    assert_eq!(success.status_code, 200);
    assert!(String::from_utf8_lossy(&success.body).contains("\"call\":2"));

    let c3 = Arc::clone(&counter);
    let cached = cached_serve_response("usage:", &cache, 60.0, 1.0, "", false, move || {
        let c3 = Arc::clone(&c3);
        async move {
            let call = c3.fetch_add(1, Ordering::SeqCst) + 1;
            router_test_response(
                &format!(r#"[{{"provider":"codex","call":{call}}}]"#),
                200,
                None,
            )
        }
    })
    .await;

    assert_eq!(cached.status_code, 200);
    assert_eq!(cached.body, success.body);
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

// upstream: CLIServeRouterTests.swift:658 serve cache resumes coalesced waiters on timeout
#[tokio::test]
async fn serve_cache_resumes_coalesced_waiters_on_timeout() {
    let cache = CliServeResponseCache::new();
    let counter = Arc::new(AtomicUsize::new(0));
    let release = TestGate::new();

    let mut handles = Vec::new();
    for _ in 0..4 {
        let cache = cache.clone();
        let counter = Arc::clone(&counter);
        let release = release.clone();
        handles.push(tokio::spawn(async move {
            cached_serve_response("usage:", &cache, 60.0, 0.01, "", false, move || {
                let counter = Arc::clone(&counter);
                let release = release.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    release.wait().await;
                    router_test_response(r#"[{"provider":"codex"}]"#, 200, None)
                }
            })
            .await
        }));
    }

    let mut responses = Vec::new();
    for h in handles {
        responses.push(h.await.unwrap());
    }
    release.open();

    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(responses.len(), 4);
    assert!(responses.iter().all(|r| r.status_code == 504));
    assert!(
        responses
            .iter()
            .all(|r| String::from_utf8_lossy(&r.body).contains("request timed out"))
    );
}

// upstream: CLIServeRouterTests.swift:692 serve cache serves last good payload when refresh fails
#[tokio::test]
async fn serve_cache_serves_last_good_payload_when_refresh_fails() {
    let (clock, cache) = make_serve_test_cache();
    let counter = Arc::new(AtomicUsize::new(0));

    let c1 = Arc::clone(&counter);
    let first = cached_serve_response(
        "usage:antigravity",
        &cache,
        1.0,
        1.0,
        "",
        false,
        move || {
            let c1 = Arc::clone(&c1);
            async move {
                let call = c1.fetch_add(1, Ordering::SeqCst) + 1;
                router_test_response(
                    &format!(r#"[{{"provider":"antigravity","call":{call}}}]"#),
                    200,
                    None,
                )
            }
        },
    )
    .await;
    assert_eq!(first.status_code, 200);

    clock.advance(1);

    let c2 = Arc::clone(&counter);
    let failed = cached_serve_response(
        "usage:antigravity",
        &cache,
        1.0,
        1.0,
        "",
        false,
        move || {
            let c2 = Arc::clone(&c2);
            async move {
                c2.fetch_add(1, Ordering::SeqCst);
                router_test_response(
                    r#"[{"provider":"antigravity","error":{"message":"transient"}}]"#,
                    200,
                    None,
                )
            }
        },
    )
    .await;

    assert_eq!(failed.status_code, 200);
    let failed_rows = json_rows(&failed);
    assert_eq!(failed_rows[0]["call"], 1);
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    clock.advance(1);

    let c3 = Arc::clone(&counter);
    let recovered = cached_serve_response(
        "usage:antigravity",
        &cache,
        1.0,
        1.0,
        "",
        false,
        move || {
            let c3 = Arc::clone(&c3);
            async move {
                let call = c3.fetch_add(1, Ordering::SeqCst) + 1;
                router_test_response(
                    &format!(r#"[{{"provider":"antigravity","call":{call}}}]"#),
                    200,
                    None,
                )
            }
        },
    )
    .await;

    assert_eq!(recovered.status_code, 200);
    assert!(String::from_utf8_lossy(&recovered.body).contains("\"call\":3"));
}

// upstream: CLIServeRouterTests.swift:743 cost refresh timeout serves the last good payload
#[tokio::test]
async fn cost_refresh_timeout_serves_the_last_good_payload() {
    let clock = ServeTestClock::new(Timestamp::from_second(1_000).unwrap());
    let deadline_gate = TestGate::new();
    let release_source = TestGate::new();
    let dg = deadline_gate.clone();
    let operations = CliServeOperationCoordinator::with_clock(
        Arc::new(Instant::now),
        Arc::new(move |_d| {
            let dg = dg.clone();
            Box::pin(async move {
                dg.wait().await;
                Ok(())
            })
        }),
    );
    let clk = Arc::clone(&clock);
    let cache = CliServeResponseCache::with_clock(operations.clone(), Arc::new(move || clk.now()));
    let counter = Arc::new(AtomicUsize::new(0));

    let c1 = Arc::clone(&counter);
    let first = cached_serve_response("cost:", &cache, 1.0, 0.0, "", false, move || {
        let c1 = Arc::clone(&c1);
        async move {
            let call = c1.fetch_add(1, Ordering::SeqCst) + 1;
            router_test_response(
                &format!(r#"[{{"provider":"codex","call":{call}}}]"#),
                200,
                None,
            )
        }
    })
    .await;
    clock.advance(1);

    let source_entered = TestGate::new();
    let se = source_entered.clone();
    let rs = release_source.clone();
    let c2 = Arc::clone(&counter);
    let cache2 = cache.clone();
    let request = tokio::spawn(async move {
        cached_serve_response("cost:", &cache2, 1.0, 30.0, "", false, move || {
            let se = se.clone();
            let rs = rs.clone();
            let c2 = Arc::clone(&c2);
            async move {
                let call = c2.fetch_add(1, Ordering::SeqCst) + 1;
                se.open();
                rs.wait().await;
                router_test_response(
                    &format!(r#"[{{"provider":"codex","call":{call}}}]"#),
                    200,
                    None,
                )
            }
        })
        .await
    });

    source_entered.wait().await;
    deadline_gate.open();
    let timed_out = request.await.unwrap();

    assert_eq!(timed_out.status_code, 200);
    let first_rows = json_rows(&first);
    let timed_out_rows = json_rows(&timed_out);
    assert_eq!(first_rows[0]["provider"], "codex");
    assert_eq!(timed_out_rows[0]["provider"], "codex");
    assert_eq!(first_rows[0]["call"], 1);
    assert_eq!(timed_out_rows[0]["call"], 1);
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    release_source.open();
    for _ in 0..1000 {
        if operations.snapshot().await.operation_count == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(operations.snapshot().await.operation_count, 0);
}

// upstream: CLIServeRouterTests.swift:799 cost refresh keeps fresh providers while replacing timed out rows
#[tokio::test]
async fn cost_refresh_keeps_fresh_providers_while_replacing_timed_out_rows() {
    let (clock, cache) = make_serve_test_cache();
    let release = TestGate::new();

    let _ = cached_serve_response("cost:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","call":1},{"provider":"claude","call":1}]"#,
            200,
            None,
        )
    })
    .await;
    clock.advance(1);

    let partial = cached_serve_response("cost:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","call":2},{"provider":"claude","error":{"message":"claude cost refresh timed out"}}]"#,
            200,
            None,
        )
    })
    .await;
    let partial_rows = json_rows(&partial);
    assert_eq!(find_row(&partial_rows, "codex", None).unwrap()["call"], 2);
    assert_eq!(find_row(&partial_rows, "claude", None).unwrap()["call"], 1);
    assert!(partial_rows.iter().all(|r| r.get("error").is_none()));

    clock.advance(1);
    let rel = release.clone();
    let timed_out = cached_serve_response("cost:", &cache, 1.0, 0.01, "", false, move || {
        let rel = rel.clone();
        async move {
            rel.wait().await;
            router_test_response(r#"[{"provider":"codex","call":3}]"#, 200, None)
        }
    })
    .await;
    release.open();
    let timeout_rows = json_rows(&timed_out);
    assert_eq!(find_row(&timeout_rows, "codex", None).unwrap()["call"], 2);
    assert_eq!(find_row(&timeout_rows, "claude", None).unwrap()["call"], 1);
}

// upstream: CLIServeRouterTests.swift:851 serve cache replaces only failed provider account rows
#[tokio::test]
async fn serve_cache_replaces_only_failed_provider_account_rows() {
    let (clock, cache) = make_serve_test_cache();

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[
              {"provider":"codex","account":"personal","call":1},
              {"provider":"antigravity","account":"work","call":1},
              {"provider":"antigravity","account":"personal","call":1}
            ]"#,
            200,
            None,
        )
    })
    .await;
    clock.advance(1);

    let refreshed = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[
              {"provider":"codex","account":"personal","call":2},
              {"provider":"antigravity","account":"work","error":{"message":"transient"}},
              {"provider":"antigravity","account":"personal","call":2}
            ]"#,
            200,
            None,
        )
    })
    .await;
    let rows = json_rows(&refreshed);

    assert_eq!(
        find_row(&rows, "codex", Some("personal")).unwrap()["call"],
        2
    );
    assert_eq!(
        find_row(&rows, "antigravity", Some("work")).unwrap()["call"],
        1
    );
    assert_eq!(
        find_row(&rows, "antigravity", Some("personal")).unwrap()["call"],
        2
    );
    assert!(rows.iter().all(|r| r.get("error").is_none()));
}

// upstream: CLIServeRouterTests.swift:894 serve cache retains newer per-row success across all-error refresh
#[tokio::test]
async fn serve_cache_retains_newer_per_row_success_across_all_error_refresh() {
    let (clock, cache) = make_serve_test_cache();

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","call":1},{"provider":"antigravity","call":1}]"#,
            200,
            None,
        )
    })
    .await;
    clock.advance(1);

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","call":2},{"provider":"antigravity","error":{"message":"transient"}}]"#,
            200,
            None,
        )
    })
    .await;
    clock.advance(1);

    let failed = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","error":{"message":"transient"}},{"provider":"antigravity","error":{"message":"transient"}}]"#,
            200,
            None,
        )
    })
    .await;
    let rows = json_rows(&failed);

    assert_eq!(find_row(&rows, "codex", None).unwrap()["call"], 2);
    assert_eq!(find_row(&rows, "antigravity", None).unwrap()["call"], 1);
}

// upstream: CLIServeRouterTests.swift:947 serve cache fails closed on timeout after merged rows
#[tokio::test]
async fn serve_cache_fails_closed_on_timeout_after_merged_rows() {
    let (clock, cache) = make_serve_test_cache();
    let release = TestGate::new();

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","call":1},{"provider":"antigravity","call":1}]"#,
            200,
            None,
        )
    })
    .await;
    clock.advance(1);

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","call":2},{"provider":"antigravity","error":{"message":"transient"}}]"#,
            200,
            None,
        )
    })
    .await;
    clock.advance(1);

    let rel = release.clone();
    let timed_out = cached_serve_response("usage:", &cache, 1.0, 0.01, "", false, move || {
        let rel = rel.clone();
        async move {
            rel.wait().await;
            router_test_response("[]", 200, None)
        }
    })
    .await;
    release.open();
    assert_eq!(timed_out.status_code, 504);
    let body = String::from_utf8_lossy(&timed_out.body);
    assert!(!body.contains("\"call\":1"));
    assert!(!body.contains("\"call\":2"));
}

// upstream: CLIServeRouterTests.swift:995 serve cache fails closed on timeout after a partial refresh
#[tokio::test]
async fn serve_cache_fails_closed_on_timeout_after_a_partial_refresh() {
    let (clock, cache) = make_serve_test_cache();
    let release = TestGate::new();

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(r#"[{"provider":"codex","call":1}]"#, 200, None)
    })
    .await;
    clock.advance(1);

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","call":2},{"provider":"antigravity","error":{"message":"transient"}}]"#,
            200,
            None,
        )
    })
    .await;
    clock.advance(1);

    let rel = release.clone();
    let timed_out = cached_serve_response("usage:", &cache, 1.0, 0.01, "", false, move || {
        let rel = rel.clone();
        async move {
            rel.wait().await;
            router_test_response("[]", 200, None)
        }
    })
    .await;
    release.open();
    assert_eq!(timed_out.status_code, 504);
    let body = String::from_utf8_lossy(&timed_out.body);
    assert!(!body.contains("\"call\":2"));
    assert!(!body.contains("antigravity"));
}

// upstream: CLIServeRouterTests.swift:1038 serve cache does not reconstruct usage rows after timeout
#[tokio::test]
async fn serve_cache_does_not_reconstruct_usage_rows_after_timeout() {
    let cache = CliServeResponseCache::new();
    let policy = CachePolicy {
        ttl: 0.0,
        stale_ttl: 10.0,
    };
    let started_at = Timestamp::from_second(1_000).unwrap();

    let _ = cache.complete_fetch(
        router_test_response(
            r#"[{"provider":"codex","call":1},{"provider":"antigravity","call":1}]"#,
            200,
            None,
        ),
        "usage:",
        policy,
        started_at,
        true,
    );

    let partial_at = started_at
        .checked_add(jiff::SignedDuration::from_secs(9))
        .unwrap();
    let _ = cache.complete_fetch(
        router_test_response(
            r#"[{"provider":"codex","call":2},{"provider":"antigravity","error":{"message":"transient"}}]"#,
            200,
            None,
        ),
        "usage:",
        policy,
        partial_at,
        false,
    );

    let timeout_at = started_at
        .checked_add(jiff::SignedDuration::from_secs(11))
        .unwrap();
    let timed_out = cache.complete_fetch(
        serve_timeout_response(),
        "usage:",
        policy,
        timeout_at,
        false,
    );
    assert_eq!(timed_out.status_code, 504);
    let body = String::from_utf8_lossy(&timed_out.body);
    assert!(body.contains("request timed out"));
    assert!(!body.contains("\"call\":2"));
}

// upstream: CLIServeRouterTests.swift:1082 serve cache preserves newer row when another failed row has no fallback
#[tokio::test]
async fn serve_cache_preserves_newer_row_when_another_failed_row_has_no_fallback() {
    let (clock, cache) = make_serve_test_cache();

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(r#"[{"provider":"codex","call":1}]"#, 200, None)
    })
    .await;
    clock.advance(1);

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","call":2},{"provider":"antigravity","error":{"message":"transient"}}]"#,
            200,
            None,
        )
    })
    .await;
    clock.advance(1);

    let failed = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","error":{"message":"transient"}},{"provider":"antigravity","error":{"message":"transient"}}]"#,
            200,
            None,
        )
    })
    .await;
    let rows = json_rows(&failed);

    assert_eq!(find_row(&rows, "codex", None).unwrap()["call"], 2);
    assert!(
        find_row(&rows, "antigravity", None)
            .unwrap()
            .get("error")
            .is_some()
    );
}

// upstream: CLIServeRouterTests.swift:1130 serve cache keeps fresh rows when a failed row has no stale match
#[tokio::test]
async fn serve_cache_keeps_fresh_rows_when_a_failed_row_has_no_stale_match() {
    let (clock, cache) = make_serve_test_cache();

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","account":"personal","call":1}]"#,
            200,
            None,
        )
    })
    .await;
    clock.advance(1);

    let refreshed = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","account":"personal","call":2},{"provider":"antigravity","account":"work","error":{"message":"transient"}}]"#,
            200,
            None,
        )
    })
    .await;
    let rows = json_rows(&refreshed);

    assert_eq!(
        find_row(&rows, "codex", Some("personal")).unwrap()["call"],
        2
    );
    assert!(
        find_row(&rows, "antigravity", Some("work"))
            .unwrap()
            .get("error")
            .is_some()
    );
}

// upstream: CLIServeRouterTests.swift:1164 serve cache does not merge duplicate provider account labels
#[tokio::test]
async fn serve_cache_does_not_merge_duplicate_provider_account_labels() {
    let (clock, cache) = make_serve_test_cache();

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[
              {"provider":"codex","account":"shared","slot":"first","call":1},
              {"provider":"codex","account":"shared","slot":"second","call":1}
            ]"#,
            200,
            None,
        )
    })
    .await;
    clock.advance(1);

    let refreshed = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[
              {"provider":"codex","account":"shared","slot":"first","error":{"message":"transient"}},
              {"provider":"codex","account":"shared","slot":"second","call":2}
            ]"#,
            200,
            None,
        )
    })
    .await;
    let rows = json_rows(&refreshed);
    let first = rows.iter().find(|r| r["slot"] == "first").unwrap();
    let second = rows.iter().find(|r| r["slot"] == "second").unwrap();

    assert!(first.get("error").is_some());
    assert_eq!(second["call"], 2);
}

// upstream: CLIServeRouterTests.swift:1210 serve cache follows stable account identity across label changes
#[tokio::test]
async fn serve_cache_follows_stable_account_identity_across_label_changes() {
    let (clock, cache) = make_serve_test_cache();

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","account":"old label","call":1}]"#,
            200,
            Some(vec![Some("account-1".to_string())]),
        )
    })
    .await;
    clock.advance(1);

    let failed = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","account":"new label","error":{"message":"transient"}}]"#,
            200,
            Some(vec![Some("account-1".to_string())]),
        )
    })
    .await;
    let rows = json_rows(&failed);
    assert_eq!(rows[0]["account"], "old label");
    assert_eq!(rows[0]["call"], 1);
}

// upstream: CLIServeRouterTests.swift:1242 serve cache does not reuse a label for a different account identity
#[tokio::test]
async fn serve_cache_does_not_reuse_a_label_for_a_different_account_identity() {
    let (clock, cache) = make_serve_test_cache();

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","account":"shared","call":1}]"#,
            200,
            Some(vec![Some("account-1".to_string())]),
        )
    })
    .await;
    clock.advance(1);

    let refreshed = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[
              {"provider":"codex","account":"shared","error":{"message":"transient"}},
              {"provider":"antigravity","account":"work","call":2}
            ]"#,
            200,
            Some(vec![
                Some("account-2".to_string()),
                Some("account-3".to_string()),
            ]),
        )
    })
    .await;
    let rows = json_rows(&refreshed);

    assert!(
        find_row(&rows, "codex", Some("shared"))
            .unwrap()
            .get("error")
            .is_some()
    );
    assert_eq!(
        find_row(&rows, "antigravity", Some("work")).unwrap()["call"],
        2
    );
}

// upstream: CLIServeRouterTests.swift:1279 serve cache does not use whole fallback after an account switch
#[tokio::test]
async fn serve_cache_does_not_use_whole_fallback_after_an_account_switch() {
    let (clock, cache) = make_serve_test_cache();
    let release = TestGate::new();

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","account":"shared","call":1}]"#,
            200,
            Some(vec![Some("account-1".to_string())]),
        )
    })
    .await;
    clock.advance(1);

    let failed = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","account":"shared","error":{"message":"transient"}}]"#,
            200,
            Some(vec![Some("account-2".to_string())]),
        )
    })
    .await;
    let rows = json_rows(&failed);
    assert!(rows[0].get("call").is_none());
    assert!(rows[0].get("error").is_some());

    let rel = release.clone();
    let timed_out = cached_serve_response("usage:", &cache, 1.0, 0.01, "", false, move || {
        let rel = rel.clone();
        async move {
            rel.wait().await;
            router_test_response(
                r#"[{"provider":"codex","account":"shared","call":3}]"#,
                200,
                Some(vec![Some("account-2".to_string())]),
            )
        }
    })
    .await;
    release.open();

    assert_eq!(timed_out.status_code, 504);
    assert!(!String::from_utf8_lossy(&timed_out.body).contains("\"call\":1"));
}

// upstream: CLIServeRouterTests.swift:1326 serve cache prunes accounts absent from a successful snapshot
#[tokio::test]
async fn serve_cache_prunes_accounts_absent_from_a_successful_snapshot() {
    let (clock, cache) = make_serve_test_cache();

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"codex","account":"shared","call":1}]"#,
            200,
            None,
        )
    })
    .await;
    clock.advance(1);

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"antigravity","account":"work","call":2}]"#,
            200,
            None,
        )
    })
    .await;
    clock.advance(1);

    let refreshed = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[
              {"provider":"codex","account":"shared","error":{"message":"transient"}},
              {"provider":"antigravity","account":"work","call":3}
            ]"#,
            200,
            None,
        )
    })
    .await;
    let rows = json_rows(&refreshed);

    assert!(
        find_row(&rows, "codex", Some("shared"))
            .unwrap()
            .get("error")
            .is_some()
    );
    assert_eq!(
        find_row(&rows, "antigravity", Some("work")).unwrap()["call"],
        3
    );
}

// upstream: CLIServeRouterTests.swift:1369 serve cache fails closed when all-error rows have ambiguous identities
#[tokio::test]
async fn serve_cache_fails_closed_when_all_error_rows_have_ambiguous_identities() {
    let (clock, cache) = make_serve_test_cache();

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[
              {"provider":"codex","account":"shared","slot":"first","call":1},
              {"provider":"codex","account":"shared","slot":"second","call":1},
              {"provider":"antigravity","account":"work","call":1}
            ]"#,
            200,
            Some(vec![None, None, None]),
        )
    })
    .await;
    clock.advance(1);

    let failed = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[
              {"provider":"codex","account":"shared","slot":"first","error":{"message":"transient"}},
              {"provider":"codex","account":"shared","slot":"second","error":{"message":"transient"}},
              {"provider":"antigravity","account":"work","error":{"message":"transient"}}
            ]"#,
            200,
            Some(vec![None, None, None]),
        )
    })
    .await;
    let rows = json_rows(&failed);

    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.get("call").is_none()));
    assert!(rows.iter().all(|r| r.get("error").is_some()));
}

// upstream: CLIServeRouterTests.swift:1424 serve cache does not whole-fallback ambiguous usage after timeout
#[tokio::test]
async fn serve_cache_does_not_whole_fallback_ambiguous_usage_after_timeout() {
    let (clock, cache) = make_serve_test_cache();
    let release = TestGate::new();

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[{"provider":"antigravity","account":"first@example.com","call":1}]"#,
            200,
            Some(vec![None]),
        )
    })
    .await;
    clock.advance(1);

    let rel = release.clone();
    let timed_out = cached_serve_response("usage:", &cache, 1.0, 0.01, "", false, move || {
        let rel = rel.clone();
        async move {
            rel.wait().await;
            router_test_response(
                r#"[{"provider":"antigravity","account":"second@example.com","call":2}]"#,
                200,
                Some(vec![None]),
            )
        }
    })
    .await;
    release.open();

    assert_eq!(timed_out.status_code, 504);
    let body = String::from_utf8_lossy(&timed_out.body);
    assert!(!body.contains("first@example.com"));
    assert!(!body.contains("\"call\":1"));
}

// upstream: CLIServeRouterTests.swift:1457 serve cache mixed identities do not enable timeout fallback
#[tokio::test]
async fn serve_cache_mixed_identities_do_not_enable_timeout_fallback() {
    let (clock, cache) = make_serve_test_cache();
    let release = TestGate::new();

    let _ = cached_serve_response("usage:", &cache, 1.0, 1.0, "", false, || async {
        router_test_response(
            r#"[
              {"provider":"codex","account":"stable@example.com","call":1},
              {"provider":"antigravity","account":"ambient@example.com","call":1}
            ]"#,
            200,
            Some(vec![Some("account-1".to_string()), None]),
        )
    })
    .await;
    clock.advance(1);

    let rel = release.clone();
    let timed_out = cached_serve_response("usage:", &cache, 1.0, 0.01, "", false, move || {
        let rel = rel.clone();
        async move {
            rel.wait().await;
            router_test_response("[]", 200, Some(vec![]))
        }
    })
    .await;
    release.open();

    assert_eq!(timed_out.status_code, 504);
    let body = String::from_utf8_lossy(&timed_out.body);
    assert!(!body.contains("stable@example.com"));
    assert!(!body.contains("ambient@example.com"));
}

// upstream: CLIServeRouterTests.swift:1492 serve stale ttl is bounded and disabled without caching
#[test]
fn serve_stale_ttl_is_bounded_and_disabled_without_caching() {
    assert_eq!(serve_stale_ttl(0.0), 0.0);
    assert_eq!(serve_stale_ttl(1.0), 300.0);
    assert_eq!(serve_stale_ttl(60.0), 600.0);
    assert_eq!(serve_stale_ttl(1800.0), 3600.0);
    assert_eq!(serve_stale_ttl(86401.0), 3600.0);
    assert_eq!(serve_stale_ttl(f64::INFINITY), 3600.0);
}

// upstream: CLIServeRouterTests.swift:1502 serve cache prunes stale variants from old configurations
#[tokio::test]
async fn serve_cache_prunes_stale_variants_from_old_configurations() {
    let cache = CliServeResponseCache::new();
    let started_at = Timestamp::from_second(1_000).unwrap();
    let policy = CachePolicy {
        ttl: 0.0,
        stale_ttl: CliServeResponseCache::MAXIMUM_STALE_TTL,
    };

    let _ = cache.complete_fetch(
        router_test_response(r#"{"status":"ok"}"#, 200, None),
        "config:old",
        policy,
        started_at,
        true,
    );
    let _ = cache.complete_fetch(
        router_test_response(
            r#"[{"provider":"codex","call":1}]"#,
            200,
            Some(vec![Some("account-1".to_string())]),
        ),
        "usage:old",
        policy,
        started_at,
        true,
    );
    assert_eq!(cache.cached_stale_variant_count(), 2);

    let expired_at = started_at
        .checked_add(jiff::SignedDuration::from_secs(
            (CliServeResponseCache::MAXIMUM_STALE_TTL as i64) + 1,
        ))
        .unwrap();
    let _ = cache.cached_response("config:new", expired_at);
    assert_eq!(cache.cached_stale_variant_count(), 0);
    let _ = cache.complete_fetch(
        router_test_response(r#"{"status":"ok"}"#, 200, None),
        "config:new",
        policy,
        expired_at,
        false,
    );
}

// upstream: CLIServeRouterTests.swift:1538 serve helper idle window outlives the refresh cadence
#[test]
fn serve_helper_idle_window_outlives_the_refresh_cadence() {
    assert_eq!(serve_cli_session_idle_window(0.0), 180.0);
    assert_eq!(serve_cli_session_idle_window(60.0), 180.0);
    assert_eq!(serve_cli_session_idle_window(300.0), 360.0);
}

// upstream: CLIServeRouterTests.swift:1545 local HTTP server stops its accept loop
#[tokio::test]
async fn local_http_server_stops_its_accept_loop() {
    let server = CliLocalHttpServer::bind("127.0.0.1", 0, 5_000, 10_000, 16)
        .await
        .unwrap();
    let (tx, rx) = watch::channel(false);
    let task = tokio::spawn(async move {
        server
            .serve(
                rx,
                Arc::new(|_req| {
                    Box::pin(async {
                        CliLocalHttpResponse::json(200, "OK", br#"{"status":"ok"}"#.to_vec())
                    })
                }),
            )
            .await;
    });
    tokio::task::yield_now().await;
    let _ = tx.send(true);
    task.await.unwrap();
}

// upstream: CLIServeRouterTests.swift:1562 serve request timeout zero disables the deadline
#[tokio::test]
async fn serve_request_timeout_zero_disables_the_deadline() {
    let cache = CliServeResponseCache::new();
    let response = cached_serve_response("usage:", &cache, 0.0, 0.0, "", false, || async {
        tokio::time::sleep(Duration::from_millis(80)).await;
        router_test_response(r#"[{"provider":"codex","slow":true}]"#, 200, None)
    })
    .await;

    assert_eq!(response.status_code, 200);
    assert!(String::from_utf8_lossy(&response.body).contains("\"slow\":true"));
}

// ============================================================================
// 6. CLIServeTimeoutTests.swift (23 tests)
// ============================================================================

struct ManualDeadlineClockState {
    instant: Instant,
    pending: Vec<(Uuid, tokio::sync::oneshot::Sender<()>)>,
    cancellation_count: usize,
}

struct ServeManualDeadlineClock {
    state: Mutex<ManualDeadlineClockState>,
    pending_notify: Notify,
    cancel_notify: Notify,
}

struct SleepCancelGuard {
    clock: Arc<ServeManualDeadlineClock>,
    id: Uuid,
    completed: bool,
}

impl Drop for SleepCancelGuard {
    fn drop(&mut self) {
        if !self.completed {
            let mut guard = self.clock.state.lock().unwrap();
            if let Some(pos) = guard.pending.iter().position(|(id, _)| *id == self.id) {
                guard.pending.remove(pos);
            }
            guard.cancellation_count += 1;
            drop(guard);
            self.clock.cancel_notify.notify_waiters();
        }
    }
}

impl ServeManualDeadlineClock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ManualDeadlineClockState {
                instant: Instant::now(),
                pending: Vec::new(),
                cancellation_count: 0,
            }),
            pending_notify: Notify::new(),
            cancel_notify: Notify::new(),
        })
    }

    fn now(&self) -> Instant {
        self.state.lock().unwrap().instant
    }

    fn advance(&self, duration: Duration) {
        let mut guard = self.state.lock().unwrap();
        guard.instant += duration;
    }

    async fn sleep(self: &Arc<Self>, _deadline: Instant) -> Result<(), CoordinatorCancelled> {
        let id = Uuid::new_v4();
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut guard = self.state.lock().unwrap();
            guard.pending.push((id, tx));
        }
        self.pending_notify.notify_waiters();
        let mut cancel_guard = SleepCancelGuard {
            clock: Arc::clone(self),
            id,
            completed: false,
        };
        let res = rx.await;
        cancel_guard.completed = true;
        res.map_err(|_| CoordinatorCancelled)
    }

    async fn wait_for_pending_sleeps(&self, expected: usize) {
        loop {
            if self.state.lock().unwrap().pending.len() >= expected {
                return;
            }
            self.pending_notify.notified().await;
        }
    }

    async fn wait_for_cancellations(&self, expected: usize) {
        loop {
            if self.state.lock().unwrap().cancellation_count >= expected {
                return;
            }
            self.cancel_notify.notified().await;
        }
    }

    fn pending_sleep_count(&self) -> usize {
        self.state.lock().unwrap().pending.len()
    }

    fn fire_all(&self) {
        let pending = {
            let mut guard = self.state.lock().unwrap();
            std::mem::take(&mut guard.pending)
        };
        for (_, tx) in pending {
            let _ = tx.send(());
        }
    }
}

struct FetchGateState {
    starts: usize,
    active: usize,
    peak: usize,
    released: bool,
}

#[derive(Clone)]
struct ServeFetchGate {
    state: Arc<Mutex<FetchGateState>>,
    release_notify: Arc<Notify>,
    start_notify: Arc<Notify>,
    active_notify: Arc<Notify>,
}

impl ServeFetchGate {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(FetchGateState {
                starts: 0,
                active: 0,
                peak: 0,
                released: false,
            })),
            release_notify: Arc::new(Notify::new()),
            start_notify: Arc::new(Notify::new()),
            active_notify: Arc::new(Notify::new()),
        }
    }

    async fn run<T>(&self, value: T) -> T {
        {
            let mut g = self.state.lock().unwrap();
            g.starts += 1;
            g.active += 1;
            g.peak = g.peak.max(g.active);
        }
        self.start_notify.notify_waiters();
        loop {
            if self.state.lock().unwrap().released {
                break;
            }
            self.release_notify.notified().await;
        }
        {
            let mut g = self.state.lock().unwrap();
            g.active -= 1;
        }
        self.active_notify.notify_waiters();
        value
    }

    async fn wait_for_starts(&self, expected: usize) {
        loop {
            if self.state.lock().unwrap().starts >= expected {
                return;
            }
            self.start_notify.notified().await;
        }
    }

    async fn wait_for_active(&self, expected: usize) {
        loop {
            if self.state.lock().unwrap().active == expected {
                return;
            }
            self.active_notify.notified().await;
        }
    }

    fn release_all(&self) {
        self.state.lock().unwrap().released = true;
        self.release_notify.notify_waiters();
    }

    fn start_count(&self) -> usize {
        self.state.lock().unwrap().starts
    }

    fn peak_count(&self) -> usize {
        self.state.lock().unwrap().peak
    }
}

fn make_manual_coordinator<T: Clone + Send + Sync + 'static>(
    clock: &Arc<ServeManualDeadlineClock>,
) -> CliServeOperationCoordinator<T> {
    let c1 = Arc::clone(clock);
    let c2 = Arc::clone(clock);
    CliServeOperationCoordinator::with_clock(
        Arc::new(move || c1.now()),
        Arc::new(move |deadline| {
            let c2 = Arc::clone(&c2);
            Box::pin(async move { c2.sleep(deadline).await })
        }),
    )
}

async fn wait_for_operation_count<T: Clone + Send + Sync + 'static>(
    expected: usize,
    coordinator: &CliServeOperationCoordinator<T>,
) {
    for _ in 0..2000 {
        if coordinator.snapshot().await.operation_count == expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("operation count did not reach {expected}");
}

async fn wait_for_waiter_count<T: Clone + Send + Sync + 'static>(
    expected: usize,
    coordinator: &CliServeOperationCoordinator<T>,
) {
    for _ in 0..2000 {
        if coordinator.snapshot().await.waiter_count == expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("waiter count did not reach {expected}");
}

fn seed_expired_last_good(
    response: CliLocalHttpResponse,
    key: &str,
    fingerprint: &str,
    cache: &CliServeResponseCache,
) {
    let recorded_at = (cache.wall_clock)()
        .checked_sub(jiff::SignedDuration::from_secs(61))
        .unwrap();
    let _ = cache.complete_fetch(
        response,
        &serve_cache_key(key, fingerprint),
        CachePolicy {
            ttl: 60.0,
            stale_ttl: 600.0,
        },
        recorded_at,
        true,
    );
}

// upstream: CLIServeTimeoutTests.swift:7 dashboard awaits pricing refresh while serve refreshes in background
#[test]
fn dashboard_awaits_pricing_refresh_while_serve_refreshes_in_background() {
    const { assert!(!DASHBOARD_COST_REFRESHES_PRICING_IN_BACKGROUND) };
    const { assert!(SERVE_COST_REFRESHES_PRICING_IN_BACKGROUND) };
}

// upstream: CLIServeTimeoutTests.swift:13 serve deadlines clamp once from request entry
#[test]
fn serve_deadlines_clamp_once_from_request_entry() {
    assert_eq!(clamped_serve_request_timeout(f64::MAX), 86400.0);
    assert_eq!(clamped_serve_request_timeout(1e308), 86400.0);
    assert_eq!(clamped_serve_request_timeout(-5.0), 0.0);

    let started_at = Instant::now();
    let deadline = serve_request_deadline(started_at, f64::MAX).unwrap();
    assert_eq!(
        deadline.duration_since(started_at),
        Duration::from_secs(86400)
    );
    assert_eq!(serve_request_deadline(started_at, 0.0), None);

    let request_deadline = started_at + Duration::from_secs(40);
    assert_eq!(
        serve_cost_provider_deadline(started_at, Some(30.0), Some(request_deadline)),
        Some(started_at + Duration::from_secs(30))
    );
    assert_eq!(
        serve_cost_provider_deadline(
            started_at + Duration::from_secs(20),
            Some(30.0),
            Some(request_deadline)
        ),
        Some(request_deadline)
    );
    assert_eq!(serve_cost_provider_deadline(started_at, None, None), None);
}

// upstream: CLIServeTimeoutTests.swift:41 timed out source stays owned and later requests never overlap
#[tokio::test]
async fn timed_out_source_stays_owned_and_later_requests_never_overlap() {
    let clock = ServeManualDeadlineClock::new();
    let gate = ServeFetchGate::new();
    let coordinator: CliServeOperationCoordinator<i32> = make_manual_coordinator(&clock);
    let deadline = clock.now() + Duration::from_secs(30);
    let source_cancelled = Arc::new(AtomicBool::new(false));

    let c1 = coordinator.clone();
    let g1 = gate.clone();
    let sc1 = Arc::clone(&source_cancelled);
    let first = tokio::spawn(async move {
        c1.value(
            "usage:",
            Some("config-a"),
            Some(deadline),
            -1,
            |v| async move { v },
            move |sink| {
                let g1 = g1.clone();
                let sc1 = Arc::clone(&sc1);
                async move {
                    let v = g1.run(1).await;
                    sc1.store(sink.cancel_token().is_cancelled(), Ordering::SeqCst);
                    v
                }
            },
        )
        .await
    });

    gate.wait_for_starts(1).await;
    clock.wait_for_pending_sleeps(1).await;
    clock.fire_all();

    assert_eq!(first.await.unwrap(), -1);
    assert_eq!(coordinator.snapshot().await.operation_count, 1);
    assert_eq!(coordinator.snapshot().await.timer_count, 0);

    let mut later = Vec::new();
    for _ in 0..4 {
        let c = coordinator.clone();
        let g = gate.clone();
        later.push(tokio::spawn(async move {
            c.value(
                "usage:",
                Some("config-a"),
                Some(deadline + Duration::from_secs(30)),
                -1,
                |v| async move { v },
                move |_sink| {
                    let g = g.clone();
                    async move { g.run(2).await }
                },
            )
            .await
        }));
    }

    wait_for_operation_count(2, &coordinator).await;
    clock.wait_for_pending_sleeps(1).await;
    clock.fire_all();
    for task in later {
        assert_eq!(task.await.unwrap(), -1);
    }
    assert_eq!(gate.start_count(), 1);
    assert_eq!(gate.peak_count(), 1);

    gate.release_all();
    gate.wait_for_active(0).await;
    wait_for_operation_count(0, &coordinator).await;
    assert!(source_cancelled.load(Ordering::SeqCst));
}

// upstream: CLIServeTimeoutTests.swift:92 late source completion feeds a queued same fingerprint successor
#[tokio::test]
async fn late_source_completion_feeds_a_queued_same_fingerprint_successor() {
    let clock = ServeManualDeadlineClock::new();
    let source = ServeFetchGate::new();
    let accept_calls = Arc::new(AtomicUsize::new(0));
    let coordinator: CliServeOperationCoordinator<i32> = make_manual_coordinator(&clock);
    let first_deadline = clock.now() + Duration::from_secs(30);

    let c1 = coordinator.clone();
    let s1 = source.clone();
    let ac1 = Arc::clone(&accept_calls);
    let first = tokio::spawn(async move {
        c1.value(
            "usage:",
            Some("config-a"),
            Some(first_deadline),
            -1,
            move |v| {
                let ac1 = Arc::clone(&ac1);
                async move {
                    ac1.fetch_add(1, Ordering::SeqCst);
                    v
                }
            },
            move |_sink| {
                let s1 = s1.clone();
                async move { s1.run(7).await }
            },
        )
        .await
    });

    source.wait_for_starts(1).await;
    clock.wait_for_pending_sleeps(1).await;
    clock.fire_all();
    assert_eq!(first.await.unwrap(), -1);

    let c2 = coordinator.clone();
    let s2 = source.clone();
    let successor = tokio::spawn(async move {
        c2.value(
            "usage:",
            Some("config-a"),
            Some(first_deadline + Duration::from_secs(30)),
            -2,
            |v| async move { v },
            move |_sink| {
                let s2 = s2.clone();
                async move { s2.run(9).await }
            },
        )
        .await
    });

    wait_for_operation_count(2, &coordinator).await;
    clock.wait_for_pending_sleeps(1).await;
    source.release_all();

    assert_eq!(successor.await.unwrap(), 7);
    assert_eq!(source.start_count(), 1);
    assert_eq!(accept_calls.load(Ordering::SeqCst), 1);
    clock.wait_for_cancellations(1).await;
    wait_for_operation_count(0, &coordinator).await;
}

// upstream: CLIServeTimeoutTests.swift:133 late source completion promotes a different fingerprint successor
#[tokio::test]
async fn late_source_completion_promotes_a_different_fingerprint_successor() {
    let clock = ServeManualDeadlineClock::new();
    let source = ServeFetchGate::new();
    let accept_calls = Arc::new(AtomicUsize::new(0));
    let coordinator: CliServeOperationCoordinator<i32> = make_manual_coordinator(&clock);
    let first_deadline = clock.now() + Duration::from_secs(30);

    let c1 = coordinator.clone();
    let s1 = source.clone();
    let ac1 = Arc::clone(&accept_calls);
    let first = tokio::spawn(async move {
        c1.value(
            "usage:",
            Some("config-a"),
            Some(first_deadline),
            -1,
            move |v| {
                let ac1 = Arc::clone(&ac1);
                async move {
                    ac1.fetch_add(1, Ordering::SeqCst);
                    v
                }
            },
            move |_sink| {
                let s1 = s1.clone();
                async move { s1.run(7).await }
            },
        )
        .await
    });

    source.wait_for_starts(1).await;
    clock.wait_for_pending_sleeps(1).await;
    clock.fire_all();
    assert_eq!(first.await.unwrap(), -1);

    let c2 = coordinator.clone();
    let s2 = source.clone();
    let successor = tokio::spawn(async move {
        c2.value(
            "usage:",
            Some("config-b"),
            Some(first_deadline + Duration::from_secs(30)),
            -2,
            |v| async move { v },
            move |_sink| {
                let s2 = s2.clone();
                async move { s2.run(9).await }
            },
        )
        .await
    });

    wait_for_operation_count(2, &coordinator).await;
    source.release_all();

    assert_eq!(successor.await.unwrap(), 9);
    assert_eq!(source.start_count(), 2);
    assert_eq!(source.peak_count(), 1);
    assert_eq!(accept_calls.load(Ordering::SeqCst), 1);
    wait_for_operation_count(0, &coordinator).await;
}

// upstream: CLIServeTimeoutTests.swift:173 late acceptance after shutdown resumes nobody and releases the slot
#[tokio::test]
async fn late_acceptance_after_shutdown_resumes_nobody_and_releases_the_slot() {
    let clock = ServeManualDeadlineClock::new();
    let source = ServeFetchGate::new();
    let acceptance = ServeFetchGate::new();
    let coordinator: CliServeOperationCoordinator<i32> = make_manual_coordinator(&clock);
    let first_deadline = clock.now() + Duration::from_secs(30);

    let c1 = coordinator.clone();
    let s1 = source.clone();
    let a1 = acceptance.clone();
    let first = tokio::spawn(async move {
        c1.value(
            "usage:",
            Some("config-a"),
            Some(first_deadline),
            -1,
            move |v| {
                let a1 = a1.clone();
                async move { a1.run(v).await }
            },
            move |_sink| {
                let s1 = s1.clone();
                async move { s1.run(7).await }
            },
        )
        .await
    });

    source.wait_for_starts(1).await;
    clock.wait_for_pending_sleeps(1).await;
    clock.fire_all();
    assert_eq!(first.await.unwrap(), -1);

    let c2 = coordinator.clone();
    let s2 = source.clone();
    let successor = tokio::spawn(async move {
        c2.value(
            "usage:",
            Some("config-a"),
            Some(first_deadline + Duration::from_secs(30)),
            -2,
            |v| async move { v },
            move |_sink| {
                let s2 = s2.clone();
                async move { s2.run(9).await }
            },
        )
        .await
    });

    wait_for_operation_count(2, &coordinator).await;
    source.release_all();
    acceptance.wait_for_starts(1).await;
    coordinator.shutdown().await;

    assert_eq!(successor.await.unwrap(), -2);
    assert_eq!(source.start_count(), 1);
    acceptance.release_all();
    wait_for_operation_count(0, &coordinator).await;
    assert_eq!(
        coordinator.snapshot().await,
        CoordinatorSnapshot {
            operation_count: 0,
            waiter_count: 0,
            timer_count: 0,
            is_shut_down: true,
        }
    );
}

// upstream: CLIServeTimeoutTests.swift:219 late response warms the cache for the next same config request
#[tokio::test]
async fn late_response_warms_the_cache_for_the_next_same_config_request() {
    let clock = ServeManualDeadlineClock::new();
    let source = ServeFetchGate::new();
    let operations: CliServeOperationCoordinator<CliServeCoordinatedResponse> =
        make_manual_coordinator(&clock);
    let cache = CliServeResponseCache::with_clock(operations.clone(), Arc::new(Timestamp::now));
    let built = CliLocalHttpResponse::json(200, "OK", br#"{"schemaVersion":1}"#.to_vec());

    let cache1 = cache.clone();
    let s1 = source.clone();
    let built1 = built.clone();
    let first = tokio::spawn(async move {
        cached_serve_response(
            "dashboard:v1:snapshot:",
            &cache1,
            60.0,
            30.0,
            "",
            false,
            move || {
                let s1 = s1.clone();
                let built1 = built1.clone();
                async move { s1.run(built1).await }
            },
        )
        .await
    });

    source.wait_for_starts(1).await;
    clock.wait_for_pending_sleeps(1).await;
    clock.fire_all();
    assert_eq!(first.await.unwrap().status_code, 504);

    source.release_all();
    wait_for_operation_count(0, &operations).await;

    let s2 = source.clone();
    let next = cached_serve_response(
        "dashboard:v1:snapshot:",
        &cache,
        60.0,
        30.0,
        "",
        false,
        move || {
            let s2 = s2.clone();
            async move {
                s2.run(CliLocalHttpResponse::json(
                    200,
                    "OK",
                    br#"{"schemaVersion":2}"#.to_vec(),
                ))
                .await
            }
        },
    )
    .await;

    assert_eq!(next.status_code, 200);
    assert_eq!(next.body, built.body);
    assert_eq!(source.start_count(), 1);
    assert_eq!(operations.snapshot().await.operation_count, 0);
}

// upstream: CLIServeTimeoutTests.swift:263 earlier follower tightens the shared absolute budget
#[tokio::test]
async fn earlier_follower_tightens_the_shared_absolute_budget() {
    let clock = ServeManualDeadlineClock::new();
    let gate = ServeFetchGate::new();
    let coordinator: CliServeOperationCoordinator<i32> = make_manual_coordinator(&clock);
    let first_deadline = clock.now() + Duration::from_secs(30);

    let c1 = coordinator.clone();
    let g1 = gate.clone();
    let first = tokio::spawn(async move {
        c1.value(
            "cost:",
            Some("config-a"),
            Some(first_deadline),
            -1,
            |v| async move { v },
            move |_sink| {
                let g1 = g1.clone();
                async move { g1.run(1).await }
            },
        )
        .await
    });

    gate.wait_for_starts(1).await;
    clock.wait_for_pending_sleeps(1).await;

    let c2 = coordinator.clone();
    let g2 = gate.clone();
    let shorter_follower = tokio::spawn(async move {
        c2.value(
            "cost:",
            Some("config-a"),
            Some(first_deadline - Duration::from_secs(1)),
            -2,
            |v| async move { v },
            move |_sink| {
                let g2 = g2.clone();
                async move { g2.run(2).await }
            },
        )
        .await
    });

    wait_for_waiter_count(2, &coordinator).await;
    clock.wait_for_cancellations(1).await;
    clock.wait_for_pending_sleeps(1).await;
    clock.advance(Duration::from_secs(29));
    clock.fire_all();

    assert_eq!(first.await.unwrap(), -1);
    assert_eq!(shorter_follower.await.unwrap(), -2);
    assert_eq!(gate.start_count(), 1);

    gate.release_all();
    gate.wait_for_active(0).await;
    wait_for_operation_count(0, &coordinator).await;
    assert_eq!(coordinator.snapshot().await.operation_count, 0);
}

// upstream: CLIServeTimeoutTests.swift:309 source completing at an overdue deadline cannot beat a delayed timer
#[tokio::test]
async fn source_completing_at_an_overdue_deadline_cannot_beat_a_delayed_timer() {
    let clock = ServeManualDeadlineClock::new();
    let gate = ServeFetchGate::new();
    let accept_calls = Arc::new(AtomicUsize::new(0));
    let coordinator: CliServeOperationCoordinator<i32> = make_manual_coordinator(&clock);

    let c1 = coordinator.clone();
    let g1 = gate.clone();
    let ac1 = Arc::clone(&accept_calls);
    let deadline = clock.now() + Duration::from_secs(30);
    let result = tokio::spawn(async move {
        c1.value(
            "usage:",
            Some("config-a"),
            Some(deadline),
            -1,
            move |v| {
                let ac1 = Arc::clone(&ac1);
                async move {
                    ac1.fetch_add(1, Ordering::SeqCst);
                    v
                }
            },
            move |_sink| {
                let g1 = g1.clone();
                async move { g1.run(7).await }
            },
        )
        .await
    });

    gate.wait_for_starts(1).await;
    clock.wait_for_pending_sleeps(1).await;
    clock.advance(Duration::from_secs(30));
    gate.release_all();

    assert_eq!(result.await.unwrap(), -1);
    assert_eq!(accept_calls.load(Ordering::SeqCst), 1);
    clock.wait_for_cancellations(1).await;
    wait_for_operation_count(0, &coordinator).await;
}

// upstream: CLIServeTimeoutTests.swift:336 shared deadline returns each waiters own timeout value
#[tokio::test]
async fn shared_deadline_returns_each_waiters_own_timeout_value() {
    let clock = ServeManualDeadlineClock::new();
    let gate = ServeFetchGate::new();
    let coordinator: CliServeOperationCoordinator<i32> = make_manual_coordinator(&clock);
    let deadline = clock.now() + Duration::from_secs(30);

    let c1 = coordinator.clone();
    let g1 = gate.clone();
    let leader = tokio::spawn(async move {
        c1.value(
            "usage:",
            Some("config-a"),
            Some(deadline),
            -1,
            |v| async move { v },
            move |_sink| {
                let g1 = g1.clone();
                async move { g1.run(1).await }
            },
        )
        .await
    });

    gate.wait_for_starts(1).await;
    clock.wait_for_pending_sleeps(1).await;

    let c2 = coordinator.clone();
    let g2 = gate.clone();
    let follower = tokio::spawn(async move {
        c2.value(
            "usage:",
            Some("config-a"),
            Some(deadline + Duration::from_secs(1)),
            -2,
            |v| async move { v },
            move |_sink| {
                let g2 = g2.clone();
                async move { g2.run(2).await }
            },
        )
        .await
    });

    wait_for_waiter_count(2, &coordinator).await;
    clock.fire_all();

    assert_eq!(leader.await.unwrap(), -1);
    assert_eq!(follower.await.unwrap(), -2);
    gate.release_all();
    gate.wait_for_active(0).await;
    wait_for_operation_count(0, &coordinator).await;
}

// upstream: CLIServeTimeoutTests.swift:375 finite follower fails closed behind deadline free source
#[tokio::test]
async fn finite_follower_fails_closed_behind_deadline_free_source() {
    let gate = ServeFetchGate::new();
    let coordinator = CliServeOperationCoordinator::<i32>::new();

    let c1 = coordinator.clone();
    let g1 = gate.clone();
    let first = tokio::spawn(async move {
        c1.value(
            "usage:",
            Some("config-a"),
            None,
            -1,
            |v| async move { v },
            move |_sink| {
                let g1 = g1.clone();
                async move { g1.run(1).await }
            },
        )
        .await
    });

    gate.wait_for_starts(1).await;

    let g2 = gate.clone();
    let follower = coordinator
        .value(
            "usage:",
            Some("config-a"),
            Some(Instant::now() + Duration::from_secs(30)),
            -2,
            |v| async move { v },
            move |_sink| {
                let g2 = g2.clone();
                async move { g2.run(2).await }
            },
        )
        .await;

    assert_eq!(follower, -2);
    assert_eq!(gate.start_count(), 1);

    gate.release_all();
    assert_eq!(first.await.unwrap(), 1);
}

// upstream: CLIServeTimeoutTests.swift:407 waiter cancellation unregisters and last waiter cancels source
#[tokio::test]
async fn waiter_cancellation_unregisters_and_last_waiter_cancels_source() {
    let clock = ServeManualDeadlineClock::new();
    let gate = ServeFetchGate::new();
    let coordinator: CliServeOperationCoordinator<i32> = make_manual_coordinator(&clock);
    let deadline = clock.now() + Duration::from_secs(30);
    let cancel_leader = TestGate::new();
    let cancel_follower = TestGate::new();
    let source_cancelled = Arc::new(AtomicBool::new(false));

    let c1 = coordinator.clone();
    let g1 = gate.clone();
    let cl = cancel_leader.clone();
    let sc1 = Arc::clone(&source_cancelled);
    let leader = tokio::spawn(async move {
        tokio::select! {
            v = c1.value(
                "usage:",
                Some("config-a"),
                Some(deadline),
                -1,
                |v| async move { v },
                move |sink| {
                    let g1 = g1.clone();
                    let sc1 = Arc::clone(&sc1);
                    async move {
                        let v = g1.run(1).await;
                        sc1.store(sink.cancel_token().is_cancelled(), Ordering::SeqCst);
                        v
                    }
                },
            ) => v,
            () = cl.wait() => -1,
        }
    });

    gate.wait_for_starts(1).await;
    clock.wait_for_pending_sleeps(1).await;

    let c2 = coordinator.clone();
    let g2 = gate.clone();
    let cf = cancel_follower.clone();
    let follower = tokio::spawn(async move {
        tokio::select! {
            v = c2.value(
                "usage:",
                Some("config-a"),
                Some(deadline + Duration::from_secs(1)),
                -2,
                |v| async move { v },
                move |_sink| {
                    let g2 = g2.clone();
                    async move { g2.run(2).await }
                },
            ) => v,
            () = cf.wait() => -2,
        }
    });

    wait_for_waiter_count(2, &coordinator).await;
    cancel_follower.open();
    assert_eq!(follower.await.unwrap(), -2);
    wait_for_waiter_count(1, &coordinator).await;
    assert_eq!(coordinator.snapshot().await.operation_count, 1);

    cancel_leader.open();
    assert_eq!(leader.await.unwrap(), -1);
    clock.wait_for_cancellations(1).await;
    let retained = coordinator.snapshot().await;
    assert_eq!(retained.operation_count, 1);
    assert_eq!(retained.waiter_count, 0);
    assert_eq!(retained.timer_count, 0);

    gate.release_all();
    gate.wait_for_active(0).await;
    wait_for_operation_count(0, &coordinator).await;
    assert!(source_cancelled.load(Ordering::SeqCst));
}

// upstream: CLIServeTimeoutTests.swift:455 source completion cancels the operation timer
#[tokio::test]
async fn source_completion_cancels_the_operation_timer() {
    let clock = ServeManualDeadlineClock::new();
    let gate = ServeFetchGate::new();
    let coordinator: CliServeOperationCoordinator<i32> = make_manual_coordinator(&clock);

    let c1 = coordinator.clone();
    let g1 = gate.clone();
    let deadline = clock.now() + Duration::from_secs(30);
    let result = tokio::spawn(async move {
        c1.value(
            "usage:",
            Some("config-a"),
            Some(deadline),
            -1,
            |v| async move { v },
            move |_sink| {
                let g1 = g1.clone();
                async move { g1.run(7).await }
            },
        )
        .await
    });

    gate.wait_for_starts(1).await;
    clock.wait_for_pending_sleeps(1).await;
    gate.release_all();

    assert_eq!(result.await.unwrap(), 7);
    clock.wait_for_cancellations(1).await;
    assert_eq!(clock.pending_sleep_count(), 0);
    assert_eq!(
        coordinator.snapshot().await,
        CoordinatorSnapshot {
            operation_count: 0,
            waiter_count: 0,
            timer_count: 0,
            is_shut_down: false,
        }
    );
}

// upstream: CLIServeTimeoutTests.swift:485 accepted value stays owned through asynchronous commit
#[tokio::test]
async fn accepted_value_stays_owned_through_asynchronous_commit() {
    let source = ServeFetchGate::new();
    let commit = ServeFetchGate::new();
    let coordinator = CliServeOperationCoordinator::<i32>::new();
    source.release_all();

    let c1 = coordinator.clone();
    let s1 = source.clone();
    let cm1 = commit.clone();
    let first = tokio::spawn(async move {
        c1.value(
            "usage:",
            Some("config-a"),
            None,
            -1,
            move |v| {
                let cm1 = cm1.clone();
                async move { cm1.run(v).await }
            },
            move |_sink| {
                let s1 = s1.clone();
                async move { s1.run(1).await }
            },
        )
        .await
    });

    commit.wait_for_starts(1).await;

    let c2 = coordinator.clone();
    let s2 = source.clone();
    let cm2 = commit.clone();
    let follower = tokio::spawn(async move {
        c2.value(
            "usage:",
            Some("config-a"),
            None,
            -2,
            move |v| {
                let cm2 = cm2.clone();
                async move { cm2.run(v).await }
            },
            move |_sink| {
                let s2 = s2.clone();
                async move { s2.run(2).await }
            },
        )
        .await
    });

    wait_for_waiter_count(2, &coordinator).await;
    assert_eq!(source.start_count(), 1);
    assert_eq!(coordinator.snapshot().await.operation_count, 1);

    commit.release_all();
    assert_eq!(first.await.unwrap(), 1);
    assert_eq!(follower.await.unwrap(), 1);
    assert_eq!(source.start_count(), 1);
    wait_for_operation_count(0, &coordinator).await;
}

// upstream: CLIServeTimeoutTests.swift:522 earlier finite follower fails closed during accepted commit
#[tokio::test]
async fn earlier_finite_follower_fails_closed_during_accepted_commit() {
    let source = ServeFetchGate::new();
    let commit = ServeFetchGate::new();
    let coordinator = CliServeOperationCoordinator::<i32>::new();
    let leader_deadline = Instant::now() + Duration::from_secs(30);
    source.release_all();

    let c1 = coordinator.clone();
    let s1 = source.clone();
    let cm1 = commit.clone();
    let leader = tokio::spawn(async move {
        c1.value(
            "usage:",
            Some("config-a"),
            Some(leader_deadline),
            -1,
            move |v| {
                let cm1 = cm1.clone();
                async move { cm1.run(v).await }
            },
            move |_sink| {
                let s1 = s1.clone();
                async move { s1.run(1).await }
            },
        )
        .await
    });

    commit.wait_for_starts(1).await;

    let s2 = source.clone();
    let follower = coordinator
        .value(
            "usage:",
            Some("config-a"),
            Some(leader_deadline - Duration::from_secs(1)),
            -2,
            |v| async move { v },
            move |_sink| {
                let s2 = s2.clone();
                async move { s2.run(2).await }
            },
        )
        .await;

    assert_eq!(follower, -2);
    assert_eq!(source.start_count(), 1);
    let accepting = coordinator.snapshot().await;
    assert_eq!(accepting.waiter_count, 1);
    assert_eq!(accepting.timer_count, 0);
    commit.release_all();
    assert_eq!(leader.await.unwrap(), 1);
    wait_for_operation_count(0, &coordinator).await;
}

// upstream: CLIServeTimeoutTests.swift:559 config change queues a nonoverlapping successor without a deadline
#[tokio::test]
async fn config_change_queues_a_nonoverlapping_successor_without_a_deadline() {
    let gate = ServeFetchGate::new();
    let coordinator = CliServeOperationCoordinator::<i32>::new();

    let c1 = coordinator.clone();
    let g1 = gate.clone();
    let old = tokio::spawn(async move {
        c1.value(
            "usage:",
            Some("config-a"),
            None,
            -1,
            |v| async move { v },
            move |_sink| {
                let g1 = g1.clone();
                async move { g1.run(1).await }
            },
        )
        .await
    });

    gate.wait_for_starts(1).await;

    let c2 = coordinator.clone();
    let g2 = gate.clone();
    let successor = tokio::spawn(async move {
        c2.value(
            "usage:",
            Some("config-b"),
            None,
            -2,
            |v| async move { v },
            move |_sink| {
                let g2 = g2.clone();
                async move { g2.run(2).await }
            },
        )
        .await
    });

    wait_for_operation_count(2, &coordinator).await;
    assert_eq!(gate.start_count(), 1);
    assert_eq!(gate.peak_count(), 1);

    gate.release_all();
    assert_eq!(old.await.unwrap(), 1);
    assert_eq!(successor.await.unwrap(), 2);
    gate.wait_for_active(0).await;
    wait_for_operation_count(0, &coordinator).await;
    assert_eq!(gate.start_count(), 2);
    assert_eq!(gate.peak_count(), 1);
}

// upstream: CLIServeTimeoutTests.swift:599 shutdown cancels owned work and rejects new operations
#[tokio::test]
async fn shutdown_cancels_owned_work_and_rejects_new_operations() {
    let clock = ServeManualDeadlineClock::new();
    let gate = ServeFetchGate::new();
    let coordinator: CliServeOperationCoordinator<i32> = make_manual_coordinator(&clock);
    let source_cancelled = Arc::new(AtomicBool::new(false));

    let c1 = coordinator.clone();
    let g1 = gate.clone();
    let sc1 = Arc::clone(&source_cancelled);
    let deadline = clock.now() + Duration::from_secs(30);
    let active = tokio::spawn(async move {
        c1.value(
            "usage:",
            Some("config-a"),
            Some(deadline),
            -1,
            |v| async move { v },
            move |sink| {
                let g1 = g1.clone();
                let sc1 = Arc::clone(&sc1);
                async move {
                    let v = g1.run(1).await;
                    sc1.store(sink.cancel_token().is_cancelled(), Ordering::SeqCst);
                    v
                }
            },
        )
        .await
    });

    gate.wait_for_starts(1).await;
    clock.wait_for_pending_sleeps(1).await;
    coordinator.shutdown().await;

    assert_eq!(active.await.unwrap(), -1);
    clock.wait_for_cancellations(1).await;

    let rejected = coordinator
        .value(
            "cost:",
            Some("config-a"),
            None,
            -2,
            |v| async move { v },
            |_sink| async { 2 },
        )
        .await;
    assert_eq!(rejected, -2);

    let retained = coordinator.snapshot().await;
    assert_eq!(retained.operation_count, 1);
    assert!(retained.is_shut_down);

    gate.release_all();
    gate.wait_for_active(0).await;
    wait_for_operation_count(0, &coordinator).await;
    assert!(source_cancelled.load(Ordering::SeqCst));
}

// upstream: CLIServeTimeoutTests.swift:639 provider timeout preserves healthy rows and cannot stack provider work
#[tokio::test]
async fn provider_timeout_preserves_healthy_rows_and_cannot_stack_provider_work() {
    let clock = ServeManualDeadlineClock::new();
    let blocked = ServeFetchGate::new();
    let healthy = ServeFetchGate::new();
    let operations: CliServeOperationCoordinator<UsageCommandOutput> =
        make_manual_coordinator(&clock);
    let deadline = clock.now() + Duration::from_secs(30);
    healthy.release_all();

    let ops1 = operations.clone();
    let b1 = blocked.clone();
    let h1 = healthy.clone();
    let first = tokio::spawn(async move {
        serve_collect_usage_outputs(
            &["claude".to_string(), "gemini".to_string()],
            "config-a",
            Some(deadline),
            &ops1,
            move |provider, _sink| {
                let b1 = b1.clone();
                let h1 = h1.clone();
                async move {
                    if provider == "claude" {
                        return b1
                            .run(UsageCommandOutput::with_sections(vec![
                                "late:claude".to_string(),
                            ]))
                            .await;
                    }
                    h1.run(UsageCommandOutput::with_sections(vec![
                        "ok:gemini".to_string(),
                    ]))
                    .await
                }
            },
        )
        .await
    });

    blocked.wait_for_starts(1).await;
    healthy.wait_for_starts(1).await;
    wait_for_operation_count(1, &operations).await;
    clock.wait_for_pending_sleeps(1).await;
    clock.advance(Duration::from_secs(30));
    clock.fire_all();

    let first_output = first.await.unwrap();
    assert_eq!(first_output.sections, vec!["ok:gemini"]);
    assert_eq!(first_output.payload.len(), 1);
    assert_eq!(first_output.payload[0].provider, "claude");
    assert_eq!(
        first_output.payload[0]
            .error
            .as_ref()
            .unwrap()
            .kind
            .as_deref(),
        Some("provider")
    );

    let ops2 = operations.clone();
    let b2 = blocked.clone();
    let second = tokio::spawn(async move {
        serve_collect_usage_outputs(
            &["claude".to_string()],
            "config-a",
            Some(deadline + Duration::from_secs(30)),
            &ops2,
            move |_provider, _sink| {
                let b2 = b2.clone();
                async move {
                    b2.run(UsageCommandOutput::with_sections(vec![
                        "overlap".to_string(),
                    ]))
                    .await
                }
            },
        )
        .await
    });

    wait_for_operation_count(2, &operations).await;
    clock.wait_for_pending_sleeps(1).await;
    clock.advance(Duration::from_secs(30));
    clock.fire_all();

    let second_output = second.await.unwrap();
    assert_eq!(
        second_output.payload[0]
            .error
            .as_ref()
            .unwrap()
            .kind
            .as_deref(),
        Some("provider")
    );
    assert_eq!(blocked.start_count(), 1);
    assert_eq!(blocked.peak_count(), 1);

    blocked.release_all();
    blocked.wait_for_active(0).await;
    wait_for_operation_count(0, &operations).await;
}

// upstream: CLIServeTimeoutTests.swift:697 cost route variants cannot stack the same provider scan
#[tokio::test]
async fn cost_route_variants_cannot_stack_the_same_provider_scan() {
    let clock = ServeManualDeadlineClock::new();
    let late = CostPayload::from_token_snapshot("claude", None, None);
    let blocked = ServeFetchGate::new();
    let operations: CliServeOperationCoordinator<CostPayload> = make_manual_coordinator(&clock);
    let request_deadline = clock.now() + Duration::from_secs(40);
    let c_now = Arc::clone(&clock);
    let first_context = ServeCostCollectionContext {
        config_fingerprint: "config-a".to_string(),
        provider_timeout: Some(30.0),
        request_deadline: Some(request_deadline),
        now: Arc::new(move || c_now.now()),
        provider_operations: operations.clone(),
    };

    let ctx1 = first_context.clone();
    let b1 = blocked.clone();
    let late1 = late.clone();
    let first = tokio::spawn(async move {
        serve_collect_cost_payloads(
            &["claude".to_string(), "codex".to_string()],
            &ctx1,
            move |provider, _sink| {
                let b1 = b1.clone();
                let late1 = late1.clone();
                async move {
                    if provider == "claude" {
                        return b1.run(late1).await;
                    }
                    CostPayload::from_token_snapshot(provider, None, None)
                }
            },
        )
        .await
    });

    blocked.wait_for_starts(1).await;
    clock.wait_for_pending_sleeps(1).await;
    clock.advance(Duration::from_secs(30));
    clock.fire_all();
    let first_payload = first.await.unwrap();

    assert_eq!(
        first_payload
            .iter()
            .map(|p| p.provider.as_str())
            .collect::<Vec<_>>(),
        vec!["claude", "codex"]
    );
    assert_eq!(
        first_payload[0].error.as_ref().unwrap().message,
        "claude cost refresh timed out"
    );
    assert!(first_payload[1].error.is_none());

    let c_now2 = Arc::clone(&clock);
    let overlapping_context = ServeCostCollectionContext {
        config_fingerprint: "config-a".to_string(),
        provider_timeout: Some(30.0),
        request_deadline: Some(request_deadline + Duration::from_secs(20)),
        now: Arc::new(move || c_now2.now()),
        provider_operations: operations.clone(),
    };
    let b2 = blocked.clone();
    let late2 = late.clone();
    let overlapping_variant = tokio::spawn(async move {
        serve_collect_cost_payloads(
            &["claude".to_string()],
            &overlapping_context,
            move |_provider, _sink| {
                let b2 = b2.clone();
                let late2 = late2.clone();
                async move { b2.run(late2).await }
            },
        )
        .await
    });

    wait_for_operation_count(2, &operations).await;
    clock.wait_for_pending_sleeps(1).await;
    clock.advance(Duration::from_secs(30));
    clock.fire_all();
    let second_payload = overlapping_variant.await.unwrap();

    assert_eq!(
        second_payload[0].error.as_ref().unwrap().message,
        "claude cost refresh timed out"
    );
    assert_eq!(blocked.start_count(), 1);
    assert_eq!(blocked.peak_count(), 1);

    blocked.release_all();
    blocked.wait_for_active(0).await;
    wait_for_operation_count(0, &operations).await;
}

// upstream: CLIServeTimeoutTests.swift:812 expired response returns stale immediately while background refresh commits
#[tokio::test]
async fn expired_response_returns_stale_immediately_while_background_refresh_commits() {
    let clock = ServeManualDeadlineClock::new();
    let source = ServeFetchGate::new();
    let operations: CliServeOperationCoordinator<CliServeCoordinatedResponse> =
        make_manual_coordinator(&clock);
    let cache = CliServeResponseCache::with_clock(operations.clone(), Arc::new(Timestamp::now));
    let old = CliLocalHttpResponse::json(200, "OK", br#"{"value":"old"}"#.to_vec());
    let new = CliLocalHttpResponse::json(200, "OK", br#"{"value":"new"}"#.to_vec());
    seed_expired_last_good(old.clone(), "dashboard:default", "config-a", &cache);

    let s1 = source.clone();
    let new1 = new.clone();
    let returned = cached_serve_response(
        "dashboard:default",
        &cache,
        60.0,
        30.0,
        "config-a",
        true,
        move || {
            let s1 = s1.clone();
            let new1 = new1.clone();
            async move { s1.run(new1).await }
        },
    )
    .await;

    assert_eq!(returned.body, old.body);
    source.wait_for_starts(1).await;
    assert_eq!(source.start_count(), 1);
    source.release_all();
    wait_for_operation_count(0, &operations).await;

    let s2 = source.clone();
    let old2 = old.clone();
    let refreshed = cached_serve_response(
        "dashboard:default",
        &cache,
        60.0,
        30.0,
        "config-a",
        false,
        move || {
            let s2 = s2.clone();
            let old2 = old2.clone();
            async move { s2.run(old2).await }
        },
    )
    .await;
    assert_eq!(refreshed.body, new.body);
    assert_eq!(source.start_count(), 1);
}

// upstream: CLIServeTimeoutTests.swift:852 concurrent stale hits coalesce into one background rebuild
#[tokio::test]
async fn concurrent_stale_hits_coalesce_into_one_background_rebuild() {
    let clock = ServeManualDeadlineClock::new();
    let source = ServeFetchGate::new();
    let operations: CliServeOperationCoordinator<CliServeCoordinatedResponse> =
        make_manual_coordinator(&clock);
    let cache = CliServeResponseCache::with_clock(operations.clone(), Arc::new(Timestamp::now));
    let old = CliLocalHttpResponse::json(200, "OK", br#"{"value":"old"}"#.to_vec());
    let new = CliLocalHttpResponse::json(200, "OK", br#"{"value":"new"}"#.to_vec());
    seed_expired_last_good(old.clone(), "dashboard:default", "config-a", &cache);

    let mut handles = Vec::new();
    for _ in 0..2 {
        let cache = cache.clone();
        let s = source.clone();
        let new = new.clone();
        handles.push(tokio::spawn(async move {
            cached_serve_response(
                "dashboard:default",
                &cache,
                60.0,
                30.0,
                "config-a",
                true,
                move || {
                    let s = s.clone();
                    let new = new.clone();
                    async move { s.run(new).await }
                },
            )
            .await
        }));
    }

    let mut responses = Vec::new();
    for h in handles {
        responses.push(h.await.unwrap());
    }

    assert!(responses.iter().all(|r| r.body == old.body));
    source.wait_for_starts(1).await;
    assert_eq!(source.start_count(), 1);
    source.release_all();
    wait_for_operation_count(0, &operations).await;
    assert_eq!(source.start_count(), 1);
}

// upstream: CLIServeTimeoutTests.swift:892 refresh interval zero blocks instead of serving stale
#[tokio::test]
async fn refresh_interval_zero_blocks_instead_of_serving_stale() {
    let clock = ServeManualDeadlineClock::new();
    let source = ServeFetchGate::new();
    let finished = Arc::new(AtomicBool::new(false));
    let operations: CliServeOperationCoordinator<CliServeCoordinatedResponse> =
        make_manual_coordinator(&clock);
    let cache = CliServeResponseCache::with_clock(operations.clone(), Arc::new(Timestamp::now));
    let old = CliLocalHttpResponse::json(200, "OK", br#"{"value":"old"}"#.to_vec());
    let new = CliLocalHttpResponse::json(200, "OK", br#"{"value":"new"}"#.to_vec());
    seed_expired_last_good(old, "dashboard:default", "config-a", &cache);

    let cache1 = cache.clone();
    let s1 = source.clone();
    let new1 = new.clone();
    let fin1 = Arc::clone(&finished);
    let request = tokio::spawn(async move {
        let resp = cached_serve_response(
            "dashboard:default",
            &cache1,
            0.0,
            30.0,
            "config-a",
            true,
            move || {
                let s1 = s1.clone();
                let new1 = new1.clone();
                async move { s1.run(new1).await }
            },
        )
        .await;
        fin1.store(true, Ordering::SeqCst);
        resp
    });

    source.wait_for_starts(1).await;
    assert!(!finished.load(Ordering::SeqCst));
    source.release_all();

    assert_eq!(request.await.unwrap().body, new.body);
    assert_eq!(source.start_count(), 1);
}

// upstream: CLIServeTimeoutTests.swift:925 config fingerprint change blocks instead of serving another keys stale response
#[tokio::test]
async fn config_fingerprint_change_blocks_instead_of_serving_another_keys_stale_response() {
    let clock = ServeManualDeadlineClock::new();
    let source = ServeFetchGate::new();
    let finished = Arc::new(AtomicBool::new(false));
    let operations: CliServeOperationCoordinator<CliServeCoordinatedResponse> =
        make_manual_coordinator(&clock);
    let cache = CliServeResponseCache::with_clock(operations.clone(), Arc::new(Timestamp::now));
    let old = CliLocalHttpResponse::json(200, "OK", br#"{"config":"a"}"#.to_vec());
    let new = CliLocalHttpResponse::json(200, "OK", br#"{"config":"b"}"#.to_vec());
    seed_expired_last_good(old, "dashboard:default", "config-a", &cache);

    let cache1 = cache.clone();
    let s1 = source.clone();
    let new1 = new.clone();
    let fin1 = Arc::clone(&finished);
    let request = tokio::spawn(async move {
        let resp = cached_serve_response(
            "dashboard:default",
            &cache1,
            60.0,
            30.0,
            "config-b",
            true,
            move || {
                let s1 = s1.clone();
                let new1 = new1.clone();
                async move { s1.run(new1).await }
            },
        )
        .await;
        fin1.store(true, Ordering::SeqCst);
        resp
    });

    source.wait_for_starts(1).await;
    assert!(!finished.load(Ordering::SeqCst));
    source.release_all();

    assert_eq!(request.await.unwrap().body, new.body);
    assert_eq!(source.start_count(), 1);
}

// ============================================================================
// 7. CLIServeWebUITests.swift (9 non-JSC tests; 13 JSC DOM tests + 2 proof-export tests documented in porting.md)
// ============================================================================

fn web_ui_html() -> String {
    String::from_utf8(web_ui::response().body).unwrap()
}

// upstream: CLIServeWebUITests.swift:118 usage display control is labeled and bundled without external resources
#[test]
fn usage_display_control_is_labeled_and_bundled_without_external_resources() {
    let html = web_ui_html();
    assert!(html.contains(r#"<label for="usage-display">Usage display</label>"#));
    assert!(html.contains(r#"<option value="server">Follow server</option>"#));
    assert!(html.contains(r#"<option value="used">Used</option>"#));
    assert!(html.contains(r#"<option value="remaining">Remaining</option>"#));
    assert!(!html.contains("<script src="));
    assert!(!html.contains(r#"rel="stylesheet""#));
}

// upstream: CLIServeWebUITests.swift:439 web ui renders account cards in titled groups for multi account providers
#[test]
fn web_ui_renders_account_cards_in_titled_groups_for_multi_account_providers() {
    let html = web_ui_html();
    assert!(html.contains("function renderAccountCard(provider, account)"));
    assert!(html.contains("provider.accountsError"));
    assert!(html.contains("group-title"));
}

// partial-upstream: CLIServeWebUITests.swift:488 web ui embeds provider icon urls and serves embedded svgs
#[test]
fn web_ui_embeds_provider_icon_urls_and_serves_embedded_svgs() {
    let html = web_ui_html();
    assert!(!html.contains("__PROVIDER_ICON_URLS__"));
    assert!(html.contains("/icons/ProviderIcon-claude.svg"));
    assert!(web_ui::icon_response("ProviderIcon-claude").is_some());
    assert!(web_ui::icon_response("ProviderIcon-nonexistent").is_none());
    assert!(web_ui::icon_response("../etc/passwd").is_none());
    assert_eq!(PROVIDER_ICON_BY_PROVIDER_ID.len(), 3);
    assert!(provider_icons::svg_data("ProviderIcon-claude").is_some());
    assert!(provider_icons::svg_data("ProviderIcon-codex").is_some());
    assert!(provider_icons::svg_data("ProviderIcon-antigravity").is_some());
    assert!(provider_icons::svg_data("ProviderIcon-cursor").is_none());
}

// upstream: CLIServeWebUITests.swift:499 web ui renders account windows alongside an error note
#[test]
fn web_ui_renders_account_windows_alongside_an_error_note() {
    let html = web_ui_html();
    let error_append = "card.append(node(\"p\", \"error-message\", account.error));";
    assert!(html.contains(error_append));
    assert!(!html.contains(&format!("{error_append}\n            return card;")));
    assert!(html.contains(
        "for (const window of visibleWindows(account.windows)) windows.append(renderWindow(window))"
    ));
}

// upstream: CLIServeWebUITests.swift:509 web ui skips windows the snapshot marks idle
#[test]
fn web_ui_skips_windows_the_snapshot_marks_idle() {
    let html = web_ui_html();
    assert!(html.contains("function visibleWindows(windows)"));
    assert!(html.contains("w.idle !== true"));
    assert!(html.contains("for (const window of visibleWindows(provider.windows))"));
    assert!(html.contains("for (const window of visibleWindows(account.windows))"));
    assert!(html.contains("worstWindowLevel(visibleWindows(account.windows))"));
}

// upstream: CLIServeWebUITests.swift:521 web ui keeps ambient windows when no accounts are present
#[test]
fn web_ui_keeps_ambient_windows_when_no_accounts_are_present() {
    let html = web_ui_html();
    assert!(html.contains("Array.isArray(provider.accounts)"));
    assert!(html.contains("renderWindow(window)"));
}

// upstream: CLIServeWebUITests.swift:528 web ui renders daily spend charts from cost history
#[test]
fn web_ui_renders_daily_spend_charts_from_cost_history() {
    let html = web_ui_html();
    assert!(html.contains("function renderCostChart(history)"));
    assert!(html.contains("refreshCostHistory(headers)"));
    assert!(html.contains("state.costHistories[provider.id]"));
    assert!(html.contains("fetch(\"/cost\""));
}

// partial-upstream: CLIServeWebUITests.swift:540 web ui progressively paints cached shell and provider snapshots
#[test]
fn web_ui_progressively_paints_cached_shell_and_provider_snapshots() {
    let html = web_ui_html();
    assert!(html.contains("ariadusage.lastSnapshot"));
    assert!(html.contains("/dashboard/v1/snapshot?detail=shell"));
    assert!(html.contains("card pending"));
    assert!(html.contains("Promise.allSettled"));
    assert!(html.contains("encodeURIComponent(provider.id)"));
}

// upstream: CLIServeWebUITests.swift:550 serve identity flag decodes like the dashboard command
#[test]
fn serve_identity_flag_decodes_like_the_dashboard_command() {
    assert_eq!(
        decode_dashboard_identity_mode(None),
        Some(DashboardIdentityMode::Full)
    );
    assert_eq!(
        decode_dashboard_identity_mode(Some("redacted")),
        Some(DashboardIdentityMode::Redacted)
    );
    assert_eq!(
        decode_dashboard_identity_mode(Some("full")),
        Some(DashboardIdentityMode::Full)
    );
    assert_eq!(decode_dashboard_identity_mode(Some("nope")), None);
}

// ============================================================================
// 8. CLIServeRequestDeadlineLinuxTests.swift (1 test)
// ============================================================================

// upstream: CLIServeRequestDeadlineLinuxTests.swift:119 trickling clients are rejected before their request headers finish
#[tokio::test]
async fn trickling_clients_are_rejected_before_their_request_headers_finish() {
    let connection_cap = 3;
    let server = CliLocalHttpServer::bind("127.0.0.1", 0, 5_000, 300, connection_cap)
        .await
        .unwrap()
        .with_allowed_hosts(CliLocalHttpAllowedHosts::LoopbackOnly);
    let port = server.port;
    let (tx, rx) = watch::channel(false);
    let server_task = tokio::spawn(async move {
        server
            .serve(
                rx,
                Arc::new(|_req| {
                    Box::pin(async {
                        CliLocalHttpResponse::json(200, "OK", br#"{"ok":true}"#.to_vec())
                    })
                }),
            )
            .await;
    });

    let mut client_tasks = Vec::new();
    for _ in 0..connection_cap {
        client_tasks.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            stream
                .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Pad: ")
                .await
                .unwrap();
            let mut buf = Vec::new();
            loop {
                tokio::select! {
                    res = stream.read_to_end(&mut buf) => {
                        let _ = res;
                        break;
                    }
                    () = tokio::time::sleep(Duration::from_millis(60)) => {
                        if stream.write_all(b"a").await.is_err() {
                            let _ = stream.read_to_end(&mut buf).await;
                            break;
                        }
                    }
                }
            }
            String::from_utf8(buf).unwrap()
        }));
    }

    for client in client_tasks {
        let response = client.await.unwrap();
        assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"));
        assert!(response.ends_with(r#"{"error":"invalid request"}"#));
    }

    let healthy_raw =
        send_raw_request(port, "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").await;
    assert!(healthy_raw.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(healthy_raw.ends_with(r#"{"ok":true}"#));

    let _ = tx.send(true);
    server_task.await.unwrap();
}
