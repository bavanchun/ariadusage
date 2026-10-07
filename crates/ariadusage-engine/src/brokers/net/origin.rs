use std::collections::BTreeSet;
use std::fmt;

use ariadusage_core::hosts::ProviderHosts;
use reqwest::Url;

/// An HTTP origin, normalized to a scheme, host, and effective port.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

impl Origin {
    pub fn of(url: &Url) -> Option<Self> {
        if !url.username().is_empty() || url.password().is_some() {
            return None;
        }

        let scheme = url.scheme().to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return None;
        }

        Some(Self {
            scheme,
            host: url.host_str()?.to_ascii_lowercase(),
            port: url.port_or_known_default()?,
        })
    }

    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Debug for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Origin")
            .field("scheme", &self.scheme)
            .field("host", &self.host)
            .field("port", &self.port)
            .finish()
    }
}

/// A non-empty set of origins a provider is allowed to contact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredOrigins(BTreeSet<Origin>);

impl DeclaredOrigins {
    pub fn new(origins: impl IntoIterator<Item = Origin>) -> Result<Self, OriginError> {
        let origins = origins.into_iter().collect::<BTreeSet<_>>();
        if origins.is_empty() {
            return Err(OriginError::Empty);
        }
        Ok(Self(origins))
    }

    pub fn from_provider_hosts(hosts: &ProviderHosts) -> Self {
        let origins = hosts
            .hosts()
            .iter()
            .map(|host| Origin {
                scheme: "https".to_owned(),
                host: host.as_str().to_owned(),
                port: 443,
            })
            .collect();
        Self(origins)
    }

    pub fn from_urls(urls: impl IntoIterator<Item = Url>) -> Result<Self, OriginError> {
        let origins = urls
            .into_iter()
            .map(|url| Origin::of(&url).ok_or(OriginError::Invalid))
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(origins)
    }

    pub fn contains(&self, origin: &Origin) -> bool {
        self.0.contains(origin)
    }

    pub fn contains_url(&self, url: &Url) -> bool {
        Origin::of(url).is_some_and(|origin| self.contains(&origin))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OriginError {
    #[error("declared origins must not be empty")]
    Empty,
    #[error("invalid declared origin")]
    Invalid,
}
