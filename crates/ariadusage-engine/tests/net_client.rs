// Ported from CodexBar Tests/CodexBarTests/ProviderHTTPClientTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

#[path = "support/https.rs"]
mod https;

use std::time::Duration;

use ariadusage_core::hosts::ProviderHosts;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::net::{
    NetBody, NetConfig, NetError, NetRequest, test_credential_header_value,
};
use ariadusage_protocol::secret::SecretString;
use httpmock::Method::{GET, POST};
use reqwest::header::HeaderName;
use reqwest::{Method, Url};

#[test]
fn default_client_has_the_expected_request_and_resource_timeouts() {
    // CodexBar: ProviderHTTPClientTests.swift:89
    let config = NetConfig::default();
    assert_eq!(config.read_timeout, Duration::from_secs(30));
    assert_eq!(config.total_timeout, Duration::from_secs(90));
}

#[tokio::test]
async fn response_body_cap_accepts_the_limit_and_rejects_one_byte_over() {
    let server = https::start_server();
    let exact = server.mock(|when, then| {
        when.method(GET).path("/cap-exact");
        then.status(200).body("four");
    });
    let over = server.mock(|when, then| {
        when.method(GET).path("/cap-over");
        then.status(200).body("fives");
    });
    let broker = https::broker(NetConfig {
        body_cap: 4,
        ..NetConfig::default()
    });
    let call = https::call();
    let exact_url = Url::parse(&server.url("/cap-exact")).unwrap();
    let origins = https::declared(&exact_url);

    let accepted = broker
        .send(&call, &origins, NetRequest::new(Method::GET, exact_url))
        .await
        .unwrap();
    assert_eq!(accepted.body(), b"four");

    let over_url = Url::parse(&server.url("/cap-over")).unwrap();
    assert!(matches!(
        broker
            .send(&call, &origins, NetRequest::new(Method::GET, over_url))
            .await,
        Err(NetError::BodyTooLarge { cap: 4 })
    ));
    assert_eq!(exact.calls(), 1);
    assert_eq!(over.calls(), 1);
}

#[tokio::test]
async fn configured_read_timeout_bounds_a_stalled_response() {
    let server = https::start_server();
    let _slow = server.mock(|when, then| {
        when.method(GET).path("/read-timeout");
        then.status(200)
            .delay(Duration::from_millis(400))
            .body("late response");
    });
    let url = Url::parse(&server.url("/read-timeout")).unwrap();
    let broker = https::broker(NetConfig {
        read_timeout: Duration::from_millis(30),
        total_timeout: Duration::from_secs(2),
        ..NetConfig::default()
    });

    let error = broker
        .send(
            &https::call(),
            &https::declared(&url),
            NetRequest::new(Method::GET, url),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        NetError::Transport(ariadusage_core::pipeline::TransportClass::Timeout)
    ));
}

#[tokio::test]
async fn undeclared_origin_is_rejected_before_credentials_or_network_io() {
    let server = https::start_server();
    let target = server.mock(|when, then| {
        when.method(GET).path("/credential");
        then.status(200);
    });
    let actual = Url::parse(&server.url("/credential")).unwrap();
    let hosts = ProviderHosts::new(["provider.invalid"]).unwrap();
    let declared = ariadusage_engine::brokers::net::DeclaredOrigins::from_provider_hosts(&hosts);
    let mut request = NetRequest::new(Method::GET, actual);
    request.credentials.push((
        HeaderName::from_static("x-api-key"),
        SecretString::new("synthetic-undeclared-origin-key"),
    ));

    assert!(matches!(
        https::broker(NetConfig::default())
            .send(&https::call(), &declared, request)
            .await,
        Err(NetError::OriginNotDeclared)
    ));
    assert_eq!(target.calls(), 0);
}

#[tokio::test]
async fn credential_headers_are_sensitive_and_attached_to_declared_requests() {
    let server = https::start_server();
    let accepted = server.mock(|when, then| {
        when.method(GET)
            .path("/credential")
            .header("x-api-key", "synthetic-boundary-key");
        then.status(200).body("ok");
    });
    let url = Url::parse(&server.url("/credential")).unwrap();
    let mut request = NetRequest::new(Method::GET, url.clone());
    request.credentials.push((
        HeaderName::from_static("x-api-key"),
        SecretString::new("synthetic-boundary-key"),
    ));

    let header =
        test_credential_header_value(&SecretString::new("synthetic-boundary-key")).unwrap();
    assert!(header.is_sensitive());
    let response = https::broker(NetConfig::default())
        .send(&https::call(), &https::declared(&url), request)
        .await
        .unwrap();
    assert_eq!(response.status.as_u16(), 200);
    assert_eq!(accepted.calls(), 1);
}

