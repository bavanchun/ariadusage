// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthCredentialModels.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthCredentials.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use ariadusage_core::digest::sha256_hex;
use ariadusage_core::oauth::TokenOwner;
use ariadusage_core::pipeline::ClassifiedError;
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::Value;

use crate::brokers::call::BrokerCall;
use crate::brokers::credential_file::{CredentialDecl, CredentialFileError, StatFingerprint, read};

pub const CLAUDE_OAUTH_TOKEN_ENV_KEY: &str = "ARIADUSAGE_CLAUDE_OAUTH_TOKEN";
pub const CLAUDE_OAUTH_SCOPES_ENV_KEY: &str = "ARIADUSAGE_CLAUDE_OAUTH_SCOPES";

/// Provenance of a loaded Claude OAuth credential record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaudeOAuthCredentialSource {
    Environment,
    CredentialsFile,
}

/// Typed errors when reading or parsing Claude OAuth credentials.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaudeOAuthCredentialsError {
    #[error("Claude OAuth credentials are invalid")]
    DecodeFailed,
    #[error("Claude OAuth credentials missing. Run `claude` to authenticate")]
    MissingOAuth,
    #[error("Claude credentials contain MCP OAuth state only (no claudeAiOauth)")]
    McpOAuthOnly,
    #[error("Claude OAuth access token missing. Run `claude` to authenticate")]
    MissingAccessToken,
    #[error("Claude OAuth credentials not found. Run `claude` to authenticate")]
    NotFound,
    #[error("Claude OAuth credentials file is untrusted")]
    Untrusted,
    #[error("Claude OAuth credentials read failed")]
    ReadFailed,
    #[error("Claude OAuth token is missing required user:profile scope")]
    MissingRequiredScope,
    #[error("Claude OAuth token expired")]
    Expired,
    #[error("Claude OAuth refresh is delegated to Claude CLI")]
    RefreshDelegatedToClaudeCli,
    #[error("operation cancelled")]
    Cancelled,
}

impl ClaudeOAuthCredentialsError {
    /// Maps a credential error to a `ClassifiedError` for the fetch pipeline.
    pub fn into_classified_error(self) -> ClassifiedError {
        match self {
            Self::NotFound | Self::MissingOAuth | Self::MissingAccessToken => ClassifiedError::new(
                ProviderErrorKind::MissingCredential,
                "Claude OAuth credentials not found",
            ),
            Self::McpOAuthOnly => ClassifiedError::new(
                ProviderErrorKind::MissingCredential,
                "Claude credentials contain MCP OAuth state only",
            ),
            Self::DecodeFailed => ClassifiedError::new(
                ProviderErrorKind::ParseFailure,
                "Claude OAuth credentials are invalid",
            ),
            Self::Untrusted | Self::MissingRequiredScope => ClassifiedError::new(
                ProviderErrorKind::PermissionDenied,
                "Claude OAuth credentials lacked required permissions",
            ),
            Self::ReadFailed => ClassifiedError::new(
                ProviderErrorKind::MissingCredential,
                "Claude OAuth credentials could not be read",
            ),
            Self::Expired | Self::RefreshDelegatedToClaudeCli => ClassifiedError::new(
                ProviderErrorKind::AuthenticationExpired,
                "Claude OAuth session expired",
            ),
            Self::Cancelled => ClassifiedError::new(
                ProviderErrorKind::Unknown,
                "Claude OAuth credential read cancelled",
            ),
        }
    }
}

/// Parsed Claude OAuth credentials with secret-redacted token fields.
#[derive(Clone, PartialEq, Eq)]
pub struct ClaudeOAuthCredentials {
    pub access_token: SecretString,
    pub refresh_token: Option<SecretString>,
    pub expires_at: Option<Timestamp>,
    pub scopes: Vec<String>,
    pub rate_limit_tier: Option<String>,
    pub subscription_type: Option<String>,
}

impl fmt::Debug for ClaudeOAuthCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaudeOAuthCredentials")
            .field("access_token", &"[redacted]")
            .field("has_refresh_token", &self.has_refresh_token())
            .field("expires_at", &self.expires_at)
            .field("scopes", &self.scopes)
            .field("rate_limit_tier", &self.rate_limit_tier)
            .field("subscription_type", &self.subscription_type)
            .finish()
    }
}

