// Ported from CodexBar Tests/CodexBarTests/ClaudeWebAccountInfoTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeWebBrowserFallbackTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeWebCloudflareChallengeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeWebCookieRenewalTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeWebEnterpriseUsageTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeWebFetchDeadlineTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeWebUsageExtraWindowTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeWebBackgroundRecoveryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeSubscriptionMetadataTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeSubscriptionCLITests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeUsageTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#[path = "support/https.rs"]
mod https;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ariadusage_core::config::CookieSource;
use ariadusage_core::model::{
    NamedRateWindow, ProviderCostSnapshot, ProviderIdentitySnapshot, RateWindow, UsageSnapshot,
};
use ariadusage_core::pipeline::{
    BoxFuture, FetchContext, FetchError, FetchInteraction, FetchPhase, FetchResult, FetchRuntime,
    FetchStrategy, ProviderPlan, Sleeper, fetch_outcome,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_engine::brokers::browser::Browser;
use ariadusage_engine::brokers::cookie_cache::CookieCache;
use ariadusage_engine::brokers::net::NetConfig;
use ariadusage_engine::providers::claude::{
    CLAUDE_OAUTH_TOKEN_ENV_KEY, CLAUDE_WEB_STRATEGY_ID, CLOUDFLARE_CHALLENGE_MESSAGE,
    ClaudeAccountIdentity, ClaudeAccountProfile, ClaudeOAuthEndpointConfig,
    ClaudeOAuthFetchStrategy, ClaudeOAuthStrategyConfig, ClaudeOAuthUsageFetcher,
    ClaudeProviderPlan, ClaudeSubscriptionMetadata, ClaudeSubscriptionParseError,
    ClaudeWebAPIFetcher, ClaudeWebBrowserFetchGate, ClaudeWebEndpointConfig,
    ClaudeWebEnrichmentAccess, ClaudeWebFetchError, ClaudeWebFetchOptions, ClaudeWebFetchStrategy,
    ClaudeWebFetchStrategyError, ClaudeWebSessionImporter, ClaudeWebSessionKeyRenewalTracker,
    ConfigOwnershipEvidence, DEFAULT_PREPAID_CREDITS_TIMEOUT, DEFAULT_WEB_REQUEST_TIMEOUT,
    OrganizationInfo, SESSION_KEY_COOKIE_NAME, SessionKeyInfo, WebAccountInfo,
    WebExtrasApplyParams, WebUsageData, apply_web_extras_if_needed, merge_extra_rate_windows,
    merge_provider_cost, web_extras_account_matches,
};
use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use ariadusage_protocol::{Confidence, ProviderId};
use httpmock::prelude::*;
use jiff::{SignedDuration, Timestamp};
use reqwest::Url;
use tokio_util::sync::CancellationToken;

fn session_hdr(key: &str) -> String {
    format!("{SESSION_KEY_COOKIE_NAME}={key}")
}

fn set_session_hdr(key: &str) -> String {
    format!("{SESSION_KEY_COOKIE_NAME}={key}; Path=/; HttpOnly")
}

struct NoopSleeper;

impl Sleeper for NoopSleeper {
    fn sleep<'a>(&'a self, _duration: Duration) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

#[test]
fn account_uuid_and_config_ownership_evidence_from_config_json() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let work = temp.path().join("work");
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    let env = BTreeMap::new();
    let call = https::call();

    // 1. Absent .config.json
    assert_eq!(
        ClaudeAccountProfile::config_ownership_evidence(&env, &work, &home, &call),
        ConfigOwnershipEvidence::SignedOut
    );
    assert_eq!(
        ClaudeAccountProfile::account_uuid(&env, &work, &home, &call),
        None
    );
    let anon_a = ClaudeAccountProfile::session_scope(&env, &work, &home, "fb-a", &call);
    let anon_b = ClaudeAccountProfile::session_scope(&env, &work, &home, "fb-b", &call);
    assert_eq!(anon_a.len(), 64);
    assert_ne!(anon_a, anon_b, "absent config scopes by fallback_id");
    assert_eq!(
        ClaudeAccountProfile::identified_session_scope(&env, &work, &home, &call),
        None
    );

    // 2. Valid oauthAccount.accountUuid
    let config_file = claude_dir.join(".config.json");
    std::fs::write(
        &config_file,
        br#"{"oauthAccount":{"accountUuid":"  Acc-UUID-42  "}}"#,
    )
    .unwrap();
    assert_eq!(
        ClaudeAccountProfile::config_ownership_evidence(&env, &work, &home, &call),
        ConfigOwnershipEvidence::SignedIn {
            account_uuid: "Acc-UUID-42".to_owned()
        }
    );
    assert_eq!(
        ClaudeAccountProfile::account_uuid(&env, &work, &home, &call).as_deref(),
        Some("Acc-UUID-42")
    );
    let signed_scope = ClaudeAccountProfile::session_scope(&env, &work, &home, "fb-a", &call);
    assert_eq!(
        ClaudeAccountProfile::identified_session_scope(&env, &work, &home, &call).as_deref(),
        Some(signed_scope.as_str())
    );

    // 3. Empty accountUuid -> Indeterminate
    std::fs::write(&config_file, br#"{"oauthAccount":{"accountUuid":"   "}}"#).unwrap();
    assert_eq!(
        ClaudeAccountProfile::config_ownership_evidence(&env, &work, &home, &call),
        ConfigOwnershipEvidence::Indeterminate
    );

    // 4. Missing oauthAccount -> SignedOut
    std::fs::write(&config_file, br#"{"theme":"dark"}"#).unwrap();
    assert_eq!(
        ClaudeAccountProfile::config_ownership_evidence(&env, &work, &home, &call),
        ConfigOwnershipEvidence::SignedOut
    );

    // 5. Malformed JSON -> Indeterminate
    std::fs::write(&config_file, b"{not-json").unwrap();
    assert_eq!(
        ClaudeAccountProfile::config_ownership_evidence(&env, &work, &home, &call),
        ConfigOwnershipEvidence::Indeterminate
    );

    // 6. Large .claude.json (> 1 MiB) still resolves SignedIn via 64 MiB bounded plain reader
    std::fs::remove_file(&config_file).unwrap();
    let claude_json = home.join(".claude.json");
    let padding = "x".repeat(1_200_000);
    let large_payload =
        format!(r#"{{"padding":"{padding}","oauthAccount":{{"accountUuid":"large-file-uuid"}}}}"#);
    std::fs::write(&claude_json, large_payload.as_bytes()).unwrap();
    assert_eq!(
        ClaudeAccountProfile::config_ownership_evidence(&env, &work, &home, &call),
        ConfigOwnershipEvidence::SignedIn {
            account_uuid: "large-file-uuid".to_owned()
        }
    );
}

#[test]
fn parses_claude_web_api_usage_responses_and_sonnet_and_enterprise_shapes() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();

    // Standard five_hour + seven_day + seven_day_opus
    let parsed = ClaudeWebAPIFetcher::parse_usage_response(
        br#"{
          "five_hour": { "utilization": 9, "resets_at": "2025-12-23T16:00:00.000Z" },
          "seven_day": { "utilization": 4, "resets_at": "2025-12-29T23:00:00.000Z" },
          "seven_day_opus": { "utilization": 1 }
        }"#,
        now,
    )
    .expect("parse usage");
    assert_eq!(parsed.session_percent_used, Some(9.0));
    assert_eq!(parsed.weekly_percent_used, Some(4.0));
    assert_eq!(parsed.opus_percent_used, Some(1.0));
    assert!(parsed.session_resets_at.is_some());
    assert!(parsed.weekly_resets_at.is_some());

    // Weekly missing
    let no_weekly = ClaudeWebAPIFetcher::parse_usage_response(
        br#"{"five_hour": { "utilization": 9, "resets_at": "2025-12-23T16:00:00.000Z" }}"#,
        now,
    )
    .expect("parse no weekly");
    assert_eq!(no_weekly.session_percent_used, Some(9.0));
    assert_eq!(no_weekly.weekly_percent_used, None);

    // Sonnet usage response
    let sonnet = ClaudeWebAPIFetcher::parse_usage_response(
        br#"{
          "five_hour": { "utilization": 9, "resets_at": "2025-12-23T16:00:00.000Z" },
          "seven_day_sonnet": { "utilization": 6, "resets_at": "2025-12-30T23:00:00.000Z" }
        }"#,
        now,
    )
    .expect("parse sonnet");
    assert_eq!(sonnet.opus_percent_used, Some(6.0));

    // Enterprise: session window is null, seven_day present
    let null_session = ClaudeWebAPIFetcher::parse_usage_response(
        br#"{
          "five_hour": null,
          "seven_day": { "utilization": 42, "resets_at": "2025-12-29T23:00:00.000Z" }
        }"#,
        now,
    )
    .expect("parse null session");
    assert_eq!(null_session.session_percent_used, None);
    assert_eq!(null_session.weekly_percent_used, Some(42.0));

    // Enterprise credit spend from usage response
    let enterprise_spend = ClaudeWebAPIFetcher::parse_usage_response(
        br#"{
          "five_hour": null,
          "seven_day": null,
          "extra_usage": {
            "monthly_limit": 100000,
            "used_credits": 4132
          }
        }"#,
        now,
    )
    .expect("parse enterprise spend");
    assert_eq!(enterprise_spend.session_percent_used, None);
    assert_eq!(enterprise_spend.weekly_percent_used, None);
    let cost = enterprise_spend.extra_usage_cost.expect("extra usage cost");
    assert_eq!(cost.used, 41.32);
    assert_eq!(cost.limit, 1000.0);
    assert_eq!(cost.currency_code, "USD");
    assert_eq!(cost.period.as_deref(), Some("Monthly cap"));
}

#[test]
fn parses_overage_spend_limit_organizations_and_account_info() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();

    // Overage spend limit EUR and USD cents
    let eur = ClaudeWebAPIFetcher::parse_overage_spend_limit(
        br#"{"monthly_credit_limit": 2000, "currency": "EUR", "used_credits": 0, "is_enabled": true}"#,
        now,
    )
    .expect("eur cost");
    assert_eq!(eur.currency_code, "EUR");
    assert_eq!(eur.limit, 20.0);
    assert_eq!(eur.used, 0.0);
    assert_eq!(eur.period.as_deref(), Some("Monthly cap"));

    let usd = ClaudeWebAPIFetcher::parse_overage_spend_limit(
        br#"{"monthly_credit_limit": 12345, "currency": "USD", "used_credits": 6789, "is_enabled": true}"#,
        now,
    )
    .expect("usd cost");
    assert_eq!(usd.currency_code, "USD");
    assert_eq!(usd.limit, 123.45);
    assert_eq!(usd.used, 67.89);

    // Organizations selection: chat > non-api-only > first, or target_organization_id
    let single_org = ClaudeWebAPIFetcher::parse_organizations_response(
        br#"[{ "uuid": "org-123", "name": "Example Org", "capabilities": [] }]"#,
        None,
    )
    .unwrap();
    assert_eq!(single_org.id, "org-123");
    assert_eq!(single_org.name.as_deref(), Some("Example Org"));

    let chat_preferred = ClaudeWebAPIFetcher::parse_organizations_response(
        br#"[
          { "uuid": "org-api", "name": "API Org", "capabilities": ["api"] },
          { "uuid": "org-chat", "name": "Chat Org", "capabilities": ["chat"] }
        ]"#,
        None,
    )
    .unwrap();
    assert_eq!(chat_preferred.id, "org-chat");

    let hybrid_preferred = ClaudeWebAPIFetcher::parse_organizations_response(
        br#"[
          { "uuid": "org-api", "name": "API Org", "capabilities": ["api"] },
          { "uuid": "org-hybrid", "name": "Hybrid Org", "capabilities": ["api", "chat"] }
        ]"#,
        None,
    )
    .unwrap();
    assert_eq!(hybrid_preferred.id, "org-hybrid");

    let targeted = ClaudeWebAPIFetcher::parse_organizations_response(
        br#"[
          { "uuid": "org-api", "name": "API Org", "capabilities": ["api"] },
          { "uuid": "org-chat", "name": "Chat Org", "capabilities": ["chat"] }
        ]"#,
        Some("org-api"),
    )
    .unwrap();
    assert_eq!(targeted.id, "org-api");

    let missing_target = ClaudeWebAPIFetcher::parse_organizations_response(
        br#"[{ "uuid": "org-chat", "name": "Chat Org", "capabilities": ["chat"] }]"#,
        Some("org-missing"),
    )
    .unwrap_err();
    assert_eq!(
        missing_target,
        ClaudeWebFetchError::OrganizationNotFound("org-missing".to_owned())
    );
    assert_eq!(
        missing_target.to_string(),
        "The configured Claude organization was not found for this session."
    );
    assert!(!format!("{missing_target:?}").contains("org-missing"));

    // Account info: seat tier & selected org
    let seats_json = br#"{
      "email_address": "synthetic@example.invalid",
      "memberships": [
        {
          "seat_tier": "team_standard",
          "organization": {
            "uuid": "org-standard",
            "name": "Standard Org",
            "rate_limit_tier": "claude_team",
            "billing_type": "stripe_subscription"
          }
        },
        {
          "seat_tier": "team_tier_1",
          "organization": {
            "uuid": "org-premium",
            "name": "Premium Org",
            "rate_limit_tier": "claude_team",
            "billing_type": "stripe_subscription"
          }
        }
      ]
    }"#;
    let premium =
        ClaudeWebAPIFetcher::parse_account_info(seats_json, Some("org-premium")).expect("premium");
    let standard = ClaudeWebAPIFetcher::parse_account_info(seats_json, Some("org-standard"))
        .expect("standard");
    assert_eq!(premium.login_method.as_deref(), Some("Claude Team Premium"));
    assert_eq!(
        standard.login_method.as_deref(),
        Some("Claude Team Standard")
    );

    let enterprise_json = br#"{
      "email_address": "enterprise@example.invalid",
      "memberships": [
        {
          "seat_tier": "team_tier_1",
          "organization": {
            "uuid": "org-enterprise",
            "name": "Enterprise Org",
            "rate_limit_tier": "claude_enterprise",
            "billing_type": "stripe_subscription"
          }
        }
      ]
    }"#;
    let ent = ClaudeWebAPIFetcher::parse_account_info(enterprise_json, Some("org-enterprise"))
        .expect("ent");
    assert_eq!(ent.login_method.as_deref(), Some("Claude Enterprise"));

    let fallback_first = ClaudeWebAPIFetcher::parse_account_info(seats_json, None).expect("first");
    assert_eq!(
        fallback_first.login_method.as_deref(),
        Some("Claude Team Standard")
    );
}

