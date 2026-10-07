//! Expiration policy decision table for OAuth tokens.

use serde::{Deserialize, Serialize};
use std::fmt;

use super::owner::TokenOwner;
use crate::pipeline::{FetchInteraction, FetchRuntime};

/// Reason why an expired token cannot be refreshed or delegated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExpireFailureReason {
    /// AriadUsage CLI never invokes child CLI processes for delegated refresh.
    CliRuntimeNeverDelegates,
    /// Background delegated refresh is not allowed without explicit user opt-in.
    DelegationNotAllowed,
    /// Native CLI credentials must be refreshed by the native tool, never redeemed.
    NativeRefreshRequired,
    /// No refresh token is available to perform a redemption.
    NoRefreshToken,
    /// Read-only external source credentials cannot be redeemed.
    ReadOnlySource,
}

impl fmt::Display for ExpireFailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CliRuntimeNeverDelegates => write!(f, "cli_runtime_never_delegates"),
            Self::DelegationNotAllowed => write!(f, "delegation_not_allowed"),
            Self::NativeRefreshRequired => write!(f, "native_refresh_required"),
            Self::NoRefreshToken => write!(f, "no_refresh_token"),
            Self::ReadOnlySource => write!(f, "read_only_source"),
        }
    }
}

/// Action to take when an OAuth access token has expired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExpiredAction {
    /// Ask the external CLI to refresh the session (e.g. running `claude /status` in a PTY).
    Delegate,
    /// Redeem the refresh token directly through provider OAuth endpoints.
    Refresh,
    /// Do not attempt refresh or delegation; fail with the specified reason.
    Fail(ExpireFailureReason),
}

/// Evaluates what action to take when an OAuth token has expired.
///
/// Rules:
/// - `Cli` -> `Delegate`, except:
///   - `Fail(CliRuntimeNeverDelegates)` when the calling runtime is `FetchRuntime::Cli`.
///   - `Fail(DelegationNotAllowed)` when interaction is `FetchInteraction::Background` without `background_opt_in`.
/// - `CliNative` -> `Fail(NativeRefreshRequired)` (never redeemed).
/// - `Environment` -> `Fail(NoRefreshToken)`.
/// - `ReadOnlyExternal` -> `Fail(ReadOnlySource)`.
/// - `SelfOwned` -> `Refresh` if `has_refresh_token` is true, else `Fail(NoRefreshToken)`.
pub fn decide_on_expired(
    owner: TokenOwner,
    has_refresh_token: bool,
    runtime: FetchRuntime,
    interaction: FetchInteraction,
    background_opt_in: bool,
) -> ExpiredAction {
    match owner {
        TokenOwner::Cli => {
            if runtime == FetchRuntime::Cli {
                ExpiredAction::Fail(ExpireFailureReason::CliRuntimeNeverDelegates)
            } else if interaction == FetchInteraction::Background && !background_opt_in {
                ExpiredAction::Fail(ExpireFailureReason::DelegationNotAllowed)
            } else {
                ExpiredAction::Delegate
            }
        }
        TokenOwner::CliNative => ExpiredAction::Fail(ExpireFailureReason::NativeRefreshRequired),
        TokenOwner::Environment => ExpiredAction::Fail(ExpireFailureReason::NoRefreshToken),
        TokenOwner::ReadOnlyExternal => ExpiredAction::Fail(ExpireFailureReason::ReadOnlySource),
        TokenOwner::SelfOwned => {
            if has_refresh_token {
                ExpiredAction::Refresh
            } else {
                ExpiredAction::Fail(ExpireFailureReason::NoRefreshToken)
            }
        }
    }
}
