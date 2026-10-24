// Ported from CodexBar Tests/CodexBarTests/ClaudeAdminAPIUsageTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AdminAPIUsageLocalDaySelectionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeAdminAPIInlineDashboardModelTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#[path = "support/https.rs"]
mod https;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::pipeline::{
    BoxFuture, ClassifiedError, FetchContext, FetchError, FetchInteraction, FetchPhase,
    FetchRuntime, FetchStrategy, ProviderEnvironmentResolver, ProviderPlan, Sleeper, fetch_outcome,
};
use ariadusage_core::projection::{ProjectionInput, RefreshState, provider_snapshot};
use ariadusage_core::providers::SourceMode;
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::SourceKind;
use ariadusage_protocol::ids::SettingId;
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use httpmock::prelude::*;
use jiff::Timestamp;
use tokio_util::sync::CancellationToken;

use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::net::{NetConfig, OriginError};
use ariadusage_engine::brokers::secret_store::memory::MemoryBackend;
use ariadusage_engine::brokers::secret_store::{SecretId, SecretStore};
use ariadusage_engine::providers::claude::{
    ADMIN_API_USER_AGENT, ANTHROPIC_VERSION_HEADER, ANTHROPIC_VERSION_VALUE, API_KEY_HEADER,
    CLAUDE_ADMIN_API_STRATEGY_ID, COST_REPORT_PATH, ClaudeAdminAPIEndpointConfig,
    ClaudeAdminAPIFetchStrategy, ClaudeAdminAPISettingsError, ClaudeAdminAPISettingsReader,
    ClaudeAdminAPIUsageError, ClaudeAdminAPIUsageFetcher, ClaudeAdminAPIUsageSnapshot,
    ClaudeOAuthStrategyConfig, ClaudeOptionalDetailPolicy, ClaudeProviderPlan, CostBreakdown,
    DailyBucket, MESSAGES_REPORT_PATH, ModelBreakdown, claude_admin_api_declared_origins,
    claude_env_descriptor, claude_token_account_env_override, validate_admin_api_url,
};

struct NoopSleeper;

impl Sleeper for NoopSleeper {
    fn sleep<'a>(&'a self, _duration: Duration) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

fn make_context(
    source_mode: SourceMode,
    env: BTreeMap<String, SecretString>,
    selected_token_account: Option<String>,
) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("claude").unwrap(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "claude-admin-api-test".to_owned(),
        source_mode,
        env,
        include_credits: true,
        selected_token_account,
        cancel: CancellationToken::new(),
    }
}

fn make_strategy_config() -> (tempfile::TempDir, ClaudeOAuthStrategyConfig) {
    let temp = tempfile::tempdir().expect("tempdir");
    let net = Arc::new(https::broker(NetConfig::default()));
    let config = ClaudeOAuthStrategyConfig::new(net, temp.path(), temp.path());
    (temp, config)
}

// Upstream @Test (ClaudeAdminAPIUsageTests.swift):
// `prefers primary Anthropic admin key environment variable`
#[test]
fn prefers_primary_anthropic_admin_key_environment_variable() {
    let mut env = BTreeMap::new();
    env.insert(
        ClaudeAdminAPISettingsReader::ALTERNATE_ADMIN_API_KEY_ENV_KEY.to_owned(),
        "sk-ant-admin-alt".to_owned(),
    );
    env.insert(
        ClaudeAdminAPISettingsReader::ADMIN_API_KEY_ENV_KEY.to_owned(),
        "sk-ant-admin-primary".to_owned(),
    );
    let token = ClaudeAdminAPISettingsReader::api_key_from_strings(&env);
    assert_eq!(
        token.as_ref().map(SecretString::expose_secret),
        Some("sk-ant-admin-primary")
    );
}

// Upstream @Test (ClaudeAdminAPIUsageTests.swift):
// `routes Claude token account admin keys into admin api environment`
#[test]
fn routes_claude_token_account_admin_keys_into_admin_api_environment() {
    let env = claude_token_account_env_override("Bearer sk-ant-admin-token")
        .expect("admin key env override");
    assert_eq!(
        env.get(ClaudeAdminAPISettingsReader::ADMIN_API_KEY_ENV_KEY)
            .map(SecretString::expose_secret),
        Some("sk-ant-admin-token")
    );
}

// Upstream @Test (ClaudeAdminAPIUsageTests.swift):
// `auto source uses configured admin api key`
#[tokio::test]
async fn auto_source_uses_configured_admin_api_key() {
    let (_temp, config) = make_strategy_config();
    let plan = ClaudeProviderPlan::new(config);
    let mut env = BTreeMap::new();
    env.insert(
        ClaudeAdminAPISettingsReader::ADMIN_API_KEY_ENV_KEY.to_owned(),
        SecretString::new("sk-ant-admin-test"),
    );
    let cx = make_context(SourceMode::Auto, env, None);
    let strategies = plan.resolve_strategies(&cx).await;
    assert_eq!(
        strategies.iter().map(|s| s.id()).collect::<Vec<_>>(),
        vec!["claude.admin-api"]
    );
}

