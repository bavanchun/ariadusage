// Ported from CodexBar Tests/CodexBarTests/AntigravityStatusProbeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use ariadusage_engine::providers::antigravity::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// upstream: AntigravityStatusProbeTests.swift:345 connection candidates preserve scheme order and endpoint tokens
#[test]
fn connection_candidates_preserve_scheme_order_and_endpoint_tokens() {
    let candidates = AntigravityStatusProbe::connection_candidates(
        &[64440],
        Some("language-token"),
        Some(64432),
        Some("extension-token"),
    );
    assert_eq!(
        candidates,
        vec![
            AntigravityConnectionEndpoint::new(
                "https",
                64440,
                "language-token",
                EndpointSource::LanguageServer
            ),
            AntigravityConnectionEndpoint::new(
                "http",
                64432,
                "extension-token",
                EndpointSource::ExtensionServer
            ),
            AntigravityConnectionEndpoint::new(
                "http",
                64432,
                "language-token",
                EndpointSource::ExtensionServer
            )
        ]
    );
}

// upstream: AntigravityStatusProbeTests.swift:373 connection candidates restrict plain http probing to the declared extension port
#[test]
fn connection_candidates_restrict_plain_http_probing_to_the_declared_extension_port() {
    let candidates = AntigravityStatusProbe::connection_candidates(
        &[64440, 64441],
        Some("language-token"),
        Some(64432),
        None,
    );
    assert_eq!(
        candidates,
        vec![
            AntigravityConnectionEndpoint::new(
                "https",
                64440,
                "language-token",
                EndpointSource::LanguageServer
            ),
            AntigravityConnectionEndpoint::new(
                "https",
                64441,
                "language-token",
                EndpointSource::LanguageServer
            ),
            AntigravityConnectionEndpoint::new(
                "http",
                64432,
                "language-token",
                EndpointSource::ExtensionServer
            )
        ]
    );
}

// upstream: AntigravityStatusProbeTests.swift:401 connection candidates preserve extension fallback when extension token is unavailable
#[test]
fn connection_candidates_preserve_extension_fallback_when_extension_token_is_unavailable() {
    let candidates = AntigravityStatusProbe::connection_candidates(
        &[64440],
        Some("language-token"),
        Some(64432),
        None,
    );
    assert_eq!(
        candidates,
        vec![
            AntigravityConnectionEndpoint::new(
                "https",
                64440,
                "language-token",
                EndpointSource::LanguageServer
            ),
            AntigravityConnectionEndpoint::new(
                "http",
                64432,
                "language-token",
                EndpointSource::ExtensionServer
            )
        ]
    );
}

// upstream: AntigravityStatusProbeTests.swift:424 connection candidates do not duplicate the same http target when ports overlap
#[test]
fn connection_candidates_do_not_duplicate_the_same_http_target_when_ports_overlap() {
    let candidates = AntigravityStatusProbe::connection_candidates(
        &[64432],
        Some("language-token"),
        Some(64432),
        None,
    );
    assert_eq!(
        candidates,
        vec![
            AntigravityConnectionEndpoint::new(
                "https",
                64432,
                "language-token",
                EndpointSource::LanguageServer
            ),
            AntigravityConnectionEndpoint::new(
                "http",
                64432,
                "language-token",
                EndpointSource::ExtensionServer
            )
        ]
    );
}

// upstream: AntigravityStatusProbeTests.swift:447 request endpoints retry extension server after language server success
#[test]
fn request_endpoints_retry_extension_server_after_language_server_success() {
    let candidates = AntigravityStatusProbe::connection_candidates(
        &[64440],
        Some("language-token"),
        Some(64432),
        Some("extension-token"),
    );
    let resolved = AntigravityConnectionEndpoint::new(
        "https",
        64440,
        "language-token",
        EndpointSource::LanguageServer,
    );
    let endpoints = AntigravityStatusProbe::request_endpoints(&resolved, &candidates);
    assert_eq!(
        endpoints,
        vec![
            resolved,
            AntigravityConnectionEndpoint::new(
                "http",
                64432,
                "extension-token",
                EndpointSource::ExtensionServer
            ),
            AntigravityConnectionEndpoint::new(
                "http",
                64432,
                "language-token",
                EndpointSource::ExtensionServer
            )
        ]
    );
}

