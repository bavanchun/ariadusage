mod error;
pub mod file;
mod id;
pub mod memory;
pub mod safe_storage;
pub mod service;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use ariadusage_core::config::{Config, SecretPresence};
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_protocol::ids::SettingId;
use ariadusage_protocol::secret::SecretString;
use zeroize::Zeroizing;

use crate::brokers::call::BrokerCall;
use crate::brokers::secret_store::file::FileBackend;

pub use error::SecretStoreError;
pub use id::{SecretId, SecretIdError, SecretKind};

/// Result vocabulary for a secret lookup.
#[derive(Clone, PartialEq, Eq)]
pub enum SecretLookup {
    Found(SecretString),
    Missing,
    Locked,
    Unavailable,
    Invalid,
}

impl std::fmt::Debug for SecretLookup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Found(secret) => f.debug_tuple("Found").field(secret).finish(),
            Self::Missing => f.write_str("Missing"),
            Self::Locked => f.write_str("Locked"),
            Self::Unavailable => f.write_str("Unavailable"),
            Self::Invalid => f.write_str("Invalid"),
        }
    }
}

pub type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Operations supported by a SecretStore backend.
///
/// The background and user methods stay separate so background callers cannot request an
/// unlock or write through the handle API.
pub trait SecretBackend: Send + Sync {
    fn lookup_background<'a>(
        &'a self,
        id: &'a SecretId,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<SecretLookup, SecretStoreError>>;

    fn lookup_user<'a>(
        &'a self,
        id: &'a SecretId,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<SecretLookup, SecretStoreError>>;

    fn set_user<'a>(
        &'a self,
        id: &'a SecretId,
        secret: &'a SecretString,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<(), SecretStoreError>>;

    fn chromium_key_background<'a>(
        &'a self,
        _app_attribute: &'a str,
        _call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<Option<Zeroizing<Vec<u8>>>, SecretStoreError>> {
        Box::pin(async { Err(SecretStoreError::Unsupported) })
    }

    fn chromium_key_user<'a>(
        &'a self,
        _app_attribute: &'a str,
        _call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<Option<Zeroizing<Vec<u8>>>, SecretStoreError>> {
        Box::pin(async { Err(SecretStoreError::Unsupported) })
    }
}

/// A broker that combines the keyring with an explicitly consented private-file fallback.
pub struct SecretStore {
    backend: Arc<dyn SecretBackend>,
    file_fallback: Option<Arc<FileBackend>>,
    file_fallback_consented: bool,
}

impl SecretStore {
    pub fn new(backend: impl SecretBackend + 'static) -> Self {
        Self {
            backend: Arc::new(backend),
            file_fallback: None,
            file_fallback_consented: false,
        }
    }

    pub fn with_file_fallback(mut self, file_fallback: FileBackend, consented: bool) -> Self {
        self.file_fallback = Some(Arc::new(file_fallback));
        self.file_fallback_consented = consented;
        self
    }

