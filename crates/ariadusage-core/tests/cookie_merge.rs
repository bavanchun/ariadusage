// Ported from CodexBar Tests/CodexBarTests/BrowserCookieProfilesTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::cookie as cookie_core;
use ariadusage_protocol::secret::SecretString;
use cookie_core::{CookieRecord, CookieStoreKind, CookieStoreRecords, merge_profiles};
use jiff::Timestamp;
use std::collections::{HashMap, HashSet};

fn make_cookie(
    name: &str,
    val: &str,
    domain: &str,
    path: &str,
    host_only: bool,
    expires: Option<Timestamp>,
) -> CookieRecord {
    CookieRecord {
        name: name.to_string(),
        value: SecretString::from(val),
        domain: domain.to_string(),
        host_only,
        path: path.to_string(),
        secure: true,
        http_only: true,
        expires,
    }
}

fn make_source(
    profile_id: &str,
    label: &str,
    kind: CookieStoreKind,
    records: Vec<CookieRecord>,
) -> CookieStoreRecords {
    CookieStoreRecords {
        profile_id: profile_id.to_string(),
        store_label: label.to_string(),
        store_kind: kind,
        records,
    }
}

// CodexBar: BrowserCookieProfilesTests.swift:8
#[test]
fn profile_stores_merge_by_expiry_and_retain_network_priority_on_ties() {
    let early = Some(Timestamp::from_second(100).unwrap());
    let late = Some(Timestamp::from_second(200).unwrap());

    let sources = vec![
        make_source(
            "alpha",
            "Alpha",
            CookieStoreKind::Primary,
            vec![
                make_cookie("tie", "primary", "example.com", "/", false, late),
                make_cookie("newer", "primary-new", "example.com", "/", false, late),
                make_cookie("persistent", "primary", "example.com", "/", false, early),
            ],
        ),
        make_source(
            "alpha",
            "Alpha (Network)",
            CookieStoreKind::Network,
            vec![
                make_cookie("tie", "network", "example.com", "/", false, late),
                make_cookie("newer", "network-old", "example.com", "/", false, early),
                make_cookie(
                    "persistent",
                    "network-session",
                    "example.com",
                    "/",
                    false,
                    None,
                ),
            ],
        ),
    ];

    let profiles = merge_profiles(sources);
    assert_eq!(profiles.len(), 1);
    let profile = &profiles[0];
    assert_eq!(profile.label, "Alpha");

    let values: HashMap<String, String> = profile
        .records
        .iter()
        .map(|r| (r.name.clone(), r.value.expose_secret().to_string()))
        .collect();

    assert_eq!(values.get("tie").map(String::as_str), Some("network"));
    assert_eq!(values.get("newer").map(String::as_str), Some("primary-new"));
    assert_eq!(
        values.get("persistent").map(String::as_str),
        Some("primary")
    );
}

// CodexBar: BrowserCookieProfilesTests.swift:31
#[test]
fn profiles_domains_and_paths_remain_separate() {
    let sources = vec![
        make_source(
            "beta",
            "Beta (Network)",
            CookieStoreKind::Network,
            vec![make_cookie(
                "session",
                "beta",
                "example.com",
                "/",
                false,
                None,
            )],
        ),
        make_source(
            "alpha",
            "Alpha",
            CookieStoreKind::Primary,
            vec![
                make_cookie("session", "root", "example.com", "/", false, None),
                make_cookie("session", "path", "example.com", "/other", false, None),
                make_cookie("session", "domain", "other.test", "/", false, None),
            ],
        ),
    ];

    let profiles = merge_profiles(sources);
    let labels: Vec<&str> = profiles.iter().map(|p| p.label.as_str()).collect();
    assert_eq!(labels, vec!["Alpha", "Beta"]);

    let alpha_vals: HashSet<&str> = profiles[0]
        .records
        .iter()
        .map(|r| r.value.expose_secret())
        .collect();
    let expected_alpha: HashSet<&str> = ["root", "path", "domain"].into_iter().collect();
    assert_eq!(alpha_vals, expected_alpha);

    let beta_vals: Vec<&str> = profiles[1]
        .records
        .iter()
        .map(|r| r.value.expose_secret())
        .collect();
    assert_eq!(beta_vals, vec!["beta"]);

    assert!(merge_profiles(Vec::new()).is_empty());
}

// CodexBar: BrowserCookieProfilesTests.swift:49
#[test]
fn session_cookie_ties_retain_network_value_and_empty_profiles_remain_ordered() {
    let sources = vec![
        make_source("empty", "Alpha", CookieStoreKind::Primary, vec![]),
        make_source(
            "signed-in",
            "Beta",
            CookieStoreKind::Primary,
            vec![make_cookie(
                "session",
                "primary",
                "example.com",
                "/",
                false,
                None,
            )],
        ),
        make_source(
            "signed-in",
            "Beta (Network)",
            CookieStoreKind::Network,
            vec![make_cookie(
                "session",
                "network",
                "example.com",
                "/",
                false,
                None,
            )],
        ),
    ];

    let profiles = merge_profiles(sources);
    let labels: Vec<&str> = profiles.iter().map(|p| p.label.as_str()).collect();
    assert_eq!(labels, vec!["Alpha", "Beta"]);
    assert!(profiles[0].records.is_empty());
    assert_eq!(profiles[1].records[0].value.expose_secret(), "network");
}

// CodexBar: BrowserCookieProfilesTests.swift:81
#[test]
fn host_only_and_domain_cookies_survive_merging_with_identical_names_and_paths() {
    let records = vec![
        make_cookie("session", "host-only", "example.test", "/", true, None),
        make_cookie("session", "domain-scope", "example.test", "/", false, None),
    ];

    let sources = vec![make_source(
        "fixture",
        "Fixture",
        CookieStoreKind::Primary,
        records,
    )];

    let profiles = merge_profiles(sources);
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].records.len(), 2);
}

#[test]
fn debug_formatting_redacts_cookie_value() {
    let secret = "sensitive-secret-token";
    let record = make_cookie("session", secret, "example.com", "/", false, None);
    let debug_output = format!("{record:?}");
    assert!(debug_output.contains("[redacted]"));
    assert!(!debug_output.contains(secret));
}
