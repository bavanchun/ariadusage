// Ported from CodexBar Tests/CodexBarTests/ClaudeOAuthTransportIdentityTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexTransportIdentityTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

#[path = "support/https.rs"]
mod https;

use std::error::Error;
use std::io;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::pipeline::{FetchError, TransportClass};
use ariadusage_engine::brokers::net::{
    DnsFailure, NetBroker, NetConfig, NetError, NetRequest, test_io_transport_class,
};
use httpmock::Method::GET;
use reqwest::dns::{Name, Resolve, Resolving};
use reqwest::{Method, Url};

struct FailingResolver;

impl Resolve for FailingResolver {
    fn resolve(&self, _name: Name) -> Resolving {
        Box::pin(async { Err(Box::new(DnsFailure) as Box<dyn Error + Send + Sync>) })
    }
}

#[tokio::test]
async fn request_timeout_maps_to_timeout_transport_class() {
    // CodexBar: ClaudeOAuthTransportIdentityTests.swift:10
    let server = https::start_server();
    let _slow = server.mock(|when, then| {
        when.method(GET).path("/slow");
        then.status(200)
            .delay(Duration::from_millis(400))
            .body("late");
    });
    let url = Url::parse(&server.url("/slow")).unwrap();
    let mut request = NetRequest::new(Method::GET, url.clone());
    request.timeout = Some(Duration::from_millis(30));

    let error = https::broker(NetConfig::default())
        .send(&https::call(), &https::declared(&url), request)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        NetError::Transport(TransportClass::Timeout)
    ));
}

#[tokio::test]
async fn refused_connection_maps_to_cannot_connect() {
    // CodexBar: CodexTransportIdentityTests.swift:33
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let url = Url::parse(&format!("https://{address}/refused")).unwrap();

    let error = https::broker(NetConfig::default())
        .send(
            &https::call(),
            &https::declared(&url),
            NetRequest::new(Method::GET, url),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        NetError::Transport(TransportClass::CannotConnect)
    ));
}

#[tokio::test]
async fn injected_dns_failure_maps_to_dns_without_system_resolution() {
    // CodexBar: ClaudeOAuthTransportIdentityTests.swift:30
    let url = Url::parse("https://synthetic-dns-failure.invalid/usage").unwrap();
    let origins = https::declared(&url);
    let broker =
        NetBroker::new_with_resolver(NetConfig::default(), Arc::new(FailingResolver)).unwrap();

    let error = broker
        .send(&https::call(), &origins, NetRequest::new(Method::GET, url))
        .await
        .unwrap_err();
    assert!(matches!(error, NetError::Transport(TransportClass::Dns)));
}

#[test]
fn io_failure_kinds_keep_the_typed_transport_mapping() {
    // CodexBar: CodexTransportIdentityTests.swift:62
    let mappings = [
        (
            io::ErrorKind::ConnectionRefused,
            Some(TransportClass::CannotConnect),
        ),
        (
            io::ErrorKind::NetworkUnreachable,
            Some(TransportClass::NotConnected),
        ),
        (
            io::ErrorKind::HostUnreachable,
            Some(TransportClass::NotConnected),
        ),
        (
            io::ErrorKind::ConnectionReset,
            Some(TransportClass::ConnectionLost),
        ),
        (
            io::ErrorKind::ConnectionAborted,
            Some(TransportClass::ConnectionLost),
        ),
        (
            io::ErrorKind::UnexpectedEof,
            Some(TransportClass::ConnectionLost),
        ),
        (
            io::ErrorKind::BrokenPipe,
            Some(TransportClass::ConnectionLost),
        ),
        (
            io::ErrorKind::AddrNotAvailable,
            Some(TransportClass::CannotFindHost),
        ),
        (
            io::ErrorKind::NotFound,
            Some(TransportClass::CannotFindHost),
        ),
        (io::ErrorKind::Other, None),
    ];
    for (kind, expected) in mappings {
        assert_eq!(test_io_transport_class(kind), expected, "{kind:?}");
    }
}

#[test]
fn only_typed_transport_failures_are_preservable_and_cancellation_stays_terminal() {
    // CodexBar: CodexTransportIdentityTests.swift:74,107
    let transport_classes = [
        TransportClass::Timeout,
        TransportClass::ConnectionLost,
        TransportClass::NotConnected,
        TransportClass::CannotFindHost,
        TransportClass::CannotConnect,
        TransportClass::Dns,
    ];
    for class in transport_classes {
        assert!(class.is_preservable());
        assert!(class.is_startup_retryable());
        assert!(!class.is_cancellation());
    }
    assert!(!TransportClass::Cancelled.is_startup_retryable());
    assert!(TransportClass::Cancelled.is_cancellation());

    let cancellation = NetError::Cancelled.into_fetch_error();
    assert!(matches!(&cancellation, FetchError::Cancelled));
    assert!(cancellation.is_cancellation());
}
