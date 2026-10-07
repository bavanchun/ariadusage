//! Credential fall-through policy across discovery sources.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Outcome of reading an OAuth or credential source candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialReadOutcome {
    /// Credential successfully read and parsed.
    Success,
    /// Credential file or source does not exist.
    NotFound,
    /// Source exists but could not be read (e.g. permissions, I/O error, non-regular file).
    Unreadable,
    /// Source exists but failed trust checks (e.g. invalid owner, symlink, untrusted mode).
    Untrusted,
    /// Source exceeds the size cap.
    TooLarge,
    /// Source was read but contents could not be decoded.
    DecodeFailed,
    /// Source was parsed but required tokens were missing.
    MissingTokens,
}

impl fmt::Display for CredentialReadOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::NotFound => write!(f, "not_found"),
            Self::Unreadable => write!(f, "unreadable"),
            Self::Untrusted => write!(f, "untrusted"),
            Self::TooLarge => write!(f, "too_large"),
            Self::DecodeFailed => write!(f, "decode_failed"),
            Self::MissingTokens => write!(f, "missing_tokens"),
        }
    }
}

/// Evaluates whether discovery may fall through to the next candidate source.
///
/// Only `NotFound` allows fall-through. An unreadable, untrusted, oversized,
/// or decode-failed file is an identity boundary, never an invitation to
/// silently substitute another session or application source.
#[inline]
pub fn may_fall_through(outcome: CredentialReadOutcome) -> bool {
    matches!(outcome, CredentialReadOutcome::NotFound)
}
