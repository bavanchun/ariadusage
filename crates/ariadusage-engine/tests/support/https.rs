#![allow(dead_code)] // Shared HTTPS helpers are only used by selected integration-test binaries.

use ariadusage_core::hosts::ProviderHosts;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::net::{DeclaredOrigins, NetBroker, NetConfig, NetRequest};
use httpmock::MockServer;
use reqwest::{Method, Url};
use tokio_util::sync::CancellationToken;

const HTTPMOCK_CA_PEM: &[u8] =
    include_bytes!("../../../../fixtures/ariadusage/httpmock-test-ca.pem.fixture");

pub fn install_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

pub fn start_server() -> MockServer {
    install_crypto_provider();
    MockServer::start()
}

pub fn broker(config: NetConfig) -> NetBroker {
    install_crypto_provider();
    NetBroker::new_with_test_ca(config, HTTPMOCK_CA_PEM).expect("test NetBroker should build")
}

pub fn broker_with_system_proxy(config: NetConfig) -> NetBroker {
    install_crypto_provider();
    NetBroker::new_with_test_ca_and_system_proxy(config, HTTPMOCK_CA_PEM)
        .expect("test NetBroker should build")
}

pub fn call() -> BrokerCall {
    BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "synthetic-test-request".to_owned(),
    }
}

pub fn request(url: &str) -> NetRequest {
    NetRequest::new(
        Method::GET,
        Url::parse(url).expect("fixture URL should parse"),
    )
}

pub fn declared(url: &Url) -> DeclaredOrigins {
    DeclaredOrigins::from_urls([url.clone()]).expect("fixture origin should be valid")
}

pub fn provider_hosts(host: &str) -> DeclaredOrigins {
    DeclaredOrigins::from_provider_hosts(
        &ProviderHosts::new([host]).expect("fixture host should be valid"),
    )
}