#[test]
fn extra_windows_prepaid_credits_and_web_extras_account_matching() {
    let now = Timestamp::from_second(1_800_000_000).unwrap();

    // 1. Boolean scoped percentages do not become 0% or 1%
    let bool_json = br#"{"five_hour":{"utilization":12},"limits":[
      {"kind":"weekly_scoped","group":"weekly","percent":true,"scope":{"model":{"display_name":"Fable"}}},
      {"kind":"weekly_scoped","group":"weekly","percent":false,"scope":{"model":{"display_name":"Story"}}}
    ]}"#;
    let web = ClaudeWebAPIFetcher::parse_usage_response(bool_json, now).unwrap();
    let oauth_resp = ClaudeOAuthUsageFetcher::decode_usage_response(bool_json).unwrap();
    let oauth_mapped =
        ClaudeOAuthUsageFetcher::map_usage_response(&oauth_resp, None, None, None, None, true, now)
            .unwrap();
    assert!(web.extra_rate_windows.is_empty());
    assert!(oauth_mapped.snapshot.extra_rate_windows.is_none());

    // 2. Prepaid credits in minor units and rejection of invalid balances
    let cost =
        ClaudeWebAPIFetcher::parse_prepaid_credits(br#"{"amount":10000,"currency":"usd"}"#, now)
            .expect("prepaid cost");
    assert_eq!(cost.balance, Some(100.0));
    assert_eq!(cost.currency_code, "USD");
    assert_eq!(cost.used, 0.0);
    assert_eq!(cost.limit, 0.0);

    assert!(
        ClaudeWebAPIFetcher::parse_prepaid_credits(br#"{"amount":-1,"currency":"USD"}"#, now)
            .is_none()
    );
    assert!(ClaudeWebAPIFetcher::parse_prepaid_credits(br#"{"amount":10000}"#, now).is_none());
    assert!(
        ClaudeWebAPIFetcher::parse_prepaid_credits(br#"{"amount":true,"currency":"USD"}"#, now)
            .is_none()
    );

    // 3. Merge prepaid balance into primary OAuth cost & currency mismatch guard
    let mut primary_cost = ProviderCostSnapshot::new(5.0, 20.0, "USD", now);
    primary_cost.period = Some("Monthly cap".to_owned());
    let mut web_usd = ProviderCostSnapshot::new(
        0.0,
        0.0,
        "usd",
        now.checked_add(SignedDuration::from_secs(1)).unwrap(),
    );
    web_usd.period = Some("Extra usage".to_owned());
    web_usd.balance = Some(100.0);

    let merged_usd =
        merge_provider_cost(Some(primary_cost.clone()), Some(web_usd), true).expect("merged usd");
    assert_eq!(merged_usd.used, 5.0);
    assert_eq!(merged_usd.limit, 20.0);
    assert_eq!(merged_usd.period.as_deref(), Some("Monthly cap"));
    assert_eq!(merged_usd.balance, Some(100.0));
    assert_eq!(merged_usd.updated_at, now);

    let mut web_eur = ProviderCostSnapshot::new(0.0, 0.0, "EUR", now);
    web_eur.balance = Some(100.0);
    let merged_eur =
        merge_provider_cost(Some(primary_cost.clone()), Some(web_eur), true).expect("merged eur");
    assert_eq!(merged_eur, primary_cost);

    // Web response without extra_usage_cost preserves primary OAuth provider_cost
    assert_eq!(
        merge_provider_cost(Some(primary_cost.clone()), None, true),
        Some(primary_cost.clone())
    );
    assert_eq!(
        merge_provider_cost(Some(primary_cost.clone()), None, false),
        Some(primary_cost.clone())
    );

    // 4. replacing_web_extras preserves history_owner_id on ClaudeOAuthMappedUsage
    let mut mapped_with_owner = oauth_mapped;
    mapped_with_owner.history_owner_id = Some("history-owner".to_owned());
    let enriched_mapped =
        mapped_with_owner.replacing_web_extras(Vec::new(), Some(merged_usd.clone()));
    assert_eq!(enriched_mapped.snapshot.provider_cost, Some(merged_usd));
    assert_eq!(
        enriched_mapped.history_owner_id.as_deref(),
        Some("history-owner")
    );

    // 5. web_extras_account_matches: OAuth and CLI rules
    let base_snap = UsageSnapshot::new(
        RateWindow::new(7.0, Some(300), None, None, None, false).ok(),
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        now,
        Some(ProviderIdentitySnapshot {
            provider_id: ProviderId::new("claude").ok(),
            account_email: None,
            account_organization: None,
            login_method: Some("Pro".to_owned()),
            account_id: None,
        }),
        Confidence::Exact,
    )
    .unwrap();
    let web_data = WebUsageData {
        session_percent_used: Some(7.0),
        account_organization: Some("Test Org".to_owned()),
        account_organization_id: Some("org-123".to_owned()),
        account_email: Some("user@example.invalid".to_owned()),
        login_method: Some("Pro".to_owned()),
        ..WebUsageData::default()
    };

    assert!(web_extras_account_matches(
        &base_snap,
        &web_data,
        Some(&ClaudeAccountIdentity {
            account_uuid: None,
            email: Some("user@example.invalid".to_owned()),
            organization_uuid: Some("org-123".to_owned()),
            organization_name: None,
        })
    ));
    assert!(!web_extras_account_matches(
        &base_snap,
        &web_data,
        Some(&ClaudeAccountIdentity {
            account_uuid: None,
            email: Some("other@example.invalid".to_owned()),
            organization_uuid: Some("org-123".to_owned()),
            organization_name: None,
        })
    ));
    assert!(!web_extras_account_matches(
        &base_snap,
        &web_data,
        Some(&ClaudeAccountIdentity {
            account_uuid: None,
            email: Some("user@example.invalid".to_owned()),
            organization_uuid: Some("org-other".to_owned()),
            organization_name: None,
        })
    ));

    // CLI (oauth_profile == None): requires matching emails AND no org name conflict
    for primary_has_email in [false, true] {
        for web_has_email in [false, true] {
            let cli_snap = base_snap
                .clone()
                .with_identity(Some(ProviderIdentitySnapshot {
                    provider_id: ProviderId::new("claude").ok(),
                    account_email: primary_has_email.then(|| " USER@example.invalid ".to_owned()),
                    account_organization: Some("Shared Team".to_owned()),
                    login_method: Some("Pro".to_owned()),
                    account_id: None,
                }));
            let cli_web = WebUsageData {
                account_email: web_has_email.then(|| "user@example.invalid".to_owned()),
                account_organization: Some("Shared Team".to_owned()),
                account_organization_id: Some("org-123".to_owned()),
                ..WebUsageData::default()
            };
            assert_eq!(
                web_extras_account_matches(&cli_snap, &cli_web, None),
                primary_has_email && web_has_email
            );
        }
    }

    let cli_email_snap = base_snap
        .clone()
        .with_identity(Some(ProviderIdentitySnapshot {
            provider_id: ProviderId::new("claude").ok(),
            account_email: Some("user@example.invalid".to_owned()),
            account_organization: Some("Shared Team".to_owned()),
            login_method: Some("Pro".to_owned()),
            account_id: None,
        }));
    let cli_other_org_web = WebUsageData {
        account_email: Some("user@example.invalid".to_owned()),
        account_organization: Some("Other Team".to_owned()),
        account_organization_id: Some("org-123".to_owned()),
        ..WebUsageData::default()
    };
    assert!(!web_extras_account_matches(
        &cli_email_snap,
        &cli_other_org_web,
        None
    ));

    // OAuth org UUID retains identity when email is unavailable
    let no_email_web = WebUsageData {
        account_email: None,
        account_organization: Some("Shared Team".to_owned()),
        account_organization_id: Some("org-123".to_owned()),
        ..WebUsageData::default()
    };
    assert!(web_extras_account_matches(
        &base_snap,
        &no_email_web,
        Some(&ClaudeAccountIdentity {
            account_uuid: None,
            email: None,
            organization_uuid: Some("org-123".to_owned()),
            organization_name: None,
        })
    ));
    assert!(!web_extras_account_matches(
        &base_snap,
        &no_email_web,
        Some(&ClaudeAccountIdentity::default())
    ));

    // Unicode case-insensitive email/org comparison
    let unicode_web = WebUsageData {
        account_email: Some("üser@example.invalid".to_owned()),
        account_organization: Some("Équipe Team".to_owned()),
        account_organization_id: Some("org-123".to_owned()),
        ..WebUsageData::default()
    };
    assert!(web_extras_account_matches(
        &base_snap,
        &unicode_web,
        Some(&ClaudeAccountIdentity {
            account_uuid: None,
            email: Some("ÜSER@example.invalid".to_owned()),
            organization_uuid: Some("org-123".to_owned()),
            organization_name: Some("équipe team".to_owned()),
        })
    ));
}

#[test]
fn extra_rate_windows_parsing_ordering_and_merging() {
    let now = Timestamp::from_second(1_800_000_000).unwrap();

    // Ignores omelette window, keeps cowork -> claude-routines
    let omelette_json = br#"{
      "five_hour": { "utilization": 9, "resets_at": "2025-12-23T16:00:00.000Z" },
      "seven_day_omelette": { "utilization": 26, "resets_at": "2025-12-30T23:00:00.000Z" },
      "seven_day_cowork": { "utilization": 11, "resets_at": "2025-12-31T23:00:00.000Z" }
    }"#;
    let parsed_omelette = ClaudeWebAPIFetcher::parse_usage_response(omelette_json, now).unwrap();
    assert_eq!(parsed_omelette.extra_rate_windows.len(), 1);
    assert_eq!(parsed_omelette.extra_rate_windows[0].id, "claude-routines");
    assert_eq!(
        parsed_omelette.extra_rate_windows[0].window.used_percent,
        11.0
    );

    // Omits routines when cowork is null
    let null_cowork = br#"{
      "five_hour": { "utilization": 9, "resets_at": "2025-12-23T16:00:00.000Z" },
      "seven_day_omelette": { "utilization": 26, "resets_at": "2025-12-30T23:00:00.000Z" },
      "seven_day_cowork": null
    }"#;
    let parsed_null = ClaudeWebAPIFetcher::parse_usage_response(null_cowork, now).unwrap();
    assert!(parsed_null.extra_rate_windows.is_empty());

    // First routine object key stops iteration even if utilization is invalid/missing (no fallthrough to later alias)
    let first_routine_invalid = br#"{
      "five_hour": { "utilization": 9 },
      "seven_day_cowork": { "utilization": "invalid" },
      "daily_routines": { "utilization": 34 }
    }"#;
    let parsed_first_invalid =
        ClaudeWebAPIFetcher::parse_usage_response(first_routine_invalid, now).unwrap();
    assert!(parsed_first_invalid.extra_rate_windows.is_empty());

    // Filters out Fable limits with non-weekly_scoped kinds (session, weekly_all)
    let filtered_kinds_json = br#"{
      "five_hour": { "utilization": 9 },
      "limits": [
        {
          "kind": "session", "group": "weekly", "percent": 15,
          "scope": { "model": { "display_name": "Fable" } }
        },
        {
          "kind": "weekly_all", "group": "weekly", "percent": 25,
          "scope": { "model": { "display_name": "Fable" } }
        },
        {
          "kind": "weekly_scoped", "group": "weekly", "percent": 35,
          "scope": { "model": { "display_name": "Fable" } }
        }
      ]
    }"#;
    let parsed_filtered =
        ClaudeWebAPIFetcher::parse_usage_response(filtered_kinds_json, now).unwrap();
    assert_eq!(parsed_filtered.extra_rate_windows.len(), 1);
    assert_eq!(parsed_filtered.extra_rate_windows[0].title, "Fable only");
    assert_eq!(
        parsed_filtered.extra_rate_windows[0].window.used_percent,
        35.0
    );

    // Orders scoped weekly windows before daily routines
    let ordered_json = br#"{
      "five_hour": { "utilization": 9, "resets_at": "2026-07-03T00:30:00.440902+00:00" },
      "seven_day": { "utilization": 20, "resets_at": "2026-07-08T09:00:00.440924+00:00" },
      "seven_day_cowork": { "utilization": 11, "resets_at": "2026-07-08T09:00:00.440924+00:00" },
      "limits": [
        {
          "kind": "weekly_scoped", "group": "weekly", "percent": 12,
          "resets_at": "2026-07-08T09:00:00.441154+00:00",
          "scope": { "model": { "id": null, "display_name": "Opus" }, "surface": null },
          "is_active": false
        },
        {
          "kind": "weekly_scoped", "group": "weekly", "percent": 29,
          "resets_at": "2026-07-08T09:00:00.441154+00:00",
          "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null },
          "is_active": false
        }
      ]
    }"#;
    let parsed_ordered = ClaudeWebAPIFetcher::parse_usage_response(ordered_json, now).unwrap();
    let titles: Vec<&str> = parsed_ordered
        .extra_rate_windows
        .iter()
        .map(|w| w.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Opus only", "Fable only", "Daily Routines"]);

    // merge_extra_rate_windows deduplicates unambiguous scoped model and preserves ambiguous same-title
    let rw = |pct: f64| RateWindow::new(pct, Some(10080), None, None, None, false).unwrap();
    let cli_fable = NamedRateWindow::new("claude-weekly-scoped-fable", "Fable only", rw(68.0));
    let web_fable = NamedRateWindow::new(
        "claude-weekly-scoped-test-only-fable-id",
        "Fable only",
        rw(68.0),
    );
    let routines = NamedRateWindow::new("claude-routines", "Daily Routines", rw(11.0));

    let merged = merge_extra_rate_windows(&[cli_fable], &[web_fable, routines]);
    let merged_ids: Vec<&str> = merged.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(
        merged_ids,
        vec!["claude-weekly-scoped-fable", "claude-routines"]
    );

    let cli_example = NamedRateWindow::new(
        "claude-weekly-scoped-example-model",
        "Example Model only",
        rw(20.0),
    );
    let web_ambiguous = vec![
        NamedRateWindow::new(
            "claude-weekly-scoped-first-id",
            "Example Model only",
            rw(25.0),
        ),
        NamedRateWindow::new(
            "claude-weekly-scoped-second-id",
            "Example Model only",
            rw(25.0),
        ),
    ];
    let merged_ambiguous = merge_extra_rate_windows(&[cli_example], &web_ambiguous);
    let amb_ids: Vec<&str> = merged_ambiguous.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(
        amb_ids,
        vec![
            "claude-weekly-scoped-example-model",
            "claude-weekly-scoped-first-id",
            "claude-weekly-scoped-second-id",
        ]
    );
}

