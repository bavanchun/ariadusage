use std::fmt;

/// Errors from secret operations. Variants intentionally carry no path, attribute, or value.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SecretStoreError {
    Unavailable,
    Locked,
    Dismissed,
    Timeout,
    Cancelled,
    Invalid,
    ConsentRequired,
    Unsupported,
    Storage,
}

impl fmt::Display for SecretStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "secret service unavailable",
            Self::Locked => "secret store is locked",
            Self::Dismissed => "secret store unlock was dismissed",
            Self::Timeout => "secret store operation timed out",
            Self::Cancelled => "secret store operation was cancelled",
            Self::Invalid => "secret is invalid",
            Self::ConsentRequired => "file fallback consent is required",
            Self::Unsupported => "secret operation unsupported on this platform",
            Self::Storage => "secret storage operation failed",
        })
    }
}

impl fmt::Debug for SecretStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "Unavailable",
            Self::Locked => "Locked",
            Self::Dismissed => "Dismissed",
            Self::Timeout => "Timeout",
            Self::Cancelled => "Cancelled",
            Self::Invalid => "Invalid",
            Self::ConsentRequired => "ConsentRequired",
            Self::Unsupported => "Unsupported",
            Self::Storage => "Storage",
        })
    }
}

impl std::error::Error for SecretStoreError {}