// Upstream @Test (ClaudeAdminAPIUsageTests.swift):
// `parses Anthropic admin cost and messages usage into daily summaries`
#[test]
fn parses_anthropic_admin_cost_and_messages_usage_into_daily_summaries() {
    let now = Timestamp::from_second(1_700_179_200).unwrap();
    let costs = r#"{
      "data": [
        {
          "starting_at": "2023-11-14T00:00:00Z",
          "ending_at": "2023-11-15T00:00:00Z",
          "results": [
            {
              "currency": "USD",
              "amount": "12345.00",
              "description": "Claude Sonnet 4 Usage - Input Tokens",
              "cost_type": "tokens"
            },
            {
              "currency": "USD",
              "amount": "2500.00",
              "description": "Web Search Usage",
              "cost_type": "web_search"
            }
          ]
        },
        {
          "starting_at": "2023-11-15T00:00:00Z",
          "ending_at": "2023-11-16T00:00:00Z",
          "results": [
            {
              "currency": "USD",
              "amount": "5000",
              "description": "Claude Haiku Usage - Output Tokens",
              "cost_type": "tokens"
            }
          ]
        }
      ],
      "has_more": false,
      "next_page": null
    }"#;
    let messages = r#"{
      "data": [
        {
          "starting_at": "2023-11-14T00:00:00Z",
          "ending_at": "2023-11-15T00:00:00Z",
          "results": [
            {
              "uncached_input_tokens": 1500,
              "cache_creation": {
                "ephemeral_1h_input_tokens": 1000,
                "ephemeral_5m_input_tokens": 500
              },
              "cache_read_input_tokens": 200,
              "output_tokens": 500,
              "model": "claude-sonnet-4-20250514"
            },
            {
              "uncached_input_tokens": 100,
              "output_tokens": 50,
              "model": "claude-opus-4-20250514"
            }
          ]
        },
        {
          "starting_at": "2023-11-15T00:00:00Z",
          "ending_at": "2023-11-16T00:00:00Z",
          "results": [
            {
              "uncached_input_tokens": 200,
              "cache_read_input_tokens": 300,
              "output_tokens": 100,
              "model": "claude-sonnet-4-20250514"
            }
          ]
        }
      ],
      "has_more": false,
      "next_page": null
    }"#;

    let snapshot = ClaudeAdminAPIUsageFetcher::parse_snapshot(
        costs.as_bytes(),
        messages.as_bytes(),
        false,
        now,
    )
    .expect("parse_snapshot");

    assert_eq!(snapshot.daily.len(), 2);
    assert!((snapshot.daily[0].cost_usd - 148.45).abs() < 0.0001);
    assert_eq!(snapshot.daily[0].input_tokens, 1600);
    assert_eq!(snapshot.daily[0].cache_creation_input_tokens, 1500);
    assert_eq!(snapshot.daily[0].cache_read_input_tokens, 200);
    assert_eq!(snapshot.daily[0].output_tokens, 550);
    assert_eq!(snapshot.daily[0].total_tokens(), 3850);
    assert!((snapshot.last_30_days().cost_usd - 198.45).abs() < 0.0001);
    assert_eq!(snapshot.last_30_days().total_tokens(), 4450);
    let top_models = snapshot.top_models();
    assert_eq!(
        top_models.first().map(|m| m.name.as_str()),
        Some("claude-sonnet-4-20250514")
    );
    assert_eq!(top_models.first().map(|m| m.total_tokens()), Some(4300));
}

// Upstream @Test (ClaudeAdminAPIUsageTests.swift):
// `maps Anthropic admin usage to Claude usage snapshot`
#[test]
fn maps_anthropic_admin_usage_to_claude_usage_snapshot() {
    let now = Timestamp::from_second(1_700_179_200).unwrap();
    let mut bucket = DailyBucket::new("2023-11-14", now);
    bucket.cost_usd = 8.5;
    bucket.input_tokens = 1000;
    bucket.cache_creation_input_tokens = 400;
    bucket.cache_read_input_tokens = 300;
    bucket.output_tokens = 250;

    let api_usage = ClaudeAdminAPIUsageSnapshot::new(vec![bucket], None, now);
    let usage = api_usage.to_usage_snapshot();

    assert!(usage.primary.is_none());
    assert_eq!(usage.provider_cost.as_ref().map(|c| c.used), Some(8.5));
    assert_eq!(usage.provider_cost.as_ref().map(|c| c.limit), Some(0.0));
    assert_eq!(
        usage
            .provider_cost
            .as_ref()
            .and_then(|c| c.period.as_deref()),
        Some("Last 30 days")
    );
    assert_eq!(
        usage.detail_row("30d tokens").map(|r| r.value.as_str()),
        Some("1,950")
    );
    assert_eq!(
        usage
            .identity
            .as_ref()
            .and_then(|i| i.provider_id.as_ref())
            .map(ProviderId::as_str),
        Some("claude")
    );
    assert_eq!(
        usage
            .identity
            .as_ref()
            .and_then(|i| i.login_method.as_deref()),
        Some("Admin API")
    );
}

