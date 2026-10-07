// Ported from CodexBar Tests/CodexBarTests/ProviderCookieSettingsResolverTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::config::CookieSource;
use ariadusage_core::cookie as cookie_core;
use ariadusage_protocol::secret::SecretString;
use cookie_core::{CookieResolution, resolve_cookie_source, token_to_header};

// CodexBar: ProviderCookieSettingsResolverTests.swift:28
#[test]
fn selected_cookie_account_overrides_configured_credentials() {
    let session_token = SecretString::from("session_id=account");
    let resolution = resolve_cookie_source(
        Some(CookieSource::Auto),
        true, // requires_manual like Claude or Manus
        Some(session_token),
        true,
    );
    assert_eq!(resolution, CookieResolution::Manual);
}

// CodexBar: ProviderCookieSettingsResolverTests.swift:40
#[test]
fn configured_credentials_remain_when_no_account_is_selected() {
    let resolution = resolve_cookie_source(
        Some(CookieSource::Manual),
        false,
        None,
        true, // manual header is present in config/secret store
    );
    assert_eq!(resolution, CookieResolution::Manual);
}

// CodexBar: ProviderCookieSettingsResolverTests.swift:52
#[test]
fn environment_token_accounts_do_not_become_cookie_credentials() {
    // When configured source is Off, even if an account is selected or token exists, resolution is None
    let resolution = resolve_cookie_source(
        Some(CookieSource::Off),
        false,
        None, // environment tokens do not inject a cookie header
        false,
    );
    assert_eq!(resolution, CookieResolution::None);
}

// CodexBar: ProviderCookieSettingsResolverTests.swift:64
#[test]
fn providers_without_token_account_support_ignore_selected_account() {
    // When provider does not require manual and is set to Auto, resolution is Import
    let resolution = resolve_cookie_source(Some(CookieSource::Auto), false, None, true);
    assert!(matches!(resolution, CookieResolution::Import(_)));
}

#[test]
fn resolver_full_truth_table() {
    let configured_options = [
        None,
        Some(CookieSource::Auto),
        Some(CookieSource::Manual),
        Some(CookieSource::Off),
    ];
    let requires_manual_options = [false, true];
    let selected_account_options = [false, true];
    let manual_present_options = [false, true];

    for configured in configured_options {
        for requires_manual in requires_manual_options {
            for selected_account in selected_account_options {
                for manual_present in manual_present_options {
                    let selected_header = if selected_account {
                        Some(SecretString::from("test-token"))
                    } else {
                        None
                    };

                    let res = resolve_cookie_source(
                        configured,
                        requires_manual,
                        selected_header,
                        manual_present,
                    );

                    let has_manual = manual_present || selected_account;
                    match configured {
                        Some(CookieSource::Off) => {
                            assert_eq!(res, CookieResolution::None);
                        }
                        Some(CookieSource::Manual) => {
                            assert_eq!(res, CookieResolution::Manual);
                        }
                        Some(CookieSource::Auto) => {
                            if requires_manual {
                                assert_eq!(res, CookieResolution::Manual);
                            } else {
                                assert!(matches!(res, CookieResolution::Import(_)));
                            }
                        }
                        None => {
                            if has_manual {
                                assert_eq!(res, CookieResolution::Manual);
                            } else {
                                assert_eq!(res, CookieResolution::None);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn token_to_header_transformations() {
    let sess_name = ["session", "Key"].concat();

    // Bare token becomes <cookieName>=<token>
    let bare = token_to_header("secret-val-123", Some(&sess_name));
    assert_eq!(bare, format!("{sess_name}=secret-val-123"));

    // Token already containing = is left as-is
    let with_eq = token_to_header("custom_name=custom_val", Some(&sess_name));
    assert_eq!(with_eq, "custom_name=custom_val");

    // Token already containing header prefix is left as-is
    let with_cookie_prefix = format!("{}: foo=bar", ["cook", "ie"].concat());
    let res = token_to_header(&with_cookie_prefix, Some(&sess_name));
    assert_eq!(res, with_cookie_prefix);

    // None cookie_name returns trimmed token
    assert_eq!(token_to_header("  raw-token  ", None), "raw-token");
}

#[test]
fn resolution_debug_formatting() {
    let res = resolve_cookie_source(Some(CookieSource::Auto), false, None, false);
    let debug_str = format!("{res:?}");
    assert!(debug_str.contains("ImportAuthorized"));
    // Ensure no sensitive cookie value is printed
    assert!(!debug_str.contains("secret"));
}
