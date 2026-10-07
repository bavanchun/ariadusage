use std::collections::BTreeSet;

use crate::hosts::{HostName, HostsError, ProviderHosts};

/// Returns true if `host` matches `domain` exactly or as a subdomain dot-suffix,
/// after stripping any single leading dot from either operand.
///
/// For example, `claude.ai` matches `claude.ai`, `.claude.ai`, and `www.claude.ai`,
/// but never matches `notclaude.ai`.
pub fn domain_matches(domain: &str, host: &str) -> bool {
    let d = domain
        .strip_prefix('.')
        .unwrap_or(domain)
        .to_ascii_lowercase();
    let h = host.strip_prefix('.').unwrap_or(host).to_ascii_lowercase();

    if d.is_empty() || h.is_empty() {
        return false;
    }

    if h == d {
        return true;
    }

    if h.ends_with(&d) {
        let prefix_len = h.len() - d.len();
        if prefix_len > 0 && h.as_bytes()[prefix_len - 1] == b'.' {
            return true;
        }
    }

    false
}

/// A non-empty, normalized set of cookie domains declared for a provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredDomains {
    domains: Vec<String>,
}

impl DeclaredDomains {
    /// Creates a DeclaredDomains instance from an iterator of domain names.
    ///
    /// Empty sets or sets containing no valid domains return `HostsError::Empty`.
    pub fn new<I, S>(domains: I) -> Result<Self, HostsError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut set = BTreeSet::new();
        for d in domains {
            let s = d.as_ref().trim();
            let stripped = s.strip_prefix('.').unwrap_or(s).to_ascii_lowercase();
            if stripped.is_empty() {
                continue;
            }
            let _ = HostName::new(&stripped)?;
            set.insert(stripped);
        }
        if set.is_empty() {
            return Err(HostsError::Empty);
        }
        Ok(Self {
            domains: set.into_iter().collect(),
        })
    }

    /// Derives DeclaredDomains from a `ProviderHosts` instance.
    pub fn from_provider_hosts(hosts: &ProviderHosts) -> Self {
        let domains = hosts
            .hosts()
            .iter()
            .map(|h| {
                let s = h.as_str();
                s.strip_prefix('.').unwrap_or(s).to_ascii_lowercase()
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Self { domains }
    }

    pub fn domains(&self) -> &[String] {
        &self.domains
    }

    /// Returns true if any declared domain matches the given `host`.
    pub fn matches(&self, host: &str) -> bool {
        self.domains.iter().any(|d| domain_matches(d, host))
    }
}

impl From<&ProviderHosts> for DeclaredDomains {
    fn from(hosts: &ProviderHosts) -> Self {
        Self::from_provider_hosts(hosts)
    }
}
