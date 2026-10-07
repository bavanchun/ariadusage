use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use ariadusage_protocol::secret::SecretString;
use zeroize::Zeroizing;

use super::{BackendFuture, SecretBackend, SecretId, SecretLookup, SecretStoreError};
use crate::brokers::call::BrokerCall;

#[derive(Clone)]
struct MemoryItem {
    secret: SecretString,
    locked: bool,
}

#[derive(Clone)]
struct SafeStorageItem {
    key: Zeroizing<Vec<u8>>,
    locked: bool,
}

#[derive(Default)]
struct MemoryState {
    available: bool,
    dismissed: bool,
    timed_out: bool,
    items: HashMap<SecretId, MemoryItem>,
    safe_storage: HashMap<String, SafeStorageItem>,
    unlock_attempts: usize,
}

/// In-memory backend for tests and callers that supply their own isolated state.
#[derive(Clone, Default)]
pub struct MemoryBackend {
    state: Arc<Mutex<MemoryState>>,
}

impl MemoryBackend {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MemoryState {
                available: true,
                ..MemoryState::default()
            })),
        }
    }

    pub fn set_available(&self, available: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.available = available;
        }
    }

    pub fn set_unlock_dismissed(&self, dismissed: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.dismissed = dismissed;
        }
    }

    pub fn set_unlock_timeout(&self, timed_out: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.timed_out = timed_out;
        }
    }

    pub fn insert(&self, id: SecretId, secret: SecretString, locked: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.items.insert(id, MemoryItem { secret, locked });
        }
    }

    pub fn insert_safe_storage(
        &self,
        app_attribute: impl Into<String>,
        key: Zeroizing<Vec<u8>>,
        locked: bool,
    ) {
        if let Ok(mut state) = self.state.lock() {
            state
                .safe_storage
                .insert(app_attribute.into(), SafeStorageItem { key, locked });
        }
    }

    pub fn unlock_attempts(&self) -> usize {
        self.state
            .lock()
            .map(|state| state.unlock_attempts)
            .unwrap_or_default()
    }
}

impl fmt::Debug for MemoryBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let counts = self
            .state
            .lock()
            .map(|state| (state.items.len(), state.safe_storage.len()))
            .unwrap_or_default();
        f.debug_struct("MemoryBackend")
            .field("item_count", &counts.0)
            .field("safe_storage_count", &counts.1)
            .finish()
    }
}

impl SecretBackend for MemoryBackend {
    fn lookup_background<'a>(
        &'a self,
        id: &'a SecretId,
        _call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<SecretLookup, SecretStoreError>> {
        Box::pin(async move {
            let state = self.state.lock().map_err(|_| SecretStoreError::Storage)?;
            if !state.available {
                return Err(SecretStoreError::Unavailable);
            }
            Ok(match state.items.get(id) {
                Some(item) if item.locked => SecretLookup::Locked,
                Some(item) if item.secret.expose_secret().is_empty() => SecretLookup::Invalid,
                Some(item) => SecretLookup::Found(item.secret.clone()),
                None => SecretLookup::Missing,
            })
        })
    }

    fn lookup_user<'a>(
        &'a self,
        id: &'a SecretId,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<SecretLookup, SecretStoreError>> {
        Box::pin(async move {
            if call.interaction == ariadusage_core::pipeline::FetchInteraction::Background {
                return self.lookup_background(id, call).await;
            }
            let mut state = self.state.lock().map_err(|_| SecretStoreError::Storage)?;
            if !state.available {
                return Err(SecretStoreError::Unavailable);
            }
            if state.timed_out {
                return Err(SecretStoreError::Timeout);
            }
            if state.dismissed && state.items.get(id).is_some_and(|item| item.locked) {
                return Err(SecretStoreError::Dismissed);
            }
            if state.items.get(id).is_some_and(|item| item.locked) {
                state.unlock_attempts += 1;
                if let Some(item) = state.items.get_mut(id) {
                    item.locked = false;
                }
            }
            Ok(match state.items.get(id) {
                Some(item) if item.secret.expose_secret().is_empty() => SecretLookup::Invalid,
                Some(item) => SecretLookup::Found(item.secret.clone()),
                None => SecretLookup::Missing,
            })
        })
    }

    fn set_user<'a>(
        &'a self,
        id: &'a SecretId,
        secret: &'a SecretString,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<(), SecretStoreError>> {
        Box::pin(async move {
            if call.interaction == ariadusage_core::pipeline::FetchInteraction::Background {
                return Err(SecretStoreError::Locked);
            }
            if secret.expose_secret().is_empty() {
                return Err(SecretStoreError::Invalid);
            }
            let mut state = self.state.lock().map_err(|_| SecretStoreError::Storage)?;
            if !state.available {
                return Err(SecretStoreError::Unavailable);
            }
            if state.timed_out {
                return Err(SecretStoreError::Timeout);
            }
            if state.dismissed && state.items.get(id).is_some_and(|item| item.locked) {
                return Err(SecretStoreError::Dismissed);
            }
            if state.items.get(id).is_some_and(|item| item.locked) {
                state.unlock_attempts += 1;
            }
            state.items.insert(
                id.clone(),
                MemoryItem {
                    secret: secret.clone(),
                    locked: false,
                },
            );
            Ok(())
        })
    }

    fn chromium_key_background<'a>(
        &'a self,
        app_attribute: &'a str,
        _call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<Option<Zeroizing<Vec<u8>>>, SecretStoreError>> {
        Box::pin(async move {
            let state = self.state.lock().map_err(|_| SecretStoreError::Storage)?;
            if !state.available {
                return Err(SecretStoreError::Unavailable);
            }
            match state.safe_storage.get(app_attribute) {
                Some(item) if item.locked => Err(SecretStoreError::Locked),
                Some(item) => Ok(Some(Zeroizing::new(item.key.to_vec()))),
                None => Ok(None),
            }
        })
    }

    fn chromium_key_user<'a>(
        &'a self,
        app_attribute: &'a str,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<Option<Zeroizing<Vec<u8>>>, SecretStoreError>> {
        Box::pin(async move {
            if call.interaction == ariadusage_core::pipeline::FetchInteraction::Background {
                return self.chromium_key_background(app_attribute, call).await;
            }
            let mut state = self.state.lock().map_err(|_| SecretStoreError::Storage)?;
            if !state.available {
                return Err(SecretStoreError::Unavailable);
            }
            if state.timed_out {
                return Err(SecretStoreError::Timeout);
            }
            if state.dismissed {
                return Err(SecretStoreError::Dismissed);
            }
            let should_unlock = state
                .safe_storage
                .get(app_attribute)
                .is_some_and(|item| item.locked);
            if should_unlock {
                state.unlock_attempts += 1;
            }
            match state.safe_storage.get_mut(app_attribute) {
                Some(item) => {
                    if should_unlock {
                        item.locked = false;
                    }
                    Ok(Some(Zeroizing::new(item.key.to_vec())))
                }
                None => Ok(None),
            }
        })
    }
}
