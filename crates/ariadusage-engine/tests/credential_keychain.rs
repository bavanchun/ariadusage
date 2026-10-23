// Ported from CodexBar Tests/CodexBarTests/ClaudeOAuthKeychainAccessGateTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeOAuthCredentialsStorePromptPolicyTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/KeychainNoUIQueryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/KeychainStringStoreTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::credential_file::{
    CLAUDE_CODE_KEYCHAIN_SERVICE, CredentialFileError, KEYCHAIN_DENIAL_COOLDOWN_SECS,
    KeychainAccessGate, KeychainCandidateRecord, KeychainCredentialDecl, KeychainCredentialError,
    KeychainCredentialRead, KeychainCredentialReader, KeychainCredentialSource,
    KeychainItemFingerprint, KeychainPromptMode, sha256_prefix, sort_keychain_candidates,
};
use ariadusage_engine::brokers::secret_store::service::os_status;
use ariadusage_engine::state_store::BrokerStateStore;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

fn call(interaction: FetchInteraction) -> BrokerCall {
    BrokerCall {
        interaction,
        cancel: CancellationToken::new(),
        request_id: "keychain-cred-test".to_owned(),
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
        let fingerprint =
            self.fingerprint
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(KeychainItemFingerprint {
                    modified_at: Some(1),
                    created_at: Some(1),
                    persistent_ref_hash: Some(sha256_prefix(&bytes)),
                });
        Ok(KeychainCredentialRead { bytes, fingerprint })
    }

    fn probe_fingerprint(
        &self,
        decl: &KeychainCredentialDecl,
    ) -> Result<Option<KeychainItemFingerprint>, i32> {
        assert_eq!(decl.service(), CLAUDE_CODE_KEYCHAIN_SERVICE);
        self.fingerprint_probes.fetch_add(1, Ordering::SeqCst);
        Ok(self.fingerprint.lock().unwrap().clone())
    }
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
    let reader = Arc::new(FakeKeychainCredentialReader::default());
    *reader.background_error.lock().unwrap() = Some(os_status::ERR_SEC_INTERACTION_NOT_ALLOWED);
    *reader.payload.lock().unwrap() = Some(b"{\"claudeAiOauth\":{}}".to_vec());

    let gate = Arc::new(KeychainAccessGate::new(false));
    let decl = KeychainCredentialDecl::claude_code();
    let source = KeychainCredentialSource::new(gate.clone())
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
    let temp = tempfile::tempdir().unwrap();
    let state_store = Arc::new(BrokerStateStore::new(temp.path().join("state")));
    let reader = Arc::new(FakeKeychainCredentialReader::default());
    *reader.interactive_error.lock().unwrap() = Some(os_status::ERR_SEC_USER_CANCELED);
    *reader.background_error.lock().unwrap() = Some(os_status::ERR_SEC_INTERACTION_NOT_ALLOWED);

    let gate = Arc::new(KeychainAccessGate::new(false).with_state_store(state_store));
    let decl = KeychainCredentialDecl::claude_code();
    let source = KeychainCredentialSource::new(gate.clone())
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
    let source = KeychainCredentialSource::new(gate)
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
    let reader = Arc::new(FakeKeychainCredentialReader::default());
    *reader.payload.lock().unwrap() = Some(vec![b'x'; (1024 * 1024) + 1]);

    let gate = Arc::new(KeychainAccessGate::new(false));
    let decl = KeychainCredentialDecl::claude_code();
    let source = KeychainCredentialSource::new(gate)
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
    use std::sync::{Arc, Mutex, OnceLock};

    use ariadusage_core::pipeline::FetchInteraction;
    use ariadusage_engine::brokers::credential_file::{
        CLAUDE_CODE_KEYCHAIN_SERVICE, KeychainAccessGate, KeychainCredentialDecl,
        KeychainCredentialError, KeychainCredentialSource, KeychainPromptMode,
    };
    use security_framework::os::macos::keychain::{
        CreateOptions, KeychainSettings, KeychainUserInteractionLock, SecKeychain,
    };

    use super::call;

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
        let temp_keychain = TempKeychain::create();
        let keychain = temp_keychain.handle();
        let decl = KeychainCredentialDecl::claude_code();
        let gate = Arc::new(KeychainAccessGate::new(false));
        let source = KeychainCredentialSource::new(gate)
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
    }
}