#[tokio::test]
async fn cloudflare_challenge_and_unauthorized_responses_classify_properly() {
    let server = https::start_server();
    let base_url = Url::parse(&server.url("")).unwrap();
    let endpoint = ClaudeWebEndpointConfig::with_base_url(base_url).unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));
    let hdr = session_hdr("sk-ant-web1");
    let options = ClaudeWebFetchOptions::default();

    // 1. 403 with cf-mitigated: challenge on /api/organizations
    let mut m1 = server.mock(|when, then| {
        when.method(GET).path("/api/organizations");
        then.status(403)
            .header("cf-mitigated", "challenge")
            .body("challenge");
    });
    let err1 = ClaudeWebAPIFetcher::fetch_usage_with_cookie_header(
        &net,
        &endpoint,
        &hdr,
        &options,
        &https::call(),
    )
    .await
    .unwrap_err();
    m1.delete();
    assert_eq!(err1, ClaudeWebFetchError::CloudflareChallenge);
    assert_eq!(err1.to_string(), CLOUDFLARE_CHALLENGE_MESSAGE);
    assert_eq!(
        err1.into_classified_error().kind,
        ProviderErrorKind::PermissionDenied
    );

    // 2. 403 with <title>Just a moment...</title> on /api/organizations/org-123/usage
    let mut m_org = server.mock(|when, then| {
        when.method(GET).path("/api/organizations");
        then.status(200)
            .body(r#"[{"uuid":"org-123","name":"Test Org","capabilities":["chat"]}]"#);
    });
    let mut m_usage_cf = server.mock(|when, then| {
        when.method(GET).path("/api/organizations/org-123/usage");
        then.status(403)
            .body("<html><title>Just a moment...</title></html>");
    });
    let err2 = ClaudeWebAPIFetcher::fetch_usage_with_cookie_header(
        &net,
        &endpoint,
        &hdr,
        &options,
        &https::call(),
    )
    .await
    .unwrap_err();
    assert_eq!(err2, ClaudeWebFetchError::CloudflareChallenge);
    // Should not retry usage without cedar_ember on Cloudflare challenge
    assert_eq!(m_usage_cf.calls(), 1);
    m_usage_cf.delete();
    m_org.delete();

    // 3. Ordinary 403 retains Unauthorized
    let mut m3 = server.mock(|when, then| {
        when.method(GET).path("/api/organizations");
        then.status(403).body("forbidden");
    });
    let err3 = ClaudeWebAPIFetcher::fetch_usage_with_cookie_header(
        &net,
        &endpoint,
        &hdr,
        &options,
        &https::call(),
    )
    .await
    .unwrap_err();
    m3.delete();
    assert_eq!(err3, ClaudeWebFetchError::Unauthorized);

    // 4. 401 with cf-mitigated: challenge still classifies as Unauthorized
    let mut m4 = server.mock(|when, then| {
        when.method(GET).path("/api/organizations");
        then.status(401)
            .header("cf-mitigated", "challenge")
            .body("<title>Just a moment...</title>");
    });
    let err4 = ClaudeWebAPIFetcher::fetch_usage_with_cookie_header(
        &net,
        &endpoint,
        &hdr,
        &options,
        &https::call(),
    )
    .await
    .unwrap_err();
    m4.delete();
    assert_eq!(err4, ClaudeWebFetchError::Unauthorized);
}

#[tokio::test]
async fn browser_importer_tries_browsers_in_default_order_and_falls_back() {
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let attempts_clone = attempts.clone();
    let importer = ClaudeWebSessionImporter::PerBrowser(Arc::new(move |browser, _call| {
        attempts_clone.lock().unwrap().push(browser);
        if browser == Browser::Firefox {
            Ok(Some(SessionKeyInfo {
                key: SecretString::new("sk-ant-ff1"),
                source_label: "Firefox".to_owned(),
                cookie_count: 1,
            }))
        } else {
            Ok(None)
        }
    }));

    let info = importer
        .import_session_key(&https::call(), false)
        .await
        .expect("import succeeds on Firefox");
    assert_eq!(info.key.expose_secret(), "sk-ant-ff1");
    assert_eq!(info.source_label, "Firefox");
    assert_eq!(
        *attempts.lock().unwrap(),
        vec![
            Browser::Chrome,
            Browser::Edge,
            Browser::Brave,
            Browser::Chromium,
            Browser::Vivaldi,
            Browser::Firefox,
        ]
    );

    // Desktop cookie mode on PerBrowser immediately returns NoDesktopSessionKeyFound without probing browsers
    attempts.lock().unwrap().clear();
    let dsk_err = importer
        .import_session_key(&https::call(), true)
        .await
        .unwrap_err();
    assert_eq!(dsk_err, ClaudeWebFetchError::NoDesktopSessionKeyFound);
    assert!(attempts.lock().unwrap().is_empty());
    let cache = CookieCache::new();
    assert!(
        !ClaudeWebAPIFetcher::has_session_key(&cache, None, &importer, true, &https::call()).await
    );
}

