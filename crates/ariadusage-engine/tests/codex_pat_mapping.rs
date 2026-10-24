// Ported from CodexBar Tests/CodexBarTests/CodexPATTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexPATAppPublicationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::{
    pipeline::*,
    projection::{ProjectionInput, RefreshState, provider_snapshot},
    providers::SourceMode,
};
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::providers::codex::*;
use ariadusage_protocol::{ProviderId, metric::SourceKind};
use jiff::Timestamp;
use std::{collections::BTreeMap, fs, sync::Arc, time::Duration};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

#[path = "support/https.rs"]
mod https;

struct NoRetry;
impl Sleeper for NoRetry {
    fn sleep<'a>(&'a self, _: Duration) -> BoxFuture<'a, ()> {
        Box::pin(async { panic!("No retry expected") })
    }
}

fn context(mode: SourceMode) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("codex").unwrap(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "fixture".into(),
        source_mode: mode,
        env: BTreeMap::new(),
        include_credits: true,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    }
}

// upstream: CodexPATTests.swift:11 parses personal access token credentials
#[test]
fn parses_personal_access_token_credentials() {
    let json = br#"{
      "OPENAI_API_KEY": null,
      "personal_access_token": "at-test-token"
    }"#;
    let credentials =
        CodexOAuthCredentialsStore::parse_pat(json, CodexOAuthCredentialSource::CodexHome).unwrap();
    assert_eq!(credentials.token_str(), "at-test-token");
    assert_eq!(credentials.source, CodexOAuthCredentialSource::CodexHome);
}

// upstream: CodexPATTests.swift:24 OAuth parse ignores a PAT-only auth file
#[test]
fn oauth_parse_ignores_a_pat_only_auth_file() {
    assert_eq!(
        CodexOAuthCredentialsStore::parse(
            br#"{"OPENAI_API_KEY":null,"personal_access_token":"at-test-token"}"#,
            CodexOAuthCredentialSource::CodexHome
        )
        .unwrap_err(),
        CodexOAuthCredentialsError::MissingTokens
    );
}

// upstream: CodexPATTests.swift:41 PAT parse does not fall through to OAuth tokens
#[test]
fn pat_parse_does_not_fall_through_to_oauth_tokens() {
    let json = br#"{
      "personal_access_token": "at-preferred",
      "OPENAI_API_KEY": "sk-test",
      "tokens": {
        "access_token": "oauth-access",
        "refresh_token": "oauth-refresh"
      }
    }"#;
    let pat =
        CodexOAuthCredentialsStore::parse_pat(json, CodexOAuthCredentialSource::CodexHome).unwrap();
    let oauth =
        CodexOAuthCredentialsStore::parse(json, CodexOAuthCredentialSource::CodexHome).unwrap();
    assert_eq!(pat.token_str(), "at-preferred");
    assert_eq!(oauth.access_token_str(), "sk-test");
    assert!(oauth.is_api_key);
}

// upstream: CodexPATTests.swift:60 blank personal access token is missing
#[test]
fn blank_personal_access_token_is_missing() {
    let json = br#"{
      "personal_access_token": "  "
    }"#;
    assert_eq!(
        CodexOAuthCredentialsStore::parse_pat(json, CodexOAuthCredentialSource::CodexHome)
            .unwrap_err(),
        CodexOAuthCredentialsError::MissingTokens
    );
}

// upstream: CodexPATTests.swift:76 auto usage strategy prefers PAT over OAuth
#[test]
fn auto_usage_strategy_prefers_pat_over_oauth() {
    for (selected, has_pat, expected) in [
        (CodexUsageDataSource::Auto, true, CodexUsageDataSource::Pat),
        (
            CodexUsageDataSource::Auto,
            false,
            CodexUsageDataSource::OAuth,
        ),
        (CodexUsageDataSource::Pat, false, CodexUsageDataSource::Pat),
    ] {
        assert_eq!(
            resolve_usage_strategy(selected, true, has_pat).data_source,
            expected
        );
    }
}

