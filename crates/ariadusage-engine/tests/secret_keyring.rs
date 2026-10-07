#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::secret_store::service::ServiceBackend;
use ariadusage_engine::brokers::secret_store::{
    SecretBackend, SecretId, SecretLookup, SecretStore, SecretStoreError,
};
use ariadusage_protocol::ids::SettingId;
use ariadusage_protocol::secret::SecretString;
use secret_service::{EncryptionType, SecretService};
use tokio_util::sync::CancellationToken;

const ISOLATED_TEST_SENTINEL: &str = "isolated-keyring-v1";

#[tokio::test]
#[ignore = "requires the isolated session created by just keyring-test"]
async fn isolated_keyring_locked_item_stays_locked_for_background_operations() {
    assert_isolated_environment();

    let service = connect_service().await;
    let collection = match service.get_default_collection().await {
        Ok(collection) => collection,
        Err(secret_service::Error::NoResult) => service
            .create_collection("AriadUsage isolated login", "default")
            .await
            .expect("create isolated default collection"),
        Err(_) => panic!("isolated default collection lookup failed"),
    };

    let setting_id = SettingId::new("providers.claude.apiKey").expect("synthetic setting id");
    let secret_id = SecretId::from_setting_id(&setting_id).expect("supported setting id");
    let attributes = secret_id.attributes();
    let attributes = attributes
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<HashMap<_, _>>();
    let item = collection
        .create_item(
            &secret_id.label(),
            attributes,
            concat!("synthetic-", "isolated-value").as_bytes(),
            true,
            "text/plain",
        )
        .await
        .expect("create synthetic item");
    item.lock().await.expect("lock synthetic item");
    assert!(item.is_locked().await.expect("read lock state"));

    let call = call(FetchInteraction::Background);
    let backend = ServiceBackend::new(false);
    let lookup = tokio::time::timeout(
        Duration::from_secs(5),
        backend.lookup_background(&secret_id, &call),
    )
    .await
    .expect("background search completed within five seconds")
    .expect("background search result");
    assert_eq!(lookup, SecretLookup::Locked);

    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(5),
            backend.lookup_user(&secret_id, &call),
        )
        .await
        .expect("background user lookup completed within five seconds")
        .expect("background user lookup result"),
        SecretLookup::Locked
    );

    let get_result = tokio::time::timeout(Duration::from_secs(5), item.get_secret())
        .await
        .expect("locked get completed within five seconds");
    let get_outcome = match get_result {
        Err(secret_service::Error::Locked) => "locked",
        Err(_) => "other-error",
        Ok(secret) => {
            drop(zeroize::Zeroizing::new(secret));
            "returned-secret"
        }
    };
    assert_ne!(get_outcome, "returned-secret");

    let store = SecretStore::new(ServiceBackend::new(false));
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(5),
            backend.set_user(&secret_id, &SecretString::new("background-write"), &call,),
        )
        .await
        .expect("backend background write rejected within five seconds"),
        Err(SecretStoreError::Locked)
    ));
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(5),
            store
                .user(call.clone())
                .set(&secret_id, &SecretString::new("background-write")),
        )
        .await
        .expect("background write rejected within five seconds"),
        Err(SecretStoreError::Locked)
    ));

    assert_eq!(
        store.user(call).lookup(&secret_id).await,
        SecretLookup::Locked
    );
}

async fn connect_service() -> SecretService<'static> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(Ok(service)) = tokio::time::timeout(
            Duration::from_millis(500),
            SecretService::connect(EncryptionType::Dh),
        )
        .await
        {
            return service;
        }
        assert!(
            Instant::now() < deadline,
            "isolated Secret Service did not become available within five seconds"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn call(interaction: FetchInteraction) -> BrokerCall {
    BrokerCall {
        interaction,
        cancel: CancellationToken::new(),
        request_id: "isolated-keyring-test".to_owned(),
    }
}

fn assert_isolated_environment() {
    assert_eq!(
        std::env::var("ARIADUSAGE_KEYRING_TEST").ok().as_deref(),
        Some(ISOLATED_TEST_SENTINEL),
        "only just keyring-test may enable this test"
    );
    assert_ne!(
        std::env::var("ARIADUSAGE_DISABLE_KEYRING").ok().as_deref(),
        Some("1"),
        "the test requires its synthetic keyring backend"
    );

    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS").expect("isolated D-Bus session");
    assert!(!bus.is_empty(), "D-Bus address must be present");
    let home = std::env::var_os("HOME").expect("temporary HOME");
    let home = Path::new(&home);
    assert!(home.is_absolute(), "temporary HOME must be absolute");
    assert!(
        home.starts_with(std::env::temp_dir()),
        "HOME must be inside the temporary directory"
    );
    let marker = std::fs::read_to_string(home.join(".ariadusage-keyring-test-sentinel"))
        .expect("HOME marker");
    assert_eq!(
        marker.trim_end_matches('\n'),
        ISOLATED_TEST_SENTINEL,
        "HOME must carry the recipe's isolated-session marker"
    );

    for variable in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_RUNTIME_DIR",
    ] {
        let path = std::env::var_os(variable).expect("temporary XDG directory");
        assert!(
            Path::new(&path).starts_with(home),
            "XDG path must be inside HOME"
        );
    }
}
