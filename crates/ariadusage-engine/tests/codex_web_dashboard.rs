// Ported from CodexBar Tests/CodexBarTests/OpenAIDashboardFetcherCreditsWaitTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexDashboardAuthorityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexWebDashboardStrategyAuthorityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CLIOpenAIDashboardCacheTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexOpenAIWorkspaceResolverTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/OpenAIDashboardParserTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/OpenAIDashboardBrowserCookieImporterTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/OpenAIDashboardSessionAuthorizationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/OpenAIWebRefreshGateTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/OpenAIDashboardModelsTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/OpenAIDashboardIdentityMergeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/CodexWorkspaceBalanceDashboardTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/OpenAISubscriptionMetadataTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ariadusage_core::config::CookieSource;
use ariadusage_core::model::identity::ProviderIdentitySnapshot;
use ariadusage_core::model::snapshot::UsageSnapshot;
use ariadusage_core::model::window::RateWindow;
use ariadusage_core::pipeline::{
    FetchContext, FetchInteraction, FetchPhase, FetchRuntime, FetchStrategy,
};
use ariadusage_core::providers::SourceMode;
use ariadusage_engine::brokers::browser::Browser;
use ariadusage_engine::brokers::cookie_cache::CookieCache;
use ariadusage_engine::brokers::net::NetConfig;
use ariadusage_engine::providers::codex::*;
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

#[path = "support/https.rs"]
mod https;

fn make_strategy_config(
    auth_home: Option<&Path>,
    known_owners: Vec<CodexDashboardKnownOwnerCandidate>,
    cache_store: OpenAIDashboardCacheStore,
) -> CodexStrategyConfig {
    let mut env = BTreeMap::new();
    let home = auth_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("/nonexistent-codex-home"));
    if let Some(auth_home) = auth_home {
        env.insert(
            "CODEX_HOME".to_owned(),
            auth_home.to_string_lossy().into_owned(),
        );
    }
    let net = Arc::new(https::broker(NetConfig::default()));
    CodexStrategyConfig::new(net, env, &home, &home)
        .with_cookie_source(Some(CookieSource::Auto))
        .with_dashboard_authority_known_owners(known_owners)
        .with_dashboard_cache_store(cache_store)
}

fn make_usage(email: Option<&str>) -> UsageSnapshot {
    let codex_id = ProviderId::new("codex").unwrap();
    let identity = ProviderIdentitySnapshot {
        provider_id: Some(codex_id),
        account_email: email.map(str::to_owned),
        account_organization: None,
        login_method: None,
        account_id: None,
    };
    UsageSnapshot::new(
        Some(RateWindow {
            used_percent: 10.0,
            window_minutes: Some(300),
            resets_at: Some(Timestamp::from_second(7200).unwrap()),
            reset_description: None,
            next_regen_percent: None,
            is_synthetic_placeholder: false,
        }),
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
        Timestamp::from_second(2000).unwrap(),
        Some(identity),
        Confidence::Exact,
    )
    .unwrap()
}

fn make_dashboard(
    email: &str,
    codex_credit_limit: Option<CodexCreditLimitSnapshot>,
) -> OpenAIDashboardSnapshot {
    let credit_events = vec![CreditEvent::new(
        Timestamp::from_second(1000).unwrap(),
        "codex",
        3.0,
    )];
    let daily_breakdown = OpenAIDashboardSnapshot::make_daily_breakdown(&credit_events, 30);
    OpenAIDashboardSnapshot::new(
        Some(email.to_owned()),
        None,
        Some(75.0),
        Some(RateWindow {
            used_percent: 25.0,
            window_minutes: Some(60),
            resets_at: Some(Timestamp::from_second(3600).unwrap()),
            reset_description: None,
            next_regen_percent: None,
            is_synthetic_placeholder: false,
        }),
        credit_events,
        daily_breakdown,
        Vec::new(),
        None,
        Some(RateWindow {
            used_percent: 10.0,
            window_minutes: Some(300),
            resets_at: Some(Timestamp::from_second(7200).unwrap()),
            reset_description: None,
            next_regen_percent: None,
            is_synthetic_placeholder: false,
        }),
        None,
        None,
        Some(42.0),
        None,
        None,
        codex_credit_limit,
        Some("pro".to_owned()),
        None,
        None,
        Timestamp::from_second(2000).unwrap(),
    )
}

fn make_dashboard_without_usage_limits(email: &str) -> OpenAIDashboardSnapshot {
    OpenAIDashboardSnapshot::new(
        Some(email.to_owned()),
        None,
        None,
        None,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        None,
        None,
        None,
        Some(42.0),
        None,
        None,
        None,
        Some("pro".to_owned()),
        None,
        None,
        Timestamp::from_second(2000).unwrap(),
    )
}

fn base64_url_no_pad(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        }
    }
    out
}

fn fake_jwt(email: Option<&str>, account_id: Option<&str>) -> String {
    let header = br#"{"alg":"none"}"#;
    let mut auth_claims = serde_json::Map::new();
    auth_claims.insert(
        "chatgpt_plan_type".to_owned(),
        serde_json::Value::String("pro".to_owned()),
    );
    if let Some(id) = account_id {
        auth_claims.insert(
            "chatgpt_account_id".to_owned(),
            serde_json::Value::String(id.to_owned()),
        );
    }
    let mut claims = serde_json::Map::new();
    claims.insert(
        "chatgpt_plan_type".to_owned(),
        serde_json::Value::String("pro".to_owned()),
    );
    claims.insert(
        "https://api.openai.com/auth".to_owned(),
        serde_json::Value::Object(auth_claims),
    );
    if let Some(em) = email {
        claims.insert("email".to_owned(), serde_json::Value::String(em.to_owned()));
    }
    let payload = serde_json::to_vec(&claims).unwrap();
    format!(
        "{}.{}.",
        base64_url_no_pad(header),
        base64_url_no_pad(&payload)
    )
}

fn write_codex_auth_file(home: &Path, email: Option<&str>, account_id: Option<&str>) {
    fs::create_dir_all(home).unwrap();
    let mut tokens = serde_json::Map::new();
    tokens.insert(
        "accessToken".to_owned(),
        serde_json::Value::String("access-token".to_owned()),
    );
    tokens.insert(
        "refreshToken".to_owned(),
        serde_json::Value::String("refresh-token".to_owned()),
    );
    tokens.insert(
        "idToken".to_owned(),
        serde_json::Value::String(fake_jwt(email, account_id)),
    );
    if let Some(id) = account_id {
        tokens.insert(
            "accountId".to_owned(),
            serde_json::Value::String(id.to_owned()),
        );
    }
    let mut root = serde_json::Map::new();
    root.insert("tokens".to_owned(), serde_json::Value::Object(tokens));
    let bytes = serde_json::to_vec(&root).unwrap();
    fs::write(home.join("auth.json"), bytes).unwrap();
}

// ============================================================================
// Suite 1: CodexDashboardAuthorityTests.swift
// ============================================================================

// upstream: CodexDashboardAuthorityTests.swift:5 email only wrong email returns fail closed
#[test]
fn email_only_wrong_email_returns_fail_closed() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::EmailOnly {
                normalized_email: "owner@example.com".to_owned(),
            },
            None,
            None,
            Some("other@example.com"),
            None,
            false,
            vec![CodexDashboardKnownOwnerCandidate::new(
                CodexIdentity::EmailOnly {
                    normalized_email: "owner@example.com".to_owned(),
                },
                Some("owner@example.com"),
                None,
            )],
        ),
        CodexDashboardRoutingHints::new(Some("owner@example.com"), None),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::FailClosed);
    assert_eq!(
        decision.reason,
        CodexDashboardDecisionReason::WrongEmail {
            expected: Some("owner@example.com".to_owned()),
            actual: Some("other@example.com".to_owned()),
        }
    );
}

// upstream: CodexDashboardAuthorityTests.swift:29 provider account wrong email returns fail closed
#[test]
fn provider_account_wrong_email_returns_fail_closed() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("owner@example.com"),
            None,
            Some("other@example.com"),
            None,
            false,
            vec![CodexDashboardKnownOwnerCandidate::new(
                CodexIdentity::ProviderAccount {
                    id: "acct-owner".to_owned(),
                },
                Some("owner@example.com"),
                None,
            )],
        ),
        CodexDashboardRoutingHints::new(Some("owner@example.com"), Some("stale@example.com")),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::FailClosed);
    assert_eq!(
        decision.reason,
        CodexDashboardDecisionReason::WrongEmail {
            expected: Some("owner@example.com".to_owned()),
            actual: Some("other@example.com".to_owned()),
        }
    );
}

// upstream: CodexDashboardAuthorityTests.swift:53 email only same email ambiguity returns display only
#[test]
fn email_only_same_email_ambiguity_returns_display_only() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::EmailOnly {
                normalized_email: "shared@example.com".to_owned(),
            },
            None,
            None,
            Some("shared@example.com"),
            None,
            false,
            vec![
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-alpha".to_owned(),
                    },
                    Some("shared@example.com"),
                    None,
                ),
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-beta".to_owned(),
                    },
                    Some("shared@example.com"),
                    None,
                ),
            ],
        ),
        CodexDashboardRoutingHints::new(Some("shared@example.com"), Some("shared@example.com")),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::DisplayOnly);
    assert_eq!(
        decision.reason,
        CodexDashboardDecisionReason::SameEmailAmbiguity {
            email: "shared@example.com".to_owned(),
        }
    );
    assert!(decision.allowed_effects.is_empty());
}

// upstream: CodexDashboardAuthorityTests.swift:81 provider account exact owner match returns attach
#[test]
fn provider_account_exact_owner_match_returns_attach() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("OWNER@example.com"),
            None,
            Some("owner@example.com"),
            None,
            false,
            vec![
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-owner".to_owned(),
                    },
                    Some("OWNER@example.com"),
                    None,
                ),
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-other".to_owned(),
                    },
                    Some("other@example.com"),
                    None,
                ),
            ],
        ),
        CodexDashboardRoutingHints::new(Some("route@example.com"), Some("stale@example.com")),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::Attach);
    assert_eq!(
        decision.reason,
        CodexDashboardDecisionReason::ExactProviderAccountMatch
    );
}

// upstream: CodexDashboardAuthorityTests.swift:108 provider account exact owner ignores duplicate profile isolation
#[test]
fn provider_account_exact_owner_ignores_duplicate_profile_isolation() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("owner@example.com"),
            None,
            Some("owner@example.com"),
            None,
            false,
            vec![
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-owner".to_owned(),
                    },
                    Some("owner@example.com"),
                    Some("profile-a"),
                ),
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-owner".to_owned(),
                    },
                    Some("owner@example.com"),
                    Some("profile-b"),
                ),
            ],
        ),
        CodexDashboardRoutingHints::new(Some("owner@example.com"), None),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::Attach);
    assert_eq!(
        decision.reason,
        CodexDashboardDecisionReason::ExactProviderAccountMatch
    );
}

// upstream: CodexDashboardAuthorityTests.swift:137 email only owners retain profile isolation ambiguity
#[test]
fn email_only_owners_retain_profile_isolation_ambiguity() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::EmailOnly {
                normalized_email: "shared@example.com".to_owned(),
            },
            Some("shared@example.com"),
            None,
            Some("shared@example.com"),
            None,
            false,
            vec![
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::EmailOnly {
                        normalized_email: "shared@example.com".to_owned(),
                    },
                    Some("shared@example.com"),
                    Some("profile-a"),
                ),
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::EmailOnly {
                        normalized_email: "shared@example.com".to_owned(),
                    },
                    Some("shared@example.com"),
                    Some("profile-b"),
                ),
            ],
        ),
        CodexDashboardRoutingHints::new(Some("shared@example.com"), None),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::DisplayOnly);
    assert_eq!(
        decision.reason,
        CodexDashboardDecisionReason::SameEmailAmbiguity {
            email: "shared@example.com".to_owned(),
        }
    );
}

// upstream: CodexDashboardAuthorityTests.swift:166 provider account exact owner stays display only when email has another owner
#[test]
fn provider_account_exact_owner_stays_display_only_when_email_has_another_owner() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::ProviderAccount {
                id: "acct-current".to_owned(),
            },
            Some("shared@example.com"),
            None,
            Some("shared@example.com"),
            None,
            false,
            vec![
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-current".to_owned(),
                    },
                    Some("shared@example.com"),
                    None,
                ),
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-other".to_owned(),
                    },
                    Some("shared@example.com"),
                    None,
                ),
            ],
        ),
        CodexDashboardRoutingHints::new(Some("shared@example.com"), None),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::DisplayOnly);
    assert_eq!(
        decision.reason,
        CodexDashboardDecisionReason::SameEmailAmbiguity {
            email: "shared@example.com".to_owned(),
        }
    );
}

// upstream: CodexDashboardAuthorityTests.swift:193 provider account same email ambiguity without exact match returns display only
#[test]
fn provider_account_same_email_ambiguity_without_exact_match_returns_display_only() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::ProviderAccount {
                id: "acct-current".to_owned(),
            },
            Some("shared@example.com"),
            None,
            Some("shared@example.com"),
            None,
            false,
            vec![
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-alpha".to_owned(),
                    },
                    Some("shared@example.com"),
                    None,
                ),
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-beta".to_owned(),
                    },
                    Some("shared@example.com"),
                    None,
                ),
            ],
        ),
        CodexDashboardRoutingHints::new(Some("shared@example.com"), None),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::DisplayOnly);
    assert_eq!(
        decision.reason,
        CodexDashboardDecisionReason::SameEmailAmbiguity {
            email: "shared@example.com".to_owned(),
        }
    );
}

// upstream: CodexDashboardAuthorityTests.swift:220 provider account nil scoped email with dashboard collision returns fail closed
#[test]
fn provider_account_nil_scoped_email_with_dashboard_collision_returns_fail_closed() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::ProviderAccount {
                id: "acct-current".to_owned(),
            },
            None,
            Some("shared@example.com"),
            Some("shared@example.com"),
            None,
            false,
            vec![
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-alpha".to_owned(),
                    },
                    Some("shared@example.com"),
                    None,
                ),
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-beta".to_owned(),
                    },
                    Some("shared@example.com"),
                    None,
                ),
            ],
        ),
        CodexDashboardRoutingHints::new(Some("shared@example.com"), None),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::FailClosed);
    assert_eq!(
        decision.reason,
        CodexDashboardDecisionReason::ProviderAccountMissingScopedEmail
    );
}

// upstream: CodexDashboardAuthorityTests.swift:247 unresolved trusted continuity without competing owner returns attach
#[test]
fn unresolved_trusted_continuity_without_competing_owner_returns_attach() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::Unresolved,
            None,
            Some("owner@example.com"),
            Some("owner@example.com"),
            None,
            false,
            vec![CodexDashboardKnownOwnerCandidate::new(
                CodexIdentity::ProviderAccount {
                    id: "acct-owner".to_owned(),
                },
                Some("owner@example.com"),
                None,
            )],
        ),
        CodexDashboardRoutingHints::new(Some("owner@example.com"), Some("route@example.com")),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::Attach);
    assert_eq!(
        decision.reason,
        CodexDashboardDecisionReason::TrustedContinuityNoCompetingOwner
    );
}

// upstream: CodexDashboardAuthorityTests.swift:271 unresolved trusted continuity with competing owner returns display only
#[test]
fn unresolved_trusted_continuity_with_competing_owner_returns_display_only() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::Unresolved,
            None,
            Some("shared@example.com"),
            Some("shared@example.com"),
            None,
            false,
            vec![
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-alpha".to_owned(),
                    },
                    Some("shared@example.com"),
                    None,
                ),
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-beta".to_owned(),
                    },
                    Some("shared@example.com"),
                    None,
                ),
            ],
        ),
        CodexDashboardRoutingHints::new(Some("shared@example.com"), Some("route@example.com")),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::DisplayOnly);
    assert_eq!(
        decision.reason,
        CodexDashboardDecisionReason::SameEmailAmbiguity {
            email: "shared@example.com".to_owned(),
        }
    );
}

// upstream: CodexDashboardAuthorityTests.swift:298 unresolved without trusted evidence returns fail closed
#[test]
fn unresolved_without_trusted_evidence_returns_fail_closed() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::Unresolved,
            None,
            None,
            Some("owner@example.com"),
            None,
            false,
            Vec::new(),
        ),
        CodexDashboardRoutingHints::new(Some("owner@example.com"), None),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::FailClosed);
    assert_eq!(
        decision.reason,
        CodexDashboardDecisionReason::UnresolvedWithoutTrustedEvidence
    );
}

// upstream: CodexDashboardAuthorityTests.swift:318 missing dashboard signed in email returns fail closed
#[test]
fn missing_dashboard_signed_in_email_returns_fail_closed() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("owner@example.com"),
            Some("owner@example.com"),
            None,
            None,
            false,
            vec![CodexDashboardKnownOwnerCandidate::new(
                CodexIdentity::ProviderAccount {
                    id: "acct-owner".to_owned(),
                },
                Some("owner@example.com"),
                None,
            )],
        ),
        CodexDashboardRoutingHints::new(Some("owner@example.com"), None),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::FailClosed);
    assert_eq!(
        decision.reason,
        CodexDashboardDecisionReason::MissingDashboardSignedInEmail
    );
}

// upstream: CodexDashboardAuthorityTests.swift:342 live web attach exposes usage credits guard and history effects
#[test]
fn live_web_attach_exposes_usage_credits_guard_and_history_effects() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("owner@example.com"),
            None,
            Some("owner@example.com"),
            None,
            false,
            vec![CodexDashboardKnownOwnerCandidate::new(
                CodexIdentity::ProviderAccount {
                    id: "acct-owner".to_owned(),
                },
                Some("owner@example.com"),
                None,
            )],
        ),
        CodexDashboardRoutingHints::default(),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::Attach);
    let expected = BTreeSet::from([
        CodexDashboardAllowedEffect::SubscriptionMetadataAttachment,
        CodexDashboardAllowedEffect::UsageBackfill,
        CodexDashboardAllowedEffect::CreditsAttachment,
        CodexDashboardAllowedEffect::RefreshGuardSeed,
        CodexDashboardAllowedEffect::HistoricalBackfill,
    ]);
    assert_eq!(decision.allowed_effects, expected);
    assert!(decision.cleanup.is_empty());
}

// upstream: CodexDashboardAuthorityTests.swift:373 cached dashboard attach exposes cached reuse only
#[test]
fn cached_dashboard_attach_exposes_cached_reuse_only() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::CachedDashboard,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::EmailOnly {
                normalized_email: "owner@example.com".to_owned(),
            },
            None,
            None,
            Some("owner@example.com"),
            None,
            false,
            vec![CodexDashboardKnownOwnerCandidate::new(
                CodexIdentity::ProviderAccount {
                    id: "acct-owner".to_owned(),
                },
                Some("owner@example.com"),
                None,
            )],
        ),
        CodexDashboardRoutingHints::new(Some("owner@example.com"), None),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::Attach);
    assert_eq!(
        decision.allowed_effects,
        BTreeSet::from([CodexDashboardAllowedEffect::CachedDashboardReuse])
    );
    assert!(decision.cleanup.is_empty());
}

// upstream: CodexDashboardAuthorityTests.swift:398 display only emits full cleanup set
#[test]
fn display_only_emits_full_cleanup_set() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::EmailOnly {
                normalized_email: "shared@example.com".to_owned(),
            },
            None,
            None,
            Some("shared@example.com"),
            None,
            false,
            vec![
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-alpha".to_owned(),
                    },
                    Some("shared@example.com"),
                    None,
                ),
                CodexDashboardKnownOwnerCandidate::new(
                    CodexIdentity::ProviderAccount {
                        id: "acct-beta".to_owned(),
                    },
                    Some("shared@example.com"),
                    None,
                ),
            ],
        ),
        CodexDashboardRoutingHints::default(),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::DisplayOnly);
    assert!(decision.allowed_effects.is_empty());
    assert_eq!(decision.cleanup, CodexDashboardCleanup::all_set());
}

// upstream: CodexDashboardAuthorityTests.swift:426 fail closed emits full cleanup set
#[test]
fn fail_closed_emits_full_cleanup_set() {
    let input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        CodexDashboardOwnershipProofContext::new(
            CodexIdentity::Unresolved,
            None,
            None,
            Some("owner@example.com"),
            None,
            false,
            Vec::new(),
        ),
        CodexDashboardRoutingHints::default(),
    );

    let decision = CodexDashboardAuthority::evaluate(&input);

    assert_eq!(decision.disposition, CodexDashboardDisposition::FailClosed);
    assert!(decision.allowed_effects.is_empty());
    assert_eq!(decision.cleanup, CodexDashboardCleanup::all_set());
}