// upstream: CodexPATTests.swift:95 User-Agent uses the Settings Codex version without a hardcoded fallback
#[test]
fn user_agent_uses_the_settings_codex_version_without_a_hardcoded_fallback() {
    assert_eq!(
        CodexPATUsageFetcher::normalized_cli_version_for_testing(Some("codex-cli 0.148.0-alpha.9"))
            .as_deref(),
        Some("0.148.0-alpha.9")
    );
    assert_eq!(
        CodexPATUsageFetcher::normalized_cli_version_for_testing(Some("1.2.3")).as_deref(),
        Some("1.2.3")
    );
    assert_eq!(
        CodexPATUsageFetcher::normalized_cli_version_for_testing(Some("  ")),
        None
    );

    let user_agent =
        CodexPATUsageFetcher::user_agent_for_testing(Some("codex-cli 0.148.0-alpha.9"));
    assert!(user_agent.starts_with("codex_cli_rs/0.148.0-alpha.9 ("));
    assert!(user_agent.contains("; "));
    assert!(!user_agent.contains("codex-cli"));
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    assert!(!user_agent.contains("0.0.0"));
    assert_eq!(
        CodexCLIUserAgent::parse_os_version_from_system_version_plist(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>ProductName</key>
    <string>macOS</string>
    <key>ProductVersion</key>
    <string>15.4</string>
</dict>
</plist>"#
        )
        .as_deref(),
        Some("15.4.0")
    );
    assert_eq!(
        CodexCLIUserAgent::format_three_part_os_version("6.12.10-arch1-1").as_deref(),
        Some("6.12.10")
    );
}

// upstream: CodexPATTests.swift:110 fetch context Settings version is the primary PAT User-Agent source
#[test]
fn fetch_context_settings_version_is_the_primary_pat_user_agent_source() {
    let home = TempDir::new().unwrap();
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let mut cfg = CodexStrategyConfig::new(net, BTreeMap::new(), home.path(), home.path());
    cfg.resolved_cli_version = Some("codex-cli 9.9.9-settings".to_owned());
    assert_eq!(
        CodexPATFetchStrategy::resolved_cli_version_for_testing(&cfg).as_deref(),
        Some("codex-cli 9.9.9-settings")
    );
    assert_eq!(
        CodexPATFetchStrategy::with_config(cfg)
            .resolved_cli_version()
            .as_deref(),
        Some("codex-cli 9.9.9-settings")
    );
}

// upstream: CodexPATTests.swift:131 PAT whoami then usage send CLI User-Agent and skip OAuth extras
#[tokio::test]
async fn pat_whoami_then_usage_send_cli_user_agent_and_skip_oauth_extras() {
    let server = https::start_server();
    let expected_ua = CodexCLIUserAgent::make(Some("1.2.3"));
    assert!(expected_ua.starts_with("codex_cli_rs/1.2.3 ("));

    let whoami_mock = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/accounts/v1/user-auth-credential/whoami")
                .header("authorization", "Bearer at-test-token")
                .header("originator", "codex_cli_rs")
                .header("user-agent", &expected_ua)
                .header("accept", "application/json")
                .header_missing("chatgpt-account-id");
            then.status(200).body(
                r#"{
                  "chatgpt_account_id": "acct-pat",
                  "chatgpt_plan_type": "team",
                  "email": "pat@example.com"
                }"#,
            );
        })
        .await;

    let usage_mock = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/backend-api/wham/usage")
                .header("authorization", "Bearer at-test-token")
                .header("originator", "codex_cli_rs")
                .header("user-agent", &expected_ua)
                .header("accept", "application/json")
                .header("chatgpt-account-id", "acct-pat");
            then.status(200).body(
                r#"{
                  "plan_type": "team",
                  "rate_limit": {
                    "primary_window": {
                      "used_percent": 68,
                      "reset_at": 1766948068,
                      "limit_window_seconds": 604800
                    },
                    "secondary_window": null
                  }
                }"#,
            );
        })
        .await;

    let home = TempDir::new().unwrap();
    fs::write(
        home.path().join("config.toml"),
        format!("chatgpt_base_url = \"{}/backend-api\"", server.base_url()),
    )
    .unwrap();
    let env = BTreeMap::from([(
        "CODEX_HOME".to_owned(),
        home.path().to_string_lossy().into_owned(),
    )]);
    let net = https::broker(NetConfig::default());
    let fetched = CodexPATUsageFetcher::fetch_usage(
        &net,
        &CodexPATCredentials::new("at-test-token", CodexOAuthCredentialSource::CodexHome),
        Some("1.2.3"),
        &env,
        Some(home.path()),
    )
    .await
    .unwrap();

    let whoami = fetched.whoami.unwrap();
    assert_eq!(whoami.account_id.as_deref(), Some("acct-pat"));
    assert_eq!(whoami.email.as_deref(), Some("pat@example.com"));
    assert_eq!(whoami.plan_type.as_deref(), Some("team"));
    assert_eq!(
        fetched
            .usage
            .rate_limit
            .unwrap()
            .primary_window
            .unwrap()
            .used_percent,
        68
    );
    whoami_mock.assert_calls_async(1).await;
    usage_mock.assert_calls_async(1).await;
}

