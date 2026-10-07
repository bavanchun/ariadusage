//! Ownership evidence rules for CLI configuration state.

use super::owner::TokenOwner;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Observed evidence of whether an external CLI is signed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OwnershipEvidence {
    /// Definitive evidence of a signed-in session.
    SignedIn,
    /// Definitive proof of absence or cleanly signed-out configuration.
    SignedOut,
    /// Present but unreadable, malformed, or ambiguous configuration.
    Indeterminate,
}

impl fmt::Display for OwnershipEvidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SignedIn => write!(f, "signed_in"),
            Self::SignedOut => write!(f, "signed_out"),
            Self::Indeterminate => write!(f, "indeterminate"),
        }
    }
}

/// Evaluates whether the absence of a CLI credential file and the configuration
/// evidence release a token chain to AriadUsage ownership (`SelfOwned`).
///
/// Only positive proof of a signed-out state with no CLI credential file releases
/// the chain. Present-but-unreadable, malformed, or ambiguous evidence preserves CLI ownership.
#[inline]
pub fn can_release_to_self_owned(has_cli_file: bool, evidence: OwnershipEvidence) -> bool {
    !has_cli_file && matches!(evidence, OwnershipEvidence::SignedOut)
}

/// Resolves the effective token owner based on the preferred candidate, the presence
/// of a CLI credential file, and configuration ownership evidence.
pub fn resolve_chain_owner(
    preferred: TokenOwner,
    has_cli_file: bool,
    evidence: OwnershipEvidence,
) -> TokenOwner {
    if preferred == TokenOwner::SelfOwned {
        if can_release_to_self_owned(has_cli_file, evidence) {
            TokenOwner::SelfOwned
        } else {
            TokenOwner::Cli
        }
    } else {
        preferred
    }
}
