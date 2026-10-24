// Ported from CodexBar Tests/CodexBarTests/CodexOAuthCredentialReadTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOAuthCredentialsStorePermissionsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#![cfg(unix)]

use ariadusage_engine::providers::codex::*;
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use tempfile::TempDir;
fn base64url_no_pad(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0x3f) as usize] as char);
        }
    }
    out
}

fn make_jwt(claims: &Value) -> String {
    make_jwt_raw(&serde_json::to_vec(claims).unwrap())
}

fn make_jwt_raw(payload_bytes: &[u8]) -> String {
    let header = base64url_no_pad(r#"{"alg":"none","typ":"JWT"}"#.as_bytes());
    let payload = base64url_no_pad(payload_bytes);
    format!("{header}.{payload}.signature")
}

fn parse(value: Value) -> CodexOAuthCredentials {
    CodexOAuthCredentialsStore::parse(
        &serde_json::to_vec(&value).unwrap(),
        CodexOAuthCredentialSource::CodexHome,
    )
    .unwrap()
}
fn native(token: &str, fresh: bool) -> CodexOAuthCredentials {
    parse(
        json!({"tokens":{"access_token":token,"refresh_token":"native-refresh"},"last_refresh":if fresh {Timestamp::now().to_string()} else {"2000-01-01T00:00:00Z".into()}}),
    )
}
fn relative(seconds: i64) -> Timestamp {
    Timestamp::from_second(Timestamp::now().as_second() + seconds).unwrap()
}
fn new_credentials(
    access: &str,
    refresh: &str,
    source: CodexOAuthCredentialSource,
    expiry: Option<Timestamp>,
    last: Option<Timestamp>,
) -> CodexOAuthCredentials {
    CodexOAuthCredentials::new(access, refresh, None, None, last, expiry, source, false)
}
fn env(home: &std::path::Path) -> BTreeMap<String, String> {
    BTreeMap::from([("CODEX_HOME".into(), home.to_string_lossy().into_owned())])
}
fn external(
    home: &std::path::Path,
    access: &str,
    refresh: Option<&str>,
    expires: Option<i64>,
) -> std::path::PathBuf {
    let path = home.join("opencode/auth.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut value = json!({"openai":{"type":"oauth","access":access}});
    if let Some(r) = refresh {
        value["openai"]["refresh"] = json!(r);
    }
    if let Some(e) = expires {
        value["openai"]["expires"] = json!(e);
    }
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    path
}
fn data_env(path: &std::path::Path) -> BTreeMap<String, String> {
    BTreeMap::from([("XDG_DATA_HOME".into(), path.to_string_lossy().into_owned())])
}

// upstream: CodexOAuthCredentialReadTests.swift:7 missing auth json maps to a not found credential error
#[test]
fn missing_auth_json_maps_to_a_not_found_credential_error() {
    let home = TempDir::new().unwrap();
    assert_eq!(
        CodexOAuthCredentialsStore::load_oauth_tokens(&env(home.path()), Some(home.path()))
            .unwrap_err(),
        CodexOAuthCredentialsError::NotFound
    );
}

// upstream: CodexOAuthCredentialReadTests.swift:22 unreadable auth json maps to an unreadable credential error
#[test]
fn unreadable_auth_json_maps_to_an_unreadable_credential_error() {
    let home = TempDir::new().unwrap();
    fs::create_dir(home.path().join("auth.json")).unwrap();
    assert_eq!(
        CodexOAuthCredentialsStore::load_oauth_tokens(&env(home.path()), Some(home.path()))
            .unwrap_err(),
        CodexOAuthCredentialsError::Unreadable
    );
}

// upstream: CodexOAuthCredentialReadTests.swift:41 malformed auth json maps to a safe decode error
#[test]
fn malformed_auth_json_maps_to_a_safe_decode_error() {
    assert_eq!(
        CodexOAuthCredentialsStore::parse(b"not-json", CodexOAuthCredentialSource::CodexHome)
            .unwrap_err(),
        CodexOAuthCredentialsError::DecodeFailed("Invalid JSON".into())
    );
}

// upstream: CodexOAuthCredentialReadTests.swift:53 missing account id falls back to the OpenAI JWT auth claim
#[test]
fn missing_account_id_falls_back_to_the_openai_jwt_auth_claim() {
    let token =
        make_jwt(&json!({"https://api.openai.com/auth":{"chatgpt_account_id":"acct-namespaced"}}));
    let mut value = json!({"tokens":{"access_token":"opaque-access","refresh_token":"refresh"}});
    value["tokens"]["id_token"] = json!(token);
    assert_eq!(parse(value).account_id.as_deref(), Some("acct-namespaced"));
}

// upstream: CodexOAuthCredentialReadTests.swift:71 missing account id falls back to the direct OpenAI JWT claim
#[test]
fn missing_account_id_falls_back_to_the_direct_openai_jwt_claim() {
    let token = make_jwt(&json!({"chatgpt_account_id":"acct-direct"}));
    let mut value = json!({"tokens":{"access_token":"opaque-access","refresh_token":"refresh"}});
    value["tokens"]["id_token"] = json!(token);
    assert_eq!(parse(value).account_id.as_deref(), Some("acct-direct"));
}

// upstream: CodexOAuthCredentialReadTests.swift:89 whitespace account id falls back to the OpenAI JWT claim
#[test]
fn whitespace_account_id_falls_back_to_the_openai_jwt_claim() {
    let token = make_jwt(&json!({"organizations":[{"id":"org-from-jwt"}]}));
    let mut value = json!({"tokens":{"access_token":"opaque-access","refresh_token":"refresh"}});
    value["tokens"]["access_token"] = json!(token);
    value["tokens"]["account_id"] = json!("  \n");
    assert_eq!(parse(value).account_id.as_deref(), Some("org-from-jwt"));
}

// upstream: CodexOAuthCredentialReadTests.swift:107 missing account id falls back to the first OpenAI organization
#[test]
fn missing_account_id_falls_back_to_the_first_openai_organization() {
    let token = make_jwt(&json!({"organizations":[{"id":"org-first"},{"id":"org-second"}]}));
    let mut value = json!({"tokens":{"access_token":"opaque-access","refresh_token":"refresh"}});
    value["tokens"]["access_token"] = json!(token);
    assert_eq!(parse(value).account_id.as_deref(), Some("org-first"));
}

// upstream: CodexOAuthCredentialReadTests.swift:124 missing account id skips blank OpenAI organizations
#[test]
fn missing_account_id_skips_blank_openai_organizations() {
    let token = make_jwt(&json!({"organizations":[{"id":"  "},{"id":"org-later"},{"id":""}]}));
    let mut value = json!({"tokens":{"access_token":"opaque-access","refresh_token":"refresh"}});
    value["tokens"]["access_token"] = json!(token);
    assert_eq!(parse(value).account_id.as_deref(), Some("org-later"));
}

// upstream: CodexOAuthCredentialReadTests.swift:141 open code oauth credentials preserve expiry and remain read only
#[test]
fn open_code_oauth_credentials_preserve_expiry_and_remain_read_only() {
    let expires = relative(3600);
    let raw=serde_json::to_vec(&json!({"openai":{"type":"oauth","access":"open-code-access","refresh":"open-code-refresh","expires":expires.as_millisecond(),"accountId":"open-code-account"}})).unwrap();
    let c = CodexOAuthCredentialsStore::parse_open_code(&raw).unwrap();
    assert_eq!(c.access_token_str(), "open-code-access");
    assert_eq!(c.refresh_token_str(), "open-code-refresh");
    assert_eq!(c.account_id.as_deref(), Some("open-code-account"));
    assert_eq!(c.source, CodexOAuthCredentialSource::OpenCode);
    assert!((c.expires_at.unwrap().as_millisecond() - expires.as_millisecond()).abs() < 1000);
    assert!(!c.needs_refresh());
    let home = TempDir::new().unwrap();
    assert_eq!(
        CodexOAuthCredentialsStore::save(&c, &env(home.path()), Some(home.path())).unwrap_err(),
        CodexOAuthCredentialsError::ReadOnlySource
    );
}

// upstream: CodexOAuthCredentialReadTests.swift:171 expired open code oauth credentials are marked for refresh
#[test]
fn expired_open_code_oauth_credentials_are_marked_for_refresh() {
    let c=CodexOAuthCredentialsStore::parse_open_code(&serde_json::to_vec(&json!({"openai":{"type":"oauth","access":"expired-access","refresh":"expired-refresh","expires":relative(-1).as_millisecond()}})).unwrap()).unwrap();
    assert_eq!(c.source, CodexOAuthCredentialSource::OpenCode);
    assert!(c.needs_refresh());
}

// upstream: CodexOAuthCredentialReadTests.swift:188 expired read-only oauth credentials fail closed without refresh
#[test]
fn expired_read_only_oauth_credentials_fail_closed_without_refresh() {
    let c = new_credentials(
        "expired-access",
        "external-refresh",
        CodexOAuthCredentialSource::OpenCode,
        Some(relative(-1)),
        None,
    );
    assert_eq!(
        CodexOAuthFetchStrategy::prepare_credentials_for_usage(&c, Timestamp::now()).unwrap_err(),
        CodexOAuthCredentialsError::ReadOnlySource
    );
}

// upstream: CodexOAuthCredentialReadTests.swift:207 expired read-only oauth credentials without a refresh token are rejected
#[test]
fn expired_read_only_oauth_credentials_without_a_refresh_token_are_rejected() {
    let c = new_credentials(
        "expired-access",
        "",
        CodexOAuthCredentialSource::LegacyCodexHome,
        Some(relative(-1)),
        None,
    );
    assert_eq!(
        CodexOAuthFetchStrategy::prepare_credentials_for_usage(&c, Timestamp::now()).unwrap_err(),
        CodexOAuthCredentialsError::ReadOnlySource
    );
}

// upstream: CodexOAuthCredentialReadTests.swift:227 expired external oauth fetch fails closed without mutating its source
#[test]
fn expired_external_oauth_fetch_fails_closed_without_mutating_its_source() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let path = external(
        data.path(),
        "expired-access",
        Some("external-refresh"),
        Some(1),
    );
    let original = fs::read(&path).unwrap();
    let c =
        CodexOAuthCredentialsStore::load_for_usage(&data_env(data.path()), true, Some(home.path()))
            .unwrap();
    assert_eq!(
        CodexOAuthFetchStrategy::prepare_credentials_for_usage(&c, Timestamp::now()).unwrap_err(),
        CodexOAuthCredentialsError::ReadOnlySource
    );
    assert_eq!(fs::read(path).unwrap(), original);
}

// upstream: CodexOAuthCredentialReadTests.swift:258 valid read-only oauth credentials pass through without refresh
#[test]
fn valid_read_only_oauth_credentials_pass_through_without_refresh() {
    let c = new_credentials(
        "valid-access",
        "external-refresh",
        CodexOAuthCredentialSource::OpenCode,
        Some(relative(3600)),
        Some(Timestamp::now()),
    );
    let resolved =
        CodexOAuthFetchStrategy::prepare_credentials_for_usage(&c, Timestamp::now()).unwrap();
    assert_eq!(resolved.access_token_str(), "valid-access");
    assert_eq!(resolved.source, CodexOAuthCredentialSource::OpenCode);
}

// upstream: CodexOAuthCredentialReadTests.swift:312 expired native oauth credentials delegate refresh to the Codex CLI
#[test]
fn expired_native_oauth_credentials_delegate_refresh_to_the_codex_cli() {
    let c = new_credentials(
        "expired-access",
        "native-refresh",
        CodexOAuthCredentialSource::CodexHome,
        None,
        Some(Timestamp::UNIX_EPOCH),
    );
    assert_eq!(
        CodexOAuthFetchStrategy::prepare_credentials_for_usage(&c, Timestamp::now()).unwrap_err(),
        CodexOAuthCredentialsError::NativeRefreshRequired
    );
}

// upstream: CodexOAuthCredentialReadTests.swift:330 valid native JWT expiry overrides a stale refresh timestamp
#[test]
fn valid_native_jwt_expiry_overrides_a_stale_refresh_timestamp() {
    let token = make_jwt(&json!({"exp":4_102_444_800_i64}));
    let c = native(&token, false);
    assert_eq!(
        c.expires_at,
        Some(Timestamp::from_second(4_102_444_800).unwrap())
    );
    assert!(!c.needs_refresh());
    assert_eq!(
        CodexOAuthFetchStrategy::prepare_credentials_for_usage(&c, Timestamp::now())
            .unwrap()
            .access_token_str(),
        token
    );
}

// upstream: CodexOAuthCredentialReadTests.swift:341 expired native JWT overrides a fresh refresh timestamp
#[test]
fn expired_native_jwt_overrides_a_fresh_refresh_timestamp() {
    let c = native(&make_jwt(&json!({"exp":0})), true);
    assert_eq!(c.expires_at, Some(Timestamp::UNIX_EPOCH));
    assert!(c.needs_refresh());
}

// upstream: CodexOAuthCredentialReadTests.swift:352 native OAuth refreshes within the Codex five minute window
#[test]
fn native_oauth_refreshes_within_the_codex_five_minute_window() {
    let expiry = Some(relative(2 * 60));
    let native = new_credentials(
        "native-access",
        "native-refresh",
        CodexOAuthCredentialSource::CodexHome,
        expiry,
        None,
    );
    let external = new_credentials(
        "external-access",
        "external-refresh",
        CodexOAuthCredentialSource::OpenCode,
        expiry,
        None,
    );
    assert!(native.needs_refresh());
    assert!(!external.needs_refresh());
}

// upstream: CodexOAuthCredentialReadTests.swift:376 native JWT accepts integer expiry values
#[test]
fn native_jwt_accepts_integer_expiry_values() {
    for expiration in [0_i64, 1_i64] {
        let c = native(&make_jwt(&json!({"exp":expiration})), false);
        assert_eq!(
            c.expires_at,
            Some(Timestamp::from_second(expiration).unwrap())
        );
    }
}

// upstream: CodexOAuthCredentialReadTests.swift:384 native JWT boolean expiry falls back to the refresh timestamp
#[test]
fn native_jwt_boolean_expiry_falls_back_to_the_refresh_timestamp() {
    let c = native(&make_jwt(&json!({"exp":true})), false);
    assert!(c.expires_at.is_none());
    assert!(c.needs_refresh());
}

// upstream: CodexOAuthCredentialReadTests.swift:393 native JWT fractional expiry falls back to the refresh timestamp
#[test]
fn native_jwt_fractional_expiry_falls_back_to_the_refresh_timestamp() {
    let c = native(&make_jwt(&json!({"exp":1.5})), false);
    assert!(c.expires_at.is_none());
    assert!(c.needs_refresh());
}

// upstream: CodexOAuthCredentialReadTests.swift:411 native JWT out of range expiry falls back to the refresh timestamp
#[test]
fn native_jwt_out_of_range_expiry_falls_back_to_the_refresh_timestamp() {
    let c = native(
        &make_jwt(&json!({"exp":9_223_372_036_854_775_808_u64})),
        false,
    );
    assert!(c.expires_at.is_none());
    assert!(c.needs_refresh());
}

// upstream: CodexOAuthCredentialReadTests.swift:421 native JWT rejects an integer outside the Codex date range
#[test]
fn native_jwt_rejects_an_integer_outside_the_codex_date_range() {
    let c = native(&make_jwt(&json!({"exp":i64::MAX})), false);
    assert!(c.expires_at.is_none());
    assert!(c.needs_refresh());
}

// upstream: CodexOAuthCredentialReadTests.swift:402 native JWT integral floating expiry falls back to the refresh timestamp
#[test]
fn native_jwt_integral_floating_expiry_falls_back_to_the_refresh_timestamp() {
    let c = native(&make_jwt_raw(br#"{"exp":4102444800.0}"#), false);
    assert!(c.expires_at.is_none());
    assert!(c.needs_refresh());
}

// upstream: CodexOAuthCredentialReadTests.swift:430 native JWT requires nonempty header and signature
#[test]
fn native_jwt_requires_nonempty_header_and_signature() {
    let valid = make_jwt(&json!({"exp":4_102_444_800_i64}));
    let parts: Vec<_> = valid.split('.').collect();
    for token in [
        format!(".{}.{}", parts[1], parts[2]),
        format!("{}.{}.", parts[0], parts[1]),
    ] {
        let c = native(&token, false);
        assert!(c.expires_at.is_none());
        assert!(c.needs_refresh());
    }
}

// upstream: CodexOAuthCredentialReadTests.swift:446 stale native probes never redeem a shared refresh token
#[test]
fn stale_native_probes_never_redeem_a_shared_refresh_token() {
    let c = new_credentials(
        "expired-access",
        "native-refresh",
        CodexOAuthCredentialSource::CodexHome,
        None,
        Some(Timestamp::UNIX_EPOCH),
    );
    for _ in 0..2 {
        assert_eq!(
            CodexOAuthFetchStrategy::prepare_credentials_for_usage(&c, Timestamp::now())
                .unwrap_err(),
            CodexOAuthCredentialsError::NativeRefreshRequired
        );
    }
}

// upstream: CodexOAuthCredentialReadTests.swift:527 open code api credentials are not accepted as oauth
#[test]
fn open_code_api_credentials_are_not_accepted_as_oauth() {
    assert_eq!(
        CodexOAuthCredentialsStore::parse_open_code(
            br#"{"openai":{"type":"api","key":"open-code-api-key"}}"#
        )
        .unwrap_err(),
        CodexOAuthCredentialsError::MissingTokens
    );
}

// upstream: CodexOAuthCredentialReadTests.swift:545 credential save preserves the supplied refresh timestamp
#[test]
fn credential_save_preserves_the_supplied_refresh_timestamp() {
    let home = TempDir::new().unwrap();
    let timestamp = Timestamp::from_second(1_700_000_000).unwrap();
    let mut c = new_credentials(
        "access",
        "refresh",
        CodexOAuthCredentialSource::CodexHome,
        None,
        Some(timestamp),
    );
    c.account_id = Some("account".into());
    CodexOAuthCredentialsStore::save(&c, &env(home.path()), Some(home.path())).unwrap();
    let value: Value =
        serde_json::from_slice(&fs::read(home.path().join("auth.json")).unwrap()).unwrap();
    let parsed = value["last_refresh"]
        .as_str()
        .unwrap()
        .parse::<Timestamp>()
        .unwrap();
    assert!((parsed.as_millisecond() - timestamp.as_millisecond()).abs() < 1);
}

// upstream: CodexOAuthCredentialReadTests.swift:586 native codex home wins over external oauth sources
#[test]
fn native_codex_home_wins_over_external_oauth_sources() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let native = home.path().join(".codex");
    fs::create_dir_all(&native).unwrap();
    fs::write(
        native.join("auth.json"),
        br#"{"tokens":{"access_token":"native-access","refresh_token":"native-refresh"}}"#,
    )
    .unwrap();
    external(data.path(), "external-access", None, None);
    let c =
        CodexOAuthCredentialsStore::load_for_usage(&data_env(data.path()), true, Some(home.path()))
            .unwrap();
    assert_eq!(c.source, CodexOAuthCredentialSource::CodexHome);
    assert_eq!(c.access_token_str(), "native-access");
}

// upstream: CodexOAuthCredentialReadTests.swift:626 legacy codex home wins before open code fallback
#[test]
fn legacy_codex_home_wins_before_open_code_fallback() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let native = home.path().join(".config/codex");
    fs::create_dir_all(&native).unwrap();
    fs::write(
        native.join("auth.json"),
        br#"{"tokens":{"access_token":"legacy-access","refresh_token":"legacy-refresh"}}"#,
    )
    .unwrap();
    external(data.path(), "external-access", None, None);
    let c =
        CodexOAuthCredentialsStore::load_for_usage(&data_env(data.path()), true, Some(home.path()))
            .unwrap();
    assert_eq!(c.source, CodexOAuthCredentialSource::LegacyCodexHome);
    assert_eq!(c.access_token_str(), "legacy-access");
}

// upstream: CodexOAuthCredentialReadTests.swift:668 legacy API keys are rejected by the external OAuth fallback
#[test]
fn legacy_api_keys_are_rejected_by_the_external_oauth_fallback() {
    let home = TempDir::new().unwrap();
    let path = home.path().join(".config/codex/auth.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, br#"{"OPENAI_API_KEY":"legacy-api-key"}"#).unwrap();
    assert_eq!(
        CodexOAuthCredentialsStore::load_for_usage(&BTreeMap::new(), true, Some(home.path()))
            .unwrap_err(),
        CodexOAuthCredentialsError::NotFound
    );
}

// upstream: CodexOAuthCredentialReadTests.swift:692 usage credential loading falls back to isolated open code data
#[test]
fn usage_credential_loading_falls_back_to_isolated_open_code_data() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    external(
        data.path(),
        "fallback-access",
        Some("fallback-refresh"),
        Some(relative(3600).as_millisecond()),
    );
    let c =
        CodexOAuthCredentialsStore::load_for_usage(&data_env(data.path()), true, Some(home.path()))
            .unwrap();
    assert_eq!(c.source, CodexOAuthCredentialSource::OpenCode);
    assert_eq!(c.access_token_str(), "fallback-access");
}

// upstream: CodexOAuthCredentialReadTests.swift:724 external fallback does not mask an unreadable native auth file
#[test]
fn external_fallback_does_not_mask_an_unreadable_native_auth_file() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    fs::create_dir_all(home.path().join(".codex/auth.json")).unwrap();
    external(data.path(), "must-not-mask-native-read-error", None, None);
    assert_eq!(
        CodexOAuthCredentialsStore::load_for_usage(&data_env(data.path()), true, Some(home.path()))
            .unwrap_err(),
        CodexOAuthCredentialsError::Unreadable
    );
}

