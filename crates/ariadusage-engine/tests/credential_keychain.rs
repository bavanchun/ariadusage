// Ported from CodexBar Tests/CodexBarTests/ClaudeOAuthKeychainAccessGateTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeOAuthCredentialsStorePromptPolicyTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/KeychainNoUIQueryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/KeychainStringStoreTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::credential_file::{
    CLAUDE_CODE_KEYCHAIN_SERVICE, CLAUDE_OAUTH_DIRECT_KEYCHAIN_READ_CONSENT_KEY,
    CredentialFileError, DirectKeychainReadConsent, KEYCHAIN_DENIAL_COOLDOWN_SECS,
    KeychainAccessGate, KeychainCandidateRecord, KeychainCredentialDecl, KeychainCredentialError,
    KeychainCredentialRead, KeychainCredentialReader, KeychainCredentialSource,
    KeychainItemFingerprint, KeychainPromptMode, sha256_prefix, sort_keychain_candidates,
};
use ariadusage_engine::brokers::secret_store::service::os_status;
use ariadusage_engine::state_store::BrokerStateStore;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

static KEYCHAIN_TEST_SERIAL: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

fn serial_guard() -> tokio::sync::MutexGuard<'static, ()> {
    loop {
        if let Ok(guard) = KEYCHAIN_TEST_SERIAL.try_lock() {
            return guard;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn call(interaction: FetchInteraction) -> BrokerCall {
    BrokerCall {
        interaction,
        cancel: CancellationToken::new(),
        request_id: "keychain-cred-test".to_owned(),
    }
}

fn default_fake_fingerprint(decl: &KeychainCredentialDecl) -> KeychainItemFingerprint {
    let ref_identity = format!(
        "genp:{}:{}:1:1:fake",
        decl.service(),
        decl.account().unwrap_or_default()
    );
    KeychainItemFingerprint {
        modified_at: Some(1),
        created_at: Some(1),
        persistent_ref_hash: Some(sha256_prefix(ref_identity.as_bytes())),
    }
}

#[derive(Default)]
struct FakeKeychainCredentialReader {
    payload: Mutex<Option<Vec<u8>>>,
    fingerprint: Mutex<Option<KeychainItemFingerprint>>,
    background_error: Mutex<Option<i32>>,
    interactive_error: Mutex<Option<i32>>,
    non_interactive_reads: AtomicUsize,
    interactive_reads: AtomicUsize,
    fingerprint_probes: AtomicUsize,
}

impl KeychainCredentialReader for FakeKeychainCredentialReader {
    fn read_item(
        &self,
        decl: &KeychainCredentialDecl,
        allow_prompt: bool,
    ) -> Result<KeychainCredentialRead, i32> {
        assert_eq!(decl.service(), CLAUDE_CODE_KEYCHAIN_SERVICE);
        if allow_prompt {
            self.interactive_reads.fetch_add(1, Ordering::SeqCst);
            if let Some(code) = *self.interactive_error.lock().unwrap() {
                return Err(code);
            }
        } else {
            self.non_interactive_reads.fetch_add(1, Ordering::SeqCst);
            if let Some(code) = *self.background_error.lock().unwrap() {
                return Err(code);
            }
        }
        let bytes = self
            .payload
            .lock()
            .unwrap()
            .as_ref()
            .map(|b| Zeroizing::new(b.clone()))
            .ok_or(os_status::ERR_SEC_ITEM_NOT_FOUND)?;
        let fingerprint = self
            .fingerprint
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| default_fake_fingerprint(decl));
        Ok(KeychainCredentialRead { bytes, fingerprint })
    }

    fn probe_fingerprint(
        &self,
        decl: &KeychainCredentialDecl,
    ) -> Result<Option<KeychainItemFingerprint>, i32> {
        assert_eq!(decl.service(), CLAUDE_CODE_KEYCHAIN_SERVICE);
        self.fingerprint_probes.fetch_add(1, Ordering::SeqCst);
        if let Some(explicit) = self.fingerprint.lock().unwrap().clone() {
            return Ok(Some(explicit));
        }
        if self.payload.lock().unwrap().is_some() {
            return Ok(Some(default_fake_fingerprint(decl)));
        }
        Ok(None)
    }
}

#[test]
fn unconsented_user_initiated_read_returns_without_calling_reader() {
    let _serial = serial_guard();
    assert_eq!(
        CLAUDE_OAUTH_DIRECT_KEYCHAIN_READ_CONSENT_KEY,
        "claudeOAuthDirectKeychainReadAllowed"
    );
    assert_eq!(
        DirectKeychainReadConsent::default(),
        DirectKeychainReadConsent::Denied
    );
    assert!(!DirectKeychainReadConsent::from(false).is_allowed());
    assert!(DirectKeychainReadConsent::from(true).is_allowed());

    let reader = Arc::new(FakeKeychainCredentialReader::default());
    *reader.payload.lock().unwrap() =
        Some(b"{\"claudeAiOauth\":{\"accessToken\":\"invented\"}}".to_vec());

    let gate = Arc::new(KeychainAccessGate::new(false));
    let decl = KeychainCredentialDecl::claude_code();
    // Default source starts with `DirectKeychainReadConsent::Denied`.
    let mut source = KeychainCredentialSource::new(gate.clone())
        .with_prompt_mode(KeychainPromptMode::OnlyOnUserAction)
        .with_reader(reader.clone());
    assert_eq!(source.consent(), DirectKeychainReadConsent::Denied);

    let err = source
        .read_at(&decl, &call(FetchInteraction::UserInitiated), 1_000)
        .unwrap_err();
    assert_eq!(err, KeychainCredentialError::ConsentRequired);
    assert!(matches!(
        CredentialFileError::from(err),
        CredentialFileError::Unreadable
    ));
    assert_eq!(
        source
            .probe_fingerprint(&decl, &call(FetchInteraction::Background))
            .unwrap(),
        None
    );
    assert_eq!(reader.interactive_reads.load(Ordering::SeqCst), 0);
    assert_eq!(reader.non_interactive_reads.load(Ordering::SeqCst), 0);
    assert_eq!(reader.fingerprint_probes.load(Ordering::SeqCst), 0);
    assert_eq!(gate.prompt_attempt_generation(), 0);
    assert!(gate.should_allow_prompt(CLAUDE_CODE_KEYCHAIN_SERVICE, 1_000));

    // Once the user explicitly grants consent, user-initiated reads can proceed.
    source.set_consent(DirectKeychainReadConsent::Granted);
    let read = source
        .read_at(&decl, &call(FetchInteraction::UserInitiated), 1_001)
        .unwrap();
    assert_eq!(
        &*read.bytes,
        b"{\"claudeAiOauth\":{\"accessToken\":\"invented\"}}"
    );
    assert_eq!(reader.interactive_reads.load(Ordering::SeqCst), 1);
}

#[test]
fn read_and_probe_fingerprints_agree_and_never_hash_secret_bytes() {
    let _serial = serial_guard();
    let reader = Arc::new(FakeKeychainCredentialReader::default());
    let secret_payload = b"{\"claudeAiOauth\":{\"accessToken\":\"secret-token-1\"}}".to_vec();
    *reader.payload.lock().unwrap() = Some(secret_payload.clone());

    let gate = Arc::new(KeychainAccessGate::new(false));
    let decl = KeychainCredentialDecl::claude_code();
    let source =
        KeychainCredentialSource::new_with_consent(gate, DirectKeychainReadConsent::Granted)
            .with_prompt_mode(KeychainPromptMode::OnlyOnUserAction)
            .with_reader(reader.clone());

    let probed = source
        .probe_fingerprint(&decl, &call(FetchInteraction::Background))
        .unwrap()
        .expect("probed fingerprint present");
    let read = source
        .read_at(&decl, &call(FetchInteraction::Background), 1_000)
        .unwrap();

    assert_eq!(read.fingerprint, probed);
    assert_ne!(
        read.fingerprint.persistent_ref_hash.as_deref(),
        Some(sha256_prefix(&secret_payload).as_str())
    );
}

#[test]
fn gate_completed_prompt_attempt_advances_generation() {
    let gate = KeychainAccessGate::new(false);
    let generation = gate.prompt_attempt_generation();
    assert_eq!(gate.record_prompt_attempt_completed(), generation + 1);
    assert_eq!(gate.prompt_attempt_generation(), generation + 1);
    assert!(gate.should_allow_prompt(CLAUDE_CODE_KEYCHAIN_SERVICE, 1_000));
}

#[test]
fn gate_blocks_until_six_hour_cooldown_expires_and_persists_state() {
    let temp = tempfile::tempdir().unwrap();
    let state_store = Arc::new(BrokerStateStore::new(temp.path().join("state")));
    let gate = KeychainAccessGate::new(false).with_state_store(state_store.clone());

    let now = 1_000;
    assert!(gate.should_allow_prompt(CLAUDE_CODE_KEYCHAIN_SERVICE, now));

    gate.record_denied(CLAUDE_CODE_KEYCHAIN_SERVICE, now);
    assert!(!gate.should_allow_prompt(CLAUDE_CODE_KEYCHAIN_SERVICE, now));
    assert!(!gate.should_allow_prompt(
        CLAUDE_CODE_KEYCHAIN_SERVICE,
        now + KEYCHAIN_DENIAL_COOLDOWN_SECS - 1
    ));
    assert!(gate.should_allow_prompt(
        CLAUDE_CODE_KEYCHAIN_SERVICE,
        now + KEYCHAIN_DENIAL_COOLDOWN_SECS
    ));

    // When `BrokerStateStore` persistence is supported on the current platform, reloading in a
    // fresh gate backed by the same `BrokerStateStore` preserves the denial window.
    gate.record_denied(CLAUDE_CODE_KEYCHAIN_SERVICE, now);
    if state_store.load().is_ok() {
        let reloaded = KeychainAccessGate::new(false).with_state_store(state_store);
        assert!(!reloaded.should_allow_prompt(
            CLAUDE_CODE_KEYCHAIN_SERVICE,
            now + KEYCHAIN_DENIAL_COOLDOWN_SECS - 1
        ));
        assert!(reloaded.should_allow_prompt(
            CLAUDE_CODE_KEYCHAIN_SERVICE,
            now + KEYCHAIN_DENIAL_COOLDOWN_SECS
        ));
    }
}

#[test]
fn gate_respects_disabled_flag_and_clear_denied_allows_immediate_retry() {
    let temp = tempfile::tempdir().unwrap();
    let state_store = Arc::new(BrokerStateStore::new(temp.path().join("state")));
    let gate = KeychainAccessGate::new(false).with_state_store(state_store);

    gate.set_disabled(true);
    assert!(!gate.should_allow_prompt(CLAUDE_CODE_KEYCHAIN_SERVICE, 1_000));
    gate.set_disabled(false);
    assert!(gate.should_allow_prompt(CLAUDE_CODE_KEYCHAIN_SERVICE, 1_000));

    let now = 3_000;
    gate.record_denied(CLAUDE_CODE_KEYCHAIN_SERVICE, now);
    assert!(!gate.should_allow_prompt(CLAUDE_CODE_KEYCHAIN_SERVICE, now));

    assert!(gate.clear_denied(CLAUDE_CODE_KEYCHAIN_SERVICE, now));
    assert!(gate.should_allow_prompt(CLAUDE_CODE_KEYCHAIN_SERVICE, now));
    assert!(!gate.clear_denied(CLAUDE_CODE_KEYCHAIN_SERVICE, now));
}

#[test]
fn background_reads_never_prompt_or_poison_denial_cooldown() {
    let _serial = serial_guard();
    let reader = Arc::new(FakeKeychainCredentialReader::default());
    *reader.background_error.lock().unwrap() = Some(os_status::ERR_SEC_INTERACTION_NOT_ALLOWED);
    *reader.payload.lock().unwrap() = Some(b"{\"claudeAiOauth\":{}}".to_vec());

    let gate = Arc::new(KeychainAccessGate::new(false));
    let decl = KeychainCredentialDecl::claude_code();
    let source = KeychainCredentialSource::new_with_consent(
        gate.clone(),
        DirectKeychainReadConsent::Granted,
    )
    .with_prompt_mode(KeychainPromptMode::OnlyOnUserAction)
    .with_reader(reader.clone());

    // Background call uses non-interactive query (`allow_prompt == false`) and returns
    // `InteractionRequired` when the item requires user interaction, without recording a denial.
    let err = source
        .read_at(&decl, &call(FetchInteraction::Background), 1_000)
        .unwrap_err();
    assert_eq!(err, KeychainCredentialError::InteractionRequired);
    assert_eq!(reader.non_interactive_reads.load(Ordering::SeqCst), 1);
    assert_eq!(reader.interactive_reads.load(Ordering::SeqCst), 0);
    assert!(gate.should_allow_prompt(CLAUDE_CODE_KEYCHAIN_SERVICE, 1_000));

    // Subsequent user-initiated call is allowed to prompt and succeeds.
    let read = source
        .read_at(&decl, &call(FetchInteraction::UserInitiated), 1_001)
        .unwrap();
    assert_eq!(&*read.bytes, b"{\"claudeAiOauth\":{}}");
    assert_eq!(reader.interactive_reads.load(Ordering::SeqCst), 1);
    assert!(!format!("{read:?}").contains("claudeAiOauth"));
}

#[test]
fn user_initiated_denial_records_six_hour_cooldown_and_blocks_repeat_prompts() {
    let _serial = serial_guard();
    let temp = tempfile::tempdir().unwrap();
    let state_store = Arc::new(BrokerStateStore::new(temp.path().join("state")));
    let reader = Arc::new(FakeKeychainCredentialReader::default());
    *reader.interactive_error.lock().unwrap() = Some(os_status::ERR_SEC_USER_CANCELED);
    *reader.background_error.lock().unwrap() = Some(os_status::ERR_SEC_INTERACTION_NOT_ALLOWED);

    let gate = Arc::new(KeychainAccessGate::new(false).with_state_store(state_store));
    let decl = KeychainCredentialDecl::claude_code();
    let source = KeychainCredentialSource::new_with_consent(
        gate.clone(),
        DirectKeychainReadConsent::Granted,
    )
    .with_prompt_mode(KeychainPromptMode::OnlyOnUserAction)
    .with_reader(reader.clone());

    let now = 5_000;
    assert_eq!(
        source
            .read_at(&decl, &call(FetchInteraction::UserInitiated), now)
            .unwrap_err(),
        KeychainCredentialError::Dismissed
    );
    assert_eq!(reader.interactive_reads.load(Ordering::SeqCst), 1);
    assert!(!gate.should_allow_prompt(CLAUDE_CODE_KEYCHAIN_SERVICE, now + 60));

    // While in the 6-hour denial cooldown, even a user-initiated call is rejected with
    // `CooldownActive` without triggering another OS prompt.
    assert_eq!(
        source
            .read_at(&decl, &call(FetchInteraction::UserInitiated), now + 60)
            .unwrap_err(),
        KeychainCredentialError::CooldownActive
    );
    assert_eq!(reader.interactive_reads.load(Ordering::SeqCst), 1);
    assert_eq!(reader.non_interactive_reads.load(Ordering::SeqCst), 0);

    // Once the 6-hour cooldown expires, user-initiated calls can prompt again.
    *reader.interactive_error.lock().unwrap() = None;
    *reader.payload.lock().unwrap() =
        Some(b"{\"claudeAiOauth\":{\"accessToken\":\"invented\"}}".to_vec());
    let read = source
        .read_at(
            &decl,
            &call(FetchInteraction::UserInitiated),
            now + KEYCHAIN_DENIAL_COOLDOWN_SECS,
        )
        .unwrap();
    assert_eq!(
        &*read.bytes,
        b"{\"claudeAiOauth\":{\"accessToken\":\"invented\"}}"
    );
    assert_eq!(reader.interactive_reads.load(Ordering::SeqCst), 2);
}

#[test]
fn prompt_mode_never_disables_prompts_even_on_user_initiated_reads() {
    let reader = Arc::new(FakeKeychainCredentialReader::default());
    let gate = Arc::new(KeychainAccessGate::new(false));
    let decl = KeychainCredentialDecl::claude_code();
    let source =
        KeychainCredentialSource::new_with_consent(gate, DirectKeychainReadConsent::Granted)
            .with_prompt_mode(KeychainPromptMode::Never)
            .with_reader(reader.clone());

    assert_eq!(
        source
            .read_at(&decl, &call(FetchInteraction::UserInitiated), 1_000)
            .unwrap_err(),
        KeychainCredentialError::InteractionRequired
    );
    assert_eq!(
        source
            .probe_fingerprint(&decl, &call(FetchInteraction::Background))
            .unwrap(),
        None
    );
    assert_eq!(reader.interactive_reads.load(Ordering::SeqCst), 0);
    assert_eq!(reader.non_interactive_reads.load(Ordering::SeqCst), 0);
}

#[test]
fn keychain_credential_source_enforces_size_limit_and_converts_errors_to_credential_file_error() {
    let _serial = serial_guard();
    let reader = Arc::new(FakeKeychainCredentialReader::default());
    *reader.payload.lock().unwrap() = Some(vec![b'x'; (1024 * 1024) + 1]);

    let gate = Arc::new(KeychainAccessGate::new(false));
    let decl = KeychainCredentialDecl::claude_code();
    let source =
        KeychainCredentialSource::new_with_consent(gate, DirectKeychainReadConsent::Granted)
            .with_prompt_mode(KeychainPromptMode::OnlyOnUserAction)
            .with_reader(reader);

    let err = source
        .read_at(&decl, &call(FetchInteraction::Background), 1_000)
        .unwrap_err();
    assert_eq!(err, KeychainCredentialError::TooLarge);
    let converted: CredentialFileError = err.into();
    assert!(matches!(converted, CredentialFileError::TooLarge));
    assert!(matches!(
        CredentialFileError::from(KeychainCredentialError::NotFound),
        CredentialFileError::NotFound
    ));
}

#[tokio::test]
async fn async_keychain_read_and_probe_run_in_spawn_blocking_and_observe_cancellation() {
    let _serial = KEYCHAIN_TEST_SERIAL.lock().await;
    let reader = Arc::new(FakeKeychainCredentialReader::default());
    *reader.payload.lock().unwrap() =
        Some(b"{\"claudeAiOauth\":{\"accessToken\":\"invented\"}}".to_vec());

    let gate = Arc::new(KeychainAccessGate::new(false));
    let decl = KeychainCredentialDecl::claude_code();
    let source =
        KeychainCredentialSource::new_with_consent(gate, DirectKeychainReadConsent::Granted)
            .with_prompt_mode(KeychainPromptMode::OnlyOnUserAction)
            .with_reader(reader.clone());

    let probed = source
        .probe_fingerprint_async(&decl, &call(FetchInteraction::Background))
        .await
        .unwrap()
        .expect("fingerprint present");
    let read = source
        .read_at_async(&decl, &call(FetchInteraction::Background), 1_000)
        .await
        .unwrap();
    assert_eq!(read.fingerprint, probed);

    let cancelled_call = call(FetchInteraction::UserInitiated);
    cancelled_call.cancel.cancel();
    assert_eq!(
        source
            .read_at_async(&decl, &cancelled_call, 1_001)
            .await
            .unwrap_err(),
        KeychainCredentialError::Cancelled
    );
    assert_eq!(
        source
            .probe_fingerprint_async(&decl, &cancelled_call)
            .await
            .unwrap_err(),
        KeychainCredentialError::Cancelled
    );
}

#[test]
fn candidate_sorting_orders_newest_first_matching_codexbar() {
    let mut rows = vec![
        KeychainCandidateRecord {
            account: "older".to_owned(),
            modified_at: Some(100),
            created_at: Some(90),
            persistent_ref_hash: Some("ref-a".to_owned()),
            data: Zeroizing::new(b"a".to_vec()),
        },
        KeychainCandidateRecord {
            account: String::new(),
            modified_at: None,
            created_at: None,
            persistent_ref_hash: None,
            data: Zeroizing::new(b"none".to_vec()),
        },
        KeychainCandidateRecord {
            account: "newest-modified".to_owned(),
            modified_at: Some(200),
            created_at: Some(50),
            persistent_ref_hash: Some("ref-b".to_owned()),
            data: Zeroizing::new(b"b".to_vec()),
        },
        KeychainCandidateRecord {
            account: "tie-modified-fallback-created".to_owned(),
            modified_at: None,
            created_at: Some(150),
            persistent_ref_hash: Some("ref-c".to_owned()),
            data: Zeroizing::new(b"c".to_vec()),
        },
    ];

    sort_keychain_candidates(&mut rows);
    assert_eq!(rows[0].account, "newest-modified");
    assert_eq!(rows[1].account, "tie-modified-fallback-created");
    assert_eq!(rows[2].account, "older");
    assert_eq!(rows[3].account, "");

    assert_eq!(sha256_prefix(b"persistent-ref-bytes").len(), 12);
}

#[cfg(target_os = "macos")]
mod macos_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    use ariadusage_core::pipeline::FetchInteraction;
    use ariadusage_engine::brokers::credential_file::{
        CLAUDE_CODE_KEYCHAIN_SERVICE, DirectKeychainReadConsent, KeychainAccessGate,
        KeychainCredentialDecl, KeychainCredentialError, KeychainCredentialRead,
        KeychainCredentialReader, KeychainCredentialSource, KeychainItemFingerprint,
        KeychainPromptMode, sha256_prefix,
    };
    use ariadusage_engine::brokers::secret_store::service::{
        is_macos_user_prompt_active, macos_background_no_ui_skipped_during_prompt_count,
    };
    use security_framework::os::macos::keychain::{
        CreateOptions, KeychainSettings, KeychainUserInteractionLock, SecKeychain,
    };
    use zeroize::Zeroizing;

    use super::{KEYCHAIN_TEST_SERIAL, call, default_fake_fingerprint, serial_guard};

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
            let path = dir.path().join("claude-oauth-test.keychain-db");
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

    #[test]
    fn macos_temporary_keychain_reads_claude_code_credentials_and_fingerprint_without_prompting() {
        let _serial = serial_guard();
        let temp_keychain = TempKeychain::create();
        let keychain = temp_keychain.handle();
        let decl = KeychainCredentialDecl::claude_code();
        let gate = Arc::new(KeychainAccessGate::new(false));
        let source =
            KeychainCredentialSource::new_with_consent(gate, DirectKeychainReadConsent::Granted)
                .with_prompt_mode(KeychainPromptMode::OnlyOnUserAction)
                .with_keychain(keychain.clone());

        assert_eq!(
            source
                .read_at(&decl, &call(FetchInteraction::Background), 1_000)
                .unwrap_err(),
            KeychainCredentialError::NotFound
        );
        assert_eq!(
            source
                .probe_fingerprint(&decl, &call(FetchInteraction::Background))
                .unwrap(),
            None
        );

        let payload = br#"{"claudeAiOauth":{"accessToken":"invented-oauth-token","expiresAt":1893456000000,"scopes":["user:profile"]}}"#;
        keychain
            .set_generic_password(CLAUDE_CODE_KEYCHAIN_SERVICE, "claude-cli", payload)
            .unwrap();

        let fingerprint = source
            .probe_fingerprint(&decl, &call(FetchInteraction::Background))
            .unwrap()
            .expect("fingerprint present");
        assert!(
            fingerprint.modified_at.is_some()
                || fingerprint.created_at.is_some()
                || fingerprint.persistent_ref_hash.is_some()
        );

        let read = source
            .read_at(&decl, &call(FetchInteraction::Background), 1_001)
            .unwrap();
        assert_eq!(&*read.bytes, payload);
        assert!(read.fingerprint.persistent_ref_hash.is_some());
        assert_eq!(read.fingerprint, fingerprint);
        assert_ne!(
            read.fingerprint.persistent_ref_hash.as_deref(),
            Some(sha256_prefix(payload).as_str())
        );
    }

    struct InFlightPromptCredentialReader {
        prompt_started: std::sync::mpsc::SyncSender<()>,
        release_prompt: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
        interactive_reads: AtomicUsize,
        non_interactive_reads: AtomicUsize,
        fingerprint_probes: AtomicUsize,
    }

    impl KeychainCredentialReader for InFlightPromptCredentialReader {
        fn read_item(
            &self,
            decl: &KeychainCredentialDecl,
            allow_prompt: bool,
        ) -> Result<KeychainCredentialRead, i32> {
            if allow_prompt {
                self.interactive_reads.fetch_add(1, Ordering::SeqCst);
                assert!(is_macos_user_prompt_active());
                let _ = self.prompt_started.send(());
                let rx = self
                    .release_prompt
                    .lock()
                    .unwrap()
                    .take()
                    .expect("single interactive prompt receiver");
                rx.recv().expect("wait for prompt release");
                Ok(KeychainCredentialRead {
                    bytes: Zeroizing::new(
                        b"{\"claudeAiOauth\":{\"accessToken\":\"invented-prompt\"}}".to_vec(),
                    ),
                    fingerprint: default_fake_fingerprint(decl),
                })
            } else {
                self.non_interactive_reads.fetch_add(1, Ordering::SeqCst);
                assert!(!is_macos_user_prompt_active());
                Ok(KeychainCredentialRead {
                    bytes: Zeroizing::new(
                        b"{\"claudeAiOauth\":{\"accessToken\":\"invented-bg\"}}".to_vec(),
                    ),
                    fingerprint: default_fake_fingerprint(decl),
                })
            }
        }

        fn probe_fingerprint(
            &self,
            decl: &KeychainCredentialDecl,
        ) -> Result<Option<KeychainItemFingerprint>, i32> {
            self.fingerprint_probes.fetch_add(1, Ordering::SeqCst);
            assert!(!is_macos_user_prompt_active());
            Ok(Some(default_fake_fingerprint(decl)))
        }
    }

    #[tokio::test]
    async fn macos_background_read_while_user_prompt_in_flight_uses_no_ui_and_skips_reader() {
        let _serial = KEYCHAIN_TEST_SERIAL.lock().await;
        ensure_no_keychain_ui();
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);

        let reader = Arc::new(InFlightPromptCredentialReader {
            prompt_started: started_tx,
            release_prompt: Mutex::new(Some(release_rx)),
            interactive_reads: AtomicUsize::new(0),
            non_interactive_reads: AtomicUsize::new(0),
            fingerprint_probes: AtomicUsize::new(0),
        });
        let gate = Arc::new(KeychainAccessGate::new(false));
        let decl = KeychainCredentialDecl::claude_code();
        let source = KeychainCredentialSource::new_with_consent(
            gate.clone(),
            DirectKeychainReadConsent::Granted,
        )
        .with_prompt_mode(KeychainPromptMode::OnlyOnUserAction)
        .with_reader(reader.clone());

        let source_for_user = source.clone();
        let decl_for_user = decl.clone();
        let user_task = tokio::spawn(async move {
            source_for_user
                .read_at_async(
                    &decl_for_user,
                    &call(FetchInteraction::UserInitiated),
                    1_000,
                )
                .await
        });

        tokio::task::spawn_blocking(move || {
            started_rx.recv().expect("user prompt should start");
        })
        .await
        .unwrap();

        assert!(is_macos_user_prompt_active());
        assert_eq!(reader.interactive_reads.load(Ordering::SeqCst), 1);
        assert_eq!(reader.non_interactive_reads.load(Ordering::SeqCst), 0);

        let skipped_before = macos_background_no_ui_skipped_during_prompt_count();

        // While the simulated user prompt holds `USER_PROMPT_LOCK`, background `read_at_async` and
        // `probe_fingerprint_async` take the no-UI path, immediately return transient `Busy`
        // without calling the reader or poisoning the 6-hour denial cooldown.
        assert_eq!(
            source
                .read_at_async(&decl, &call(FetchInteraction::Background), 1_001)
                .await
                .unwrap_err(),
            KeychainCredentialError::Busy
        );
        assert_eq!(
            source
                .probe_fingerprint_async(&decl, &call(FetchInteraction::Background))
                .await
                .unwrap_err(),
            KeychainCredentialError::Busy
        );
        assert_eq!(
            macos_background_no_ui_skipped_during_prompt_count(),
            skipped_before + 2
        );
        assert_eq!(reader.interactive_reads.load(Ordering::SeqCst), 1);
        assert_eq!(reader.non_interactive_reads.load(Ordering::SeqCst), 0);
        assert_eq!(reader.fingerprint_probes.load(Ordering::SeqCst), 0);
        assert!(gate.should_allow_prompt(CLAUDE_CODE_KEYCHAIN_SERVICE, 1_001));

        release_tx.send(()).unwrap();
        let user_read = user_task.await.unwrap().unwrap();
        assert_eq!(
            &*user_read.bytes,
            b"{\"claudeAiOauth\":{\"accessToken\":\"invented-prompt\"}}"
        );
        assert!(!is_macos_user_prompt_active());

        // After the user prompt completes, background read and probe run on the no-UI path and
        // invoke the reader with `allow_prompt == false`.
        let bg_read = source
            .read_at_async(&decl, &call(FetchInteraction::Background), 1_002)
            .await
            .unwrap();
        assert_eq!(
            &*bg_read.bytes,
            b"{\"claudeAiOauth\":{\"accessToken\":\"invented-bg\"}}"
        );
        assert_eq!(reader.non_interactive_reads.load(Ordering::SeqCst), 1);
        assert!(
            source
                .probe_fingerprint_async(&decl, &call(FetchInteraction::Background))
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(reader.fingerprint_probes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn macos_unfiltered_interaction_required_probe_and_cache_work_when_no_prompt_is_active() {
        let _serial = serial_guard();
        let temp_keychain = TempKeychain::create();
        let keychain = temp_keychain.handle();
        let decl = KeychainCredentialDecl::claude_code();
        let gate = Arc::new(KeychainAccessGate::new(false));
        let source =
            KeychainCredentialSource::new_with_consent(gate, DirectKeychainReadConsent::Granted)
                .with_prompt_mode(KeychainPromptMode::OnlyOnUserAction)
                .with_keychain_simulating_authenticated_skip(keychain.clone());

        // Before the item exists, unfiltered attribute probe also sees `ERR_SEC_ITEM_NOT_FOUND`.
        assert_eq!(
            source
                .read_at(&decl, &call(FetchInteraction::Background), 1_000)
                .unwrap_err(),
            KeychainCredentialError::NotFound
        );
        assert_eq!(
            source
                .probe_fingerprint(&decl, &call(FetchInteraction::Background))
                .unwrap(),
            None
        );

        // Once the item exists in the isolated temporary keychain, `skip_authenticated_items(true)`
        // simulates skipping it, `is_macos_user_prompt_active()` is `false` inside the background
        // helper, and `probe_unfiltered_interaction_required` finds the item's attributes without
        // UI, caches the key in `interaction_required_cache`, and reports `InteractionRequired`.
        keychain
            .set_generic_password(
                CLAUDE_CODE_KEYCHAIN_SERVICE,
                "claude-cli",
                br#"{"claudeAiOauth":{"accessToken":"invented"}}"#,
            )
            .unwrap();

        assert_eq!(
            source
                .read_at(&decl, &call(FetchInteraction::Background), 1_001)
                .unwrap_err(),
            KeychainCredentialError::InteractionRequired
        );
        assert_eq!(
            source
                .probe_fingerprint(&decl, &call(FetchInteraction::Background))
                .unwrap_err(),
            KeychainCredentialError::InteractionRequired
        );
    }
}
