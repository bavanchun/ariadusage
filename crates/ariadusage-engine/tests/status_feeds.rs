// Ported from CodexBar Tests/CodexBarTests/ProviderStatusFetchTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatuspageSummaryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ProviderStatusRefreshPublicationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#[path = "support/https.rs"]
mod https;

use std::sync::Arc;

use ariadusage_core::providers::find_by_id_str;
use ariadusage_engine::brokers::net::{DeclaredOrigins, NetConfig};
use ariadusage_engine::brokers::secret_store::SecretStore;
use ariadusage_engine::brokers::secret_store::memory::MemoryBackend;
use ariadusage_engine::{
    ConfigStore, EngineRuntime, ProviderRegistry, ProviderStatusFetcher, StatusFetchError,
    status_declared_hosts,
};
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::usage::StatusIndicator;
use httpmock::Method::GET;
use reqwest::Url;
use tempfile::TempDir;

#[test]
fn declared_status_hosts_cover_first_party_descriptors() {
    let hosts = status_declared_hosts();
    let origins = DeclaredOrigins::from_provider_hosts(&hosts);
    for id in ["codex", "claude", "antigravity"] {
        let descriptor = find_by_id_str(id).expect("first-party descriptor");
        assert!(descriptor.supports_status_polling());
        if let Some(raw_url) = descriptor.status_page_url {
            let url = Url::parse(raw_url).expect("valid status page url");
            assert!(origins.contains_url(&url));
        }
    }
    let workspace_url =
        Url::parse(ariadusage_engine::GOOGLE_WORKSPACE_INCIDENTS_URL).expect("valid workspace url");
    assert!(origins.contains_url(&workspace_url));
}

#[tokio::test]
async fn fetch_status_summary_uses_incident_io_proxy_and_overlays_statuspage_status() {
    let server = https::start_server();
    let base_url = Url::parse(&server.base_url()).expect("server base url");

    let proxy_mock = server.mock(|when, then| {
        when.method(GET).path("/proxy/status.claude.com");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{
                    "summary": {
                        "ongoing_incidents": [],
                        "scheduled_maintenances": [],
                        "affected_components": [
                            {
                                "component_id": "comp-1",
                                "status": "under_maintenance"
                            },
                            {
                                "component_id": "comp-2",
                                "status": "partial_outage"
                            }
                        ],
                        "structure": {
                            "items": [
                                {
                                    "group": {
                                        "id": "grp-core",
                                        "name": "Core Services",
                                        "hidden": false,
                                        "components": [
                                            {
                                                "id": "comp-1",
                                                "name": "Claude API",
                                                "hidden": false
                                            }
                                        ]
                                    },
                                    "component": null
                                },
                                {
                                    "group": null,
                                    "component": {
                                        "id": "comp-2",
                                        "name": "claude.ai",
                                        "hidden": false
                                    }
                                }
                            ]
                        }
                    }
                }"#,
            );
    });

    let status_mock = server.mock(|when, then| {
        when.method(GET).path("/api/v2/status.json");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{
                    "page": { "updated_at": "2026-03-04T09:15:00Z" },
                    "status": {
                        "indicator": "major",
                        "description": "Partially Degraded Service"
                    }
                }"#,
            );
    });

    let net = https::broker(NetConfig::default());
    let fetcher = ProviderStatusFetcher::new()
        .with_declared_origins(https::declared(&base_url))
        .with_provider_base_url("claude", base_url.clone())
        .with_provider_proxy_host("claude", "status.claude.com");

    let claude_desc = find_by_id_str("claude").expect("claude descriptor");
    let summary = fetcher
        .fetch_for_descriptor(&net, &https::call(), claude_desc)
        .await
        .expect("fetch succeeds")
        .expect("summary present");

    proxy_mock.assert();
    status_mock.assert();

    assert_eq!(summary.status.indicator, StatusIndicator::Major);
    assert_eq!(
        summary.status.description.as_deref(),
        Some("Partially Degraded Service")
    );
    assert_eq!(
        summary.status.updated_at,
        Some("2026-03-04T09:15:00Z".parse().unwrap())
    );
    assert_eq!(summary.components.len(), 2);
    assert_eq!(summary.components[0].name, "Core Services");
    assert_eq!(summary.components[0].status, "under_maintenance");
    assert_eq!(summary.components[0].children.len(), 1);
    assert_eq!(summary.components[0].children[0].name, "Claude API");
    assert_eq!(summary.components[1].name, "claude.ai");
    assert_eq!(summary.components[1].status, "partial_outage");
}

