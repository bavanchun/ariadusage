// Ported from CodexBar Tests/CodexBarTests/ClaudeConfigPathsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::path::Path;

use ariadusage_engine::providers::claude::ClaudeConfigPaths;

fn env_from(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[test]
fn resolves_relative_claude_config_dir_against_working_directory() {
    let working_directory = Path::new("/tmp/workspace/project");
    let home_directory = Path::new("/Users/tester");

    let resolved = ClaudeConfigPaths::config_root(
        &env_from(&[("CLAUDE_CONFIG_DIR", "../.claude-work")]),
        working_directory,
        home_directory,
    );

    assert_eq!(resolved, Path::new("/tmp/workspace/.claude-work"));
}

#[test]
fn resolves_tilde_prefixed_claude_config_dir_as_literal_relative_path() {
    let working_directory = Path::new("/tmp/workspace/project");
    let home_directory = Path::new("/Users/tester");

    let resolved = ClaudeConfigPaths::config_root(
        &env_from(&[("CLAUDE_CONFIG_DIR", "~/.claude-work")]),
        working_directory,
        home_directory,
    );

    assert_eq!(resolved, Path::new("/tmp/workspace/project/~/.claude-work"));
}

#[test]
fn uses_legacy_home_account_config_when_default_config_file_does_not_exist() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let working = temp.path().join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&working).unwrap();

    let resolved = ClaudeConfigPaths::account_config_path(&BTreeMap::new(), &working, &home);

    assert_eq!(resolved, home.join(".claude.json"));
}

#[test]
fn prefers_config_json_when_present_in_config_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let working = temp.path().join("work");
    let config_root = home.join(".claude");
    let preferred_config = config_root.join(".config.json");
    std::fs::create_dir_all(&config_root).unwrap();
    std::fs::write(&preferred_config, b"{}").unwrap();

    let resolved = ClaudeConfigPaths::account_config_path(&BTreeMap::new(), &working, &home);

    assert_eq!(resolved, preferred_config);
}

#[test]
fn uses_custom_config_root_for_legacy_account_config_fallback() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let working = temp.path().join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&working).unwrap();

    let resolved = ClaudeConfigPaths::account_config_path(
        &env_from(&[("CLAUDE_CONFIG_DIR", "custom-claude")]),
        &working,
        &home,
    );

    assert_eq!(resolved, working.join("custom-claude").join(".claude.json"));
}

#[test]
fn resolves_secure_storage_credentials_path_including_empty_override() {
    let working_directory = Path::new("/tmp/workspace/project");
    let home_directory = Path::new("/Users/tester");

    let custom_secure_storage = ClaudeConfigPaths::credentials_path(
        &env_from(&[
            ("CLAUDE_CONFIG_DIR", "/tmp/ignored-root"),
            ("CLAUDE_SECURESTORAGE_CONFIG_DIR", "secure-store"),
        ]),
        working_directory,
        home_directory,
    );
    assert_eq!(
        custom_secure_storage,
        Path::new("/tmp/workspace/project/secure-store/.credentials.json")
    );

    let empty_secure_storage = ClaudeConfigPaths::credentials_path(
        &env_from(&[
            ("CLAUDE_CONFIG_DIR", "/tmp/ignored-root"),
            ("CLAUDE_SECURESTORAGE_CONFIG_DIR", "   "),
        ]),
        working_directory,
        home_directory,
    );
    assert_eq!(
        empty_secure_storage,
        Path::new("/Users/tester/.claude/.credentials.json")
    );
}

#[test]
fn empty_config_and_secure_roots_follow_relative_home_from_working_directory() {
    let working_directory = Path::new("/tmp/workspace/probe");
    let home_directory = Path::new("/Users/tester");
    let env = env_from(&[
        ("HOME", "relative-home"),
        ("CLAUDE_CONFIG_DIR", ""),
        ("CLAUDE_SECURESTORAGE_CONFIG_DIR", ""),
    ]);

    assert_eq!(
        ClaudeConfigPaths::config_root(&env, working_directory, home_directory),
        Path::new("/tmp/workspace/probe/relative-home/.claude")
    );
    assert_eq!(
        ClaudeConfigPaths::account_config_path(&env, working_directory, home_directory),
        Path::new("/tmp/workspace/probe/relative-home/.claude.json")
    );
    assert_eq!(
        ClaudeConfigPaths::credentials_path(&env, working_directory, home_directory),
        Path::new("/tmp/workspace/probe/relative-home/.claude/.credentials.json")
    );
}
