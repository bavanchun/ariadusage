use std::fmt;
use std::sync::Arc;

use ariadusage_core::pipeline::FetchInteraction;
use zeroize::Zeroizing;

use super::{SecretBackend, SecretStoreError};
use crate::brokers::call::BrokerCall;

/// Errors produced while looking up a Chromium-family safe-storage key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SafeStorageError {
    NoSecretService,
    KWalletUnsupported,
    Locked,
    Dismissed,
    Timeout,
    Cancelled,
    Invalid,
    Unsupported,
    Storage,
}

impl fmt::Display for SafeStorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoSecretService => "secret service unavailable",
            Self::KWalletUnsupported => "KWallet safe-storage entries are unsupported",
            Self::Locked => "safe-storage keyring item is locked",
            Self::Dismissed => "safe-storage unlock was dismissed",
            Self::Timeout => "safe-storage lookup timed out",
            Self::Cancelled => "safe-storage lookup was cancelled",
            Self::Invalid => "safe-storage key is invalid",
            Self::Unsupported => "safe-storage lookup unsupported on this platform",
            Self::Storage => "safe-storage lookup failed",
        })
    }
}

impl fmt::Debug for SafeStorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoSecretService => "NoSecretService",
            Self::KWalletUnsupported => "KWalletUnsupported",
            Self::Locked => "Locked",
            Self::Dismissed => "Dismissed",
            Self::Timeout => "Timeout",
            Self::Cancelled => "Cancelled",
            Self::Invalid => "Invalid",
            Self::Unsupported => "Unsupported",
            Self::Storage => "Storage",
        })
    }
}

impl std::error::Error for SafeStorageError {}

/// Narrow seam used by browser importers to retrieve Chromium's safe-storage key.
pub struct SafeStorageKeySource {
    backend: Arc<dyn SecretBackend>,
}

impl SafeStorageKeySource {
    pub fn new(backend: impl SecretBackend + 'static) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }

    pub async fn chromium_safe_storage(
        &self,
        app_attribute: &str,
        call: &BrokerCall,
    ) -> Result<Zeroizing<Vec<u8>>, SafeStorageError> {
        if app_attribute.is_empty() {
            return Err(SafeStorageError::Invalid);
        }

        let result = match call.interaction {
            FetchInteraction::Background => {
                self.backend
                    .chromium_key_background(app_attribute, call)
                    .await
            }
            FetchInteraction::UserInitiated => {
                self.backend.chromium_key_user(app_attribute, call).await
            }
        }
        .map_err(map_error)?;

        result.ok_or(SafeStorageError::KWalletUnsupported)
    }
}

impl fmt::Debug for SafeStorageKeySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SafeStorageKeySource")
            .finish_non_exhaustive()
    }
}

fn map_error(error: SecretStoreError) -> SafeStorageError {
    match error {
        SecretStoreError::Unavailable => SafeStorageError::NoSecretService,
        SecretStoreError::Unsupported => SafeStorageError::KWalletUnsupported,
        SecretStoreError::Locked => SafeStorageError::Locked,
        SecretStoreError::Dismissed => SafeStorageError::Dismissed,
        SecretStoreError::Timeout => SafeStorageError::Timeout,
        SecretStoreError::Cancelled => SafeStorageError::Cancelled,
        SecretStoreError::Invalid => SafeStorageError::Invalid,
        SecretStoreError::ConsentRequired | SecretStoreError::Storage => SafeStorageError::Storage,
    }
}
