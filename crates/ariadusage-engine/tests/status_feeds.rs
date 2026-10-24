// Ported from CodexBar Tests/CodexBarTests/ProviderStatusFetchTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatuspageSummaryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/GoogleWorkspaceStatusTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ProviderStatusRefreshPublicationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#[path = "support/https.rs"]
mod https;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ariadusage_core::providers::find_by_id_str;
use ariadusage_engine::brokers::net::{DeclaredOrigins, NetConfig};
use ariadusage_engine::brokers::secret_store::SecretStore;
use ariadusage_engine::brokers::secret_store::memory::MemoryBackend;
use ariadusage_engine::{
    ConfigStore, EngineRuntime, ProviderRegistry, ProviderStatusFetcher, ProviderStatusSummary,
    StatusFetchError, status_declared_hosts,
};
use ariadusage_protocol::usage::StatusIndicator;
use ariadusage_protocol::{ProviderId, ProviderStatus};
use httpmock::Method::GET;
use reqwest::Url;
use tempfile::TempDir;
use tokio::sync::Notify;

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

// upstream: Tests/CodexBarTests/StatuspageSummaryTests.swift:121 fetchStatusSummary overlays description and updatedAt when incident io succeeds
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

// upstream: Tests/CodexBarTests/StatuspageSummaryTests.swift:95 fetchStatusSummary returns status with empty components when component feed fails
#[tokio::test]
async fn fetch_status_summary_returns_status_with_empty_components_when_component_feed_fails() {
    let server = https::start_server();
    let base_url = Url::parse(&server.base_url()).expect("server base url");

    let _proxy = server.mock(|when, then| {
        when.method(GET).path("/proxy/status.openai.com");
        then.status(404);
    });
    let _summary = server.mock(|when, then| {
        when.method(GET).path("/api/v2/summary.json");
        then.status(200)
            .header("content-type", "application/json")
            .body(
                r#"{
                    "page": {"updated_at": "2026-06-18T19:41:22Z"},
                    "status": {"indicator": "minor", "description": "Partial Outage"}
                }"#,
            );
    });
    let _components = server.mock(|when, then| {
        when.method(GET).path("/api/v2/components.json");
        then.status(503);
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

    assert_eq!(summary.status.indicator, StatusIndicator::Minor);
    assert_eq!(
        summary.status.description.as_deref(),
        Some("Partial Outage")
    );
    assert!(summary.components.is_empty());
}

fn make_status_summary(indicator: StatusIndicator, description: &str) -> ProviderStatusSummary {
    ProviderStatusSummary::new(
        ProviderStatus {
            indicator,
            description: Some(description.to_string()),
            updated_at: None,
            page_url: None,
            components: Vec::new(),
        },
        Vec::new(),
    )
}

fn make_status_runtime(tmp: &TempDir, fetcher: ProviderStatusFetcher) -> Arc<EngineRuntime> {
    let config_store = ConfigStore::new(tmp.path().join("config.json"));
    let codex_id = ProviderId::new("codex").unwrap();
    let claude_id = ProviderId::new("claude").unwrap();
    config_store
        .update(|cfg| {
            ariadusage_core::config::set_provider_enabled(cfg, &codex_id, true);
            ariadusage_core::config::set_provider_enabled(cfg, &claude_id, true);
        })
        .expect("enable codex and claude");
    let secret_store = Arc::new(SecretStore::new(MemoryBackend::new()));
    let net = Arc::new(https::broker(NetConfig::default()));
    Arc::new(
        EngineRuntime::new(config_store, secret_store, net, ProviderRegistry::empty())
            .with_status_fetcher(fetcher),
    )
}

// upstream: Tests/CodexBarTests/ProviderStatusRefreshPublicationTests.swift:8 late scoped status cannot replace a newer background status
#[tokio::test]
async fn late_scoped_status_cannot_replace_a_newer_background_status() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_id = ProviderId::new("codex").unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let s1 = Arc::clone(&started);
    let r1 = Arc::clone(&release);
    let c1 = Arc::clone(&calls);
    let fetcher = ProviderStatusFetcher::new().with_fetch_override(Arc::new(move |_id| {
        let req = c1.fetch_add(1, Ordering::SeqCst) + 1;
        let s1 = Arc::clone(&s1);
        let r1 = Arc::clone(&r1);
        Box::pin(async move {
            if req == 1 {
                s1.notify_waiters();
                r1.notified().await;
                Ok(Some(make_status_summary(
                    StatusIndicator::None,
                    "Older operational status",
                )))
            } else {
                Ok(Some(make_status_summary(
                    StatusIndicator::Major,
                    "Current incident",
                )))
            }
        })
    }));
    let runtime = make_status_runtime(&tmp, fetcher);
    let rt_older = Arc::clone(&runtime);
    let codex1 = codex_id.clone();
    let older_task = tokio::spawn(async move { rt_older.refresh_provider_status(codex1).await });
    started.notified().await;

    let newer_status = runtime
        .refresh_provider_status(codex_id.clone())
        .await
        .expect("newer status succeeds")
        .expect("status");
    assert_eq!(
        newer_status.description.as_deref(),
        Some("Current incident")
    );

    release.notify_waiters();
    let older_res = older_task
        .await
        .unwrap()
        .expect("older finishes")
        .expect("status");
    assert_eq!(older_res.description.as_deref(), Some("Current incident"));
    let snap = runtime.current_snapshot();
    let codex_snap = snap.providers.iter().find(|p| p.id == codex_id).unwrap();
    assert_eq!(
        codex_snap
            .status
            .as_ref()
            .unwrap()
            .value
            .as_ref()
            .unwrap()
            .description
            .as_deref(),
        Some("Current incident")
    );
}

