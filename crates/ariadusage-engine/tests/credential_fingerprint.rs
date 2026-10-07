// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthCredentials.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/CodexManagedAccounts.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::path::PathBuf;

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::credential_file::{
    CredentialDecl, LastSeen, Quarantine, StatFingerprint, content_fingerprint, read,
};
use tokio_util::sync::CancellationToken;

fn make_call() -> BrokerCall {
    BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "test-req".to_string(),
    }
}

#[test]
fn test_content_fingerprint_sha256_hex() {
    let bytes = b"hello ariadusage";
    let fp = content_fingerprint(bytes);
    // sha256("hello ariadusage") = 25b13e1104620077ec23fbc062828b4382c40c885da33575971a8e1045e2213e
    assert_eq!(fp.len(), 64);
    assert_eq!(
        fp,
        "5483620297ccb7a99fc0a68bceca5ce0e1599388afe4cb75fda99aa732026739"
    );
}

#[test]
fn test_stat_fingerprint_changes_on_atomic_same_size_replacement() {
    let call = make_call();
    let temp = tempfile::tempdir().expect("tempdir");
    let target_path = temp.path().join("credentials.json");

    // Initial write
    std::fs::write(&target_path, b"initial payload!").expect("write initial");
    let decl = CredentialDecl::for_test(&target_path);
    let fp1 = read(&decl, &call).expect("read 1").stat;

    // Atomic replacement with the same size
    let stage_path = temp.path().join("staging.json");
    std::fs::write(&stage_path, b"changed payload!").expect("write staging");
    std::fs::rename(&stage_path, &target_path).expect("atomic rename");

    let fp2 = read(&decl, &call).expect("read 2").stat;

    assert_eq!(fp1.size, fp2.size, "file sizes are identical");
    assert_ne!(
        fp1.ino, fp2.ino,
        "inode numbers must differ on atomic replacement"
    );
    assert_ne!(
        fp1, fp2,
        "StatFingerprint must differ on atomic replacement even with identical size"
    );
}

#[test]
fn test_last_seen_change_detection() {
    let mut last_seen = LastSeen::new();
    let profile = "profile-digest-12345";

    let fp1 = StatFingerprint {
        path: PathBuf::from("/fakehome/creds.json"),
        dev: 1,
        ino: 100,
        mtime_ns: 1_000_000,
        size: 256,
    };
    let fp2 = StatFingerprint {
        path: PathBuf::from("/fakehome/creds.json"),
        dev: 1,
        ino: 101, // New inode
        mtime_ns: 1_000_000,
        size: 256,
    };

    // First time seen: has_changed is true
    assert!(last_seen.has_changed(profile, &fp1));

    // Record fp1
    last_seen.record(profile, fp1.clone());
    assert!(!last_seen.has_changed(profile, &fp1));

    // Now check fp2: has_changed is true
    assert!(last_seen.has_changed(profile, &fp2));

    // Record fp2
    last_seen.record(profile, fp2.clone());
    assert!(!last_seen.has_changed(profile, &fp2));
}

#[test]
fn test_quarantine_holds_until_fingerprint_changes() {
    let mut quarantine = Quarantine::new();
    let profile = "profile-digest-bad-file";

    let bad_fp = StatFingerprint {
        path: PathBuf::from("/fakehome/bad.json"),
        dev: 1,
        ino: 500,
        mtime_ns: 2_000_000,
        size: 512,
    };

    let fixed_fp = StatFingerprint {
        path: PathBuf::from("/fakehome/bad.json"),
        dev: 1,
        ino: 501, // User edited or replaced the file
        mtime_ns: 3_000_000,
        size: 600,
    };

    // Initially not quarantined
    assert!(!quarantine.is_quarantined(profile, &bad_fp));

    // Place under quarantine
    quarantine.quarantine(profile, bad_fp.clone());

    // As long as current == stored, quarantine holds
    assert!(quarantine.is_quarantined(profile, &bad_fp));
    assert!(quarantine.is_quarantined(profile, &bad_fp));

    // When fingerprint changes, quarantine auto-clears and returns false
    assert!(!quarantine.is_quarantined(profile, &fixed_fp));

    // Even if checked with old bad_fp now, quarantine was cleared
    assert!(!quarantine.is_quarantined(profile, &bad_fp));
}

#[test]
fn test_stat_fingerprint_debug_redaction() {
    let path = PathBuf::from("/home/secretuser/.claude/credentials.json");
    let fp = StatFingerprint {
        path,
        dev: 42,
        ino: 999,
        mtime_ns: 123456789,
        size: 100,
    };

    let debug_repr = format!("{:?}", fp);
    assert!(
        !debug_repr.contains("secretuser"),
        "Debug representation must not contain user path text: {}",
        debug_repr
    );
    assert!(debug_repr.contains("path_len:"));
}