// upstream: AntigravityStatusProbeTests.swift:478 request endpoints preserve extension fallback when extension token is unavailable
#[test]
fn request_endpoints_preserve_extension_fallback_when_extension_token_is_unavailable() {
    let candidates = AntigravityStatusProbe::connection_candidates(
        &[64440],
        Some("language-token"),
        Some(64432),
        None,
    );
    let resolved = AntigravityConnectionEndpoint::new(
        "https",
        64440,
        "language-token",
        EndpointSource::LanguageServer,
    );
    let endpoints = AntigravityStatusProbe::request_endpoints(&resolved, &candidates);
    assert_eq!(
        endpoints,
        vec![
            resolved,
            AntigravityConnectionEndpoint::new(
                "http",
                64432,
                "language-token",
                EndpointSource::ExtensionServer
            )
        ]
    );
}

// upstream: AntigravityStatusProbeTests.swift:504 request endpoints retry alternate token after extension server wins discovery
#[test]
fn request_endpoints_retry_alternate_token_after_extension_server_wins_discovery() {
    let candidates = AntigravityStatusProbe::connection_candidates(
        &[64440],
        Some("language-token"),
        Some(64432),
        Some("extension-token"),
    );
    let resolved = AntigravityConnectionEndpoint::new(
        "http",
        64432,
        "extension-token",
        EndpointSource::ExtensionServer,
    );
    let endpoints = AntigravityStatusProbe::request_endpoints(&resolved, &candidates);
    assert_eq!(
        endpoints,
        vec![
            resolved,
            AntigravityConnectionEndpoint::new(
                "http",
                64432,
                "language-token",
                EndpointSource::ExtensionServer
            ),
            AntigravityConnectionEndpoint::new(
                "https",
                64440,
                "language-token",
                EndpointSource::LanguageServer
            )
        ]
    );
}

// upstream: AntigravityStatusProbeTests.swift:535 request endpoints keep https language server fallback after extension probe wins
#[test]
fn request_endpoints_keep_https_language_server_fallback_after_extension_probe_wins() {
    let candidates = AntigravityStatusProbe::connection_candidates(
        &[64432, 64440],
        Some("language-token"),
        Some(64432),
        None,
    );
    let resolved = AntigravityConnectionEndpoint::new(
        "http",
        64432,
        "language-token",
        EndpointSource::ExtensionServer,
    );
    let endpoints = AntigravityStatusProbe::request_endpoints(&resolved, &candidates);
    assert_eq!(
        endpoints,
        vec![
            resolved,
            AntigravityConnectionEndpoint::new(
                "https",
                64432,
                "language-token",
                EndpointSource::LanguageServer
            ),
            AntigravityConnectionEndpoint::new(
                "https",
                64440,
                "language-token",
                EndpointSource::LanguageServer
            )
        ]
    );
}

// upstream: AntigravityStatusProbeTests.swift:613 endpoint resolver prefers successful https language server candidate
#[tokio::test]
async fn endpoint_resolver_prefers_successful_https_language_server_candidate() {
    let candidates = AntigravityStatusProbe::connection_candidates(
        &[64440],
        Some("language-token"),
        Some(64432),
        Some("extension-token"),
    );
    let attempted = Arc::new(Mutex::new(Vec::new()));
    let recorder = attempted.clone();
    let endpoint = AntigravityStatusProbe::resolve_working_endpoint(
        &candidates,
        &OperationDeadline::new(Duration::from_secs(1)),
        move |endpoint, _| {
            let recorder = recorder.clone();
            async move {
                recorder.lock().unwrap().push(endpoint.clone());
                if endpoint.scheme == "https" && endpoint.port == 64440 {
                    Ok(())
                } else {
                    Err(AntigravityStatusProbeError::ApiError("unreachable".into()))
                }
            }
        },
    )
    .await
    .unwrap();
    assert_eq!(endpoint, candidates[0]);
    assert_eq!(*attempted.lock().unwrap(), candidates[..1]);
}

