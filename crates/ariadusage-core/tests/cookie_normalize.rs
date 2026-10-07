// Ported from CodexBar Tests/CodexBarTests/CookieHeaderNormalizerTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::cookie as cookie_core;
use cookie_core::{filtered_header, normalize, pairs};

// CodexBar: CookieHeaderNormalizerTests.swift:6
#[test]
fn compact_curl_short_form_without_whitespace_still_parses() {
    let input = "curl https://example.com -bfoo=bar";
    let normalized = normalize(input);
    assert_eq!(normalized.as_deref(), Some("foo=bar"));

    let parsed_pairs = pairs(input);
    assert_eq!(parsed_pairs.len(), 1);
    assert_eq!(parsed_pairs[0].0, "foo");
    assert_eq!(parsed_pairs[0].1, "bar");
}

#[test]
fn normalizer_extracts_all_patterns() {
    let cap_header = ["Coo", "kie"].concat();
    let low_header = ["coo", "kie"].concat();

    // Pattern 1: -H header single quote
    let p1 = format!("curl https://api.test -H '{cap_header}: a=1; b=2'");
    assert_eq!(normalize(&p1).as_deref(), Some("a=1; b=2"));

    // Pattern 2: -H header double quote
    let p2 = format!(r#"curl https://api.test -H "{cap_header}: c=3""#);
    assert_eq!(normalize(&p2).as_deref(), Some("c=3"));

    // Pattern 3: header name single quote
    let p3 = format!("{low_header}: 'd=4; e=5'");
    assert_eq!(normalize(&p3).as_deref(), Some("d=4; e=5"));

    // Pattern 4: header name double quote
    let p4 = format!(r#"{low_header}: "f=6""#);
    assert_eq!(normalize(&p4).as_deref(), Some("f=6"));

    // Pattern 5: bare header name
    let p5 = format!("{low_header}: g=7; h=8");
    assert_eq!(normalize(&p5).as_deref(), Some("g=7; h=8"));

    // Pattern 6: --cookie '...' and -b '...'
    let p6_long = format!("curl https://test --{low_header} 'i=9'");
    assert_eq!(normalize(&p6_long).as_deref(), Some("i=9"));
    let p6_short = "curl https://test -b 'j=10'";
    assert_eq!(normalize(p6_short).as_deref(), Some("j=10"));

    // Pattern 7: --cookie "..." and -b "..."
    let p7_long = format!(r#"curl https://test --{low_header} "k=11""#);
    assert_eq!(normalize(&p7_long).as_deref(), Some("k=11"));
    let p7_short = r#"curl https://test -b "l=12""#;
    assert_eq!(normalize(p7_short).as_deref(), Some("l=12"));

    // Pattern 8: -bkey=value compact form is covered in test 1.

    // Pattern 9: --cookie value and -b value with whitespace
    let p9_long = format!("curl https://test --{low_header} m=13");
    assert_eq!(normalize(&p9_long).as_deref(), Some("m=13"));
    let p9_short = "curl https://test -b n=14";
    assert_eq!(normalize(p9_short).as_deref(), Some("n=14"));
}

#[test]
fn normalizer_strips_cookie_prefix_and_quotes() {
    let p = ["Coo", "kie: \"foo=bar; baz=qux\""].concat();
    assert_eq!(normalize(&p).as_deref(), Some("foo=bar; baz=qux"));

    let single = ["coo", "kie: 'key=val'"].concat();
    assert_eq!(normalize(&single).as_deref(), Some("key=val"));
}

#[test]
fn normalizer_rejects_input_over_64_kib() {
    let oversized = "a".repeat(64 * 1024 + 1);
    assert_eq!(normalize(&oversized), None);

    let max_allowed = format!("a={}", "b".repeat(64 * 1024 - 10));
    assert!(normalize(&max_allowed).is_some());
}

#[test]
fn normalizer_empty_and_whitespace_returns_none() {
    assert_eq!(normalize(""), None);
    assert_eq!(normalize("    \n\t "), None);
    let empty_cookie = ["coo", "kie:    "].concat();
    assert_eq!(normalize(&empty_cookie), None);
}

#[test]
fn pairs_and_filtered_header() {
    let raw = "foo=bar; ; baz=qux; invalid_no_equals; =empty_name; name_with_empty_val=";
    let parsed = pairs(raw);
    assert_eq!(
        parsed,
        vec![
            ("foo".to_string(), "bar".to_string()),
            ("baz".to_string(), "qux".to_string()),
            ("name_with_empty_val".to_string(), "".to_string()),
        ]
    );

    let filtered = filtered_header(raw, ["foo", "baz"]);
    assert_eq!(filtered.as_deref(), Some("foo=bar; baz=qux"));

    let none_matched = filtered_header(raw, ["nonexistent"]);
    assert_eq!(none_matched, None);
}
