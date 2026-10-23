use ariadusage_core::config::Config;
#[cfg(target_os = "linux")]
use ariadusage_core::config::ProviderConfig;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
#[cfg(target_os = "linux")]
use ariadusage_engine::brokers::secret_store::file::FileBackend;
use ariadusage_engine::brokers::secret_store::memory::MemoryBackend;
#[cfg(target_os = "linux")]
use ariadusage_engine::brokers::secret_store::{BackendFuture, SecretBackend};
use ariadusage_engine::brokers::secret_store::{
    SecretId, SecretKind, SecretLookup, SecretStore, SecretStoreError,
};
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::ids::SettingId;
use ariadusage_protocol::secret::SecretString;
use tokio_util::sync::CancellationToken;

fn call(interaction: FetchInteraction) -> BrokerCall {
    BrokerCall {
        interaction,
        cancel: CancellationToken::new(),
        request_id: "secret-test".to_owned(),
    }
}

fn secret_id(value: &str) -> SecretId {
    let setting = SettingId::new(value).expect("valid setting id");
    SecretId::from_setting_id(&setting).expect("known secret setting id")
}

#[cfg(target_os = "linux")]
struct SetFailureBackend(SecretStoreError);

#[cfg(target_os = "linux")]
impl SecretBackend for SetFailureBackend {
    fn lookup_background<'a>(
        &'a self,
        _id: &'a SecretId,
        _call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<SecretLookup, SecretStoreError>> {
        Box::pin(async { Ok(SecretLookup::Missing) })
    }

    fn lookup_user<'a>(
        &'a self,
        _id: &'a SecretId,
        _call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<SecretLookup, SecretStoreError>> {
        Box::pin(async { Ok(SecretLookup::Missing) })
    }

    fn set_user<'a>(
        &'a self,
        _id: &'a SecretId,
        _secret: &'a SecretString,
        _call: &'a BrokerCall,
    ) -> BackendFuture<'a, Result<(), SecretStoreError>> {
        let error = self.0;
        Box::pin(async move { Err(error) })
    }
}

#[tokio::test]
async fn background_lookup_keeps_locked_secret_present_without_unlocking() {
    let id = secret_id("providers.claude.apiKey");
    let memory = MemoryBackend::new();
    memory.insert(id.clone(), SecretString::new("invented-key"), true);
    let store = SecretStore::new(memory.clone());
    let background = store.background(call(FetchInteraction::Background));

    assert_eq!(background.lookup(&id).await, SecretLookup::Locked);
    let presence = store
        .presence(
            &Config::new(Config::CURRENT_VERSION, Vec::new()),
            call(FetchInteraction::Background),
        )
        .await;
    assert!(presence.has_api_key(&ProviderId::new("claude").unwrap()));
    assert_eq!(memory.unlock_attempts(), 0);
}

#[tokio::test]
async fn user_handle_can_unlock_a_locked_secret() {
    let id = secret_id("providers.claude.apiKey");
    let memory = MemoryBackend::new();
    memory.insert(id.clone(), SecretString::new("invented-key"), true);
    let store = SecretStore::new(memory.clone());
    let user = store.user(call(FetchInteraction::UserInitiated));

    assert!(matches!(user.lookup(&id).await, SecretLookup::Found(_)));
    assert_eq!(memory.unlock_attempts(), 1);
}

#[tokio::test]
async fn a_user_handle_with_background_interaction_cannot_unlock_or_write() {
    let id = secret_id("providers.claude.apiKey");
    let memory = MemoryBackend::new();
    memory.insert(id.clone(), SecretString::new("invented-key"), true);
    let store = SecretStore::new(memory.clone());
    let user = store.user(call(FetchInteraction::Background));

    assert_eq!(user.lookup(&id).await, SecretLookup::Locked);
    assert_eq!(
        user.set(&id, &SecretString::new("replacement"))
            .await
            .unwrap_err(),
        SecretStoreError::Locked
    );
    assert_eq!(memory.unlock_attempts(), 0);
}

