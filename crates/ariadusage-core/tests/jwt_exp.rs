// Ported from CodexBar TestsLinux/CodexNativeJWTExpiryTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::jwt::exp_claim;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

fn make_token_from_payload(payload: &str) -> String {
    let header = "eyJhbGciOiJIUzI1NiJ9";
    let encoded_payload = STANDARD.encode(payload.as_bytes());
    let sig = "fixture-sig";
    format!("{header}.{encoded_payload}.{sig}")
}

// CodexBar: TestsLinux/CodexNativeJWTExpiryTests.swift:7
#[test]
fn test_accepts_signed_integer_spellings_within_the_codex_date_range() {
    let cases = [
        ("-8334601228800", -8_334_601_228_800),
        ("-1", -1),
        ("-0", 0),
        ("0", 0),
        ("1", 1),
        ("4102444800", 4_102_444_800),
        ("8210266876799", 8_210_266_876_799),
    ];

    for (raw, expected) in cases {
        let payload = format!(r#"{{"exp":{raw}}}"#);
        let token = make_token_from_payload(&payload);
        assert_eq!(
            exp_claim(&token),
            Some(expected),
            "Failed for raw value {raw}"
        );
    }
}

// CodexBar: TestsLinux/CodexNativeJWTExpiryTests.swift:23
#[test]
fn test_rejected_raw_claim_spellings_retain_timestamp_freshness() {
    let rejected = [
        "true",
        "false",
        "null",
        "\"4102444800\"",
        "[]",
        "{}",
        "1.5",
        "-1.5",
        "0.0",
        "-0.0",
        "1.0",
        "4102444800.0",
        "1e0",
        "1E0",
        "4102444800e0",
        "4102444800E+0",
        "41024448000e-1",
        "4.1024448e9",
        "1e309",
        "1e-999",
        "9223372036854775807",
        "9223372036854775808",
        "18446744073709551616",
        "-9223372036854775808",
        "-9223372036854775809",
        "-8334601228801",
        "8210266876800",
    ];

    for raw in rejected {
        let payload = format!(r#"{{"exp":{raw}}}"#);
        let token = make_token_from_payload(&payload);
        assert_eq!(
            exp_claim(&token),
            None,
            "Expected None for rejected raw spelling {raw}"
        );
    }
}

// CodexBar: TestsLinux/CodexNativeJWTExpiryTests.swift:38
#[test]
fn test_absent_malformed_nested_and_ambiguous_claims_fail_soft() {
    let payloads = [
        "{}",
        r#"{"nested":{"exp":4102444800}}"#,
        r#"{"exp":[4102444800]}"#,
        r#"{"exp":{"exp":4102444800}}"#,
        r#"{"note":"\"exp\":4102444800"}"#,
        r#"{"exp":4102444800,"exp":0}"#,
        r#"{"exp":0,"exp":4102444800}"#,
        r#"{"exp":null,"exp":4102444800}"#,
        r#"{"exp":4102444800,"\u0065xp":0}"#,
        "[]",
        "null",
        "not-json",
        r#"{"exp":4102444800"#,
        r#"{"exp":+1}"#,
        r#"{"exp":01}"#,
    ];

    for payload in payloads {
        let token = make_token_from_payload(payload);
        assert_eq!(
            exp_claim(&token),
            None,
            "Expected None for ambiguous/malformed payload: {payload}"
        );
    }
}

// CodexBar: TestsLinux/CodexNativeJWTExpiryTests.swift:51
#[test]
fn test_only_the_top_level_expiration_spelling_matters() {
    let payloads = [
        r#"{"\u0065xp":4102444800}"#,
        r#"{"nested":[{"exp":0},1.0],"note":"exp \" [ }","exp":4102444800,"after":1e0}"#,
        r#"{"exp":4102444800,"nested":{"exp":0},"note":"\"exp\":0"}"#,
    ];

    for payload in payloads {
        let token = make_token_from_payload(payload);
        assert_eq!(
            exp_claim(&token),
            Some(4_102_444_800),
            "Failed for payload {payload}"
        );
    }
}

// CodexBar: TestsLinux/CodexNativeJWTExpiryTests.swift:58
#[test]
fn test_opaque_and_malformed_tokens_retain_the_age_fallback() {
    let tokens = [
        "opaque".to_string(),
        "a.%%%.c".to_string(),
        "a..c".to_string(),
        ".e30.c".to_string(),
        "a.e30.".to_string(),
        "a.e30.c.extra".to_string(),
    ];

    for token in &tokens {
        assert_eq!(
            exp_claim(token),
            None,
            "Expected None for malformed token {token}"
        );
    }
}

// CodexBar: TestsLinux/CodexNativeJWTExpiryTests.swift:82
#[test]
fn test_expiry_does_not_validate_headers_signatures_or_change_account_recovery() {
    let payload =
        STANDARD.encode(r#"{"exp":4102444800,"chatgpt_account_id":"fixture-account"}"#.as_bytes());

    let unverified = format!("%%.{payload}.%%");
    assert_eq!(exp_claim(&unverified), Some(4_102_444_800));

    let leading_dot = format!(".{payload}.signature");
    assert_eq!(exp_claim(&leading_dot), None);

    let trailing_dot = format!("header.{payload}.");
    assert_eq!(exp_claim(&trailing_dot), None);
}
