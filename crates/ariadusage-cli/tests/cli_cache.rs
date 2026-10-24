// Ported from CodexBar Tests/CodexBarTests/CLICacheTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::process::Command;
use std::sync::Arc;

use ariadusage_cli::{
    CacheClearOptions, CacheClearResult, OutputFormat, cache_clear_provider_scope_error,
    cache_help, execute_cache_clear,
};
use ariadusage_core::config::Config;
use ariadusage_engine::brokers::cookie_cache::{CacheScope, CookieCache};
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::build_default_secret_store;
use ariadusage_engine::config_store::ConfigStore;
use ariadusage_engine::cost::store::CostUsageStore;
use ariadusage_engine::registry::ProviderRegistry;
use ariadusage_engine::runtime::EngineRuntime;
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::ids::ActionId;
use assert_cmd::cargo::cargo_bin;
use jiff::Timestamp;

// upstream: CLICacheTests.swift:6 cache clear parses cookies provider flags
#[test]
fn cache_clear_parses_cookies_provider_flags() {
    let parsed =
        CacheClearOptions::parse_from_slice(&["--cookies", "--provider", "claude", "--json"])
            .expect("parse cache clear flags");

    assert!(parsed.cookies);
    assert!(parsed.json_shortcut);
    assert_eq!(parsed.provider.as_deref(), Some("claude"));
    assert_eq!(parsed.effective_format(), OutputFormat::Json);
}

// upstream: CLICacheTests.swift:17 provider scope is rejected for cost clearing
#[test]
fn provider_scope_is_rejected_for_cost_clearing() {
    assert_eq!(cache_clear_provider_scope_error(None, true), None);
    assert_eq!(
        cache_clear_provider_scope_error(Some("claude"), false),
        None
    );
    assert!(
        cache_clear_provider_scope_error(Some("claude"), true)
            .is_some_and(|err| err.contains("--provider only scopes cookie caches"))
    );
}

// upstream: CLICacheTests.swift:25 cache help documents provider as cookie scoped
#[test]
fn cache_help_documents_provider_as_cookie_scoped() {
    let help = cache_help("0.0.0");

    assert!(help.contains("--provider with --cookies"));
    assert!(help.contains("ariadusage cache clear --cookies --provider claude"));
}

// Rust-added: verifies IPC clearCostCache and CLI cache clear --cost against CostUsageStore
#[test]
fn cache_clear_removes_cookie_entries_and_cost_cache_files() {
    let cache = CookieCache::new();
    let claude = ProviderId::new("claude").expect("claude id");
    let account_scope = CacheScope::ManagedAccount("acct-1".to_owned());

    cache.store(
        &claude,
        None,
        "sessionKey=test",
        "Chrome",
        Timestamp::UNIX_EPOCH,
    );
    cache.store(
        &claude,
        Some(&account_scope),
        "sessionKey=test-acct",
        "Chrome",
        Timestamp::UNIX_EPOCH,
    );

    let summary = cache.clear_all_scopes_detailed(&claude);
    assert_eq!(summary.cleared_count, 2);
    assert_eq!(summary.failed_count, 0);
    assert!(cache.load(&claude, None).is_none());
    assert!(cache.load(&claude, Some(&account_scope)).is_none());

    let opts =
        CacheClearOptions::parse_from_slice(&["--cookies", "--provider", "claude", "--json"])
            .expect("parse options");
    let (results, rendered) = execute_cache_clear(&opts, None).expect("execute cache clear");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].cache, "cookies");
    assert_eq!(results[0].provider.as_deref(), Some("claude"));
    assert!(rendered.contains("\"cache\":\"cookies\""));

    let temp = tempfile::tempdir().expect("tempdir");
    let xdg_cache_home = temp.path().join("xdg-cache");
    let cache_root = xdg_cache_home.join("ariadusage");
    let mut store = CostUsageStore::new(Some(&cache_root));
    store.configuration().expect("materialize cost store");
    assert!(store.database_path().exists());

    let sqlite_copies_dir = cache_root.join("codex-sqlite-copies");
    std::fs::create_dir_all(&sqlite_copies_dir).expect("create codex-sqlite-copies");
    let copy_marker = sqlite_copies_dir.join("copy.sqlite");
    std::fs::write(&copy_marker, b"keep").expect("write copy marker");

    let config_path = temp.path().join("config.json");
    let secret_store =
        build_default_secret_store(&Config::new(Config::CURRENT_VERSION, Vec::new()), true)
            .expect("secret store");
    let runtime = EngineRuntime::new(
        ConfigStore::new(config_path.clone()),
        secret_store,
        Arc::new(NetBroker::new(NetConfig::default()).expect("net broker")),
        ProviderRegistry::empty(),
    )
    .with_cost_cache_root(&cache_root);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let payload = rt
        .block_on(
            runtime.run_action_with_payload(&ActionId::new("clearCostCache").expect("id"), None),
        )
        .expect("run clearCostCache")
        .expect("clearCostCache payload");
    assert_eq!(payload["cleared"], 1);
    assert!(payload["error"].is_null());
    assert!(!cache_root.join("cost-usage").exists());
    assert!(copy_marker.exists());

    let (second_cleared, second_err) = runtime.clear_cost_cache();
    assert_eq!(second_cleared, 0);
    assert!(second_err.is_none());

    let mut store = CostUsageStore::new(Some(&cache_root));
    store.configuration().expect("re-materialize cost store");
    assert!(store.database_path().exists());

    let runtime_dir = temp.path().join("runtime");
    std::fs::create_dir_all(&runtime_dir).expect("create runtime dir");
    let output = Command::new(cargo_bin("ariadusage"))
        .args(["cache", "clear", "--cost", "--json"])
        .env_clear()
        .env("HOME", temp.path())
        .env("CFFIXED_USER_HOME", temp.path())
        .env("XDG_CACHE_HOME", &xdg_cache_home)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("ARIADUSAGE_CONFIG", &config_path)
        .env("ARIADUSAGE_DISABLE_KEYRING", "1")
        .output()
        .expect("run ariadusage cache clear --cost --json");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli_results: Vec<CacheClearResult> =
        serde_json::from_slice(&output.stdout).expect("decode CacheClearResult");
    assert_eq!(cli_results.len(), 1);
    assert_eq!(cli_results[0].cache, "cost");
    assert_eq!(cli_results[0].cleared, 1);
    assert_eq!(cli_results[0].error, None);
    assert!(!cache_root.join("cost-usage").exists());
    assert!(copy_marker.exists());
}
