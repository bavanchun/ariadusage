// Ported from CodexBar Tests/CodexBarTests/ClaudeCredentialRoutingTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_engine::providers::claude::{ClaudeCredentialRoute, ClaudeCredentialRouting};
use ariadusage_protocol::secret::SecretString;

fn cookie_header_line(cookies: &str) -> String {
    format!("{}: {cookies}", "Cookie")
}

#[test]
fn resolves_raw_oauth_token() {
    let routing = ClaudeCredentialRouting::resolve(Some("sk-ant-oat-test-token"), None);
    assert_eq!(
        routing,
        ClaudeCredentialRoute::Oauth {
            access_token: SecretString::new("sk-ant-oat-test-token"),
        }
    );
}

#[test]
fn resolves_bearer_oauth_token() {
    let routing = ClaudeCredentialRouting::resolve(Some("Bearer sk-ant-oat-test-token"), None);
    assert_eq!(
        routing,
        ClaudeCredentialRoute::Oauth {
            access_token: SecretString::new("sk-ant-oat-test-token"),
        }
    );
    assert!(ClaudeCredentialRouting::is_oauth_access_token(Some(
        "  bearer sk-ant-oat-test-token  "
    )));
    assert!(!ClaudeCredentialRouting::is_oauth_access_token(Some(
        "sessionKey=sid-1"
    )));
}

#[test]
fn resolves_admin_api_keys_to_admin_api() {
    assert_eq!(
        ClaudeCredentialRouting::resolve(Some("sk-ant-admin01-key"), None),
        ClaudeCredentialRoute::AdminApiKey(SecretString::new("sk-ant-admin01-key"))
    );
    assert_eq!(
        ClaudeCredentialRouting::resolve(Some("Bearer sk-ant-admin-key"), None),
        ClaudeCredentialRoute::AdminApiKey(SecretString::new("sk-ant-admin-key"))
    );
    assert!(ClaudeCredentialRouting::is_admin_api_key(Some(
        "  bearer sk-ant-admin-key  "
    )));
    assert!(!ClaudeCredentialRouting::is_admin_api_key(Some(
        "sk-ant-oat01-token"
    )));
}

#[test]
fn resolves_session_token_to_cookie_header() {
    let routing = ClaudeCredentialRouting::resolve(Some("sid-token-1"), None);
    assert_eq!(
        routing,
        ClaudeCredentialRoute::WebCookie {
            header: SecretString::new("sessionKey=sid-token-1"),
        }
    );
}

#[test]
fn resolves_config_cookie_header_through_shared_normalizer() {
    let raw_header = cookie_header_line("sessionKey=sid-token-1; foo=bar");
    let routing = ClaudeCredentialRouting::resolve(None, Some(&raw_header));
    assert_eq!(
        routing,
        ClaudeCredentialRoute::WebCookie {
            header: SecretString::new("sessionKey=sid-token-1; foo=bar"),
        }
    );
    assert_eq!(
        ClaudeCredentialRouting::resolve(Some("sessionKey=sk-ant-oat-1"), None),
        ClaudeCredentialRoute::WebCookie {
            header: SecretString::new("sessionKey=sk-ant-oat-1"),
        }
    );
}

#[test]
fn token_account_input_wins_over_config_cookie_fallback() {
    let fallback_header = cookie_header_line("sessionKey=sid-token-1");
    let routing = ClaudeCredentialRouting::resolve(
        Some("Bearer sk-ant-oat-test-token"),
        Some(&fallback_header),
    );
    assert_eq!(
        routing,
        ClaudeCredentialRoute::Oauth {
            access_token: SecretString::new("sk-ant-oat-test-token"),
        }
    );
}

#[test]
fn empty_inputs_resolve_to_none_and_whitespace_token_falls_back_to_manual_cookie() {
    let fallback_header = cookie_header_line("sessionKey=sid-manual-1");
    assert_eq!(
        ClaudeCredentialRouting::resolve(Some("   "), Some(&fallback_header)),
        ClaudeCredentialRoute::WebCookie {
            header: SecretString::new("sessionKey=sid-manual-1"),
        }
    );
    assert_eq!(
        ClaudeCredentialRouting::resolve(Some("   "), Some("\n")),
        ClaudeCredentialRoute::None
    );
    assert_eq!(
        ClaudeCredentialRouting::resolve(None, None),
        ClaudeCredentialRoute::None
    );
}
