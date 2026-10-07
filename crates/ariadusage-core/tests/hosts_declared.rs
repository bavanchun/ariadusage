use ariadusage_core::hosts::{HostsError, ProviderHosts};

#[test]
fn provider_hosts_reject_empty_declarations() {
    assert_eq!(
        ProviderHosts::new(Vec::<String>::new()),
        Err(HostsError::Empty)
    );
}

#[test]
fn provider_hosts_normalize_case_and_deduplicate() {
    let hosts = ProviderHosts::new(["API.Example.COM", "api.example.com"]).unwrap();

    assert_eq!(hosts.hosts().len(), 1);
    assert_eq!(hosts.hosts()[0].as_str(), "api.example.com");
}

#[test]
fn provider_hosts_derive_https_origins_on_port_443() {
    let hosts = ProviderHosts::new(["api.example.com", "auth.example.com"]).unwrap();

    assert_eq!(
        hosts.origins().collect::<Vec<_>>(),
        [
            "https://api.example.com:443",
            "https://auth.example.com:443"
        ]
    );
}

#[test]
fn provider_hosts_reject_non_dns_and_ambiguous_values() {
    for invalid in [
        "",
        "bad host.example",
        "-bad.example",
        "bad-.example",
        "bad..example",
        "example.com:443",
        "https://example.com",
        "example.com.",
    ] {
        assert_eq!(ProviderHosts::new([invalid]), Err(HostsError::InvalidHost));
    }
}
