#[path = "support/https.rs"]
mod https;

use std::process::Command;

use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::net::{NetConfig, NetRequest};
use httpmock::Method::GET;
use reqwest::{Method, Url};
use tokio_util::sync::CancellationToken;

#[test]
fn main_client_uses_child_environment_proxy_and_loopback_client_bypasses_it() {
    let loopback_server = https::start_server();
    let proxy_server = https::start_server();
    let provider = proxy_server.mock(|when, then| {
        when.method(GET).host("provider.invalid").path("/provider");
        then.status(200).body("provider via proxy");
    });
    let loopback_direct = loopback_server.mock(|when, then| {
        when.any_request();
        then.status(200).body("loopback direct");
    });
    let loopback_proxy_route = proxy_server.mock(|when, then| {
        when.host("127.0.0.1").port(loopback_server.port());
        then.status(200).body("loopback via proxy");
    });

    let home = tempfile::tempdir().unwrap();
    let executable = std::env::current_exe().unwrap();
    let output = Command::new(executable)
        .arg("--exact")
        .arg("proxy_child_uses_synthetic_environment_and_keeps_loopback_direct")
        .env_clear()
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("ARIADUSAGE_PROXY_CHILD", "1")
        .env(
            "ARIADUSAGE_PROVIDER_URL",
            "https://provider.invalid/provider",
        )
        .env("ARIADUSAGE_LOOPBACK_URL", loopback_server.url("/loopback"))
        .env(
            "HTTPS_PROXY",
            format!("http://{}:{}", proxy_server.host(), proxy_server.port()),
        )
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "proxy child failed (provider={}, loopback_direct={}, loopback_proxy={}). stdout: {}\nstderr: {}",
        provider.calls(),
        loopback_direct.calls(),
        loopback_proxy_route.calls(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(provider.calls(), 1);
    assert_eq!(loopback_direct.calls(), 1);
    assert_eq!(loopback_proxy_route.calls(), 0);
}

#[tokio::test]
async fn proxy_child_uses_synthetic_environment_and_keeps_loopback_direct() {
    if std::env::var_os("ARIADUSAGE_PROXY_CHILD").is_none() {
        return;
    }

    let provider = std::env::var("ARIADUSAGE_PROVIDER_URL").unwrap();
    let provider_url = Url::parse(&provider).unwrap();
    let broker = https::broker_with_system_proxy(NetConfig::default());
    let call = BrokerCall {
        interaction: ariadusage_core::pipeline::FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "synthetic-proxy-test".to_owned(),
    };
    let provider_response = broker
        .send(
            &call,
            &https::declared(&provider_url),
            NetRequest::new(Method::GET, provider_url),
        )
        .await
        .unwrap();
    assert_eq!(provider_response.body(), b"provider via proxy");

    let loopback_url = Url::parse(&std::env::var("ARIADUSAGE_LOOPBACK_URL").unwrap()).unwrap();
    let loopback_response = broker
        .send_loopback(&call, NetRequest::new(Method::GET, loopback_url))
        .await
        .unwrap();
    assert_eq!(loopback_response.body(), b"loopback direct");
}