// Upstream @Test (ClaudeAdminAPIUsageTests.swift):
// `current day summary is zero when Claude admin history is stale`
#[test]
fn current_day_summary_is_zero_when_claude_admin_history_is_stale() {
    let now = "2023-11-17T12:00:00Z".parse::<Timestamp>().unwrap();
    let bucket_day = "2023-11-14T12:00:00Z".parse::<Timestamp>().unwrap();
    let mut bucket = DailyBucket::new("2023-11-14", bucket_day);
    bucket.cost_usd = 8.5;
    bucket.input_tokens = 1000;
    bucket.cache_creation_input_tokens = 400;
    bucket.cache_read_input_tokens = 300;
    bucket.output_tokens = 250;

    let api_usage = ClaudeAdminAPIUsageSnapshot::new(vec![bucket], None, now);

    assert_eq!(api_usage.current_day().cost_usd, 0.0);
    assert_eq!(api_usage.current_day().total_tokens(), 0);
    assert_eq!(api_usage.latest_day().cost_usd, 8.5);
    assert_eq!(api_usage.latest_day().total_tokens(), 1950);
}

// Upstream @Test (ClaudeAdminAPIUsageTests.swift):
// `fetch strategy reports admin api source label`
#[tokio::test]
async fn fetch_strategy_reports_admin_api_source_label() {
    for enabled in [false, true] {
        let server = https::start_server();
        let cost_mock = server
            .mock_async(|when, then| {
                let mut m = when
                    .method(GET)
                    .path(COST_REPORT_PATH)
                    .header(API_KEY_HEADER, "sk-ant-admin-test")
                    .query_param("group_by[]", "description");
                if enabled {
                    m = m.query_param("group_by[]", "workspace_id");
                }
                let _ = m;
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"data":[]}"#);
            })
            .await;
        let messages_mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path(MESSAGES_REPORT_PATH)
                    .header(API_KEY_HEADER, "sk-ant-admin-test");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"data":[]}"#);
            })
            .await;

        let (_temp, config) = make_strategy_config();
        let endpoint = ClaudeAdminAPIEndpointConfig::with_base_url_str(&server.base_url())
            .expect("loopback url");
        let config = config.with_admin_api_endpoint(endpoint);
        let strategy = ClaudeAdminAPIFetchStrategy::new(config);

        let mut env = BTreeMap::new();
        env.insert(
            ClaudeAdminAPISettingsReader::ADMIN_API_KEY_ENV_KEY.to_owned(),
            SecretString::new("sk-ant-admin-test"),
        );
        env.insert(
            ClaudeAdminAPISettingsReader::WORKSPACE_SPEND_ENV_KEY.to_owned(),
            SecretString::new(enabled.to_string()),
        );
        let cx = make_context(SourceMode::Api, env, None);
        let result = strategy.fetch(&cx).await.expect("strategy fetch");

        assert_eq!(result.source_label, "admin-api");
        assert_eq!(
            result
                .usage
                .identity
                .as_ref()
                .and_then(|i| i.login_method.as_deref()),
            Some("Admin API")
        );
        cost_mock.assert_async().await;
        messages_mock.assert_async().await;
    }
}

// Upstream @Test (ClaudeAdminAPIUsageTests.swift):
// `workspace details preserve organization totals and default workspace cents`
#[test]
fn workspace_details_preserve_organization_totals_and_default_workspace_cents() {
    let costs = r#"{"data":[
      {"starting_at":"2026-09-23T00:00:00Z","ending_at":"2026-09-24T00:00:00Z","results":[
        {"currency":"USD","amount":"1250","description":"Input tokens","workspace_id":"wrk_fixture"},
        {"currency":"USD","amount":"250","description":"Output tokens","workspace_id":"wrk_fixture"},
        {"currency":"USD","amount":"50","description":"Input tokens","workspace_id":null}]},
      {"starting_at":"2026-09-24T00:00:00Z","ending_at":"2026-09-25T00:00:00Z","results":[
        {"currency":"USD","amount":"75","description":"Input tokens","workspace_id":null}]}]}"#;
    let now = "2026-09-24T12:00:00Z".parse::<Timestamp>().unwrap();

    for enabled in [false, true] {
        let snapshot = ClaudeAdminAPIUsageFetcher::parse_snapshot(
            costs.as_bytes(),
            br#"{"data":[]}"#,
            enabled,
            now,
        )
        .expect("parse_snapshot");
        let usage = snapshot.to_usage_snapshot();
        assert_eq!(usage.provider_cost.as_ref().map(|c| c.used), Some(16.25));
        assert_eq!(
            snapshot.top_cost_items().first().map(|c| c.cost_usd),
            Some(13.75)
        );
        let section = usage
            .details
            .iter()
            .find(|s| s.title.as_deref() == Some("Workspace spend · 30d"));
        if enabled {
            let rows = &section.expect("workspace section").rows;
            assert_eq!(
                rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
                vec!["wrk_fixture", "Default"]
            );
            assert_eq!(
                rows.iter().map(|r| r.value.as_str()).collect::<Vec<_>>(),
                vec!["$15.00", "$1.25"]
            );
        } else {
            assert!(section.is_none());
        }
    }
}