// upstream: CodexPATTests.swift:206 PAT usage mapping keeps pat source and does not request reset credits
#[test]
fn pat_usage_mapping_keeps_pat_source_and_does_not_request_reset_credits() {
    let raw=br#"{"plan_type":"team","rate_limit":{"primary_window":{"used_percent":68,"reset_at":1766948068,"limit_window_seconds":604800},"secondary_window":null}}"#;
    let whoami = CodexPATWhoami {
        account_id: Some("acct-pat".into()),
        email: Some("pat@example.com".into()),
        plan_type: Some("team".into()),
    };
    let r = CodexPATFetchStrategy::map_result_for_testing(raw, Some(&whoami), Timestamp::now())
        .unwrap();
    assert_eq!(r.source_label, "pat");
    assert_eq!(r.strategy_id, "codex.pat");
    assert_eq!(r.strategy_kind, SourceKind::Api);
    assert!(r.codex_reset_credits_attempted);
    assert!(r.codex_reset_credits.is_none());
    assert!(r.usage.primary.is_none());
    assert_eq!(r.usage.secondary.unwrap().used_percent, 68.);
    let id = r.usage.identity.unwrap();
    assert_eq!(id.account_email.as_deref(), Some("pat@example.com"));
    assert_eq!(id.login_method.as_deref(), Some("team"));
}

// upstream: CodexPATTests.swift:236 explicit PAT source does not include OAuth or CLI strategies
#[tokio::test]
async fn explicit_pat_source_does_not_include_oauth_or_cli_strategies() {
    let home = TempDir::new().unwrap();
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let plan = CodexProviderPlan::new(CodexStrategyConfig::new(
        net,
        BTreeMap::new(),
        home.path(),
        home.path(),
    ));
    let strategies = plan.resolve_strategies(&context(SourceMode::Api)).await;
    assert_eq!(
        strategies.iter().map(|s| s.id()).collect::<Vec<_>>(),
        ["codex.pat"]
    );
    assert_eq!(
        strategies.iter().map(|s| s.kind()).collect::<Vec<_>>(),
        [SourceKind::Api]
    );
}

// upstream: CodexPATTests.swift:257 PAT ignores managed and fail-closed CODEX_HOME when loading credentials
#[test]
fn pat_ignores_managed_and_fail_closed_codex_home_when_loading_credentials() {
    let fail_closed = "/Users/test/Library/Application Support/CodexBar/managed-store-unreadable";
    let managed_home = "/Users/test/Library/Application Support/CodexBar/managed-codex-homes/00000000-0000-0000-0000-000000000001";
    let root = TempDir::new().unwrap();
    let profile_home = root.path().join("codex-pat-profile");
    let profile_with_pat = root.path().join("codex-pat-profile-with-pat");
    fs::create_dir_all(&profile_home).unwrap();
    fs::create_dir_all(&profile_with_pat).unwrap();
    CodexOAuthCredentialsStore::write_private_file_for_testing(
        &profile_with_pat.join("auth.json"),
        br#"{"personal_access_token":"at-profile"}"#,
    )
    .unwrap();

    assert_eq!(
        CodexPATFetchStrategy::credential_environment_for_testing(&BTreeMap::from([
            ("CODEX_HOME".to_owned(), fail_closed.to_owned()),
            ("PATH".to_owned(), "/usr/bin".to_owned()),
        ]))
        .get("CODEX_HOME"),
        None
    );
    assert_eq!(
        CodexPATFetchStrategy::credential_environment_for_testing(&BTreeMap::from([(
            "CODEX_HOME".to_owned(),
            managed_home.to_owned()
        )]))
        .get("CODEX_HOME"),
        None
    );
    assert_eq!(
        CodexPATFetchStrategy::credential_environment_for_testing(&BTreeMap::from([(
            "CODEX_HOME".to_owned(),
            profile_home.to_string_lossy().into_owned()
        )]))
        .get("CODEX_HOME"),
        None
    );
    let profile_with_pat_str = profile_with_pat.to_string_lossy().into_owned();
    assert_eq!(
        CodexPATFetchStrategy::credential_environment_for_testing(&BTreeMap::from([(
            "CODEX_HOME".to_owned(),
            profile_with_pat_str.clone()
        )]))
        .get("CODEX_HOME")
        .map(String::as_str),
        Some(profile_with_pat_str.as_str())
    );
    assert_eq!(
        CodexPATFetchStrategy::credential_environment_for_testing(&BTreeMap::new())
            .get("CODEX_HOME"),
        None
    );
}