// upstream: CodexOAuthCredentialReadTests.swift:756 external fallback does not mask malformed native auth json
#[test]
fn external_fallback_does_not_mask_malformed_native_auth_json() {
    let home = TempDir::new().unwrap();
    fs::create_dir(home.path().join(".codex")).unwrap();
    fs::write(home.path().join(".codex/auth.json"), br#"not-json"#).unwrap();
    assert!(matches!(
        CodexOAuthCredentialsStore::load_for_usage(&BTreeMap::new(), true, Some(home.path()))
            .unwrap_err(),
        CodexOAuthCredentialsError::DecodeFailed(_)
    ));
}

// upstream: CodexOAuthCredentialReadTests.swift:777 external fallback does not mask native auth without oauth tokens
#[test]
fn external_fallback_does_not_mask_native_auth_without_oauth_tokens() {
    let home = TempDir::new().unwrap();
    fs::create_dir(home.path().join(".codex")).unwrap();
    fs::write(home.path().join(".codex/auth.json"), br#"{"tokens":{}}"#).unwrap();
    assert!(matches!(
        CodexOAuthCredentialsStore::load_for_usage(&BTreeMap::new(), true, Some(home.path()))
            .unwrap_err(),
        CodexOAuthCredentialsError::MissingTokens
    ));
}

// upstream: CodexOAuthCredentialReadTests.swift:798 external OAuth fallback is disabled without explicit consent
#[test]
fn external_oauth_fallback_is_disabled_without_explicit_consent() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    external(data.path(), "must-not-be-read", None, None);
    assert_eq!(
        CodexOAuthCredentialsStore::load_for_usage(
            &data_env(data.path()),
            false,
            Some(home.path())
        )
        .unwrap_err(),
        CodexOAuthCredentialsError::NotFound
    );
}

// upstream: CodexOAuthCredentialReadTests.swift:827 explicit codex home does not borrow open code credentials
#[test]
fn explicit_codex_home_does_not_borrow_open_code_credentials() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    external(data.path(), "should-not-be-used", None, None);
    let mut env = data_env(data.path());
    env.extend(crate::env(home.path()));
    assert_eq!(
        CodexOAuthCredentialsStore::load_for_usage(&env, false, Some(home.path())).unwrap_err(),
        CodexOAuthCredentialsError::NotFound
    );
}

