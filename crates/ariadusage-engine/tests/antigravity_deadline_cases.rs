// Ported from CodexBar Tests/CodexBarTests/AntigravityDeadlineTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use ariadusage_core::pipeline::TransportClass;
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::providers::antigravity::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
#[path = "support/https.rs"]
mod https;
fn process(pid: i32, token: &str, command: &str) -> ProcessInfoResult {
    ProcessInfoResult {
        pid,
        extension_port: None,
        extension_csrf_token: None,
        csrf_token: Some(token.into()),
        command: command.into(),
        identity: None,
    }
}
fn processes() -> Vec<ProcessInfoResult> {
    vec![process(1, "first", "first"), process(2, "second", "second")]
}
fn partial() -> AntigravityStatusSnapshot {
    AntigravityStatusSnapshot::new(vec![], Some("partial@example.com".into()), None)
}

// upstream: AntigravityDeadlineTests.swift:48 process candidates probe concurrently while preserving result order
#[tokio::test]
async fn process_candidates_probe_concurrently_while_preserving_result_order() {
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let result = AntigravityStatusProbe::fetch_process_snapshots(
        &processes(),
        &OperationDeadline::new(Duration::from_secs(4)),
        {
            let active = active.clone();
            let maximum = maximum.clone();
            move |process| {
                let active = active.clone();
                let maximum = maximum.clone();
                async move {
                    maximum.fetch_max(active.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(if process.pid == 1 {
                        120
                    } else {
                        20
                    }))
                    .await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(AntigravityStatusSnapshot::new(
                        vec![],
                        Some(format!("{}@example.com", process.pid)),
                        None,
                    ))
                }
            }
        },
    )
    .await
    .unwrap();
    assert_eq!(maximum.load(Ordering::SeqCst), 2);
    assert_eq!(
        result
            .iter()
            .map(|s| s.account_email.as_deref())
            .collect::<Vec<_>>(),
        [Some("1@example.com"), Some("2@example.com")]
    );
}

// upstream: AntigravityDeadlineTests.swift:87 process candidate transport error preserves url error identity
#[tokio::test]
async fn process_candidate_transport_error_preserves_url_error_identity() {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let endpoint = AntigravityConnectionEndpoint::new(
        "http",
        socket.local_addr().unwrap().port(),
        "",
        EndpointSource::LanguageServer,
    );
    let net = NetBroker::new(NetConfig::default()).unwrap();
    let error = AntigravityStatusProbe::send_loopback_request(
        &net,
        &https::call(),
        &RequestPayload::new("/closed", serde_json::json!({})),
        &endpoint,
        Duration::from_secs(1),
    )
    .await
    .unwrap_err();
    let result = AntigravityStatusProbe::fetch_process_snapshots(
        &[process(1, "token", "command")],
        &OperationDeadline::new(Duration::from_secs(4)),
        move |_| {
            let error = error.clone();
            async move { Err(error) }
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        result.into_classified_error().transport,
        Some(TransportClass::CannotConnect)
    );
}

// upstream: AntigravityDeadlineTests.swift:103 process candidate cancellation rejects partial success
#[tokio::test]
async fn process_candidate_cancellation_rejects_partial_success() {
    let result = AntigravityStatusProbe::fetch_process_snapshots(
        &processes(),
        &OperationDeadline::new(Duration::from_secs(4)),
        |process| async move {
            if process.pid == 2 {
                Err(AntigravityStatusProbeError::Cancelled)
            } else {
                Ok(partial())
            }
        },
    )
    .await;
    assert_eq!(result.unwrap_err(), AntigravityStatusProbeError::Cancelled);
}

// upstream: AntigravityDeadlineTests.swift:133 cancelled process request rejects partial success
#[tokio::test]
async fn cancelled_process_request_rejects_partial_success() {
    let call = https::call();
    call.cancel.cancel();
    let net = NetBroker::new(NetConfig::default()).unwrap();
    let cancelled = AntigravityStatusProbe::send_loopback_request(
        &net,
        &call,
        &RequestPayload::new("/cancelled", serde_json::json!({})),
        &AntigravityConnectionEndpoint::new("http", 64001, "", EndpointSource::LanguageServer),
        Duration::from_secs(1),
    )
    .await
    .unwrap_err();
    let result = AntigravityStatusProbe::fetch_process_snapshots(
        &processes(),
        &OperationDeadline::new(Duration::from_secs(4)),
        move |process| {
            let cancelled = cancelled.clone();
            async move {
                if process.pid == 2 {
                    Err(cancelled)
                } else {
                    Ok(partial())
                }
            }
        },
    )
    .await;
    assert_eq!(result.unwrap_err(), AntigravityStatusProbeError::Cancelled);
}

// upstream: AntigravityDeadlineTests.swift:163 shared deadline reserves time for later endpoint probes
#[tokio::test]
async fn shared_deadline_reserves_time_for_later_endpoint_probes() {
    let endpoints = [64001, 64002].map(|port| {
        AntigravityConnectionEndpoint::new("https", port, "token", EndpointSource::LanguageServer)
    });
    let timeouts = Arc::new(Mutex::new(Vec::new()));
    let base = Instant::now();
    let elapsed_ms = Arc::new(AtomicUsize::new(0));
    let clock = elapsed_ms.clone();
    let deadline = OperationDeadline::with_now_provider(Duration::from_secs(1), move || {
        base + Duration::from_millis(clock.load(Ordering::SeqCst) as u64)
    });
    let result = AntigravityStatusProbe::resolve_working_endpoint(&endpoints, &deadline, {
        let timeouts = timeouts.clone();
        move |endpoint, timeout| {
            timeouts.lock().unwrap().push(timeout);
            if endpoint.port == 64001 {
                elapsed_ms.fetch_add(timeout.as_millis() as usize, Ordering::SeqCst);
            }
            async move {
                if endpoint.port == 64001 {
                    Err(AntigravityStatusProbeError::TimedOut)
                } else {
                    Ok(())
                }
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(result.port, 64002);
    assert_eq!(
        *timeouts.lock().unwrap(),
        [Duration::from_millis(500), Duration::from_millis(500)]
    );
}
