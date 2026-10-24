// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityCLIPrintFailure.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Safe classification for failures from `agy -p /usage`.
//!
//! The CLI writes account URLs, proxy details and local paths to stderr.  The
//! classifier deliberately retains only a small, fixed diagnostic vocabulary.

use std::fmt;

use crate::brokers::process::ProcessError;

use super::cli_session::AuthenticationPrompt;
use super::status_probe::AntigravityStatusProbeError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntigravityCLIPrintFailure {
    ExecutableNotFound,
    LaunchFailed,
    Exited { code: i32, reason: ExitReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Unspecified,
    Network,
    EligibilityNetwork,
    Ineligible,
}

impl fmt::Display for AntigravityCLIPrintFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExecutableNotFound => formatter.write_str("agy executable not found"),
            Self::LaunchFailed => formatter.write_str("agy failed to launch"),
            Self::Exited { code, reason } => {
                write!(formatter, "agy exited {code}")?;
                match reason {
                    ExitReason::Unspecified => {}
                    ExitReason::Network => formatter.write_str(
                        "; a network request failed (check network or proxy settings)",
                    )?,
                    ExitReason::EligibilityNetwork => formatter.write_str(
                        "; the eligibility check failed on a network request (check network or proxy settings)",
                    )?,
                    ExitReason::Ineligible => {
                        formatter.write_str("; the account is not eligible for Antigravity")?
                    }
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for AntigravityCLIPrintFailure {}

impl AntigravityCLIPrintFailure {
    /// Classifies a process-broker failure without exposing broker diagnostics.
    pub fn process_error(error: &ProcessError) -> AntigravityStatusProbeError {
        match error {
            ProcessError::Cancelled => AntigravityStatusProbeError::Cancelled,
            ProcessError::TimedOut => AntigravityStatusProbeError::TimedOut,
            ProcessError::OutputTooLarge { .. } => {
                AntigravityStatusProbeError::ParseFailed("CLI usage report failed".to_owned())
            }
            // The broker intentionally does not preserve a public distinction
            // between executable preflight and spawn failure.  Callers that
            // resolve a missing executable before constructing a Command map
            // that case to ExecutableNotFound; broker launch errors stay safe.
            ProcessError::LaunchFailed
            | ProcessError::LaunchSuppressed { .. }
            | ProcessError::ShuttingDown
            | ProcessError::Unsupported
            | ProcessError::Io => AntigravityStatusProbeError::CliReportFailed(Self::LaunchFailed),
        }
    }

    /// Classifies stderr from a non-zero `agy` exit into a fixed diagnostic.
    pub fn non_zero_exit_error(code: i32, stderr: &[u8]) -> AntigravityStatusProbeError {
        let text = ascii_lowercase(stderr);
        if AuthenticationPrompt::contains(stderr)
            || SIGN_IN_MARKERS.iter().any(|marker| text.contains(marker))
        {
            return AntigravityStatusProbeError::AuthenticationRequired;
        }

        let eligibility_failed = ELIGIBILITY_MARKERS
            .iter()
            .any(|marker| text.contains(marker));
        let network_failed = NETWORK_MARKERS
            .iter()
            .any(|marker| marker_matches(&text, marker));
        if eligibility_failed {
            return AntigravityStatusProbeError::CliReportFailed(Self::Exited {
                code,
                reason: if network_failed {
                    ExitReason::EligibilityNetwork
                } else {
                    ExitReason::Ineligible
                },
            });
        }
        if network_failed {
            return AntigravityStatusProbeError::CliReportFailed(Self::Exited {
                code,
                reason: ExitReason::Network,
            });
        }
        AntigravityStatusProbeError::CliReportFailed(Self::Exited {
            code,
            reason: ExitReason::Unspecified,
        })
    }
}

const SIGN_IN_MARKERS: &[&str] = &[
    "not logged in",
    "not signed in",
    "unauthenticated",
    "authentication required",
    "login required",
    "please log in",
    "please sign in",
];

const ELIGIBILITY_MARKERS: &[&str] = &[
    "eligibility check failed",
    "not eligible",
    "does not support google tos",
    "unsupported country",
    "unsupported region",
];

const NETWORK_MARKERS: &[&str] = &[
    "eof",
    "timed out",
    "timeout",
    "deadline exceeded",
    "i/o timeout",
    "no such host",
    "dns",
    "connection refused",
    "connection reset",
    "network is unreachable",
    "unreachable",
    "proxy",
    "certificate",
    "tls",
    "ssl",
    "get \"http",
    "post \"http",
];

fn ascii_lowercase(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect()
}

fn marker_matches(text: &str, marker: &str) -> bool {
    if marker.starts_with("get \"") || marker.starts_with("post \"") {
        return text.contains(marker);
    }
    let mut offset = 0;
    while let Some(relative) = text[offset..].find(marker) {
        let start = offset + relative;
        let end = start + marker.len();
        let left_ok = start == 0 || !is_word(text.as_bytes()[start - 1]);
        let right_ok = end == text.len() || !is_word(text.as_bytes()[end]);
        if left_ok && right_ok {
            return true;
        }
        offset = start.saturating_add(1);
        if offset >= text.len() {
            break;
        }
    }
    false
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}
