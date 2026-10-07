use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProbeError {
    #[error("local probe is unsupported on this platform")]
    Unsupported,
    #[error("procfs unavailable")]
    ProcUnavailable,
    #[error("no listening ports found")]
    NoListeningPorts,
    #[error("local probe was cancelled")]
    Cancelled,
    #[error("local probe timed out")]
    TimedOut,
}