// Upstream @Test (ClaudeAdminAPIUsageTests.swift):
// `workspace option groups the existing cost request only`
#[tokio::test]
async fn workspace_option_groups_the_existing_cost_request_only() {
    for enabled in [false, true] {
        let server = https::start_server();
        let cost_mock = server
            .mock_async(|when, then| {
                let mut m = when
                    .method(GET)
                    .path(COST_REPORT_PATH)
                    .header(API_KEY_HEADER, "fixture-admin-key")
                    .query_param("group_by[]", "description");
                if enabled {
                    m = m.query_param("group_by[]", "workspace_id");
                }
                let _ = m;
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"data":[]}"#);
            })
            .await;
        let messages_mock = server
            .mock_async(|when, then| {
                when.method(GET)
                    .path(MESSAGES_REPORT_PATH)
                    .header(API_KEY_HEADER, "fixture-admin-key")
                    .query_param("group_by[]", "model");
                then.status(200)
                    .header("content-type", "application/json")
                    .body(r#"{"data":[]}"#);
            })
            .await;

        let net = https::broker(NetConfig::default());
        let endpoint = ClaudeAdminAPIEndpointConfig::with_base_url_str(&server.base_url())
            .expect("loopback url");
        let now = "2026-09-24T12:00:00Z".parse::<Timestamp>().unwrap();
        let call = https::call();

        let snapshot = ClaudeAdminAPIUsageFetcher::fetch_usage(
            &net,
            &endpoint,
            "fixture-admin-key",
            enabled,
            now,
            &call,
        )
        .await
        .expect("fetch_usage");

        cost_mock.assert_calls_async(1).await;
        messages_mock.assert_calls_async(1).await;
        assert!(
            snapshot
                .to_usage_snapshot()
                .details
                .iter()
                .all(|s| s.title.as_deref() != Some("Workspace spend · 30d"))
        );
    }
}