// upstream: CodexDashboardAuthorityTests.swift:447 routing hints do not change evaluation result
#[test]
fn routing_hints_do_not_change_evaluation_result() {
    let proof = CodexDashboardOwnershipProofContext::new(
        CodexIdentity::ProviderAccount {
            id: "acct-owner".to_owned(),
        },
        Some("owner@example.com"),
        None,
        Some("owner@example.com"),
        None,
        false,
        vec![CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("owner@example.com"),
            None,
        )],
    );
    let base_input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        proof.clone(),
        CodexDashboardRoutingHints::new(Some("owner@example.com"), Some("owner@example.com")),
    );
    let conflicting_routing_input = CodexDashboardAuthorityInput::new(
        CodexDashboardSourceKind::LiveWeb,
        proof,
        CodexDashboardRoutingHints::new(Some("wrong@example.com"), Some("stale@example.com")),
    );

    let base_decision = CodexDashboardAuthority::evaluate(&base_input);
    let conflicting_decision = CodexDashboardAuthority::evaluate(&conflicting_routing_input);

    assert_eq!(base_decision, conflicting_decision);
}

// ============================================================================
// Suite 2: CodexWebDashboardStrategyAuthorityTests.swift
// ============================================================================

// upstream: CodexWebDashboardStrategyAuthorityTests.swift:8 direct web dashboard result preserves subscription metadata
#[test]
fn direct_web_dashboard_result_preserves_subscription_metadata() {
    let dashboard = make_dashboard("owner@example.com", None);
    let renewal = Timestamp::from_second(1_787_236_207).unwrap();
    let enriched = CodexWebDashboardStrategy::dashboard_by_merging_subscription_metadata(
        dashboard.clone(),
        &OpenAISubscriptionFetchResult::Resolved(OpenAISubscriptionMetadata {
            subscription_expires_at: None,
            subscription_expires_at_is_date_only: false,
            subscription_renews_at: Some(renewal),
            subscription_renews_at_is_date_only: false,
        }),
    );

    assert_eq!(enriched.subscription_renews_at, Some(renewal));
    assert_eq!(enriched.subscription_expires_at, None);
    assert_eq!(
        CodexWebDashboardStrategy::dashboard_by_merging_subscription_metadata(
            dashboard.clone(),
            &OpenAISubscriptionFetchResult::Unavailable,
        ),
        dashboard
    );
}

// upstream: CodexWebDashboardStrategyAuthorityTests.swift:24 web dashboard attach converts snapshot with authority attachment email
#[test]
fn web_dashboard_attach_converts_snapshot_with_authority_attachment_email() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, Some("owner@example.com"), Some("acct-owner"));

    let config = make_strategy_config(
        Some(&auth_home),
        vec![CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("owner@example.com"),
            None,
        )],
        cache_store,
    );
    let result = CodexWebDashboardStrategy::make_authorized_dashboard_result_for_testing(
        make_dashboard("owner@example.com", None),
        &config,
        Some("route@example.com"),
    )
    .unwrap();

    assert_eq!(
        result
            .usage
            .identity_for(&ProviderId::new("codex").unwrap())
            .and_then(|id| id.account_email.as_deref()),
        Some("owner@example.com")
    );
    assert_eq!(result.credits.as_ref().map(|c| c.remaining), Some(42.0));
}

// upstream: CodexWebDashboardStrategyAuthorityTests.swift:50 web dashboard attach maps monthly credits to extra usage
#[test]
fn web_dashboard_attach_maps_monthly_credits_to_extra_usage() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, Some("owner@example.com"), Some("acct-owner"));

    let config = make_strategy_config(
        Some(&auth_home),
        vec![CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("owner@example.com"),
            None,
        )],
        cache_store,
    );
    let updated_at = Timestamp::from_second(2000).unwrap();
    let resets_at = Timestamp::from_second(4000).unwrap();
    let result = CodexWebDashboardStrategy::make_authorized_dashboard_result_for_testing(
        make_dashboard(
            "owner@example.com",
            Some(CodexCreditLimitSnapshot::new_monthly(
                125.0,
                500.0,
                75.0,
                Some(resets_at),
                updated_at,
            )),
        ),
        &config,
        Some("route@example.com"),
    )
    .unwrap();

    let cost = result.usage.provider_cost.as_ref().unwrap();
    assert_eq!(cost.used, 125.0);
    assert_eq!(cost.limit, 500.0);
    assert_eq!(cost.resets_at, Some(resets_at));
    assert_eq!(cost.currency_code, CodexExtraUsageCost::CURRENCY_CODE);
}

// upstream: CodexWebDashboardStrategyAuthorityTests.swift:87 web dashboard attach preserves credits when usage limits are absent
#[test]
fn web_dashboard_attach_preserves_credits_when_usage_limits_are_absent() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, Some("owner@example.com"), Some("acct-owner"));

    let config = make_strategy_config(
        Some(&auth_home),
        vec![CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("owner@example.com"),
            None,
        )],
        cache_store,
    );
    let dashboard = make_dashboard_without_usage_limits("owner@example.com");

    let result = CodexWebDashboardStrategy::make_authorized_dashboard_result_for_testing(
        dashboard.clone(),
        &config,
        Some("route@example.com"),
    )
    .unwrap();

    assert_eq!(result.usage.primary, None);
    assert_eq!(result.usage.secondary, None);
    assert_eq!(result.usage.updated_at, dashboard.updated_at);
    assert_eq!(
        result
            .usage
            .identity
            .as_ref()
            .and_then(|i| i.account_email.as_deref()),
        Some("owner@example.com")
    );
    assert_eq!(
        result
            .usage
            .identity
            .as_ref()
            .and_then(|i| i.login_method.as_deref()),
        Some("pro")
    );
    assert_eq!(result.credits.as_ref().map(|c| c.remaining), Some(42.0));
    assert_eq!(result.dashboard, dashboard);
}

// upstream: CodexWebDashboardStrategyAuthorityTests.swift:120 web dashboard display only throws typed policy error
#[test]
fn web_dashboard_display_only_throws_typed_policy_error() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, Some("shared@example.com"), None);

    let known_owners = vec![
        CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-alpha".to_owned(),
            },
            Some("shared@example.com"),
            None,
        ),
        CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-beta".to_owned(),
            },
            Some("shared@example.com"),
            None,
        ),
    ];
    let config = make_strategy_config(Some(&auth_home), known_owners.clone(), cache_store);
    let dashboard = make_dashboard("shared@example.com", None);
    let auth = CodexIdentityResolver::load_auth_backed_codex_account(
        &config.environment,
        false,
        Some(&config.home_directory),
    );
    let expected_decision =
        CodexDashboardAuthority::evaluate(&CodexCLIDashboardAuthorityContext::make_live_web_input(
            &dashboard,
            Some(&auth),
            known_owners,
            Some("route@example.com"),
        ));

    let err = CodexWebDashboardStrategy::make_authorized_dashboard_result_for_testing(
        dashboard,
        &config,
        Some("route@example.com"),
    )
    .unwrap_err();

    assert_eq!(
        err,
        CodexWebDashboardError::Policy(CodexDashboardPolicyError::DisplayOnly(expected_decision))
    );
}

// upstream: CodexWebDashboardStrategyAuthorityTests.swift:158 web dashboard fail closed throws policy rejection
#[test]
fn web_dashboard_fail_closed_throws_policy_rejection() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, Some("owner@example.com"), Some("acct-owner"));

    let known_owners = vec![CodexDashboardKnownOwnerCandidate::new(
        CodexIdentity::ProviderAccount {
            id: "acct-other".to_owned(),
        },
        Some("owner@example.com"),
        None,
    )];
    let config = make_strategy_config(Some(&auth_home), known_owners.clone(), cache_store);
    let dashboard = make_dashboard("owner@example.com", None);
    let auth = CodexIdentityResolver::load_auth_backed_codex_account(
        &config.environment,
        false,
        Some(&config.home_directory),
    );
    let expected_decision =
        CodexDashboardAuthority::evaluate(&CodexCLIDashboardAuthorityContext::make_live_web_input(
            &dashboard,
            Some(&auth),
            known_owners,
            Some("route@example.com"),
        ));

    let err = CodexWebDashboardStrategy::make_authorized_dashboard_result_for_testing(
        dashboard,
        &config,
        Some("route@example.com"),
    )
    .unwrap_err();

    assert_eq!(
        err,
        CodexWebDashboardError::Web(OpenAIWebCodexError::PolicyRejected(expected_decision))
    );
}

// upstream: CodexWebDashboardStrategyAuthorityTests.swift:195 web dashboard wrong email throws policy rejection
#[test]
fn web_dashboard_wrong_email_throws_policy_rejection() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, Some("owner@example.com"), Some("acct-owner"));

    let known_owners = vec![CodexDashboardKnownOwnerCandidate::new(
        CodexIdentity::ProviderAccount {
            id: "acct-owner".to_owned(),
        },
        Some("owner@example.com"),
        None,
    )];
    let config = make_strategy_config(Some(&auth_home), known_owners.clone(), cache_store);
    let dashboard = make_dashboard("other@example.com", None);
    let auth = CodexIdentityResolver::load_auth_backed_codex_account(
        &config.environment,
        false,
        Some(&config.home_directory),
    );
    let expected_decision =
        CodexDashboardAuthority::evaluate(&CodexCLIDashboardAuthorityContext::make_live_web_input(
            &dashboard,
            Some(&auth),
            known_owners,
            Some("owner@example.com"),
        ));

    let err = CodexWebDashboardStrategy::make_authorized_dashboard_result_for_testing(
        dashboard,
        &config,
        Some("owner@example.com"),
    )
    .unwrap_err();

    assert_eq!(
        err,
        CodexWebDashboardError::Web(OpenAIWebCodexError::PolicyRejected(
            expected_decision.clone()
        ))
    );
    assert_eq!(
        expected_decision.reason,
        CodexDashboardDecisionReason::WrongEmail {
            expected: Some("owner@example.com".to_owned()),
            actual: Some("other@example.com".to_owned()),
        }
    );
}

// upstream: CodexWebDashboardStrategyAuthorityTests.swift:235 web dashboard provider account without scoped auth email fail closes on dashboard collision
#[test]
fn web_dashboard_provider_account_without_scoped_auth_email_fail_closes_on_dashboard_collision() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, None, Some("acct-owner"));

    let known_owners = vec![
        CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-alpha".to_owned(),
            },
            Some("shared@example.com"),
            None,
        ),
        CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-beta".to_owned(),
            },
            Some("shared@example.com"),
            None,
        ),
    ];
    let config = make_strategy_config(Some(&auth_home), known_owners.clone(), cache_store);
    let dashboard = make_dashboard("shared@example.com", None);
    let auth = CodexIdentityResolver::load_auth_backed_codex_account(
        &config.environment,
        false,
        Some(&config.home_directory),
    );
    let expected_decision =
        CodexDashboardAuthority::evaluate(&CodexCLIDashboardAuthorityContext::make_live_web_input(
            &dashboard,
            Some(&auth),
            known_owners,
            Some("shared@example.com"),
        ));

    let err = CodexWebDashboardStrategy::make_authorized_dashboard_result_for_testing(
        dashboard,
        &config,
        Some("shared@example.com"),
    )
    .unwrap_err();

    assert_eq!(
        err,
        CodexWebDashboardError::Web(OpenAIWebCodexError::PolicyRejected(
            expected_decision.clone()
        ))
    );
    assert_eq!(
        expected_decision.reason,
        CodexDashboardDecisionReason::ProviderAccountMissingScopedEmail
    );
}

// upstream: CodexWebDashboardStrategyAuthorityTests.swift:276 web dashboard attach saves cache with attached email not routing fallback
#[test]
fn web_dashboard_attach_saves_cache_with_attached_email_not_routing_fallback() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, Some("owner@example.com"), Some("acct-owner"));

    let config = make_strategy_config(
        Some(&auth_home),
        vec![CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("owner@example.com"),
            None,
        )],
        cache_store.clone(),
    );

    let _ = CodexWebDashboardStrategy::make_authorized_dashboard_result_for_testing(
        make_dashboard("owner@example.com", None),
        &config,
        Some("route@example.com"),
    )
    .unwrap();

    let cache = cache_store.load().unwrap();
    assert_eq!(cache.account_email, "owner@example.com");
    assert_ne!(cache.account_email, "route@example.com");
}

// upstream: CodexWebDashboardStrategyAuthorityTests.swift:304 web dashboard fail closed clears stale cache
#[test]
fn web_dashboard_fail_closed_clears_stale_cache() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    cache_store.save(&OpenAIDashboardCache::new(
        "stale@example.com",
        make_dashboard("stale@example.com", None),
    ));

    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, Some("owner@example.com"), Some("acct-owner"));

    let config = make_strategy_config(
        Some(&auth_home),
        vec![CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-other".to_owned(),
            },
            Some("owner@example.com"),
            None,
        )],
        cache_store.clone(),
    );

    let res = CodexWebDashboardStrategy::make_authorized_dashboard_result_for_testing(
        make_dashboard("owner@example.com", None),
        &config,
        Some("route@example.com"),
    );
    assert!(matches!(res, Err(CodexWebDashboardError::Web(_))));
    assert_eq!(cache_store.load(), None);
}

// upstream: CodexWebDashboardStrategyAuthorityTests.swift:335 web dashboard display only clears stale cache
#[test]
fn web_dashboard_display_only_clears_stale_cache() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    cache_store.save(&OpenAIDashboardCache::new(
        "stale@example.com",
        make_dashboard("stale@example.com", None),
    ));

    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, Some("shared@example.com"), None);

    let config = make_strategy_config(
        Some(&auth_home),
        vec![
            CodexDashboardKnownOwnerCandidate::new(
                CodexIdentity::ProviderAccount {
                    id: "acct-alpha".to_owned(),
                },
                Some("shared@example.com"),
                None,
            ),
            CodexDashboardKnownOwnerCandidate::new(
                CodexIdentity::ProviderAccount {
                    id: "acct-beta".to_owned(),
                },
                Some("shared@example.com"),
                None,
            ),
        ],
        cache_store.clone(),
    );

    let res = CodexWebDashboardStrategy::make_authorized_dashboard_result_for_testing(
        make_dashboard("shared@example.com", None),
        &config,
        Some("route@example.com"),
    );
    assert!(matches!(res, Err(CodexWebDashboardError::Policy(_))));
    assert_eq!(cache_store.load(), None);
}

// ============================================================================
// Suite 3: CLIOpenAIDashboardCacheTests.swift
// ============================================================================

// upstream: CLIOpenAIDashboardCacheTests.swift:8 dashboard cache requires a fixture URL in tests
#[tokio::test]
async fn dashboard_cache_requires_a_fixture_url_in_tests() {
    let fixture_path = PathBuf::from("/synthetic/codexbar/dashboard.json");
    let scoped = OpenAIDashboardCacheStore::with_file_path(fixture_path.clone());
    assert_eq!(scoped.cache_url(), Some(fixture_path.as_path()));

    let unscoped = tokio::spawn(async { OpenAIDashboardCacheStore::new().cache_url().is_none() })
        .await
        .unwrap();
    assert!(unscoped);
    assert_eq!(OpenAIDashboardCacheStore::new().cache_url(), None);
}

// upstream: CLIOpenAIDashboardCacheTests.swift:21 cached dashboard restores when authority allows cached reuse
#[test]
fn cached_dashboard_restores_when_authority_allows_cached_reuse() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, Some("owner@example.com"), Some("acct-owner"));

    let dashboard = make_dashboard("owner@example.com", None);
    cache_store.save(&OpenAIDashboardCache::new(
        "stale-route@example.com",
        dashboard.clone(),
    ));
    let config = make_strategy_config(
        Some(&auth_home),
        vec![CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("owner@example.com"),
            None,
        )],
        cache_store,
    );

    let restored = CodexWebDashboardStrategy::load_openai_dashboard_if_available(
        &make_usage(None),
        "openai-web",
        &config,
    );

    assert_eq!(restored, Some(dashboard));
}

// upstream: CLIOpenAIDashboardCacheTests.swift:51 cached dashboard returns nil on display only and clears cache
#[test]
fn cached_dashboard_returns_nil_on_display_only_and_clears_cache() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, Some("shared@example.com"), None);

    cache_store.save(&OpenAIDashboardCache::new(
        "shared@example.com",
        make_dashboard("shared@example.com", None),
    ));
    let config = make_strategy_config(
        Some(&auth_home),
        vec![
            CodexDashboardKnownOwnerCandidate::new(
                CodexIdentity::ProviderAccount {
                    id: "acct-alpha".to_owned(),
                },
                Some("shared@example.com"),
                None,
            ),
            CodexDashboardKnownOwnerCandidate::new(
                CodexIdentity::ProviderAccount {
                    id: "acct-beta".to_owned(),
                },
                Some("shared@example.com"),
                None,
            ),
        ],
        cache_store.clone(),
    );

    let restored = CodexWebDashboardStrategy::load_openai_dashboard_if_available(
        &make_usage(Some("shared@example.com")),
        "codex-cli",
        &config,
    );

    assert_eq!(restored, None);
    assert_eq!(cache_store.load(), None);
}

// upstream: CLIOpenAIDashboardCacheTests.swift:82 cached dashboard returns nil on fail closed and clears cache
#[test]
fn cached_dashboard_returns_nil_on_fail_closed_and_clears_cache() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, Some("owner@example.com"), Some("acct-owner"));

    cache_store.save(&OpenAIDashboardCache::new(
        "owner@example.com",
        make_dashboard("owner@example.com", None),
    ));
    let config = make_strategy_config(
        Some(&auth_home),
        vec![CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-other".to_owned(),
            },
            Some("owner@example.com"),
            None,
        )],
        cache_store.clone(),
    );

    let restored = CodexWebDashboardStrategy::load_openai_dashboard_if_available(
        &make_usage(Some("owner@example.com")),
        "codex-cli",
        &config,
    );

    assert_eq!(restored, None);
    assert_eq!(cache_store.load(), None);
}

// upstream: CLIOpenAIDashboardCacheTests.swift:112 cached dashboard wrong email returns nil and clears cache
#[test]
fn cached_dashboard_wrong_email_returns_nil_and_clears_cache() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, Some("owner@example.com"), Some("acct-owner"));

    cache_store.save(&OpenAIDashboardCache::new(
        "other@example.com",
        make_dashboard("other@example.com", None),
    ));
    let config = make_strategy_config(
        Some(&auth_home),
        vec![CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("owner@example.com"),
            None,
        )],
        cache_store.clone(),
    );

    let restored = CodexWebDashboardStrategy::load_openai_dashboard_if_available(
        &make_usage(Some("owner@example.com")),
        "codex-cli",
        &config,
    );

    assert_eq!(restored, None);
    assert_eq!(cache_store.load(), None);
}

// upstream: CLIOpenAIDashboardCacheTests.swift:142 cached dashboard provider account without scoped auth email fails closed
#[test]
fn cached_dashboard_provider_account_without_scoped_auth_email_fails_closed() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, None, Some("acct-owner"));

    cache_store.save(&OpenAIDashboardCache::new(
        "shared@example.com",
        make_dashboard("shared@example.com", None),
    ));
    let config = make_strategy_config(
        Some(&auth_home),
        vec![
            CodexDashboardKnownOwnerCandidate::new(
                CodexIdentity::ProviderAccount {
                    id: "acct-alpha".to_owned(),
                },
                Some("shared@example.com"),
                None,
            ),
            CodexDashboardKnownOwnerCandidate::new(
                CodexIdentity::ProviderAccount {
                    id: "acct-beta".to_owned(),
                },
                Some("shared@example.com"),
                None,
            ),
        ],
        cache_store.clone(),
    );

    let restored = CodexWebDashboardStrategy::load_openai_dashboard_if_available(
        &make_usage(Some("shared@example.com")),
        "codex-cli",
        &config,
    );

    assert_eq!(restored, None);
    assert_eq!(cache_store.load(), None);
}

// upstream: CLIOpenAIDashboardCacheTests.swift:173 cached dashboard unresolved trusted continuity with competing owner returns nil
#[test]
fn cached_dashboard_unresolved_trusted_continuity_with_competing_owner_returns_nil() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let empty_home = tmp.path().join("empty-home");
    fs::create_dir_all(&empty_home).unwrap();

    cache_store.save(&OpenAIDashboardCache::new(
        "shared@example.com",
        make_dashboard("shared@example.com", None),
    ));
    let config = make_strategy_config(
        Some(&empty_home),
        vec![
            CodexDashboardKnownOwnerCandidate::new(
                CodexIdentity::ProviderAccount {
                    id: "acct-alpha".to_owned(),
                },
                Some("shared@example.com"),
                None,
            ),
            CodexDashboardKnownOwnerCandidate::new(
                CodexIdentity::ProviderAccount {
                    id: "acct-beta".to_owned(),
                },
                Some("shared@example.com"),
                None,
            ),
        ],
        cache_store.clone(),
    );

    let restored = CodexWebDashboardStrategy::load_openai_dashboard_if_available(
        &make_usage(Some("shared@example.com")),
        "codex-cli",
        &config,
    );

    assert_eq!(restored, None);
    assert_eq!(cache_store.load(), None);
}