#[tokio::test]
async fn fetch_status_summary_falls_back_to_statuspage_summary_and_components() {
    let server = https::start_server();
    let base_url = Url::parse(&server.base_url()).expect("server base url");

    let proxy_mock = server.mock(|when, then| {
        when.method(GET).path("/proxy/status.openai.com");
        then.status(404);
    });

    let summary_mock = server.mock(|when, then| {
        when.method(GET).path("/api/v2/summary.json");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{
                    "page": { "updated_at": "2026-03-04T10:00:00Z" },
                    "status": {
                        "indicator": "minor",
                        "description": "Minor Service Outage"
                    }
                }"#,
            );
    });

    let components_mock = server.mock(|when, then| {
        when.method(GET).path("/api/v2/components.json");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{
                    "components": [
                        {
                            "id": "grp-1",
                            "name": "API",
                            "status": "operational",
                            "group": true
                        },
                        {
                            "id": "cmp-1",
                            "name": "Responses",
                            "status": "partial_outage",
                            "group_id": "grp-1"
                        },
                        {
                            "id": "cmp-2",
                            "name": "Dashboard",
                            "status": "operational"
                        }
                    ]
                }"#,
            );
    });

    let net = https::broker(NetConfig::default());
    let fetcher = ProviderStatusFetcher::new()
        .with_declared_origins(https::declared(&base_url))
        .with_provider_base_url("codex", base_url.clone())
        .with_provider_proxy_host("codex", "status.openai.com");

    let codex_desc = find_by_id_str("codex").expect("codex descriptor");
    let summary = fetcher
        .fetch_for_descriptor(&net, &https::call(), codex_desc)
        .await
        .expect("fetch succeeds")
        .expect("summary present");

    proxy_mock.assert();
    summary_mock.assert();
    components_mock.assert();

    assert_eq!(summary.status.indicator, StatusIndicator::Minor);
    assert_eq!(
        summary.status.description.as_deref(),
        Some("Minor Service Outage")
    );
    assert_eq!(summary.components.len(), 2);
    assert_eq!(summary.components[0].name, "API");
    assert_eq!(summary.components[0].children.len(), 1);
    assert_eq!(summary.components[0].children[0].name, "Responses");
    assert_eq!(summary.components[0].children[0].status, "partial_outage");
}

#[tokio::test]
async fn fetch_workspace_status_for_antigravity_maps_active_incidents() {
    let server = https::start_server();
    let base_url = Url::parse(&server.base_url()).expect("server base url");
    let incidents_url = base_url
        .join("appsstatus/dashboard/incidents.json")
        .expect("incidents url");

    let ws_mock = server.mock(|when, then| {
        when.method(GET)
            .path("/appsstatus/dashboard/incidents.json");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"[
                    {
                        "begin": "2026-03-04T08:00:00Z",
                        "end": null,
                        "external_desc": "Gemini Code Assist latency",
                        "currently_affected_products": [
                            { "id": "npdyhgECDJ6tB66MxXyo", "title": "Gemini" }
                        ],
                        "most_recent_update": {
                            "status": "SERVICE_DISRUPTION",
                            "text": "**Summary**\n- Engineers are mitigating elevated latency.",
                            "when": "2026-03-04T08:30:00Z"
                        }
                    }
                ]"#,
            );
    });

    let net = https::broker(NetConfig::default());
    let fetcher = ProviderStatusFetcher::new()
        .with_declared_origins(https::declared(&base_url))
        .with_workspace_incidents_url(incidents_url);

    let agy_desc = find_by_id_str("antigravity").expect("antigravity descriptor");
    let summary = fetcher
        .fetch_for_descriptor(&net, &https::call(), agy_desc)
        .await
        .expect("workspace fetch succeeds")
        .expect("summary present");

    ws_mock.assert();
    assert_eq!(summary.status.indicator, StatusIndicator::Major);
    assert_eq!(
        summary.status.description.as_deref(),
        Some("Engineers are mitigating elevated latency.")
    );
    assert_eq!(
        summary.status.updated_at,
        Some("2026-03-04T08:30:00Z".parse().unwrap())
    );
}

