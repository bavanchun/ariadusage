use std::fmt;

#[cfg(target_os = "linux")]
use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::future::Future;
#[cfg(target_os = "linux")]
use std::time::Duration;

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_protocol::secret::SecretString;

use super::{BackendFuture, SecretBackend, SecretId, SecretLookup, SecretStoreError};
use crate::brokers::call::BrokerCall;

#[cfg(target_os = "linux")]
const DBUS_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(target_os = "linux")]
const USER_UNLOCK_TIMEOUT: Duration = Duration::from_secs(120);

/// Secret Service backend. The kill switch is injected by the caller.
pub struct ServiceBackend {
    disabled: bool,
}

impl ServiceBackend {
    pub const fn new(disabled: bool) -> Self {
        Self { disabled }
    }
}

impl fmt::Debug for ServiceBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceBackend")
            .field("disabled", &self.disabled)
            .finish()
    }
}

impl SecretBackend for ServiceBackend {
    fn lookup_background<'a>(
        &'a self,
        id: &'a SecretId,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<SecretLookup, SecretStoreError>> {
        Box::pin(async move {
            if self.disabled {
                return Ok(SecretLookup::Unavailable);
            }
            #[cfg(target_os = "linux")]
            {
                lookup_secret(id, call, false).await
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (id, call);
                Ok(SecretLookup::Unavailable)
            }
        })
    }

    fn lookup_user<'a>(
        &'a self,
        id: &'a SecretId,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<SecretLookup, SecretStoreError>> {
        Box::pin(async move {
            if self.disabled {
                return Ok(SecretLookup::Unavailable);
            }
            #[cfg(target_os = "linux")]
            {
                lookup_secret(
                    id,
                    call,
                    call.interaction == FetchInteraction::UserInitiated,
                )
                .await
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (id, call);
                Ok(SecretLookup::Unavailable)
            }
        })
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
            if self.disabled {
                return Err(SecretStoreError::Unavailable);
            }
            if secret.expose_secret().is_empty() {
                return Err(SecretStoreError::Invalid);
            }
            #[cfg(target_os = "linux")]
            {
                set_secret(id, secret, call).await
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (id, call);
                Err(SecretStoreError::Unavailable)
            }
        })
    }

    fn chromium_key_background<'a>(
        &'a self,
        app_attribute: &'a str,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<Option<zeroize::Zeroizing<Vec<u8>>>, SecretStoreError>> {
        Box::pin(async move {
            if self.disabled {
                return Err(SecretStoreError::Unavailable);
            }
            #[cfg(target_os = "linux")]
            {
                chromium_key(app_attribute, call, false).await
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (app_attribute, call);
                Err(SecretStoreError::Unavailable)
            }
        })
    }

    fn chromium_key_user<'a>(
        &'a self,
        app_attribute: &'a str,
        call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<Option<zeroize::Zeroizing<Vec<u8>>>, SecretStoreError>> {
        Box::pin(async move {
            if self.disabled {
                return Err(SecretStoreError::Unavailable);
            }
            #[cfg(target_os = "linux")]
            {
                chromium_key(
                    app_attribute,
                    call,
                    call.interaction == FetchInteraction::UserInitiated,
                )
                .await
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = (app_attribute, call);
                Err(SecretStoreError::Unavailable)
            }
        })
    }
}

#[cfg(target_os = "linux")]
async fn lookup_secret(
    id: &SecretId,
    call: &BrokerCall,
    user_initiated: bool,
) -> Result<SecretLookup, SecretStoreError> {
    use zeroize::Zeroizing;

    let service = connect(call).await?;
    let collection = timed(call, DBUS_TIMEOUT, service.get_default_collection()).await?;
    let attrs = id.attributes();
    let search = attrs
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<HashMap<_, _>>();
    let items = timed(call, DBUS_TIMEOUT, collection.search_items(search)).await?;
    if items.is_empty() {
        return Ok(SecretLookup::Missing);
    }

    let mut found_locked = false;
    for item in items {
        let locked = timed(call, DBUS_TIMEOUT, item.is_locked()).await?;
        if locked {
            found_locked = true;
            if !user_initiated {
                continue;
            }
            timed(call, USER_UNLOCK_TIMEOUT, item.unlock()).await?;
        }
        let mut secret = Zeroizing::new(timed(call, DBUS_TIMEOUT, item.get_secret()).await?);
        if secret.is_empty() {
            return Ok(SecretLookup::Invalid);
        }
        let bytes = std::mem::take(&mut *secret);
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => {
                drop(Zeroizing::new(error.into_bytes()));
                return Ok(SecretLookup::Invalid);
            }
        };
        return Ok(SecretLookup::Found(SecretString::new(text)));
    }

    if found_locked {
        Ok(SecretLookup::Locked)
    } else {
        Ok(SecretLookup::Missing)
    }
}