impl ClaudeOAuthCredentials {
    /// Returns true if the credential is expired at `now` (or has no `expires_at` timestamp).
    pub fn is_expired_at(&self, now: Timestamp) -> bool {
        match self.expires_at {
            Some(exp) => now >= exp,
            None => true,
        }
    }

    /// Returns true if `refresh_token` is present and non-empty after trimming.
    pub fn has_refresh_token(&self) -> bool {
        self.refresh_token
            .as_ref()
            .is_some_and(|t| !t.expose_secret().trim().is_empty())
    }

    /// Returns true if `scopes` includes `required`.
    pub fn has_scope(&self, required: &str) -> bool {
        self.scopes.iter().any(|s| s == required)
    }

    /// Computes the one-way history owner identifier (`ariadusage:claude-oauth-history-owner:v1`).
    ///
    /// Prefers `refresh_token` over `access_token` so routine access-token rotations keep the
    /// same history bucket.
    pub fn history_owner_identifier(&self) -> Option<String> {
        if let Some(refresh) = self
            .refresh_token
            .as_ref()
            .map(|s| s.expose_secret().trim())
            .filter(|s| !s.is_empty())
        {
            return Some(sha256_hex(
                "ariadusage:claude-oauth-history-owner:v1",
                &["refresh", refresh],
            ));
        }

        let access = self.access_token.expose_secret().trim();
        if access.is_empty() {
            return None;
        }
        Some(sha256_hex(
            "ariadusage:claude-oauth-history-owner:v1",
            &["access", access],
        ))
    }

    /// Validates a 64-character lowercase hexadecimal history owner identifier.
    pub fn normalized_history_owner_identifier(identifier: Option<&str>) -> Option<String> {
        let normalized = identifier?.trim().to_ascii_lowercase();
        if normalized.len() == 64
            && normalized
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            Some(normalized)
        } else {
            None
        }
    }

    /// Returns true when the payload is a JSON object with `mcpOAuth` and no `claudeAiOauth`.
    pub fn is_mcp_oauth_only_payload(data: &[u8]) -> bool {
        let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(data) else {
            return false;
        };
        let has_claude_oauth = map.get("claudeAiOauth").is_some_and(|v| !v.is_null());
        let has_mcp_oauth = map.get("mcpOAuth").is_some_and(|v| !v.is_null());
        !has_claude_oauth && has_mcp_oauth
    }

    /// Parses Claude OAuth credentials from JSON bytes (`.credentials.json`).
    pub fn parse(data: &[u8]) -> Result<Self, ClaudeOAuthCredentialsError> {
        if Self::is_mcp_oauth_only_payload(data) {
            return Err(ClaudeOAuthCredentialsError::McpOAuthOnly);
        }

        let root: RootPayload =
            serde_json::from_slice(data).map_err(|_| ClaudeOAuthCredentialsError::DecodeFailed)?;
        let oauth = root
            .claude_ai_oauth
            .ok_or(ClaudeOAuthCredentialsError::MissingOAuth)?;

        let access_raw = oauth
            .access_token
            .as_ref()
            .map(|s| s.expose_secret().trim())
            .unwrap_or("");
        if access_raw.is_empty() {
            return Err(ClaudeOAuthCredentialsError::MissingAccessToken);
        }

        let refresh_token = oauth
            .refresh_token
            .as_ref()
            .map(|s| s.expose_secret().trim())
            .filter(|s| !s.is_empty())
            .map(SecretString::new);

        let expires_at = oauth.expires_at.and_then(millis_to_timestamp);

        let rate_limit_tier = oauth
            .rate_limit_tier
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let subscription_type = oauth
            .subscription_type
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        Ok(Self {
            access_token: SecretString::new(access_raw),
            refresh_token,
            expires_at,
            scopes: oauth.scopes.unwrap_or_default(),
            rate_limit_tier,
            subscription_type,
        })
    }
}

#[derive(Deserialize)]
struct RootPayload {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OAuthPayload>,
}

