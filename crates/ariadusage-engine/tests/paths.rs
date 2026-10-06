// Ported from CodexBar Tests/CodexBarTests/ConfigValidationTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ariadusage_engine::paths::{PathError, resolve_config_path};

fn make_env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, OsString> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), OsString::from(v)))
        .collect();
    move |key| map.get(key).cloned()
}

#[test]
fn config_store_default_path_honors_environment_override() {
    let home = Path::new("/tmp/test-home");

    // Tilde override
    let env_tilde = make_env(&[("ARIADUSAGE_CONFIG", "~/custom/config.json")]);
    let path = resolve_config_path(env_tilde, Some(home)).unwrap();
    assert_eq!(path, home.join("custom/config.json"));

    // Absolute override
    let env_abs = make_env(&[("ARIADUSAGE_CONFIG", "/var/lib/ariadusage/config.json")]);
    let path_abs = resolve_config_path(env_abs, Some(home)).unwrap();
    assert_eq!(path_abs, PathBuf::from("/var/lib/ariadusage/config.json"));
}

#[test]
fn config_store_default_path_rejects_relative_override() {
    let home = Path::new("/tmp/test-home");
    let env = make_env(&[("ARIADUSAGE_CONFIG", "relative/path/config.json")]);
    let err = resolve_config_path(env, Some(home)).unwrap_err();
    assert_eq!(
        err,
        PathError::NonAbsoluteOverride(PathBuf::from("relative/path/config.json"))
    );
}

#[test]
fn config_store_default_path_honors_xdg_config_home() {
    let home = Path::new("/tmp/test-home");
    let env = make_env(&[("XDG_CONFIG_HOME", "/custom/xdg")]);
    let path = resolve_config_path(env, Some(home)).unwrap();
    assert_eq!(path, PathBuf::from("/custom/xdg/ariadusage/config.json"));
}

#[test]
fn config_store_default_path_ignores_relative_xdg_config_home() {
    let home = Path::new("/tmp/test-home");
    let env = make_env(&[("XDG_CONFIG_HOME", "relative-xdg")]);
    let path = resolve_config_path(env, Some(home)).unwrap();
    assert_eq!(path, home.join(".config/ariadusage/config.json"));
}

#[test]
fn config_store_default_path_creates_in_xdg_default_for_new_installs() {
    let home = Path::new("/tmp/test-home");
    let env = make_env(&[]);
    let path = resolve_config_path(env, Some(home)).unwrap();
    assert_eq!(path, home.join(".config/ariadusage/config.json"));
}
