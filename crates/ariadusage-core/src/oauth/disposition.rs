//! HTTP OAuth refresh response disposition table.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Disposition indicating how an OAuth refresh failure should be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RefreshDisposition {
    /// Terminal failure indicating invalid grant or revoked authorization; do not retry.
    Terminal,
    /// Transient failure (e.g. rate limit, temporary server error, undecodable response); retry with backoff.
    Transient,
    /// Status code is not an authentication/authorization failure; let general pipeline error handling proceed.
    Unhandled,
}

impl fmt::Display for RefreshDisposition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Terminal => write!(f, "terminal"),
            Self::Transient => write!(f, "transient"),
            Self::Unhandled => write!(f, "unhandled"),
        }
    }
}

#[derive(Deserialize)]
struct OAuthErrorBody {
    error: Option<String>,
}

/// Extracts the OAuth error code from a JSON response body if present.
pub fn extract_oauth_error_code(body: &[u8]) -> Option<String> {
    let parsed: OAuthErrorBody = serde_json::from_slice(body).ok()?;
    parsed.error
}

/// Classifies an HTTP status code and response body from a refresh request.
///
/// Rules:
/// - 400 or 401 with `error == "invalid_grant"` -> `Terminal`.
/// - Other 400 or 401 responses (including undecodable bodies) -> `Transient`.
/// - Other status codes -> `Unhandled`.
pub fn refresh_disposition(status_code: u16, body: &[u8]) -> RefreshDisposition {
    if status_code != 400 && status_code != 401 {
        return RefreshDisposition::Unhandled;
    }

    if extract_oauth_error_code(body).as_deref() == Some("invalid_grant") {
        return RefreshDisposition::Terminal;
    }

    RefreshDisposition::Transient
}