// upstream: AntigravityStatusProbeTests.swift:634 endpoint resolver falls back to extension server after https language server candidates
#[tokio::test]
async fn endpoint_resolver_falls_back_to_extension_server_after_https_language_server_candidates() {
    let candidates = AntigravityStatusProbe::connection_candidates(
        &[64440, 64441],
        Some("language-token"),
        Some(64432),
        Some("extension-token"),
    );
    let attempted = Arc::new(Mutex::new(Vec::new()));
    let recorder = attempted.clone();
    let endpoint = AntigravityStatusProbe::resolve_working_endpoint(
        &candidates,
        &OperationDeadline::new(Duration::from_secs(1)),
        move |endpoint, _| {
            let recorder = recorder.clone();
            async move {
                recorder.lock().unwrap().push(endpoint.clone());
                if endpoint.scheme == "http"
                    && endpoint.port == 64432
                    && endpoint.source == EndpointSource::ExtensionServer
                {
                    Ok(())
                } else {
                    Err(AntigravityStatusProbeError::ApiError("unreachable".into()))
                }
            }
        },
    )
    .await
    .unwrap();
    assert_eq!(endpoint, candidates[2]);
    assert_eq!(*attempted.lock().unwrap(), candidates[..3]);
}

// upstream: AntigravityStatusProbeTests.swift:655 endpoint resolver falls back to alternate extension token after primary token fails
#[tokio::test]
async fn endpoint_resolver_falls_back_to_alternate_extension_token_after_primary_token_fails() {
    let candidates = AntigravityStatusProbe::connection_candidates(
        &[64440],
        Some("language-token"),
        Some(64432),
        Some("extension-token"),
    );
    let attempted = Arc::new(Mutex::new(Vec::new()));
    let recorder = attempted.clone();
    let endpoint = AntigravityStatusProbe::resolve_working_endpoint(
        &candidates,
        &OperationDeadline::new(Duration::from_secs(1)),
        move |endpoint, _| {
            let recorder = recorder.clone();
            async move {
                recorder.lock().unwrap().push(endpoint.clone());
                if endpoint.source == EndpointSource::ExtensionServer
                    && endpoint.csrf_token == "language-token"
                {
                    Ok(())
                } else {
                    Err(AntigravityStatusProbeError::ApiError("unreachable".into()))
                }
            }
        },
    )
    .await
    .unwrap();
    assert_eq!(endpoint, candidates[2]);
    assert_eq!(*attempted.lock().unwrap(), candidates[..3]);
    assert_eq!(endpoint.csrf_token, "language-token");
}

// upstream: AntigravityStatusProbeTests.swift:566 parsed request retries later endpoints after api level error payload
#[tokio::test]
async fn parsed_request_retries_later_endpoints_after_api_level_error_payload() {
    let endpoints = vec![
        AntigravityConnectionEndpoint::new(
            "https",
            64440,
            "bad-token",
            EndpointSource::LanguageServer,
        ),
        AntigravityConnectionEndpoint::new(
            "http",
            64432,
            "good-token",
            EndpointSource::ExtensionServer,
        ),
    ];
    let attempted = Arc::new(Mutex::new(Vec::new()));
    let recorder = attempted.clone();
    let send: EndpointRequestSender = Arc::new(move |_, endpoint, _| {
        let recorder = recorder.clone();
        Box::pin(async move {
            recorder.lock().unwrap().push(endpoint.clone());
            if endpoint.csrf_token == "bad-token" {
                Ok(br#"{"code":16}"#.to_vec())
            } else {
                Ok(br#"{"code":0,"userStatus":{"email":"test@example.com","cascadeModelConfigData":{"clientModelConfigs":[]}}}"#.to_vec())
            }
        })
    });
    let snapshot = AntigravityStatusProbe::make_parsed_request(
        RequestPayload::new(
            "/exa.language_server_pb.LanguageServerService/GetUserStatus",
            serde_json::json!({
                "metadata":{
                }
            }),
        ),
        &RequestContext {
            endpoints: endpoints.clone(),
            timeout: Duration::from_secs(1),
        },
        &OperationDeadline::new(Duration::from_secs(1)),
        send,
        AntigravityStatusProbe::parse_user_status_response,
    )
    .await
    .unwrap();
    assert_eq!(snapshot.account_email.as_deref(), Some("test@example.com"));
    assert_eq!(*attempted.lock().unwrap(), endpoints);
}