// upstream: CLIOpenAIDashboardCacheTests.swift:203 cached dashboard trusts codex cli usage continuity
#[test]
fn cached_dashboard_trusts_codex_cli_usage_continuity() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let empty_home = tmp.path().join("empty-home");
    fs::create_dir_all(&empty_home).unwrap();

    let dashboard = make_dashboard("owner@example.com", None);
    cache_store.save(&OpenAIDashboardCache::new(
        "stale-route@example.com",
        dashboard.clone(),
    ));
    let config = make_strategy_config(
        Some(&empty_home),
        vec![CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("owner@example.com"),
            None,
        )],
        cache_store,
    );

    let restored = CodexWebDashboardStrategy::load_openai_dashboard_if_available(
        &make_usage(Some("owner@example.com")),
        "codex-cli",
        &config,
    );

    assert_eq!(restored, Some(dashboard));
}

// upstream: CLIOpenAIDashboardCacheTests.swift:230 cached dashboard trusts oauth usage continuity
#[test]
fn cached_dashboard_trusts_oauth_usage_continuity() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let empty_home = tmp.path().join("empty-home");
    fs::create_dir_all(&empty_home).unwrap();

    let dashboard = make_dashboard("owner@example.com", None);
    cache_store.save(&OpenAIDashboardCache::new(
        "stale-route@example.com",
        dashboard.clone(),
    ));
    let config = make_strategy_config(
        Some(&empty_home),
        vec![CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("owner@example.com"),
            None,
        )],
        cache_store,
    );

    let restored = CodexWebDashboardStrategy::load_openai_dashboard_if_available(
        &make_usage(Some("owner@example.com")),
        "oauth",
        &config,
    );

    assert_eq!(restored, Some(dashboard));
}

// upstream: CLIOpenAIDashboardCacheTests.swift:257 cached dashboard does not trust open A I web usage continuity
#[test]
fn cached_dashboard_does_not_trust_open_ai_web_usage_continuity() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let empty_home = tmp.path().join("empty-home");
    fs::create_dir_all(&empty_home).unwrap();

    cache_store.save(&OpenAIDashboardCache::new(
        "owner@example.com",
        make_dashboard("owner@example.com", None),
    ));
    let config = make_strategy_config(
        Some(&empty_home),
        vec![CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-owner".to_owned(),
            },
            Some("owner@example.com"),
            None,
        )],
        cache_store.clone(),
    );

    let restored = CodexWebDashboardStrategy::load_openai_dashboard_if_available(
        &make_usage(Some("owner@example.com")),
        "openai-web",
        &config,
    );

    assert_eq!(restored, None);
    assert_eq!(cache_store.load(), None);
}

// upstream: CLIOpenAIDashboardCacheTests.swift:284 cached dashboard ignores cached account email equality when authority rejects
#[test]
fn cached_dashboard_ignores_cached_account_email_equality_when_authority_rejects() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let auth_home = tmp.path().join("codex-home");
    write_codex_auth_file(&auth_home, Some("owner@example.com"), Some("acct-owner"));

    cache_store.save(&OpenAIDashboardCache::new(
        "owner@example.com",
        make_dashboard("owner@example.com", None),
    ));
    let config = make_strategy_config(
        Some(&auth_home),
        vec![CodexDashboardKnownOwnerCandidate::new(
            CodexIdentity::ProviderAccount {
                id: "acct-other".to_owned(),
            },
            Some("owner@example.com"),
            None,
        )],
        cache_store.clone(),
    );

    let restored = CodexWebDashboardStrategy::load_openai_dashboard_if_available(
        &make_usage(Some("owner@example.com")),
        "codex-cli",
        &config,
    );

    assert_eq!(restored, None);
    assert_eq!(cache_store.load(), None);
}

// ============================================================================
// Suite 4: CodexOpenAIWorkspaceResolverTests.swift
// ============================================================================

// upstream: CodexOpenAIWorkspaceResolverTests.swift:7 resolver returns workspace identity and sends expected headers
#[tokio::test]
async fn resolver_returns_workspace_identity_and_sends_expected_headers() {
    let server = https::start_server();
    let accounts_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts")
            .header("authorization", "Bearer access-token")
            .header("chatgpt-account-id", "account-live")
            .header("user-agent", "codex-cli");
        then.status(200).body(
            r#"{
              "items": [
                { "id": "account-live", "name": "Team Alpha" }
              ]
            }"#,
        );
    });

    let credentials = CodexOAuthCredentials::new(
        "access-token",
        "refresh-token",
        None,
        Some(" account-live ".to_owned()),
        None,
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    let broker = https::broker(NetConfig::default());
    let url = format!("{}/backend-api/accounts", server.base_url());

    let identity = CodexOpenAIWorkspaceResolver::resolve_with_url_and_call(
        &broker,
        &credentials,
        &url,
        Some(&https::call()),
    )
    .await
    .unwrap();

    assert_eq!(
        identity,
        Some(CodexOpenAIWorkspaceIdentity::new(
            "account-live",
            Some("Team Alpha")
        ))
    );
    accounts_mock.assert_calls(1);
}

// upstream: CodexOpenAIWorkspaceResolverTests.swift:56 resolver default uses isolated authenticated transport
#[tokio::test]
async fn resolver_default_uses_isolated_authenticated_transport() {
    let server = https::start_server();
    let accounts_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts")
            .header("authorization", "Bearer test-a");
        then.status(200)
            .header("cache-control", "public, max-age=300")
            .body(r#"{"items":[{"id":"account-live","name":"Team Alpha"}]}"#);
    });

    let credentials = CodexOAuthCredentials::new(
        "test-a",
        "test-r",
        None,
        Some("account-live".to_owned()),
        None,
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    let broker = https::broker(NetConfig::default());
    let url = format!("{}/backend-api/accounts", server.base_url());

    let identity = CodexOpenAIWorkspaceResolver::resolve_with_url_and_call(
        &broker,
        &credentials,
        &url,
        Some(&https::call()),
    )
    .await
    .unwrap();

    assert_eq!(
        identity.as_ref().and_then(|i| i.workspace_label.as_deref()),
        Some("Team Alpha")
    );
    accounts_mock.assert_calls(1);
}

// upstream: CodexOpenAIWorkspaceResolverTests.swift:83 resolver returns personal when account name is empty
#[tokio::test]
async fn resolver_returns_personal_when_account_name_is_empty() {
    let server = https::start_server();
    let accounts_mock = server.mock(|when, then| {
        when.method("GET").path("/backend-api/accounts");
        then.status(200).body(
            r#"{
              "items": [
                { "id": "account-live", "name": "   " }
              ]
            }"#,
        );
    });

    let credentials = CodexOAuthCredentials::new(
        "access-token",
        "refresh-token",
        None,
        Some("account-live".to_owned()),
        None,
        None,
        CodexOAuthCredentialSource::CodexHome,
        false,
    );
    let broker = https::broker(NetConfig::default());
    let url = format!("{}/backend-api/accounts", server.base_url());

    let identity = CodexOpenAIWorkspaceResolver::resolve_with_url_and_call(
        &broker,
        &credentials,
        &url,
        Some(&https::call()),
    )
    .await
    .unwrap();

    assert_eq!(
        identity.as_ref().and_then(|i| i.workspace_label.as_deref()),
        Some("Personal")
    );
    accounts_mock.assert_calls(1);
}

// upstream: CodexOpenAIWorkspaceResolverTests.swift:123 workspace identity cache persists and normalizes workspace ids
#[test]
fn workspace_identity_cache_persists_and_normalizes_workspace_ids() {
    let tmp = TempDir::new().unwrap();
    let file_path = tmp.path().join("codex-openai-workspaces.json");
    let cache = CodexOpenAIWorkspaceIdentityCache::new(Some(file_path));

    cache
        .store(&CodexOpenAIWorkspaceIdentity::new(
            " Account-Live ",
            Some("Team Alpha"),
        ))
        .unwrap();

    assert_eq!(
        cache.workspace_label(Some("account-live")).as_deref(),
        Some("Team Alpha")
    );
    assert_eq!(
        cache.workspace_label(Some(" ACCOUNT-LIVE ")).as_deref(),
        Some("Team Alpha")
    );
}

// ============================================================================
// Suite 5: OpenAIDashboardParserTests.swift
// ============================================================================

// upstream: OpenAIDashboardParserTests.swift:7 parses signed in email from client bootstrap HTML
#[test]
fn parses_signed_in_email_from_client_bootstrap_html() {
    let html = r#"
    <html>
    <head></head>
    <body>
    <script type="application/json" id="client-bootstrap">
    {"authStatus":"logged_in","session":{"user":{"email":"studpete@gmail.com"}}}
    </script>
    </body>
    </html>
    "#;
    assert_eq!(
        OpenAIDashboardParser::parse_signed_in_email_from_client_bootstrap(html).as_deref(),
        Some("studpete@gmail.com")
    );
    assert_eq!(
        OpenAIDashboardParser::parse_auth_status_from_client_bootstrap(html).as_deref(),
        Some("logged_in")
    );
}

// upstream: OpenAIDashboardParserTests.swift:23 parses code review remaining percent inline
#[test]
fn parses_code_review_remaining_percent_inline() {
    let body = "Balance\nCode review 42% remaining\nCredits remaining 291";
    assert_eq!(
        OpenAIDashboardParser::parse_code_review_remaining_percent(body),
        Some(42.0)
    );
}

// upstream: OpenAIDashboardParserTests.swift:29 parses code review remaining percent multiline
#[test]
fn parses_code_review_remaining_percent_multiline() {
    let body = "Balance\nCode review\n100% remaining\nWeekly usage limit\n0% remaining";
    assert_eq!(
        OpenAIDashboardParser::parse_code_review_remaining_percent(body),
        Some(100.0)
    );
}

// upstream: OpenAIDashboardParserTests.swift:35 parses code review limit with reset
#[test]
fn parses_code_review_limit_with_reset() {
    let body = "Balance\nCode review\n42% remaining\nResets tomorrow at 2:15 PM";
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let limit = OpenAIDashboardParser::parse_code_review_limit(body, now).unwrap();
    assert!((limit.used_percent - 58.0).abs() < 0.001);
    assert!(
        limit
            .reset_description
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("resets")
    );
}

// upstream: OpenAIDashboardParserTests.swift:48 parses core review limit with reset
#[test]
fn parses_core_review_limit_with_reset() {
    let body = "Balance\nCore review\n42% remaining\nResets tomorrow at 2:15 PM";
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let limit = OpenAIDashboardParser::parse_code_review_limit(body, now).unwrap();
    assert!((limit.used_percent - 58.0).abs() < 0.001);
    assert!(
        limit
            .reset_description
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("resets")
    );
}

// upstream: OpenAIDashboardParserTests.swift:61 parses credits remaining
#[test]
fn parses_credits_remaining() {
    let body = "Balance\nCredits remaining 1,234.56\nUsage";
    let value = OpenAIDashboardParser::parse_credits_remaining(body).unwrap();
    assert!((value - 1234.56).abs() < 0.001);
}

// upstream: OpenAIDashboardParserTests.swift:68 parses rate limits
#[test]
fn parses_rate_limits() {
    let body = "Usage limits\n5h limit\n72% remaining\nResets today at 2:15 PM\nWeekly limit\n41% remaining\nResets Fri at 9:00 AM";
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let (primary, secondary) = OpenAIDashboardParser::parse_rate_limits(body, now);
    let primary = primary.unwrap();
    let secondary = secondary.unwrap();
    assert!((primary.used_percent - 28.0).abs() < 0.001);
    assert_eq!(primary.window_minutes, Some(300));
    assert!(
        primary
            .reset_description
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("resets")
    );
    assert!((secondary.used_percent - 59.0).abs() < 0.001);
    assert_eq!(secondary.window_minutes, Some(10080));
}

// upstream: OpenAIDashboardParserTests.swift:87 parses spaced five hour limit label
#[test]
fn parses_spaced_five_hour_limit_label() {
    let body = concat!("Lim", "ite 5 h\n72 % restant");
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let (primary, _) = OpenAIDashboardParser::parse_rate_limits(body, now);
    let primary = primary.unwrap();
    assert!((primary.used_percent - 28.0).abs() < 0.001);
    assert_eq!(primary.window_minutes, Some(300));
}

// upstream: OpenAIDashboardParserTests.swift:98 parses plan from client bootstrap
#[test]
fn parses_plan_from_client_bootstrap() {
    let html = r#"
    <html>
    <body>
    <script type="application/json" id="client-bootstrap">
    {"session":{"user":{"email":"user@example.com"}},"planType":"plus"}
    </script>
    </body>
    </html>
    "#;
    assert_eq!(
        OpenAIDashboardParser::parse_plan_from_html(html).as_deref(),
        Some("Plus")
    );
}

// upstream: OpenAIDashboardParserTests.swift:112 parses prolite plan from client bootstrap
#[test]
fn parses_prolite_plan_from_client_bootstrap() {
    let html = r#"
    <html>
    <body>
    <script type="application/json" id="client-bootstrap">
    {"session":{"user":{"email":"user@example.com"}},"planType":"prolite"}
    </script>
    </body>
    </html>
    "#;
    assert_eq!(
        OpenAIDashboardParser::parse_plan_from_html(html).as_deref(),
        Some("Pro 5x")
    );
}

// upstream: OpenAIDashboardParserTests.swift:126 parses credit events from table rows
#[test]
fn parses_credit_events_from_table_rows() {
    let rows = [
        [
            "Dec 18, 2025".to_owned(),
            "CLI".to_owned(),
            "397.205 credits".to_owned(),
        ],
        [
            "Dec 17, 2025".to_owned(),
            "GitHub Code Review".to_owned(),
            "506.235 credits".to_owned(),
        ],
    ];
    let events = OpenAIDashboardParser::parse_credit_events(&rows);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].service, "CLI");
    assert!((events[0].credits_used - 397.205).abs() < 0.0001);
    assert_eq!(events[1].service, "GitHub Code Review");
    assert!((events[1].credits_used - 506.235).abs() < 0.0001);
}

// upstream: OpenAIDashboardParserTests.swift:140 parses credit event amount with localized credit label
#[test]
fn parses_credit_event_amount_with_localized_credit_label() {
    let rows = [[
        "Dec 18, 2025".to_owned(),
        "CLI".to_owned(),
        "397,205 crédits".to_owned(),
    ]];
    let events = OpenAIDashboardParser::parse_credit_events(&rows);
    assert_eq!(events.len(), 1);
    assert!((events[0].credits_used - 397.205).abs() < 0.0001);
}

// upstream: OpenAIDashboardParserTests.swift:150 parses credit event amount with english comma thousands
#[test]
fn parses_credit_event_amount_with_english_comma_thousands() {
    let rows = [[
        "Dec 18, 2025".to_owned(),
        "CLI".to_owned(),
        "1,234 credits".to_owned(),
    ]];
    let events = OpenAIDashboardParser::parse_credit_events(&rows);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].credits_used, 1234.0);
}

// upstream: OpenAIDashboardParserTests.swift:160 builds daily breakdown from events
#[test]
fn builds_daily_breakdown_from_events() {
    let dec18: Timestamp = "2025-12-18T00:00:00Z".parse().unwrap();
    let dec17: Timestamp = "2025-12-17T00:00:00Z".parse().unwrap();
    let events = vec![
        CreditEvent::new(dec18, "CLI", 10.0),
        CreditEvent::new(dec18, "CLI", 5.0),
        CreditEvent::new(dec18, "GitHub Code Review", 2.0),
        CreditEvent::new(dec17, "CLI", 1.0),
    ];

    let breakdown = OpenAIDashboardSnapshot::make_daily_breakdown(&events, 30);
    assert_eq!(breakdown.len(), 2);
    assert_eq!(breakdown[0].services[0].service, "CLI");
    assert!((breakdown[0].services[0].credits_used - 15.0).abs() < 0.0001);
}

// upstream: OpenAIDashboardParserTests.swift:188 decodes snapshot without usage breakdown field
#[test]
fn decodes_snapshot_without_usage_breakdown_field() {
    let json = r#"{
      "signedInEmail": "user@example.com",
      "codeReviewRemainingPercent": 42,
      "creditEvents": [],
      "dailyBreakdown": [],
      "updatedAt": "2025-12-18T00:00:00Z"
    }"#;
    let snapshot: OpenAIDashboardSnapshot = serde_json::from_str(json).unwrap();
    assert!(snapshot.usage_breakdown.is_empty());
}

// upstream: OpenAIDashboardParserTests.swift:205 weekly only dashboard usage projects into secondary slot
#[test]
fn weekly_only_dashboard_usage_projects_into_secondary_slot() {
    let snapshot = OpenAIDashboardSnapshot::new(
        Some("user@example.com".to_owned()),
        None,
        None,
        None,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        Some(RateWindow {
            used_percent: 25.0,
            window_minutes: Some(10080),
            resets_at: None,
            reset_description: None,
            next_regen_percent: None,
            is_synthetic_placeholder: false,
        }),
        None,
        None,
        None,
        None,
        None,
        None,
        Some("pro".to_owned()),
        None,
        None,
        Timestamp::from_second(1_700_000_000).unwrap(),
    );

    let codex_id = ProviderId::new("codex").unwrap();
    let usage = snapshot
        .to_usage_snapshot(codex_id.clone(), None, None)
        .unwrap();

    assert_eq!(usage.primary, None);
    assert_eq!(usage.secondary.as_ref().map(|w| w.used_percent), Some(25.0));
    assert_eq!(
        usage.secondary.as_ref().and_then(|w| w.window_minutes),
        Some(10080)
    );
    assert_eq!(
        usage.identity.as_ref().and_then(|i| i.provider_id.as_ref()),
        Some(&codex_id)
    );
    assert_eq!(
        usage
            .identity
            .as_ref()
            .and_then(|i| i.account_email.as_deref()),
        Some("user@example.com")
    );
}

// upstream: OpenAIDashboardParserTests.swift:233 dashboard usage projection returns nil when all limits are absent
#[test]
fn dashboard_usage_projection_returns_nil_when_all_limits_are_absent() {
    let snapshot = OpenAIDashboardSnapshot::new(
        Some("user@example.com".to_owned()),
        None,
        None,
        None,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("pro".to_owned()),
        None,
        None,
        Timestamp::from_second(1_700_000_000).unwrap(),
    );

    let codex_id = ProviderId::new("codex").unwrap();
    assert_eq!(snapshot.to_usage_snapshot(codex_id, None, None), None);
}