// upstream: CodexPATTests.swift:296 PAT ambient fallback uses env HOME instead of the process user home
#[test]
fn pat_ambient_fallback_uses_env_home_instead_of_the_process_user_home() {
    let root = TempDir::new().unwrap();
    let ambient_root = root.path().join("codex-pat-home");
    let ambient_codex_home = ambient_root.join(".codex");
    let managed_home = root.path().join("codex-pat-managed");
    fs::create_dir_all(&ambient_codex_home).unwrap();
    fs::create_dir_all(&managed_home).unwrap();
    CodexOAuthCredentialsStore::write_private_file_for_testing(
        &ambient_codex_home.join("auth.json"),
        br#"{"personal_access_token":"at-from-home"}"#,
    )
    .unwrap();

    let env = BTreeMap::from([
        (
            "HOME".to_owned(),
            ambient_root.to_string_lossy().into_owned(),
        ),
        (
            "CODEX_HOME".to_owned(),
            managed_home.to_string_lossy().into_owned(),
        ),
    ]);
    let resolved = CodexPATFetchStrategy::credential_environment_for_testing(&env);
    let expected_codex_home = ambient_codex_home.to_string_lossy().into_owned();
    assert_eq!(
        resolved.get("CODEX_HOME").map(String::as_str),
        Some(expected_codex_home.as_str())
    );
    assert_eq!(
        CodexOAuthCredentialsStore::load_pat_resolving_scoped_home(&env, None)
            .unwrap()
            .token_str(),
        "at-from-home"
    );
}

// upstream: CodexPATTests.swift:321 explicit PAT source does not fall back, Auto does after an unusable PAT
#[test]
fn explicit_pat_source_does_not_fall_back_auto_does_after_an_unusable_pat() {
    let strategy = CodexPATFetchStrategy::new();
    let explicit = context(SourceMode::Api);
    let auto = context(SourceMode::Auto);
    let unauthorized = CodexOAuthFetchError::Unauthorized.into_fetch_error();
    let missing =
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::MissingTokens).into_fetch_error();
    let not_found =
        CodexOAuthError::Credentials(CodexOAuthCredentialsError::NotFound).into_fetch_error();
    assert!(!strategy.should_fallback(&unauthorized, &explicit));
    assert!(!strategy.should_fallback(&missing, &explicit));
    assert!(strategy.should_fallback(&unauthorized, &auto));
    assert!(strategy.should_fallback(&not_found, &auto));
    assert!(strategy.should_fallback(&missing, &auto));
    assert!(!strategy.should_fallback(
        &CodexOAuthFetchError::InvalidResponse.into_fetch_error(),
        &auto
    ));
    assert!(!strategy.should_fallback(
        &CodexOAuthFetchError::ServerError(500, None).into_fetch_error(),
        &auto
    ));
}

