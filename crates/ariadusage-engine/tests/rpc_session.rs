// Ported from CodexBar Tests/CodexBarTests/RPCRequestTimeoutTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/RPCChildProcessTeardownTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/BoundedChildProcessProofTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
#![cfg(target_os = "linux")]

#[path = "support/process.rs"]
mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use ariadusage_core::gates::launch::LaunchGate;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::process::{ProcessRegistry, RpcError, RpcSession};
use serde_json::value::RawValue;

fn raw(json: &str) -> Box<RawValue> {
    RawValue::from_string(json.to_owned()).unwrap()
}

async fn spawn(args: &[&str], registry: &ProcessRegistry) -> RpcSession {
    RpcSession::spawn(
        support::call(FetchInteraction::UserInitiated),
        support::command(args),
        registry,
        &LaunchGate::default(),
    )
    .await
    .unwrap()
}

#[tokio::test]
// CodexBar: RPCRequestTimeoutTests.swift:13
async fn request_timeout_wins_over_the_eof_caused_by_teardown() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let session = spawn(&["rpc-ignore-term"], &registry).await;
    let started = Instant::now();
    let result = session
        .request("wait", &raw("{}"), Duration::from_millis(30))
        .await;
    assert!(matches!(result, Err(RpcError::TimedOut)));
    assert!(started.elapsed() >= Duration::from_millis(350));
    assert_eq!(registry.active_launches(), 0);
}

#[tokio::test]
// CodexBar: RPCRequestTimeoutTests.swift:36
async fn success_keeps_the_session_alive_until_explicit_shutdown() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let session = spawn(&["rpc-echo"], &registry).await;
    let result = session
        .request("echo", &raw(r#"{"n":7}"#), Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(result.get(), r#"{"n":7}"#);
    assert_eq!(registry.active_launches(), 1);
    session.shutdown().await;
    assert_eq!(registry.active_launches(), 0);
}

#[tokio::test]
async fn explicit_null_result_is_preserved_as_raw_json() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let session = spawn(&["rpc-echo"], &registry).await;
    let result = session
        .request("null", &raw("null"), Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(result.get(), "null");
    session.shutdown().await;
}

#[tokio::test]
async fn a_response_without_a_result_is_malformed() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let session = spawn(&["rpc-echo", "missing-result"], &registry).await;
    let result = session
        .request("missing", &raw("{}"), Duration::from_secs(2))
        .await;
    assert!(matches!(result, Err(RpcError::Malformed)));
    session.shutdown().await;
}

#[tokio::test]
// CodexBar: RPCRequestTimeoutTests.swift:49
async fn request_error_is_classified_without_retaining_its_message() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let session = spawn(&["rpc-echo", "error"], &registry).await;
    let error = session
        .request("fail", &raw("{}"), Duration::from_secs(2))
        .await
        .unwrap_err();
    assert_eq!(error, RpcError::RequestFailed);
    assert!(!format!("{error:?}").contains("synthetic-rpc-private-value"));
    session.shutdown().await;
}

#[tokio::test]
// CodexBar: RPCRequestTimeoutTests.swift:65
async fn cancellation_does_not_tear_down_the_session() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let call = support::call(FetchInteraction::UserInitiated);
    let cancel = call.cancel.clone();
    let session = Arc::new(
        RpcSession::spawn(
            call,
            support::command(&["rpc-ignore-term"]),
            &registry,
            &LaunchGate::default(),
        )
        .await
        .unwrap(),
    );
    let task_session = Arc::clone(&session);
    let task = tokio::spawn(async move {
        task_session
            .request("wait", &raw("{}"), Duration::from_secs(3))
            .await
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    cancel.cancel();
    assert!(matches!(task.await.unwrap(), Err(RpcError::Cancelled)));
    assert_eq!(registry.active_launches(), 1);
    session.shutdown().await;
    assert_eq!(registry.active_launches(), 0);
}

#[tokio::test]
// CodexBar: RPCChildProcessTeardownTests.swift:14
async fn writes_after_shutdown_fail_as_closed_without_restarting_cleanup() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let session = spawn(&["rpc-echo"], &registry).await;
    session.shutdown().await;
    for _ in 0..2 {
        assert!(matches!(
            session
                .request("after-shutdown", &raw("{}"), Duration::from_secs(1))
                .await,
            Err(RpcError::Closed)
        ));
    }
    assert_eq!(registry.active_launches(), 0);
}

#[tokio::test]
// CodexBar: RPCChildProcessTeardownTests.swift:51
async fn closing_stdin_turns_a_pending_request_into_a_normal_closed_error() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let session = Arc::new(spawn(&["rpc-ignore-term"], &registry).await);
    let task_session = Arc::clone(&session);
    let task = tokio::spawn(async move {
        task_session
            .request("wait", &raw("{}"), Duration::from_secs(3))
            .await
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    session.shutdown().await;
    assert!(matches!(task.await.unwrap(), Err(RpcError::Closed)));
    assert_eq!(registry.active_launches(), 0);
}

#[tokio::test]
// CodexBar: RPCChildProcessTeardownTests.swift:110
async fn shutdown_escalates_against_a_term_ignoring_child() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let session = spawn(&["rpc-ignore-term"], &registry).await;
    let started = Instant::now();
    session.shutdown().await;
    assert!(started.elapsed() >= Duration::from_millis(350));
    assert_eq!(registry.active_launches(), 0);
}

#[tokio::test]
// CodexBar: BoundedChildProcessProofTests.swift:59
async fn framed_responses_skip_notifications_and_non_json_and_match_out_of_order_ids() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let session = spawn(&["rpc-echo", "reverse"], &registry).await;
    let first_params = raw(r#"{"order":1}"#);
    let second_params = raw(r#"{"order":2}"#);
    let (first, second) = tokio::join!(
        session.request("first", &first_params, Duration::from_secs(2)),
        session.request("second", &second_params, Duration::from_secs(2)),
    );
    assert_eq!(first.unwrap().get(), r#"{"order":1}"#);
    assert_eq!(second.unwrap().get(), r#"{"order":2}"#);
    session.shutdown().await;
}

#[tokio::test]
// CodexBar: BoundedChildProcessProofTests.swift:96
async fn an_overlong_rpc_line_terminates_the_session() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let session = spawn(&["rpc-echo", "oversize"], &registry).await;
    let result = session
        .request("oversize", &raw("{}"), Duration::from_secs(3))
        .await;
    assert!(matches!(result, Err(RpcError::LineTooLong)));
    assert_eq!(registry.active_launches(), 0);
}

#[tokio::test]
// CodexBar: RPCChildProcessTeardownTests.swift:32
async fn a_child_exit_closes_later_requests_after_delivering_its_response() {
    support::require_nextest();
    let registry = ProcessRegistry::new();
    let session = spawn(&["rpc-echo", "exit"], &registry).await;
    let result = session
        .request("exit", &raw("{}"), Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(result.get(), "{}");
    tokio::time::timeout(Duration::from_secs(2), async {
        while registry.active_launches() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        session
            .request("after-exit", &raw("{}"), Duration::from_secs(1))
            .await,
        Err(RpcError::Closed)
    ));
}