#[tokio::test]
async fn prepaid_credits_merges_skips_when_disabled_and_tolerates_stalled_request() {
    let server = https::start_server();
    let base_url = Url::parse(&server.url("")).unwrap();
    let endpoint = ClaudeWebEndpointConfig::with_base_url(base_url)
        .unwrap()
        .with_prepaid_credits_timeout(Duration::from_millis(30));
    let net = Arc::new(https::broker(NetConfig::default()));
    let hdr = session_hdr("sk-ant-man1");

    let _m_org = server.mock(|when, then| {
        when.method(GET).path("/api/organizations");
        then.status(200)
            .body(r#"[{"uuid":"org-123","name":"Test Org","capabilities":["chat"]}]"#);
    });
    let _m_acc = server.mock(|when, then| {
        when.method(GET).path("/api/account");
        then.status(404).body("{}");
    });

    // 1. Merges prepaid credits into extra_usage cost
    let mut m_usage = server.mock(|when, then| {
        when.method(GET).path("/api/organizations/org-123/usage");
        then.status(200).body(
            r#"{
              "five_hour": { "utilization": 11 },
              "extra_usage": {
                "is_enabled": true,
                "monthly_limit": 2000,
                "used_credits": 500,
                "currency": "USD"
              }
            }"#,
        );
    });
    let mut m_prepaid = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/prepaid/credits");
        then.status(200)
            .body(r#"{"amount":10000,"currency":"USD"}"#);
    });

    let usage = ClaudeWebAPIFetcher::fetch_usage_with_cookie_header(
        &net,
        &endpoint,
        &hdr,
        &ClaudeWebFetchOptions::default(),
        &https::call(),
    )
    .await
    .expect("fetch succeeds");
    let cost = usage.extra_usage_cost.expect("cost");
    assert_eq!(cost.used, 5.0);
    assert_eq!(cost.limit, 20.0);
    assert_eq!(cost.balance, Some(100.0));
    assert_eq!(cost.currency_code, "USD");
    m_prepaid.delete();
    m_usage.delete();

    // 2. Balance-only fetch (include_usage_details: false, include_prepaid_balance: true) skips overage_spend_limit
    let mut m_usage_simple = server.mock(|when, then| {
        when.method(GET).path("/api/organizations/org-123/usage");
        then.status(200).body(r#"{"five_hour":{"utilization":11}}"#);
    });
    let mut m_overage = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/overage_spend_limit");
        then.status(200)
            .body(r#"{"monthly_credit_limit":2000,"currency":"USD","used_credits":500,"is_enabled":true}"#);
    });
    let mut m_prepaid2 = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/prepaid/credits");
        then.status(200)
            .body(r#"{"amount":10000,"currency":"USD"}"#);
    });

    let balance_only = ClaudeWebAPIFetcher::fetch_usage_with_cookie_header(
        &net,
        &endpoint,
        &hdr,
        &ClaudeWebFetchOptions {
            include_usage_details: false,
            include_prepaid_balance: true,
            ..ClaudeWebFetchOptions::default()
        },
        &https::call(),
    )
    .await
    .expect("balance only succeeds");
    assert_eq!(m_overage.calls(), 0);
    let bal_cost = balance_only.extra_usage_cost.expect("bal cost");
    assert_eq!(bal_cost.balance, Some(100.0));
    assert_eq!(bal_cost.used, 0.0);
    assert_eq!(bal_cost.limit, 0.0);
    m_prepaid2.delete();

    // 3. Skips prepaid credits when include_prepaid_balance is false
    let mut m_prepaid_skip = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/prepaid/credits");
        then.status(200)
            .body(r#"{"amount":10000,"currency":"USD"}"#);
    });
    let no_prepaid = ClaudeWebAPIFetcher::fetch_usage_with_cookie_header(
        &net,
        &endpoint,
        &hdr,
        &ClaudeWebFetchOptions {
            include_usage_details: true,
            include_prepaid_balance: false,
            ..ClaudeWebFetchOptions::default()
        },
        &https::call(),
    )
    .await
    .expect("no prepaid succeeds");
    assert_eq!(no_prepaid.session_percent_used, Some(11.0));
    assert_eq!(m_prepaid_skip.calls(), 0);
    m_prepaid_skip.delete();
    m_overage.delete();

    // 4. Stalled prepaid credits request times out gracefully without failing usage
    let mut m_overage_404 = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/overage_spend_limit");
        then.status(404).body("{}");
    });
    let mut m_prepaid_stalled = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/prepaid/credits");
        then.status(200)
            .delay(Duration::from_millis(250))
            .body(r#"{"amount":10000,"currency":"USD"}"#);
    });
    let stalled_usage = ClaudeWebAPIFetcher::fetch_usage_with_cookie_header(
        &net,
        &endpoint,
        &hdr,
        &ClaudeWebFetchOptions::default(),
        &https::call(),
    )
    .await
    .expect("stalled prepaid still succeeds");
    assert_eq!(stalled_usage.session_percent_used, Some(11.0));
    assert_eq!(stalled_usage.extra_usage_cost, None);

    // 5. Caller cancellation during prepaid credits surfaces Cancelled instead of Ok(None)
    let cancel_token = CancellationToken::new();
    let mut cancelled_call = https::call();
    cancelled_call.cancel = cancel_token.clone();
    let cancel_trigger = cancel_token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_trigger.cancel();
    });
    let cancel_err = ClaudeWebAPIFetcher::fetch_usage_with_cookie_header(
        &net,
        &endpoint,
        &hdr,
        &ClaudeWebFetchOptions::default(),
        &cancelled_call,
    )
    .await
    .unwrap_err();
    assert_eq!(cancel_err, ClaudeWebFetchError::Cancelled);

    m_prepaid_stalled.delete();
    m_overage_404.delete();
    m_usage_simple.delete();
}

#[tokio::test]
async fn session_key_renewal_and_cookie_cache_semantics() {
    let server = https::start_server();
    let base_url = Url::parse(&server.url("")).unwrap();
    let endpoint = ClaudeWebEndpointConfig::with_base_url(base_url).unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));
    let provider_id = ProviderId::new("claude").unwrap();
    let now = Timestamp::from_second(1_800_000_000).unwrap();
    let gate = ClaudeWebBrowserFetchGate::new();

    // 1. Cached session key renews from Set-Cookie across requests and updates cache;
    //    manual fetch observes Set-Cookie in-flight but does not rewrite CookieCache.
    let cache = CookieCache::new();
    cache.store(
        &provider_id,
        None,
        &session_hdr("sk-ant-old1"),
        "Chrome",
        now,
    );

    let m_org = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations")
            .header("cookie", session_hdr("sk-ant-old1"));
        then.status(200)
            .header("Set-Cookie", set_session_hdr("sk-ant-new1"))
            .body(r#"[{"uuid":"org-123","name":"Test Org","capabilities":["chat"]}]"#);
    });
    let m_usage = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/usage")
            .header("cookie", session_hdr("sk-ant-new1"));
        then.status(200)
            .header("Set-Cookie", set_session_hdr("sk-ant-new2"))
            .body(r#"{"five_hour":{"utilization":11},"seven_day":{"utilization":22}}"#);
    });
    let m_overage = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/overage_spend_limit")
            .header("cookie", session_hdr("sk-ant-new2"));
        then.status(404).body("{}");
    });
    let m_prepaid = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/prepaid/credits")
            .header("cookie", session_hdr("sk-ant-new2"));
        then.status(200)
            .header("Set-Cookie", set_session_hdr("sk-ant-new3"))
            .body(r#"{"amount":10000,"currency":"USD"}"#);
    });
    let m_acc = server.mock(|when, then| {
        when.method(GET)
            .path("/api/account")
            .header("cookie", session_hdr("sk-ant-new3"));
        then.status(404).body("{}");
    });

    let usage = ClaudeWebAPIFetcher::fetch_usage_serialized(
        &net,
        &endpoint,
        &cache,
        &ClaudeWebSessionImporter::None,
        &gate,
        &ClaudeWebFetchOptions {
            now,
            ..ClaudeWebFetchOptions::default()
        },
        &https::call(),
    )
    .await
    .expect("cached fetch succeeds");
    assert_eq!(usage.session_percent_used, Some(11.0));
    assert_eq!(usage.weekly_percent_used, Some(22.0));
    assert_eq!(m_org.calls(), 1);
    assert_eq!(m_usage.calls(), 1);
    assert_eq!(m_overage.calls(), 1);
    assert_eq!(m_prepaid.calls(), 1);
    assert_eq!(m_acc.calls(), 1);

    let cached = cache.load(&provider_id, None).expect("cached entry");
    assert_eq!(cached.header.expose_secret(), session_hdr("sk-ant-new3"));
    assert_eq!(cached.source_label, "Chrome");

    // Manual fetch using sk-ant-old1 uses the same Set-Cookie chain but leaves CookieCache untouched
    cache.store(
        &provider_id,
        None,
        &session_hdr("sk-ant-keep"),
        "Firefox",
        now,
    );
    let manual_usage = ClaudeWebAPIFetcher::fetch_usage_with_cookie_header(
        &net,
        &endpoint,
        &session_hdr("sk-ant-old1"),
        &ClaudeWebFetchOptions {
            now,
            ..ClaudeWebFetchOptions::default()
        },
        &https::call(),
    )
    .await
    .expect("manual fetch succeeds");
    assert_eq!(manual_usage.session_percent_used, Some(11.0));
    let still_keep = cache.load(&provider_id, None).expect("still keep");
    assert_eq!(
        still_keep.header.expose_secret(),
        session_hdr("sk-ant-keep")
    );
    assert_eq!(still_keep.source_label, "Firefox");
}

#[tokio::test]
async fn renewal_can_return_to_initial_key_and_last_assignment_in_header_wins() {
    let server = https::start_server();
    let base_url = Url::parse(&server.url("")).unwrap();
    let endpoint = ClaudeWebEndpointConfig::with_base_url(base_url).unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));
    let provider_id = ProviderId::new("claude").unwrap();
    let now = Timestamp::from_second(1_800_000_000).unwrap();
    let gate = ClaudeWebBrowserFetchGate::new();
    let cache = CookieCache::new();

    cache.store(
        &provider_id,
        None,
        &session_hdr("sk-ant-init"),
        "Chrome",
        now,
    );

    // /organizations sets comma-joined Set-Cookie where last assignment is sk-ant-mid1;
    // /usage sets Set-Cookie back to sk-ant-init.
    let combined_set_cookie = format!(
        "{}, {}",
        set_session_hdr("sk-ant-ign1"),
        set_session_hdr("sk-ant-mid1")
    );
    let _m_org = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations")
            .header("cookie", session_hdr("sk-ant-init"));
        then.status(200)
            .header("Set-Cookie", combined_set_cookie.clone())
            .body(r#"[{"uuid":"org-123","name":"Test Org","capabilities":["chat"]}]"#);
    });
    let m_usage = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/usage")
            .header("cookie", session_hdr("sk-ant-mid1"));
        then.status(200)
            .header("Set-Cookie", set_session_hdr("sk-ant-init"))
            .body(r#"{"five_hour":{"utilization":11}}"#);
    });
    let m_overage = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/overage_spend_limit")
            .header("cookie", session_hdr("sk-ant-init"));
        then.status(404).body("{}");
    });
    let _m_prepaid = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/prepaid/credits")
            .header("cookie", session_hdr("sk-ant-init"));
        then.status(404).body("{}");
    });
    let _m_acc = server.mock(|when, then| {
        when.method(GET)
            .path("/api/account")
            .header("cookie", session_hdr("sk-ant-init"));
        then.status(404).body("{}");
    });

    ClaudeWebAPIFetcher::fetch_usage_serialized(
        &net,
        &endpoint,
        &cache,
        &ClaudeWebSessionImporter::None,
        &gate,
        &ClaudeWebFetchOptions {
            now,
            ..ClaudeWebFetchOptions::default()
        },
        &https::call(),
    )
    .await
    .expect("roundtrip renewal succeeds");

    assert_eq!(m_usage.calls(), 1);
    assert_eq!(m_overage.calls(), 1);
    let cached = cache.load(&provider_id, None).expect("cached");
    assert_eq!(cached.header.expose_secret(), session_hdr("sk-ant-init"));

    // Cached fetch without renewal does not overwrite an intermediate cache update
    let cache2 = CookieCache::new();
    cache2.store(
        &provider_id,
        None,
        &session_hdr("sk-ant-init"),
        "Chrome",
        now,
    );
    // Simulate a concurrent cache update after initial load by storing a newer entry before check:
    // Because the roundtrip fetch returns renewed_cookie_header() == None and persist_initial_session_key == false,
    // persist_session_key_if_needed is a no-op.
    let tracker = ClaudeWebSessionKeyRenewalTracker::new(SecretString::new("sk-ant-init"));
    assert!(tracker.renewed_cookie_header().is_none());
    let extracted =
        ClaudeWebSessionKeyRenewalTracker::extract_from_set_cookie_header(&combined_set_cookie)
            .expect("extracted");
    assert_eq!(extracted.expose_secret(), "sk-ant-mid1");
}

