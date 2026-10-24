// Ported from CodexBar Sources/CodexBarCLI/CLIServeAuth.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIServeCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
//
// Bearer token authentication, constant-time digest comparison, and host/startup
// security validation for `ariadusage serve`.

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use super::http_server::{CliLocalHttpAllowedHosts, CliLocalHttpResponse};

/// Bearer token validator for `serve` routes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliServeDashboardAuth {
    expected_token_digest: Option<[u8; 32]>,
}

impl CliServeDashboardAuth {
    #[must_use]
    pub fn new(token: Option<&str>) -> Self {
        let normalized = token.map(str::trim).filter(|s| !s.is_empty());
        let expected_token_digest = normalized.map(|value| Sha256::digest(value.as_bytes()).into());
        Self {
            expected_token_digest,
        }
    }

    #[must_use]
    pub fn is_configured(&self) -> bool {
        self.expected_token_digest.is_some()
    }

    #[must_use]
    pub fn authorizes(&self, authorization_header: Option<&str>) -> bool {
        let Some(expected_digest) = self.expected_token_digest.as_ref() else {
            return false;
        };
        let Some(candidate) = Self::bearer_token(authorization_header) else {
            return false;
        };
        let candidate_digest: [u8; 32] = Sha256::digest(candidate.as_bytes()).into();
        Self::constant_time_equals(&candidate_digest, expected_digest)
    }

    #[must_use]
    pub fn bearer_token(authorization_header: Option<&str>) -> Option<&str> {
        let header = authorization_header?.trim();
        let prefix = "Bearer ";
        if header.len() < prefix.len() {
            return None;
        }
        let (head, tail) = header.split_at(prefix.len());
        if !head.eq_ignore_ascii_case(prefix) {
            return None;
        }
        let token = tail.trim();
        (!token.is_empty()).then_some(token)
    }

    #[must_use]
    pub fn constant_time_equals(lhs: &[u8], rhs: &[u8]) -> bool {
        if lhs.len() != rhs.len() {
            return false;
        }
        let mut diff = 0u8;
        for (l, r) in lhs.iter().zip(rhs.iter()) {
            diff |= l ^ r;
        }
        diff == 0
    }
}

/// Startup validation error when binding `serve` to a non-loopback host.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CliServeStartupError {
    #[error(
        "Refusing to bind serve to non-loopback host '{host}' without a dashboard token. \
         Set ARIADUSAGE_DASHBOARD_TOKEN (recommended) or pass --dashboard-token <token>, \
         and pass --allow-plain-http to acknowledge cleartext HTTP transport, \
         or use --host 127.0.0.1."
    )]
    MissingDashboardToken { host: String },
    #[error(
        "Refusing to bind serve to non-loopback host '{host}' over plain HTTP without \
         explicit acknowledgement. Bearer tokens and dashboard responses cross the network \
         in cleartext; pass --allow-plain-http only on a trusted network or behind a \
         TLS-terminating reverse proxy."
    )]
    PlainHttpNotAccepted { host: String },
}

impl CliServeStartupError {
    #[must_use]
    pub fn error_description(&self) -> String {
        self.to_string()
    }
}

/// Bind-host normalization, Host-header allowlist derivation, and startup checks.
pub struct CliServeSecurity;

impl CliServeSecurity {
    #[must_use]
    pub fn bind_host(raw_host: Option<&str>) -> String {
        let trimmed = raw_host.unwrap_or("127.0.0.1").trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("localhost") {
            return "127.0.0.1".to_string();
        }
        trimmed.to_string()
    }

    #[must_use]
    pub fn is_loopback_host(raw_host: &str) -> bool {
        let normalized = Self::bind_host(Some(raw_host)).to_ascii_lowercase();
        if matches!(
            normalized.as_str(),
            "127.0.0.1" | "::1" | "[::1]" | "0:0:0:0:0:0:0:1" | "[0:0:0:0:0:0:0:1]"
        ) {
            return true;
        }
        Self::is_supported_ipv4_bind_host(&normalized) && normalized.starts_with("127.")
    }

    #[must_use]
    pub fn is_wildcard_host(raw_host: &str) -> bool {
        let normalized = Self::bind_host(Some(raw_host)).to_ascii_lowercase();
        matches!(
            normalized.as_str(),
            "0.0.0.0" | "::" | "[::]" | "0:0:0:0:0:0:0:0" | "[0:0:0:0:0:0:0:0]"
        )
    }

    #[must_use]
    pub fn is_supported_ipv4_bind_host(raw_host: &str) -> bool {
        let normalized = Self::bind_host(Some(raw_host));
        let octets: Vec<&str> = normalized.split('.').collect();
        if octets.len() != 4 {
            return false;
        }
        for octet in octets {
            if octet.is_empty() {
                return false;
            }
            if octet.len() > 1 && octet.starts_with('0') {
                return false;
            }
            if !octet.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
            if octet.parse::<u8>().is_err() {
                return false;
            }
        }
        true
    }

    #[must_use]
    pub fn allowed_hosts(for_bind_host: &str) -> CliLocalHttpAllowedHosts {
        let normalized = Self::bind_host(Some(for_bind_host)).to_ascii_lowercase();
        if matches!(
            normalized.as_str(),
            "127.0.0.1" | "::1" | "[::1]" | "0:0:0:0:0:0:0:1" | "[0:0:0:0:0:0:0:1]"
        ) {
            return CliLocalHttpAllowedHosts::LoopbackOnly;
        }
        if Self::is_wildcard_host(&normalized) {
            return CliLocalHttpAllowedHosts::Any;
        }
        CliLocalHttpAllowedHosts::LoopbackAnd(BTreeSet::from([normalized]))
    }

    pub fn validate_startup(
        host: &str,
        auth: &CliServeDashboardAuth,
        allow_plain_http: bool,
    ) -> Result<(), CliServeStartupError> {
        if Self::is_loopback_host(host) {
            return Ok(());
        }
        if !auth.is_configured() {
            return Err(CliServeStartupError::MissingDashboardToken {
                host: Self::bind_host(Some(host)),
            });
        }
        if !allow_plain_http {
            return Err(CliServeStartupError::PlainHttpNotAccepted {
                host: Self::bind_host(Some(host)),
            });
        }
        Ok(())
    }

    #[must_use]
    pub fn requires_account_data_auth(host: &str, auth: &CliServeDashboardAuth) -> bool {
        !Self::is_loopback_host(host) || auth.is_configured()
    }
}

/// Standard `401 Unauthorized` response for `serve` endpoints.
#[must_use]
pub fn serve_unauthorized_response() -> CliLocalHttpResponse {
    CliLocalHttpResponse {
        status_code: 401,
        reason_phrase: "Unauthorized".to_string(),
        content_type: "application/json; charset=utf-8".to_string(),
        headers: BTreeMap::from([
            ("Cache-Control".to_string(), "no-store".to_string()),
            ("WWW-Authenticate".to_string(), "Bearer".to_string()),
        ]),
        body: br#"{"error":"unauthorized"}"#.to_vec(),
        usage_cache_keys: None,
    }
}
