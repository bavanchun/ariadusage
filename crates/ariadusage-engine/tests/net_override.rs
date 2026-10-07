// Ported from CodexBar Tests/CodexBarTests/ProviderEndpointOverrideSecurityTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/ProviderEndpointOverrideSecurityLinuxTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/AzureEndpointOverrideSecurityTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

#[path = "support/https.rs"]
mod https;

use ariadusage_engine::brokers::net::{
    NetConfig, NetRequest, OverrideError, OverridePolicy, is_loopback_host,
    is_private_network_host, validate_override,
};
use ariadusage_protocol::secret::SecretString;
use httpmock::Method::GET;
use reqwest::Method;

#[test]
fn bracketed_ipv6_literals_are_valid_https_endpoints() {
    // CodexBar: ProviderEndpointOverrideSecurityTests.swift:7
    let endpoint = validate_override("https://[::1]:8443/v1", &OverridePolicy::HttpsOnly).unwrap();
    assert_eq!(endpoint.as_str(), "https://[::1]:8443/v1");
}

#[test]
fn userinfo_and_encoded_host_delimiters_are_rejected() {
    // CodexBar: ProviderEndpointOverrideSecurityTests.swift:32
    for endpoint in [
        "https://user:pass@proxy.test/v1",
        "https://proxy.test%2f.attacker.test/v1",
        "https://bad host/v1",
        "https://bad%20host/v1",
        "https://bad%09host/v1",
    ] {
        assert!(
            validate_override(endpoint, &OverridePolicy::HttpsOnly).is_err(),
            "{endpoint}"
        );
    }
}

#[test]
fn generic_endpoint_overrides_normalize_bare_hosts_and_enforce_https() {
    // CodexBar: AzureEndpointOverrideSecurityTests.swift:11
    let bare = validate_override("resource.example.test", &OverridePolicy::HttpsOnly).unwrap();
    assert_eq!(bare.as_str(), "https://resource.example.test/");

    let host_port = validate_override("localhost:8443/openai", &OverridePolicy::HttpsOnly).unwrap();
    assert_eq!(host_port.as_str(), "https://localhost:8443/openai");

    assert_eq!(
        validate_override("http://attacker.test", &OverridePolicy::HttpsOnly),
        Err(OverrideError::HttpNotAllowed)
    );
    assert!(validate_override("ftp://provider.test/path", &OverridePolicy::HttpsOnly).is_err());
}

#[test]
fn provider_owned_policy_accepts_only_exact_hosts_and_dot_suffixes() {
    let policy = OverridePolicy::ProviderOwnedOnly {
        allowed_hosts: vec!["api.provider.test".to_owned()],
        allowed_domain_suffixes: vec!["provider.test".to_owned()],
    };
    assert!(validate_override("https://api.provider.test/v1", &policy).is_ok());
    assert!(validate_override("https://eu.provider.test/v1", &policy).is_ok());
    assert_eq!(
        validate_override("https://evilprovider.test/v1", &policy),
        Err(OverrideError::HostNotAllowed)
    );
    assert_eq!(
        validate_override("https://provider.test/v1", &policy)
            .unwrap()
            .host_str(),
        Some("provider.test")
    );
    assert_eq!(
        validate_override("http://api.provider.test/v1", &policy),
        Err(OverrideError::HttpNotAllowed)
    );
}

#[test]
fn loopback_http_policy_rejects_private_network_endpoints() {
    // CodexBar: ProviderEndpointOverrideSecurityLinuxTests.swift:207
    let loopback = OverridePolicy::AllowLoopbackHttp;
    assert!(validate_override("http://127.0.0.1:4000", &loopback).is_ok());
    assert!(validate_override("http://[::1]:4000", &loopback).is_ok());
    assert!(validate_override("http://localhost:4000", &loopback).is_ok());
    for endpoint in [
        "http://192.168.1.10:4000",
        "http://[fd00::1]:4000",
        "http://proxy.local:4000",
    ] {
        assert!(
            validate_override(endpoint, &loopback).is_err(),
            "{endpoint}"
        );
    }
}

#[test]
fn private_network_http_policy_matches_private_and_public_boundaries() {
    // CodexBar: ProviderEndpointOverrideSecurityLinuxTests.swift:221
    let policy = OverridePolicy::AllowPrivateNetworkHttp;
    let private = [
        "localhost",
        "127.0.0.1",
        "::1",
        "10.255.255.255",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.1.10",
        "169.254.10.20",
        "fc00::1",
        "fdff:ffff::1",
        "fe80::1",
        "febf:ffff::1",
        "proxy.local",
        "proxy.local.",
    ];
    let public = [
        "attacker.test",
        "8.8.8.8",
        "172.15.255.255",
        "172.32.0.0",
        "169.253.255.255",
        "192.169.0.1",
        "2606:4700:4700::1111",
        "fec0::1",
    ];

    for host in private {
        assert!(is_private_network_host(host), "expected private: {host}");
        let endpoint = if host.contains(':') {
            format!("http://[{host}]:4000")
        } else {
            format!("http://{host}:4000")
        };
        assert!(validate_override(&endpoint, &policy).is_ok(), "{endpoint}");
    }
    for host in public {
        assert!(!is_private_network_host(host), "expected public: {host}");
        let endpoint = if host.contains(':') {
            format!("http://[{host}]:4000")
        } else {
            format!("http://{host}:4000")
        };
        assert!(validate_override(&endpoint, &policy).is_err(), "{endpoint}");
    }

    assert!(is_loopback_host("127.42.0.9"));
    assert!(!is_loopback_host("192.168.1.10"));
}

#[tokio::test]
async fn insecure_override_is_rejected_before_a_credentialed_request_is_built() {
    // CodexBar: AzureEndpointOverrideSecurityTests.swift:38
    let server = https::start_server();
    let target = server.mock(|when, then| {
        when.method(GET).path("/api");
        then.status(200);
    });
    let override_value = format!("http://127.0.0.1:{}/api", server.port());
    let validation = validate_override(&override_value, &OverridePolicy::HttpsOnly);
    assert_eq!(validation, Err(OverrideError::HttpNotAllowed));

    if let Ok(url) = validation {
        let mut request = NetRequest::new(Method::GET, url.clone());
        request.credentials.push((
            reqwest::header::HeaderName::from_static("api-key"),
            SecretString::new("synthetic-insecure-override-key"),
        ));
        let _ = https::broker(NetConfig::default())
            .send(&https::call(), &https::declared(&url), request)
            .await;
    }
    assert_eq!(target.calls(), 0);
}
