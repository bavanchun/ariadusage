// Ported from CodexBar Tests/CodexBarTests/ProviderSessionStoreFileTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

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

#[test]
fn test_macos_runtime_dir_resolution_and_var_canonicalization() {
    use ariadusage_engine::paths::{canonicalize_macos_var_prefix, resolve_macos_runtime_dir_with};

    assert_eq!(
        canonicalize_macos_var_prefix(Path::new("/var/folders/ab/cd/T")),
        PathBuf::from("/private/var/folders/ab/cd/T")
    );
    assert_eq!(
        canonicalize_macos_var_prefix(Path::new("/private/var/folders/ab/cd/T")),
        PathBuf::from("/private/var/folders/ab/cd/T")
    );

    // 1. XDG_RUNTIME_DIR takes precedence over Darwin temp dir when set and absolute.
    let env_xdg = make_env(&[("XDG_RUNTIME_DIR", "/custom/xdg/run")]);
    assert_eq!(
        resolve_macos_runtime_dir_with(&env_xdg, Some(Path::new("/Users/alice")), || {
            Some(PathBuf::from("/var/folders/ab/cd/T/"))
        }),
        Some(PathBuf::from("/custom/xdg/run"))
    );

    // 2. Falls back to Darwin temp dir when XDG_RUNTIME_DIR is unset or relative, stripping trailing slash.
    let empty_env = make_env(&[]);
    assert_eq!(
        resolve_macos_runtime_dir_with(&empty_env, Some(Path::new("/Users/alice")), || {
            Some(PathBuf::from("/var/folders/ab/cd/T/"))
        }),
        Some(PathBuf::from("/var/folders/ab/cd/T"))
    );
    let rel_env = make_env(&[("XDG_RUNTIME_DIR", "rel/run")]);
    assert_eq!(
        resolve_macos_runtime_dir_with(&rel_env, Some(Path::new("/Users/alice")), || {
            Some(PathBuf::from("/var/folders/ab/cd/T"))
        }),
        Some(PathBuf::from("/var/folders/ab/cd/T"))
    );
}

#[cfg(unix)]
#[test]
fn test_macos_trusted_runtime_dir_enforces_mode_uid_and_rejects_symlinks() {
    use ariadusage_engine::paths::{
        canonicalize_macos_var_prefix, resolve_macos_trusted_runtime_dir_with,
    };
    use std::os::unix::fs::PermissionsExt;

    let temp_parent = tempfile::tempdir().expect("tempdir");
    // Canonicalize the temp parent through /private/var on macOS so ancestor lstat checks
    // start from a symlink-free root.
    let canon_parent = canonicalize_macos_var_prefix(temp_parent.path());
    let run_dir = canon_parent.join("darwin_run");
    std::fs::create_dir(&run_dir).expect("create run_dir");
    let euid = rustix::process::geteuid().as_raw();
    let empty_env = make_env(&[]);

    // 1. Mode 0755 -> rejected
    std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o755)).expect("set 0755");
    assert_eq!(
        resolve_macos_trusted_runtime_dir_with(&empty_env, None, || Some(run_dir.clone()), euid),
        None,
        "0755 runtime directory must be rejected"
    );

    // 2. Mode 0700 -> accepted for matching EUID, rejected for mismatched EUID
    std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o700)).expect("set 0700");
    assert_eq!(
        resolve_macos_trusted_runtime_dir_with(
            &empty_env,
            None,
            || Some(run_dir.clone()),
            euid.wrapping_add(1)
        ),
        None,
        "runtime directory owned by different UID must be rejected"
    );
    assert_eq!(
        resolve_macos_trusted_runtime_dir_with(&empty_env, None, || Some(run_dir.clone()), euid),
        Some(run_dir.clone()),
        "0700 directory owned by EUID with no symlink ancestors must be accepted"
    );

    // 3. Leaf symlink escape -> rejected
    let leaf_link = canon_parent.join("symlink_leaf");
    std::os::unix::fs::symlink(&run_dir, &leaf_link).expect("create leaf symlink");
    assert_eq!(
        resolve_macos_trusted_runtime_dir_with(&empty_env, None, || Some(leaf_link.clone()), euid),
        None,
        "symlink leaf runtime directory must be rejected"
    );

    // 4. Intermediate symlink escape -> rejected
    let nested_dir = run_dir.join("nested");
    std::fs::create_dir(&nested_dir).expect("create nested_dir");
    std::fs::set_permissions(&nested_dir, std::fs::Permissions::from_mode(0o700))
        .expect("set nested 0700");
    let via_symlink = leaf_link.join("nested");
    assert_eq!(
        resolve_macos_trusted_runtime_dir_with(&empty_env, None, || Some(via_symlink), euid),
        None,
        "runtime directory traversed through an intermediate symlink must be rejected"
    );
}

#[test]
fn test_socket_path_enforces_sun_path_104_byte_limit() {
    use ariadusage_engine::paths::{
        ENGINE_SOCKET_REL_PATH, PathError, SUN_PATH_MAX_BYTES,
        resolve_socket_path_from_runtime_dir, validate_socket_path_len,
    };

    let short_dir = Path::new("/private/var/folders/ab/cd/T");
    let sock = resolve_socket_path_from_runtime_dir(short_dir).expect("short socket path valid");
    assert_eq!(
        sock,
        PathBuf::from("/private/var/folders/ab/cd/T/ariadusage/engine.sock")
    );

    // Exactly 103 bytes (< 104) is accepted.
    let suffix_len = 1 + ENGINE_SOCKET_REL_PATH.len(); // "/ariadusage/engine.sock" = 23 bytes
    let dir_103 = format!("/{}", "a".repeat(SUN_PATH_MAX_BYTES - 1 - 1 - suffix_len));
    let sock_103 =
        resolve_socket_path_from_runtime_dir(Path::new(&dir_103)).expect("103-byte socket valid");
    assert_eq!(sock_103.as_os_str().len(), 103);

    // 104 bytes (>= SUN_PATH_MAX_BYTES) fails closed with SocketPathTooLong.
    let dir_104 = format!("/{}", "a".repeat(SUN_PATH_MAX_BYTES - 1 - suffix_len));
    let err = resolve_socket_path_from_runtime_dir(Path::new(&dir_104))
        .expect_err("104-byte socket path must fail closed");
    assert!(matches!(
        err,
        PathError::SocketPathTooLong {
            len: 104,
            max: 104,
            ..
        }
    ));

    let over_path = PathBuf::from(format!("/{}", "x".repeat(120)));
    assert!(matches!(
        validate_socket_path_len(&over_path),
        Err(PathError::SocketPathTooLong {
            len: 121,
            max: 104,
            ..
        })
    ));
}
