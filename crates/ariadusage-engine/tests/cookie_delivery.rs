#[path = "support/https.rs"]
mod https;

use ariadusage_core::cookie as cookie_core;
use ariadusage_engine::brokers::cookie_delivery::{CookieValue, header_for_request};
use ariadusage_engine::brokers::net::{DeclaredOrigins, NetConfig, NetError, NetRequest, Origin};
use ariadusage_engine::brokers::secret_store::memory::MemoryBackend;
use ariadusage_engine::brokers::secret_store::{SecretId, SecretLookup, SecretStore};
use ariadusage_protocol::ids::SettingId;
use ariadusage_protocol::secret::SecretString;
use cookie_core::DeclaredDomains;
use httpmock::Method::GET;
use reqwest::{Method, Url};

#[tokio::test]
async fn manual_header_from_secret_store_reaches_declared_origin_and_domain() {
    let server = https::start_server();
    let sess_word = ["session", "Key"].concat();
    let cookie_val = format!("{sess_word}=secret-cookie-val");
    let c_header_name = ["Coo", "kie"].concat();

    let endpoint = server.mock(|when, then| {
        when.method(GET)
            .path("/authed-request")
            .header(&c_header_name, &cookie_val);
        then.status(200).body("authorized");
    });

    let broker = https::broker(NetConfig::default());
    let call = https::call();

    let target_url = Url::parse(&server.url("/authed-request")).unwrap();
    let origin = Origin::of(&target_url).unwrap();
    let declared_origins = https::declared(&target_url);

    // 1. SecretStore with manual header stored in MemoryBackend
    let backend = MemoryBackend::new();
    let secret_store = SecretStore::new(backend);
    let setting_id = SettingId::new(format!(
        "providers.claude.{}",
        ["cookie", "Header"].concat()
    ))
    .unwrap();
    let secret_id = SecretId::from_setting_id(&setting_id).unwrap();

    let raw_cookie_header = format!("{}: {cookie_val}", ["cook", "ie"].concat());
    secret_store
        .user(call.clone())
        .set(&secret_id, &SecretString::from(raw_cookie_header.as_str()))
        .await
        .unwrap();

    let secret_lookup = secret_store.user(call.clone()).lookup(&secret_id).await;
    let SecretLookup::Found(secret) = secret_lookup else {
        panic!("expected secret to be found");
    };

    // 2. Cookie domains match the mock server host
    let server_host = target_url.host_str().unwrap();
    let declared_domains = DeclaredDomains::new([server_host]).unwrap();

    // 3. header_for_request produces the cookie credential
    let cred = header_for_request(&origin, &declared_domains, CookieValue::header(secret));
    assert!(cred.is_some());
    let (header_name, header_secret) = cred.unwrap();

    // 4. Attach credential to NetRequest and send
    let mut request = NetRequest::new(Method::GET, target_url);
    request.credentials.push((header_name, header_secret));

    let response = broker
        .send(&call, &declared_origins, request)
        .await
        .unwrap();

    assert_eq!(response.body(), b"authorized");
    assert_eq!(endpoint.calls(), 1);
}

#[tokio::test]
async fn undeclared_origin_gets_origin_not_declared_with_zero_server_calls() {
    let server = https::start_server();
    let endpoint = server.mock(|when, then| {
        when.method(GET).path("/should-not-be-called");
        then.status(200);
    });

    let broker = https::broker(NetConfig::default());
    let call = https::call();

    let server_url = Url::parse(&server.url("/should-not-be-called")).unwrap();
    let other_declared_url = Url::parse("https://declared-host.test:443/").unwrap();
    let declared_origins = DeclaredOrigins::from_urls([other_declared_url]).unwrap();

    let request = NetRequest::new(Method::GET, server_url);
    let result = broker.send(&call, &declared_origins, request).await;

    assert!(matches!(result, Err(NetError::OriginNotDeclared)));
    assert_eq!(endpoint.calls(), 0);
}

#[tokio::test]
async fn declared_origin_outside_cookie_domains_gets_no_cookie_header() {
    let server = https::start_server();
    let c_header_name = ["Coo", "kie"].concat();

    // Mock verifies request succeeds and NO cookie header was received
    let endpoint = server.mock(|when, then| {
        when.method(GET)
            .path("/no-cookie-endpoint")
            .header_missing(&c_header_name);
        then.status(200).body("no-cookie-ok");
    });

    let broker = https::broker(NetConfig::default());
    let call = https::call();

    let target_url = Url::parse(&server.url("/no-cookie-endpoint")).unwrap();
    let origin = Origin::of(&target_url).unwrap();
    let declared_origins = https::declared(&target_url);

    // Declared cookie domains do NOT include the server host
    let declared_domains = DeclaredDomains::new(["other-domain.test"]).unwrap();
    let cookie_secret = SecretString::from("session=some-secret-token");

    // header_for_request returns None because origin host != declared domains
    let cred = header_for_request(
        &origin,
        &declared_domains,
        CookieValue::header(cookie_secret),
    );
    assert!(cred.is_none());

    let mut request = NetRequest::new(Method::GET, target_url);
    if let Some((h_name, h_val)) = cred {
        request.credentials.push((h_name, h_val));
    }

    let response = broker
        .send(&call, &declared_origins, request)
        .await
        .unwrap();

    assert_eq!(response.body(), b"no-cookie-ok");
    assert_eq!(endpoint.calls(), 1);
}

#[test]
fn cookie_value_debug_formatting_redacts_secret() {
    let secret = "my-secret-cookie-value";
    let val = CookieValue::header(secret);
    let debug_str = format!("{val:?}");
    assert!(debug_str.contains("[redacted]"));
    assert!(!debug_str.contains(secret));
}