    pub fn background(&self, call: BrokerCall) -> BackgroundHandle<'_> {
        BackgroundHandle {
            store: self,
            call,
            file_fallback_allowed: self.file_fallback_consented,
        }
    }

    pub fn user(&self, call: BrokerCall) -> UserHandle<'_> {
        UserHandle { store: self, call }
    }

    pub async fn presence(&self, config: &Config, call: BrokerCall) -> SecretPresence {
        let background = BackgroundHandle {
            store: self,
            call,
            file_fallback_allowed: self.file_fallback_consented
                && config.secret_file_fallback == Some(true),
        };
        let mut presence = SecretPresence::none();

        for descriptor in ariadusage_core::providers::first_party_order() {
            let provider = &descriptor.id;
            if let Some(id) = make_secret_id(provider.as_str(), "apiKey", None)
                && lookup_counts_as_set(background.lookup(&id).await)
            {
                presence.api_keys.insert(provider.clone());
            }
            if let Some(id) = make_secret_id(provider.as_str(), "cookieHeader", None)
                && lookup_counts_as_set(background.lookup(&id).await)
            {
                presence.cookie_headers.insert(provider.clone());
            }
        }

        for entry in &config.providers {
            let Some(provider_config) = entry.as_typed() else {
                continue;
            };
            let Some(accounts) = &provider_config.token_accounts else {
                continue;
            };
            for account in &accounts.accounts {
                let Some(id) =
                    make_secret_id(provider_config.id.as_str(), "accounts", Some(&account.id))
                else {
                    continue;
                };
                if lookup_counts_as_set(background.lookup(&id).await) {
                    presence
                        .token_account_tokens
                        .insert((provider_config.id.clone(), account.id.clone()));
                }
            }
        }

        presence
    }

    async fn lookup_background(
        &self,
        id: &SecretId,
        call: &BrokerCall,
        file_fallback_allowed: bool,
    ) -> SecretLookup {
        match self.backend.lookup_background(id, call).await {
            Ok(SecretLookup::Unavailable) if file_fallback_allowed => {
                self.file_lookup(id, call, false).await
            }
            Err(SecretStoreError::Unavailable) if file_fallback_allowed => {
                self.file_lookup(id, call, false).await
            }
            Ok(result) => result,
            Err(SecretStoreError::Locked) => SecretLookup::Locked,
            Err(SecretStoreError::Invalid) => SecretLookup::Invalid,
            Err(SecretStoreError::Unavailable | SecretStoreError::Unsupported) => {
                SecretLookup::Unavailable
            }
            Err(_) => SecretLookup::Unavailable,
        }
    }

    async fn lookup_user(&self, id: &SecretId, call: &BrokerCall) -> SecretLookup {
        if call.interaction == FetchInteraction::Background {
            return self
                .lookup_background(id, call, self.file_fallback_consented)
                .await;
        }
        match self.backend.lookup_user(id, call).await {
            Ok(SecretLookup::Unavailable) if self.file_fallback_consented => {
                self.file_lookup(id, call, true).await
            }
            Err(SecretStoreError::Unavailable) if self.file_fallback_consented => {
                self.file_lookup(id, call, true).await
            }
            Ok(result) => result,
            Err(SecretStoreError::Locked) => SecretLookup::Locked,
            Err(SecretStoreError::Invalid) => SecretLookup::Invalid,
            Err(SecretStoreError::Unavailable | SecretStoreError::Unsupported) => {
                SecretLookup::Unavailable
            }
            Err(_) => SecretLookup::Unavailable,
        }
    }

    async fn file_lookup(&self, id: &SecretId, call: &BrokerCall, user: bool) -> SecretLookup {
        let Some(file) = &self.file_fallback else {
            return SecretLookup::Unavailable;
        };
        let result = if user {
            file.lookup_user(id, call).await
        } else {
            file.lookup_background(id, call).await
        };
        result.unwrap_or(SecretLookup::Unavailable)
    }

    async fn set_user(
        &self,
        id: &SecretId,
        secret: &SecretString,
        call: &BrokerCall,
    ) -> Result<(), SecretStoreError> {
        if call.interaction == FetchInteraction::Background {
            return Err(SecretStoreError::Locked);
        }
        match self.backend.set_user(id, secret, call).await {
            Err(SecretStoreError::Unavailable) if self.file_fallback_consented => {
                let file = self
                    .file_fallback
                    .as_ref()
                    .ok_or(SecretStoreError::Unavailable)?;
                file.set_user(id, secret, call).await
            }
            result => result,
        }
    }
}

/// Read-only handle for work that must never unlock the keyring or write secrets.
pub struct BackgroundHandle<'a> {
    store: &'a SecretStore,
    call: BrokerCall,
    file_fallback_allowed: bool,
}

impl BackgroundHandle<'_> {
    pub async fn lookup(&self, id: &SecretId) -> SecretLookup {
        self.store
            .lookup_background(id, &self.call, self.file_fallback_allowed)
            .await
    }

    pub fn interaction(&self) -> FetchInteraction {
        self.call.interaction
    }
}

/// User-initiated handle that may unlock the keyring and write a secret.
pub struct UserHandle<'a> {
    store: &'a SecretStore,
    call: BrokerCall,
}

impl UserHandle<'_> {
    pub async fn lookup(&self, id: &SecretId) -> SecretLookup {
        self.store.lookup_user(id, &self.call).await
    }

    pub async fn set(&self, id: &SecretId, secret: &SecretString) -> Result<(), SecretStoreError> {
        self.store.set_user(id, secret, &self.call).await
    }

    pub fn interaction(&self) -> FetchInteraction {
        self.call.interaction
    }
}

fn lookup_counts_as_set(lookup: SecretLookup) -> bool {
    matches!(lookup, SecretLookup::Found(_) | SecretLookup::Locked)
}

fn make_secret_id(provider: &str, kind: &str, account: Option<&str>) -> Option<SecretId> {
    let setting = match (kind, account) {
        ("apiKey", None) => format!("providers.{provider}.apiKey"),
        ("cookieHeader", None) => format!("providers.{provider}.cookieHeader"),
        ("accounts", Some(account)) => {
            format!("providers.{provider}.accounts.{account}.token")
        }
        _ => return None,
    };
    let setting = SettingId::new(setting).ok()?;
    SecretId::from_setting_id(&setting).ok()
}