#[tokio::test]
async fn fetch_status_rejects_undeclared_origin() {
    let server = https::start_server();
    let base_url = Url::parse(&server.base_url()).expect("server base url");
    let net = https::broker(NetConfig::default());

    // Default fetcher only allows status.claude.com, status.anthropic.com, status.openai.com, www.google.com
    let fetcher = ProviderStatusFetcher::new().with_provider_base_url("claude", base_url);
    let claude_desc = find_by_id_str("claude").expect("claude descriptor");
    let err = fetcher
        .fetch_for_descriptor(&net, &https::call(), claude_desc)
        .await
        .expect_err("undeclared local server origin must be rejected");
    assert_eq!(err, StatusFetchError::OriginNotDeclared);
}

#[tokio::test]
async fn runtime_status_refresh_preserves_last_good_status_when_newer_poll_fails() {
    // Ported from ProviderStatusRefreshPublicationTests.swift
    let server = https::start_server();
    let base_url = Url::parse(&server.base_url()).expect("server base url");
    let tmp = TempDir::new().expect("tempdir");
    let config_store = ConfigStore::new(tmp.path().join("config.json"));
    let claude_id = ProviderId::new("claude").unwrap();
    config_store
        .update(|cfg| {
            ariadusage_core::config::set_provider_enabled(cfg, &claude_id, true);
        })
        .expect("enable claude");
    let secret_store = Arc::new(SecretStore::new(MemoryBackend::new()));
    let net = Arc::new(https::broker(NetConfig::default()));

    let mut proxy_mock = server.mock(|when, then| {
        when.method(GET).path("/proxy/status.claude.com");
        then.status(404);
    });
    let mut summary_mock = server.mock(|when, then| {
        when.method(GET).path("/api/v2/summary.json");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{
                    "page": { "updated_at": "2026-03-04T11:00:00Z" },
                    "status": {
                        "indicator": "minor",
                        "description": "Elevated API error rate"
                    }
                }"#,
            );
    });
    let mut components_mock = server.mock(|when, then| {
        when.method(GET).path("/api/v2/components.json");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{
                    "components": [
                        {
                            "id": "cmp-api",
                            "name": "Claude API",
                            "status": "degraded_performance"
                        }
                    ]
                }"#,
            );
    });

    let fetcher = ProviderStatusFetcher::new()
        .with_declared_origins(https::declared(&base_url))
        .with_provider_base_url("claude", base_url.clone())
        .with_provider_proxy_host("claude", "status.claude.com");

    let runtime = EngineRuntime::new(config_store, secret_store, net, ProviderRegistry::empty())
        .with_status_fetcher(fetcher);

    let claude_id = ProviderId::new("claude").unwrap();
    let status = runtime
        .refresh_provider_status(claude_id.clone())
        .await
        .expect("initial status poll succeeds")
        .expect("status present");
    assert_eq!(status.indicator, StatusIndicator::Minor);
    assert_eq!(
        status.page_url.as_deref(),
        Some("https://status.claude.com/")
    );
    assert_eq!(runtime.provider_status_components(&claude_id).len(), 1);

    // Replace server mocks with 503 failure and verify last-good status + components remain intact.
    proxy_mock.delete();
    summary_mock.delete();
    components_mock.delete();

    let _fail_proxy = server.mock(|when, then| {
        when.method(GET).path("/proxy/status.claude.com");
        then.status(503);
    });
    let _fail_summary = server.mock(|when, then| {
        when.method(GET).path("/api/v2/summary.json");
        then.status(503);
    });

    let err = runtime
        .refresh_provider_status(claude_id.clone())
        .await
        .expect_err("second status poll fails");
    assert_eq!(err, StatusFetchError::Http { status: 503 });

    let snap = runtime.current_snapshot();
    let claude_snap = snap
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .expect("claude snapshot");
    let kept_status = claude_snap
        .status
        .as_ref()
        .expect("status metric preserved")
        .value
        .as_ref()
        .expect("status value preserved");
    assert_eq!(kept_status.indicator, StatusIndicator::Minor);
    assert_eq!(
        kept_status.description.as_deref(),
        Some("Elevated API error rate")
    );
    assert_eq!(runtime.provider_status_components(&claude_id).len(), 1);
}
