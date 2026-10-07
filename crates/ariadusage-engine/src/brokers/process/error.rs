use std::time::Instant;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

impl std::fmt::Display for OutputStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        })
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProcessError {
    #[error("process call was cancelled")]
    Cancelled,
    #[error("process timed out")]
    TimedOut,
    #[error("process {stream} exceeded its {cap}-byte output cap")]
    OutputTooLarge { stream: OutputStream, cap: usize },
    #[error("process could not be launched")]
    LaunchFailed,
    #[error("background process launch is suppressed until {until:?}")]
    LaunchSuppressed { until: Instant },
    #[error("process broker is shutting down")]
    ShuttingDown,
    #[error("process operation is unsupported on this platform")]
    Unsupported,
    #[error("process I/O failed")]
    Io,
}
