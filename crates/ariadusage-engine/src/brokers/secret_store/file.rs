use std::fmt;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use std::collections::BTreeMap;

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_protocol::secret::SecretString;

use super::{BackendFuture, SecretBackend, SecretId, SecretLookup, SecretStoreError};
use crate::brokers::call::BrokerCall;

/// Consent-gated `$XDG_DATA_HOME/ariadusage/secrets.json` backend.
pub struct FileBackend {
    path: PathBuf,
    consented: bool,
}

impl FileBackend {
    pub fn new(path: PathBuf, consented: bool) -> Self {
        Self { path, consented }
    }
}

impl fmt::Debug for FileBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileBackend")
            .field("path_len", &self.path.as_os_str().len())
            .field("consented", &self.consented)
            .finish()
    }
}

impl SecretBackend for FileBackend {
    fn lookup_background<'a>(
        &'a self,
        id: &'a SecretId,
        _call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<SecretLookup, SecretStoreError>> {
        Box::pin(async move { self.lookup(id) })
    }

    fn lookup_user<'a>(
        &'a self,
        id: &'a SecretId,
        _call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<SecretLookup, SecretStoreError>> {
        Box::pin(async move { self.lookup(id) })
    }

    fn set_user<'a>(
        &'a self,
        id: &'a SecretId,
        secret: &'a SecretString,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<(), SecretStoreError>> {
        Box::pin(async move {
            if call.interaction != FetchInteraction::UserInitiated {
                return Err(SecretStoreError::Locked);
            }
            self.set(id, secret)
        })
    }
}

impl FileBackend {
    fn lookup(&self, id: &SecretId) -> Result<SecretLookup, SecretStoreError> {
        if !self.consented {
            return Err(SecretStoreError::ConsentRequired);
        }

        #[cfg(target_os = "linux")]
        {
            let secrets = self.read_secrets()?;
            match secrets.get(id.as_setting_id().as_str()) {
                Some(secret) if !secret.expose_secret().is_empty() => {
                    Ok(SecretLookup::Found(secret.clone()))
                }
                Some(_) => Ok(SecretLookup::Invalid),
                None => Ok(SecretLookup::Missing),
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = id;
            Err(SecretStoreError::Unsupported)
        }
    }

    fn set(&self, id: &SecretId, secret: &SecretString) -> Result<(), SecretStoreError> {
        if !self.consented {
            return Err(SecretStoreError::ConsentRequired);
        }
        if secret.expose_secret().is_empty() {
            return Err(SecretStoreError::Invalid);
        }

        #[cfg(target_os = "linux")]
        {
            use std::io::Write;

            use zeroize::Zeroizing;

            if let Some(parent) = self.path.parent()
                && parent.exists()
            {
                self.check_parent()?;
            }
            if self.path.exists() {
                self.check_existing_file()?;
            }

            let mut secrets = self.read_secrets()?;
            secrets.insert(id.as_setting_id().as_str().to_owned(), secret.clone());
            let mut encoded = Zeroizing::new(Vec::with_capacity(1024));
            serde_json::to_writer(&mut *encoded, &secrets)
                .map_err(|_| SecretStoreError::Storage)?;
            encoded.flush().map_err(|_| SecretStoreError::Storage)?;

            crate::private_file::write_private(
                &self.path,
                &encoded,
                &crate::private_file::WriteHooks::default(),
            )
            .map_err(|_| SecretStoreError::Storage)?;
            self.check_parent()?;
            self.check_existing_file()?;
            Ok(())
        }

        #[cfg(not(target_os = "linux"))]
        {
            let _ = id;
            Err(SecretStoreError::Unsupported)
        }
    }

    #[cfg(target_os = "linux")]
    fn read_secrets(&self) -> Result<BTreeMap<String, SecretString>, SecretStoreError> {
        use std::io::Read;

        use zeroize::Zeroizing;

        let Some(parent) = self.path.parent() else {
            return Err(SecretStoreError::Storage);
        };
        if !parent.exists() {
            return Ok(BTreeMap::new());
        }
        self.check_parent()?;
        let Some(mut file) =
            crate::trust::check_file_trust(&self.path, crate::trust::TrustPolicy::PRIVATE)
                .map_err(|_| SecretStoreError::Storage)?
        else {
            return Ok(BTreeMap::new());
        };

        let mut bytes = Zeroizing::new(Vec::new());
        file.read_to_end(&mut bytes)
            .map_err(|_| SecretStoreError::Storage)?;
        serde_json::from_slice(&bytes).map_err(|_| SecretStoreError::Storage)
    }

    #[cfg(target_os = "linux")]
    fn check_parent(&self) -> Result<(), SecretStoreError> {
        let parent = self.path.parent().ok_or(SecretStoreError::Storage)?;
        crate::trust::check_parent_trust(parent, crate::trust::TrustPolicy::PRIVATE)
            .map_err(|_| SecretStoreError::Storage)
    }

    #[cfg(target_os = "linux")]
    fn check_existing_file(&self) -> Result<(), SecretStoreError> {
        crate::trust::check_file_trust(&self.path, crate::trust::TrustPolicy::PRIVATE)
            .map_err(|_| SecretStoreError::Storage)?
            .ok_or(SecretStoreError::Storage)?;
        Ok(())
    }
}