#[tokio::test]
async fn empty_secret_is_invalid_and_unavailable_backend_stays_unavailable() {
    let id = secret_id("providers.claude.apiKey");
    let memory = MemoryBackend::new();
    memory.insert(id.clone(), SecretString::new(""), false);
    let store = SecretStore::new(memory.clone());
    assert_eq!(
        store
            .background(call(FetchInteraction::Background))
            .lookup(&id)
            .await,
        SecretLookup::Invalid
    );
    assert!(matches!(
        store
            .user(call(FetchInteraction::UserInitiated))
            .set(&id, &SecretString::new(""))
            .await,
        Err(ariadusage_engine::brokers::secret_store::SecretStoreError::Invalid)
    ));

    memory.set_available(false);
    assert_eq!(
        store
            .background(call(FetchInteraction::Background))
            .lookup(&id)
            .await,
        SecretLookup::Unavailable
    );
}

#[test]
fn identifier_grammar_accepts_only_supported_first_party_secret_shapes() {
    for id in [
        "providers.claude.apiKey",
        "providers.codex.cookieHeader",
        "providers.antigravity.accounts.account_1-token.token",
    ] {
        let parsed = secret_id(id);
        assert!(matches!(
            parsed.kind(),
            SecretKind::ApiKey | SecretKind::CookieHeader | SecretKind::AccountToken
        ));
    }

    for id in [
        "providers.unknown.apiKey",
        "providers.claude.secretKey",
        "providers.claude.apiKey.extra",
        "providers.claude.accounts.account.id.token",
    ] {
        let setting = SettingId::new(id).expect("setting id grammar accepts this string");
        assert!(SecretId::from_setting_id(&setting).is_err(), "{id}");
    }
}

