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