#[tokio::test]
async fn concurrent_cached_fetches_serialize_rotations_and_cancelled_waiter_relinquishes_gate() {
    let server = https::start_server();
    let base_url = Url::parse(&server.url("")).unwrap();
    let endpoint = ClaudeWebEndpointConfig::with_base_url(base_url).unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));
    let provider_id = ProviderId::new("claude").unwrap();
    let now = Timestamp::from_second(1_800_000_000).unwrap();
    let gate = ClaudeWebBrowserFetchGate::new();
    let cache = Arc::new(CookieCache::new());

    cache.store(
        &provider_id,
        None,
        &session_hdr("sk-ant-rot0"),
        "Chrome",
        now,
    );

    let m_org_1 = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations")
            .header("cookie", session_hdr("sk-ant-rot0"));
        then.status(200)
            .delay(Duration::from_millis(80))
            .header("Set-Cookie", set_session_hdr("sk-ant-rot1"))
            .body(r#"[{"uuid":"org-123","name":"Test Org","capabilities":["chat"]}]"#);
    });
    let m_org_2 = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations")
            .header("cookie", session_hdr("sk-ant-rot1"));
        then.status(200)
            .header("Set-Cookie", set_session_hdr("sk-ant-rot2"))
            .body(r#"[{"uuid":"org-123","name":"Test Org","capabilities":["chat"]}]"#);
    });
    let _m_usage = server.mock(|when, then| {
        when.method(GET).path("/api/organizations/org-123/usage");
        then.status(200).body(r#"{"five_hour":{"utilization":11}}"#);
    });
    let _m_overage = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/overage_spend_limit");
        then.status(404).body("{}");
    });
    let _m_prepaid = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/prepaid/credits");
        then.status(404).body("{}");
    });
    let _m_acc = server.mock(|when, then| {
        when.method(GET).path("/api/account");
        then.status(404).body("{}");
    });

    // Launch first task (holds gate for ~80ms), then a cancelled waiter, then a second task
    let net1 = net.clone();
    let ep1 = endpoint.clone();
    let cache1 = cache.clone();
    let gate1 = gate.clone();
    let t1 = tokio::spawn(async move {
        ClaudeWebAPIFetcher::fetch_usage_serialized(
            &net1,
            &ep1,
            &cache1,
            &ClaudeWebSessionImporter::None,
            &gate1,
            &ClaudeWebFetchOptions {
                now,
                ..ClaudeWebFetchOptions::default()
            },
            &https::call(),
        )
        .await
    });

    tokio::time::sleep(Duration::from_millis(15)).await;

    let cancel_waiter = CancellationToken::new();
    let mut waiter_call = https::call();
    waiter_call.cancel = cancel_waiter.clone();
    let net_w = net.clone();
    let ep_w = endpoint.clone();
    let cache_w = cache.clone();
    let gate_w = gate.clone();
    let tw = tokio::spawn(async move {
        ClaudeWebAPIFetcher::fetch_usage_serialized(
            &net_w,
            &ep_w,
            &cache_w,
            &ClaudeWebSessionImporter::None,
            &gate_w,
            &ClaudeWebFetchOptions {
                now,
                ..ClaudeWebFetchOptions::default()
            },
            &waiter_call,
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    cancel_waiter.cancel();
    let waiter_res = tw.await.unwrap();
    assert_eq!(waiter_res, Err(ClaudeWebFetchError::Cancelled));

    let net2 = net.clone();
    let ep2 = endpoint.clone();
    let cache2 = cache.clone();
    let gate2 = gate.clone();
    let t2 = tokio::spawn(async move {
        ClaudeWebAPIFetcher::fetch_usage_serialized(
            &net2,
            &ep2,
            &cache2,
            &ClaudeWebSessionImporter::None,
            &gate2,
            &ClaudeWebFetchOptions {
                now,
                ..ClaudeWebFetchOptions::default()
            },
            &https::call(),
        )
        .await
    });

    t1.await.unwrap().expect("t1 succeeds");
    t2.await.unwrap().expect("t2 succeeds");

    assert_eq!(m_org_1.calls(), 1);
    assert_eq!(m_org_2.calls(), 1);
    let final_cached = cache.load(&provider_id, None).expect("final cached");
    assert_eq!(
        final_cached.header.expose_secret(),
        session_hdr("sk-ant-rot2")
    );
}

#[tokio::test]
async fn background_recovery_and_desktop_cookie_isolation_semantics() {
    let server = https::start_server();
    let base_url = Url::parse(&server.url("")).unwrap();
    let endpoint = ClaudeWebEndpointConfig::with_base_url(base_url).unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));
    let provider_id = ProviderId::new("claude").unwrap();
    let now = Timestamp::from_second(1_800_000_000).unwrap();
    let gate = ClaudeWebBrowserFetchGate::new();

    // 1. Cloudflare challenge preserves cached cookie without browser recovery
    let cache = CookieCache::new();
    cache.store(
        &provider_id,
        None,
        &session_hdr("sk-ant-cur1"),
        "Chrome",
        now,
    );
    let mut m_cf = server.mock(|when, then| {
        when.method(GET).path("/api/organizations");
        then.status(403)
            .header("cf-mitigated", "challenge")
            .body("challenge");
    });
    let import_called = Arc::new(AtomicBool::new(false));
    let import_called_cl = import_called.clone();
    let importer = ClaudeWebSessionImporter::Custom(Arc::new(move |_| {
        import_called_cl.store(true, Ordering::SeqCst);
        Ok(SessionKeyInfo {
            key: SecretString::new("sk-ant-imp1"),
            source_label: "Firefox".to_owned(),
            cookie_count: 1,
        })
    }));

    let cf_err = ClaudeWebAPIFetcher::fetch_usage_serialized(
        &net,
        &endpoint,
        &cache,
        &importer,
        &gate,
        &ClaudeWebFetchOptions {
            now,
            ..ClaudeWebFetchOptions::default()
        },
        &https::call(),
    )
    .await
    .unwrap_err();
    m_cf.delete();
    assert_eq!(cf_err, ClaudeWebFetchError::CloudflareChallenge);
    assert!(!import_called.load(Ordering::SeqCst));
    assert_eq!(
        cache
            .load(&provider_id, None)
            .unwrap()
            .header
            .expose_secret(),
        session_hdr("sk-ant-cur1")
    );

    // 2. Stale cached cookie (401) clears cache and surfaces original Unauthorized when browser recovery finds nothing
    cache.store(
        &provider_id,
        None,
        &session_hdr("sk-ant-stl1"),
        "Chrome",
        now,
    );
    let mut m_stale_401 = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations")
            .header("cookie", session_hdr("sk-ant-stl1"));
        then.status(401).body("{}");
    });
    let auth_err = ClaudeWebAPIFetcher::fetch_usage_serialized(
        &net,
        &endpoint,
        &cache,
        &ClaudeWebSessionImporter::None,
        &gate,
        &ClaudeWebFetchOptions {
            now,
            ..ClaudeWebFetchOptions::default()
        },
        &https::call(),
    )
    .await
    .unwrap_err();
    assert_eq!(auth_err, ClaudeWebFetchError::Unauthorized);
    assert!(cache.load(&provider_id, None).is_none());

    // 3. Stale cached cookie recovers when browser cookie import succeeds and updates cache
    cache.store(
        &provider_id,
        None,
        &session_hdr("sk-ant-stl1"),
        "Chrome",
        now,
    );
    let mut m_org_imp = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations")
            .header("cookie", session_hdr("sk-ant-imp1"));
        then.status(200)
            .body(r#"[{"uuid":"org-123","name":"Test Org","capabilities":["chat"]}]"#);
    });
    let mut m_usage = server.mock(|when, then| {
        when.method(GET).path("/api/organizations/org-123/usage");
        then.status(200).body(r#"{"five_hour":{"utilization":11}}"#);
    });
    let _m_overage = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/overage_spend_limit");
        then.status(404).body("{}");
    });
    let _m_prepaid = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/prepaid/credits");
        then.status(404).body("{}");
    });
    let _m_acc = server.mock(|when, then| {
        when.method(GET).path("/api/account");
        then.status(404).body("{}");
    });

    let recovered = ClaudeWebAPIFetcher::fetch_usage_serialized(
        &net,
        &endpoint,
        &cache,
        &importer,
        &gate,
        &ClaudeWebFetchOptions {
            now,
            ..ClaudeWebFetchOptions::default()
        },
        &https::call(),
    )
    .await
    .expect("recovered");
    assert_eq!(recovered.session_percent_used, Some(11.0));
    let recovered_cache = cache.load(&provider_id, None).expect("recovered cache");
    assert_eq!(
        recovered_cache.header.expose_secret(),
        session_hdr("sk-ant-imp1")
    );
    assert_eq!(recovered_cache.source_label, "Firefox");
    m_org_imp.delete();

    // 4. Recovered session failure (503 / Cloudflare challenge) is not replaced by stale cookie 401
    cache.store(
        &provider_id,
        None,
        &session_hdr("sk-ant-stl1"),
        "Chrome",
        now,
    );
    let mut m_org_503 = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations")
            .header("cookie", session_hdr("sk-ant-imp1"));
        then.status(503).body("{}");
    });
    let err_503 = ClaudeWebAPIFetcher::fetch_usage_serialized(
        &net,
        &endpoint,
        &cache,
        &importer,
        &gate,
        &ClaudeWebFetchOptions {
            now,
            ..ClaudeWebFetchOptions::default()
        },
        &https::call(),
    )
    .await
    .unwrap_err();
    m_org_503.delete();
    assert_eq!(err_503, ClaudeWebFetchError::ServerError { status: 503 });

    cache.store(
        &provider_id,
        None,
        &session_hdr("sk-ant-stl1"),
        "Chrome",
        now,
    );
    let mut m_org_cf = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations")
            .header("cookie", session_hdr("sk-ant-imp1"));
        then.status(403)
            .header("cf-mitigated", "challenge")
            .body("challenge");
    });
    let err_cf = ClaudeWebAPIFetcher::fetch_usage_serialized(
        &net,
        &endpoint,
        &cache,
        &importer,
        &gate,
        &ClaudeWebFetchOptions {
            now,
            ..ClaudeWebFetchOptions::default()
        },
        &https::call(),
    )
    .await
    .unwrap_err();
    m_org_cf.delete();
    m_stale_401.delete();
    assert_eq!(err_cf, ClaudeWebFetchError::CloudflareChallenge);

    // 5. Desktop cookie refresh follows account switches without borrowing or replacing browser cache,
    //    and missing desktop session cannot use cached browser account
    cache.store(
        &provider_id,
        None,
        &session_hdr("sk-ant-brw1"),
        "Chrome",
        now,
    );
    for token in ["sk-ant-dsk1", "sk-ant-dsk2"] {
        let tok_string = token.to_owned();
        let desktop_importer = ClaudeWebSessionImporter::Custom(Arc::new(move |_| {
            Ok(SessionKeyInfo {
                key: SecretString::new(tok_string.clone()),
                source_label: "Claude desktop".to_owned(),
                cookie_count: 1,
            })
        }));
        let mut m_dsk = server.mock(|when, then| {
            when.method(GET)
                .path("/api/organizations")
                .header("cookie", session_hdr(token));
            then.status(200)
                .body(r#"[{"uuid":"org-123","name":"Test Org","capabilities":["chat"]}]"#);
        });
        let dsk_usage = ClaudeWebAPIFetcher::fetch_usage_serialized(
            &net,
            &endpoint,
            &cache,
            &desktop_importer,
            &gate,
            &ClaudeWebFetchOptions {
                use_desktop_cookies: true,
                now,
                ..ClaudeWebFetchOptions::default()
            },
            &https::call(),
        )
        .await
        .expect("desktop usage");
        assert_eq!(dsk_usage.session_percent_used, Some(11.0));
        m_dsk.delete();
    }
    assert_eq!(
        cache
            .load(&provider_id, None)
            .unwrap()
            .header
            .expose_secret(),
        session_hdr("sk-ant-brw1")
    );

    assert!(
        !ClaudeWebAPIFetcher::has_session_key(
            &cache,
            None,
            &ClaudeWebSessionImporter::None,
            true,
            &https::call()
        )
        .await
    );
    let missing_dsk = ClaudeWebAPIFetcher::fetch_usage_serialized(
        &net,
        &endpoint,
        &cache,
        &ClaudeWebSessionImporter::None,
        &gate,
        &ClaudeWebFetchOptions {
            use_desktop_cookies: true,
            now,
            ..ClaudeWebFetchOptions::default()
        },
        &https::call(),
    )
    .await
    .unwrap_err();
    assert_eq!(missing_dsk, ClaudeWebFetchError::NoDesktopSessionKeyFound);
    m_usage.delete();
}

struct StubCliStrategy;

impl FetchStrategy for StubCliStrategy {
    fn id(&self) -> &'static str {
        "claude.cli"
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Cli
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async { true })
    }

    fn fetch<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async {
            let now = Timestamp::from_second(1_800_000_100).unwrap();
            let snap = UsageSnapshot::new(
                RateWindow::new(20.0, Some(300), None, None, None, false).ok(),
                None,
                None,
                None,
                None,
                Vec::new(),
                None,
                None,
                now,
                None,
                Confidence::PercentOnly,
            )
            .unwrap();
            Ok(FetchResult::new(
                snap,
                "claude",
                "claude.cli",
                SourceKind::Cli,
            ))
        })
    }

    fn should_fallback(&self, _error: &FetchError, _cx: &FetchContext) -> bool {
        false
    }
}

struct WebWithCliFallbackPlan {
    web: ClaudeOAuthStrategyConfig,
}

impl ProviderPlan for WebWithCliFallbackPlan {
    fn resolve_strategies<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
        let web_cfg = self.web.clone();
        Box::pin(async move {
            vec![
                Box::new(ClaudeWebFetchStrategy::new(web_cfg)) as Box<dyn FetchStrategy>,
                Box::new(StubCliStrategy),
            ]
        })
    }
}

