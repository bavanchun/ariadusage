// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeCredentialRouting.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::cookie::normalize;
use ariadusage_protocol::secret::SecretString;

const COOKIE_HEADER_PREFIX: &str = "cookie:";

/// Resolved credential route for a Claude account token or manual cookie header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeCredentialRoute {
    None,
    Oauth { access_token: SecretString },
    WebCookie { header: SecretString },
    AdminApiKey(SecretString),
}

/// Classifies and normalizes Claude account tokens and manual cookie headers.
pub struct ClaudeCredentialRouting;

impl ClaudeCredentialRouting {
    /// Returns true if `raw` is a Claude OAuth access token (`sk-ant-oat...`, with optional `Bearer `).
    pub fn is_oauth_access_token(raw: Option<&str>) -> bool {
        Self::normalize_oauth_access_token(raw).is_some()
    }

    /// Returns true if `raw` is a Claude Admin API key (`sk-ant-admin...`, with optional `Bearer `).
    pub fn is_admin_api_key(raw: Option<&str>) -> bool {
        Self::normalize_admin_api_key(raw).is_some()
    }

    /// Normalizes a Claude Admin API key by stripping surrounding whitespace and an optional `Bearer ` prefix.
    pub fn normalize_admin_api_key(raw: Option<&str>) -> Option<SecretString> {
        let cleaned = Self::strip_bearer_and_validate_token_shape(raw?)?;
        if has_ascii_prefix_ignore_case(cleaned, "sk-ant-admin") {
            Some(SecretString::new(cleaned))
        } else {
            None
        }
    }

    /// Resolves `token_account_token` (preferred) or `manual_cookie_header` (fallback) into a route.
    pub fn resolve(
        token_account_token: Option<&str>,
        manual_cookie_header: Option<&str>,
    ) -> ClaudeCredentialRoute {
        if let Some(primary) = Self::non_empty(token_account_token) {
            if let Some(admin_key) = Self::normalize_admin_api_key(Some(primary)) {
                return ClaudeCredentialRoute::AdminApiKey(admin_key);
            }
            if let Some(access_token) = Self::normalize_oauth_access_token(Some(primary)) {
                return ClaudeCredentialRoute::Oauth { access_token };
            }
            if let Some(cookie_header) = Self::normalize_web_cookie_header(Some(primary)) {
                return ClaudeCredentialRoute::WebCookie {
                    header: cookie_header,
                };
            }
        }

        if let Some(fallback_cookie) = Self::normalize_web_cookie_header(manual_cookie_header) {
            return ClaudeCredentialRoute::WebCookie {
                header: fallback_cookie,
            };
        }

        ClaudeCredentialRoute::None
    }

    pub(crate) fn normalize_oauth_access_token(raw: Option<&str>) -> Option<SecretString> {
        let cleaned = Self::strip_bearer_and_validate_token_shape(raw?)?;
        if has_ascii_prefix_ignore_case(cleaned, "sk-ant-oat") {
            Some(SecretString::new(cleaned))
        } else {
            None
        }
    }

    fn strip_bearer_and_validate_token_shape(raw: &str) -> Option<&str> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        if contains_ascii_ignore_case(trimmed, COOKIE_HEADER_PREFIX) || trimmed.contains('=') {
            return None;
        }
        let cleaned = if has_ascii_prefix_ignore_case(trimmed, "bearer ") {
            trimmed[7..].trim()
        } else {
            trimmed
        };
        if cleaned.is_empty() {
            None
        } else {
            Some(cleaned)
        }
    }

    fn normalize_web_cookie_header(raw: Option<&str>) -> Option<SecretString> {
        let normalized = normalize::normalize(raw?)?;
        if normalized.contains('=') {
            Some(SecretString::new(normalized))
        } else {
            let mut header = String::from("sessionKey");
            header.push('=');
            header.push_str(&normalized);
            Some(SecretString::new(header))
        }
    }

    fn non_empty(raw: Option<&str>) -> Option<&str> {
        let trimmed = raw?.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }
}

fn has_ascii_prefix_ignore_case(haystack: &str, prefix: &str) -> bool {
    haystack
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn contains_ascii_ignore_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}
