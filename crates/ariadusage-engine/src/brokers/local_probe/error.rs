// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityStatusProbe+PortDetection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use crate::brokers::process::ProcessError;
use ariadusage_protocol::secret::SecretString;
use thiserror::Error;

/// Retains subprocess identity without exposing captured diagnostics in logs.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ListenerCommandError {
    #[error("antigravity-lsof executable unavailable")]
    ExecutableUnavailable,
    #[error("{label}: {source}")]
    Process {
        label: &'static str,
        source: ProcessError,
    },
    #[error("antigravity-lsof exited with status {code}")]
    NonZeroExit { code: i32, stderr: SecretString },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProbeError {
    #[error("local probe is unsupported on this platform")]
    Unsupported,
    #[error(transparent)]
    ListenerCommand(ListenerCommandError),
    /// A recoverable lsof diagnostic retained while the process is starting.
    #[error("{0}")]
    ListenerDiscoveryPending(#[source] ListenerCommandError),
    #[error("procfs unavailable")]
    ProcUnavailable,
    #[error("no listening ports found")]
    NoListeningPorts,
    #[error("local probe was cancelled")]
    Cancelled,
    #[error("local probe timed out")]
    TimedOut,
}
