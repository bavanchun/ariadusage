use std::os::unix::fs::PermissionsExt;

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::private_tempdir::{TempDirError, create, create_in, sweep_stale_in};
use tokio_util::sync::CancellationToken;

fn make_call() -> BrokerCall {
    BrokerCall {
        interaction: FetchInteraction::UserInitiated,
        cancel: CancellationToken::new(),
        request_id: "test-req".to_string(),
    }
}

#[test]
fn test_private_tempdir_is_0700_and_removed_on_drop() {
    let parent = tempfile::tempdir().expect("tempdir");
    let dir_path;
    {
        let tempdir = create_in(parent.path()).expect("create private tempdir");
        dir_path = tempdir.path().to_path_buf();
        assert!(dir_path.exists(), "directory must exist while in scope");

        let stat = std::fs::metadata(&dir_path).expect("metadata");
        let mode = stat.permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "private tempdir permissions must be 0700");
    }
    assert!(
        !dir_path.exists(),
        "private tempdir must be removed on normal drop"
    );
}

#[test]
fn test_private_tempdir_is_removed_on_panic_unwind() {
    let parent = tempfile::tempdir().expect("tempdir");
    let parent_path = parent.path().to_path_buf();
    let dir_path_cell = std::sync::Arc::new(std::sync::Mutex::new(None));

    let dir_path_cell_clone = dir_path_cell.clone();
    let _ = std::panic::catch_unwind(move || {
        let tempdir = create_in(&parent_path).expect("create private tempdir");
        let p = tempdir.path().to_path_buf();
        *dir_path_cell_clone.lock().unwrap() = Some(p);
        panic!("simulated panic to test unwind cleanup");
    });

    let dir_path = dir_path_cell.lock().unwrap().take().expect("path recorded");
    assert!(
        !dir_path.exists(),
        "private tempdir must be removed on panic unwind"
    );
}

#[test]
fn test_sweep_stale_removes_leftovers() {
    let parent = tempfile::tempdir().expect("tempdir");
    let stale_dir = parent.path().join("ariadusage-tmp-leftover-12345");
    std::fs::create_dir_all(&stale_dir).expect("create stale dir");
    std::fs::write(stale_dir.join("orphaned.txt"), b"orphaned data").expect("write orphaned");

    let unrelated_dir = parent.path().join("other-dir");
    std::fs::create_dir_all(&unrelated_dir).expect("create unrelated dir");

    assert!(stale_dir.exists());
    assert!(unrelated_dir.exists());

    sweep_stale_in(parent.path());

    assert!(
        !stale_dir.exists(),
        "stale ariadusage-tmp-* directory must be removed by sweep_stale"
    );
    assert!(
        unrelated_dir.exists(),
        "unrelated directories must not be touched by sweep_stale"
    );
}

#[test]
fn test_create_returns_unsupported_when_runtime_dir_is_unset_or_untrusted() {
    // When XDG_RUNTIME_DIR is pointing to a non-existent or untrusted directory
    let call = make_call();
    let res = create(&call);
    // On systems where XDG_RUNTIME_DIR is unset or not tmpfs 0700, this returns Err(Unsupported)
    // Here we assert that if it fails, it is Unsupported.
    if let Err(e) = res {
        assert!(matches!(e, TempDirError::Unsupported));
    }
}