// upstream: CodexOAuthTests.swift:89 reset-credit token load ignores an API key beside O auth tokens
#[test]
fn reset_credit_token_load_ignores_an_api_key_beside_o_auth_tokens() {
    let home = TempDir::new().unwrap();
    fs::write(home.path().join("auth.json"),br#"{"OPENAI_API_KEY":"sk-test","tokens":{"access_token":"oauth-access-token","refresh_token":"oauth-refresh-token","account_id":"account-123"},"last_refresh":"2026-07-01T12:00:00Z"}"#).unwrap();
    let c = CodexOAuthCredentialsStore::load_oauth_tokens(&env(home.path()), Some(home.path()))
        .unwrap();
    assert_eq!(c.access_token_str(), "oauth-access-token");
    assert_eq!(c.refresh_token_str(), "oauth-refresh-token");
    assert_eq!(c.account_id.as_deref(), Some("account-123"));
}

// upstream: CodexOAuthCredentialsStorePermissionsTests.swift:11 saving O auth credentials keeps auth json private
#[test]
fn saving_o_auth_credentials_keeps_auth_json_private() {
    use std::os::unix::fs::PermissionsExt;
    let home = TempDir::new().unwrap();
    let c = CodexOAuthCredentials::new(
        "access-token",
        "refresh-token",
        Some(SecretString::new("id-token")),
        Some("account-123".into()),
        Some(Timestamp::now()),
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    CodexOAuthCredentialsStore::save(&c, &env(home.path()), Some(home.path())).unwrap();
    assert_eq!(
        fs::metadata(home.path().join("auth.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[path = "support/https.rs"]
mod https;
use ariadusage_core::{
    pipeline::{FetchContext, FetchInteraction, FetchPhase, FetchRuntime},
    providers::SourceMode,
};
use ariadusage_engine::brokers::net::NetConfig;
use ariadusage_protocol::ProviderId;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
fn context() -> FetchContext {
    FetchContext {
        provider: ProviderId::new("codex").unwrap(),
        runtime: FetchRuntime::Cli,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "fixture".into(),
        source_mode: SourceMode::Oauth,
        env: BTreeMap::new(),
        include_credits: false,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    }
}
// upstream: CodexOAuthCredentialReadTests.swift:274 managed workspace metadata supplies the account header without rewriting auth
#[tokio::test]
async fn managed_workspace_metadata_supplies_the_account_header_without_rewriting_auth() {
    let home = TempDir::new().unwrap();
    let server = https::start_server();
    fs::write(
        home.path().join("config.toml"),
        format!("chatgpt_base_url = \"{}/backend-api\"", server.base_url()),
    )
    .unwrap();
    let c = CodexOAuthCredentials::new(
        "valid-access",
        "refresh",
        None,
        Some("auth-account".into()),
        Some(Timestamp::now()),
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    let mut config = CodexStrategyConfig::new(
        Arc::new(https::broker(NetConfig::default())),
        env(home.path()),
        home.path(),
        home.path(),
    );
    config.managed_workspace_account_id = Some("workspace-team".into());
    let mock=server.mock_async(|when,then| {when.header("chatgpt-account-id","workspace-team").path("/backend-api/wham/usage");then.status(200).body(r#"{"rate_limit":{"primary_window":{"used_percent":4,"reset_at":1786161204,"limit_window_seconds":18000}}}"#);}).await;
    let r = CodexOAuthFetchStrategy::new(config)
        .fetch_with_credentials(&context(), c)
        .await
        .unwrap();
    assert_eq!(r.usage.primary.unwrap().used_percent, 4.);
    mock.assert_calls_async(1).await;
}

// upstream: CodexOAuthCredentialReadTests.swift:469 consented external OAuth fetch uses the token without mutating its source
#[tokio::test]
async fn consented_external_oauth_fetch_uses_the_token_without_mutating_its_source() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let auth = external(
        data.path(),
        "external-access",
        Some("external-refresh"),
        Some(4102444800000),
    );
    let bytes = fs::read(&auth).unwrap();
    let server = https::start_server();
    fs::create_dir(home.path().join(".codex")).unwrap();
    fs::write(
        home.path().join(".codex/config.toml"),
        format!("chatgpt_base_url = \"{}/backend-api\"", server.base_url()),
    )
    .unwrap();
    let env = data_env(data.path());
    let c = CodexOAuthCredentialsStore::load_for_usage(&env, true, Some(home.path())).unwrap();
    let mut config = CodexStrategyConfig::new(
        Arc::new(https::broker(NetConfig::default())),
        env,
        home.path(),
        home.path(),
    );
    config.allow_external_oauth_sources = true;
    let mock=server.mock_async(|when,then| {when.path("/backend-api/wham/usage").header("authorization","Bearer external-access");then.status(200).body(r#"{"rate_limit":{"primary_window":{"used_percent":12,"reset_at":1786161204,"limit_window_seconds":18000},"secondary_window":null}}"#);}).await;
    let r = CodexOAuthFetchStrategy::new(config)
        .fetch_with_credentials(&context(), c)
        .await
        .unwrap();
    assert_eq!(r.usage.primary.unwrap().used_percent, 12.);
    assert_eq!(fs::read(auth).unwrap(), bytes);
    mock.assert_calls_async(1).await;
}

// upstream: CodexOAuthCredentialsStorePermissionsTests.swift:36 auth json is private before atomic publication
#[test]
fn auth_json_is_private_before_atomic_publication() {
    use ariadusage_engine::{
        error::StoreError,
        private_file::{WriteHooks, write_private},
    };
    use std::os::unix::fs::PermissionsExt;
    let home = TempDir::new().unwrap();
    let auth = home.path().join("auth.json");
    fs::write(&auth, b"original").unwrap();
    let target = auth.clone();
    let hooks = WriteHooks {
        before_write: None,
        before_publish: Some(Arc::new(move |staged| {
            assert_eq!(
                fs::metadata(staged).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(fs::read(&target).unwrap(), b"original");
            Err(StoreError::Io(std::io::Error::other("stop")))
        })),
    };
    let error = write_private(&auth, b"replacement", &hooks).unwrap_err();
    assert!(error.to_string().contains("stop"));
    assert_eq!(fs::read(auth).unwrap(), b"original");
    let entries = fs::read_dir(home.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(entries, ["auth.json"]);
}