// upstream: CodexPATAppPublicationTests.swift:7 ambient PAT publishes while a different managed Codex account is active
#[tokio::test]
async fn ambient_pat_publishes_while_a_different_managed_codex_account_is_active() {
    let server = https::start_server();
    let _whoami_mock = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/accounts/v1/user-auth-credential/whoami");
            then.status(200).body(
                r#"{
                  "chatgpt_account_id": "acct-pat",
                  "chatgpt_plan_type": "team",
                  "email": "pat@example.com"
                }"#,
            );
        })
        .await;
    let _usage_mock = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/backend-api/wham/usage");
            then.status(200).body(
                r#"{
                  "plan_type": "team",
                  "rate_limit": {
                    "primary_window": {
                      "used_percent": 68,
                      "reset_at": 1766948068,
                      "limit_window_seconds": 18000
                    },
                    "secondary_window": null
                  }
                }"#,
            );
        })
        .await;

    let root = TempDir::new().unwrap();
    let managed_home = root
        .path()
        .join("managed-codex-homes")
        .join("AAAAAAAA-BBBB-CCCC-DDDD-999999999999");
    let ambient_root = root.path().join("codex-pat-ambient");
    let ambient_codex_home = ambient_root.join(".codex");
    fs::create_dir_all(&managed_home).unwrap();
    fs::create_dir_all(&ambient_codex_home).unwrap();
    CodexOAuthCredentialsStore::write_private_file_for_testing(
        &ambient_codex_home.join("auth.json"),
        br#"{"personal_access_token":"at-test-token"}"#,
    )
    .unwrap();
    fs::write(
        ambient_codex_home.join("config.toml"),
        format!("chatgpt_base_url = \"{}/backend-api\"", server.base_url()),
    )
    .unwrap();

    let environment = BTreeMap::from([
        (
            "HOME".to_owned(),
            ambient_root.to_string_lossy().into_owned(),
        ),
        (
            "CODEX_HOME".to_owned(),
            managed_home.to_string_lossy().into_owned(),
        ),
        (
            "XDG_CONFIG_HOME".to_owned(),
            ambient_root.join(".config").to_string_lossy().into_owned(),
        ),
    ]);

    assert!(
        CodexOAuthCredentialsStore::load_pat_resolving_scoped_home(
            &environment,
            Some(&ambient_root)
        )
        .is_ok()
    );

    let net = Arc::new(https::broker(NetConfig::default()));
    let mut cfg = CodexStrategyConfig::new(net, environment, &ambient_root, &ambient_root);
    cfg.managed_workspace_account_id = Some("managed-workspace".to_owned());
    let plan = CodexProviderPlan::new(cfg);
    let cx = context(SourceMode::Auto);
    let outcome = fetch_outcome(&cx.provider, &plan, &cx, &NoRetry).await;
    let r = outcome.result.unwrap();
    assert_eq!(r.source_label, "pat");
    assert_eq!(r.strategy_id, CODEX_PAT_STRATEGY_ID);
    assert_eq!(
        r.usage.identity.as_ref().unwrap().account_email.as_deref(),
        Some("pat@example.com")
    );
    assert_eq!(r.usage.primary.as_ref().unwrap().used_percent, 68.0);
    let projected = provider_snapshot(
        &r.usage,
        &ProjectionInput {
            provider: cx.provider.clone(),
            display_name: "Codex".into(),
            enabled: true,
            source_mode: "auto".into(),
            refresh: RefreshState::Fresh,
            kept_snapshot: false,
            as_of: Timestamp::now(),
            source_kind: r.strategy_kind,
            source_label: r.source_label.clone(),
            error: None,
            pace: None,
        },
    );
    let id_metric = projected.identity.as_ref().unwrap();
    assert_eq!(id_metric.source.as_ref().unwrap().label, "pat");
    assert_eq!(
        id_metric.value.as_ref().unwrap().account_email.as_deref(),
        Some("pat@example.com")
    );
}

