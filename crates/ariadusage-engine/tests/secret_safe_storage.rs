use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::secret_store::memory::MemoryBackend;
use ariadusage_engine::brokers::secret_store::safe_storage::{
    SafeStorageError, SafeStorageKeySource,
};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

fn call(interaction: FetchInteraction) -> BrokerCall {
    BrokerCall {
        interaction,
        cancel: CancellationToken::new(),
        request_id: "safe-storage-test".to_owned(),
    }
}

#[tokio::test]
async fn background_locked_key_returns_locked_without_unlock_attempt() {
    let backend = MemoryBackend::new();
    backend.insert_safe_storage("Google Chrome", Zeroizing::new(vec![7, 8, 9]), true);
    let source = SafeStorageKeySource::new(backend.clone());

    assert_eq!(
        source
            .chromium_safe_storage("Google Chrome", &call(FetchInteraction::Background))
            .await
            .unwrap_err(),
        SafeStorageError::Locked
    );
    assert_eq!(backend.unlock_attempts(), 0);
}

#[tokio::test]
async fn user_initiated_lookup_unlocks_and_returns_zeroizing_key() {
    let backend = MemoryBackend::new();
    backend.insert_safe_storage("Chromium", Zeroizing::new(vec![1, 2, 3, 4]), true);
    let source = SafeStorageKeySource::new(backend.clone());
    let key = source
        .chromium_safe_storage("Chromium", &call(FetchInteraction::UserInitiated))
        .await
        .unwrap();

    assert_eq!(&*key, &[1, 2, 3, 4]);
    assert_eq!(backend.unlock_attempts(), 1);
}

#[tokio::test]
async fn dismissed_unlock_is_not_retried_or_converted_to_missing() {
    let backend = MemoryBackend::new();
    backend.insert_safe_storage("Brave", Zeroizing::new(vec![2, 4, 6]), true);
    backend.set_unlock_dismissed(true);
    let source = SafeStorageKeySource::new(backend.clone());

    assert_eq!(
        source
            .chromium_safe_storage("Brave", &call(FetchInteraction::UserInitiated))
            .await
            .unwrap_err(),
        SafeStorageError::Dismissed
    );
    assert_eq!(backend.unlock_attempts(), 0);
}

#[tokio::test]
async fn no_service_and_unsupported_keyring_have_distinct_errors() {
    let backend = MemoryBackend::new();
    let source = SafeStorageKeySource::new(backend.clone());
    assert_eq!(
        source
            .chromium_safe_storage("Chrome", &call(FetchInteraction::Background))
            .await
            .unwrap_err(),
        SafeStorageError::KWalletUnsupported
    );

    backend.set_available(false);
    assert_eq!(
        source
            .chromium_safe_storage("Chrome", &call(FetchInteraction::Background))
            .await
            .unwrap_err(),
        SafeStorageError::NoSecretService
    );
}

#[tokio::test]
async fn safe_storage_debug_redacts_app_attribute() {
    let source = SafeStorageKeySource::new(MemoryBackend::new());
    assert_eq!(format!("{source:?}"), "SafeStorageKeySource { .. }");
}
