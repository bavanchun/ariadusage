// Ported from CodexBar Tests/CodexBarTests/CodexOAuthConfigTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_engine::providers::codex::CodexOAuthUsageFetcher;
use std::collections::BTreeMap;
use tempfile::TempDir;

// upstream: CodexOAuthConfigTests.swift:7 ignores commented chat GPT base URL
#[test]
fn ignores_commented_chat_gpt_base_url() {
    for prefix in ["#", "# ", "  # ", "\t# "] {
        let home = TempDir::new().unwrap();
        let env = BTreeMap::from([(
            "CODEX_HOME".into(),
            home.path().to_string_lossy().into_owned(),
        )]);
        let config = format!("{prefix}chatgpt_base_url = \"http://127.0.0.1:8788/backend-api/\"\n");
        assert_eq!(
            CodexOAuthUsageFetcher::resolve_usage_url(&env, Some(&config), None).as_str(),
            "https://chatgpt.com/backend-api/wham/usage"
        );
    }
}

// upstream: CodexOAuthConfigTests.swift:17 uses active chat GPT base URL after commented override
#[test]
fn uses_active_chat_gpt_base_url_after_commented_override() {
    let config = r#"# chatgpt_base_url = "http://127.0.0.1:8788/backend-api/"
chatgpt_base_url = "https://proxy.example/backend-api/" # Active override"#;
    assert_eq!(
        CodexOAuthUsageFetcher::resolve_usage_url(&BTreeMap::new(), Some(config), None).as_str(),
        "https://proxy.example/backend-api/wham/usage"
    );
}

// upstream: CodexOAuthConfigTests.swift:27 loads only active base URL overrides from config file
#[test]
fn loads_only_active_base_url_overrides_from_config_file() {
    for has_override in [false, true] {
        let home = TempDir::new().unwrap();
        let mut config = "# chatgpt_base_url = \"http://127.0.0.1:8788/backend-api/\"\n".to_owned();
        if has_override {
            config.push_str(
                "chatgpt_base_url = 'https://proxy.example/backend-api/' # Active override\n",
            );
        }
        std::fs::write(home.path().join("config.toml"), config).unwrap();
        let env = BTreeMap::from([(
            "CODEX_HOME".into(),
            home.path().to_string_lossy().into_owned(),
        )]);
        let base = if has_override {
            "https://proxy.example"
        } else {
            "https://chatgpt.com"
        };
        assert_eq!(
            CodexOAuthUsageFetcher::resolve_usage_url(&env, None, Some(home.path())).as_str(),
            format!("{base}/backend-api/wham/usage")
        );
    }
}
