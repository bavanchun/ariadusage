// Ported from CodexBar Tests/CodexBarTests/ProviderSessionStoreFileTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ariadusage_engine::paths::{
    PathError, resolve_data_dir, resolve_runtime_dir, resolve_state_dir,
};

fn make_env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
    let map: HashMap<String, OsString> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), OsString::from(v)))
        .collect();
    move |key: &str| map.get(key).cloned()
}

// CodexBar: Tests/CodexBarTests/ProviderSessionStoreFileTests.swift:8
#[test]
fn test_paths_never_write_real_directories_and_use_injected_xdg() {
    let fake_home = Path::new("/fakehome/user");
    let fake_state = Path::new("/custom/state");
    let fake_data = Path::new("/custom/data");
    let fake_run = Path::new("/custom/run");

    let env = make_env(&[
        ("XDG_STATE_HOME", "/custom/state"),
        ("XDG_DATA_HOME", "/custom/data"),
        ("XDG_RUNTIME_DIR", "/custom/run"),
    ]);

    let state = resolve_state_dir(&env, Some(fake_home)).expect("resolves state dir");
    assert_eq!(state, fake_state);

    let data = resolve_data_dir(&env, Some(fake_home)).expect("resolves data dir");
    assert_eq!(data, fake_data);

    let run = resolve_runtime_dir(&env, Some(fake_home)).expect("resolves runtime dir");
    assert_eq!(run, fake_run);
}

#[test]
fn test_resolve_state_and_data_defaults_when_xdg_unset() {
    let fake_home = Path::new("/fakehome/user");
    let empty_env = make_env(&[]);

    let state = resolve_state_dir(&empty_env, Some(fake_home)).expect("resolves default state");
    assert_eq!(state, PathBuf::from("/fakehome/user/.local/state"));

    let data = resolve_data_dir(&empty_env, Some(fake_home)).expect("resolves default data");
    assert_eq!(data, PathBuf::from("/fakehome/user/.local/share"));

    let run = resolve_runtime_dir(&empty_env, Some(fake_home));
    assert_eq!(
        run, None,
        "runtime dir has no default and must be None when unset"
    );
}

#[test]
fn test_resolve_relative_xdg_paths_ignored_and_fall_back() {
    let fake_home = Path::new("/fakehome/user");
    let rel_env = make_env(&[
        ("XDG_STATE_HOME", "relative/state"),
        ("XDG_DATA_HOME", "relative/data"),
        ("XDG_RUNTIME_DIR", "relative/run"),
    ]);

    let state = resolve_state_dir(&rel_env, Some(fake_home)).expect("resolves fallback state");
    assert_eq!(state, PathBuf::from("/fakehome/user/.local/state"));

    let data = resolve_data_dir(&rel_env, Some(fake_home)).expect("resolves fallback data");
    assert_eq!(data, PathBuf::from("/fakehome/user/.local/share"));

    let run = resolve_runtime_dir(&rel_env, Some(fake_home));
    assert_eq!(
        run, None,
        "relative XDG_RUNTIME_DIR must be ignored as None"
    );
}

#[test]
fn test_resolve_tilde_expansion_for_xdg_dirs() {
    let fake_home = Path::new("/fakehome/user");
    let tilde_env = make_env(&[
        ("XDG_STATE_HOME", "~/state"),
        ("XDG_DATA_HOME", "~/share"),
        ("XDG_RUNTIME_DIR", "~/run"),
    ]);

    let state = resolve_state_dir(&tilde_env, Some(fake_home)).expect("resolves tilde state");
    assert_eq!(state, PathBuf::from("/fakehome/user/state"));

    let data = resolve_data_dir(&tilde_env, Some(fake_home)).expect("resolves tilde data");
    assert_eq!(data, PathBuf::from("/fakehome/user/share"));

    let run = resolve_runtime_dir(&tilde_env, Some(fake_home)).expect("resolves tilde run");
    assert_eq!(run, PathBuf::from("/fakehome/user/run"));
}

#[test]
fn test_resolve_fails_when_home_is_missing_or_relative() {
    let empty_env = make_env(&[]);

    assert_eq!(
        resolve_state_dir(&empty_env, None),
        Err(PathError::NoHomeDirectory)
    );
    assert_eq!(
        resolve_data_dir(&empty_env, None),
        Err(PathError::NoHomeDirectory)
    );

    let rel_home = Path::new("relative/home");
    assert_eq!(
        resolve_state_dir(&empty_env, Some(rel_home)),
        Err(PathError::NonAbsoluteHome(PathBuf::from("relative/home")))
    );
    assert_eq!(
        resolve_data_dir(&empty_env, Some(rel_home)),
        Err(PathError::NonAbsoluteHome(PathBuf::from("relative/home")))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_trusted_runtime_dir_rejections() {
    use ariadusage_engine::paths::resolve_trusted_runtime_dir_with;
    use std::os::unix::fs::PermissionsExt;

    // 1. Unset value -> None
    let empty_env = make_env(&[]);
    assert_eq!(
        resolve_trusted_runtime_dir_with(&empty_env, Some(Path::new("/fakehome/user")), |_| true),
        None,
        "unset XDG_RUNTIME_DIR must reject"
    );

    // Create a temporary directory for permission and statfs tests.
    let temp_parent = tempfile::tempdir().expect("tempdir");
    let test_dir = temp_parent.path().join("run_dir");
    std::fs::create_dir(&test_dir).expect("create test_dir");

    // 2. Mode 0755 -> rejected (requires 0700)
    std::fs::set_permissions(&test_dir, std::fs::Permissions::from_mode(0o755)).expect("set 0755");
    let env_0755 = make_env(&[("XDG_RUNTIME_DIR", test_dir.to_str().unwrap())]);
    assert_eq!(
        resolve_trusted_runtime_dir_with(&env_0755, None, |_| true),
        None,
        "0755 runtime directory must be rejected"
    );

    // 3. Mode 0700 but non-tmpfs (statfs simulated returning false) -> rejected
    std::fs::set_permissions(&test_dir, std::fs::Permissions::from_mode(0o700)).expect("set 0700");
    let env_0700 = make_env(&[("XDG_RUNTIME_DIR", test_dir.to_str().unwrap())]);
    assert_eq!(
        resolve_trusted_runtime_dir_with(&env_0700, None, |_| false),
        None,
        "non-tmpfs directory must be rejected"
    );

    // 4. Mode 0700 + tmpfs -> accepted
    assert_eq!(
        resolve_trusted_runtime_dir_with(&env_0700, None, |_| true),
        Some(test_dir),
        "0700 directory on tmpfs owned by current EUID must be accepted"
    );
}
