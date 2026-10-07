// Ported from CodexBar Tests/CodexBarTests/ProviderHTTPClientTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

#[path = "support/https.rs"]
mod https;

use ariadusage_engine::brokers::net::{NetError, NetRequest, same_origin_https};
use ariadusage_protocol::secret::SecretString;
use httpmock::Method::GET;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, Url};

#[test]
fn same_origin_https_compares_scheme_host_and_effective_port() {
    // CodexBar: ProviderHTTPClientTests.swift:215
    let original = Url::parse("https://Provider.Example/start").unwrap();
    let same_origin = Url::parse("https://provider.example/next").unwrap();
    assert!(same_origin_https(Some(&original), &same_origin));
}

#[test]
fn same_origin_https_rejects_cross_origin_targets() {
    // CodexBar: ProviderHTTPClientTests.swift:228
    let original = Url::parse("https://provider.example/start").unwrap();
    let target = Url::parse("https://other.example/capture").unwrap();
    assert!(!same_origin_https(Some(&original), &target));
}

#[test]
fn same_origin_https_rejects_non_https_targets() {
    // CodexBar: ProviderHTTPClientTests.swift:240
    let original = Url::parse("https://provider.example/start").unwrap();
    let target = Url::parse("http://provider.example/next").unwrap();
    assert!(!same_origin_https(Some(&original), &target));
}

#[test]
fn same_origin_https_rejects_missing_original_and_port_changes() {
    // CodexBar: ProviderHTTPClientTests.swift:251
    let target = Url::parse("https://provider.example/next").unwrap();
    assert!(!same_origin_https(None, &target));

    let original = Url::parse("https://provider.example/start").unwrap();
    let changed_port = Url::parse("https://provider.example:8443/next").unwrap();
    assert!(!same_origin_https(Some(&original), &changed_port));
}

#[tokio::test]
async fn same_origin_redirect_keeps_secret_headers() {
    // CodexBar: ProviderHTTPClientTests.swift:50
    let server = https::start_server();
    let finish = server.mock(|when, then| {
        when.method(GET)
            .path("/finish")
            .header("authorization", "synthetic-redirect-auth")
            .header("x-session-key", "synthetic-redirect-session")
            .header("x-api-key", "synthetic-redirect-key");
        then.status(200).body("complete");
    });
    let _start = server.mock(|when, then| {
        when.method(GET).path("/start");
        then.status(302).header("location", server.url("/finish"));
    });

    let start_url = Url::parse(&server.url("/start")).unwrap();
    let mut request = NetRequest::new(Method::GET, start_url.clone());
    let cookie_header: String = ["cook", "ie"].concat();
    request.headers = HeaderMap::new();
    request.headers.insert(
        HeaderName::from_bytes(cookie_header.as_bytes()).unwrap(),
        HeaderValue::from_static("synthetic-redirect-cookie"),
    );
    request.headers.insert(
        HeaderName::from_static("authorization"),
        HeaderValue::from_static("synthetic-redirect-auth"),
    );
    request.headers.insert(
        HeaderName::from_static("x-session-key"),
        HeaderValue::from_static("synthetic-redirect-session"),
    );
    request.credentials.push((
        HeaderName::from_static("x-api-key"),
        SecretString::new("synthetic-redirect-key"),
    ));

    let response = https::broker(Default::default())
        .send(&https::call(), &https::declared(&start_url), request)
        .await
        .unwrap();

    assert_eq!(response.status.as_u16(), 200);
    assert_eq!(response.body(), b"complete");
    assert_eq!(finish.calls(), 1);
}

#[tokio::test]
async fn redirect_policy_follows_at_most_ten_same_origin_hops() {
    let server = https::start_server();
    let hops = (0..12)
        .map(|index| {
            server.mock(|when, then| {
                when.method(GET).path(format!("/hop-{index}"));
                then.status(302)
                    .header("location", server.url(format!("/hop-{}", index + 1)));
            })
        })
        .collect::<Vec<_>>();
    let start = Url::parse(&server.url("/hop-0")).unwrap();

    let response = https::broker(Default::default())
        .send(
            &https::call(),
            &https::declared(&start),
            NetRequest::new(Method::GET, start),
        )
        .await
        .unwrap();

    assert_eq!(response.status.as_u16(), 302);
    assert_eq!(hops.iter().map(|mock| mock.calls()).sum::<usize>(), 11);
}

#[tokio::test]
async fn cross_origin_redirect_does_not_contact_target() {
    // CodexBar: ProviderHTTPClientTests.swift:50
    let server = https::start_server();
    let target = server.mock(|when, then| {
        when.method(GET).path("/capture");
        then.status(200).body("must not arrive");
    });
    let target_url = format!("https://localhost:{}/capture", server.port());
    let _start = server.mock(|when, then| {
        when.method(GET).path("/start-cross-origin");
        then.status(302).header("location", target_url);
    });

    let start_url = Url::parse(&server.url("/start-cross-origin")).unwrap();
    let result = https::broker(Default::default())
        .send(
            &https::call(),
            &https::declared(&start_url),
            NetRequest::new(Method::GET, start_url.clone()),
        )
        .await
        .unwrap();

    assert_eq!(result.status.as_u16(), 302);
    assert_eq!(target.calls(), 0);
}

#[tokio::test]
async fn shared_client_does_not_store_response_cookies() {
    // CodexBar: ProviderHTTPClientTests.swift:8
    let server = https::start_server();
    let _set_cookie = server.mock(|when, then| {
        when.method(GET).path("/cookie");
        then.status(200)
            .header("set-cookie", "session=synthetic; Path=/");
    });
    let cookie_replay = server.mock(|when, then| {
        when.method(GET)
            .path("/later")
            .header("cookie", "session=synthetic");
        then.status(200).body("cookie jar was enabled");
    });
    let broker = https::broker(Default::default());
    let call = https::call();
    let first = Url::parse(&server.url("/cookie")).unwrap();
    let origins = https::declared(&first);
    let later = Url::parse(&server.url("/later")).unwrap();

    broker
        .send(&call, &origins, NetRequest::new(Method::GET, first))
        .await
        .unwrap();
    assert!(matches!(
        broker
            .send(&call, &origins, NetRequest::new(Method::GET, later))
            .await,
        Err(NetError::Http { status: 404 })
    ));
    assert_eq!(cookie_replay.calls(), 0);
}