#[cfg(target_os = "linux")]
async fn set_secret(
    id: &SecretId,
    secret: &SecretString,
    call: &BrokerCall,
) -> Result<(), SecretStoreError> {
    let service = connect(call).await?;
    let collection = timed(call, DBUS_TIMEOUT, service.get_default_collection()).await?;
    if timed(call, DBUS_TIMEOUT, collection.is_locked()).await? {
        timed(call, USER_UNLOCK_TIMEOUT, collection.unlock()).await?;
        if timed(call, DBUS_TIMEOUT, collection.is_locked()).await? {
            return Err(SecretStoreError::Locked);
        }
    }

    let attrs = id.attributes();
    let properties = attrs
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<HashMap<_, _>>();
    timed(
        call,
        DBUS_TIMEOUT,
        collection.create_item(
            &id.label(),
            properties,
            secret.expose_secret().as_bytes(),
            true,
            "text/plain",
        ),
    )
    .await?;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn chromium_key(
    app_attribute: &str,
    call: &BrokerCall,
    user_initiated: bool,
) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, SecretStoreError> {
    use std::collections::HashMap;

    use zeroize::Zeroizing;

    let service = connect(call).await?;
    let results = timed(
        call,
        DBUS_TIMEOUT,
        service.search_items(HashMap::from([("application", app_attribute)])),
    )
    .await?;

    let (item, locked) = if let Some(item) = results.unlocked.into_iter().next() {
        (item, false)
    } else if let Some(item) = results.locked.into_iter().next() {
        (item, true)
    } else {
        return Err(SecretStoreError::Unsupported);
    };

    if locked {
        if !user_initiated {
            return Err(SecretStoreError::Locked);
        }
        timed(call, USER_UNLOCK_TIMEOUT, item.unlock()).await?;
    }
    if timed(call, DBUS_TIMEOUT, item.is_locked()).await? {
        return Err(SecretStoreError::Locked);
    }
    let key = Zeroizing::new(timed(call, DBUS_TIMEOUT, item.get_secret()).await?);
    if key.is_empty() {
        return Err(SecretStoreError::Invalid);
    }
    Ok(Some(key))
}

#[cfg(target_os = "linux")]
async fn connect(
    call: &BrokerCall,
) -> Result<secret_service::SecretService<'static>, SecretStoreError> {
    use secret_service::{EncryptionType, SecretService};

    tokio::select! {
        _ = call.cancel.cancelled() => Err(SecretStoreError::Cancelled),
        result = tokio::time::timeout(DBUS_TIMEOUT, SecretService::connect(EncryptionType::Dh)) => {
            match result {
                Err(_) => Err(SecretStoreError::Timeout),
                Ok(Err(_)) => Err(SecretStoreError::Unavailable),
                Ok(Ok(service)) => Ok(service),
            }
        }
    }
}

#[cfg(target_os = "linux")]
async fn timed<F, T>(
    call: &BrokerCall,
    duration: Duration,
    future: F,
) -> Result<T, SecretStoreError>
where
    F: Future<Output = Result<T, secret_service::Error>>,
{
    tokio::select! {
        _ = call.cancel.cancelled() => Err(SecretStoreError::Cancelled),
        result = tokio::time::timeout(duration, future) => {
            match result {
                Err(_) => Err(SecretStoreError::Timeout),
                Ok(result) => result.map_err(map_error),
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn map_error(error: secret_service::Error) -> SecretStoreError {
    use secret_service::Error;

    match error {
        Error::Locked => SecretStoreError::Locked,
        Error::Prompt => SecretStoreError::Dismissed,
        Error::Unavailable | Error::NoResult => SecretStoreError::Unavailable,
        _ => SecretStoreError::Storage,
    }
}