// upstream: Tests/CodexBarTests/ProviderStatusRefreshPublicationTests.swift:47 a newer request can publish after an overlapping older success
#[tokio::test]
async fn a_newer_request_can_publish_after_an_overlapping_older_success() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_id = ProviderId::new("codex").unwrap();
    let started1 = Arc::new(Notify::new());
    let release1 = Arc::new(Notify::new());
    let started2 = Arc::new(Notify::new());
    let release2 = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let s1 = Arc::clone(&started1);
    let r1 = Arc::clone(&release1);
    let s2 = Arc::clone(&started2);
    let r2 = Arc::clone(&release2);
    let c = Arc::clone(&calls);
    let fetcher = ProviderStatusFetcher::new().with_fetch_override(Arc::new(move |_id| {
        let req = c.fetch_add(1, Ordering::SeqCst) + 1;
        let s1 = Arc::clone(&s1);
        let r1 = Arc::clone(&r1);
        let s2 = Arc::clone(&s2);
        let r2 = Arc::clone(&r2);
        Box::pin(async move {
            if req == 1 {
                s1.notify_waiters();
                r1.notified().await;
                Ok(Some(make_status_summary(
                    StatusIndicator::Minor,
                    "Older degraded status",
                )))
            } else {
                s2.notify_waiters();
                r2.notified().await;
                Ok(Some(make_status_summary(
                    StatusIndicator::Critical,
                    "Newer outage status",
                )))
            }
        })
    }));
    let runtime = make_status_runtime(&tmp, fetcher);
    let rt1 = Arc::clone(&runtime);
    let cid1 = codex_id.clone();
    let older_task = tokio::spawn(async move { rt1.refresh_provider_status(cid1).await });
    started1.notified().await;

    let rt2 = Arc::clone(&runtime);
    let cid2 = codex_id.clone();
    let newer_task = tokio::spawn(async move { rt2.refresh_provider_status(cid2).await });
    started2.notified().await;

    release1.notify_waiters();
    let older = older_task.await.unwrap().unwrap().unwrap();
    assert_eq!(older.description.as_deref(), Some("Older degraded status"));

    release2.notify_waiters();
    let newer = newer_task.await.unwrap().unwrap().unwrap();
    assert_eq!(newer.description.as_deref(), Some("Newer outage status"));
}

// upstream: Tests/CodexBarTests/ProviderStatusRefreshPublicationTests.swift:77 an older success remains useful when a newer request fails
#[tokio::test]
async fn an_older_success_remains_useful_when_a_newer_request_fails() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_id = ProviderId::new("codex").unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let s2 = Arc::clone(&started);
    let r2 = Arc::clone(&release);
    let c2 = Arc::clone(&calls);
    let fetcher = ProviderStatusFetcher::new().with_fetch_override(Arc::new(move |_id| {
        let req = c2.fetch_add(1, Ordering::SeqCst) + 1;
        let s2 = Arc::clone(&s2);
        let r2 = Arc::clone(&r2);
        Box::pin(async move {
            if req == 1 {
                s2.notify_waiters();
                r2.notified().await;
                Ok(Some(make_status_summary(
                    StatusIndicator::Major,
                    "Available incident",
                )))
            } else {
                Err(StatusFetchError::Http { status: 503 })
            }
        })
    }));
    let runtime = make_status_runtime(&tmp, fetcher);
    let rt_older = Arc::clone(&runtime);
    let codex2 = codex_id.clone();
    let older = tokio::spawn(async move { rt_older.refresh_provider_status(codex2).await });
    started.notified().await;

    assert!(
        runtime
            .refresh_provider_status(codex_id.clone())
            .await
            .is_err()
    );
    release.notify_waiters();
    let older_status = older
        .await
        .unwrap()
        .expect("older succeeds")
        .expect("status");
    assert_eq!(
        older_status.description.as_deref(),
        Some("Available incident")
    );
}