fn assert_fetch_error_eq(actual: FetchError, expected: FetchError) {
    match (actual, expected) {
        (FetchError::Cancelled, FetchError::Cancelled) => {}
        (FetchError::Classified(a), FetchError::Classified(b)) => {
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.message, b.message);
            assert_eq!(a.transport, b.transport);
        }
        (a, b) => panic!("FetchError mismatch: {a:?} != {b:?}"),
    }
}

#[tokio::test]
async fn web_fetch_deadline_shared_budget_fallback_cancellation_and_unsafe_timeouts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let net = Arc::new(https::broker(NetConfig::default()));
    let provider_id = ProviderId::new("claude").unwrap();

    // 1. Unsafe timeouts rejected before invoking loader
    let loader_invoked = Arc::new(AtomicBool::new(false));
    for bad_timeout in [-1.0, f64::NAN, f64::INFINITY, f64::MAX] {
        let invoked_cl = loader_invoked.clone();
        let cfg = ClaudeOAuthStrategyConfig::new(net.clone(), temp.path(), temp.path())
            .with_web_timeout_seconds(bad_timeout)
            .with_web_usage_loader(move |_| {
                invoked_cl.store(true, Ordering::SeqCst);
                Box::pin(async { Ok(WebUsageData::default()) })
            });
        let strategy = ClaudeWebFetchStrategy::new(cfg);
        let cx = FetchContext {
            provider: provider_id.clone(),
            runtime: FetchRuntime::Cli,
            interaction: FetchInteraction::UserInitiated,
            phase: FetchPhase::Regular,
            request_id: "bad-timeout".to_owned(),
            source_mode: SourceMode::Web,
            env: BTreeMap::new(),
            include_credits: false,
            selected_token_account: None,
            cancel: CancellationToken::new(),
        };
        let err = strategy.fetch(&cx).await.unwrap_err();
        assert_fetch_error_eq(
            err,
            ClaudeWebFetchStrategyError::InvalidTimeout.into_fetch_error(),
        );
    }
    assert!(!loader_invoked.load(Ordering::SeqCst));

    // 2. App Auto availability and fetch share one web deadline budget
    for avail_elapsed in [
        Duration::from_millis(119_990),
        Duration::from_secs(120),
        Duration::from_secs(240),
    ] {
        let base_instant = Instant::now();
        let current_instant = Arc::new(Mutex::new(base_instant));
        let clock_ref = current_instant.clone();
        let probe_clock = current_instant.clone();
        let loader_started = Arc::new(AtomicBool::new(false));
        let loader_started_cl = loader_started.clone();

        let cfg = ClaudeOAuthStrategyConfig::new(net.clone(), temp.path(), temp.path())
            .with_cookie_source(Some(CookieSource::Auto))
            .with_web_timeout_seconds(120.0)
            .with_deadline_now(move || *clock_ref.lock().unwrap())
            .with_web_availability_probe(move |_| {
                let pc = probe_clock.clone();
                Box::pin(async move {
                    let mut g = pc.lock().unwrap();
                    *g += avail_elapsed;
                    true
                })
            })
            .with_web_usage_loader(move |_| {
                let ls = loader_started_cl.clone();
                Box::pin(async move {
                    ls.store(true, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    Ok(WebUsageData::default())
                })
            });

        let strategy = ClaudeWebFetchStrategy::new(cfg);
        let cx = FetchContext {
            provider: provider_id.clone(),
            runtime: FetchRuntime::Daemon,
            interaction: FetchInteraction::UserInitiated,
            phase: FetchPhase::Regular,
            request_id: "shared-deadline".to_owned(),
            source_mode: SourceMode::Auto,
            env: BTreeMap::new(),
            include_credits: false,
            selected_token_account: None,
            cancel: CancellationToken::new(),
        };

        assert!(strategy.is_available(&cx).await);
        let err = strategy.fetch(&cx).await.unwrap_err();
        assert_fetch_error_eq(
            err,
            ClaudeWebFetchStrategyError::TimedOut { seconds: 120.0 }.into_fetch_error(),
        );
        if avail_elapsed >= Duration::from_secs(120) {
            assert!(
                !loader_started.load(Ordering::SeqCst),
                "exhausted shared deadline must not start usage load"
            );
        }
    }

    // 3. CLI Auto timeout cancels web and falls back to CLI; explicit Web timeout does NOT fall back
    let timed_out_cfg = ClaudeOAuthStrategyConfig::new(net.clone(), temp.path(), temp.path())
        .with_manual_cookie_header(Some(SecretString::new(session_hdr("sk-ant-web1"))))
        .with_web_timeout_seconds(0.01)
        .with_web_usage_loader(|_| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(WebUsageData::default())
            })
        });
    let plan = WebWithCliFallbackPlan {
        web: timed_out_cfg.clone(),
    };

    let cli_auto_cx = FetchContext {
        provider: provider_id.clone(),
        runtime: FetchRuntime::Cli,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "cli-auto-timeout".to_owned(),
        source_mode: SourceMode::Auto,
        env: BTreeMap::new(),
        include_credits: false,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    };
    let auto_outcome = fetch_outcome(&provider_id, &plan, &cli_auto_cx, &NoopSleeper).await;
    let res = auto_outcome.result.expect("falls back to claude.cli");
    assert_eq!(res.strategy_id, "claude.cli");
    assert_eq!(auto_outcome.attempts.len(), 2);
    assert_eq!(auto_outcome.attempts[0].strategy_id, CLAUDE_WEB_STRATEGY_ID);
    assert_eq!(auto_outcome.attempts[1].strategy_id, "claude.cli");

    let explicit_web_cx = FetchContext {
        source_mode: SourceMode::Web,
        ..cli_auto_cx.clone()
    };
    let web_outcome = fetch_outcome(&provider_id, &plan, &explicit_web_cx, &NoopSleeper).await;
    assert_fetch_error_eq(
        web_outcome.result.unwrap_err(),
        ClaudeWebFetchStrategyError::TimedOut { seconds: 0.01 }.into_fetch_error(),
    );
    assert_eq!(web_outcome.attempts.len(), 1);
    assert_eq!(web_outcome.attempts[0].strategy_id, CLAUDE_WEB_STRATEGY_ID);

    // 4. Caller cancellation during web fetch stops pipeline without falling back to CLI
    let cancel_token = CancellationToken::new();
    let cancel_trigger = cancel_token.clone();
    let cancel_cfg = ClaudeOAuthStrategyConfig::new(net.clone(), temp.path(), temp.path())
        .with_manual_cookie_header(Some(SecretString::new(session_hdr("sk-ant-web1"))))
        .with_web_timeout_seconds(60.0)
        .with_web_usage_loader(move |_| {
            let ct = cancel_trigger.clone();
            Box::pin(async move {
                ct.cancel();
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(WebUsageData::default())
            })
        });
    let cancel_plan = WebWithCliFallbackPlan { web: cancel_cfg };
    let cancel_cx = FetchContext {
        cancel: cancel_token,
        ..cli_auto_cx
    };
    let cancel_outcome = fetch_outcome(&provider_id, &cancel_plan, &cancel_cx, &NoopSleeper).await;
    assert_fetch_error_eq(cancel_outcome.result.unwrap_err(), FetchError::Cancelled);
    assert_eq!(cancel_outcome.attempts.len(), 1);
    assert_eq!(
        cancel_outcome.attempts[0].strategy_id,
        CLAUDE_WEB_STRATEGY_ID
    );

    // 5. Explicit Web and CLI Auto defer browser probe during is_available()
    let probe_called = Arc::new(AtomicBool::new(false));
    let probe_called_cl = probe_called.clone();
    let defer_cfg = ClaudeOAuthStrategyConfig::new(net.clone(), temp.path(), temp.path())
        .with_cookie_source(Some(CookieSource::Auto))
        .with_web_availability_probe(move |_| {
            let pc = probe_called_cl.clone();
            Box::pin(async move {
                pc.store(true, Ordering::SeqCst);
                false
            })
        });
    let defer_strategy = ClaudeWebFetchStrategy::new(defer_cfg);
    let explicit_web_auto_cookie_cx = FetchContext {
        provider: provider_id.clone(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "defer-explicit-web".to_owned(),
        source_mode: SourceMode::Web,
        env: BTreeMap::new(),
        include_credits: false,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    };
    assert!(
        defer_strategy
            .is_available(&explicit_web_auto_cookie_cx)
            .await
    );
    assert!(!probe_called.load(Ordering::SeqCst));

    let cli_auto_defer_cx = FetchContext {
        runtime: FetchRuntime::Cli,
        source_mode: SourceMode::Auto,
        ..explicit_web_auto_cookie_cx
    };
    assert!(defer_strategy.is_available(&cli_auto_defer_cx).await);
    assert!(!probe_called.load(Ordering::SeqCst));
}

