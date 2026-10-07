//! Token ownership model.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Identifies the entity that owns and is responsible for refreshing an OAuth token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TokenOwner {
    /// Token is owned and managed by a provider's external CLI (e.g., Claude Code).
    Cli,
    /// Token is native to a provider CLI and must never be redeemed by AriadUsage (e.g., Codex).
    CliNative,
    /// Token was supplied directly via environment variables.
    Environment,
    /// Token was acquired from a read-only external tool or configuration source.
    ReadOnlyExternal,
    /// Token is owned directly by AriadUsage and stored via SecretStore.
    SelfOwned,
}

impl fmt::Display for TokenOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cli => write!(f, "cli"),
            Self::CliNative => write!(f, "cli_native"),
            Self::Environment => write!(f, "environment"),
            Self::ReadOnlyExternal => write!(f, "read_only_external"),
            Self::SelfOwned => write!(f, "self_owned"),
        }
    }
}
