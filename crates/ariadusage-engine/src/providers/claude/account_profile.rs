// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeAccountProfile.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::Path;

use ariadusage_protocol::secret::SecretString;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::brokers::call::BrokerCall;

use super::config_paths::ClaudeConfigPaths;

/// Maximum byte size read from Claude's plaintext `.config.json` / `.claude.json` when
/// inspecting `oauthAccount.accountUuid` (64 MiB, accommodating large project histories).
pub const MAX_ACCOUNT_CONFIG_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Deserialize)]
struct ClaudeConfigAccount {
    #[serde(rename = "oauthAccount")]
    oauth_account: Option<OAuthAccount>,
}

#[derive(Deserialize)]
struct OAuthAccount {
    #[serde(rename = "accountUuid")]
    account_uuid: Option<String>,
}

/// What Claude's plaintext account config can prove about CLI credential ownership.
///
/// `SignedOut` requires positive evidence (no config file, or a cleanly parsed config with no
/// OAuth account); an unreadable, malformed, or unrecognized config is `Indeterminate` because
/// it cannot establish that CLI storage is absent.
#[derive(Clone, PartialEq, Eq)]
pub enum ConfigOwnershipEvidence {
    SignedIn { account_uuid: String },
    SignedOut,
    Indeterminate,
}

impl fmt::Debug for ConfigOwnershipEvidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignedIn { .. } => f
                .debug_struct("SignedIn")
                .field("account_uuid", &"[redacted]")
                .finish(),
            Self::SignedOut => f.write_str("SignedOut"),
            Self::Indeterminate => f.write_str("Indeterminate"),
        }
    }
}

/// Prompt-free Claude profile identity derived only from Claude-owned plain-text configuration.
pub struct ClaudeAccountProfile;

impl ClaudeAccountProfile {
    /// Returns the signed-in `accountUuid` from Claude's plaintext `.config.json`, if present.
    pub fn account_uuid(
        env: &BTreeMap<String, SecretString>,
        working_directory: &Path,
        home_directory: &Path,
        call: &BrokerCall,
    ) -> Option<String> {
        match Self::config_ownership_evidence(env, working_directory, home_directory, call) {
            ConfigOwnershipEvidence::SignedIn { account_uuid } => Some(account_uuid),
            ConfigOwnershipEvidence::SignedOut | ConfigOwnershipEvidence::Indeterminate => None,
        }
    }

    /// Evaluates what Claude's plaintext `.config.json` proves about CLI credential ownership.
    pub fn config_ownership_evidence(
        env: &BTreeMap<String, SecretString>,
        working_directory: &Path,
        home_directory: &Path,
        call: &BrokerCall,
    ) -> ConfigOwnershipEvidence {
        let path = ClaudeConfigPaths::account_config_path_from_secret_env(
            env,
            working_directory,
            home_directory,
        );
        Self::config_ownership_evidence_at_path(&path, call)
    }

    /// Evaluates ownership evidence directly from a resolved `.config.json` path.
    pub fn config_ownership_evidence_at_path(
        account_config_path: &Path,
        call: &BrokerCall,
    ) -> ConfigOwnershipEvidence {
        if call.cancel.is_cancelled() {
            return ConfigOwnershipEvidence::Indeterminate;
        }
        let meta = match std::fs::symlink_metadata(account_config_path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // A verifiably missing config is positive evidence that no signed-in CLI exists here.
                return ConfigOwnershipEvidence::SignedOut;
            }
            Err(_) => {
                // Present but unreadable (permissions, I/O): cannot prove CLI storage is absent.
                return ConfigOwnershipEvidence::Indeterminate;
            }
        };
        if !meta.file_type().is_file() || meta.len() > MAX_ACCOUNT_CONFIG_BYTES {
            return ConfigOwnershipEvidence::Indeterminate;
        }

        let file = match std::fs::File::open(account_config_path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return ConfigOwnershipEvidence::SignedOut;
            }
            Err(_) => {
                return ConfigOwnershipEvidence::Indeterminate;
            }
        };
        let mut bytes = Vec::new();
        if file
            .take(MAX_ACCOUNT_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() as u64 > MAX_ACCOUNT_CONFIG_BYTES
        {
            return ConfigOwnershipEvidence::Indeterminate;
        }

        let Ok(decoded) = serde_json::from_slice::<ClaudeConfigAccount>(&bytes) else {
            return ConfigOwnershipEvidence::Indeterminate;
        };
        let Some(oauth_account) = decoded.oauth_account else {
            return ConfigOwnershipEvidence::SignedOut;
        };
        let Some(uuid) = oauth_account
            .account_uuid
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            // An OAuth account stanza without a usable identity is not proof of a signed-out CLI.
            return ConfigOwnershipEvidence::Indeterminate;
        };

        ConfigOwnershipEvidence::SignedIn {
            account_uuid: uuid.to_owned(),
        }
    }

    /// Process-local ownership key for Claude TUI reuse. Missing identity fails closed with `fallback_id`.
    pub fn session_scope(
        env: &BTreeMap<String, SecretString>,
        working_directory: &Path,
        home_directory: &Path,
        fallback_id: &str,
        call: &BrokerCall,
    ) -> String {
        let identity = Self::account_uuid(env, working_directory, home_directory, call);
        Self::make_session_scope(
            env,
            working_directory,
            home_directory,
            identity.as_deref(),
            fallback_id,
        )
    }

    /// Stable ownership key only when Claude's selected profile exposes an account identity.
    pub fn identified_session_scope(
        env: &BTreeMap<String, SecretString>,
        working_directory: &Path,
        home_directory: &Path,
        call: &BrokerCall,
    ) -> Option<String> {
        let identity = Self::account_uuid(env, working_directory, home_directory, call)?;
        Some(Self::make_session_scope(
            env,
            working_directory,
            home_directory,
            Some(&identity),
            "",
        ))
    }

    fn make_session_scope(
        env: &BTreeMap<String, SecretString>,
        working_directory: &Path,
        home_directory: &Path,
        identity: Option<&str>,
        fallback_id: &str,
    ) -> String {
        let account_config_path = ClaudeConfigPaths::account_config_path_from_secret_env(
            env,
            working_directory,
            home_directory,
        );
        let credentials_path = ClaudeConfigPaths::credentials_path_from_secret_env(
            env,
            working_directory,
            home_directory,
        );
        let account_str = account_config_path.to_string_lossy();
        let creds_str = credentials_path.to_string_lossy();
        let normalized_identity = identity
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase);

        let material = if let Some(id) = normalized_identity {
            format!("claude:cli-session:v2:{account_str}:{creds_str}:{id}")
        } else {
            let fallback_lower = fallback_id.trim().to_ascii_lowercase();
            format!("claude:cli-session-ephemeral:v2:{account_str}:{creds_str}:{fallback_lower}")
        };

        let digest = Sha256::digest(material.as_bytes());
        let mut hex = String::with_capacity(64);
        for byte in digest {
            let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{byte:02x}"));
        }
        hex
    }
}