// ============================================================================
// Suite 6: OpenAIDashboardBrowserCookieImporterTests.swift
// ============================================================================

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:50 profile denial names exact running component
#[test]
fn profile_denial_names_exact_running_component() {
    let hint = OpenAIDashboardBrowserCookieImporter::browser_profile_access_hint(
        Browser::Chrome,
        true,
        "CodexBarCLI",
        Some("/Applications/CodexBar.app/Contents/Helpers/CodexBarCLI"),
    );
    assert!(hint.contains("macOS denied Chrome profile access"));
    assert!(hint.contains("CodexBarCLI (/Applications/CodexBar.app/Contents/Helpers/CodexBarCLI)"));
    assert!(hint.contains("Full Disk Access"));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:63 profile denial names app bundle for menu refresh
#[test]
fn profile_denial_names_app_bundle_for_menu_refresh() {
    let hint = OpenAIDashboardBrowserCookieImporter::browser_profile_access_hint(
        Browser::Chrome,
        true,
        "CodexBar",
        Some("/Applications/CodexBar.app/Contents/MacOS/CodexBar"),
    );
    assert!(hint.contains("CodexBar.app (/Applications/CodexBar.app)"));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:74 browser cookie timeout remains distinct from permission denial
#[test]
fn browser_cookie_timeout_remains_distinct_from_permission_denial() {
    let error = OpenAIDashboardBrowserCookieImporter::browser_cookie_load_timeout_error(
        Browser::Chrome,
        "CodexBarCLI",
        Some("/Applications/CodexBar.app/Contents/Helpers/CodexBarCLI"),
    );
    assert!(matches!(
        error,
        OpenAIDashboardCookieImportError::BrowserCookieLoadTimedOut { .. }
    ));
    let message = error.to_string();
    assert!(message.contains("Chrome did not finish before the web timeout"));
    assert!(!message.contains("access denied"));
    assert!(message.contains("CodexBarCLI"));
    assert!(message.contains("Keychain prompt"));
    assert!(message.contains("Full Disk Access"));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:93 shared deadline clamps each local timeout to remaining budget
#[test]
fn shared_deadline_clamps_each_local_timeout_to_remaining_budget() {
    // Instant replaces Date(referenceDate: 1000); only relative durations matter.
    let start = Instant::now();
    let remaining = OpenAIDashboardBrowserCookieImporter::remaining_timeout(
        Some(start + Duration::from_secs(30)),
        Some(Duration::from_secs(10)),
        start + Duration::from_secs(27),
    )
    .unwrap();
    assert_eq!(remaining, Duration::from_secs(3));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:106 shared deadline preserves smaller local timeout
#[test]
fn shared_deadline_preserves_smaller_local_timeout() {
    let start = Instant::now();
    let remaining = OpenAIDashboardBrowserCookieImporter::remaining_timeout(
        Some(start + Duration::from_secs(30)),
        Some(Duration::from_secs(10)),
        start + Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(remaining, Duration::from_secs(10));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:119 expired shared deadline throws structured timeout
#[test]
fn expired_shared_deadline_throws_structured_timeout() {
    let deadline = Instant::now();
    assert!(matches!(
        OpenAIDashboardBrowserCookieImporter::remaining_timeout(Some(deadline), None, deadline),
        Err(OpenAIDashboardCookieImportError::BrowserCookieLoadTimedOut { .. })
    ));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:135 blocking browser cookie load cannot exceed shared deadline
#[tokio::test]
async fn blocking_browser_cookie_load_cannot_exceed_shared_deadline() {
    let timeout_probe = Arc::new(AtomicBool::new(false));
    let observer = timeout_probe.clone();
    let (release, released) = std::sync::mpsc::channel();
    let (finished, completion) = tokio::sync::oneshot::channel();
    let result = OpenAIDashboardBrowserCookieImporter::run_bounded_cookie_load(
        Some(Instant::now() + Duration::from_secs_f64(0.05)),
        Some(Arc::new(move || {
            observer.store(true, Ordering::SeqCst);
        })),
        move || {
            let result = released.recv_timeout(Duration::from_secs(60));
            let _ = finished.send(());
            assert!(result.is_ok());
            Ok(true)
        },
    )
    .await;
    // Release even on a failing assertion so the blocking worker cannot linger.
    release.send(()).unwrap();
    assert_eq!(result, Err(PersistentCookieSessionError::TimedOut));
    assert!(timeout_probe.load(Ordering::SeqCst));
    tokio::time::timeout(Duration::from_secs(30), completion)
        .await
        .unwrap()
        .unwrap();
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:167 timeout observer stays silent when operation wins
#[tokio::test]
async fn timeout_observer_stays_silent_when_operation_wins() {
    let probe = Arc::new(AtomicBool::new(false));
    let observer = probe.clone();
    let value = OpenAIDashboardBrowserCookieImporter::run_bounded_cookie_load(
        Some(Instant::now() + Duration::from_secs_f64(0.05)),
        Some(Arc::new(move || {
            observer.store(true, Ordering::SeqCst);
        })),
        || Ok(true),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(value);
    assert!(!probe.load(Ordering::SeqCst));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:208 timed out cookie cache work stays ordered before retry
#[tokio::test]
async fn timed_out_cookie_cache_work_stays_ordered_before_retry() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let first_log = log.clone();
    let (started, start) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let first = OpenAIDashboardBrowserCookieImporter::run_bounded_cookie_cache_operation(
        Some(Instant::now() + Duration::from_secs_f64(0.05)),
        move || {
            first_log.lock().unwrap().push("first-start");
            started.send(()).unwrap();
            let _ = released.recv_timeout(Duration::from_secs(5));
            first_log.lock().unwrap().push("first-end");
            Ok(true)
        },
    )
    .await;
    assert_eq!(first, Err(PersistentCookieSessionError::TimedOut));
    tokio::time::timeout(Duration::from_secs(5), start)
        .await
        .unwrap()
        .unwrap();
    let second_log = log.clone();
    let second = OpenAIDashboardBrowserCookieImporter::run_bounded_cookie_cache_operation(
        Some(Instant::now() + Duration::from_secs_f64(0.05)),
        move || {
            second_log.lock().unwrap().push("second");
            Ok(true)
        },
    )
    .await;
    release.send(()).unwrap();
    assert_eq!(second, Err(PersistentCookieSessionError::TimedOut));
    OpenAIDashboardBrowserCookieImporter::run_bounded_cookie_cache_operation(
        Some(Instant::now() + Duration::from_secs(1)),
        || Ok(true),
    )
    .await
    .unwrap();
    assert_eq!(*log.lock().unwrap(), ["first-start", "first-end", "second"]);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:256 slow callback times out before completion
#[tokio::test]
async fn slow_callback_times_out_before_completion() {
    let probe = Arc::new(AtomicBool::new(false));
    let observer = probe.clone();
    let mut callback = None;
    let result = OpenAIDashboardBrowserCookieImporter::run_bounded_callback(
        Some(Instant::now() + Duration::from_secs_f64(0.05)),
        Some(Arc::new(move || {
            observer.store(true, Ordering::SeqCst);
        })),
        |completion| {
            callback = Some(completion);
        },
    )
    .await;
    callback.take().unwrap()();
    assert_eq!(result, Err(PersistentCookieSessionError::TimedOut));
    assert!(probe.load(Ordering::SeqCst));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:278 slow value callback times out before completion
#[tokio::test]
async fn slow_value_callback_times_out_before_completion() {
    let probe = Arc::new(AtomicBool::new(false));
    let observer = probe.clone();
    let mut callback = None;
    let result =
        OpenAIDashboardBrowserCookieImporter::run_bounded_value_callback::<Vec<String>, _>(
            Some(Instant::now() + Duration::from_secs_f64(0.05)),
            Some(Arc::new(move || {
                observer.store(true, Ordering::SeqCst);
            })),
            |completion| {
                callback = Some(completion);
            },
        )
        .await;
    callback.take().unwrap()(vec![]);
    assert_eq!(result, Err(PersistentCookieSessionError::TimedOut));
    assert!(probe.load(Ordering::SeqCst));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:300 retry waits for timed out cookie store mutation
#[tokio::test]
async fn retry_waits_for_timed_out_cookie_store_mutation() {
    let (captured, capture) = tokio::sync::oneshot::channel();
    let first = OpenAIDashboardBrowserCookieImporter::run_serialized_callback(
        "cookie-mutation",
        Some(Instant::now() + Duration::from_secs_f64(0.05)),
        move |completion| {
            let _ = captured.send(completion);
        },
    )
    .await;
    assert_eq!(first, Err(PersistentCookieSessionError::TimedOut));
    let callback = capture.await.unwrap();
    let started = Arc::new(AtomicBool::new(false));
    let second_started = started.clone();
    let second = tokio::spawn(async move {
        OpenAIDashboardBrowserCookieImporter::run_serialized_callback(
            "cookie-mutation",
            Some(Instant::now() + Duration::from_secs(1)),
            move |completion| {
                second_started.store(true, Ordering::SeqCst);
                completion();
            },
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!started.load(Ordering::SeqCst));
    callback();
    second.await.unwrap().unwrap();
    assert!(started.load(Ordering::SeqCst));
}

// partial-upstream: OpenAIDashboardBrowserCookieImporterTests.swift:334 mismatch error mentions source label
#[test]
fn local_cookie_mismatch_preserves_source_labels_and_redacts_identifiers() {
    let error = OpenAIDashboardCookieImportError::NoMatchingAccount {
        found: vec![
            FoundAccount::new("Safari", "a@example.com"),
            FoundAccount::new("Chrome", "b@example.com"),
        ],
    };
    // Deliberate privacy divergence: retain the upstream inputs and labels, but
    // the upstream email-bearing display assertion is intentionally not ported.
    let message = error.to_string();
    assert!(message.contains("Safari"));
    assert!(message.contains("Chrome"));
    for diagnostic in [message, format!("{error:?}")] {
        assert!(!diagnostic.contains("a@example.com"));
        assert!(!diagnostic.contains("b@example.com"));
    }
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:346 timed out persistent validation keeps verified session
#[test]
fn timed_out_persistent_validation_keeps_verified_session() {
    let failure = OpenAIDashboardBrowserCookieImporter::persistent_validation_failure(
        PersistentCookieSessionError::TimedOut,
    );
    assert!(OpenAIDashboardBrowserCookieImporter::should_trust_verified_session(&failure));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:353 raw cookie mutation timeout is not trusted
#[test]
fn raw_cookie_mutation_timeout_is_not_trusted() {
    assert!(
        !OpenAIDashboardBrowserCookieImporter::should_trust_verified_session(
            &PersistentCookieSessionError::TimedOut
        )
    );
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:359 non-timeout persistent validation failures are not trusted
#[test]
fn non_timeout_persistent_validation_failures_are_not_trusted() {
    assert!(
        !OpenAIDashboardBrowserCookieImporter::should_trust_verified_session(
            &PersistentCookieSessionError::Import(
                OpenAIDashboardCookieImportError::DashboardStillRequiresLogin
            )
        )
    );
}

async fn cookie_script(
    probe: Result<Option<String>, PersistentCookieSessionError>,
    target: Option<&str>,
) -> (
    Result<OpenAIDashboardCookieImportResult, PersistentCookieSessionError>,
    Vec<String>,
) {
    let jar = Arc::new(Mutex::new(None));
    let log = Arc::new(Mutex::new(vec![]));
    let key = format!("script-{:p}", Arc::as_ptr(&jar));
    let result =
        cookie_tracked_commit(&key, "Chrome", jar, log.clone(), target, || async { probe }).await;
    let entries = log
        .lock()
        .unwrap()
        .iter()
        .map(|entry: &String| entry.strip_prefix("Chrome ").unwrap().to_owned())
        .collect();
    (result, entries)
}

async fn cookie_tracked_commit<F, Fut>(
    key: &str,
    label: &'static str,
    jar: Arc<Mutex<Option<String>>>,
    log: Arc<Mutex<Vec<String>>>,
    target: Option<&str>,
    probe: F,
) -> Result<OpenAIDashboardCookieImportResult, PersistentCookieSessionError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Option<String>, PersistentCookieSessionError>>,
{
    let clear_jar = jar.clone();
    let clear_log = log.clone();
    let write_log = log.clone();
    let probe_log = log.clone();
    OpenAIDashboardBrowserCookieImporter::commit_persistent_cookie_session(
        label,
        0,
        target,
        key,
        None,
        move || {
            *clear_jar.lock().unwrap() = None;
            clear_log.lock().unwrap().push(format!("{label} clear"));
        },
        move || {
            *jar.lock().unwrap() = Some(label.to_owned());
            write_log.lock().unwrap().push(format!("{label} write"));
        },
        move || async move {
            probe_log.lock().unwrap().push(format!("{label} probe"));
            probe().await
        },
        move || {
            log.lock().unwrap().push(format!("{label} evict"));
        },
    )
    .await
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:365 account mismatch removes the cookies just written for that email
#[tokio::test]
async fn account_mismatch_removes_the_cookies_just_written_for_that_email() {
    let (result, log) = cookie_script(
        Ok(Some("other@example.com".into())),
        Some("owner@example.com"),
    )
    .await;
    assert_eq!(
        result.unwrap_err(),
        PersistentCookieSessionError::Import(OpenAIDashboardCookieImportError::NoMatchingAccount {
            found: vec![FoundAccount::new("Chrome", "other@example.com")]
        })
    );
    assert_eq!(log, ["clear", "write", "probe", "evict", "clear"]);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:390 login failure removes the cookies just written for that email
#[tokio::test]
async fn login_failure_removes_the_cookies_just_written_for_that_email() {
    let (result, log) = cookie_script(
        Err(PersistentCookieSessionError::Fetch(
            OpenAIDashboardFetchError::LoginRequired,
        )),
        Some("owner@example.com"),
    )
    .await;
    assert_eq!(
        result.unwrap_err(),
        PersistentCookieSessionError::Import(
            OpenAIDashboardCookieImportError::DashboardStillRequiresLogin
        )
    );
    assert_eq!(log, ["clear", "write", "probe", "evict", "clear"]);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:410 timed out persistent validation keeps the cookies just written
#[tokio::test]
async fn timed_out_persistent_validation_keeps_the_cookies_just_written() {
    let (result, log) = cookie_script(
        Err(PersistentCookieSessionError::TimedOut),
        Some("owner@example.com"),
    )
    .await;
    assert!(
        OpenAIDashboardBrowserCookieImporter::should_trust_verified_session(&result.unwrap_err())
    );
    assert_eq!(log, ["clear", "write", "probe", "evict"]);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:425 default store keeps cookies when persistent validation times out
#[tokio::test]
async fn default_store_keeps_cookies_when_persistent_validation_times_out() {
    // Rust represents an empty, non-matching target with None.
    let (result, log) = cookie_script(Err(PersistentCookieSessionError::TimedOut), None).await;
    let error = result.unwrap_err();
    assert_eq!(error, PersistentCookieSessionError::TimedOut);
    assert!(!OpenAIDashboardBrowserCookieImporter::should_trust_verified_session(&error));
    assert_eq!(log, ["clear", "write", "probe", "evict"]);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:443 default store login failure removes the cookies just written
#[tokio::test]
async fn default_store_login_failure_removes_the_cookies_just_written() {
    let (result, log) = cookie_script(
        Err(PersistentCookieSessionError::Fetch(
            OpenAIDashboardFetchError::LoginRequired,
        )),
        None,
    )
    .await;
    assert_eq!(
        result.unwrap_err(),
        PersistentCookieSessionError::Import(
            OpenAIDashboardCookieImportError::DashboardStillRequiresLogin
        )
    );
    assert_eq!(log, ["clear", "write", "probe", "evict", "clear"]);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:463 matching session keeps the cookies it just wrote
#[tokio::test]
async fn matching_session_keeps_the_cookies_it_just_wrote() {
    let (result, log) = cookie_script(
        Ok(Some("owner@example.com".into())),
        Some("owner@example.com"),
    )
    .await;
    let result = result.unwrap();
    assert_eq!(result.signed_in_email.as_deref(), Some("owner@example.com"));
    assert!(result.matches_codex_email);
    assert_eq!(log, ["clear", "write", "probe"]);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:477 unrelated probe failure keeps the cookies it just wrote
#[tokio::test]
async fn unrelated_probe_failure_keeps_the_cookies_it_just_wrote() {
    // Rust intentionally carries no response body in NoDashboardData.
    let (result, log) = cookie_script(
        Err(PersistentCookieSessionError::Fetch(
            OpenAIDashboardFetchError::NoDashboardData,
        )),
        Some("owner@example.com"),
    )
    .await;
    assert_eq!(
        result.unwrap_err(),
        PersistentCookieSessionError::Fetch(OpenAIDashboardFetchError::NoDashboardData)
    );
    assert_eq!(log, ["clear", "write", "probe", "evict"]);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:497 older rejection cannot erase a newer import on the same account store
#[tokio::test]
async fn older_rejection_cannot_erase_a_newer_import_on_the_same_account_store() {
    let jar = Arc::new(Mutex::new(None));
    let log = Arc::new(Mutex::new(vec![]));
    let (entered, enter) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let older_jar = jar.clone();
    let older_log = log.clone();
    let older = tokio::spawn(async move {
        cookie_tracked_commit(
            "shared-account",
            "older",
            older_jar,
            older_log,
            Some("owner@example.com"),
            move || async move {
                entered.send(()).unwrap();
                released.await.unwrap();
                Err(PersistentCookieSessionError::Fetch(
                    OpenAIDashboardFetchError::LoginRequired,
                ))
            },
        )
        .await
    });
    enter.await.unwrap();
    let newer_jar = jar.clone();
    let newer_log = log.clone();
    let (started, start) = tokio::sync::oneshot::channel();
    let newer = tokio::spawn(async move {
        started.send(()).unwrap();
        cookie_tracked_commit(
            "shared-account",
            "newer",
            newer_jar,
            newer_log,
            Some("owner@example.com"),
            || async { Ok(Some("owner@example.com".into())) },
        )
        .await
    });
    start.await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(jar.lock().unwrap().as_deref(), Some("older"));
    release.send(()).unwrap();
    assert_eq!(
        older.await.unwrap().unwrap_err(),
        PersistentCookieSessionError::Import(
            OpenAIDashboardCookieImportError::DashboardStillRequiresLogin
        )
    );
    let result = newer.await.unwrap().unwrap();
    assert_eq!(result.signed_in_email.as_deref(), Some("owner@example.com"));
    assert_eq!(jar.lock().unwrap().as_deref(), Some("newer"));
    assert_eq!(
        *log.lock().unwrap(),
        [
            "older clear",
            "older write",
            "older probe",
            "older evict",
            "older clear",
            "newer clear",
            "newer write",
            "newer probe"
        ]
    );
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:574 an in-flight import does not block a different account store
#[tokio::test]
async fn an_in_flight_import_does_not_block_a_different_account_store() {
    let older_jar = Arc::new(Mutex::new(None));
    let newer_jar = Arc::new(Mutex::new(None));
    let log = Arc::new(Mutex::new(vec![]));
    let (entered, enter) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let jar = older_jar.clone();
    let older_log = log.clone();
    let older = tokio::spawn(async move {
        cookie_tracked_commit(
            "account-a",
            "older",
            jar,
            older_log,
            Some("owner@example.com"),
            move || async move {
                entered.send(()).unwrap();
                released.await.unwrap();
                Ok(Some("owner@example.com".into()))
            },
        )
        .await
    });
    enter.await.unwrap();
    let result = cookie_tracked_commit(
        "account-b",
        "newer",
        newer_jar.clone(),
        log,
        Some("owner@example.com"),
        || async { Ok(Some("owner@example.com".into())) },
    )
    .await
    .unwrap();
    assert_eq!(older_jar.lock().unwrap().as_deref(), Some("older"));
    assert_eq!(newer_jar.lock().unwrap().as_deref(), Some("newer"));
    release.send(()).unwrap();
    older.await.unwrap().unwrap();
    assert_eq!(result.signed_in_email.as_deref(), Some("owner@example.com"));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:628 waiting account transaction respects deadline and cancellation
#[tokio::test]
async fn waiting_account_transaction_respects_deadline_and_cancellation() {
    let (entered, enter) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let key = "waiting-cookie-transaction";
    let touched = Arc::new(AtomicBool::new(false));
    let holder = tokio::spawn(async move {
        OpenAIDashboardBrowserCookieImporter::with_persistent_cookie_transaction(
            key,
            None,
            None,
            move || async move {
                entered.send(()).unwrap();
                released.await.unwrap();
                Ok(())
            },
        )
        .await
    });
    enter.await.unwrap();
    let action_touched = touched.clone();
    let result = OpenAIDashboardBrowserCookieImporter::with_persistent_cookie_transaction(
        key,
        Some(Instant::now() + Duration::from_secs_f64(0.03)),
        None,
        || async {
            action_touched.store(true, Ordering::SeqCst);
            Ok(())
        },
    )
    .await;
    assert_eq!(result, Err(PersistentCookieSessionError::TimedOut));
    let cancel = CancellationToken::new();
    let pending_cancel = cancel.clone();
    let pending_touched = touched.clone();
    let (started, start) = tokio::sync::oneshot::channel();
    let pending = tokio::spawn(async move {
        started.send(()).unwrap();
        OpenAIDashboardBrowserCookieImporter::with_persistent_cookie_transaction(
            key,
            None,
            Some(&pending_cancel),
            || async {
                pending_touched.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
    });
    start.await.unwrap();
    cancel.cancel();
    assert_eq!(
        pending.await.unwrap(),
        Err(PersistentCookieSessionError::Cancelled)
    );
    assert!(!touched.load(Ordering::SeqCst));
    release.send(()).unwrap();
    holder.await.unwrap().unwrap();
    OpenAIDashboardBrowserCookieImporter::with_persistent_cookie_transaction(
        key,
        Some(Instant::now() + Duration::from_secs(1)),
        None,
        || async {
            touched.store(true, Ordering::SeqCst);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert!(touched.load(Ordering::SeqCst));
}

// ============================================================================
// Suite 7: OpenAIDashboardSessionAuthorizationTests.swift
// ============================================================================

fn endpoint_for_server(server: &httpmock::MockServer) -> OpenAIDashboardEndpointConfig {
    OpenAIDashboardEndpointConfig::from_base_url_str(&server.base_url()).unwrap()
}

fn authorization_stage(request: &httpmock::HttpMockRequest) -> &'static str {
    let uri = request.uri();
    match uri.path() {
        "/api/auth/session" => "session",
        path if path.ends_with("/monthly-usage") => "monthly",
        path if path.ends_with("/remaining_balance") => "balance",
        _ if request.headers().contains_key("authorization") => "retry",
        _ => "initial",
    }
}

fn authorization_session(token: &str, email: &str) -> String {
    format!(
        r#"{{"accessToken":"{token}","user":{{"email":" {email} "}},"other":{{"email":"other@example.com"}}}}"#
    )
}

fn authorization_usage(account: &str, balance: &str, monthly: bool) -> String {
    let monthly = if monthly {
        r#", "spend_control":{"individual_limit":null}"#
    } else {
        ""
    };
    format!(
        r#"{{"account_id":"{account}","plan_type":"business","rate_limit":{{"primary_window":{{"used_percent":12,"reset_at":1700003600,"limit_window_seconds":18000}}}},"credits":{{"has_credits":true,"unlimited":false,"balance":{balance}}}{monthly}}}"#
    )
}

fn authorization_response(
    request: &httpmock::HttpMockRequest,
    balance: f64,
    monthly: bool,
) -> httpmock::HttpMockResponse {
    let (status, payload) = match authorization_stage(request) {
        "session" => (
            200,
            authorization_session("fixture-token", "owner@example.com"),
        ),
        "retry" => (
            200,
            authorization_usage("workspace-fixture", "null", monthly),
        ),
        "balance" => (200, format!(r#"{{"balance":"{balance:.1}"}}"#)),
        _ => (401, "{}".into()),
    };
    authorization_http(status, payload)
}

fn authorization_http(status: u16, payload: impl Into<String>) -> httpmock::HttpMockResponse {
    httpmock::HttpMockResponse::builder()
        .status(status)
        .body(payload.into())
        .build()
}

struct AuthorizationHarness {
    server: httpmock::MockServer,
    requests: Arc<Mutex<Vec<httpmock::HttpMockRequest>>>,
}

impl AuthorizationHarness {
    fn new(
        responder: impl Fn(&httpmock::HttpMockRequest) -> httpmock::HttpMockResponse
        + Send
        + Sync
        + 'static,
    ) -> Self {
        let server = https::start_server();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        server.mock(|when, then| {
            when.any_request();
            then.respond_with(move |request| {
                recorded.lock().unwrap().push(request.clone());
                responder(request)
            });
        });
        Self { server, requests }
    }

    fn stages(&self) -> Vec<&'static str> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(authorization_stage)
            .collect()
    }

    async fn fetch(
        &self,
        cookie: &str,
        deadline: Option<Instant>,
        call: &ariadusage_engine::brokers::call::BrokerCall,
    ) -> Result<Option<DashboardAPIResponse>, OpenAIDashboardFetchError> {
        OpenAIDashboardFetcher::fetch_dashboard_api_response(
            &https::broker(NetConfig::default()),
            &endpoint_for_server(&self.server),
            cookie,
            deadline,
            Timestamp::now(),
            &Instant::now,
            Some(call),
        )
        .await
    }
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:8 cookie retry pairs workspace balance with the same session identity
#[tokio::test]
async fn cookie_retry_pairs_workspace_balance_with_the_same_session_identity() {
    for balance in [0.0, 1234.0] {
        let harness = AuthorizationHarness::new(move |request| {
            authorization_response(request, balance, false)
        });
        let response = harness
            .fetch("session=fixture", None, &https::call())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            response.verified_signed_in_email.as_deref(),
            Some("owner@example.com")
        );
        assert_eq!(response.api_data.credits_remaining, Some(balance));
        assert_eq!(response.api_data.balance_is_workspace, Some(true));
        assert_eq!(
            response
                .api_data
                .primary_limit
                .as_ref()
                .map(|w| w.used_percent),
            Some(12.0)
        );
        assert_eq!(harness.stages(), ["initial", "session", "retry", "balance"]);
        for request in harness.requests.lock().unwrap().iter() {
            // A local HTTPS origin replaces the pinned upstream host; NetBroker has
            // no URL cache, so there is no URLRequest cachePolicy to set or inspect.
            let endpoint = endpoint_for_server(&harness.server);
            assert_eq!(endpoint.usage_url().unwrap().scheme(), "https");
            assert_eq!(request.headers().get("cookie").unwrap(), "session=fixture");
            let stage = authorization_stage(request);
            let expected = ["retry", "balance"]
                .contains(&stage)
                .then_some("Bearer fixture-token");
            assert_eq!(
                request
                    .headers()
                    .get("authorization")
                    .map(|v| v.to_str().unwrap()),
                expected
            );
            if stage == "balance" {
                assert_eq!(
                    request.uri().path(),
                    "/backend-api/accounts/workspace-fixture/remaining_balance"
                );
                assert_eq!(
                    request.headers().get("chatgpt-account-id").unwrap(),
                    "workspace-fixture"
                );
            }
        }
        // Rust exposes redacted diagnostics rather than the Swift logger callback.
        let diagnostics = format!("{response:?}");
        assert!(!diagnostics.contains("fixture-token"));
        assert!(!diagnostics.contains("owner@example.com"));
        let dashboard = OpenAIDashboardFetcher::snapshot_by_merging_api(
            &response.api_data,
            None,
            response.verified_signed_in_email.as_deref(),
            OpenAISubscriptionFetchResult::Unavailable,
            Timestamp::now(),
        );
        assert!(
            !serde_json::to_string(&dashboard)
                .unwrap()
                .contains("fixture-token")
        );
        assert_eq!(
            dashboard.to_credits_snapshot().unwrap().display_remaining(),
            Some(balance)
        );
    }
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:45 successful cookie usage does not fetch a session or invent an authenticated email
#[tokio::test]
async fn successful_cookie_usage_does_not_fetch_a_session_or_invent_an_authenticated_email() {
    let harness = AuthorizationHarness::new(|request| {
        assert!(!request.headers().contains_key("authorization"));
        authorization_http(200, authorization_usage("workspace-fixture", "14", false))
    });
    let response = harness
        .fetch("session=fixture", None, &https::call())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.api_data.credits_remaining, Some(14.0));
    assert_eq!(response.verified_signed_in_email, None);
    assert_eq!(harness.stages(), ["initial"]);
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:58 invalid typed session credentials cannot authorize a retry
#[tokio::test]
async fn invalid_typed_session_credentials_cannot_authorize_a_retry() {
    for payload in [
        "not-json",
        r#"{}"#,
        r#"{"accessToken":"fixture-token","email":"other@example.com"}"#,
        r#"{"accessToken":"fixture-token","user":{"email":""}}"#,
        r#"{"accessToken":"fixture-token","user":{"email":" \n "}}"#,
        r#"{"accessToken":"fixture-token","user":{"email":123}}"#,
        r#"{"user":{"email":"owner@example.com"}}"#,
        r#"{"accessToken":null,"user":{"email":"owner@example.com"}}"#,
        r#"{"accessToken":"","user":{"email":"owner@example.com"}}"#,
        r#"{"accessToken":" ","user":{"email":"owner@example.com"}}"#,
        r#"{"accessToken":"fixture token","user":{"email":"owner@example.com"}}"#,
        r#"{"accessToken":"fixture-token\n","user":{"email":"owner@example.com"}}"#,
        r#"{"accessToken":"fixture\u0000token","user":{"email":"owner@example.com"}}"#,
    ] {
        let harness = AuthorizationHarness::new(move |request| {
            if authorization_stage(request) == "session" {
                authorization_http(200, payload)
            } else {
                authorization_response(request, 1234.0, false)
            }
        });
        assert!(
            harness
                .fetch("session=fixture", None, &https::call())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(harness.stages(), ["initial", "session"]);
    }
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:84 nonauthentication usage failures do not start session authentication
#[tokio::test]
async fn nonauthentication_usage_failures_do_not_start_session_authentication() {
    for status in [403, 429, 500] {
        let harness = AuthorizationHarness::new(move |_| authorization_http(status, "{}"));
        assert!(
            harness
                .fetch("session=fixture", None, &https::call())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(harness.stages(), ["initial"]);
    }
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:93 malformed successful usage does not start session authentication
#[tokio::test]
async fn malformed_successful_usage_does_not_start_session_authentication() {
    let harness = AuthorizationHarness::new(|_| authorization_http(200, "not-json"));
    assert!(
        harness
            .fetch("session=fixture", None, &https::call())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(harness.stages(), ["initial"]);
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:102 session HTTP failures stop before bearer usage
#[tokio::test]
async fn session_http_failures_stop_before_bearer_usage() {
    for status in [401, 403, 500] {
        let harness = AuthorizationHarness::new(move |request| {
            if authorization_stage(request) == "session" {
                authorization_http(status, "{}")
            } else {
                authorization_response(request, 1234.0, false)
            }
        });
        assert!(
            harness
                .fetch("session=fixture", None, &https::call())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(harness.stages(), ["initial", "session"]);
    }
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:114 a second usage authentication failure does not repeat session bootstrap
#[tokio::test]
async fn a_second_usage_authentication_failure_does_not_repeat_session_bootstrap() {
    let harness = AuthorizationHarness::new(|request| {
        if authorization_stage(request) == "retry" {
            authorization_http(401, "{}")
        } else {
            authorization_response(request, 1234.0, false)
        }
    });
    assert!(
        harness
            .fetch("session=fixture", None, &https::call())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(harness.stages(), ["initial", "session", "retry"]);
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:126 independent cookies keep their bearer identity and workspace isolated
#[tokio::test]
async fn independent_cookies_keep_their_bearer_identity_and_workspace_isolated() {
    let harness = AuthorizationHarness::new(|request| {
        let first = request.headers().get("cookie").unwrap() == "session=first";
        let name = if first { "first" } else { "second" };
        let stage = authorization_stage(request);
        if stage == "session" {
            return authorization_http(
                200,
                authorization_session(&format!("fixture-{name}"), &format!("{name}@example.com")),
            );
        }
        if ["retry", "balance"].contains(&stage) {
            assert_eq!(
                request
                    .headers()
                    .get("authorization")
                    .unwrap()
                    .to_str()
                    .unwrap(),
                format!("Bearer fixture-{name}")
            );
        }
        if stage == "retry" {
            return authorization_http(
                200,
                authorization_usage(&format!("workspace-{name}"), "null", false),
            );
        }
        if stage == "balance" {
            assert_eq!(
                request
                    .headers()
                    .get("chatgpt-account-id")
                    .unwrap()
                    .to_str()
                    .unwrap(),
                format!("workspace-{name}")
            );
            assert_eq!(
                request.uri().path(),
                format!("/backend-api/accounts/workspace-{name}/remaining_balance")
            );
        }
        authorization_response(request, if first { 1234.0 } else { 1200.0 }, false)
    });
    let first = harness
        .fetch("session=first", None, &https::call())
        .await
        .unwrap()
        .unwrap();
    let second = harness
        .fetch("session=second", None, &https::call())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first.verified_signed_in_email.as_deref(),
        Some("first@example.com")
    );
    assert_eq!(
        second.verified_signed_in_email.as_deref(),
        Some("second@example.com")
    );
    assert_eq!(first.api_data.credits_remaining, Some(1234.0));
    assert_eq!(second.api_data.credits_remaining, Some(1200.0));
    assert_eq!(harness.stages().len(), 8);
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:158 optional unauthorized enrichments retain authenticated usage without reauthenticating
#[tokio::test]
async fn optional_unauthorized_enrichments_retain_authenticated_usage_without_reauthenticating() {
    for hide_balance in [false, true] {
        for status in [401, 403] {
            let harness = AuthorizationHarness::new(move |request| {
                let stage = authorization_stage(request);
                if ["monthly", "balance"].contains(&stage) {
                    assert_eq!(
                        request.headers().get("authorization").unwrap(),
                        "Bearer fixture-token"
                    );
                    assert_eq!(request.headers().get("cookie").unwrap(), "session=fixture");
                }
                if stage == "monthly" || (hide_balance && stage == "balance") {
                    authorization_http(status, "{}")
                } else {
                    authorization_response(request, 1234.0, true)
                }
            });
            let response = harness
                .fetch("session=fixture", None, &https::call())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                response.verified_signed_in_email.as_deref(),
                Some("owner@example.com")
            );
            assert_eq!(
                response
                    .api_data
                    .primary_limit
                    .as_ref()
                    .map(|w| w.used_percent),
                Some(12.0)
            );
            assert_eq!(response.api_data.codex_credit_limit, None);
            assert_eq!(response.api_data.credits_available, Some(true));
            assert_eq!(
                response.api_data.credits_remaining,
                if hide_balance { None } else { Some(1234.0) }
            );
            assert_eq!(
                response.api_data.balance_is_workspace,
                if hide_balance { None } else { Some(true) }
            );
            assert_eq!(
                harness.stages(),
                ["initial", "session", "retry", "monthly", "balance"]
            );
        }
    }
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:185 an expired shared deadline sends no authentication requests
#[tokio::test]
async fn an_expired_shared_deadline_sends_no_authentication_requests() {
    let harness =
        AuthorizationHarness::new(|request| authorization_response(request, 1234.0, false));
    assert!(
        harness
            .fetch(
                "session=fixture",
                Some(Instant::now() - Duration::from_secs(1)),
                &https::call()
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(harness.stages().is_empty());
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:195 deadline expiry after cookie rejection prevents the session request
#[tokio::test]
async fn deadline_expiry_after_cookie_rejection_prevents_the_session_request() {
    let harness =
        AuthorizationHarness::new(|request| authorization_response(request, 1234.0, false));
    // Real HTTPS replaces the transport stub: delay the initial rejection beyond
    // the one-second deadline, retaining the upstream 0.01-second overrun.
    harness.server.reset();
    let recorded = harness.requests.clone();
    harness.server.mock(|when, then| {
        when.any_request();
        then.respond_with(move |request| {
            recorded.lock().unwrap().push(request.clone());
            assert_eq!(authorization_stage(request), "initial");
            std::thread::sleep(Duration::from_secs_f64(1.01));
            authorization_http(401, "{}")
        });
    });
    assert!(
        harness
            .fetch(
                "session=fixture",
                Some(Instant::now() + Duration::from_secs(1)),
                &https::call()
            )
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(harness.stages(), ["initial"]);
}

// partial-upstream: OpenAIDashboardSessionAuthorizationTests.swift:208 session retry and optional requests share the remaining deadline
#[tokio::test]
async fn local_session_deadline_budget_and_request_sequence() {
    let harness =
        AuthorizationHarness::new(|request| authorization_response(request, 1234.0, true));
    let start = Instant::now();
    let deadline = start + Duration::from_secs_f64(3.5);
    let times = Mutex::new(Vec::new());
    let clock = || {
        let now = Instant::now();
        times.lock().unwrap().push(now);
        now
    };
    let response = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &https::broker(NetConfig::default()),
        &endpoint_for_server(&harness.server),
        "session=fixture",
        Some(deadline),
        Timestamp::now(),
        &clock,
        Some(&https::call()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.api_data.credits_remaining, Some(1234.0));
    assert_eq!(
        harness.stages(),
        ["initial", "session", "retry", "monthly", "balance"]
    );
    // Partial: the HTTPS server cannot inspect client request timeouts. The
    // injected clock proves that the sequence uses the same remaining budget;
    // the session-specific two-second cap remains unobservable here.
    let mut previous = Duration::from_secs_f64(3.5);
    for now in times.into_inner().unwrap() {
        let remaining = deadline.saturating_duration_since(now);
        assert!(remaining > Duration::ZERO && remaining <= Duration::from_secs_f64(3.5));
        assert!(remaining <= previous);
        previous = remaining;
    }
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:229 cancellation before fetch sends no requests
#[tokio::test]
async fn cancellation_before_fetch_sends_no_requests() {
    let harness =
        AuthorizationHarness::new(|request| authorization_response(request, 1234.0, false));
    let call = https::call();
    call.cancel.cancel();
    assert!(matches!(
        harness.fetch("session=fixture", None, &call).await,
        Err(OpenAIDashboardFetchError::Cancelled)
    ));
    assert!(harness.stages().is_empty());
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:245 cancellation after a transport response stops the authentication sequence
#[tokio::test]
async fn cancellation_after_a_transport_response_stops_the_authentication_sequence() {
    for stage in ["initial", "session", "retry", "monthly", "balance"] {
        let call = https::call();
        let cancel = call.cancel.clone();
        let harness = AuthorizationHarness::new(move |request| {
            if authorization_stage(request) == stage {
                cancel.cancel();
            }
            authorization_response(request, 1234.0, true)
        });
        assert!(matches!(
            harness.fetch("session=fixture", None, &call).await,
            Err(OpenAIDashboardFetchError::Cancelled)
        ));
        let stages = harness.stages();
        assert_eq!(stages.last(), Some(&stage));
        assert_eq!(
            stages,
            ["initial", "session", "retry", "monthly", "balance"][..stages.len()]
        );
    }
}

// ============================================================================
// Suite 8: OpenAIWebRefreshGateTests.swift
// ============================================================================

// upstream: OpenAIWebRefreshGateTests.swift:7 Battery saver keeps background OpenAI web refreshes off
#[test]
fn battery_saver_keeps_background_open_ai_web_refreshes_off() {
    assert!(!should_run_openai_web_refresh(
        OpenAIWebRefreshPolicyContext {
            access_enabled: true,
            battery_saver_enabled: true,
            force: false,
            refresh_phase: FetchPhase::Regular,
        }
    ));
}

// upstream: OpenAIWebRefreshGateTests.swift:18 Disabling battery saver restores normal OpenAI web refreshes
#[test]
fn disabling_battery_saver_restores_normal_open_ai_web_refreshes() {
    assert!(should_run_openai_web_refresh(
        OpenAIWebRefreshPolicyContext {
            access_enabled: true,
            battery_saver_enabled: false,
            force: false,
            refresh_phase: FetchPhase::Regular,
        }
    ));
}

// upstream: OpenAIWebRefreshGateTests.swift:29 Manual refresh still forces OpenAI web refreshes with battery saver enabled
#[test]
fn manual_refresh_still_forces_open_ai_web_refreshes_with_battery_saver_enabled() {
    assert!(should_run_openai_web_refresh(
        OpenAIWebRefreshPolicyContext {
            access_enabled: true,
            battery_saver_enabled: true,
            force: true,
            refresh_phase: FetchPhase::Regular,
        }
    ));
}

// upstream: OpenAIWebRefreshGateTests.swift:40 Startup skips automatic OpenAI web refreshes
#[test]
fn startup_skips_automatic_open_ai_web_refreshes() {
    assert!(!should_run_openai_web_refresh(
        OpenAIWebRefreshPolicyContext {
            access_enabled: true,
            battery_saver_enabled: false,
            force: false,
            refresh_phase: FetchPhase::Startup,
        }
    ));
}

// upstream: OpenAIWebRefreshGateTests.swift:51 Startup connectivity retry remains startup only for OpenAI web refresh gate
#[test]
fn startup_connectivity_retry_remains_startup_only_for_open_ai_web_refresh_gate() {
    let provider_phase = refresh_phase(true);
    let web_phase = openai_web_refresh_phase(provider_phase, 1);

    assert_eq!(provider_phase, FetchPhase::Regular);
    assert_eq!(web_phase, FetchPhase::Startup);
    assert!(!should_run_openai_web_refresh(
        OpenAIWebRefreshPolicyContext {
            access_enabled: true,
            battery_saver_enabled: false,
            force: false,
            refresh_phase: web_phase,
        }
    ));
}

// upstream: OpenAIWebRefreshGateTests.swift:69 Manual startup refresh still forces OpenAI web refreshes
#[test]
fn manual_startup_refresh_still_forces_open_ai_web_refreshes() {
    assert!(should_run_openai_web_refresh(
        OpenAIWebRefreshPolicyContext {
            access_enabled: true,
            battery_saver_enabled: true,
            force: true,
            refresh_phase: FetchPhase::Startup,
        }
    ));
}

// upstream: OpenAIWebRefreshGateTests.swift:80 Battery saver stale-submenu refresh respects the cooldown
#[test]
fn battery_saver_stale_submenu_refresh_respects_the_cooldown() {
    assert!(!force_openai_web_refresh_for_stale_request(true));
}

// upstream: OpenAIWebRefreshGateTests.swift:87 Normal stale-submenu refresh still forces when battery saver is off
#[test]
fn normal_stale_submenu_refresh_still_forces_when_battery_saver_is_off() {
    assert!(force_openai_web_refresh_for_stale_request(false));
}

// upstream: OpenAIWebRefreshGateTests.swift:94 Recent successful dashboard refresh stays throttled
#[test]
fn recent_successful_dashboard_refresh_stays_throttled() {
    let now = Timestamp::now();
    let recent = now - Duration::from_secs(60);
    let context = OpenAIWebRefreshGateContext {
        force: false,
        account_did_change: false,
        last_error: None,
        last_snapshot_at: Some(recent),
        last_attempt_at: Some(recent),
        now,
        refresh_interval_seconds: 300.0,
    };

    assert!(should_skip_openai_web_refresh(&context));
}

// upstream: OpenAIWebRefreshGateTests.swift:110 Recent failed dashboard refresh also stays throttled
#[test]
fn recent_failed_dashboard_refresh_also_stays_throttled() {
    let now = Timestamp::now();
    let recent = now - Duration::from_secs(60);
    let context = OpenAIWebRefreshGateContext {
        force: false,
        account_did_change: false,
        last_error: Some("login required".to_owned()),
        last_snapshot_at: None,
        last_attempt_at: Some(recent),
        now,
        refresh_interval_seconds: 300.0,
    };

    assert!(should_skip_openai_web_refresh(&context));
}

// upstream: OpenAIWebRefreshGateTests.swift:126 Force refresh bypasses throttle after failures
#[test]
fn force_refresh_bypasses_throttle_after_failures() {
    let now = Timestamp::now();
    let recent = now - Duration::from_secs(60);
    let context = OpenAIWebRefreshGateContext {
        force: true,
        account_did_change: false,
        last_error: Some("login required".to_owned()),
        last_snapshot_at: None,
        last_attempt_at: Some(recent),
        now,
        refresh_interval_seconds: 300.0,
    };

    assert!(!should_skip_openai_web_refresh(&context));
}

// upstream: OpenAIWebRefreshGateTests.swift:142 Account switches bypass the prior-attempt cooldown
#[test]
fn account_switches_bypass_the_prior_attempt_cooldown() {
    let now = Timestamp::now();
    let recent = now - Duration::from_secs(60);
    let context = OpenAIWebRefreshGateContext {
        force: false,
        account_did_change: true,
        last_error: Some("mismatch".to_owned()),
        last_snapshot_at: None,
        last_attempt_at: Some(recent),
        now,
        refresh_interval_seconds: 300.0,
    };

    assert!(!should_skip_openai_web_refresh(&context));
}

// upstream: OpenAIWebRefreshGateTests.swift:158 Empty dashboard history retry is throttled after a recent attempt
#[test]
fn empty_dashboard_history_retry_is_throttled_after_a_recent_attempt() {
    let now = Timestamp::now();
    let snapshot_at = now - Duration::from_secs(120);
    let attempt_at = now - Duration::from_secs(60);
    let context = OpenAIWebRefreshGateContext {
        force: false,
        account_did_change: false,
        last_error: None,
        last_snapshot_at: Some(snapshot_at),
        last_attempt_at: Some(attempt_at),
        now,
        refresh_interval_seconds: 300.0,
    };

    assert!(should_skip_openai_web_empty_history_retry(&context));
}

// upstream: OpenAIWebRefreshGateTests.swift:174 code review alone is not dashboard page history
#[test]
fn code_review_alone_is_not_dashboard_page_history() {
    let snapshot = OpenAIDashboardSnapshot {
        signed_in_email: Some("user@example.com".to_owned()),
        code_review_remaining_percent: Some(80.0),
        updated_at: Timestamp::now(),
        ..Default::default()
    };

    assert!(!dashboard_has_page_history(Some(&snapshot)));
}

// upstream: OpenAIWebRefreshGateTests.swift:188 cached history without an in memory scrape stamp does not scrape
#[test]
fn cached_history_without_an_in_memory_scrape_stamp_does_not_scrape() {
    let now = Timestamp::now();
    let gate = OpenAIDashboardPageScrapeGate {
        force: false,
        user_initiated: false,
        has_history: true,
        last_page_scrape_at: None,
        history_updated_at: Some(now - Duration::from_secs(60)),
        now,
        refresh_interval_seconds: 300.0,
    };

    assert!(!should_allow_openai_dashboard_page_scrape(&gate));
}

// upstream: OpenAIWebRefreshGateTests.swift:203 page scrape stays on when history has never been collected
#[test]
fn page_scrape_stays_on_when_history_has_never_been_collected() {
    let gate = OpenAIDashboardPageScrapeGate {
        force: false,
        user_initiated: false,
        has_history: false,
        last_page_scrape_at: None,
        history_updated_at: None,
        now: Timestamp::now(),
        refresh_interval_seconds: 300.0,
    };

    assert!(should_allow_openai_dashboard_page_scrape(&gate));
}

// upstream: OpenAIWebRefreshGateTests.swift:217 page scrape is skipped when recent history already exists
#[test]
fn page_scrape_is_skipped_when_recent_history_already_exists() {
    let now = Timestamp::now();
    let gate = OpenAIDashboardPageScrapeGate {
        force: false,
        user_initiated: false,
        has_history: true,
        last_page_scrape_at: Some(now - Duration::from_secs(60)),
        history_updated_at: Some(now - Duration::from_secs(60)),
        now,
        refresh_interval_seconds: 300.0,
    };

    assert!(!should_allow_openai_dashboard_page_scrape(&gate));
}

// upstream: OpenAIWebRefreshGateTests.swift:232 user initiated force refresh still scrapes recent history
#[test]
fn user_initiated_force_refresh_still_scrapes_recent_history() {
    let now = Timestamp::now();
    let gate = OpenAIDashboardPageScrapeGate {
        force: true,
        user_initiated: true,
        has_history: true,
        last_page_scrape_at: Some(now - Duration::from_secs(60)),
        history_updated_at: Some(now - Duration::from_secs(60)),
        now,
        refresh_interval_seconds: 300.0,
    };

    assert!(should_allow_openai_dashboard_page_scrape(&gate));
}

// upstream: OpenAIWebRefreshGateTests.swift:247 background force refresh does not scrape recent history
#[test]
fn background_force_refresh_does_not_scrape_recent_history() {
    let now = Timestamp::now();
    let gate = OpenAIDashboardPageScrapeGate {
        force: true,
        user_initiated: false,
        has_history: true,
        last_page_scrape_at: Some(now - Duration::from_secs(60)),
        history_updated_at: Some(now - Duration::from_secs(60)),
        now,
        refresh_interval_seconds: 300.0,
    };

    assert!(!should_allow_openai_dashboard_page_scrape(&gate));
}

// upstream: OpenAIWebRefreshGateTests.swift:262 page scrape returns after the dashboard interval
#[test]
fn page_scrape_returns_after_the_dashboard_interval() {
    let now = Timestamp::now();
    let gate = OpenAIDashboardPageScrapeGate {
        force: false,
        user_initiated: false,
        has_history: true,
        last_page_scrape_at: Some(now - Duration::from_secs(301)),
        history_updated_at: Some(now - Duration::from_secs(301)),
        now,
        refresh_interval_seconds: 300.0,
    };

    assert!(should_allow_openai_dashboard_page_scrape(&gate));
}

// upstream: OpenAIWebRefreshGateTests.swift:277 Empty dashboard history retry runs once for a newer empty snapshot
#[test]
fn empty_dashboard_history_retry_runs_once_for_a_newer_empty_snapshot() {
    let now = Timestamp::now();
    let attempt_at = now - Duration::from_secs(120);
    let newer_snapshot_at = now - Duration::from_secs(60);
    let context = OpenAIWebRefreshGateContext {
        force: false,
        account_did_change: false,
        last_error: None,
        last_snapshot_at: Some(newer_snapshot_at),
        last_attempt_at: Some(attempt_at),
        now,
        refresh_interval_seconds: 300.0,
    };

    assert!(!should_skip_openai_web_empty_history_retry(&context));
}

// ============================================================================
// Suite 9: OpenAIDashboardModelsTests.swift
// ============================================================================

fn models_day(
    day: &str,
    services: Vec<OpenAIDashboardServiceUsage>,
    total: f64,
) -> OpenAIDashboardDailyBreakdown {
    OpenAIDashboardDailyBreakdown::new(day, services, total)
}

fn models_snapshot(
    usage: Vec<OpenAIDashboardDailyBreakdown>,
    updated_at: Timestamp,
    primary: Option<RateWindow>,
    renewal: Option<Timestamp>,
) -> OpenAIDashboardSnapshot {
    OpenAIDashboardSnapshot::new(
        Some("codex@example.com".into()),
        None,
        None,
        None,
        vec![],
        vec![],
        usage,
        None,
        primary,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        renewal,
        updated_at,
    )
}

fn models_now() -> Timestamp {
    "2026-06-30T12:00:00Z".parse().unwrap()
}

fn models_days(summary: &OpenAIDashboardUsageBreakdownSummary) -> Vec<&str> {
    summary.daily.iter().map(|day| day.day.as_str()).collect()
}

// upstream: OpenAIDashboardModelsTests.swift:16 removes skill usage services from usage breakdown
#[test]
fn removes_skill_usage_services_from_usage_breakdown() {
    let breakdown = vec![
        models_day(
            "2026-04-30",
            vec![
                OpenAIDashboardServiceUsage::new("Desktop App", 10.0),
                OpenAIDashboardServiceUsage::new("Skillusage:imagegen", 7.0),
                OpenAIDashboardServiceUsage::new(" skillusage:github:github ", 2.0),
            ],
            19.0,
        ),
        models_day(
            "2026-04-29",
            vec![OpenAIDashboardServiceUsage::new(
                "Skillusage:deep Research",
                3.0,
            )],
            3.0,
        ),
    ];
    assert_eq!(
        OpenAIDashboardSnapshot::removing_skill_usage_services(&breakdown),
        [models_day(
            "2026-04-30",
            vec![OpenAIDashboardServiceUsage::new("Desktop App", 10.0)],
            10.0
        )]
    );
}

// upstream: OpenAIDashboardModelsTests.swift:47 snapshot initializer sanitizes usage breakdown
#[test]
fn snapshot_initializer_sanitizes_usage_breakdown() {
    let snapshot = models_snapshot(
        vec![models_day(
            "2026-04-30",
            vec![
                OpenAIDashboardServiceUsage::new("CLI", 4.0),
                OpenAIDashboardServiceUsage::new("Skillusage:pdf Renderer", 6.0),
            ],
            10.0,
        )],
        Timestamp::from_second(1_700_000_000).unwrap(),
        None,
        None,
    );
    assert_eq!(
        snapshot.usage_breakdown,
        [models_day(
            "2026-04-30",
            vec![OpenAIDashboardServiceUsage::new("CLI", 4.0)],
            4.0
        )]
    );
}

// upstream: OpenAIDashboardModelsTests.swift:76 subscription renewal survives snapshot encoding and Codex projection
#[test]
fn subscription_renewal_survives_snapshot_encoding_and_codex_projection() {
    let renewal: Timestamp = "2026-08-20T12:00:00Z".parse().unwrap();
    let snapshot = models_snapshot(
        vec![],
        renewal,
        Some(RateWindow {
            used_percent: 10.0,
            window_minutes: Some(300),
            resets_at: Some(renewal),
            reset_description: None,
            next_regen_percent: None,
            is_synthetic_placeholder: false,
        }),
        Some(renewal),
    );
    assert_eq!(
        snapshot
            .to_usage_snapshot(ProviderId::new("codex").unwrap(), None, None)
            .unwrap()
            .subscription_renews_at,
        Some(renewal)
    );
    let decoded: OpenAIDashboardSnapshot =
        serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
    assert_eq!(decoded.subscription_renews_at, Some(renewal));
}

// upstream: OpenAIDashboardModelsTests.swift:106 snapshot decoder drops empty zero usage buckets
#[test]
fn snapshot_decoder_drops_empty_zero_usage_buckets() {
    let json = r#"{
        "signedInEmail":"codex@example.com", "codeReviewRemainingPercent":null,
        "creditEvents":[], "dailyBreakdown":[],
        "usageBreakdown":[
            {"day":"2026-04-30","services":[],"totalCreditsUsed":0},
            {"day":"2026-04-29","services":[],"totalCreditsUsed":4}
        ],
        "creditsPurchaseURL":null,"updatedAt":"2026-04-30T19:27:07Z"
    }"#;
    let snapshot: OpenAIDashboardSnapshot = serde_json::from_str(json).unwrap();
    assert_eq!(
        snapshot.usage_breakdown,
        [models_day("2026-04-29", vec![], 4.0)]
    );
}

// upstream: OpenAIDashboardModelsTests.swift:135 recent credit totals use calendar days and exclude future rows
#[test]
fn recent_credit_totals_use_calendar_days_and_exclude_future_rows() {
    let summary = OpenAIDashboardDailyBreakdown::recent_usage_summary(
        &[
            models_day("2026-05-31", vec![], 100.0),
            models_day("2026-06-01", vec![], 1.0),
            models_day("2026-06-29", vec![], 2.0),
            models_day("2026-06-30", vec![], 3.0),
            models_day("2026-07-01", vec![], 200.0),
        ],
        30,
        models_now(),
        &TimeZone::UTC,
    );
    assert_eq!(summary.history_days, 30);
    assert_eq!(summary.today_credits, Some(3.0));
    assert_eq!(summary.total_credits, Some(6.0));
    assert_eq!(
        models_days(&summary),
        ["2026-06-01", "2026-06-29", "2026-06-30"]
    );
}

// upstream: OpenAIDashboardModelsTests.swift:155 recent credit totals preserve gaps and sanitize invalid values
#[test]
fn recent_credit_totals_preserve_gaps_and_sanitize_invalid_values() {
    let summary = OpenAIDashboardDailyBreakdown::recent_usage_summary(
        &[
            models_day(
                "2026-06-20",
                vec![
                    OpenAIDashboardServiceUsage::new("CLI", 4.0),
                    OpenAIDashboardServiceUsage::new("bad", f64::NAN),
                    OpenAIDashboardServiceUsage::new("negative", -2.0),
                ],
                999.0,
            ),
            models_day("2026-06-31", vec![], 9.0),
            models_day("2026-06-30", vec![], f64::INFINITY),
        ],
        30,
        models_now(),
        &TimeZone::UTC,
    );
    assert_eq!(summary.today_credits, Some(0.0));
    assert_eq!(summary.total_credits, Some(4.0));
    let day = summary.daily.first().unwrap();
    assert_eq!(day.day, "2026-06-20");
    assert_eq!(day.total_credits_used, 4.0);
    assert_eq!(
        day.services
            .iter()
            .map(|s| s.service.as_str())
            .collect::<Vec<_>>(),
        ["CLI"]
    );
}

// upstream: OpenAIDashboardModelsTests.swift:181 recent credit totals report zero when history has no row for today
#[test]
fn recent_credit_totals_report_zero_when_history_has_no_row_for_today() {
    let summary = OpenAIDashboardDailyBreakdown::recent_usage_summary(
        &[models_day("2026-06-29", vec![], 4.0)],
        30,
        models_now(),
        &TimeZone::UTC,
    );
    assert_eq!(summary.today_credits, Some(0.0));
    assert_eq!(summary.total_credits, Some(4.0));
    assert_eq!(models_days(&summary), ["2026-06-29"]);
}

// upstream: OpenAIDashboardModelsTests.swift:195 recent credit totals fail closed on overflow
#[test]
fn recent_credit_totals_fail_closed_on_overflow() {
    let summary = OpenAIDashboardDailyBreakdown::recent_usage_summary(
        &[models_day(
            "2026-06-30",
            vec![
                OpenAIDashboardServiceUsage::new("CLI", f64::MAX),
                OpenAIDashboardServiceUsage::new("Desktop App", f64::MAX),
            ],
            1.0,
        )],
        30,
        models_now(),
        &TimeZone::UTC,
    );
    assert!(summary.daily.is_empty());
    assert_eq!(summary.today_credits, None);
    assert_eq!(summary.total_credits, None);
}

// upstream: OpenAIDashboardModelsTests.swift:215 recent credit totals respect the selected timezone
#[test]
fn recent_credit_totals_respect_the_selected_timezone() {
    let pacific = TimeZone::get("America/Los_Angeles").unwrap();
    let now: Timestamp = "2026-07-01T06:30:00Z".parse().unwrap();
    let summary = OpenAIDashboardDailyBreakdown::recent_usage_summary(
        &[
            models_day("2026-06-30", vec![], 7.0),
            models_day("2026-07-01", vec![], 11.0),
        ],
        30,
        now,
        &pacific,
    );
    assert_eq!(summary.today_credits, Some(7.0));
    assert_eq!(summary.total_credits, Some(7.0));
    assert_eq!(models_days(&summary), ["2026-06-30"]);
}

// partial-upstream: OpenAIDashboardModelsTests.swift:234 recent credit totals keep Gregorian dashboard keys with a non Gregorian system calendar
#[test]
fn local_recent_credit_totals_use_gregorian_dashboard_keys() {
    // Partial: Jiff accepts a timezone and always uses the Gregorian civil
    // calendar; there is no Buddhist system-calendar argument to exercise.
    let summary = OpenAIDashboardDailyBreakdown::recent_usage_summary(
        &[models_day("2026-06-30", vec![], 7.0)],
        30,
        models_now(),
        &TimeZone::UTC,
    );
    assert_eq!(summary.today_credits, Some(7.0));
    assert_eq!(summary.total_credits, Some(7.0));
}

// ============================================================================
// Suite 10: OpenAIDashboardIdentityMergeTests.swift
// ============================================================================

fn identity_window(used: f64) -> RateWindow {
    RateWindow {
        used_percent: used,
        window_minutes: Some(300),
        resets_at: None,
        reset_description: None,
        next_regen_percent: None,
        is_synthetic_placeholder: false,
    }
}

fn identity_api_data(balance: Option<f64>) -> DashboardAPIData {
    DashboardAPIData {
        account_id: Some("workspace-a".into()),
        primary_limit: Some(identity_window(12.0)),
        secondary_limit: None,
        extra_rate_windows: Some(vec![]),
        credits_remaining: balance,
        credits_available: balance.map(|_| true),
        balance_is_workspace: balance.map(|_| true),
        codex_credit_limit: None,
        account_plan: Some("business".into()),
    }
}

fn identity_incoming(email: Option<&str>, account_id: Option<&str>) -> OpenAIDashboardSnapshot {
    OpenAIDashboardSnapshot {
        signed_in_email: email.map(str::to_owned),
        account_id: account_id.map(str::to_owned),
        primary_limit: Some(identity_window(12.0)),
        updated_at: Timestamp::from_second(1_700_000_100).unwrap(),
        ..Default::default()
    }
}

fn identity_previous(
    email: Option<&str>,
    account_id: Option<&str>,
    workspace: Option<bool>,
) -> OpenAIDashboardSnapshot {
    let date = Timestamp::from_second(1_700_000_000).unwrap();
    let history = OpenAIDashboardDailyBreakdown::new(
        "2023-11-14",
        vec![OpenAIDashboardServiceUsage::new("Codex", 2.0)],
        2.0,
    );
    OpenAIDashboardSnapshot {
        signed_in_email: email.map(str::to_owned),
        account_id: account_id.map(str::to_owned),
        code_review_remaining_percent: Some(81.0),
        code_review_limit: Some(identity_window(19.0)),
        credit_events: vec![CreditEvent::new(date, "Codex", 2.0)],
        daily_breakdown: vec![history.clone()],
        usage_breakdown: vec![history],
        credits_purchase_url: Some("https://chatgpt.com/checkout".into()),
        primary_limit: Some(identity_window(99.0)),
        secondary_limit: Some(identity_window(98.0)),
        credits_remaining: Some(1234.0),
        credits_available: Some(true),
        balance_is_workspace: workspace,
        codex_credit_limit: Some(CodexCreditLimitSnapshot::new_monthly(
            300.0, 400.0, 25.0, None, date,
        )),
        account_plan: Some("Previous plan".into()),
        subscription_expires_at: Some(Timestamp::from_second(1_700_000_000 + 3600).unwrap()),
        subscription_renews_at: Some(Timestamp::from_second(1_700_000_000 + 7200).unwrap()),
        updated_at: date,
        ..Default::default()
    }
}

fn identity_expect_no_cached_fields(result: &OpenAIDashboardSnapshot) {
    assert!(result.credit_events.is_empty());
    assert!(result.daily_breakdown.is_empty());
    assert!(result.usage_breakdown.is_empty());
    assert_eq!(result.code_review_remaining_percent, None);
    assert_eq!(result.code_review_limit, None);
    assert_eq!(result.credits_purchase_url, None);
    assert_eq!(result.secondary_limit, None);
    assert_eq!(result.extra_rate_windows, None);
    assert_eq!(result.credits_remaining, None);
    assert_eq!(result.credits_available, None);
    assert_eq!(result.balance_is_workspace, None);
    assert_eq!(result.codex_credit_limit, None);
    assert_eq!(result.subscription_expires_at, None);
    assert_eq!(result.subscription_renews_at, None);
}

fn identity_merge_api(
    api: &DashboardAPIData,
    verified: &str,
    previous: &OpenAIDashboardSnapshot,
) -> OpenAIDashboardSnapshot {
    OpenAIDashboardFetcher::snapshot_by_merging_api(
        api,
        Some(previous),
        Some(verified),
        OpenAISubscriptionFetchResult::Unavailable,
        Timestamp::now(),
    )
}

fn identity_fill_page(
    snapshot: OpenAIDashboardSnapshot,
    previous: Option<&OpenAIDashboardSnapshot>,
) -> OpenAIDashboardSnapshot {
    OpenAIDashboardFetcher::filling_missing_page_fields(
        snapshot,
        previous,
        OpenAISubscriptionFetchResult::Unavailable,
    )
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:8 legacy personal dashboard fields survive page and API refreshes without account IDs
#[test]
fn legacy_personal_dashboard_fields_survive_page_and_api_refreshes_without_account_ids() {
    for account_id in [None, Some(""), Some("workspace-a")] {
        for workspace in [None, Some(false)] {
            let previous = identity_previous(Some("owner@example.com"), account_id, workspace);
            let api = identity_merge_api(&identity_api_data(None), "owner@example.com", &previous);
            let page = identity_fill_page(
                identity_incoming(Some("owner@example.com"), None),
                Some(&previous),
            );
            for result in [api, page] {
                assert_eq!(result.credits_remaining, previous.credits_remaining);
                assert_eq!(result.credit_events, previous.credit_events);
                assert_eq!(result.daily_breakdown, previous.daily_breakdown);
                assert_eq!(result.usage_breakdown, previous.usage_breakdown);
                assert_eq!(result.codex_credit_limit, previous.codex_credit_limit);
                assert_eq!(result.secondary_limit, previous.secondary_limit);
                assert_eq!(
                    result.subscription_expires_at,
                    previous.subscription_expires_at
                );
                assert_eq!(
                    result.subscription_renews_at,
                    previous.subscription_renews_at
                );
                assert!(!result.requires_workspace_balance_scope());
            }
        }
    }
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:30 unidentified page waits without inheriting the API identity
#[test]
fn unidentified_page_waits_without_inheriting_the_api_identity() {
    for page_email in [None, Some(""), Some(" \n ")] {
        assert!(OpenAIDashboardFetcher::should_wait_for_page_identity(
            Some("owner@example.com"),
            page_email
        ));
        assert!(!OpenAIDashboardFetcher::should_wait_for_page_identity(
            Some("owner@example.com"),
            Some("other@example.com")
        ));
    }
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:38 API usage excludes all cached fields from an unpaired identity
#[test]
fn api_usage_excludes_all_cached_fields_from_an_unpaired_identity() {
    for previous_email in [None, Some(""), Some("other@example.com")] {
        let result = identity_merge_api(
            &identity_api_data(None),
            "owner@example.com",
            &identity_previous(previous_email, Some("workspace-a"), Some(true)),
        );
        assert_eq!(result.signed_in_email.as_deref(), Some("owner@example.com"));
        assert_eq!(
            result.primary_limit.as_ref().map(|w| w.used_percent),
            Some(12.0)
        );
        assert_eq!(result.account_plan.as_deref(), Some("business"));
        identity_expect_no_cached_fields(&result);
    }
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:51 API merge cannot use a cached email as its verified identity
#[test]
fn api_merge_cannot_use_a_cached_email_as_its_verified_identity() {
    for verified in ["", " \n "] {
        let result = identity_merge_api(
            &identity_api_data(None),
            verified,
            &identity_previous(Some("owner@example.com"), Some("workspace-a"), Some(true)),
        );
        assert!(
            result
                .signed_in_email
                .as_deref()
                .is_none_or(|v| v.trim().is_empty())
        );
        identity_expect_no_cached_fields(&result);
    }
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:62 page merge cannot infer an identity or copy another accounts data
#[test]
fn page_merge_cannot_infer_an_identity_or_copy_another_accounts_data() {
    for scenario in [
        "different",
        "missing incoming",
        "missing previous",
        "both missing",
        "blank incoming",
    ] {
        let current = match scenario {
            "missing incoming" | "both missing" => None,
            "blank incoming" => Some(" \n "),
            _ => Some("owner@example.com"),
        };
        let previous = match scenario {
            "missing previous" | "both missing" => None,
            _ => Some("other@example.com"),
        };
        let incoming = identity_incoming(current, Some("workspace-a"));
        assert_eq!(
            identity_fill_page(
                incoming.clone(),
                Some(&identity_previous(
                    previous,
                    Some("workspace-a"),
                    Some(true)
                )),
            ),
            incoming
        );
    }
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:81 normalized matching identities preserve history and independently missing fields
#[test]
fn normalized_matching_identities_preserve_history_and_independently_missing_fields() {
    let previous = identity_previous(
        Some(" Owner@Example.COM \n"),
        Some("workspace-a"),
        Some(true),
    )
    .with_subscription_metadata(None);
    let api = identity_merge_api(&identity_api_data(None), "owner@example.com", &previous);
    let page = identity_fill_page(
        identity_incoming(Some("owner@example.com"), Some("workspace-a")),
        Some(&previous),
    );
    for result in [&api, &page] {
        assert_eq!(result.signed_in_email.as_deref(), Some("owner@example.com"));
        assert_eq!(
            result.primary_limit.as_ref().map(|w| w.used_percent),
            Some(12.0)
        );
        assert_eq!(result.credit_events, previous.credit_events);
        assert_eq!(result.daily_breakdown, previous.daily_breakdown);
        assert_eq!(result.usage_breakdown, previous.usage_breakdown);
        assert_eq!(result.credits_remaining, Some(1234.0));
        assert_eq!(result.balance_is_workspace, Some(true));
        assert_eq!(result.codex_credit_limit, previous.codex_credit_limit);
        assert_eq!(result.secondary_limit, previous.secondary_limit);
        assert_eq!(result.credits_purchase_url, previous.credits_purchase_url);
        assert_eq!(
            result.subscription_renews_at,
            previous.subscription_renews_at
        );
    }
    assert_eq!(api.account_plan, previous.account_plan);
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:108 missing page identity waits and mismatched page returns verified API data
#[test]
fn missing_page_identity_waits_and_mismatched_page_returns_verified_api_data() {
    for page_email in [None, Some(""), Some("other@example.com")] {
        let result = OpenAIDashboardFetcher::snapshot_for_unpaired_page(
            Some(&identity_api_data(Some(14.0))),
            Some("owner@example.com"),
            page_email,
            OpenAISubscriptionFetchResult::Unavailable,
            Some(&identity_previous(
                Some("other@example.com"),
                Some("workspace-a"),
                Some(true),
            )),
            Timestamp::now(),
        )
        .unwrap();
        if page_email.is_none_or(str::is_empty) {
            assert_eq!(result, None);
            continue;
        }
        let snapshot = result.unwrap();
        assert_eq!(
            snapshot.signed_in_email.as_deref(),
            Some("owner@example.com")
        );
        assert_eq!(
            snapshot.primary_limit.as_ref().map(|w| w.used_percent),
            Some(12.0)
        );
        assert_eq!(snapshot.credits_remaining, Some(14.0));
        assert_eq!(snapshot.balance_is_workspace, Some(true));
        assert!(snapshot.credit_events.is_empty());
        assert!(snapshot.daily_breakdown.is_empty());
        assert!(snapshot.usage_breakdown.is_empty());
        assert_eq!(snapshot.account_plan.as_deref(), Some("business"));
        assert_eq!(snapshot.code_review_remaining_percent, None);
        assert_eq!(snapshot.credits_purchase_url, None);
        assert_eq!(snapshot.codex_credit_limit, None);
        assert_eq!(snapshot.subscription_renews_at, None);
    }
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:135 matching page identity allows the normal page merge
#[test]
fn matching_page_identity_allows_the_normal_page_merge() {
    assert_eq!(
        OpenAIDashboardFetcher::snapshot_for_unpaired_page(
            Some(&identity_api_data(Some(14.0))),
            Some(" Owner@Example.COM \n"),
            Some("owner@example.com"),
            OpenAISubscriptionFetchResult::Unavailable,
            None,
            Timestamp::now(),
        )
        .unwrap(),
        None
    );
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:146 API only fallback requires independently verified API identity
#[test]
fn api_only_fallback_requires_independently_verified_api_identity() {
    for scenario in ["missing API", "missing verification", "blank verification"] {
        let api = identity_api_data(Some(14.0));
        let verified = match scenario {
            "missing verification" => None,
            "blank verification" => Some(" \n "),
            _ => Some("owner@example.com"),
        };
        assert_eq!(
            OpenAIDashboardFetcher::snapshot_for_unpaired_page(
                (scenario != "missing API").then_some(&api),
                verified,
                Some("other@example.com"),
                OpenAISubscriptionFetchResult::Unavailable,
                Some(&identity_previous(
                    Some("owner@example.com"),
                    Some("workspace-a"),
                    Some(true)
                )),
                Timestamp::now(),
            )
            .unwrap(),
            None
        );
    }
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:158 metadata only API cannot be mixed into an unpaired page
#[test]
fn metadata_only_api_cannot_be_mixed_into_an_unpaired_page() {
    for page_email in [None, Some("other@example.com")] {
        let api = DashboardAPIData {
            account_id: None,
            primary_limit: None,
            secondary_limit: None,
            extra_rate_windows: Some(vec![]),
            credits_remaining: None,
            credits_available: Some(false),
            balance_is_workspace: None,
            codex_credit_limit: None,
            account_plan: Some("business".into()),
        };
        assert!(!api.has_usage_data());
        let result = OpenAIDashboardFetcher::snapshot_for_unpaired_page(
            Some(&api),
            Some("owner@example.com"),
            page_email,
            OpenAISubscriptionFetchResult::Unavailable,
            Some(&identity_previous(
                Some("owner@example.com"),
                Some("workspace-a"),
                Some(true),
            )),
            Timestamp::now(),
        );
        if page_email.is_none() {
            assert_eq!(result.unwrap(), None);
        } else {
            // Rust's typed NoDashboardData carries no response body; its display/debug must not expose either identity.
            let error = result.unwrap_err();
            assert_eq!(error, OpenAIDashboardFetchError::NoDashboardData);
            for text in [error.to_string(), format!("{error:?}")] {
                assert!(!text.contains("owner@example.com"));
                assert!(!text.contains("other@example.com"));
            }
        }
    }
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:244 matching email cannot reuse another or unknown workspace
#[test]
fn matching_email_cannot_reuse_another_or_unknown_workspace() {
    for account_id in [None, Some(""), Some("workspace-b")] {
        let previous = identity_previous(Some("owner@example.com"), account_id, Some(true));
        let persisted: OpenAIDashboardSnapshot = serde_json::from_slice(
            &serde_json::to_vec(&previous.with_subscription_metadata(None)).unwrap(),
        )
        .unwrap();
        let result = identity_merge_api(&identity_api_data(None), "owner@example.com", &persisted);
        identity_expect_no_cached_fields(&result);
        assert_eq!(result.account_id.as_deref(), Some("workspace-a"));
        let incoming = identity_incoming(Some("owner@example.com"), Some("workspace-a"));
        assert_eq!(
            identity_fill_page(incoming.clone(), Some(&persisted)),
            incoming
        );
    }
}

// ============================================================================
// Suite 11: CodexWorkspaceBalanceDashboardTests.swift
// ============================================================================

fn workspace_updated_at() -> Timestamp {
    Timestamp::from_second(1_786_000_000).unwrap()
}

fn workspace_snapshot(
    balance: Option<f64>,
    available: Option<bool>,
    workspace: Option<bool>,
) -> OpenAIDashboardSnapshot {
    OpenAIDashboardSnapshot {
        signed_in_email: Some("user@example.com".into()),
        account_id: Some("workspace-a".into()),
        credits_remaining: balance,
        credits_available: available,
        balance_is_workspace: workspace,
        updated_at: workspace_updated_at(),
        ..Default::default()
    }
}

fn workspace_api_data(
    balance: Option<f64>,
    available: Option<bool>,
    workspace: Option<bool>,
) -> DashboardAPIData {
    DashboardAPIData {
        account_id: Some("workspace-a".into()),
        primary_limit: None,
        secondary_limit: None,
        extra_rate_windows: Some(vec![]),
        credits_remaining: balance,
        credits_available: available,
        balance_is_workspace: workspace,
        codex_credit_limit: None,
        account_plan: Some("business".into()),
    }
}

async fn workspace_fetch_balance_response(
    status: u16,
    payload: &str,
    include_cap: bool,
) -> DashboardAPIData {
    let cap = if include_cap {
        r#", "individual_limit":{"limit":100,"used":100}"#
    } else {
        ""
    };
    let usage = format!(
        r#"{{"account_id":"workspace-123","plan_type":"business","credits":{{"has_credits":true,"unlimited":false,"balance":null}}{cap}}}"#
    );
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());
    let usage_mock = server.mock(|when, then| {
        when.method("GET").path("/backend-api/wham/usage");
        then.status(200).body(usage);
    });
    let balance_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts/workspace-123/remaining_balance")
            .header("ChatGPT-Account-Id", "workspace-123")
            .header("Cookie", "session=fixture");
        then.status(status).body(payload);
    });
    let unexpected = server.mock(|when, then| {
        when.any_request();
        then.status(404);
    });
    let start = Instant::now();
    let deadline = start + Duration::from_secs(10);
    // Partial: the HTTPS server cannot inspect the client's timeoutInterval;
    // no request-timeout observer is exposed by the public NetBroker API.
    let response = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "session=fixture",
        Some(deadline),
        Timestamp::now(),
        &Instant::now,
        Some(&https::call()),
    )
    .await
    .unwrap()
    .unwrap();
    usage_mock.assert_calls(1);
    balance_mock.assert_calls(1);
    unexpected.assert_calls(0);
    response.api_data
}

fn workspace_merge_api(
    api: &DashboardAPIData,
    previous: Option<&OpenAIDashboardSnapshot>,
) -> OpenAIDashboardSnapshot {
    OpenAIDashboardFetcher::snapshot_by_merging_api(
        api,
        previous,
        Some("user@example.com"),
        OpenAISubscriptionFetchResult::Unavailable,
        Timestamp::now(),
    )
}

// partial-upstream: CodexWorkspaceBalanceDashboardTests.swift:83 owner balance remains distinct from an exhausted personal cap
#[tokio::test]
async fn local_owner_balance_remains_distinct_from_an_exhausted_personal_cap() {
    for balance in [0.0, 1234.0] {
        let api =
            workspace_fetch_balance_response(200, &format!(r#"{{"balance":{balance:.1}}}"#), true)
                .await;
        let dashboard = workspace_merge_api(&api, None);
        let credits = dashboard.to_credits_snapshot().unwrap();
        assert_eq!(
            credits.codex_credit_limit.as_ref().map(|l| l.remaining),
            Some(0.0)
        );
        assert!(credits.balance_is_workspace);
        assert!(credits.balance_read_succeeded);
        assert_eq!(credits.display_remaining(), Some(balance));
    }
}

// partial-upstream: CodexWorkspaceBalanceDashboardTests.swift:101 unavailable owner endpoint retains hidden credits without rate windows
#[tokio::test]
async fn local_unavailable_owner_endpoint_retains_hidden_credits_without_rate_windows() {
    for status in [403, 500] {
        let api =
            workspace_fetch_balance_response(status, r#"{"error":"unavailable"}"#, false).await;
        let dashboard = workspace_merge_api(
            &api,
            Some(&workspace_snapshot(Some(1234.0), Some(true), Some(true))),
        );
        let credits = dashboard.to_credits_snapshot().unwrap();
        assert!(api.has_usage_data());
        assert_eq!(credits.credits_available, Some(true));
        assert!(!credits.balance_read_succeeded);
        assert!(!credits.balance_is_workspace);
        assert_eq!(credits.display_remaining(), None);
    }
}

// partial-upstream: CodexWorkspaceBalanceDashboardTests.swift:117 missing balance payload never revives a cached workspace amount
#[tokio::test]
async fn local_missing_balance_payload_never_revives_a_cached_workspace_amount() {
    let api = workspace_fetch_balance_response(200, r#"{"balance":null}"#, false).await;
    let previous = workspace_snapshot(Some(1234.0), Some(true), Some(true));
    let current = workspace_merge_api(&api, Some(&previous));
    let filled = OpenAIDashboardFetcher::filling_missing_page_fields(
        current,
        Some(&previous),
        OpenAISubscriptionFetchResult::Unavailable,
    );
    assert_eq!(filled.credits_remaining, None);
    assert_eq!(filled.credits_available, Some(true));
    assert_ne!(filled.balance_is_workspace, Some(true));
}

// upstream: CodexWorkspaceBalanceDashboardTests.swift:133 merges keep provenance with the selected numeric balance
#[test]
fn merges_keep_provenance_with_the_selected_numeric_balance() {
    let previous = workspace_snapshot(Some(1234.0), Some(true), Some(true));
    let inherited = workspace_merge_api(&workspace_api_data(None, None, None), Some(&previous));
    let replaced = workspace_merge_api(
        &workspace_api_data(Some(12.0), Some(true), None),
        Some(&previous),
    );
    let page_replaced = OpenAIDashboardFetcher::filling_missing_page_fields(
        workspace_snapshot(Some(12.0), None, None),
        Some(&previous),
        OpenAISubscriptionFetchResult::Unavailable,
    );
    assert_eq!(inherited.credits_remaining, Some(1234.0));
    assert_eq!(inherited.balance_is_workspace, Some(true));
    assert_eq!(replaced.credits_remaining, Some(12.0));
    assert_ne!(replaced.balance_is_workspace, Some(true));
    assert_eq!(page_replaced.credits_remaining, Some(12.0));
    assert_ne!(page_replaced.balance_is_workspace, Some(true));
}

// upstream: CodexWorkspaceBalanceDashboardTests.swift:156 page placeholder cannot replace the API hidden balance state
#[test]
fn page_placeholder_cannot_replace_the_api_hidden_balance_state() {
    let scrape = DashboardScrapePayload {
        login_required: false,
        body_text: "Credits remaining 0".into(),
        signed_in_email: Some("user@example.com".into()),
        has_credits_remaining: true,
        ..Default::default()
    };
    assert_eq!(
        OpenAIDashboardParser::parse_credits_remaining(&scrape.body_text),
        Some(0.0)
    );
    // Rust combines parseDashboardScrape and makePageSnapshot/filling in one
    // public parser call; WebView viewport/scroll-only fields are not inputs.
    let page = OpenAIDashboardFetcher::parse_dashboard_scrape_with_api(
        &scrape,
        Some(&workspace_api_data(None, Some(true), None)),
        Some(&workspace_snapshot(Some(1234.0), Some(true), Some(true))),
        workspace_updated_at(),
    )
    .unwrap();
    assert!(OpenAIDashboardFetcher::has_returnable_dashboard_data(&page));
    assert_eq!(page.credits_remaining, None);
    assert_eq!(
        page.to_credits_snapshot().map(|c| c.balance_read_succeeded),
        Some(false)
    );
}

// upstream: CodexWorkspaceBalanceDashboardTests.swift:194 dashboard cache preserves provenance and accepts older snapshots
#[test]
fn dashboard_cache_preserves_provenance_and_accepts_older_snapshots() {
    let original = workspace_snapshot(Some(1234.0), Some(true), Some(true));
    let data = serde_json::to_vec(&original).unwrap();
    let restored: OpenAIDashboardSnapshot = serde_json::from_slice(&data).unwrap();
    assert_eq!(restored, original);
    assert_eq!(
        restored
            .with_subscription_metadata(None)
            .balance_is_workspace,
        Some(true)
    );
    let mut legacy: serde_json::Value = serde_json::from_slice(&data).unwrap();
    legacy.as_object_mut().unwrap().remove("creditsAvailable");
    legacy.as_object_mut().unwrap().remove("balanceIsWorkspace");
    let decoded: OpenAIDashboardSnapshot = serde_json::from_value(legacy).unwrap();
    assert_eq!(decoded.credits_remaining, Some(1234.0));
    assert_eq!(decoded.credits_available, None);
    assert_eq!(
        decoded
            .to_credits_snapshot()
            .map(|c| c.balance_is_workspace),
        Some(false)
    );
}

// ============================================================================
// Suite 12: OpenAISubscriptionMetadataTests.swift
// ============================================================================

// upstream: OpenAISubscriptionMetadataTests.swift:7 test_mapsRenewingSubscriptionToRenewalDate
#[test]
fn test_maps_renewing_subscription_to_renewal_date() {
    let data = br#"{
      "subscriptions": [
        {
          "active_until": "2026-04-21T07:55:10Z",
          "will_renew": true
        }
      ]
    }"#;

    let metadata = OpenAISubscriptionMetadataParser::parse(data).unwrap();
    let expected = Timestamp::from_second(1_776_758_110).unwrap();

    assert_eq!(metadata.subscription_renews_at, Some(expected));
    assert_eq!(metadata.subscription_expires_at, None);
    assert!(!metadata.subscription_renews_at_is_date_only);
}

// upstream: OpenAISubscriptionMetadataTests.swift:16 test_mapsNonRenewingSubscriptionToExpirationDate
#[test]
fn test_maps_non_renewing_subscription_to_expiration_date() {
    let data = br#"{
      "active_until": "2026-04-21T07:55:10Z",
      "will_renew": false
    }"#;

    let metadata = OpenAISubscriptionMetadataParser::parse(data).unwrap();
    let expected = Timestamp::from_second(1_776_758_110).unwrap();

    assert_eq!(metadata.subscription_expires_at, Some(expected));
    assert_eq!(metadata.subscription_renews_at, None);
    assert!(!metadata.subscription_expires_at_is_date_only);
}

// upstream: OpenAISubscriptionMetadataTests.swift:25 test_parsesFractionalISO8601Date
#[test]
fn test_parses_fractional_iso8601_date() {
    let data = br#"{
      "activeUntil": "2026-04-21T07:55:10.500Z",
      "willRenew": true
    }"#;

    let metadata = OpenAISubscriptionMetadataParser::parse(data).unwrap();
    let expected: Timestamp = "2026-04-21T07:55:10.500Z".parse().unwrap();

    assert_eq!(metadata.subscription_renews_at, Some(expected));
    assert_eq!(metadata.subscription_expires_at, None);
}

// upstream: OpenAISubscriptionMetadataTests.swift:34 test_rejectsMissingOrMalformedMetadata
#[test]
fn test_rejects_missing_or_malformed_metadata() {
    for raw in [
        &b"{}"[..],
        &br#"{"active_until":"not-a-date","will_renew":true}"#[..],
        &br#"{"active_until":"2026-04-21T07:55:10Z"}"#[..],
        &br#"{"will_renew":false}"#[..],
        &b"not-json"[..],
    ] {
        assert_eq!(OpenAISubscriptionMetadataParser::parse(raw), None);
        assert_eq!(
            OpenAISubscriptionMetadataParser::parse_result(raw),
            OpenAISubscriptionFetchResult::Unavailable
        );
    }
}

// upstream: OpenAISubscriptionMetadataTests.swift:40 test_numericRenewalFlagsAreUnavailableRatherThanEmpty
#[test]
fn test_numeric_renewal_flags_are_unavailable_rather_than_empty() {
    for raw in [
        &br#"{"active_until":"2026-04-21T07:55:10Z","will_renew":1}"#[..],
        &br#"{"active_until":null,"will_renew":0}"#[..],
    ] {
        assert_eq!(
            OpenAISubscriptionMetadataParser::parse_result(raw),
            OpenAISubscriptionFetchResult::Unavailable
        );
    }
}

// ============================================================================
// Security Tests (Required by Brief)
// ============================================================================

#[tokio::test]
async fn cookie_import_opt_in_only() {
    let tmp = TempDir::new().unwrap();
    let cache_store = OpenAIDashboardCacheStore::with_file_path(tmp.path().join("cache.json"));
    let empty_home = tmp.path().join("empty-home");
    fs::create_dir_all(&empty_home).unwrap();

    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let usage_mock = server.mock(|when, then| {
        when.method("GET").path("/backend-api/wham/usage");
        then.status(200).body(r#"{}"#);
    });

    let candidate = OpenAIDashboardCookieCandidate::from_cookie_header(
        "Chrome Default",
        "__Secure-next-auth.session-token=secret-cookie",
    )
    .unwrap();
    let net = Arc::new(https::broker(NetConfig::default()));

    // 1. cookie_source = None -> unavailable in Web mode, never touches candidates or network
    let config_none =
        CodexStrategyConfig::new(Arc::clone(&net), BTreeMap::new(), &empty_home, &empty_home)
            .with_cookie_source(None)
            .with_dashboard_endpoint_config(endpoint.clone())
            .with_cookie_importer(Arc::new(OpenAIDashboardBrowserCookieImporter::new(
                Arc::clone(&net),
                endpoint.clone(),
                Arc::new(CookieCache::new()),
                OpenAIDashboardCandidateSource::StaticCandidates(vec![candidate.clone()]),
            )))
            .with_dashboard_cache_store(cache_store.clone());

    let strategy_none = CodexWebDashboardStrategy::with_config(config_none);
    let ctx_web = FetchContext {
        provider: ProviderId::new("codex").unwrap(),
        runtime: FetchRuntime::Cli,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "test-req".into(),
        source_mode: SourceMode::Web,
        env: BTreeMap::new(),
        include_credits: true,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    };
    assert!(!strategy_none.is_available(&ctx_web).await);

    // 2. cookie_source = Some(CookieSource::Off) -> unavailable in Web and Auto modes, never imports
    let config_off =
        CodexStrategyConfig::new(Arc::clone(&net), BTreeMap::new(), &empty_home, &empty_home)
            .with_cookie_source(Some(CookieSource::Off))
            .with_dashboard_endpoint_config(endpoint.clone())
            .with_cookie_importer(Arc::new(OpenAIDashboardBrowserCookieImporter::new(
                Arc::clone(&net),
                endpoint.clone(),
                Arc::new(CookieCache::new()),
                OpenAIDashboardCandidateSource::StaticCandidates(vec![candidate]),
            )))
            .with_dashboard_cache_store(cache_store);

    let strategy_off = CodexWebDashboardStrategy::with_config(config_off);
    assert!(!strategy_off.is_available(&ctx_web).await);
    let ctx_auto = FetchContext {
        source_mode: SourceMode::Auto,
        ..ctx_web
    };
    assert!(!strategy_off.is_available(&ctx_auto).await);

    // Importer directly with None / Off also returns NoCookiesFound without making network calls
    let importer = OpenAIDashboardBrowserCookieImporter::new(
        net,
        endpoint,
        Arc::new(CookieCache::new()),
        OpenAIDashboardCandidateSource::None,
    );
    assert_eq!(
        importer
            .import_for_cookie_source(
                &https::call(),
                None,
                None,
                Some("user@example.com"),
                true,
                true,
                None,
                None,
            )
            .await,
        Err(OpenAIDashboardCookieImportError::NoCookiesFound)
    );
    assert_eq!(
        importer
            .import_for_cookie_source(
                &https::call(),
                Some(CookieSource::Off),
                None,
                Some("user@example.com"),
                true,
                true,
                None,
                None,
            )
            .await,
        Err(OpenAIDashboardCookieImportError::NoCookiesFound)
    );
    usage_mock.assert_calls(0);
}

#[test]
fn hostile_urls_rejected_without_network_calls() {
    for hostile in [
        "https://evil.example.com",
        "http://chatgpt.com",
        "http://evil.example.com/backend-api/wham/usage",
        "https://user:pass@chatgpt.com",
        "https://chatgpt.com?leak=secret",
        "https://chatgpt.com#frag",
        "file:///etc/passwd",
        "ftp://chatgpt.com",
    ] {
        assert!(
            OpenAIDashboardEndpointConfig::from_base_url_str(hostile).is_err(),
            "expected hostile URL {hostile:?} to be rejected"
        );
    }

    // Valid default and loopback URLs succeed
    assert!(OpenAIDashboardEndpointConfig::from_base_url_str("https://chatgpt.com").is_ok());
    assert!(OpenAIDashboardEndpointConfig::from_base_url_str("https://127.0.0.1:8443").is_ok());
    assert!(OpenAIDashboardEndpointConfig::from_base_url_str("http://127.0.0.1:8080").is_ok());
}

#[tokio::test]
async fn error_strings_and_debug_redact_bodies_tokens_cookies_and_emails_across_all_six_endpoints()
{
    const BODY_SENTINEL: &str = "SUPER_SECRET_DASHBOARD_HTTP_BODY_SENTINEL_998877";
    const COOKIE_SENTINEL: &str = "SUPER_SECRET_COOKIE_TOKEN_SENTINEL_112233";
    const BEARER_SENTINEL: &str = "SUPER_SECRET_BEARER_TOKEN_SENTINEL_445566";
    const EMAIL_SENTINEL: &str = "secret-victim-user@example.com";

    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    server.mock(|when, then| {
        when.method("GET");
        then.status(502).body(format!(
            r#"{{"error":"{BODY_SENTINEL}","cookie":"{COOKIE_SENTINEL}","token":"{BEARER_SENTINEL}","email":"{EMAIL_SENTINEL}"}}"#
        ));
    });

    let cookie_header = format!("__Secure-next-auth.session-token={COOKIE_SENTINEL}");
    let now = Timestamp::from_second(1_776_000_000).unwrap();

    // 1. /backend-api/wham/usage
    let (usage_bytes, usage_status, usage_auth) =
        OpenAIDashboardFetcher::fetch_dashboard_usage_response(
            &broker,
            &endpoint,
            &cookie_header,
            None,
            &Instant::now,
            Some(&https::call()),
        )
        .await
        .unwrap();
    assert_eq!(usage_status, 502);
    assert!(usage_auth.is_none());
    assert!(!usage_bytes.is_empty());

    let fetch_err = OpenAIDashboardFetchError::ServerError {
        status: usage_status,
    };
    let classified = fetch_err.clone().into_classified_error();
    for text in [
        fetch_err.to_string(),
        format!("{fetch_err:?}"),
        classified.to_string(),
        format!("{classified:?}"),
    ] {
        assert!(!text.contains(BODY_SENTINEL), "leaked body in {text}");
        assert!(!text.contains(COOKIE_SENTINEL), "leaked cookie in {text}");
        assert!(!text.contains(BEARER_SENTINEL), "leaked bearer in {text}");
        assert!(!text.contains(EMAIL_SENTINEL), "leaked email in {text}");
    }

    // 2. /api/auth/session
    let session_res = OpenAIDashboardFetcher::fetch_dashboard_session_authorization(
        &broker,
        &endpoint,
        &cookie_header,
        None,
        &Instant::now,
        Some(&https::call()),
    )
    .await
    .unwrap();
    assert_eq!(session_res, None);

    // 3. /backend-api/accounts/{id}/spend-controls/current-user/monthly-usage
    // 4. /backend-api/accounts/{id}/remaining_balance
    let dummy_usage: CodexUsageResponse = serde_json::from_str(
        r#"{
          "account_id": "acct-sec",
          "plan_type": "team",
          "credits": {
            "has_credits": true,
            "unlimited": false,
            "balance": null
          }
        }"#,
    )
    .unwrap();
    let monthly_res = OpenAIDashboardFetcher::fetch_spend_controls_monthly_usage_if_needed(
        &broker,
        &endpoint,
        &dummy_usage,
        &cookie_header,
        Some(BEARER_SENTINEL),
        None,
        now,
        &Instant::now,
        Some(&https::call()),
    )
    .await
    .unwrap();
    assert_eq!(monthly_res, None);

    let balance_res = OpenAIDashboardFetcher::fetch_workspace_remaining_balance_if_needed(
        &broker,
        &endpoint,
        &dummy_usage,
        &cookie_header,
        Some(BEARER_SENTINEL),
        None,
        &Instant::now,
        Some(&https::call()),
    )
    .await
    .unwrap();
    assert_eq!(balance_res, None);

    // 5. /backend-api/subscriptions
    let sub_res = OpenAIDashboardFetcher::fetch_subscription_from_api(
        &broker,
        &endpoint,
        &cookie_header,
        Some("acct-sec"),
        None,
        &Instant::now,
        Some(&https::call()),
    )
    .await;
    assert_eq!(sub_res, OpenAISubscriptionFetchResult::Unavailable);

    // 6. /backend-api/me
    let me_res = OpenAIDashboardFetcher::fetch_signed_in_email_from_api(
        &broker,
        &endpoint,
        &cookie_header,
        None,
        &Instant::now,
        Some(&https::call()),
    )
    .await
    .unwrap();
    assert_eq!(me_res, None);

    // Verify Debug redaction on SessionAuthorization, OpenAIDashboardCookieImportResult, FoundAccount, and OpenAIDashboardCookieImportError
    let auth_debug = format!(
        "{:?}",
        SessionAuthorization {
            bearer_token: SecretString::from(BEARER_SENTINEL),
            verified_email: EMAIL_SENTINEL.to_owned(),
        }
    );
    assert!(!auth_debug.contains(BEARER_SENTINEL));
    assert!(!auth_debug.contains(EMAIL_SENTINEL));

    let import_res_debug = format!(
        "{:?}",
        OpenAIDashboardCookieImportResult {
            source_label: "Manual".to_owned(),
            cookie_count: 2,
            signed_in_email: Some(EMAIL_SENTINEL.to_owned()),
            matches_codex_email: true,
            cookie_header: SecretString::from(COOKIE_SENTINEL),
        }
    );
    assert!(!import_res_debug.contains(COOKIE_SENTINEL));
    assert!(!import_res_debug.contains(EMAIL_SENTINEL));

    let mismatch_err_debug = format!(
        "{:?}",
        OpenAIDashboardCookieImportError::NoMatchingAccount {
            found: vec![FoundAccount::new("Chrome", EMAIL_SENTINEL)],
        }
    );
    assert!(!mismatch_err_debug.contains(EMAIL_SENTINEL));

    // The same invented address must remain absent at every policy/import
    // boundary, including the classified error published in snapshots.
    let policy = OpenAIWebCodexError::PolicyRejected(CodexDashboardAuthorityDecision::new(
        CodexDashboardDisposition::FailClosed,
        CodexDashboardDecisionReason::WrongEmail {
            expected: Some(EMAIL_SENTINEL.to_owned()),
            actual: Some(EMAIL_SENTINEL.to_owned()),
        },
        BTreeSet::new(),
        BTreeSet::new(),
    ));
    let cookie_error = OpenAIDashboardCookieImportError::NoMatchingAccount {
        found: vec![FoundAccount::new("Chrome", EMAIL_SENTINEL)],
    };
    let concise =
        concise_openai_cookie_mismatch_status(&[EMAIL_SENTINEL.to_owned()], Some(EMAIL_SENTINEL));
    let web_wrapper = CodexWebDashboardError::Web(policy.clone());
    let cookie_wrapper = CodexWebDashboardError::CookieImport(cookie_error.to_string());
    let mut diagnostics = vec![
        policy.to_string(),
        format!("{policy:?}"),
        cookie_error.to_string(),
        format!("{cookie_error:?}"),
        concise.to_string(),
        format!("{concise:?}"),
        web_wrapper.to_string(),
        format!("{web_wrapper:?}"),
        cookie_wrapper.to_string(),
        format!("{cookie_wrapper:?}"),
    ];
    for wrapper in [web_wrapper, cookie_wrapper] {
        let error = wrapper.into_fetch_error();
        let ariadusage_core::pipeline::FetchError::Classified(classified) = error else {
            panic!("expected classified error")
        };
        diagnostics.push(classified.to_string());
        diagnostics.push(format!("{classified:?}"));
    }
    for diagnostic in diagnostics {
        assert!(
            !diagnostic.contains(EMAIL_SENTINEL),
            "identifier leaked in {diagnostic}"
        );
    }
}

// ============================================================================
// OpenAIDashboardFetcherCreditsWaitTests.swift: pure merge and metadata cases
// ============================================================================

fn credits_wait_previous() -> OpenAIDashboardSnapshot {
    OpenAIDashboardSnapshot {
        signed_in_email: Some("user@example.com".into()),
        account_id: Some("workspace-fixture".into()),
        updated_at: Timestamp::from_second(1).unwrap(),
        ..Default::default()
    }
}

fn credits_wait_api(
    primary: Option<RateWindow>,
    balance: Option<f64>,
    plan: Option<&str>,
) -> DashboardAPIData {
    DashboardAPIData {
        account_id: Some("workspace-fixture".into()),
        primary_limit: primary,
        secondary_limit: None,
        extra_rate_windows: Some(vec![]),
        credits_remaining: balance,
        credits_available: None,
        balance_is_workspace: None,
        codex_credit_limit: None,
        account_plan: plan.map(str::to_owned),
    }
}

fn credits_wait_merge(
    api: &DashboardAPIData,
    previous: &OpenAIDashboardSnapshot,
    subscription: &OpenAISubscriptionFetchResult,
) -> OpenAIDashboardSnapshot {
    OpenAIDashboardFetcher::snapshot_by_merging_api(
        api,
        Some(previous),
        Some("user@example.com"),
        subscription.clone(),
        Timestamp::from_second(2).unwrap(),
    )
}

// upstream: OpenAIDashboardFetcherCreditsWaitTests.swift:6 subscription enrichment changes only subscription metadata
#[test]
fn subscription_enrichment_changes_only_subscription_metadata() {
    let snapshot = OpenAIDashboardSnapshot {
        signed_in_email: Some("user@example.com".into()),
        code_review_remaining_percent: Some(90.0),
        credits_remaining: Some(10.0),
        account_plan: Some("Pro".into()),
        updated_at: Timestamp::from_second(1000).unwrap(),
        ..Default::default()
    };
    let renewal = Timestamp::from_second(2000).unwrap();
    let enriched = snapshot
        .clone()
        .with_subscription_metadata(Some(OpenAISubscriptionMetadata::new(None, Some(renewal))));
    assert_eq!(enriched.signed_in_email, snapshot.signed_in_email);
    assert_eq!(enriched.credits_remaining, snapshot.credits_remaining);
    assert_eq!(enriched.account_plan, snapshot.account_plan);
    assert_eq!(enriched.updated_at, snapshot.updated_at);
    assert_eq!(enriched.subscription_renews_at, Some(renewal));
    assert_eq!(enriched.subscription_expires_at, None);
}

// upstream: OpenAIDashboardFetcherCreditsWaitTests.swift:375 api merge prefers the page derived plan over the generic api plan
#[test]
fn api_merge_prefers_the_page_derived_plan_over_the_generic_api_plan() {
    let previous = OpenAIDashboardSnapshot {
        account_plan: Some("Pro 5x".into()),
        ..credits_wait_previous()
    };
    let api = credits_wait_api(Some(identity_window(12.0)), None, Some("pro"));
    let snapshot = credits_wait_merge(&api, &previous, &OpenAISubscriptionFetchResult::Unavailable);
    assert_eq!(snapshot.account_plan.as_deref(), Some("Pro 5x"));
}

// upstream: OpenAIDashboardFetcherCreditsWaitTests.swift:443 api snapshot keeps previous credits history and overwrites live usage
#[test]
fn api_snapshot_keeps_previous_credits_history_and_overwrites_live_usage() {
    let history = vec![OpenAIDashboardDailyBreakdown::new(
        "2026-04-19",
        vec![OpenAIDashboardServiceUsage::new("Codex", 2.0)],
        2.0,
    )];
    let previous = OpenAIDashboardSnapshot {
        code_review_remaining_percent: Some(81.0),
        credit_events: vec![CreditEvent::new(
            Timestamp::from_second(1_700_000_000).unwrap(),
            "Codex",
            2.0,
        )],
        daily_breakdown: history.clone(),
        usage_breakdown: history,
        credits_purchase_url: Some("https://chatgpt.com/checkout".into()),
        primary_limit: Some(identity_window(10.0)),
        credits_remaining: Some(10.0),
        ..credits_wait_previous()
    };
    let api = credits_wait_api(Some(identity_window(44.0)), Some(7.5), Some("pro"));
    let subscription =
        OpenAISubscriptionMetadata::parse_fields(Some("2026-08-20T14:30:07Z"), Some(true));
    let snapshot = credits_wait_merge(
        &api,
        &previous,
        &OpenAISubscriptionFetchResult::Resolved(subscription.unwrap()),
    );
    assert_eq!(
        snapshot.signed_in_email.as_deref(),
        Some("user@example.com")
    );
    assert_eq!(
        snapshot.primary_limit.as_ref().map(|w| w.used_percent),
        Some(44.0)
    );
    assert_eq!(snapshot.credits_remaining, Some(7.5));
    assert_eq!(snapshot.account_plan.as_deref(), Some("pro"));
    assert_eq!(snapshot.credit_events.len(), 1);
    assert_eq!(snapshot.daily_breakdown.len(), 1);
    assert_eq!(snapshot.usage_breakdown.len(), 1);
    assert_eq!(snapshot.code_review_remaining_percent, Some(81.0));
    assert_eq!(
        snapshot.credits_purchase_url.as_deref(),
        Some("https://chatgpt.com/checkout")
    );
    assert!(snapshot.subscription_renews_at.is_some());
}

// upstream: OpenAIDashboardFetcherCreditsWaitTests.swift:499 subscription replacement clears the mutually exclusive previous date
#[test]
fn subscription_replacement_clears_the_mutually_exclusive_previous_date() {
    let previous = OpenAIDashboardSnapshot {
        subscription_renews_at: Some(Timestamp::from_second(1_800_000_000).unwrap()),
        ..credits_wait_previous()
    };
    let api = credits_wait_api(None, None, None);
    let subscription =
        OpenAISubscriptionMetadata::parse_fields(Some("2026-08-20T14:30:07Z"), Some(false));
    let snapshot = credits_wait_merge(
        &api,
        &previous,
        &OpenAISubscriptionFetchResult::Resolved(subscription.unwrap()),
    );
    assert!(snapshot.subscription_expires_at.is_some());
    assert_eq!(snapshot.subscription_renews_at, None);
}

// upstream: OpenAIDashboardFetcherCreditsWaitTests.swift:569 successful empty subscription response clears stale dates
#[test]
fn successful_empty_subscription_response_clears_stale_dates() {
    let previous = OpenAIDashboardSnapshot {
        subscription_renews_at: Some(Timestamp::from_second(1_800_000_000).unwrap()),
        ..credits_wait_previous()
    };
    let api = credits_wait_api(None, None, None);
    let snapshot = credits_wait_merge(
        &api,
        &previous,
        &OpenAISubscriptionFetchResult::ConfirmedAbsent,
    );
    assert_eq!(snapshot.subscription_expires_at, None);
    assert_eq!(snapshot.subscription_renews_at, None);
}
