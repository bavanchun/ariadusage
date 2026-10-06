use std::fmt;
use std::path::PathBuf;

use ariadusage_core::config::ConfigError;

use crate::paths::PathError;

/// Typed errors produced by engine configuration store operations.
#[derive(thiserror::Error)]
pub enum StoreError {
    #[error("configuration file not found: {0}")]
    NotFound(PathBuf),

    #[error("failed to decode configuration: {0}")]
    Decode(#[from] ConfigError),

    #[error("untrusted configuration file: {0}")]
    UntrustedFile(String),

    #[error("untrusted configuration directory: {0}")]
    UntrustedDirectory(String),

    #[error("lock rejected: {0}")]
    LockRejected(String),

    #[error("lock contention: write lock held by another process")]
    LockContention,

    #[error("configuration path error: {0}")]
    Path(#[from] PathError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("operation unsupported on this platform: {0}")]
    Unsupported(&'static str),
}

impl fmt::Debug for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(p) => f.debug_tuple("NotFound").field(p).finish(),
            Self::Decode(e) => f.debug_tuple("Decode").field(e).finish(),
            Self::UntrustedFile(r) => f.debug_tuple("UntrustedFile").field(r).finish(),
            Self::UntrustedDirectory(r) => f.debug_tuple("UntrustedDirectory").field(r).finish(),
            Self::LockRejected(r) => f.debug_tuple("LockRejected").field(r).finish(),
            Self::LockContention => f.write_str("LockContention"),
            Self::Path(e) => f.debug_tuple("Path").field(e).finish(),
            Self::Io(e) => f.debug_struct("Io").field("kind", &e.kind()).finish(),
            Self::Unsupported(m) => f.debug_tuple("Unsupported").field(m).finish(),
        }
    }
}