// upstream: Tests/CodexBarTests/ProviderStatusRefreshPublicationTests.swift:102 an older status failure cannot request a retry after newer success
#[tokio::test]
async fn an_older_status_failure_cannot_request_a_retry_after_newer_success() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_id = ProviderId::new("codex").unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let s = Arc::clone(&started);
    let r = Arc::clone(&release);
    let c = Arc::clone(&calls);
    let fetcher = ProviderStatusFetcher::new().with_fetch_override(Arc::new(move |_id| {
        let req = c.fetch_add(1, Ordering::SeqCst) + 1;
        let s = Arc::clone(&s);
        let r = Arc::clone(&r);
        Box::pin(async move {
            if req == 1 {
                s.notify_waiters();
                r.notified().await;
                Err(StatusFetchError::Http { status: 503 })
            } else {
                Ok(Some(make_status_summary(
                    StatusIndicator::Major,
                    "Current incident",
                )))
            }
        })
    }));
    let runtime = make_status_runtime(&tmp, fetcher);
    let rt_older = Arc::clone(&runtime);
    let cid = codex_id.clone();
    let older = tokio::spawn(async move { rt_older.refresh_provider_status(cid).await });
    started.notified().await;

    let newer = runtime
        .refresh_provider_status(codex_id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(newer.description.as_deref(), Some("Current incident"));

    release.notify_waiters();
    assert!(older.await.unwrap().is_err());

    let snap = runtime.current_snapshot();
    let codex_snap = snap.providers.iter().find(|p| p.id == codex_id).unwrap();
    assert_eq!(
        codex_snap
            .status
            .as_ref()
            .unwrap()
            .value
            .as_ref()
            .unwrap()
            .description
            .as_deref(),
        Some("Current incident")
    );
}

// upstream: Tests/CodexBarTests/ProviderStatusRefreshPublicationTests.swift:127 status publication ordering is independent for each provider
#[tokio::test]
async fn status_publication_ordering_is_independent_for_each_provider() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_id = ProviderId::new("codex").unwrap();
    let claude_id = ProviderId::new("claude").unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let s3 = Arc::clone(&started);
    let r3 = Arc::clone(&release);
    let fetcher = ProviderStatusFetcher::new().with_fetch_override(Arc::new(move |id| {
        let s3 = Arc::clone(&s3);
        let r3 = Arc::clone(&r3);
        Box::pin(async move {
            if id.as_str() == "codex" {
                s3.notify_waiters();
                r3.notified().await;
                Ok(Some(make_status_summary(StatusIndicator::Major, "codex")))
            } else {
                Ok(Some(make_status_summary(StatusIndicator::Major, "claude")))
            }
        })
    }));
    let runtime = make_status_runtime(&tmp, fetcher);
    let rt_codex = Arc::clone(&runtime);
    let codex3 = codex_id.clone();
    let codex_task = tokio::spawn(async move { rt_codex.refresh_provider_status(codex3).await });
    started.notified().await;

    let claude_st = runtime
        .refresh_provider_status(claude_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claude_st.description.as_deref(), Some("claude"));

    release.notify_waiters();
    let codex_st = codex_task.await.unwrap().unwrap().unwrap();
    assert_eq!(codex_st.description.as_deref(), Some("codex"));
}

// upstream: Tests/CodexBarTests/ProviderStatusRefreshPublicationTests.swift:148 a later successful request can report a genuine recovery
#[tokio::test]
async fn a_later_successful_request_can_report_a_genuine_recovery() {
    let tmp = TempDir::new().expect("tempdir");
    let codex_id = ProviderId::new("codex").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&calls);
    let fetcher = ProviderStatusFetcher::new().with_fetch_override(Arc::new(move |_id| {
        let n = c.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            if n == 1 {
                Ok(Some(make_status_summary(
                    StatusIndicator::Major,
                    "Current incident",
                )))
            } else {
                Ok(Some(make_status_summary(
                    StatusIndicator::None,
                    "Recovered",
                )))
            }
        })
    }));
    let runtime = make_status_runtime(&tmp, fetcher);
    let first = runtime
        .refresh_provider_status(codex_id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.indicator, StatusIndicator::Major);

    let recovered = runtime
        .refresh_provider_status(codex_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.indicator, StatusIndicator::None);
    assert_eq!(recovered.description.as_deref(), Some("Recovered"));
}