// upstream: CodexPATAppPublicationTests.swift:75 profile-only PAT skips stacked fan-out and publishes
#[tokio::test]
async fn profile_only_pat_skips_stacked_fan_out_and_publishes() {
    let server = https::start_server();
    let _whoami_mock = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/accounts/v1/user-auth-credential/whoami");
            then.status(200).body(
                r#"{
                  "chatgpt_account_id": "acct-pat-profile",
                  "chatgpt_plan_type": "team",
                  "email": "pat-profile@example.com"
                }"#,
            );
        })
        .await;
    let _usage_mock = server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/backend-api/wham/usage");
            then.status(200).body(
                r#"{
                  "plan_type": "team",
                  "rate_limit": {
                    "primary_window": {
                      "used_percent": 41,
                      "reset_at": 1766948068,
                      "limit_window_seconds": 18000
                    },
                    "secondary_window": null
                  }
                }"#,
            );
        })
        .await;

    let root = TempDir::new().unwrap();
    let profile_home = root.path().join("codex-pat-profile");
    let ambient_root = root.path().join("codex-pat-ambient-empty");
    let ambient_codex_home = ambient_root.join(".codex");
    fs::create_dir_all(&profile_home).unwrap();
    fs::create_dir_all(&ambient_codex_home).unwrap();
    CodexOAuthCredentialsStore::write_private_file_for_testing(
        &profile_home.join("auth.json"),
        br#"{"personal_access_token":"at-profile-only"}"#,
    )
    .unwrap();
    fs::write(
        profile_home.join("config.toml"),
        format!("chatgpt_base_url = \"{}/backend-api\"", server.base_url()),
    )
    .unwrap();

    let environment_base = BTreeMap::from([
        (
            "HOME".to_owned(),
            ambient_root.to_string_lossy().into_owned(),
        ),
        (
            "CODEX_HOME".to_owned(),
            ambient_codex_home.to_string_lossy().into_owned(),
        ),
        (
            "XDG_CONFIG_HOME".to_owned(),
            ambient_root.join(".config").to_string_lossy().into_owned(),
        ),
    ]);
    let profile_home_str = profile_home.to_string_lossy().into_owned();
    let fetch_env = CodexHomeScope::scoped_environment(&environment_base, Some(&profile_home_str));

    assert!(CodexOAuthCredentialsStore::load_pat(&environment_base, Some(&ambient_root)).is_err());
    assert!(
        CodexOAuthCredentialsStore::load_pat_resolving_scoped_home(&fetch_env, Some(&ambient_root))
            .is_ok()
    );

    let net = Arc::new(https::broker(NetConfig::default()));
    let cfg = CodexStrategyConfig::new(net, fetch_env, &ambient_root, &ambient_root);
    let plan = CodexProviderPlan::new(cfg);
    let cx = context(SourceMode::Auto);
    let outcome = fetch_outcome(&cx.provider, &plan, &cx, &NoRetry).await;
    let r = outcome.result.unwrap();
    assert_eq!(r.source_label, "pat");
    assert_eq!(r.strategy_id, CODEX_PAT_STRATEGY_ID);
    assert_eq!(
        r.usage.identity.as_ref().unwrap().account_email.as_deref(),
        Some("pat-profile@example.com")
    );
    assert_eq!(r.usage.primary.as_ref().unwrap().used_percent, 41.0);
    let projected = provider_snapshot(
        &r.usage,
        &ProjectionInput {
            provider: cx.provider.clone(),
            display_name: "Codex".into(),
            enabled: true,
            source_mode: "auto".into(),
            refresh: RefreshState::Fresh,
            kept_snapshot: false,
            as_of: Timestamp::now(),
            source_kind: r.strategy_kind,
            source_label: r.source_label.clone(),
            error: None,
            pace: None,
        },
    );
    let id_metric = projected.identity.as_ref().unwrap();
    assert_eq!(id_metric.source.as_ref().unwrap().label, "pat");
    assert_eq!(
        id_metric.value.as_ref().unwrap().account_email.as_deref(),
        Some("pat-profile@example.com")
    );
}

#[tokio::test]
async fn hostile_chatgpt_base_url_in_config_blocks_pat_token_exfiltration_and_keeps_whoami_on_openai_auth()
 {
    let server = https::start_server();
    let catch_all = server
        .mock_async(|_when, then| {
            then.status(200).body("{}");
        })
        .await;

    let home = TempDir::new().unwrap();
    let env = BTreeMap::from([(
        "CODEX_HOME".to_owned(),
        home.path().to_string_lossy().into_owned(),
    )]);
    let net = https::broker(NetConfig::default());
    let creds = CodexPATCredentials::new("at-secret-token", CodexOAuthCredentialSource::CodexHome);

    let same_port_hostile = format!("https://evil.localhost:{}/backend-api", server.port());
    for hostile_url in [
        same_port_hostile.as_str(),
        "https://evil.invalid/backend-api",
        "https://192.168.1.50/backend-api",
        "http://169.254.169.254/backend-api",
    ] {
        fs::write(
            home.path().join("config.toml"),
            format!("chatgpt_base_url = \"{hostile_url}\"\n"),
        )
        .unwrap();
        let err = CodexPATUsageFetcher::fetch_usage(&net, &creds, None, &env, Some(home.path()))
            .await
            .unwrap_err();
        assert_eq!(err, CodexOAuthFetchError::InvalidResponse);
    }
    catch_all.assert_calls_async(0).await;

    fs::write(
        home.path().join("config.toml"),
        "chatgpt_base_url = \"https://chatgpt.com/backend-api\"\n",
    )
    .unwrap();
    let (whoami_url, loopback_override) =
        CodexPATUsageFetcher::resolve_whoami_url(&env, Some(home.path())).unwrap();
    assert_eq!(whoami_url.as_str(), DEFAULT_PAT_WHOAMI_URL);
    assert_eq!(loopback_override, None);
}