#[test]
fn attributes_and_labels_contain_no_account_name_email_or_secret() {
    let id = secret_id("providers.claude.accounts.acc-1.token");
    let attrs = id.attributes();
    assert_eq!(attrs.len(), 5);
    assert_eq!(attrs["application"], "ariadusage");
    assert_eq!(attrs["ariadusage:kind"], "token");
    assert_eq!(attrs["ariadusage:provider"], "claude");
    assert_eq!(attrs["ariadusage:account"], "acc-1");
    assert_eq!(attrs["xdg:schema"], "io.github.bavanchun.ariadusage.Secret");
    assert_eq!(id.keychain_service(), "io.github.bavanchun.ariadusage");
    assert_eq!(id.keychain_account(), "claude:token:acc-1");
    let api_key_id = secret_id("providers.claude.apiKey");
    assert_eq!(
        api_key_id.keychain_service(),
        "io.github.bavanchun.ariadusage"
    );
    assert_eq!(api_key_id.keychain_account(), "claude:apiKey");
    let label = id.label();
    assert_eq!(label, "AriadUsage Claude account token");
    assert!(!label.contains("acc-1"));
    assert!(!label.contains("fixture@example.invalid"));
    assert!(!format!("{id:?}").contains("acc-1"));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn file_fallback_is_consulted_only_after_unavailable_and_recorded_consent() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("ariadusage");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    let path = parent.join("secrets.json");
    let id = secret_id("providers.claude.apiKey");
    let file = FileBackend::new(path, true);
    let backend_call = call(FetchInteraction::UserInitiated);
    file.set_user(&id, &SecretString::new("file-secret"), &backend_call)
        .await
        .unwrap();

    let memory = MemoryBackend::new();
    memory.insert(id.clone(), SecretString::new("keyring-secret"), true);
    let store = SecretStore::new(memory.clone())
        .with_file_fallback(FileBackend::new(parent.join("secrets.json"), true), true);
    assert_eq!(
        store
            .background(call(FetchInteraction::Background))
            .lookup(&id)
            .await,
        SecretLookup::Locked
    );

    memory.set_available(false);
    let store = SecretStore::new(memory)
        .with_file_fallback(FileBackend::new(parent.join("secrets.json"), true), true);
    assert!(matches!(
        store
            .background(call(FetchInteraction::Background))
            .lookup(&id)
            .await,
        SecretLookup::Found(_)
    ));
    assert!(matches!(
        store
            .user(call(FetchInteraction::UserInitiated))
            .lookup(&id)
            .await,
        SecretLookup::Found(_)
    ));

    let mut config = Config::new(Config::CURRENT_VERSION, Vec::new());
    let mut provider = ProviderConfig::new(ProviderId::new("claude").unwrap());
    provider.enabled = Some(true);
    config.set_provider_config(provider);

    let unavailable = MemoryBackend::new();
    unavailable.set_available(false);
    let unconsented = SecretStore::new(unavailable)
        .with_file_fallback(FileBackend::new(parent.join("secrets.json"), true), true);
    assert!(
        unconsented
            .presence(&config, call(FetchInteraction::Background))
            .await
            .api_keys
            .is_empty()
    );

    config.secret_file_fallback = Some(true);
    let unavailable = MemoryBackend::new();
    unavailable.set_available(false);
    let consented = SecretStore::new(unavailable)
        .with_file_fallback(FileBackend::new(parent.join("secrets.json"), true), true);
    assert!(
        consented
            .presence(&config, call(FetchInteraction::Background))
            .await
            .has_api_key(&ProviderId::new("claude").unwrap())
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn file_fallback_writes_only_after_an_unavailable_error() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("ariadusage");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    let path = parent.join("secrets.json");
    let id = secret_id("providers.claude.apiKey");
    FileBackend::new(path.clone(), true)
        .set_user(
            &id,
            &SecretString::new("existing-file-value"),
            &call(FetchInteraction::UserInitiated),
        )
        .await
        .unwrap();

    for error in [
        SecretStoreError::Locked,
        SecretStoreError::Dismissed,
        SecretStoreError::Timeout,
    ] {
        let store = SecretStore::new(SetFailureBackend(error))
            .with_file_fallback(FileBackend::new(path.clone(), true), true);
        assert_eq!(
            store
                .user(call(FetchInteraction::UserInitiated))
                .set(&id, &SecretString::new("replacement-value"))
                .await
                .unwrap_err(),
            error
        );
        assert!(matches!(
            FileBackend::new(path.clone(), true)
                .lookup_background(&id, &call(FetchInteraction::Background))
                .await
                .unwrap(),
            SecretLookup::Found(found) if found.expose_secret() == "existing-file-value"
        ));
    }

    let store = SecretStore::new(SetFailureBackend(SecretStoreError::Unavailable))
        .with_file_fallback(FileBackend::new(path.clone(), true), true);
    store
        .user(call(FetchInteraction::UserInitiated))
        .set(&id, &SecretString::new("replacement-value"))
        .await
        .unwrap();
    assert!(matches!(
        FileBackend::new(path, true)
            .lookup_background(&id, &call(FetchInteraction::Background))
            .await
            .unwrap(),
        SecretLookup::Found(found) if found.expose_secret() == "replacement-value"
    ));
}

#[tokio::test]
async fn debug_output_redacts_secret_material() {
    let secret = SecretString::new("invented-secret-value");
    let lookup = SecretLookup::Found(secret);
    assert_eq!(format!("{lookup:?}"), "Found([redacted])");
}

#[cfg(target_os = "macos")]
mod macos_tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    use ariadusage_core::config::{Config, ProviderConfig};
    use ariadusage_core::pipeline::FetchInteraction;
    use ariadusage_engine::brokers::secret_store::safe_storage::{
        SafeStorageError, SafeStorageKeySource,
    };
    use ariadusage_engine::brokers::secret_store::service::{
        MacosKeychainOperations, ServiceBackend, os_status, read_keychain_item_attributes,
    };
    use ariadusage_engine::brokers::secret_store::{SecretLookup, SecretStore, SecretStoreError};
    use ariadusage_protocol::ProviderId;
    use ariadusage_protocol::secret::SecretString;
    use security_framework::os::macos::keychain::{
        CreateOptions, KeychainSettings, KeychainUserInteractionLock, SecKeychain,
    };
    use security_framework::os::macos::passwords::find_generic_password;
    use zeroize::Zeroizing;

    use super::{call, secret_id};

    fn ensure_no_keychain_ui() {
        static NO_UI_GUARD: OnceLock<Mutex<KeychainUserInteractionLock>> = OnceLock::new();
        let _ = NO_UI_GUARD.get_or_init(|| {
            Mutex::new(
                SecKeychain::disable_user_interaction()
                    .expect("disable macOS Keychain user interaction for tests"),
            )
        });
    }

    struct TempKeychain {
        _dir: tempfile::TempDir,
        keychain: SecKeychain,
    }

    impl TempKeychain {
        fn create() -> Self {
            ensure_no_keychain_ui();
            let dir = tempfile::tempdir().expect("create temp keychain dir");
            let path = dir.path().join("ariadusage-test.keychain-db");
            let mut keychain = CreateOptions::new()
                .password("invented-test-passphrase")
                .create(&path)
                .expect("create isolated temporary keychain");
            let mut settings = KeychainSettings::new();
            settings.set_lock_on_sleep(false);
            let _ = keychain.set_settings(&settings);
            Self {
                _dir: dir,
                keychain,
            }
        }

        fn handle(&self) -> SecKeychain {
            self.keychain.clone()
        }
    }

    type KeychainItemMap = BTreeMap<(String, String), (Vec<u8>, String)>;

    #[derive(Default)]
    struct FakeKeychainOps {
        items: Mutex<KeychainItemMap>,
        next_get_error: Mutex<Option<i32>>,
        next_set_error: Mutex<Option<i32>>,
        interactive_get_calls: AtomicUsize,
        non_interactive_get_calls: AtomicUsize,
        set_calls: AtomicUsize,
    }

    impl MacosKeychainOperations for FakeKeychainOps {
        fn read_generic_password(
            &self,
            service: &str,
            account: &str,
            allow_interaction: bool,
        ) -> Result<Zeroizing<Vec<u8>>, i32> {
            if allow_interaction {
                self.interactive_get_calls.fetch_add(1, Ordering::SeqCst);
            } else {
                self.non_interactive_get_calls
                    .fetch_add(1, Ordering::SeqCst);
            }
            if let Some(code) = *self.next_get_error.lock().unwrap() {
                return Err(code);
            }
            let items = self.items.lock().unwrap();
            items
                .get(&(service.to_owned(), account.to_owned()))
                .map(|(bytes, _)| Zeroizing::new(bytes.clone()))
                .ok_or(os_status::ERR_SEC_ITEM_NOT_FOUND)
        }

        fn write_generic_password(
            &self,
            service: &str,
            account: &str,
            label: &str,
            secret: &[u8],
        ) -> Result<(), i32> {
            self.set_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(code) = *self.next_set_error.lock().unwrap() {
                return Err(code);
            }
            self.items.lock().unwrap().insert(
                (service.to_owned(), account.to_owned()),
                (secret.to_vec(), label.to_owned()),
            );
            Ok(())
        }
    }

    #[tokio::test]
    async fn macos_service_backend_background_and_user_policies_with_injected_operations() {
        let ops = Arc::new(FakeKeychainOps::default());
        let backend = ServiceBackend::with_operations(ops.clone());
        let store = SecretStore::new(backend);
        let id = secret_id("providers.claude.apiKey");

        // Missing item in background does not prompt (`allow_interaction == false`).
        assert_eq!(
            store
                .background(call(FetchInteraction::Background))
                .lookup(&id)
                .await,
            SecretLookup::Missing
        );
        assert_eq!(ops.non_interactive_get_calls.load(Ordering::SeqCst), 1);
        assert_eq!(ops.interactive_get_calls.load(Ordering::SeqCst), 0);

        // Locked item in background maps `ERR_SEC_INTERACTION_NOT_ALLOWED` to `SecretLookup::Locked`
        // and still counts as present in `SecretStore::presence`.
        *ops.next_get_error.lock().unwrap() = Some(os_status::ERR_SEC_INTERACTION_NOT_ALLOWED);
        assert_eq!(
            store
                .background(call(FetchInteraction::Background))
                .lookup(&id)
                .await,
            SecretLookup::Locked
        );
        let mut config = Config::new(Config::CURRENT_VERSION, Vec::new());
        let mut provider = ProviderConfig::new(ProviderId::new("claude").unwrap());
        provider.enabled = Some(true);
        config.set_provider_config(provider);
        assert!(
            store
                .presence(&config, call(FetchInteraction::Background))
                .await
                .has_api_key(&ProviderId::new("claude").unwrap())
        );

        // User-initiated lookup with dismissed prompt maps to `SecretLookup::Unavailable`.
        *ops.next_get_error.lock().unwrap() = Some(os_status::ERR_SEC_USER_CANCELED);
        assert_eq!(
            store
                .user(call(FetchInteraction::UserInitiated))
                .lookup(&id)
                .await,
            SecretLookup::Unavailable
        );
        assert_eq!(ops.interactive_get_calls.load(Ordering::SeqCst), 1);

        // User-initiated set with dismissed prompt maps to `SecretStoreError::Dismissed`.
        *ops.next_set_error.lock().unwrap() = Some(os_status::ERR_SEC_USER_CANCELED);
        assert_eq!(
            store
                .user(call(FetchInteraction::UserInitiated))
                .set(&id, &SecretString::new("invented-api-key"))
                .await
                .unwrap_err(),
            SecretStoreError::Dismissed
        );

        // Successful user write and roundtrip read.
        *ops.next_set_error.lock().unwrap() = None;
        *ops.next_get_error.lock().unwrap() = None;
        store
            .user(call(FetchInteraction::UserInitiated))
            .set(&id, &SecretString::new("invented-api-key"))
            .await
            .unwrap();
        assert!(matches!(
            store
                .background(call(FetchInteraction::Background))
                .lookup(&id)
                .await,
            SecretLookup::Found(found) if found.expose_secret() == "invented-api-key"
        ));

        // Disabled backend never touches Keychain operations.
        let disabled_ops = Arc::new(FakeKeychainOps::default());
        let disabled_store = SecretStore::new(ServiceBackend::with_operations_disabled(
            disabled_ops.clone(),
            true,
        ));
        assert_eq!(
            disabled_store
                .background(call(FetchInteraction::Background))
                .lookup(&id)
                .await,
            SecretLookup::Unavailable
        );
        assert_eq!(
            disabled_store
                .user(call(FetchInteraction::UserInitiated))
                .set(&id, &SecretString::new("invented-api-key"))
                .await
                .unwrap_err(),
            SecretStoreError::Unavailable
        );
        assert_eq!(
            disabled_ops
                .non_interactive_get_calls
                .load(Ordering::SeqCst),
            0
        );
        assert_eq!(disabled_ops.set_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn macos_safe_storage_maps_keychain_statuses() {
        let ops = Arc::new(FakeKeychainOps::default());
        ops.write_generic_password(
            "Chrome Safe Storage",
            "Chrome",
            "Chrome Safe Storage",
            b"invented-safe-storage-passphrase",
        )
        .unwrap();
        let source = SafeStorageKeySource::new(ServiceBackend::with_operations(ops.clone()));

        let key = source
            .chromium_safe_storage("Google Chrome", &call(FetchInteraction::Background))
            .await
            .unwrap();
        assert_eq!(&*key, b"invented-safe-storage-passphrase");

        assert_eq!(
            source
                .chromium_safe_storage("Brave", &call(FetchInteraction::Background))
                .await
                .unwrap_err(),
            SafeStorageError::KWalletUnsupported
        );

        *ops.next_get_error.lock().unwrap() = Some(os_status::ERR_SEC_INTERACTION_NOT_ALLOWED);
        assert_eq!(
            source
                .chromium_safe_storage("Google Chrome", &call(FetchInteraction::Background))
                .await
                .unwrap_err(),
            SafeStorageError::Locked
        );

        *ops.next_get_error.lock().unwrap() = Some(os_status::ERR_SEC_USER_CANCELED);
        assert_eq!(
            source
                .chromium_safe_storage("Google Chrome", &call(FetchInteraction::UserInitiated))
                .await
                .unwrap_err(),
            SafeStorageError::Dismissed
        );
    }

    #[tokio::test]
    async fn macos_temporary_keychain_roundtrip_and_attribute_isolation() {
        let temp_keychain = TempKeychain::create();
        let keychain = temp_keychain.handle();
        let store = SecretStore::new(ServiceBackend::with_keychain(keychain.clone()));

        let api_id = secret_id("providers.claude.apiKey");
        let token_id = secret_id("providers.claude.accounts.acc-1.token");

        assert_eq!(
            store
                .background(call(FetchInteraction::Background))
                .lookup(&api_id)
                .await,
            SecretLookup::Missing
        );

        store
            .user(call(FetchInteraction::UserInitiated))
            .set(&api_id, &SecretString::new("invented-claude-key-v1"))
            .await
            .unwrap();
        store
            .user(call(FetchInteraction::UserInitiated))
            .set(&token_id, &SecretString::new("invented-account-token-v1"))
            .await
            .unwrap();

        assert!(matches!(
            store
                .background(call(FetchInteraction::Background))
                .lookup(&api_id)
                .await,
            SecretLookup::Found(found) if found.expose_secret() == "invented-claude-key-v1"
        ));

        // Update existing secret in place.
        store
            .user(call(FetchInteraction::UserInitiated))
            .set(&api_id, &SecretString::new("invented-claude-key-v2"))
            .await
            .unwrap();
        assert!(matches!(
            store
                .user(call(FetchInteraction::UserInitiated))
                .lookup(&api_id)
                .await,
            SecretLookup::Found(found) if found.expose_secret() == "invented-claude-key-v2"
        ));

        // Verify stored Keychain attributes contain no account name, email, or secret material.
        let api_attrs = read_keychain_item_attributes(
            &keychain,
            api_id.keychain_service(),
            &api_id.keychain_account(),
        )
        .unwrap();
        assert_eq!(
            api_attrs.get("svce").map(String::as_str),
            Some("io.github.bavanchun.ariadusage")
        );
        assert_eq!(
            api_attrs.get("acct").map(String::as_str),
            Some("claude:apiKey")
        );
        assert_eq!(
            api_attrs.get("labl").map(String::as_str),
            Some("AriadUsage Claude API key")
        );

        let token_attrs = read_keychain_item_attributes(
            &keychain,
            token_id.keychain_service(),
            &token_id.keychain_account(),
        )
        .unwrap();
        assert_eq!(
            token_attrs.get("acct").map(String::as_str),
            Some("claude:token:acc-1")
        );
        let token_label = token_attrs.get("labl").expect("label present");
        assert_eq!(token_label, "AriadUsage Claude account token");
        assert!(!token_label.contains("acc-1"));
        assert!(!token_label.contains("invented-account-token-v1"));

        // Verify Chromium Safe Storage lookup against the isolated temporary keychain.
        let _ = find_generic_password(
            Some(std::slice::from_ref(&keychain)),
            "Chrome Safe Storage",
            "Chrome",
        )
        .unwrap_err();
        keychain
            .set_generic_password(
                "Chrome Safe Storage",
                "Chrome",
                b"invented-chrome-safe-storage-secret",
            )
            .unwrap();
        let safe_source =
            SafeStorageKeySource::new(ServiceBackend::with_keychain(keychain.clone()));
        let safe_key = safe_source
            .chromium_safe_storage("Google Chrome", &call(FetchInteraction::Background))
            .await
            .unwrap();
        assert_eq!(&*safe_key, b"invented-chrome-safe-storage-secret");

        // Pre-cancelled calls short-circuit before touching Keychain operations.
        let cancelled = call(FetchInteraction::UserInitiated);
        cancelled.cancel.cancel();
        assert_eq!(
            store
                .user(cancelled)
                .set(&api_id, &SecretString::new("invented-claude-key-v3"))
                .await
                .unwrap_err(),
            SecretStoreError::Cancelled
        );
    }
}
