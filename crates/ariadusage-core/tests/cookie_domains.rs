use ariadusage_core::cookie as cookie_core;
use ariadusage_core::hosts::{HostsError, ProviderHosts};
use cookie_core::{DeclaredDomains, domain_matches};

#[test]
fn domain_matches_exact_and_dot_suffix() {
    // Exact match
    assert!(domain_matches("claude.ai", "claude.ai"));
    // Leading dot on domain or host
    assert!(domain_matches(".claude.ai", "claude.ai"));
    assert!(domain_matches("claude.ai", ".claude.ai"));
    assert!(domain_matches(".claude.ai", ".claude.ai"));

    // Subdomain dot-suffix
    assert!(domain_matches("claude.ai", "www.claude.ai"));
    assert!(domain_matches("claude.ai", "sub.www.claude.ai"));
    assert!(domain_matches(".claude.ai", "api.claude.ai"));

    // Stricter than SweetCookieKit: notclaude.ai must NOT match claude.ai
    assert!(!domain_matches("claude.ai", "notclaude.ai"));
    assert!(!domain_matches("claude.ai", "myclaude.ai"));

    // Non-subdomain / unrelated domains
    assert!(!domain_matches("claude.ai", "ai"));
    assert!(!domain_matches("claude.ai", "google.com"));
    assert!(!domain_matches("claude.ai", "claude.ai.evil.com"));

    // Case insensitivity
    assert!(domain_matches("Claude.AI", "WWW.CLAUDE.AI"));
    assert!(domain_matches("claude.ai", "Www.Claude.Ai"));

    // Empty / whitespace
    assert!(!domain_matches("", "claude.ai"));
    assert!(!domain_matches("claude.ai", ""));
}

#[test]
fn declared_domains_empty_construction_fails() {
    let empty_vec: Vec<&str> = Vec::new();
    assert_eq!(DeclaredDomains::new(empty_vec), Err(HostsError::Empty));

    let only_whitespace = vec!["   ", ""];
    assert_eq!(
        DeclaredDomains::new(only_whitespace),
        Err(HostsError::Empty)
    );
}

#[test]
fn declared_domains_from_provider_hosts() {
    let hosts = ProviderHosts::new(["claude.ai", "api.anthropic.com"]).unwrap();
    let declared = DeclaredDomains::from_provider_hosts(&hosts);

    assert_eq!(declared.domains().len(), 2);
    assert!(declared.matches("claude.ai"));
    assert!(declared.matches("www.claude.ai"));
    assert!(declared.matches("api.anthropic.com"));
    assert!(!declared.matches("notclaude.ai"));
    assert!(!declared.matches("evil.com"));

    let from_trait: DeclaredDomains = (&hosts).into();
    assert_eq!(from_trait.domains(), declared.domains());
}

#[test]
fn declared_domains_new_validation() {
    let declared = DeclaredDomains::new([".example.com", "api.example.com"]).unwrap();
    assert!(declared.matches("example.com"));
    assert!(declared.matches("sub.example.com"));
    assert!(declared.matches("api.example.com"));
    assert!(!declared.matches("notexample.com"));

    // Invalid host name inside iterator
    assert!(DeclaredDomains::new(["valid.com", "invalid..com"]).is_err());
}
