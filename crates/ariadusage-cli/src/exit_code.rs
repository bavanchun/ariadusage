// Ported from CodexBar Sources/CodexBarCLI/CLIExitCode.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Exit codes returned by the `ariadusage` CLI (`CLIExitCode.swift` + `docs/cli-exit-codes.md`).
//!
//! Exit code `17` is permanently reserved for unsupported platform secret operations and is never
//! reused by any other command.

use std::process::ExitCode as ProcessExitCode;

use ariadusage_protocol::usage::{ProviderError, ProviderErrorKind};

/// Canonical CLI exit codes (`CLIExitCode.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum CliExitCode {
    #[default]
    Success = 0,
    UnexpectedFailure = 1,
    ProviderNotInstalled = 2,
    ParseError = 3,
    Timeout = 4,
    InvalidArguments = 64,
}

/// Alias matching upstream `ExitCode` naming in `CLIExitCode.swift`.
pub type ExitCode = CliExitCode;

/// Provider error descriptors mapped by `CLIUsageCommand.mapError` (`CLIUsageCommand.swift:713`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliErrorDescriptor {
    BinaryNotFound,
    TimedOut,
    ParseFailed,
    NoRateLimitsFound,
    Other,
}

/// Maps a [`CliErrorDescriptor`] to [`CliExitCode`] (`CLIUsageCommand.mapError`).
#[must_use]
pub const fn map_cli_error(descriptor: CliErrorDescriptor) -> CliExitCode {
    match descriptor {
        CliErrorDescriptor::BinaryNotFound => CliExitCode::ProviderNotInstalled,
        CliErrorDescriptor::TimedOut => CliExitCode::Timeout,
        CliErrorDescriptor::ParseFailed | CliErrorDescriptor::NoRateLimitsFound => {
            CliExitCode::ParseError
        }
        CliErrorDescriptor::Other => CliExitCode::UnexpectedFailure,
    }
}

impl CliExitCode {
    #[allow(non_upper_case_globals)]
    pub const Failure: Self = Self::UnexpectedFailure;
    #[allow(non_upper_case_globals)]
    pub const ProviderMissing: Self = Self::ProviderNotInstalled;
    #[allow(non_upper_case_globals)]
    pub const BinaryNotFound: Self = Self::ProviderNotInstalled;

    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub const fn raw_value(self) -> i32 {
        self as i32
    }

    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Maps a [`ProviderErrorKind`] from an engine snapshot to the matching [`CliExitCode`]
    /// (`CLIUsageCommand.mapError` in `CLIUsageCommand.swift:713`).
    #[must_use]
    pub const fn from_provider_error_kind(kind: ProviderErrorKind) -> Self {
        match kind {
            ProviderErrorKind::ParseFailure => Self::ParseError,
            _ => Self::UnexpectedFailure,
        }
    }

    /// Maps a [`ProviderError`] from an engine snapshot to the matching [`CliExitCode`]
    /// (`CLIUsageCommand.mapError` in `CLIUsageCommand.swift:713-753`).
    #[must_use]
    pub fn from_provider_error(error: &ProviderError) -> Self {
        match error.kind {
            ProviderErrorKind::ParseFailure => Self::ParseError,
            _ => Self::from_error_message(&error.message),
        }
    }

    /// Maps an error message string to the matching [`CliExitCode`] using upstream's fallback
    /// heuristics (`CLIUsageCommand.mapError` in `CLIUsageCommand.swift:713-753`).
    #[must_use]
    pub fn from_error_message(message: &str) -> Self {
        let lower = message.to_ascii_lowercase();
        if lower.contains("binary not found")
            || lower.contains("not installed")
            || lower.contains("missing binary")
        {
            Self::ProviderNotInstalled
        } else if lower.contains("timed out") || lower.contains("timeout") {
            Self::Timeout
        } else if lower.contains("could not parse")
            || lower.contains("parse failed")
            || lower.contains("no rate limits found")
            || lower.contains("invalid json")
        {
            Self::ParseError
        } else {
            Self::UnexpectedFailure
        }
    }
}

impl From<CliExitCode> for ProcessExitCode {
    fn from(value: CliExitCode) -> Self {
        Self::from(value.as_u8())
    }
}
