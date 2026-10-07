use ariadusage_core::cookie as cookie_core;
use ariadusage_protocol::secret::SecretString;
use cookie_core::{DeclaredDomains, normalize};
use reqwest::header::{COOKIE, HeaderName};

use super::net::Origin;

/// A cookie value to attach to a request, as raw pairs or formatted header.
#[derive(Clone)]
pub enum CookieValue {
    Header(SecretString),
    Raw(SecretString),
}

impl CookieValue {
    pub fn header(value: impl Into<SecretString>) -> Self {
        Self::Header(value.into())
    }

    pub fn raw(value: impl Into<SecretString>) -> Self {
        Self::Raw(value.into())
    }

    pub fn as_secret(&self) -> &SecretString {
        match self {
            Self::Header(s) | Self::Raw(s) => s,
        }
    }
}

impl std::fmt::Debug for CookieValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookieValue")
            .field("value", &"[redacted]")
            .finish()
    }
}

/// Returns a `(HeaderName, SecretString)` credential pair for a request only when
/// the request origin's host matches the provider's declared cookie domains.
///
/// If the host does not match, or the cookie value does not normalize to a non-empty header,
/// returns `None`.
pub fn header_for_request(
    origin: &Origin,
    declared_domains: &DeclaredDomains,
    cookie_value: CookieValue,
) -> Option<(HeaderName, SecretString)> {
    if !declared_domains.matches(origin.host()) {
        return None;
    }

    let secret_ref = cookie_value.as_secret();
    let normalized = normalize(secret_ref.expose_secret())?;
    Some((COOKIE, SecretString::from(normalized)))
}