#[derive(Deserialize)]
struct OAuthPayload {
    #[serde(rename = "accessToken")]
    access_token: Option<SecretString>,
    #[serde(rename = "refreshToken")]
    refresh_token: Option<SecretString>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<f64>,
    scopes: Option<Vec<String>>,
    #[serde(rename = "rateLimitTier")]
    rate_limit_tier: Option<String>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
}

fn millis_to_timestamp(millis: f64) -> Option<Timestamp> {
    if !millis.is_finite() {
        return None;
    }
    let nanos = (millis * 1_000_000.0).round();
    if nanos < (i128::MIN as f64) || nanos > (i128::MAX as f64) {
        return None;
    }
    Timestamp::from_nanosecond(nanos as i128).ok()
}

/// Loaded Claude OAuth credential record with ownership and source metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeOAuthCredentialRecord {
    pub credentials: ClaudeOAuthCredentials,
    pub owner: TokenOwner,
    pub source: ClaudeOAuthCredentialSource,
    pub stat_fingerprint: Option<StatFingerprint>,
}

impl ClaudeOAuthCredentialRecord {
    pub fn history_owner_identifier(&self) -> Option<String> {
        self.credentials.history_owner_identifier()
    }
}

/// Computes the one-way profile digest (`ariadusage:claude-oauth-cache-profile:v1`) for a credentials path.
pub fn credentials_profile_identifier(credentials_path: &Path) -> String {
    let path_str = credentials_path.to_string_lossy();
    sha256_hex(
        "ariadusage:claude-oauth-cache-profile:v1",
        &[path_str.as_bytes()],
    )
}

/// Loads an environment-provided Claude OAuth credential record if `ARIADUSAGE_CLAUDE_OAUTH_TOKEN` is set.
pub fn load_from_environment(
    env: &BTreeMap<String, SecretString>,
) -> Option<ClaudeOAuthCredentialRecord> {
    let token = env
        .get(CLAUDE_OAUTH_TOKEN_ENV_KEY)
        .map(SecretString::expose_secret)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;

    let scopes = match env
        .get(CLAUDE_OAUTH_SCOPES_ENV_KEY)
        .map(SecretString::expose_secret)
    {
        Some(raw) => {
            let parsed: Vec<String> = raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            if parsed.is_empty() {
                vec!["user:profile".to_owned()]
            } else {
                parsed
            }
        }
        None => vec!["user:profile".to_owned()],
    };

    Some(ClaudeOAuthCredentialRecord {
        credentials: ClaudeOAuthCredentials {
            access_token: SecretString::new(token),
            refresh_token: None,
            expires_at: Some(Timestamp::MAX),
            scopes,
            rate_limit_tier: None,
            subscription_type: None,
        },
        owner: TokenOwner::Environment,
        source: ClaudeOAuthCredentialSource::Environment,
        stat_fingerprint: None,
    })
}

/// Reads and parses a Claude OAuth credential record from `credentials_path` through the `CredentialFile` broker.
pub fn load_from_credentials_file(
    credentials_path: &Path,
    call: &BrokerCall,
) -> Result<ClaudeOAuthCredentialRecord, ClaudeOAuthCredentialsError> {
    let decl = CredentialDecl::new(credentials_path);
    let read_res = read(&decl, call).map_err(|err| match err {
        CredentialFileError::NotFound => ClaudeOAuthCredentialsError::NotFound,
        CredentialFileError::Untrusted => ClaudeOAuthCredentialsError::Untrusted,
        CredentialFileError::Cancelled => ClaudeOAuthCredentialsError::Cancelled,
        CredentialFileError::Unreadable
        | CredentialFileError::TooLarge
        | CredentialFileError::Malformed => ClaudeOAuthCredentialsError::ReadFailed,
    })?;

    let credentials = ClaudeOAuthCredentials::parse(&read_res.bytes)?;
    Ok(ClaudeOAuthCredentialRecord {
        credentials,
        owner: TokenOwner::Cli,
        source: ClaudeOAuthCredentialSource::CredentialsFile,
        stat_fingerprint: Some(read_res.stat),
    })
}
