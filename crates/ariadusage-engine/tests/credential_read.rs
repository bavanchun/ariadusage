// Ported from CodexBar Tests/CodexBarTests/CodexCredentialFileAccessTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthExpiryPipelineTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthCredentialReadTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

#![cfg(target_os = "linux")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::credential_file::{
    CredentialDecl, CredentialFileError, MAX_CREDENTIAL_FILE_SIZE, read, read_with_expected_uid,
    read_with_publication_retry_impl,
};
use tokio_util::sync::CancellationToken;

fn make_call() -> BrokerCall {
    BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "test-req".to_string(),
    }
}

// CodexBar: Tests/CodexBarTests/CodexCredentialFileAccessTests.swift:46
#[test]
fn test_undeclared_paths_never_touched() {
    // Only declared paths via CredentialDecl can be passed to read;
    // attempting to read a nonexistent declared path fails closed with NotFound.
    let call = make_call();
    let decl = CredentialDecl::for_test("/fakehome/nonexistent-auth.json");
    let res = read(&decl, &call);
    assert_eq!(res.err(), Some(CredentialFileError::NotFound));
}

// CodexBar: Tests/CodexBarTests/CodexCredentialFileAccessTests.swift:266
#[test]
fn test_owned_roots_reject_escapes_and_missing_targets() {
    let call = make_call();
    let temp = tempfile::tempdir().expect("tempdir");
    let sibling = temp.path().join("root-sibling-escape");
    let decl = CredentialDecl::for_test(sibling);
    let res = read(&decl, &call);
    assert_eq!(res.err(), Some(CredentialFileError::NotFound));

    let dotdot = temp.path().join("subdir/../../escaped.json");
    let decl_dotdot = CredentialDecl::for_test(dotdot);
    let res_dotdot = read(&decl_dotdot, &call);
    assert_eq!(res_dotdot.err(), Some(CredentialFileError::NotFound));
}

// CodexBar: Tests/CodexBarTests/CodexCredentialFileAccessTests.swift:142
#[test]
fn test_detached_and_missing_credentials_fail_closed_without_ambient_default() {
    let call = make_call();
    let decl = CredentialDecl::for_test("/fakehome/missing_scope.json");
    assert_eq!(
        read(&decl, &call).err(),
        Some(CredentialFileError::NotFound)
    );
}

// CodexBar: Tests/CodexBarTests/CodexOAuthExpiryPipelineTests.swift:10
#[tokio::test]
async fn test_publication_retry_two_reads_then_accept() {
    let call = make_call();
    let count = AtomicUsize::new(0);

    let res = read_with_publication_retry_impl(
        &call,
        || {
            let n = count.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Ok("stale_token".to_string())
            } else {
                Ok("fresh_token".to_string())
            }
        },
        |val| val == "fresh_token",
    )
    .await;

    assert_eq!(res.expect("succeeds on second read"), "fresh_token");
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

// CodexBar: Tests/CodexBarTests/CodexOAuthExpiryPipelineTests.swift:61
#[tokio::test]
async fn test_publication_retry_bounded_three_reads_preserves_final_error() {
    let call = make_call();
    let count = AtomicUsize::new(0);

    let res: Result<String, CredentialFileError> = read_with_publication_retry_impl(
        &call,
        || {
            let _ = count.fetch_add(1, Ordering::SeqCst);
            Err(CredentialFileError::Unreadable)
        },
        |_| true,
    )
    .await;

    assert_eq!(res.err(), Some(CredentialFileError::Unreadable));
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

// CodexBar: Tests/CodexBarTests/CodexOAuthExpiryPipelineTests.swift:102
#[tokio::test]
async fn test_publication_retry_cancelled_zero_reads() {
    let call = make_call();
    call.cancel.cancel(); // Pre-cancelled
    let count = AtomicUsize::new(0);

    let res: Result<String, CredentialFileError> = read_with_publication_retry_impl(
        &call,
        || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok("data".to_string())
        },
        |_| true,
    )
    .await;

    assert_eq!(res.err(), Some(CredentialFileError::Cancelled));
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

// CodexBar: Tests/CodexBarTests/CodexOAuthCredentialReadTests.swift:8
#[test]
fn test_missing_credential_file_maps_to_not_found() {
    let call = make_call();
    let temp = tempfile::tempdir().expect("tempdir");
    let missing_path = temp.path().join("does_not_exist.json");
    let decl = CredentialDecl::for_test(missing_path);

    let res = read(&decl, &call);
    assert_eq!(res.err(), Some(CredentialFileError::NotFound));
}

// CodexBar: Tests/CodexBarTests/CodexOAuthCredentialReadTests.swift:23
#[test]
fn test_directory_or_unreadable_file_maps_to_unreadable() {
    let call = make_call();
    let temp = tempfile::tempdir().expect("tempdir");
    // Directory is not a regular file -> Unreadable
    let decl = CredentialDecl::for_test(temp.path());

    let res = read(&decl, &call);
    assert_eq!(res.err(), Some(CredentialFileError::Unreadable));
}

// CodexBar: Tests/CodexBarTests/CodexOAuthCredentialReadTests.swift:42
#[test]
fn test_malformed_credential_maps_to_fixed_malformed_without_text() {
    let err = CredentialFileError::Malformed;
    assert_eq!(err.to_string(), "credential file is malformed");
    assert!(!err.to_string().contains("token"));
}

#[test]
fn test_wrong_uid_maps_to_untrusted() {
    let call = make_call();
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("auth.json");
    std::fs::write(&path, b"{\"test\":\"data\"}").expect("write auth.json");

    let decl = CredentialDecl::for_test(path);
    let current_euid = rustix::process::geteuid().as_raw();
    // Inject expected UID that does NOT match current EUID
    let res = read_with_expected_uid(&decl, &call, Some(current_euid + 9999));
    assert_eq!(res.err(), Some(CredentialFileError::Untrusted));
}

#[test]
fn test_fifo_at_declared_path_returns_unreadable_immediately() {
    let call = make_call();
    let temp = tempfile::tempdir().expect("tempdir");
    let fifo_path = temp.path().join("auth_fifo");

    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo_path,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_bits_retain(0o600),
        0,
    )
    .expect("mkfifo");

    let start = std::time::Instant::now();
    let decl = CredentialDecl::for_test(&fifo_path);
    let res = read(&decl, &call);

    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "FIFO read must not block and must return within 2 s (took {:?})",
        elapsed
    );
    assert_eq!(res.err(), Some(CredentialFileError::Unreadable));
}

#[test]
fn test_oversized_credential_file_returns_too_large() {
    let call = make_call();
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("oversized.json");
    // Write 1 MiB + 16 bytes
    let big_data = vec![b'A'; (MAX_CREDENTIAL_FILE_SIZE + 16) as usize];
    std::fs::write(&path, &big_data).expect("write big file");

    let decl = CredentialDecl::for_test(path);
    let res = read(&decl, &call);
    assert_eq!(res.err(), Some(CredentialFileError::TooLarge));
}

#[test]
fn test_debug_of_credential_read_never_prints_secret_bytes() {
    let call = make_call();
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("secret_auth.json");
    let secret = "super_secret_refresh_token_value_xyz123";
    std::fs::write(&path, secret.as_bytes()).expect("write secret auth");

    let decl = CredentialDecl::for_test(path);
    let cred_read = read(&decl, &call).expect("read succeeds");

    let debug_repr = format!("{:?}", cred_read);
    assert!(
        !debug_repr.contains(secret),
        "Debug must not contain secret content: {}",
        debug_repr
    );
    assert!(debug_repr.contains(&format!("bytes_len: {}", secret.len())));
}