#[tokio::test]
async fn secret_request_body_is_delivered_to_the_fixture_server() {
    let server = https::start_server();
    let payload = "synthetic-secret-body-marker";
    let received = server.mock(|when, then| {
        when.method(POST).path("/secret-body").body(payload);
        then.status(200).body("accepted");
    });
    let url = Url::parse(&server.url("/secret-body")).unwrap();
    let mut request = NetRequest::new(Method::POST, url.clone());
    request.body = NetBody::secret(payload.as_bytes().to_vec());

    let response = https::broker(NetConfig::default())
        .send(&https::call(), &https::declared(&url), request)
        .await
        .unwrap();

    assert_eq!(response.body(), b"accepted");
    assert_eq!(received.calls(), 1);
}

#[tokio::test]
async fn cancellation_before_send_makes_no_request() {
    let server = https::start_server();
    let target = server.mock(|when, then| {
        when.method(GET).path("/cancel-before");
        then.status(200);
    });
    let url = Url::parse(&server.url("/cancel-before")).unwrap();
    let call = https::call();
    call.cancel.cancel();

    assert!(matches!(
        https::broker(NetConfig::default())
            .send(
                &call,
                &https::declared(&url),
                NetRequest::new(Method::GET, url),
            )
            .await,
        Err(NetError::Cancelled)
    ));
    assert_eq!(target.calls(), 0);
}

#[tokio::test]
async fn cancellation_while_waiting_for_a_response_returns_cancelled() {
    let server = https::start_server();
    let _slow = server.mock(|when, then| {
        when.method(GET).path("/cancel-during");
        then.status(200)
            .delay(Duration::from_millis(400))
            .body("late response");
    });
    let url = Url::parse(&server.url("/cancel-during")).unwrap();
    let broker = https::broker(NetConfig::default());
    let call = https::call();
    let cancellation = call.cancel.clone();
    let declared = https::declared(&url);
    let send = broker.send(&call, &declared, NetRequest::new(Method::GET, url));
    tokio::pin!(send);
    tokio::select! {
        result = &mut send => panic!("request completed before cancellation: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(30)) => cancellation.cancel(),
    }
    assert!(matches!(send.await, Err(NetError::Cancelled)));
}

#[tokio::test]
async fn loopback_client_requires_literal_addresses_and_never_follows_redirects() {
    let server = https::start_server();
    let destination = server.mock(|when, then| {
        when.method(GET).path("/redirect-destination");
        then.status(200).body("should not arrive");
    });
    let redirect = server.mock(|when, then| {
        when.method(GET).path("/redirect");
        then.status(302)
            .header("location", server.url("/redirect-destination"));
    });
    let broker = https::broker(NetConfig::default());
    for address in ["https://example.com/status", "https://localhost/status"] {
        assert!(matches!(
            broker
                .send_loopback(&https::call(), https::request(address))
                .await,
            Err(NetError::LoopbackOnly)
        ));
    }

    let url = Url::parse(&server.url("/redirect")).unwrap();
    let response = broker
        .send_loopback(&https::call(), NetRequest::new(Method::GET, url))
        .await
        .unwrap();
    assert_eq!(response.status.as_u16(), 302);
    assert_eq!(redirect.calls(), 1);
    assert_eq!(destination.calls(), 0);
}

#[test]
fn broker_request_response_and_error_formatting_redacts_synthetic_secrets() {
    let secret = "synthetic-format-secret-marker";
    let mut request = NetRequest::new(
        Method::POST,
        Url::parse("https://provider.invalid/token?access_token=synthetic-format-secret-marker")
            .unwrap(),
    );
    request.credentials.push((
        HeaderName::from_static("authorization"),
        SecretString::new(secret),
    ));
    request.body = NetBody::secret(format!("refresh_token={secret}"));
    let request_debug = format!("{request:?}");

    let call = BrokerCall {
        interaction: ariadusage_core::pipeline::FetchInteraction::UserInitiated,
        cancel: tokio_util::sync::CancellationToken::new(),
        request_id: secret.to_owned(),
    };
    let call_debug = format!("{call:?}");
    let server = https::start_server();
    let failure = server.mock(|when, then| {
        when.method(GET).path("/failure");
        then.status(503).body(secret);
    });
    let url = Url::parse(&server.url("/failure")).unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let error = runtime
        .block_on(https::broker(NetConfig::default()).send(
            &https::call(),
            &https::declared(&url),
            NetRequest::new(Method::GET, url.clone()),
        ))
        .unwrap_err();
    let error_debug = format!("{error:?} {error}");
    assert_eq!(failure.calls(), 1);

    let response = runtime.block_on(async {
        let success = server.mock(|when, then| {
            when.method(GET).path("/response");
            then.status(200).body(secret);
        });
        let response_url = Url::parse(&server.url("/response")).unwrap();
        let response = https::broker(NetConfig::default())
            .send(
                &https::call(),
                &https::declared(&response_url),
                NetRequest::new(Method::GET, response_url),
            )
            .await
            .unwrap();
        assert_eq!(success.calls(), 1);
        format!("{response:?}")
    });

    for formatted in [request_debug, call_debug, error_debug, response] {
        assert!(!formatted.contains(secret), "redaction failed: {formatted}");
    }
}