// Upstream @Test (ClaudeAdminAPIUsageTests.swift):
// `workspace rows are bounded and single workspace retains the organization view`
#[test]
fn workspace_rows_are_bounded_and_single_workspace_retains_the_organization_view() {
    let now = "2026-09-24T12:00:00Z".parse::<Timestamp>().unwrap();
    for count in [1usize, 25usize] {
        let rows = (0..count)
            .map(|i| {
                format!(
                    r#"{{"amount":"{}","workspace_id":"wrk_fixture_{i}"}}"#,
                    i * 100
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let costs = format!(
            r#"{{"data":[{{"starting_at":"2026-09-24T00:00:00Z","ending_at":"2026-09-25T00:00:00Z","results":[{rows}]}}]}}"#
        );
        let snapshot = ClaudeAdminAPIUsageFetcher::parse_snapshot(
            costs.as_bytes(),
            br#"{"data":[]}"#,
            true,
            now,
        )
        .expect("parse_snapshot");
        let usage = snapshot.to_usage_snapshot();
        let section = usage
            .details
            .iter()
            .find(|s| s.title.as_deref() == Some("Workspace spend · 30d"));
        assert_eq!(
            usage.provider_cost.as_ref().map(|c| c.used),
            Some((count * (count - 1) / 2) as f64)
        );
        assert_eq!(
            section.map(|s| s.rows.len()),
            if count == 1 { None } else { Some(20) }
        );
        if count > 1 {
            let section = section.unwrap();
            assert_eq!(
                section.rows.first().map(|r| r.label.as_str()),
                Some("wrk_fixture_24")
            );
            assert_eq!(
                section.rows.last().map(|r| r.label.as_str()),
                Some("wrk_fixture_5")
            );
        }
    }
}

// Upstream @Test (ClaudeAdminAPIUsageTests.swift):
// `workspace totals use the same last thirty buckets as organization spend`
#[test]
fn workspace_totals_use_the_same_last_thirty_buckets_as_organization_spend() {
    let now = "2026-09-24T12:00:00Z".parse::<Timestamp>().unwrap();
    let buckets = (0..31usize)
        .map(|offset| {
            let start = now
                .checked_sub(jiff::SignedDuration::from_secs((offset as i64) * 86_400))
                .unwrap();
            let end = start
                .checked_add(jiff::SignedDuration::from_secs(86_400))
                .unwrap();
            let amount = if offset == 30 { 10000 } else { 100 };
            format!(
                r#"{{"starting_at":"{}","ending_at":"{}","results":[{{"amount":"{amount}","workspace_id":"wrk_fixture_{}"}}]}}"#,
                ClaudeAdminAPIUsageFetcher::iso8601_string(start),
                ClaudeAdminAPIUsageFetcher::iso8601_string(end),
                offset % 2
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let costs = format!(r#"{{"data":[{buckets}]}}"#);
    let snapshot =
        ClaudeAdminAPIUsageFetcher::parse_snapshot(costs.as_bytes(), br#"{"data":[]}"#, true, now)
            .expect("parse_snapshot");

    assert_eq!(snapshot.last_30_days().cost_usd, 30.0);
    assert_eq!(
        snapshot
            .workspace_costs
            .as_ref()
            .map(|w| w.iter().map(|i| i.cost_usd).collect::<Vec<_>>()),
        Some(vec![15.0, 15.0])
    );
    let encoded = serde_json::to_vec(&snapshot).expect("encode snapshot");
    let decoded: ClaudeAdminAPIUsageSnapshot =
        serde_json::from_slice(&encoded).expect("decode snapshot");
    assert_eq!(decoded, snapshot);
}

#[tokio::test]
async fn admin_api_fetch_strategy_is_unavailable_in_auto_when_token_account_is_selected() {
    let (_temp, config) = make_strategy_config();
    let config = config
        .with_token_account_token(Some(SecretString::new("sk-ant-oat-selected")))
        .with_admin_api_key(Some(SecretString::new("sk-ant-admin-saved")));
    let strategy = ClaudeAdminAPIFetchStrategy::new(config);

    let mut env = BTreeMap::new();
    env.insert(
        "ANTHROPIC_ADMIN_KEY".to_owned(),
        SecretString::new("sk-ant-admin-env"),
    );
    let cx = make_context(SourceMode::Auto, env, Some("acct-1".to_owned()));

    assert!(!strategy.is_available(&cx).await);
}

#[tokio::test]
async fn admin_api_fetch_strategy_is_available_when_selected_account_is_configured_for_api() {
    let (_temp, config) = make_strategy_config();
    let config = config
        .with_token_account_token(Some(SecretString::new("Bearer sk-ant-admin-selected")))
        .with_admin_api_key(Some(SecretString::new("sk-ant-admin-saved")));
    let strategy = ClaudeAdminAPIFetchStrategy::new(config);

    let mut env = BTreeMap::new();
    env.insert(
        "ANTHROPIC_ADMIN_KEY".to_owned(),
        SecretString::new("sk-ant-admin-env"),
    );
    let cx = make_context(SourceMode::Api, env, Some("acct-1".to_owned()));

    assert!(strategy.is_available(&cx).await);
    let err = FetchError::Classified(ClassifiedError::new(
        ProviderErrorKind::MissingCredential,
        ClaudeAdminAPISettingsError::MissingToken.to_string(),
    ));
    assert!(!strategy.should_fallback(&err, &cx));
}

#[tokio::test]
async fn admin_api_fetch_strategy_rejects_selected_account_with_subscription_credential_in_api_mode()
 {
    let (_temp, config) = make_strategy_config();
    let config = config
        .with_token_account_token(Some(SecretString::new("sk-ant-oat-selected")))
        .with_admin_api_key(Some(SecretString::new("sk-ant-admin-saved")));
    let strategy = ClaudeAdminAPIFetchStrategy::new(config);

    let mut env = BTreeMap::new();
    env.insert(
        "ANTHROPIC_ADMIN_KEY".to_owned(),
        SecretString::new("sk-ant-admin-env"),
    );
    let cx = make_context(SourceMode::Api, env, Some("acct-1".to_owned()));

    assert!(!strategy.is_available(&cx).await);
    let result = strategy.fetch(&cx).await;
    match result {
        Err(FetchError::Classified(c)) => {
            assert_eq!(c.kind, ProviderErrorKind::MissingCredential);
            assert_eq!(
                c.message,
                ClaudeAdminAPISettingsError::MissingToken.to_string()
            );
        }
        other => panic!("expected MissingCredential error, got {other:?}"),
    }
}

// Upstream @Test (AdminAPIUsageLocalDaySelectionTests.swift):
// `Claude Admin current day includes UTC bucket containing positive timezone morning`
#[test]
fn claude_admin_current_day_includes_utc_bucket_containing_positive_timezone_morning() {
    // 2026-05-18 08:00 Australia/Sydney (UTC+10) == 2026-05-17T22:00:00Z
    let now = "2026-05-17T22:00:00Z".parse::<Timestamp>().unwrap();
    let stale_utc_start = "2026-05-16T00:00:00Z".parse::<Timestamp>().unwrap();
    let overlapping_utc_start = "2026-05-17T00:00:00Z".parse::<Timestamp>().unwrap();

    let mut b1 = DailyBucket::new("2026-05-16", stale_utc_start);
    b1.cost_usd = 9.0;
    b1.input_tokens = 900;
    b1.cache_creation_input_tokens = 90;
    b1.cache_read_input_tokens = 45;
    b1.output_tokens = 90;

    let mut b2 = DailyBucket::new("2026-05-17", overlapping_utc_start);
    b2.cost_usd = 2.5;
    b2.input_tokens = 200;
    b2.cache_creation_input_tokens = 20;
    b2.cache_read_input_tokens = 10;
    b2.output_tokens = 30;

    let usage = ClaudeAdminAPIUsageSnapshot::new(vec![b1, b2], None, now);
    let today = usage.summary_for_local_day_containing(now);

    assert_eq!(today.cost_usd, 2.5);
    assert_eq!(today.input_tokens, 200);
    assert_eq!(today.total_tokens(), 260);
}

// Upstream @Test (AdminAPIUsageLocalDaySelectionTests.swift):
// `Claude Admin current day does not sum adjacent UTC buckets after negative timezone UTC rollover`
#[test]
fn claude_admin_current_day_does_not_sum_adjacent_utc_buckets_after_negative_timezone_utc_rollover()
{
    // 2026-06-22 20:00 America/Los_Angeles (UTC-7 PDT) == 2026-06-23T03:00:00Z
    let now = "2026-06-23T03:00:00Z".parse::<Timestamp>().unwrap();
    let previous_utc_start = "2026-06-22T00:00:00Z".parse::<Timestamp>().unwrap();
    let current_utc_start = "2026-06-23T00:00:00Z".parse::<Timestamp>().unwrap();

    let mut b1 = DailyBucket::new("2026-06-22", previous_utc_start);
    b1.cost_usd = 2.5;
    b1.input_tokens = 200;
    b1.cache_creation_input_tokens = 20;
    b1.cache_read_input_tokens = 10;
    b1.output_tokens = 30;

    let mut b2 = DailyBucket::new("2026-06-23", current_utc_start);
    b2.cost_usd = 4.5;
    b2.input_tokens = 400;
    b2.cache_creation_input_tokens = 40;
    b2.cache_read_input_tokens = 20;
    b2.output_tokens = 50;

    let usage = ClaudeAdminAPIUsageSnapshot::new(vec![b1, b2], None, now);
    let today = usage.summary_for_local_day_containing(now);

    assert_eq!(today.cost_usd, 4.5);
    assert_eq!(today.input_tokens, 400);
    assert_eq!(today.total_tokens(), 510);
}

// Upstream @Test (ClaudeAdminAPIInlineDashboardModelTests.swift):
// `claude admin api usage gets inline dashboard`
#[test]
fn claude_admin_api_usage_gets_inline_dashboard() {
    let now = "2023-11-17T12:00:00Z".parse::<Timestamp>().unwrap();
    let bucket_day = "2023-11-14T12:00:00Z".parse::<Timestamp>().unwrap();

    let mut bucket = DailyBucket::new("2023-11-14", bucket_day);
    bucket.cost_usd = 1.25;
    bucket.input_tokens = 1000;
    bucket.cache_creation_input_tokens = 400;
    bucket.cache_read_input_tokens = 300;
    bucket.output_tokens = 250;
    bucket.cost_items = vec![CostBreakdown::new("Claude Sonnet Usage", 1.25)];
    bucket.models = vec![ModelBreakdown {
        name: "claude-sonnet-4-20250514".to_owned(),
        input_tokens: 1000,
        cache_creation_input_tokens: 400,
        cache_read_input_tokens: 300,
        output_tokens: 250,
    }];

    let usage = ClaudeAdminAPIUsageSnapshot::new(vec![bucket], None, now);
    let snapshot = usage.to_usage_snapshot();

    assert!(snapshot.primary.is_none());
    assert!(snapshot.secondary.is_none());
    assert!(snapshot.tertiary.is_none());
    assert!(!ClaudeOptionalDetailPolicy::shows_provider_cost_section(
        &snapshot, true, true
    ));

    let provider_details =
        ClaudeOptionalDetailPolicy::visible_details(&snapshot.details, true, true);
    assert_eq!(
        provider_details
            .first()
            .and_then(|s| s.rows.first())
            .map(|r| r.value.as_str()),
        Some("$0.00")
    );
    assert_eq!(
        provider_details
            .first()
            .and_then(|s| s.chart.as_ref())
            .and_then(|c| c.points.first())
            .map(|p| p.label.as_str()),
        Some("2023-11-14")
    );
    assert_eq!(
        provider_details
            .first()
            .and_then(|s| s.chart.as_ref())
            .and_then(|c| c.points.first())
            .map(|p| p.value),
        Some(1.25)
    );
    assert!(
        provider_details
            .first()
            .is_some_and(|s| s.rows.iter().any(|r| r.label == "30d tokens"))
    );
    assert!(provider_details.first().is_some_and(|s| {
        s.rows
            .iter()
            .any(|r| r.label == "Top model" && r.value == "claude-sonnet-4-20250514")
    }));
    assert_eq!(
        snapshot
            .identity
            .as_ref()
            .and_then(|i| i.login_method.as_deref()),
        Some("Admin API")
    );

    // Also verify protocol projection omits limit when limit <= 0
    let projected = provider_snapshot(
        &snapshot,
        &ProjectionInput {
            provider: ProviderId::new("claude").unwrap(),
            display_name: "Claude".to_owned(),
            enabled: true,
            source_mode: "api".to_owned(),
            refresh: RefreshState::Fresh,
            kept_snapshot: false,
            as_of: snapshot.updated_at,
            source_kind: SourceKind::Api,
            source_label: "admin-api".to_owned(),
            error: None,
            pace: None,
        },
    );
    assert_eq!(
        projected
            .cost
            .as_ref()
            .and_then(|c| c.value.as_ref())
            .and_then(|c| c.limit),
        None
    );
    assert_eq!(
        projected
            .cost
            .as_ref()
            .and_then(|c| c.value.as_ref())
            .and_then(|c| c.balance),
        None
    );
    assert_eq!(projected.details.len(), 2);

    // And when inline dashboard is disabled, sections hide
    let hidden = ClaudeOptionalDetailPolicy::visible_details(&snapshot.details, false, true);
    assert!(hidden.is_empty());
}

#[tokio::test]
async fn endpoint_override_rejects_untrusted_hosts_before_sending_secret() {
    assert_eq!(
        ClaudeAdminAPIEndpointConfig::with_base_url_str("https://evil.example.com"),
        Err(OriginError::Invalid)
    );
    assert_eq!(
        ClaudeAdminAPIEndpointConfig::with_base_url_str("https://claude.ai"),
        Err(OriginError::Invalid)
    );
    assert_eq!(
        ClaudeAdminAPIEndpointConfig::with_base_url_str("http://api.anthropic.com"),
        Err(OriginError::Invalid)
    );
    assert_eq!(
        ClaudeAdminAPIEndpointConfig::with_base_url_str("https://api.anthropic.com:8443"),
        Err(OriginError::Invalid)
    );
    assert!(ClaudeAdminAPIEndpointConfig::with_base_url_str("https://api.anthropic.com").is_ok());
    assert!(
        validate_admin_api_url(
            &reqwest::Url::parse("https://evil.example.com/v1/organizations/cost_report").unwrap()
        )
        .is_err()
    );

    // Even if a caller attempts to send a request to another host against the declared
    // Claude Admin API origins, NetBroker rejects the origin before any network call.
    let net = https::broker(NetConfig::default());
    let mut req = https::request("https://evil.example.com/v1/organizations/cost_report");
    req.credentials.push((
        reqwest::header::HeaderName::from_static(API_KEY_HEADER),
        SecretString::new("sk-ant-admin01-never-send"),
    ));
    let call = https::call();
    let err = net
        .send_response(&call, claude_admin_api_declared_origins(), req)
        .await
        .expect_err("untrusted host must be rejected by NetBroker");
    assert!(matches!(
        err,
        ariadusage_engine::brokers::net::NetError::OriginNotDeclared
    ));
}

#[tokio::test]
async fn fetch_usage_sends_pinned_headers_and_query_params_and_parses_reports() {
    let server = https::start_server();

    let cost_mock = server
        .mock_async(|when, then| {
            when.method(GET)
                .path(COST_REPORT_PATH)
                .header(ANTHROPIC_VERSION_HEADER, ANTHROPIC_VERSION_VALUE)
                .header(API_KEY_HEADER, "sk-ant-admin01-test-key")
                .header("accept", "application/json")
                .header("user-agent", ADMIN_API_USER_AGENT)
                .query_param("starting_at", "2026-02-19T00:00:00Z")
                .query_param("ending_at", "2026-03-22T00:00:00Z")
                .query_param("bucket_width", "1d")
                .query_param("limit", "31")
                .query_param("group_by[]", "description")
                .query_param("group_by[]", "workspace_id");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{
                      "data": [
                        {
                          "starting_at": "2026-03-21T00:00:00Z",
                          "ending_at": "2026-03-22T00:00:00Z",
                          "results": [
                            {
                              "amount": "450",
                              "description": "Claude Sonnet 4 Input Tokens",
                              "workspace_id": "wrk_1"
                            }
                          ]
                        }
                      ]
                    }"#,
                );
        })
        .await;

    let messages_mock = server
        .mock_async(|when, then| {
            when.method(GET)
                .path(MESSAGES_REPORT_PATH)
                .header(ANTHROPIC_VERSION_HEADER, ANTHROPIC_VERSION_VALUE)
                .header(API_KEY_HEADER, "sk-ant-admin01-test-key")
                .header("accept", "application/json")
                .header("user-agent", ADMIN_API_USER_AGENT)
                .query_param("starting_at", "2026-02-19T00:00:00Z")
                .query_param("ending_at", "2026-03-22T00:00:00Z")
                .query_param("bucket_width", "1d")
                .query_param("limit", "31")
                .query_param("group_by[]", "model");
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    r#"{
                      "data": [
                        {
                          "starting_at": "2026-03-21T00:00:00Z",
                          "ending_at": "2026-03-22T00:00:00Z",
                          "results": [
                            {
                              "model": "claude-sonnet-4-20250514",
                              "uncached_input_tokens": 500,
                              "cache_read_input_tokens": 100,
                              "output_tokens": 200,
                              "cache_creation": {
                                "ephemeral_1h_input_tokens": 25,
                                "ephemeral_5m_input_tokens": 25
                              }
                            }
                          ]
                        }
                      ]
                    }"#,
                );
        })
        .await;

    let (_temp, config) = make_strategy_config();
    let endpoint =
        ClaudeAdminAPIEndpointConfig::with_base_url_str(&server.base_url()).expect("loopback url");
    let now = "2026-03-21T15:30:00Z".parse::<Timestamp>().unwrap();

    // Test SecretStore resolution via ClaudeProviderPlan
    let secret_store = Arc::new(SecretStore::new(MemoryBackend::new()));
    let call = BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "store-seed".to_owned(),
    };
    let api_key_id = SecretId::from_setting_id(&SettingId::new("providers.claude.apiKey").unwrap())
        .expect("secret id");
    secret_store
        .user(call)
        .set(
            &api_key_id,
            &SecretString::new("Bearer sk-ant-admin01-test-key"),
        )
        .await
        .expect("store api key");

    let config = config
        .with_admin_api_endpoint(endpoint)
        .with_workspace_spend_enabled(true)
        .with_secret_store(secret_store)
        .with_clock(move || now);
    let plan = ClaudeProviderPlan::new(config);
    let cx = make_context(SourceMode::Auto, BTreeMap::new(), None);
    let outcome = fetch_outcome(
        &ProviderId::new("claude").unwrap(),
        &plan,
        &cx,
        &NoopSleeper,
    )
    .await;

    let result = outcome.result.expect("admin-api fetch should succeed");
    assert_eq!(result.strategy_id, CLAUDE_ADMIN_API_STRATEGY_ID);
    assert_eq!(result.source_label, "admin-api");
    assert!((result.usage.provider_cost.as_ref().unwrap().used - 4.50).abs() < 0.0001);
    cost_mock.assert_async().await;
    messages_mock.assert_async().await;
}

#[tokio::test]
async fn fetch_usage_maps_401_to_unauthorized_and_500_to_http_error_message() {
    let server = https::start_server();

    server
        .mock_async(|when, then| {
            when.method(GET).path(COST_REPORT_PATH);
            then.status(401)
                .header("content-type", "application/json")
                .body(r#"{"error":{"message":"invalid x-api-key"}}"#);
        })
        .await;
    server
        .mock_async(|when, then| {
            when.method(GET).path(MESSAGES_REPORT_PATH);
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":[]}"#);
        })
        .await;

    let net = https::broker(NetConfig::default());
    let endpoint =
        ClaudeAdminAPIEndpointConfig::with_base_url_str(&server.base_url()).expect("loopback url");
    let now = "2026-03-21T15:30:00Z".parse::<Timestamp>().unwrap();
    let call = BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "err-401".to_owned(),
    };

    let err = ClaudeAdminAPIUsageFetcher::fetch_usage(
        &net,
        &endpoint,
        "sk-ant-admin01-bad",
        false,
        now,
        &call,
    )
    .await
    .expect_err("401 should fail");
    assert_eq!(err, ClaudeAdminAPIUsageError::Unauthorized);
    assert_eq!(
        err.into_classified_error().kind,
        ProviderErrorKind::AuthenticationExpired
    );

    let server_500 = https::start_server();
    server_500
        .mock_async(|when, then| {
            when.method(GET).path(COST_REPORT_PATH);
            then.status(503)
                .header("content-type", "application/json")
                .body(r#"{"error":{"message":"Organization cost report unavailable"}}"#);
        })
        .await;
    server_500
        .mock_async(|when, then| {
            when.method(GET).path(MESSAGES_REPORT_PATH);
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"data":[]}"#);
        })
        .await;

    let endpoint_500 = ClaudeAdminAPIEndpointConfig::with_base_url_str(&server_500.base_url())
        .expect("loopback url");
    let err_500 = ClaudeAdminAPIUsageFetcher::fetch_usage(
        &net,
        &endpoint_500,
        "sk-ant-admin01-ok",
        false,
        now,
        &call,
    )
    .await
    .expect_err("503 should fail");
    assert_eq!(
        err_500,
        ClaudeAdminAPIUsageError::HttpError {
            status_code: 503,
            message: Some("Organization cost report unavailable".to_owned()),
        }
    );
    let classified = err_500.into_classified_error();
    assert_eq!(classified.kind, ProviderErrorKind::ProviderUnavailable);
    assert_eq!(
        classified.message,
        "Anthropic Admin API returned HTTP 503: Organization cost report unavailable"
    );
}

#[test]
fn claude_env_descriptor_projects_config_api_key_and_scrubs_on_selected_account() {
    struct TestClaudeCfg {
        api_key: Option<String>,
    }
    let descriptor = claude_env_descriptor(|c: &TestClaudeCfg| c.api_key.clone());
    let mut base = BTreeMap::new();
    base.insert(
        "ANTHROPIC_ADMIN_KEY".to_owned(),
        SecretString::new("sk-ant-admin-ambient"),
    );
    base.insert(
        "ANTHROPIC_ADMIN_API_KEY".to_owned(),
        SecretString::new("sk-ant-admin-alt"),
    );
    base.insert(
        "ARIADUSAGE_CLAUDE_OAUTH_TOKEN".to_owned(),
        SecretString::new("sk-ant-oat-ambient"),
    );

    let cfg = TestClaudeCfg {
        api_key: Some("sk-ant-admin-config".to_owned()),
    };
    let resolved = ProviderEnvironmentResolver::resolve(&base, Some(&cfg), None, &descriptor);
    assert_eq!(
        resolved
            .get("ANTHROPIC_ADMIN_KEY")
            .map(SecretString::expose_secret),
        Some("sk-ant-admin-config")
    );

    let selected = SecretString::new("sk-ant-oat-selected");
    let scrubbed =
        ProviderEnvironmentResolver::resolve(&base, Some(&cfg), Some(&selected), &descriptor);
    assert!(!scrubbed.contains_key("ANTHROPIC_ADMIN_KEY"));
    assert!(!scrubbed.contains_key("ANTHROPIC_ADMIN_API_KEY"));
    assert!(!scrubbed.contains_key("ARIADUSAGE_CLAUDE_OAUTH_TOKEN"));
}