#[test]
fn subscription_metadata_parses_dates_clears_when_empty_and_roundtrips_json() {
    let make_json = |status: &str,
                     next_at: serde_json::Value,
                     next_date: serde_json::Value,
                     end_at: serde_json::Value,
                     end_before: serde_json::Value| {
        serde_json::to_vec(&serde_json::json!({
            "status": status,
            "next_charge_at": next_at,
            "next_charge_date": next_date,
            "plan_ending_at": end_at,
            "plan_ending_before": end_before,
        }))
        .unwrap()
    };

    // 1. Renewal keeps timestamp precision for active and trialing
    for status in ["active", "trialing"] {
        let meta = ClaudeSubscriptionMetadata::parse(&make_json(
            status,
            serde_json::json!("2026-11-05T14:15:04Z"),
            serde_json::json!("2026-11-05"),
            serde_json::Value::Null,
            serde_json::Value::Null,
        ))
        .unwrap();
        let renews = meta.renews.expect("renews");
        assert_eq!(
            renews.date,
            "2026-11-05T14:15:04Z".parse::<Timestamp>().unwrap()
        );
        assert!(!renews.is_date_only);
        assert!(meta.expires.is_none());
    }

    // 2. Scheduled ending overrides renewal and preserves calendar precision; plan_ending_at takes precedence with timestamp precision
    for status in ["active", "canceled"] {
        let meta = ClaudeSubscriptionMetadata::parse(&make_json(
            status,
            serde_json::json!("2026-11-05T14:15:04Z"),
            serde_json::json!("2026-11-05"),
            serde_json::Value::Null,
            serde_json::json!("2026-11-05"),
        ))
        .unwrap();
        assert!(meta.renews.is_none());
        let exp = meta.expires.expect("expires");
        assert_eq!(
            exp.date,
            "2026-11-05T00:00:00Z".parse::<Timestamp>().unwrap()
        );
        assert!(exp.is_date_only);

        let meta_at = ClaudeSubscriptionMetadata::parse(&make_json(
            status,
            serde_json::json!("2026-11-05T14:15:04Z"),
            serde_json::json!("2026-11-05"),
            serde_json::json!("2026-11-05T18:30:00Z"),
            serde_json::json!("2026-11-05"),
        ))
        .unwrap();
        assert!(meta_at.renews.is_none());
        let exp_at = meta_at.expires.expect("expires at");
        assert_eq!(
            exp_at.date,
            "2026-11-05T18:30:00Z".parse::<Timestamp>().unwrap()
        );
        assert!(!exp_at.is_date_only);
    }

    // 3. Canceled subscription never infers renewal from stale next_charge
    let canceled = ClaudeSubscriptionMetadata::parse(&make_json(
        "canceled",
        serde_json::json!("2026-11-05T14:15:04Z"),
        serde_json::json!("2026-11-05"),
        serde_json::Value::Null,
        serde_json::Value::Null,
    ))
    .unwrap();
    assert!(canceled.renews.is_none() && canceled.expires.is_none());

    // 4. Explicit empty clears dates without replacing quota or identity
    let now = Timestamp::from_second(100).unwrap();
    let snap = UsageSnapshot::new(
        RateWindow::new(67.0, Some(300), None, None, None, false).ok(),
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        Some(Timestamp::from_second(200).unwrap()),
        now,
        Some(ProviderIdentitySnapshot {
            provider_id: ProviderId::new("claude").ok(),
            account_email: Some("fixture@example.invalid".to_owned()),
            account_organization: Some("Fixture".to_owned()),
            login_method: Some("Claude Pro".to_owned()),
            account_id: None,
        }),
        Confidence::Exact,
    )
    .unwrap();
    let cleared = canceled.applying(&snap);
    assert!(cleared.subscription_renews_at.is_none());
    assert!(cleared.subscription_expires_at.is_none());
    assert_eq!(cleared.primary.as_ref().map(|w| w.used_percent), Some(67.0));
    assert_eq!(cleared.updated_at, now);

    // 5. Invalid calendar dates and unknown/partial schemas are rejected
    for invalid_day in ["2026-02-30", "2026-2-03", "not-a-date", ""] {
        assert_eq!(
            ClaudeSubscriptionMetadata::parse(&make_json(
                "active",
                serde_json::Value::Null,
                serde_json::Value::Null,
                serde_json::Value::Null,
                serde_json::json!(invalid_day),
            )),
            Err(ClaudeSubscriptionParseError::Unrecognized)
        );
    }
    assert_eq!(
        ClaudeSubscriptionMetadata::parse(b"{}"),
        Err(ClaudeSubscriptionParseError::Unrecognized)
    );
    assert_eq!(
        ClaudeSubscriptionMetadata::parse(&make_json(
            "unknown",
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
        )),
        Err(ClaudeSubscriptionParseError::Unrecognized)
    );
    assert_eq!(
        ClaudeSubscriptionMetadata::parse(&make_json(
            "active",
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::Value::Null,
            serde_json::json!(12345),
        )),
        Err(ClaudeSubscriptionParseError::Unrecognized)
    );

    // 6. Date precision survives UsageSnapshot JSON roundtrip
    let date_only_meta = ClaudeSubscriptionMetadata::parse(&make_json(
        "active",
        serde_json::Value::Null,
        serde_json::Value::Null,
        serde_json::Value::Null,
        serde_json::json!("2026-11-05"),
    ))
    .unwrap();
    let with_meta = date_only_meta.applying(&snap);
    let encoded = serde_json::to_vec(&with_meta).unwrap();
    let roundtrip: UsageSnapshot = serde_json::from_slice(&encoded).unwrap();
    assert!(roundtrip.subscription_expires_at_is_date_only);
    assert!(
        !roundtrip
            .with_subscription_metadata(None, None, false, false)
            .subscription_expires_at_is_date_only
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BillingScenario {
    Renewal,
    Expiration,
    Empty,
    Unavailable,
    Malformed,
    ChangedAccount,
    ChangedOrganization,
    Stalled,
}

async fn run_subscription_scenario(
    scenario: BillingScenario,
    runtime: FetchRuntime,
    source_mode: SourceMode,
    cookie_source: CookieSource,
    sub_timeout: Duration,
) -> (FetchResult, usize) {
    let temp = tempfile::tempdir().expect("tempdir");
    let server = https::start_server();
    let base_url = Url::parse(&server.url("")).unwrap();
    let oauth_ep = ClaudeOAuthEndpointConfig::with_base_url(base_url.clone()).unwrap();
    let web_ep = ClaudeWebEndpointConfig::with_base_url(base_url.join("/api").unwrap()).unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));
    let now = Timestamp::from_second(1_800_000_000).unwrap();

    let org_id = "11111111-1111-4111-8111-111111111111";
    let other_org = "22222222-2222-4222-8222-222222222222";
    let billed = Arc::new(AtomicBool::new(false));
    let sub_calls = Arc::new(AtomicUsize::new(0));

    let _m_oauth_usage = server.mock(|when, then| {
        when.method(GET).path("/api/oauth/usage");
        then.status(200).body(r#"{"five_hour":{"utilization":11}}"#);
    });
    let _m_orgs = server.mock(move |when, then| {
        when.method(GET).path("/api/organizations");
        then.status(200).body(format!(
            r#"[{{"uuid":"{org_id}","name":"Fixture","capabilities":["chat"]}}]"#
        ));
    });
    let _m_web_usage = server.mock(move |when, then| {
        when.method(GET)
            .path(format!("/api/organizations/{org_id}/usage"));
        then.status(200).body(r#"{"five_hour":{"utilization":11}}"#);
    });
    let _m_overage = server.mock(move |when, then| {
        when.method(GET)
            .path(format!("/api/organizations/{org_id}/overage_spend_limit"));
        then.status(404).body("{}");
    });

    // When scenario == ChangedOrganization or ChangedAccount, the post-billing check sees a different owner
    if scenario == BillingScenario::ChangedOrganization {
        // First 2 profile calls (usage identity + pre-billing check) return org_id; 3rd returns other_org
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = call_count.clone();
        server.mock(move |when, then| {
            when.method(GET).path("/api/oauth/profile");
            then.respond_with(move |_req: &HttpMockRequest| {
                let idx = cc.fetch_add(1, Ordering::SeqCst);
                let active_org = if idx >= 2 { other_org } else { org_id };
                HttpMockResponse::builder()
                    .status(200)
                    .body(format!(
                        r#"{{"account":{{"uuid":"fixture-account","email_address":"fixture@example.invalid"}},"organization":{{"uuid":"{active_org}"}}}}"#
                    ))
                    .build()
            });
        });
    } else {
        server.mock(move |when, then| {
            when.method(GET).path("/api/oauth/profile");
            then.status(200).body(format!(
                r#"{{"account":{{"uuid":"fixture-account","email_address":"fixture@example.invalid"}},"organization":{{"uuid":"{org_id}"}}}}"#
            ));
        });
    }

    let billed_acc = billed.clone();
    server.mock(move |when, then| {
        when.method(GET).path("/api/account");
        then.respond_with(move |_req: &HttpMockRequest| {
            let is_after_bill = billed_acc.load(Ordering::SeqCst);
            let email = if is_after_bill && scenario == BillingScenario::ChangedAccount {
                "other@example.invalid"
            } else {
                "fixture@example.invalid"
            };
            let acct = if is_after_bill && scenario == BillingScenario::ChangedAccount {
                "other-account"
            } else {
                "fixture-account"
            };
            HttpMockResponse::builder()
                .status(200)
                .body(format!(
                    r#"{{"uuid":"{acct}","email_address":"{email}","memberships":[{{"organization":{{"uuid":"{org_id}","billing_type":"stripe_subscription"}}}},{{"organization":{{"uuid":"{other_org}"}}}}]}}"#
                ))
                .build()
        });
    });

    let billed_sub = billed.clone();
    let sub_calls_cl = sub_calls.clone();
    server.mock(move |when, then| {
        when.method(GET)
            .path(format!("/api/organizations/{org_id}/subscription_details"));
        then.respond_with(move |_req: &HttpMockRequest| {
            sub_calls_cl.fetch_add(1, Ordering::SeqCst);
            billed_sub.store(true, Ordering::SeqCst);
            if scenario == BillingScenario::Stalled {
                std::thread::sleep(Duration::from_millis(100));
            }
            let status = if scenario == BillingScenario::Unavailable {
                403
            } else {
                200
            };
            let end_val = if scenario == BillingScenario::Expiration {
                "\"2026-11-05\""
            } else {
                "null"
            };
            let renewal_val = if scenario == BillingScenario::Empty {
                "null"
            } else {
                "\"2026-11-05\""
            };
            let body = if scenario == BillingScenario::Malformed {
                "{}".to_owned()
            } else {
                format!(
                    r#"{{"status":"active","next_charge_at":null,"next_charge_date":{renewal_val},"plan_ending_at":null,"plan_ending_before":{end_val}}}"#
                )
            };
            HttpMockResponse::builder().status(status).body(body).build()
        });
    });

    let config = ClaudeOAuthStrategyConfig::new(net, temp.path(), temp.path())
        .with_endpoint(oauth_ep)
        .with_web_endpoint(web_ep)
        .with_cookie_source(Some(cookie_source))
        .with_manual_cookie_header(Some(SecretString::new(session_hdr("sk-ant-web1"))))
        .with_include_optional_usage(false)
        .with_subscription_metadata_timeout(sub_timeout)
        .with_clock(move || now);

    let mut env = BTreeMap::new();
    env.insert(
        CLAUDE_OAUTH_TOKEN_ENV_KEY.to_owned(),
        SecretString::new("sk-ant-oat01-1"),
    );

    let cx = FetchContext {
        provider: ProviderId::new("claude").unwrap(),
        runtime,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "sub-cli-test".to_owned(),
        source_mode,
        env,
        include_credits: false,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    };

    let res = if source_mode == SourceMode::Oauth {
        ClaudeOAuthFetchStrategy::new(config)
            .fetch(&cx)
            .await
            .expect("oauth fetch succeeds")
    } else {
        ClaudeWebFetchStrategy::new(config)
            .fetch(&cx)
            .await
            .expect("web fetch succeeds")
    };

    (res, sub_calls.load(Ordering::SeqCst))
}

#[tokio::test]
async fn subscription_metadata_enrichment_across_web_and_oauth_strategies() {
    // 1. Web and accepted OAuth credentials export the same billing JSON across CLI and Daemon
    for runtime in [FetchRuntime::Cli, FetchRuntime::Daemon] {
        for source_mode in [SourceMode::Web, SourceMode::Oauth] {
            let (res, _) = run_subscription_scenario(
                BillingScenario::Renewal,
                runtime,
                source_mode,
                CookieSource::Manual,
                Duration::from_secs(2),
            )
            .await;
            assert_eq!(
                res.usage.primary.as_ref().map(|w| w.used_percent),
                Some(11.0)
            );
            assert_eq!(
                res.usage.subscription_renews_at,
                Some("2026-11-05T00:00:00Z".parse::<Timestamp>().unwrap())
            );
            assert!(res.usage.subscription_renews_at_is_date_only);
            let json: serde_json::Value = serde_json::to_value(&res.usage).unwrap();
            assert_eq!(json["subscriptionRenewsAt"], "2026-11-05T00:00:00Z");
            assert_eq!(json["subscriptionRenewsAtIsDateOnly"], true);
            assert!(json.get("subscriptionExpiresAt").is_none());
        }
    }

    // 2. Billing failures and authority changes preserve accepted quota
    for scenario in [
        BillingScenario::Renewal,
        BillingScenario::Expiration,
        BillingScenario::Empty,
        BillingScenario::Unavailable,
        BillingScenario::Malformed,
        BillingScenario::ChangedAccount,
        BillingScenario::ChangedOrganization,
    ] {
        for source_mode in [SourceMode::Web, SourceMode::Oauth] {
            let (res, _) = run_subscription_scenario(
                scenario,
                FetchRuntime::Cli,
                source_mode,
                CookieSource::Manual,
                Duration::from_secs(2),
            )
            .await;
            assert_eq!(
                res.usage.primary.as_ref().map(|w| w.used_percent),
                Some(11.0)
            );
            let has_renewal = scenario == BillingScenario::Renewal
                || (source_mode == SourceMode::Web
                    && scenario == BillingScenario::ChangedOrganization);
            assert_eq!(res.usage.subscription_renews_at.is_some(), has_renewal);
            assert_eq!(
                res.usage.subscription_expires_at.is_some(),
                scenario == BillingScenario::Expiration
            );
        }
    }

    // 3. CookieSource::Off makes no billing request for OAuth
    let (off_res, sub_calls) = run_subscription_scenario(
        BillingScenario::Renewal,
        FetchRuntime::Cli,
        SourceMode::Oauth,
        CookieSource::Off,
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(
        off_res.usage.primary.as_ref().map(|w| w.used_percent),
        Some(11.0)
    );
    assert!(off_res.usage.subscription_renews_at.is_none());
    assert_eq!(sub_calls, 0);

    // 4. Optional deadline (Duration::ZERO) cannot discard successful quota
    let (zero_res, _) = run_subscription_scenario(
        BillingScenario::Stalled,
        FetchRuntime::Cli,
        SourceMode::Web,
        CookieSource::Manual,
        Duration::ZERO,
    )
    .await;
    assert_eq!(
        zero_res.usage.primary.as_ref().map(|w| w.used_percent),
        Some(11.0)
    );
    assert!(zero_res.usage.subscription_renews_at.is_none());
}

#[tokio::test]
async fn claude_provider_plan_web_auto_and_oauth_web_extras_end_to_end() {
    let temp = tempfile::tempdir().expect("tempdir");
    let server = https::start_server();
    let base_url = Url::parse(&server.url("")).unwrap();
    let oauth_ep = ClaudeOAuthEndpointConfig::with_base_url(base_url.clone()).unwrap();
    let web_ep = ClaudeWebEndpointConfig::with_base_url(base_url.join("/api").unwrap()).unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));
    let provider_id = ProviderId::new("claude").unwrap();
    let now = Timestamp::from_second(1_800_000_000).unwrap();

    let org_id = "11111111-1111-4111-8111-111111111111";
    let _m_oauth_usage = server.mock(|when, then| {
        when.method(GET).path("/api/oauth/usage");
        then.status(200).body(
            r#"{
              "five_hour": { "utilization": 15 },
              "extra_usage": {
                "is_enabled": true,
                "monthly_limit": 2000,
                "used_credits": 500,
                "currency": "USD"
              }
            }"#,
        );
    });
    let _m_oauth_profile = server.mock(move |when, then| {
        when.method(GET).path("/api/oauth/profile");
        then.status(200).body(format!(
            r#"{{"account":{{"uuid":"fixture-account","email_address":"fixture@example.invalid"}},"organization":{{"uuid":"{org_id}","name":"Fixture Org","rate_limit_tier":"default_claude_max_5x"}}}}"#
        ));
    });
    let _m_orgs = server.mock(move |when, then| {
        when.method(GET).path("/api/organizations");
        then.status(200).body(format!(
            r#"[{{"uuid":"{org_id}","name":"Fixture Org","capabilities":["chat"]}}]"#
        ));
    });
    let _m_web_usage = server.mock(move |when, then| {
        when.method(GET)
            .path(format!("/api/organizations/{org_id}/usage"));
        then.status(200).body(
            r#"{
              "five_hour": { "utilization": 15 },
              "seven_day_cowork": { "utilization": 33, "resets_at": "2026-07-08T09:00:00Z" }
            }"#,
        );
    });
    let _m_overage = server.mock(move |when, then| {
        when.method(GET)
            .path(format!("/api/organizations/{org_id}/overage_spend_limit"));
        then.status(404).body("{}");
    });
    let _m_prepaid = server.mock(move |when, then| {
        when.method(GET)
            .path(format!("/api/organizations/{org_id}/prepaid/credits"));
        then.status(200)
            .body(r#"{"amount":10000,"currency":"USD"}"#);
    });
    let _m_acc = server.mock(move |when, then| {
        when.method(GET).path("/api/account");
        then.status(200).body(format!(
            r#"{{"uuid":"fixture-account","email_address":"fixture@example.invalid","memberships":[{{"organization":{{"uuid":"{org_id}","name":"Fixture Org","rate_limit_tier":"default_claude_max_5x","billing_type":"stripe_subscription"}}}}]}}"#
        ));
    });
    let _m_sub = server.mock(move |when, then| {
        when.method(GET)
            .path(format!("/api/organizations/{org_id}/subscription_details"));
        then.status(404).body("{}");
    });

    let config = ClaudeOAuthStrategyConfig::new(net, temp.path(), temp.path())
        .with_endpoint(oauth_ep)
        .with_web_endpoint(web_ep)
        .with_cookie_source(Some(CookieSource::Manual))
        .with_manual_cookie_header(Some(SecretString::new(session_hdr("sk-ant-web1"))))
        .with_web_extras_enabled(true)
        .with_clock(move || now);
    let plan = ClaudeProviderPlan::new(config);

    // 1. OAuth mode with web_extras_enabled enriches extra_rate_windows (Daily Routines) and prepaid balance
    let mut env = BTreeMap::new();
    env.insert(
        CLAUDE_OAUTH_TOKEN_ENV_KEY.to_owned(),
        SecretString::new("sk-ant-oat01-1"),
    );
    let oauth_cx = FetchContext {
        provider: provider_id.clone(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "plan-oauth-extras".to_owned(),
        source_mode: SourceMode::Oauth,
        env,
        include_credits: true,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    };
    let oauth_outcome = fetch_outcome(&provider_id, &plan, &oauth_cx, &NoopSleeper).await;
    let oauth_res = oauth_outcome
        .result
        .expect("oauth with web extras succeeds");
    assert_eq!(oauth_res.strategy_id, "claude.oauth");
    let extras = oauth_res
        .usage
        .extra_rate_windows
        .expect("extra rate windows merged");
    assert_eq!(extras.len(), 1);
    assert_eq!(extras[0].id, "claude-routines");
    assert_eq!(extras[0].window.used_percent, 33.0);
    let cost = oauth_res.usage.provider_cost.expect("provider cost merged");
    assert_eq!(cost.used, 5.0);
    assert_eq!(cost.limit, 20.0);
    assert_eq!(cost.balance, Some(100.0));

    // 2. Explicit Web mode via ClaudeProviderPlan
    let web_cx = FetchContext {
        source_mode: SourceMode::Web,
        env: BTreeMap::new(),
        ..oauth_cx
    };
    let web_outcome = fetch_outcome(&provider_id, &plan, &web_cx, &NoopSleeper).await;
    let web_res = web_outcome.result.expect("web mode succeeds");
    assert_eq!(web_res.strategy_id, CLAUDE_WEB_STRATEGY_ID);
    assert_eq!(
        web_res.usage.primary.as_ref().map(|w| w.used_percent),
        Some(15.0)
    );
}

#[tokio::test]
async fn web_extras_profile_strict_failure_email_override_and_redaction_invariants() {
    assert_eq!(DEFAULT_WEB_REQUEST_TIMEOUT, Duration::from_secs(15));
    assert_eq!(DEFAULT_PREPAID_CREDITS_TIMEOUT, Duration::from_secs(2));

    // 1. Debug redaction on sensitive Claude web/account types
    let access = ClaudeWebEnrichmentAccess {
        is_available: true,
        manual_cookie_header: Some(SecretString::new(session_hdr("sk-ant-secret1"))),
    };
    assert!(!format!("{access:?}").contains("sk-ant-secret1"));

    let org_info = OrganizationInfo {
        id: "org-secret-id".to_owned(),
        name: Some("Secret Org Name".to_owned()),
    };
    assert!(!format!("{org_info:?}").contains("org-secret-id"));
    assert!(!format!("{org_info:?}").contains("Secret Org Name"));

    let acct_info = WebAccountInfo {
        email: Some("secret@example.invalid".to_owned()),
        login_method: Some("Claude Pro".to_owned()),
    };
    assert!(!format!("{acct_info:?}").contains("secret@example.invalid"));

    let web_usage = WebUsageData {
        account_organization: Some("Secret Org".to_owned()),
        account_organization_id: Some("org-secret".to_owned()),
        account_email: Some("secret@example.invalid".to_owned()),
        ..WebUsageData::default()
    };
    let web_debug = format!("{web_usage:?}");
    assert!(!web_debug.contains("Secret Org"));
    assert!(!web_debug.contains("org-secret"));
    assert!(!web_debug.contains("secret@example.invalid"));

    let signed_in = ConfigOwnershipEvidence::SignedIn {
        account_uuid: "uuid-secret-42".to_owned(),
    };
    assert!(!format!("{signed_in:?}").contains("uuid-secret-42"));

    // 2. OAuth profile HTTP failure aborts web-extras enrichment and preserves primary snapshot
    let server = https::start_server();
    let base_url = Url::parse(&server.url("")).unwrap();
    let oauth_ep = ClaudeOAuthEndpointConfig::with_base_url(base_url.clone()).unwrap();
    let web_ep = ClaudeWebEndpointConfig::with_base_url(base_url.join("/api").unwrap()).unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));
    let now = Timestamp::from_second(1_800_000_000).unwrap();

    let mut primary_cost = ProviderCostSnapshot::new(5.0, 20.0, "USD", now);
    primary_cost.period = Some("Monthly cap".to_owned());
    let base_snap = UsageSnapshot::new(
        RateWindow::new(15.0, Some(300), None, None, None, false).ok(),
        None,
        None,
        None,
        Some(primary_cost.clone()),
        Vec::new(),
        None,
        None,
        now,
        Some(ProviderIdentitySnapshot {
            provider_id: ProviderId::new("claude").ok(),
            account_email: Some("stale@example.invalid".to_owned()),
            account_organization: Some("Fixture Org".to_owned()),
            login_method: Some("Claude Max".to_owned()),
            account_id: Some("fixture-account".to_owned()),
        }),
        Confidence::Exact,
    )
    .unwrap();

    let mut m_profile_fail = server.mock(|when, then| {
        when.method(GET).path("/api/oauth/profile");
        then.status(503).body("{}");
    });
    let mut m_web_orgs = server.mock(|when, then| {
        when.method(GET).path("/api/organizations");
        then.status(200)
            .body(r#"[{"uuid":"org-123","name":"Fixture Org","capabilities":["chat"]}]"#);
    });

    let cookie_cache = CookieCache::new();
    let gate = ClaudeWebBrowserFetchGate::new();
    let token = SecretString::new("sk-ant-oat01-1");
    let hdr = session_hdr("sk-ant-web1");
    let call = https::call();

    let preserved = apply_web_extras_if_needed(
        base_snap.clone(),
        WebExtrasApplyParams {
            net: &net,
            oauth_endpoint: &oauth_ep,
            web_endpoint: &web_ep,
            cookie_cache: &cookie_cache,
            importer: &ClaudeWebSessionImporter::None,
            browser_gate: &gate,
            manual_cookie_header: Some(&hdr),
            oauth_access_token: Some(&token),
            oauth_profile: None,
            use_web_extras: true,
            include_prepaid_balance: true,
            web_extras_timeout: Some(Duration::from_secs(5)),
            options: ClaudeWebFetchOptions {
                now,
                ..ClaudeWebFetchOptions::default()
            },
            call: &call,
        },
    )
    .await
    .expect("strict profile failure preserves primary snapshot");
    assert_eq!(m_web_orgs.calls(), 0);
    assert_eq!(preserved.provider_cost, Some(primary_cost.clone()));
    assert_eq!(
        preserved
            .identity
            .as_ref()
            .and_then(|i| i.account_email.as_deref()),
        Some("stale@example.invalid")
    );
    m_profile_fail.delete();

    // 3. When web response has no extra_usage_cost, primary OAuth provider_cost is preserved,
    //    and OAuth profile email overrides existing snapshot email
    let _m_profile_ok = server.mock(|when, then| {
        when.method(GET).path("/api/oauth/profile");
        then.status(200).body(
            r#"{"account":{"uuid":"fixture-account","email_address":"fresh@example.invalid"},"organization":{"uuid":"org-123","name":"Fixture Org"}}"#,
        );
    });
    let _m_web_usage = server.mock(|when, then| {
        when.method(GET).path("/api/organizations/org-123/usage");
        then.status(200)
            .body(r#"{"five_hour":{"utilization":15},"seven_day_cowork":{"utilization":22}}"#);
    });
    let _m_overage = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/overage_spend_limit");
        then.status(404).body("{}");
    });
    let _m_prepaid = server.mock(|when, then| {
        when.method(GET)
            .path("/api/organizations/org-123/prepaid/credits");
        then.status(404).body("{}");
    });
    let _m_acc = server.mock(|when, then| {
        when.method(GET).path("/api/account");
        then.status(200).body(
            r#"{"uuid":"fixture-account","email_address":"fresh@example.invalid","memberships":[{"organization":{"uuid":"org-123","name":"Fixture Org"}}]}"#,
        );
    });

    let enriched = apply_web_extras_if_needed(
        base_snap,
        WebExtrasApplyParams {
            net: &net,
            oauth_endpoint: &oauth_ep,
            web_endpoint: &web_ep,
            cookie_cache: &cookie_cache,
            importer: &ClaudeWebSessionImporter::None,
            browser_gate: &gate,
            manual_cookie_header: Some(&hdr),
            oauth_access_token: Some(&token),
            oauth_profile: None,
            use_web_extras: true,
            include_prepaid_balance: true,
            web_extras_timeout: Some(Duration::from_secs(5)),
            options: ClaudeWebFetchOptions {
                now,
                ..ClaudeWebFetchOptions::default()
            },
            call: &call,
        },
    )
    .await
    .expect("enrichment succeeds");
    assert_eq!(enriched.provider_cost, Some(primary_cost));
    assert_eq!(
        enriched
            .identity
            .as_ref()
            .and_then(|i| i.account_email.as_deref()),
        Some("fresh@example.invalid")
    );
    m_web_orgs.delete();
}
