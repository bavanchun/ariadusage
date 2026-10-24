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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
        "ariadusage",
        Some("/opt/homebrew/bin/ariadusage"),
    );

    assert!(hint.contains("Chrome"));
    assert!(hint.contains("ariadusage (/opt/homebrew/bin/ariadusage)"));
    assert!(hint.contains("Full Disk Access"));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:63 profile denial names app bundle for menu refresh
#[test]
fn profile_denial_names_app_bundle_for_menu_refresh() {
    let hint = OpenAIDashboardBrowserCookieImporter::browser_profile_access_hint(
        Browser::Safari,
        false,
        "AriadUsage",
        Some("/Applications/AriadUsage.app/Contents/MacOS/AriadUsage"),
    );

    assert!(hint.contains("Safari"));
    assert!(hint.contains("AriadUsage.app (/Applications/AriadUsage.app)"));
    assert!(hint.contains("Full Disk Access"));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:74 browser cookie timeout remains distinct from permission denial
#[test]
fn browser_cookie_timeout_remains_distinct_from_permission_denial() {
    let error = OpenAIDashboardBrowserCookieImporter::browser_cookie_load_timeout_error(
        Browser::Chrome,
        "AriadUsage",
        Some("/Applications/AriadUsage.app/Contents/MacOS/AriadUsage"),
    );

    let msg = error.to_string();
    assert!(msg.starts_with("Browser cookie loading timed out."));
    assert!(!msg.contains("Browser cookie access denied."));
    assert!(msg.contains("Chrome"));
    assert!(msg.contains("AriadUsage.app (/Applications/AriadUsage.app)"));
    assert!(msg.contains("Keychain"));
    assert!(msg.contains("Full Disk Access"));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:93 shared deadline clamps each local timeout to remaining budget
#[test]
fn shared_deadline_clamps_each_local_timeout_to_remaining_budget() {
    let now = Instant::now();
    let deadline = OpenAIDashboardFetcher::deadline_from(now, Duration::from_secs(3));

    let remaining = OpenAIDashboardBrowserCookieImporter::remaining_timeout(
        Some(deadline),
        Some(Duration::from_secs(6)),
        now + Duration::from_millis(2200),
    )
    .unwrap();

    assert!((remaining.as_secs_f64() - 0.8).abs() < 0.001);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:106 shared deadline preserves smaller local timeout
#[test]
fn shared_deadline_preserves_smaller_local_timeout() {
    let now = Instant::now();
    let deadline = OpenAIDashboardFetcher::deadline_from(now, Duration::from_secs(10));

    let remaining = OpenAIDashboardBrowserCookieImporter::remaining_timeout(
        Some(deadline),
        Some(Duration::from_secs(4)),
        now + Duration::from_secs(2),
    )
    .unwrap();

    assert!((remaining.as_secs_f64() - 4.0).abs() < 0.001);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:119 expired shared deadline throws structured timeout
#[test]
fn expired_shared_deadline_throws_structured_timeout() {
    let now = Instant::now();
    let deadline = OpenAIDashboardFetcher::deadline_from(now, Duration::from_secs(2));

    let err = OpenAIDashboardBrowserCookieImporter::remaining_timeout(
        Some(deadline),
        Some(Duration::from_secs(6)),
        now + Duration::from_millis(2100),
    )
    .unwrap_err();

    let OpenAIDashboardCookieImportError::BrowserCookieLoadTimedOut { details } = err else {
        panic!("expected BrowserCookieLoadTimedOut, got {err:?}");
    };
    assert!(details.contains("deadline"));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:135 blocking browser cookie load cannot exceed shared deadline
#[tokio::test]
async fn blocking_browser_cookie_load_cannot_exceed_shared_deadline() {
    let timed_out = Arc::new(AtomicBool::new(false));
    let timed_out_clone = Arc::clone(&timed_out);
    let started_at = Instant::now();
    let deadline = OpenAIDashboardFetcher::deadline_from(started_at, Duration::from_millis(50));

    let err = OpenAIDashboardBrowserCookieImporter::run_bounded_cookie_load(
        Some(deadline),
        Some(Arc::new(move || {
            timed_out_clone.store(true, Ordering::SeqCst);
        })),
        || {
            std::thread::sleep(Duration::from_millis(300));
            Ok(())
        },
    )
    .await
    .unwrap_err();

    assert_eq!(err, PersistentCookieSessionError::TimedOut);
    assert!(started_at.elapsed() < Duration::from_millis(250));
    assert!(timed_out.load(Ordering::SeqCst));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:167 timeout observer stays silent when operation wins
#[tokio::test]
async fn timeout_observer_stays_silent_when_operation_wins() {
    let timed_out = Arc::new(AtomicBool::new(false));
    let timed_out_clone = Arc::clone(&timed_out);
    let deadline =
        OpenAIDashboardFetcher::deadline_from(Instant::now(), Duration::from_millis(500));

    let value = OpenAIDashboardBrowserCookieImporter::run_bounded_cookie_load(
        Some(deadline),
        Some(Arc::new(move || {
            timed_out_clone.store(true, Ordering::SeqCst);
        })),
        || Ok(42),
    )
    .await
    .unwrap();

    assert_eq!(value, 42);
    assert!(!timed_out.load(Ordering::SeqCst));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:183 bounded cookie loads preserve explicit retry context
#[tokio::test]
async fn bounded_cookie_loads_preserve_explicit_retry_context() {
    let acknowledged_retry = true;
    let deadline =
        OpenAIDashboardFetcher::deadline_from(Instant::now(), Duration::from_millis(500));

    let observed = OpenAIDashboardBrowserCookieImporter::run_bounded_cookie_load(
        Some(deadline),
        None,
        move || Ok(acknowledged_retry),
    )
    .await
    .unwrap();

    assert!(observed);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:208 timed out cookie cache work stays ordered before retry
#[tokio::test]
async fn timed_out_cookie_cache_work_stays_ordered_before_retry() {
    let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();

    let events_first = Arc::clone(&events);
    let first_deadline =
        OpenAIDashboardFetcher::deadline_from(Instant::now(), Duration::from_millis(40));
    let first_res = OpenAIDashboardBrowserCookieImporter::run_bounded_cookie_cache_operation(
        Some(first_deadline),
        move || {
            events_first.lock().unwrap().push("first-start");
            let _ = started_tx.send(());
            let _ = release_rx.recv();
            events_first.lock().unwrap().push("first-end");
            Ok(())
        },
    )
    .await;
    assert_eq!(first_res, Err(PersistentCookieSessionError::TimedOut));
    started_rx.await.unwrap();

    let events_second = Arc::clone(&events);
    let second_deadline =
        OpenAIDashboardFetcher::deadline_from(Instant::now(), Duration::from_millis(500));
    let second_task = tokio::spawn(async move {
        OpenAIDashboardBrowserCookieImporter::run_bounded_cookie_cache_operation(
            Some(second_deadline),
            move || {
                events_second.lock().unwrap().push("second");
                Ok(())
            },
        )
        .await
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(*events.lock().unwrap(), vec!["first-start"]);
    let _ = release_tx.send(());
    second_task.await.unwrap().unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        vec!["first-start", "first-end", "second"]
    );
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:256 slow callback times out before completion
#[tokio::test]
async fn slow_callback_times_out_before_completion() {
    let timed_out = Arc::new(AtomicBool::new(false));
    let timed_out_clone = Arc::clone(&timed_out);
    let started_at = Instant::now();
    let deadline = OpenAIDashboardFetcher::deadline_from(started_at, Duration::from_millis(50));

    let err = OpenAIDashboardBrowserCookieImporter::run_bounded_callback(
        Some(deadline),
        Some(Arc::new(move || {
            timed_out_clone.store(true, Ordering::SeqCst);
        })),
        |complete| {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                complete();
            });
        },
    )
    .await
    .unwrap_err();

    assert_eq!(err, PersistentCookieSessionError::TimedOut);
    assert!(started_at.elapsed() < Duration::from_millis(250));
    assert!(timed_out.load(Ordering::SeqCst));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:278 slow value callback times out before completion
#[tokio::test]
async fn slow_value_callback_times_out_before_completion() {
    let timed_out = Arc::new(AtomicBool::new(false));
    let timed_out_clone = Arc::clone(&timed_out);
    let started_at = Instant::now();
    let deadline = OpenAIDashboardFetcher::deadline_from(started_at, Duration::from_millis(50));

    let err = OpenAIDashboardBrowserCookieImporter::run_bounded_value_callback::<i32, _>(
        Some(deadline),
        Some(Arc::new(move || {
            timed_out_clone.store(true, Ordering::SeqCst);
        })),
        |complete| {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                complete(7);
            });
        },
    )
    .await
    .unwrap_err();

    assert_eq!(err, PersistentCookieSessionError::TimedOut);
    assert!(started_at.elapsed() < Duration::from_millis(250));
    assert!(timed_out.load(Ordering::SeqCst));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:300 retry waits for timed out cookie store mutation
#[tokio::test]
async fn retry_waits_for_timed_out_cookie_store_mutation() {
    let events: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();

    let events_first = Arc::clone(&events);
    let first_deadline =
        OpenAIDashboardFetcher::deadline_from(Instant::now(), Duration::from_millis(40));
    let first_res = OpenAIDashboardBrowserCookieImporter::run_serialized_callback(
        "store-retry-test",
        Some(first_deadline),
        move |complete| {
            events_first.lock().unwrap().push("first-start");
            let _ = started_tx.send(());
            std::thread::spawn(move || {
                let _ = release_rx.recv();
                events_first.lock().unwrap().push("first-end");
                complete();
            });
        },
    )
    .await;
    assert_eq!(first_res, Err(PersistentCookieSessionError::TimedOut));
    started_rx.await.unwrap();

    let events_second = Arc::clone(&events);
    let second_deadline =
        OpenAIDashboardFetcher::deadline_from(Instant::now(), Duration::from_millis(500));
    let second_task = tokio::spawn(async move {
        OpenAIDashboardBrowserCookieImporter::run_serialized_callback(
            "store-retry-test",
            Some(second_deadline),
            move |complete| {
                events_second.lock().unwrap().push("second");
                complete();
            },
        )
        .await
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(*events.lock().unwrap(), vec!["first-start"]);
    let _ = release_tx.send(());
    second_task.await.unwrap().unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        vec!["first-start", "first-end", "second"]
    );
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:334 mismatch error mentions source label
#[test]
fn mismatch_error_mentions_source_label() {
    let err = OpenAIDashboardCookieImportError::NoMatchingAccount {
        found: vec![
            FoundAccount::new("Chrome Default", "a@b.com"),
            FoundAccount::new("Safari", "c@d.com"),
        ],
    };
    let msg = err.to_string();
    assert!(msg.contains("Chrome Default=a@b.com"));
    assert!(msg.contains("Safari=c@d.com"));
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:346 timed out persistent validation keeps verified session
#[test]
fn timed_out_persistent_validation_keeps_verified_session() {
    let err = OpenAIDashboardBrowserCookieImporter::persistent_validation_failure(
        PersistentCookieSessionError::TimedOut,
    );
    assert!(OpenAIDashboardBrowserCookieImporter::should_trust_verified_session(&err));
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
    assert!(
        !OpenAIDashboardBrowserCookieImporter::should_trust_verified_session(
            &PersistentCookieSessionError::Import(
                OpenAIDashboardCookieImportError::NoMatchingAccount { found: Vec::new() }
            )
        )
    );
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:365 account mismatch removes the cookies just written for that email
#[tokio::test]
async fn account_mismatch_removes_the_cookies_just_written_for_that_email() {
    let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let ev_clear = Arc::clone(&events);
    let ev_write = Arc::clone(&events);
    let ev_evict = Arc::clone(&events);

    let err = OpenAIDashboardBrowserCookieImporter::commit_persistent_cookie_session(
        "Manual",
        2,
        Some("expected@example.com"),
        "store-mismatch",
        None,
        move || ev_clear.lock().unwrap().push("clear"),
        move || ev_write.lock().unwrap().push("write"),
        || async { Ok(Some("other@example.com".to_owned())) },
        move || ev_evict.lock().unwrap().push("evict"),
    )
    .await
    .unwrap_err();

    assert_eq!(
        err,
        PersistentCookieSessionError::Import(OpenAIDashboardCookieImportError::NoMatchingAccount {
            found: vec![FoundAccount::new("Manual", "other@example.com")],
        })
    );
    assert_eq!(
        *events.lock().unwrap(),
        vec!["clear", "write", "evict", "clear"]
    );
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:390 login failure removes the cookies just written for that email
#[tokio::test]
async fn login_failure_removes_the_cookies_just_written_for_that_email() {
    let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let ev_clear = Arc::clone(&events);
    let ev_write = Arc::clone(&events);
    let ev_evict = Arc::clone(&events);

    let err = OpenAIDashboardBrowserCookieImporter::commit_persistent_cookie_session(
        "Manual",
        2,
        Some("expected@example.com"),
        "store-login-fail",
        None,
        move || ev_clear.lock().unwrap().push("clear"),
        move || ev_write.lock().unwrap().push("write"),
        || async {
            Err(PersistentCookieSessionError::Fetch(
                OpenAIDashboardFetchError::LoginRequired,
            ))
        },
        move || ev_evict.lock().unwrap().push("evict"),
    )
    .await
    .unwrap_err();

    assert_eq!(
        err,
        PersistentCookieSessionError::Import(
            OpenAIDashboardCookieImportError::DashboardStillRequiresLogin
        )
    );
    assert_eq!(
        *events.lock().unwrap(),
        vec!["clear", "write", "evict", "clear"]
    );
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:410 timed out persistent validation keeps the cookies just written
#[tokio::test]
async fn timed_out_persistent_validation_keeps_the_cookies_just_written() {
    let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let ev_clear = Arc::clone(&events);
    let ev_write = Arc::clone(&events);
    let ev_evict = Arc::clone(&events);

    let err = OpenAIDashboardBrowserCookieImporter::commit_persistent_cookie_session(
        "Manual",
        2,
        Some("expected@example.com"),
        "store-timeout-keep",
        None,
        move || ev_clear.lock().unwrap().push("clear"),
        move || ev_write.lock().unwrap().push("write"),
        || async { Err(PersistentCookieSessionError::TimedOut) },
        move || ev_evict.lock().unwrap().push("evict"),
    )
    .await
    .unwrap_err();

    assert!(OpenAIDashboardBrowserCookieImporter::should_trust_verified_session(&err));
    assert_eq!(*events.lock().unwrap(), vec!["clear", "write", "evict"]);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:425 default store keeps cookies when persistent validation times out
#[tokio::test]
async fn default_store_keeps_cookies_when_persistent_validation_times_out() {
    let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let ev_clear = Arc::clone(&events);
    let ev_write = Arc::clone(&events);
    let ev_evict = Arc::clone(&events);

    let err = OpenAIDashboardBrowserCookieImporter::commit_persistent_cookie_session(
        "Manual",
        2,
        None,
        "default-store-timeout",
        None,
        move || ev_clear.lock().unwrap().push("clear"),
        move || ev_write.lock().unwrap().push("write"),
        || async { Err(PersistentCookieSessionError::TimedOut) },
        move || ev_evict.lock().unwrap().push("evict"),
    )
    .await
    .unwrap_err();

    assert!(!OpenAIDashboardBrowserCookieImporter::should_trust_verified_session(&err));
    assert_eq!(*events.lock().unwrap(), vec!["clear", "write", "evict"]);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:443 default store login failure removes the cookies just written
#[tokio::test]
async fn default_store_login_failure_removes_the_cookies_just_written() {
    let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let ev_clear = Arc::clone(&events);
    let ev_write = Arc::clone(&events);
    let ev_evict = Arc::clone(&events);

    let err = OpenAIDashboardBrowserCookieImporter::commit_persistent_cookie_session(
        "Manual",
        2,
        None,
        "default-store-login-fail",
        None,
        move || ev_clear.lock().unwrap().push("clear"),
        move || ev_write.lock().unwrap().push("write"),
        || async {
            Err(PersistentCookieSessionError::Fetch(
                OpenAIDashboardFetchError::LoginRequired,
            ))
        },
        move || ev_evict.lock().unwrap().push("evict"),
    )
    .await
    .unwrap_err();

    assert_eq!(
        err,
        PersistentCookieSessionError::Import(
            OpenAIDashboardCookieImportError::DashboardStillRequiresLogin
        )
    );
    assert_eq!(
        *events.lock().unwrap(),
        vec!["clear", "write", "evict", "clear"]
    );
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:463 matching session keeps the cookies it just wrote
#[tokio::test]
async fn matching_session_keeps_the_cookies_it_just_wrote() {
    let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let ev_clear = Arc::clone(&events);
    let ev_write = Arc::clone(&events);
    let ev_evict = Arc::clone(&events);

    let res = OpenAIDashboardBrowserCookieImporter::commit_persistent_cookie_session(
        "Manual",
        2,
        Some("expected@example.com"),
        "store-matching",
        None,
        move || ev_clear.lock().unwrap().push("clear"),
        move || ev_write.lock().unwrap().push("write"),
        || async { Ok(Some("expected@example.com".to_owned())) },
        move || ev_evict.lock().unwrap().push("evict"),
    )
    .await
    .unwrap();

    assert_eq!(res.signed_in_email.as_deref(), Some("expected@example.com"));
    assert!(res.matches_codex_email);
    assert_eq!(*events.lock().unwrap(), vec!["clear", "write"]);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:477 unrelated probe failure keeps the cookies it just wrote
#[tokio::test]
async fn unrelated_probe_failure_keeps_the_cookies_it_just_wrote() {
    let events = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let ev_clear = Arc::clone(&events);
    let ev_write = Arc::clone(&events);
    let ev_evict = Arc::clone(&events);

    let err = OpenAIDashboardBrowserCookieImporter::commit_persistent_cookie_session(
        "Manual",
        2,
        Some("expected@example.com"),
        "store-unrelated-fail",
        None,
        move || ev_clear.lock().unwrap().push("clear"),
        move || ev_write.lock().unwrap().push("write"),
        || async {
            Err(PersistentCookieSessionError::Fetch(
                OpenAIDashboardFetchError::NoDashboardData,
            ))
        },
        move || ev_evict.lock().unwrap().push("evict"),
    )
    .await
    .unwrap_err();

    assert_eq!(
        err,
        PersistentCookieSessionError::Fetch(OpenAIDashboardFetchError::NoDashboardData)
    );
    assert_eq!(*events.lock().unwrap(), vec!["clear", "write", "evict"]);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:497 older rejection cannot erase a newer import on the same account store
#[tokio::test]
async fn older_rejection_cannot_erase_a_newer_import_on_the_same_account_store() {
    let store_state: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let (probe_started_tx, probe_started_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_probe_tx, release_probe_rx) = tokio::sync::oneshot::channel::<()>();

    let state1_clear = Arc::clone(&store_state);
    let state1_write = Arc::clone(&store_state);
    let first = tokio::spawn(async move {
        OpenAIDashboardBrowserCookieImporter::commit_persistent_cookie_session(
            "Older",
            2,
            Some("expected@example.com"),
            "store-same-account",
            None,
            move || state1_clear.lock().unwrap().clear(),
            move || state1_write.lock().unwrap().push("older-cookie"),
            move || async move {
                let _ = probe_started_tx.send(());
                let _ = release_probe_rx.await;
                Ok(Some("wrong@example.com".to_owned()))
            },
            || {},
        )
        .await
    });

    probe_started_rx.await.unwrap();

    let state2_clear = Arc::clone(&store_state);
    let state2_write = Arc::clone(&store_state);
    let second = tokio::spawn(async move {
        OpenAIDashboardBrowserCookieImporter::commit_persistent_cookie_session(
            "Newer",
            2,
            Some("expected@example.com"),
            "store-same-account",
            None,
            move || state2_clear.lock().unwrap().clear(),
            move || state2_write.lock().unwrap().push("newer-cookie"),
            || async { Ok(Some("expected@example.com".to_owned())) },
            || {},
        )
        .await
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(*store_state.lock().unwrap(), vec!["older-cookie"]);
    let _ = release_probe_tx.send(());

    let _ = first.await.unwrap();
    let second_res = second.await.unwrap().unwrap();
    assert_eq!(second_res.source_label, "Newer");
    assert_eq!(*store_state.lock().unwrap(), vec!["newer-cookie"]);
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:574 an in-flight import does not block a different account store
#[tokio::test]
async fn an_in_flight_import_does_not_block_a_different_account_store() {
    let (probe_started_tx, probe_started_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_probe_tx, release_probe_rx) = tokio::sync::oneshot::channel::<()>();

    let first = tokio::spawn(async move {
        OpenAIDashboardBrowserCookieImporter::commit_persistent_cookie_session(
            "Alpha",
            2,
            Some("alpha@example.com"),
            "store-alpha",
            None,
            || {},
            || {},
            move || async move {
                let _ = probe_started_tx.send(());
                let _ = release_probe_rx.await;
                Ok(Some("alpha@example.com".to_owned()))
            },
            || {},
        )
        .await
    });

    probe_started_rx.await.unwrap();

    let second = OpenAIDashboardBrowserCookieImporter::commit_persistent_cookie_session(
        "Beta",
        2,
        Some("beta@example.com"),
        "store-beta",
        None,
        || {},
        || {},
        || async { Ok(Some("beta@example.com".to_owned())) },
        || {},
    )
    .await
    .unwrap();

    assert_eq!(second.signed_in_email.as_deref(), Some("beta@example.com"));
    let _ = release_probe_tx.send(());
    let first_res = first.await.unwrap().unwrap();
    assert_eq!(
        first_res.signed_in_email.as_deref(),
        Some("alpha@example.com")
    );
}

// upstream: OpenAIDashboardBrowserCookieImporterTests.swift:628 waiting account transaction respects deadline and cancellation
#[tokio::test]
async fn waiting_account_transaction_respects_deadline_and_cancellation() {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

    let holder = tokio::spawn(async move {
        OpenAIDashboardBrowserCookieImporter::with_persistent_cookie_transaction(
            "store-wait-test",
            None,
            None,
            move || async move {
                let _ = started_tx.send(());
                let _ = release_rx.await;
                Ok(())
            },
        )
        .await
    });

    started_rx.await.unwrap();

    let deadline = OpenAIDashboardFetcher::deadline_from(Instant::now(), Duration::from_millis(40));
    let timed_out = OpenAIDashboardBrowserCookieImporter::with_persistent_cookie_transaction(
        "store-wait-test",
        Some(deadline),
        None,
        || async { Ok(()) },
    )
    .await;
    assert_eq!(timed_out, Err(PersistentCookieSessionError::TimedOut));

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel_clone.cancel();
    });
    let cancelled = OpenAIDashboardBrowserCookieImporter::with_persistent_cookie_transaction(
        "store-wait-test",
        None,
        Some(&cancel),
        || async { Ok(()) },
    )
    .await;
    assert_eq!(cancelled, Err(PersistentCookieSessionError::Cancelled));

    let _ = release_tx.send(());
    holder.await.unwrap().unwrap();
}

// ============================================================================
// Suite 7: OpenAIDashboardSessionAuthorizationTests.swift
// ============================================================================

fn endpoint_for_server(server: &httpmock::MockServer) -> OpenAIDashboardEndpointConfig {
    OpenAIDashboardEndpointConfig::from_base_url_str(&server.base_url()).unwrap()
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:8 cookie retry pairs workspace balance with the same session identity
#[tokio::test]
async fn cookie_retry_pairs_workspace_balance_with_the_same_session_identity() {
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    let usage_unauth_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/wham/usage")
            .header_missing("authorization");
        then.status(401)
            .body(r#"{"detail":"Missing bearer token"}"#);
    });
    let usage_auth_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/wham/usage")
            .header("authorization", "Bearer session-access-token");
        then.status(200).body(
            r#"{
              "account_id": "acct-workspace",
              "plan_type": "team",
              "rate_limit": {
                "primary_window": {
                  "used_percent": 25,
                  "reset_at": 1700003600,
                  "limit_window_seconds": 18000
                }
              },
              "credits": {
                "has_credits": true,
                "unlimited": false,
                "balance": null
              }
            }"#,
        );
    });

    let session_mock = server.mock(|when, then| {
        when.method("GET").path("/api/auth/session");
        then.status(200).body(
            r#"{
              "accessToken": "session-access-token",
              "user": { "email": "member@example.com" }
            }"#,
        );
    });

    let balance_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts/acct-workspace/remaining_balance")
            .header("authorization", "Bearer session-access-token")
            .header("chatgpt-account-id", "acct-workspace");
        then.status(200).body(r#"{"balance": 124.25}"#);
    });

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let result = OpenAIDashboardFetcher::fetch_dashboard_api_preflight(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=tok",
        None,
        now,
        &Instant::now,
        Some(&https::call()),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(result.1.as_deref(), Some("member@example.com"));
    assert_eq!(result.0.account_id.as_deref(), Some("acct-workspace"));
    assert_eq!(result.0.credits_remaining, Some(124.25));
    assert_eq!(result.0.balance_is_workspace, Some(true));
    usage_unauth_mock.assert_calls(1);
    usage_auth_mock.assert_calls(1);
    session_mock.assert_calls(1);
    balance_mock.assert_calls(1);
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:45 successful cookie usage does not fetch a session or invent an authenticated email
#[tokio::test]
async fn successful_cookie_usage_does_not_fetch_a_session_or_invent_an_authenticated_email() {
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    let usage_mock = server.mock(|when, then| {
        when.method("GET").path("/backend-api/wham/usage");
        then.status(200).body(
            r#"{
              "plan_type": "plus",
              "rate_limit": {
                "primary_window": {
                  "used_percent": 15,
                  "reset_at": 1700003600,
                  "limit_window_seconds": 18000
                }
              }
            }"#,
        );
    });
    let session_mock = server.mock(|when, then| {
        when.method("GET").path("/api/auth/session");
        then.status(200).body(r#"{}"#);
    });

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let response = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=tok",
        None,
        now,
        &Instant::now,
        Some(&https::call()),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(response.verified_signed_in_email, None);
    assert_eq!(
        response
            .api_data
            .primary_limit
            .as_ref()
            .map(|w| w.used_percent),
        Some(15.0)
    );
    usage_mock.assert_calls(1);
    session_mock.assert_calls(0);
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:58 invalid typed session credentials cannot authorize a retry
#[tokio::test]
async fn invalid_typed_session_credentials_cannot_authorize_a_retry() {
    for session_body in [
        r#"{"accessToken":"token","user":{"email":"   "}}"#,
        r#"{"accessToken":"bad token\nwith-newline","user":{"email":"member@example.com"}}"#,
        r#"{"accessToken":"","user":{"email":"member@example.com"}}"#,
        r#"{"user":{"email":"member@example.com"}}"#,
    ] {
        let server = https::start_server();
        let endpoint = endpoint_for_server(&server);
        let broker = https::broker(NetConfig::default());

        let usage_mock = server.mock(|when, then| {
            when.method("GET").path("/backend-api/wham/usage");
            then.status(401).body(r#"{"detail":"Unauthorized"}"#);
        });
        let session_mock = server.mock(|when, then| {
            when.method("GET").path("/api/auth/session");
            then.status(200).body(session_body);
        });

        let now = Timestamp::from_second(1_700_000_000).unwrap();
        let response = OpenAIDashboardFetcher::fetch_dashboard_api_response(
            &broker,
            &endpoint,
            "__Secure-next-auth.session-token=tok",
            None,
            now,
            &Instant::now,
            Some(&https::call()),
        )
        .await
        .unwrap();

        assert_eq!(response, None);
        usage_mock.assert_calls(1);
        session_mock.assert_calls(1);
    }
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:84 nonauthentication usage failures do not start session authentication
#[tokio::test]
async fn nonauthentication_usage_failures_do_not_start_session_authentication() {
    for status in [403u16, 404, 429, 500, 503] {
        let server = https::start_server();
        let endpoint = endpoint_for_server(&server);
        let broker = https::broker(NetConfig::default());

        let usage_mock = server.mock(|when, then| {
            when.method("GET").path("/backend-api/wham/usage");
            then.status(status).body(r#"{"detail":"Failed"}"#);
        });
        let session_mock = server.mock(|when, then| {
            when.method("GET").path("/api/auth/session");
            then.status(200)
                .body(r#"{"accessToken":"tok","user":{"email":"member@example.com"}}"#);
        });

        let now = Timestamp::from_second(1_700_000_000).unwrap();
        let response = OpenAIDashboardFetcher::fetch_dashboard_api_response(
            &broker,
            &endpoint,
            "__Secure-next-auth.session-token=tok",
            None,
            now,
            &Instant::now,
            Some(&https::call()),
        )
        .await
        .unwrap();

        assert_eq!(response, None);
        usage_mock.assert_calls(1);
        session_mock.assert_calls(0);
    }
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:93 malformed successful usage does not start session authentication
#[tokio::test]
async fn malformed_successful_usage_does_not_start_session_authentication() {
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    let usage_mock = server.mock(|when, then| {
        when.method("GET").path("/backend-api/wham/usage");
        then.status(200).body("not-json");
    });
    let session_mock = server.mock(|when, then| {
        when.method("GET").path("/api/auth/session");
        then.status(200)
            .body(r#"{"accessToken":"tok","user":{"email":"member@example.com"}}"#);
    });

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let response = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=tok",
        None,
        now,
        &Instant::now,
        Some(&https::call()),
    )
    .await
    .unwrap();

    assert_eq!(response, None);
    usage_mock.assert_calls(1);
    session_mock.assert_calls(0);
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:102 session HTTP failures stop before bearer usage
#[tokio::test]
async fn session_http_failures_stop_before_bearer_usage() {
    for status in [401u16, 403, 500] {
        let server = https::start_server();
        let endpoint = endpoint_for_server(&server);
        let broker = https::broker(NetConfig::default());

        let usage_mock = server.mock(|when, then| {
            when.method("GET").path("/backend-api/wham/usage");
            then.status(401).body(r#"{"detail":"Unauthorized"}"#);
        });
        let session_mock = server.mock(|when, then| {
            when.method("GET").path("/api/auth/session");
            then.status(status)
                .body(r#"{"accessToken":"tok","user":{"email":"member@example.com"}}"#);
        });

        let now = Timestamp::from_second(1_700_000_000).unwrap();
        let response = OpenAIDashboardFetcher::fetch_dashboard_api_response(
            &broker,
            &endpoint,
            "__Secure-next-auth.session-token=tok",
            None,
            now,
            &Instant::now,
            Some(&https::call()),
        )
        .await
        .unwrap();

        assert_eq!(response, None);
        usage_mock.assert_calls(1);
        session_mock.assert_calls(1);
    }
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:114 a second usage authentication failure does not repeat session bootstrap
#[tokio::test]
async fn a_second_usage_authentication_failure_does_not_repeat_session_bootstrap() {
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    let usage_mock = server.mock(|when, then| {
        when.method("GET").path("/backend-api/wham/usage");
        then.status(401).body(r#"{"detail":"Unauthorized"}"#);
    });
    let session_mock = server.mock(|when, then| {
        when.method("GET").path("/api/auth/session");
        then.status(200)
            .body(r#"{"accessToken":"tok","user":{"email":"member@example.com"}}"#);
    });

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let response = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=tok",
        None,
        now,
        &Instant::now,
        Some(&https::call()),
    )
    .await
    .unwrap();

    assert_eq!(response, None);
    usage_mock.assert_calls(2);
    session_mock.assert_calls(1);
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:126 independent cookies keep their bearer identity and workspace isolated
#[tokio::test]
async fn independent_cookies_keep_their_bearer_identity_and_workspace_isolated() {
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/wham/usage")
            .header_missing("authorization");
        then.status(401).body(r#"{"detail":"Unauthorized"}"#);
    });
    server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/wham/usage")
            .header("cookie", "__Secure-next-auth.session-token=cookie-alpha")
            .header("authorization", "Bearer token-alpha");
        then.status(200).body(
            r#"{"account_id":"acct-alpha","plan_type":"team","credits":{"has_credits":true,"unlimited":false,"balance":null}}"#,
        );
    });
    server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/wham/usage")
            .header("cookie", "__Secure-next-auth.session-token=cookie-beta")
            .header("authorization", "Bearer token-beta");
        then.status(200).body(
            r#"{"account_id":"acct-beta","plan_type":"team","credits":{"has_credits":true,"unlimited":false,"balance":null}}"#,
        );
    });
    server.mock(|when, then| {
        when.method("GET")
            .path("/api/auth/session")
            .header("cookie", "__Secure-next-auth.session-token=cookie-alpha");
        then.status(200)
            .body(r#"{"accessToken":"token-alpha","user":{"email":"alpha@example.com"}}"#);
    });
    server.mock(|when, then| {
        when.method("GET")
            .path("/api/auth/session")
            .header("cookie", "__Secure-next-auth.session-token=cookie-beta");
        then.status(200)
            .body(r#"{"accessToken":"token-beta","user":{"email":"beta@example.com"}}"#);
    });
    server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts/acct-alpha/remaining_balance")
            .header("authorization", "Bearer token-alpha");
        then.status(200).body(r#"{"balance":10.0}"#);
    });
    server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts/acct-beta/remaining_balance")
            .header("authorization", "Bearer token-beta");
        then.status(200).body(r#"{"balance":20.0}"#);
    });

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let alpha = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=cookie-alpha",
        None,
        now,
        &Instant::now,
        Some(&https::call()),
    )
    .await
    .unwrap()
    .unwrap();
    let beta = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=cookie-beta",
        None,
        now,
        &Instant::now,
        Some(&https::call()),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(
        alpha.verified_signed_in_email.as_deref(),
        Some("alpha@example.com")
    );
    assert_eq!(alpha.api_data.account_id.as_deref(), Some("acct-alpha"));
    assert_eq!(alpha.api_data.credits_remaining, Some(10.0));

    assert_eq!(
        beta.verified_signed_in_email.as_deref(),
        Some("beta@example.com")
    );
    assert_eq!(beta.api_data.account_id.as_deref(), Some("acct-beta"));
    assert_eq!(beta.api_data.credits_remaining, Some(20.0));
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:158 optional unauthorized enrichments retain authenticated usage without reauthenticating
#[tokio::test]
async fn optional_unauthorized_enrichments_retain_authenticated_usage_without_reauthenticating() {
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    let usage_unauth_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/wham/usage")
            .header_missing("authorization");
        then.status(401).body(r#"{"detail":"Unauthorized"}"#);
    });
    let usage_auth_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/wham/usage")
            .header("authorization", "Bearer session-access-token");
        then.status(200).body(
            r#"{
              "account_id": "acct-workspace",
              "plan_type": "team",
              "rate_limit": {
                "primary_window": {
                  "used_percent": 33,
                  "reset_at": 1700003600,
                  "limit_window_seconds": 18000
                }
              },
              "credits": {
                "has_credits": true,
                "unlimited": false,
                "balance": null
              },
              "spend_control": {
                "individual_limit": null
              }
            }"#,
        );
    });

    let session_mock = server.mock(|when, then| {
        when.method("GET").path("/api/auth/session");
        then.status(200).body(
            r#"{
              "accessToken": "session-access-token",
              "user": { "email": "member@example.com" }
            }"#,
        );
    });
    let monthly_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts/acct-workspace/spend-controls/current-user/monthly-usage");
        then.status(401).body(r#"{"detail":"Forbidden"}"#);
    });
    let balance_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts/acct-workspace/remaining_balance");
        then.status(403).body(r#"{"detail":"Forbidden"}"#);
    });

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let response = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=tok",
        None,
        now,
        &Instant::now,
        Some(&https::call()),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(
        response.verified_signed_in_email.as_deref(),
        Some("member@example.com")
    );
    assert_eq!(
        response
            .api_data
            .primary_limit
            .as_ref()
            .map(|w| w.used_percent),
        Some(33.0)
    );
    assert_eq!(response.api_data.credits_remaining, None);
    assert_eq!(response.api_data.credits_available, Some(true));
    usage_unauth_mock.assert_calls(1);
    usage_auth_mock.assert_calls(1);
    session_mock.assert_calls(1);
    monthly_mock.assert_calls(1);
    balance_mock.assert_calls(1);
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:185 an expired shared deadline sends no authentication requests
#[tokio::test]
async fn an_expired_shared_deadline_sends_no_authentication_requests() {
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    let usage_mock = server.mock(|when, then| {
        when.method("GET").path("/backend-api/wham/usage");
        then.status(200).body(r#"{}"#);
    });

    let base = Instant::now();
    let deadline = base + Duration::from_secs(1);
    let now_fn = move || base + Duration::from_secs(2);
    let now = Timestamp::from_second(1_700_000_000).unwrap();

    let response = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=tok",
        Some(deadline),
        now,
        &now_fn,
        Some(&https::call()),
    )
    .await
    .unwrap();

    assert_eq!(response, None);
    usage_mock.assert_calls(0);
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:195 deadline expiry after cookie rejection prevents the session request
#[tokio::test]
async fn deadline_expiry_after_cookie_rejection_prevents_the_session_request() {
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    let usage_mock = server.mock(|when, then| {
        when.method("GET").path("/backend-api/wham/usage");
        then.status(401).body(r#"{"detail":"Unauthorized"}"#);
    });
    let session_mock = server.mock(|when, then| {
        when.method("GET").path("/api/auth/session");
        then.status(200)
            .body(r#"{"accessToken":"tok","user":{"email":"member@example.com"}}"#);
    });

    let base = Instant::now();
    let deadline = base + Duration::from_secs(1);
    let calls = AtomicUsize::new(0);
    let now_fn = move || {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            base
        } else {
            base + Duration::from_secs(2)
        }
    };
    let now = Timestamp::from_second(1_700_000_000).unwrap();

    let response = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=tok",
        Some(deadline),
        now,
        &now_fn,
        Some(&https::call()),
    )
    .await
    .unwrap();

    assert_eq!(response, None);
    usage_mock.assert_calls(1);
    session_mock.assert_calls(0);
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:208 session retry and optional requests share the remaining deadline
#[tokio::test]
async fn session_retry_and_optional_requests_share_the_remaining_deadline() {
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    let usage_unauth_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/wham/usage")
            .header_missing("authorization");
        then.status(401).body(r#"{"detail":"Unauthorized"}"#);
    });
    let usage_auth_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/wham/usage")
            .header("authorization", "Bearer tok");
        then.status(200).body(
            r#"{
              "account_id": "acct-workspace",
              "plan_type": "team",
              "rate_limit": {
                "primary_window": {
                  "used_percent": 20,
                  "reset_at": 1700003600,
                  "limit_window_seconds": 18000
                }
              },
              "credits": {
                "has_credits": true,
                "unlimited": false,
                "balance": null
              }
            }"#,
        );
    });

    let session_mock = server.mock(|when, then| {
        when.method("GET").path("/api/auth/session");
        then.status(200)
            .body(r#"{"accessToken":"tok","user":{"email":"member@example.com"}}"#);
    });
    let monthly_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts/acct-workspace/spend-controls/current-user/monthly-usage");
        then.status(200).body(r#"{}"#);
    });
    let balance_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts/acct-workspace/remaining_balance");
        then.status(200).body(r#"{"total_credits_balance":50.0}"#);
    });

    let base = Instant::now();
    let deadline = base + Duration::from_secs(3);
    // Allow first usage (call 0), session (call 1), retry usage (call 2), then expire before optional enrichments.
    let clock_calls = AtomicUsize::new(0);
    let now_fn = move || {
        if clock_calls.fetch_add(1, Ordering::SeqCst) < 3 {
            base + Duration::from_secs(1)
        } else {
            base + Duration::from_secs(4)
        }
    };
    let now = Timestamp::from_second(1_700_000_000).unwrap();

    let response = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=tok",
        Some(deadline),
        now,
        &now_fn,
        Some(&https::call()),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(
        response.verified_signed_in_email.as_deref(),
        Some("member@example.com")
    );
    usage_unauth_mock.assert_calls(1);
    usage_auth_mock.assert_calls(1);
    session_mock.assert_calls(1);
    monthly_mock.assert_calls(0);
    balance_mock.assert_calls(0);
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:229 cancellation before fetch sends no requests
#[tokio::test]
async fn cancellation_before_fetch_sends_no_requests() {
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    let usage_mock = server.mock(|when, then| {
        when.method("GET").path("/backend-api/wham/usage");
        then.status(200).body(r#"{}"#);
    });

    let call = https::call();
    call.cancel.cancel();
    let now = Timestamp::from_second(1_700_000_000).unwrap();

    let err = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=tok",
        None,
        now,
        &Instant::now,
        Some(&call),
    )
    .await
    .unwrap_err();

    assert_eq!(err, OpenAIDashboardFetchError::Cancelled);
    usage_mock.assert_calls(0);
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:245 cancellation after a transport response stops the authentication sequence
#[tokio::test]
async fn cancellation_after_a_transport_response_stops_the_authentication_sequence() {
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    let call = https::call();
    let cancel_on_usage = call.cancel.clone();
    let usage_mock = server.mock(|when, then| {
        when.method("GET").path("/backend-api/wham/usage");
        then.status(401).body(r#"{"detail":"Unauthorized"}"#);
    });

    let session_mock = server.mock(|when, then| {
        when.method("GET").path("/api/auth/session");
        then.status(200)
            .body(r#"{"accessToken":"tok","user":{"email":"member@example.com"}}"#);
    });

    let clock_calls = AtomicUsize::new(0);
    let now_fn = move || {
        if clock_calls.fetch_add(1, Ordering::SeqCst) == 1 {
            cancel_on_usage.cancel();
        }
        Instant::now()
    };

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let err = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=tok",
        None,
        now,
        &now_fn,
        Some(&call),
    )
    .await
    .unwrap_err();

    assert_eq!(err, OpenAIDashboardFetchError::Cancelled);
    usage_mock.assert_calls(1);
    session_mock.assert_calls(0);
}

// upstream: OpenAIDashboardSessionAuthorizationTests.swift:264 transport cancellation remains terminal during optional enrichment
#[tokio::test]
async fn transport_cancellation_remains_terminal_during_optional_enrichment() {
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    let call = https::call();
    let cancel_on_monthly = call.cancel.clone();
    let usage_mock = server.mock(|when, then| {
        when.method("GET").path("/backend-api/wham/usage");
        then.status(200).body(
            r#"{
              "account_id": "acct-workspace",
              "plan_type": "team",
              "rate_limit": {
                "primary_window": {
                  "used_percent": 20,
                  "reset_at": 1700003600,
                  "limit_window_seconds": 18000
                }
              },
              "credits": {
                "has_credits": true,
                "unlimited": false,
                "balance": null
              }
            }"#,
        );
    });
    let monthly_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts/acct-workspace/spend-controls/current-user/monthly-usage");
        then.status(200).body(r#"{}"#);
    });
    let balance_mock = server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts/acct-workspace/remaining_balance");
        then.status(200).body(r#"{"total_credits_balance":50.0}"#);
    });

    let clock_calls = AtomicUsize::new(0);
    let now_fn = move || {
        if clock_calls.fetch_add(1, Ordering::SeqCst) == 1 {
            cancel_on_monthly.cancel();
        }
        Instant::now()
    };

    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let err = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=tok",
        None,
        now,
        &now_fn,
        Some(&call),
    )
    .await
    .unwrap_err();

    assert_eq!(err, OpenAIDashboardFetchError::Cancelled);
    usage_mock.assert_calls(1);
    monthly_mock.assert_calls(0);
    balance_mock.assert_calls(0);
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
            battery_saver_enabled: false,
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
    let now = Timestamp::from_second(1_000).unwrap();
    let recent = Timestamp::from_second(800).unwrap();
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
    let now = Timestamp::from_second(1_000).unwrap();
    let recent = Timestamp::from_second(850).unwrap();
    let context = OpenAIWebRefreshGateContext {
        force: false,
        account_did_change: false,
        last_error: Some("Codex web dashboard request timed out.".to_owned()),
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
    let now = Timestamp::from_second(1_000).unwrap();
    let recent = Timestamp::from_second(995).unwrap();
    let context = OpenAIWebRefreshGateContext {
        force: true,
        account_did_change: false,
        last_error: Some("Codex web dashboard request timed out.".to_owned()),
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
    let now = Timestamp::from_second(1_000).unwrap();
    let recent = Timestamp::from_second(995).unwrap();
    let context = OpenAIWebRefreshGateContext {
        force: false,
        account_did_change: true,
        last_error: Some("No matching OpenAI web session found.".to_owned()),
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
    let now = Timestamp::from_second(1_000).unwrap();
    let snapshot_at = Timestamp::from_second(900).unwrap();
    let attempt_at = Timestamp::from_second(920).unwrap();
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
        updated_at: Timestamp::from_second(1_000).unwrap(),
        ..Default::default()
    };

    assert!(!dashboard_has_page_history(Some(&snapshot)));
}

// upstream: OpenAIWebRefreshGateTests.swift:188 cached history without an in memory scrape stamp does not scrape
#[test]
fn cached_history_without_an_in_memory_scrape_stamp_does_not_scrape() {
    let gate = OpenAIDashboardPageScrapeGate {
        force: false,
        user_initiated: false,
        has_history: true,
        last_page_scrape_at: None,
        history_updated_at: Some(Timestamp::from_second(900).unwrap()),
        now: Timestamp::from_second(1_000).unwrap(),
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
        last_page_scrape_at: Some(Timestamp::from_second(995).unwrap()),
        history_updated_at: Some(Timestamp::from_second(995).unwrap()),
        now: Timestamp::from_second(1_000).unwrap(),
        refresh_interval_seconds: 300.0,
    };

    assert!(should_allow_openai_dashboard_page_scrape(&gate));
}

// upstream: OpenAIWebRefreshGateTests.swift:217 page scrape is skipped when recent history already exists
#[test]
fn page_scrape_is_skipped_when_recent_history_already_exists() {
    let gate = OpenAIDashboardPageScrapeGate {
        force: false,
        user_initiated: false,
        has_history: true,
        last_page_scrape_at: Some(Timestamp::from_second(900).unwrap()),
        history_updated_at: Some(Timestamp::from_second(900).unwrap()),
        now: Timestamp::from_second(1_000).unwrap(),
        refresh_interval_seconds: 300.0,
    };

    assert!(!should_allow_openai_dashboard_page_scrape(&gate));
}

// upstream: OpenAIWebRefreshGateTests.swift:232 user initiated force refresh still scrapes recent history
#[test]
fn user_initiated_force_refresh_still_scrapes_recent_history() {
    let gate = OpenAIDashboardPageScrapeGate {
        force: true,
        user_initiated: true,
        has_history: true,
        last_page_scrape_at: Some(Timestamp::from_second(995).unwrap()),
        history_updated_at: Some(Timestamp::from_second(995).unwrap()),
        now: Timestamp::from_second(1_000).unwrap(),
        refresh_interval_seconds: 300.0,
    };

    assert!(should_allow_openai_dashboard_page_scrape(&gate));
}

// upstream: OpenAIWebRefreshGateTests.swift:247 background force refresh does not scrape recent history
#[test]
fn background_force_refresh_does_not_scrape_recent_history() {
    let gate = OpenAIDashboardPageScrapeGate {
        force: true,
        user_initiated: false,
        has_history: true,
        last_page_scrape_at: Some(Timestamp::from_second(995).unwrap()),
        history_updated_at: Some(Timestamp::from_second(995).unwrap()),
        now: Timestamp::from_second(1_000).unwrap(),
        refresh_interval_seconds: 300.0,
    };

    assert!(!should_allow_openai_dashboard_page_scrape(&gate));
}

// upstream: OpenAIWebRefreshGateTests.swift:262 page scrape returns after the dashboard interval
#[test]
fn page_scrape_returns_after_the_dashboard_interval() {
    let gate = OpenAIDashboardPageScrapeGate {
        force: false,
        user_initiated: false,
        has_history: true,
        last_page_scrape_at: Some(Timestamp::from_second(600).unwrap()),
        history_updated_at: Some(Timestamp::from_second(600).unwrap()),
        now: Timestamp::from_second(1_000).unwrap(),
        refresh_interval_seconds: 300.0,
    };

    assert!(should_allow_openai_dashboard_page_scrape(&gate));
}

// upstream: OpenAIWebRefreshGateTests.swift:277 Empty dashboard history retry runs once for a newer empty snapshot
#[test]
fn empty_dashboard_history_retry_runs_once_for_a_newer_empty_snapshot() {
    let now = Timestamp::from_second(1_000).unwrap();
    let attempt_at = Timestamp::from_second(800).unwrap();
    let newer_snapshot_at = Timestamp::from_second(850).unwrap();
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

// upstream: OpenAIDashboardModelsTests.swift:16 removes skill usage services from usage breakdown
#[test]
fn removes_skill_usage_services_from_usage_breakdown() {
    let breakdown = vec![
        OpenAIDashboardDailyBreakdown::new(
            "2026-04-06",
            vec![
                OpenAIDashboardServiceUsage::new("CLI", 86.24),
                OpenAIDashboardServiceUsage::new("skillUsage:create-plan", 1.0),
                OpenAIDashboardServiceUsage::new("SKILLUSAGE:gh-fix-ci", 2.0),
            ],
            89.24,
        ),
        OpenAIDashboardDailyBreakdown::new(
            "2026-04-05",
            vec![OpenAIDashboardServiceUsage::new(
                "skillUsage:only-skill",
                1.0,
            )],
            1.0,
        ),
    ];

    let sanitized = OpenAIDashboardSnapshot::removing_skill_usage_services(&breakdown);

    assert_eq!(sanitized.len(), 1);
    assert_eq!(sanitized[0].day, "2026-04-06");
    assert_eq!(
        sanitized[0].services,
        vec![OpenAIDashboardServiceUsage::new("CLI", 86.24)]
    );
    assert!((sanitized[0].total_credits_used - 86.24).abs() < 0.0001);
}

// upstream: OpenAIDashboardModelsTests.swift:47 snapshot initializer sanitizes usage breakdown
#[test]
fn snapshot_initializer_sanitizes_usage_breakdown() {
    let snapshot = OpenAIDashboardSnapshot::new(
        Some("user@example.com".to_owned()),
        None,
        Some(100.0),
        None,
        Vec::new(),
        Vec::new(),
        vec![OpenAIDashboardDailyBreakdown::new(
            "2026-04-06",
            vec![
                OpenAIDashboardServiceUsage::new("CLI", 12.5),
                OpenAIDashboardServiceUsage::new("skillUsage:yeet", 1.0),
            ],
            13.5,
        )],
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Timestamp::from_second(1_775_433_600).unwrap(),
    );

    assert_eq!(
        snapshot.usage_breakdown,
        vec![OpenAIDashboardDailyBreakdown::new(
            "2026-04-06",
            vec![OpenAIDashboardServiceUsage::new("CLI", 12.5)],
            12.5,
        )]
    );
}

// upstream: OpenAIDashboardModelsTests.swift:76 subscription renewal survives snapshot encoding and Codex projection
#[test]
fn subscription_renewal_survives_snapshot_encoding_and_codex_projection() {
    let renews_at = Timestamp::from_second(1_776_754_510).unwrap();
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
            window_minutes: Some(300),
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
        Some("plus".to_owned()),
        None,
        Some(renews_at),
        Timestamp::from_second(1_775_433_600).unwrap(),
    );

    let data = serde_json::to_vec(&snapshot).unwrap();
    let decoded: OpenAIDashboardSnapshot = serde_json::from_slice(&data).unwrap();
    let codex_id = ProviderId::new("codex").unwrap();
    let usage = decoded.to_usage_snapshot(codex_id, None, None).unwrap();

    assert_eq!(decoded.subscription_renews_at, Some(renews_at));
    assert_eq!(usage.subscription_renews_at, Some(renews_at));
    assert_eq!(usage.subscription_expires_at, None);
}

// upstream: OpenAIDashboardModelsTests.swift:106 snapshot decoder drops empty zero usage buckets
#[test]
fn snapshot_decoder_drops_empty_zero_usage_buckets() {
    let json = r#"{
      "creditEvents": [],
      "dailyBreakdown": [],
      "usageBreakdown": [
        {
          "day": "2026-04-06",
          "services": [
            { "service": "skillUsage:yeet", "creditsUsed": 1 }
          ],
          "totalCreditsUsed": 0
        },
        {
          "day": "2026-04-05",
          "services": [],
          "totalCreditsUsed": 14.5
        }
      ],
      "updatedAt": "2026-04-06T00:00:00Z"
    }"#;

    let decoded: OpenAIDashboardSnapshot = serde_json::from_str(json).unwrap();

    assert_eq!(
        decoded.usage_breakdown,
        vec![OpenAIDashboardDailyBreakdown::new(
            "2026-04-05",
            Vec::new(),
            14.5
        )]
    );
}

// upstream: OpenAIDashboardModelsTests.swift:135 recent credit totals use calendar days and exclude future rows
#[test]
fn recent_credit_totals_use_calendar_days_and_exclude_future_rows() {
    let now: Timestamp = "2026-04-12T15:30:00Z".parse().unwrap();
    let breakdown = vec![
        OpenAIDashboardDailyBreakdown::new(
            "2026-04-13",
            vec![OpenAIDashboardServiceUsage::new("CLI", 999.0)],
            999.0,
        ),
        OpenAIDashboardDailyBreakdown::new(
            "2026-04-12",
            vec![OpenAIDashboardServiceUsage::new("CLI", 12.5)],
            12.5,
        ),
        OpenAIDashboardDailyBreakdown::new(
            "2026-03-14",
            vec![OpenAIDashboardServiceUsage::new("CLI", 20.0)],
            20.0,
        ),
        OpenAIDashboardDailyBreakdown::new(
            "2026-03-13",
            vec![OpenAIDashboardServiceUsage::new("CLI", 50.0)],
            50.0,
        ),
    ];

    let summary =
        OpenAIDashboardDailyBreakdown::recent_usage_summary(&breakdown, 30, now, &TimeZone::UTC);

    assert_eq!(summary.history_days, 30);
    assert_eq!(summary.today_credits, Some(12.5));
    assert_eq!(summary.total_credits, Some(32.5));
    assert_eq!(
        summary
            .daily
            .iter()
            .map(|d| d.day.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-03-14", "2026-04-12"]
    );
}

// upstream: OpenAIDashboardModelsTests.swift:155 recent credit totals preserve gaps and sanitize invalid values
#[test]
fn recent_credit_totals_preserve_gaps_and_sanitize_invalid_values() {
    let now: Timestamp = "2026-04-12T15:30:00Z".parse().unwrap();
    let breakdown = vec![
        OpenAIDashboardDailyBreakdown::new(
            "2026-04-12",
            vec![
                OpenAIDashboardServiceUsage::new("CLI", 10.0),
                OpenAIDashboardServiceUsage::new("Bad", f64::NAN),
                OpenAIDashboardServiceUsage::new("skillUsage:plan", 5.0),
            ],
            999.0,
        ),
        OpenAIDashboardDailyBreakdown::new(
            "2026-04-10",
            vec![OpenAIDashboardServiceUsage::new("Web", 4.5)],
            4.5,
        ),
        OpenAIDashboardDailyBreakdown::new(
            "not-a-day",
            vec![OpenAIDashboardServiceUsage::new("CLI", 100.0)],
            100.0,
        ),
        OpenAIDashboardDailyBreakdown::new("2026-04-09", Vec::new(), f64::INFINITY),
    ];

    let summary =
        OpenAIDashboardDailyBreakdown::recent_usage_summary(&breakdown, 30, now, &TimeZone::UTC);

    assert_eq!(summary.today_credits, Some(10.0));
    assert_eq!(summary.total_credits, Some(14.5));
    assert_eq!(summary.daily.len(), 2);
    assert_eq!(summary.daily[0].day, "2026-04-10");
    assert_eq!(summary.daily[1].day, "2026-04-12");
    assert_eq!(summary.daily[1].total_credits_used, 10.0);
}

// upstream: OpenAIDashboardModelsTests.swift:181 recent credit totals report zero when history has no row for today
#[test]
fn recent_credit_totals_report_zero_when_history_has_no_row_for_today() {
    let now: Timestamp = "2026-04-12T15:30:00Z".parse().unwrap();
    let breakdown = vec![OpenAIDashboardDailyBreakdown::new(
        "2026-04-11",
        vec![OpenAIDashboardServiceUsage::new("CLI", 7.25)],
        7.25,
    )];

    let summary =
        OpenAIDashboardDailyBreakdown::recent_usage_summary(&breakdown, 30, now, &TimeZone::UTC);

    assert_eq!(summary.today_credits, Some(0.0));
    assert_eq!(summary.total_credits, Some(7.25));
}

// upstream: OpenAIDashboardModelsTests.swift:195 recent credit totals fail closed on overflow
#[test]
fn recent_credit_totals_fail_closed_on_overflow() {
    let now: Timestamp = "2026-04-12T15:30:00Z".parse().unwrap();
    let breakdown = vec![
        OpenAIDashboardDailyBreakdown::new(
            "2026-04-12",
            vec![OpenAIDashboardServiceUsage::new("CLI", f64::MAX)],
            f64::MAX,
        ),
        OpenAIDashboardDailyBreakdown::new(
            "2026-04-11",
            vec![OpenAIDashboardServiceUsage::new("CLI", f64::MAX)],
            f64::MAX,
        ),
    ];

    let summary =
        OpenAIDashboardDailyBreakdown::recent_usage_summary(&breakdown, 30, now, &TimeZone::UTC);

    assert_eq!(summary.today_credits, Some(f64::MAX));
    assert_eq!(summary.total_credits, None);
    assert_eq!(summary.daily.len(), 2);
}

// upstream: OpenAIDashboardModelsTests.swift:215 recent credit totals respect the selected timezone
#[test]
fn recent_credit_totals_respect_the_selected_timezone() {
    // 2026-04-12 03:30 UTC is still 2026-04-11 20:30 in UTC-7.
    let now: Timestamp = "2026-04-12T03:30:00Z".parse().unwrap();
    let pacific = TimeZone::fixed(jiff::tz::Offset::from_hours(-7).unwrap());
    let breakdown = vec![
        OpenAIDashboardDailyBreakdown::new(
            "2026-04-12",
            vec![OpenAIDashboardServiceUsage::new("CLI", 90.0)],
            90.0,
        ),
        OpenAIDashboardDailyBreakdown::new(
            "2026-04-11",
            vec![OpenAIDashboardServiceUsage::new("CLI", 15.0)],
            15.0,
        ),
    ];

    let summary =
        OpenAIDashboardDailyBreakdown::recent_usage_summary(&breakdown, 30, now, &pacific);

    assert_eq!(summary.today_credits, Some(15.0));
    assert_eq!(summary.total_credits, Some(15.0));
    assert_eq!(
        summary
            .daily
            .iter()
            .map(|d| d.day.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-04-11"]
    );
}

// upstream: OpenAIDashboardModelsTests.swift:234 recent credit totals keep Gregorian dashboard keys with a non Gregorian system calendar
#[test]
fn recent_credit_totals_keep_gregorian_dashboard_keys_with_a_non_gregorian_system_calendar() {
    let now: Timestamp = "2026-04-12T15:30:00Z".parse().unwrap();
    let breakdown = vec![OpenAIDashboardDailyBreakdown::new(
        "2026-04-12",
        vec![OpenAIDashboardServiceUsage::new("CLI", 21.0)],
        21.0,
    )];

    let summary =
        OpenAIDashboardDailyBreakdown::recent_usage_summary(&breakdown, 30, now, &TimeZone::UTC);

    assert_eq!(summary.today_credits, Some(21.0));
    assert_eq!(summary.total_credits, Some(21.0));
    assert_eq!(
        summary
            .daily
            .iter()
            .map(|d| d.day.as_str())
            .collect::<Vec<_>>(),
        vec!["2026-04-12"]
    );
}

// ============================================================================
// Suite 10: OpenAIDashboardIdentityMergeTests.swift
// ============================================================================

fn make_api_data_for_merge(account_id: Option<&str>) -> DashboardAPIData {
    DashboardAPIData {
        account_id: account_id.map(str::to_owned),
        primary_limit: Some(RateWindow {
            used_percent: 25.0,
            window_minutes: Some(300),
            resets_at: None,
            reset_description: None,
            next_regen_percent: None,
            is_synthetic_placeholder: false,
        }),
        secondary_limit: None,
        extra_rate_windows: None,
        credits_remaining: Some(42.0),
        credits_available: Some(true),
        balance_is_workspace: Some(false),
        codex_credit_limit: None,
        account_plan: Some("pro".to_owned()),
    }
}

fn make_previous_snapshot_for_merge(
    email: Option<&str>,
    account_id: Option<&str>,
) -> OpenAIDashboardSnapshot {
    let event = CreditEvent::new(Timestamp::from_second(1_700_000_000).unwrap(), "CLI", 10.0);
    let breakdown = vec![OpenAIDashboardDailyBreakdown::new(
        "2026-04-10",
        vec![OpenAIDashboardServiceUsage::new("CLI", 10.0)],
        10.0,
    )];
    OpenAIDashboardSnapshot::new(
        email.map(str::to_owned),
        account_id.map(str::to_owned),
        Some(88.0),
        Some(RateWindow {
            used_percent: 12.0,
            window_minutes: None,
            resets_at: None,
            reset_description: Some("Resets tomorrow".to_owned()),
            next_regen_percent: None,
            is_synthetic_placeholder: false,
        }),
        vec![event],
        breakdown.clone(),
        breakdown,
        Some("https://chatgpt.com/codex/settings/usage".to_owned()),
        Some(RateWindow {
            used_percent: 5.0,
            window_minutes: Some(300),
            resets_at: None,
            reset_description: None,
            next_regen_percent: None,
            is_synthetic_placeholder: false,
        }),
        Some(RateWindow {
            used_percent: 40.0,
            window_minutes: Some(10080),
            resets_at: None,
            reset_description: None,
            next_regen_percent: None,
            is_synthetic_placeholder: false,
        }),
        None,
        Some(99.0),
        Some(true),
        Some(false),
        None,
        Some("Plus".to_owned()),
        None,
        Some(Timestamp::from_second(1_776_000_000).unwrap()),
        Timestamp::from_second(1_700_000_000).unwrap(),
    )
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:8 legacy personal dashboard fields survive page and API refreshes without account IDs
#[test]
fn legacy_personal_dashboard_fields_survive_page_and_api_refreshes_without_account_ids() {
    let previous = make_previous_snapshot_for_merge(Some("user@example.com"), None);
    let api_data = make_api_data_for_merge(None);
    let now = Timestamp::from_second(1_700_001_000).unwrap();

    let api_merged = OpenAIDashboardFetcher::snapshot_by_merging_api(
        &api_data,
        Some(&previous),
        Some("user@example.com"),
        OpenAISubscriptionFetchResult::Unavailable,
        now,
    );
    assert_eq!(api_merged.code_review_remaining_percent, Some(88.0));
    assert_eq!(api_merged.credit_events.len(), 1);
    assert_eq!(api_merged.daily_breakdown.len(), 1);
    assert_eq!(api_merged.usage_breakdown.len(), 1);

    let page_snapshot = OpenAIDashboardSnapshot {
        signed_in_email: Some("user@example.com".to_owned()),
        account_id: None,
        primary_limit: api_data.primary_limit.clone(),
        updated_at: now,
        ..Default::default()
    };
    let page_merged = OpenAIDashboardFetcher::filling_missing_page_fields(
        page_snapshot,
        Some(&previous),
        OpenAISubscriptionFetchResult::Unavailable,
    );
    assert_eq!(page_merged.code_review_remaining_percent, Some(88.0));
    assert_eq!(page_merged.credit_events.len(), 1);
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:30 unidentified page waits without inheriting the API identity
#[test]
fn unidentified_page_waits_without_inheriting_the_api_identity() {
    assert!(OpenAIDashboardFetcher::should_wait_for_page_identity(
        Some("verified@example.com"),
        None,
    ));
    assert!(OpenAIDashboardFetcher::should_wait_for_page_identity(
        Some("verified@example.com"),
        Some("   "),
    ));
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:38 API usage excludes all cached fields from an unpaired identity
#[test]
fn api_usage_excludes_all_cached_fields_from_an_unpaired_identity() {
    let previous = make_previous_snapshot_for_merge(Some("other@example.com"), Some("acct-1"));
    let api_data = make_api_data_for_merge(Some("acct-1"));
    let now = Timestamp::from_second(1_700_001_000).unwrap();

    let merged = OpenAIDashboardFetcher::snapshot_by_merging_api(
        &api_data,
        Some(&previous),
        Some("verified@example.com"),
        OpenAISubscriptionFetchResult::Unavailable,
        now,
    );

    assert_eq!(
        merged.signed_in_email.as_deref(),
        Some("verified@example.com")
    );
    assert_eq!(merged.code_review_remaining_percent, None);
    assert!(merged.credit_events.is_empty());
    assert!(merged.daily_breakdown.is_empty());
    assert!(merged.usage_breakdown.is_empty());
    assert_eq!(merged.secondary_limit, None);
    assert_eq!(merged.subscription_renews_at, None);
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:51 API merge cannot use a cached email as its verified identity
#[test]
fn api_merge_cannot_use_a_cached_email_as_its_verified_identity() {
    let previous = make_previous_snapshot_for_merge(Some("cached@example.com"), Some("acct-1"));
    let api_data = make_api_data_for_merge(Some("acct-1"));
    let now = Timestamp::from_second(1_700_001_000).unwrap();

    let merged = OpenAIDashboardFetcher::snapshot_by_merging_api(
        &api_data,
        Some(&previous),
        None,
        OpenAISubscriptionFetchResult::Unavailable,
        now,
    );

    assert_eq!(merged.signed_in_email, None);
    assert_eq!(merged.code_review_remaining_percent, None);
    assert!(merged.credit_events.is_empty());
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:62 page merge cannot infer an identity or copy another accounts data
#[test]
fn page_merge_cannot_infer_an_identity_or_copy_another_accounts_data() {
    let previous = make_previous_snapshot_for_merge(Some("cached@example.com"), Some("acct-1"));
    let now = Timestamp::from_second(1_700_001_000).unwrap();

    let anonymous_page = OpenAIDashboardSnapshot {
        signed_in_email: None,
        account_id: Some("acct-1".to_owned()),
        primary_limit: Some(RateWindow {
            used_percent: 30.0,
            window_minutes: Some(300),
            resets_at: None,
            reset_description: None,
            next_regen_percent: None,
            is_synthetic_placeholder: false,
        }),
        updated_at: now,
        ..Default::default()
    };
    let merged_anon = OpenAIDashboardFetcher::filling_missing_page_fields(
        anonymous_page,
        Some(&previous),
        OpenAISubscriptionFetchResult::Unavailable,
    );
    assert_eq!(merged_anon.signed_in_email, None);
    assert_eq!(merged_anon.code_review_remaining_percent, None);
    assert!(merged_anon.credit_events.is_empty());

    let other_page = OpenAIDashboardSnapshot {
        signed_in_email: Some("different@example.com".to_owned()),
        account_id: Some("acct-1".to_owned()),
        primary_limit: Some(RateWindow {
            used_percent: 30.0,
            window_minutes: Some(300),
            resets_at: None,
            reset_description: None,
            next_regen_percent: None,
            is_synthetic_placeholder: false,
        }),
        updated_at: now,
        ..Default::default()
    };
    let merged_other = OpenAIDashboardFetcher::filling_missing_page_fields(
        other_page,
        Some(&previous),
        OpenAISubscriptionFetchResult::Unavailable,
    );
    assert_eq!(
        merged_other.signed_in_email.as_deref(),
        Some("different@example.com")
    );
    assert_eq!(merged_other.code_review_remaining_percent, None);
    assert!(merged_other.credit_events.is_empty());
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:81 normalized matching identities preserve history and independently missing fields
#[test]
fn normalized_matching_identities_preserve_history_and_independently_missing_fields() {
    let previous = make_previous_snapshot_for_merge(Some("User@Example.com"), Some("acct-alpha"));
    let api_data = make_api_data_for_merge(Some(" ACCT-ALPHA "));
    let now = Timestamp::from_second(1_700_001_000).unwrap();

    let merged = OpenAIDashboardFetcher::snapshot_by_merging_api(
        &api_data,
        Some(&previous),
        Some("  user@example.com  "),
        OpenAISubscriptionFetchResult::Unavailable,
        now,
    );

    assert_eq!(merged.signed_in_email.as_deref(), Some("user@example.com"));
    assert_eq!(merged.code_review_remaining_percent, Some(88.0));
    assert_eq!(merged.credit_events.len(), 1);
    assert_eq!(merged.secondary_limit, previous.secondary_limit);
    assert_eq!(
        merged.subscription_renews_at,
        previous.subscription_renews_at
    );
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:108 missing page identity waits and mismatched page returns verified API data
#[test]
fn missing_page_identity_waits_and_mismatched_page_returns_verified_api_data() {
    let previous = make_previous_snapshot_for_merge(Some("page-user@example.com"), Some("acct-1"));
    let api_data = make_api_data_for_merge(Some("acct-1"));
    let now = Timestamp::from_second(1_700_001_000).unwrap();

    let waiting = OpenAIDashboardFetcher::snapshot_for_unpaired_page(
        Some(&api_data),
        Some("api-user@example.com"),
        None,
        OpenAISubscriptionFetchResult::Unavailable,
        Some(&previous),
        now,
    )
    .unwrap();
    assert_eq!(waiting, None);

    let mismatched = OpenAIDashboardFetcher::snapshot_for_unpaired_page(
        Some(&api_data),
        Some("api-user@example.com"),
        Some("page-user@example.com"),
        OpenAISubscriptionFetchResult::Unavailable,
        Some(&previous),
        now,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        mismatched.signed_in_email.as_deref(),
        Some("api-user@example.com")
    );
    assert_eq!(mismatched.code_review_remaining_percent, None);
    assert!(mismatched.credit_events.is_empty());
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:135 matching page identity allows the normal page merge
#[test]
fn matching_page_identity_allows_the_normal_page_merge() {
    let api_data = make_api_data_for_merge(Some("acct-1"));
    let now = Timestamp::from_second(1_700_001_000).unwrap();

    let unpaired = OpenAIDashboardFetcher::snapshot_for_unpaired_page(
        Some(&api_data),
        Some("Same@Example.com"),
        Some(" same@example.com "),
        OpenAISubscriptionFetchResult::Unavailable,
        None,
        now,
    )
    .unwrap();
    assert_eq!(unpaired, None);
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:146 API only fallback requires independently verified API identity
#[test]
fn api_only_fallback_requires_independently_verified_api_identity() {
    let api_data = make_api_data_for_merge(Some("acct-1"));
    let now = Timestamp::from_second(1_700_001_000).unwrap();

    let unpaired = OpenAIDashboardFetcher::snapshot_for_unpaired_page(
        Some(&api_data),
        None,
        Some("page-user@example.com"),
        OpenAISubscriptionFetchResult::Unavailable,
        None,
        now,
    )
    .unwrap();
    assert_eq!(unpaired, None);
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:158 metadata only API cannot be mixed into an unpaired page
#[test]
fn metadata_only_api_cannot_be_mixed_into_an_unpaired_page() {
    let metadata_only = DashboardAPIData {
        account_id: Some("acct-1".to_owned()),
        primary_limit: None,
        secondary_limit: None,
        extra_rate_windows: None,
        credits_remaining: None,
        credits_available: Some(false),
        balance_is_workspace: None,
        codex_credit_limit: None,
        account_plan: Some("plus".to_owned()),
    };
    let now = Timestamp::from_second(1_700_001_000).unwrap();

    let err = OpenAIDashboardFetcher::snapshot_for_unpaired_page(
        Some(&metadata_only),
        Some("api-user@example.com"),
        Some("page-user@example.com"),
        OpenAISubscriptionFetchResult::Unavailable,
        None,
        now,
    )
    .unwrap_err();
    assert_eq!(err, OpenAIDashboardFetchError::NoDashboardData);
}

// upstream: OpenAIDashboardIdentityMergeTests.swift:244 matching email cannot reuse another or unknown workspace
#[test]
fn matching_email_cannot_reuse_another_or_unknown_workspace() {
    let mut previous_alpha =
        make_previous_snapshot_for_merge(Some("user@example.com"), Some("acct-alpha"));
    previous_alpha.balance_is_workspace = Some(true);
    let api_beta = make_api_data_for_merge(Some("acct-beta"));
    let api_unknown = make_api_data_for_merge(None);
    let now = Timestamp::from_second(1_700_001_000).unwrap();

    let merged_beta = OpenAIDashboardFetcher::snapshot_by_merging_api(
        &api_beta,
        Some(&previous_alpha),
        Some("user@example.com"),
        OpenAISubscriptionFetchResult::Unavailable,
        now,
    );
    assert_eq!(merged_beta.code_review_remaining_percent, None);
    assert!(merged_beta.credit_events.is_empty());

    let merged_unknown = OpenAIDashboardFetcher::snapshot_by_merging_api(
        &api_unknown,
        Some(&previous_alpha),
        Some("user@example.com"),
        OpenAISubscriptionFetchResult::Unavailable,
        now,
    );
    assert_eq!(merged_unknown.code_review_remaining_percent, None);
    assert!(merged_unknown.credit_events.is_empty());
}

// ============================================================================
// Suite 11: CodexWorkspaceBalanceDashboardTests.swift
// ============================================================================

// upstream: CodexWorkspaceBalanceDashboardTests.swift:83 owner balance remains distinct from an exhausted personal cap
#[tokio::test]
async fn owner_balance_remains_distinct_from_an_exhausted_personal_cap() {
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    server.mock(|when, then| {
        when.method("GET").path("/backend-api/wham/usage");
        then.status(200).body(
            r#"{
              "account_id": "acct-workspace",
              "plan_type": "team",
              "credits": {
                "has_credits": true,
                "unlimited": false,
                "balance": null
              },
              "individual_limit": {
                "limit": 50,
                "used": 50
              }
            }"#,
        );
    });
    server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts/acct-workspace/remaining_balance");
        then.status(200).body(r#"{"balance": 325.5}"#);
    });

    let now = Timestamp::from_second(1_776_000_000).unwrap();
    let response = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=tok",
        None,
        now,
        &Instant::now,
        Some(&https::call()),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(response.api_data.credits_remaining, Some(325.5));
    assert_eq!(response.api_data.balance_is_workspace, Some(true));
    let limit = response.api_data.codex_credit_limit.as_ref().unwrap();
    assert_eq!(limit.remaining, 0.0);
    assert_eq!(limit.limit, 50.0);
}

// upstream: CodexWorkspaceBalanceDashboardTests.swift:101 unavailable owner endpoint retains hidden credits without rate windows
#[tokio::test]
async fn unavailable_owner_endpoint_retains_hidden_credits_without_rate_windows() {
    let server = https::start_server();
    let endpoint = endpoint_for_server(&server);
    let broker = https::broker(NetConfig::default());

    server.mock(|when, then| {
        when.method("GET").path("/backend-api/wham/usage");
        then.status(200).body(
            r#"{
              "account_id": "acct-workspace",
              "plan_type": "team",
              "credits": {
                "has_credits": true,
                "unlimited": false,
                "balance": null
              }
            }"#,
        );
    });
    server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts/acct-workspace/spend-controls/current-user/monthly-usage");
        then.status(404).body(r#"{}"#);
    });
    server.mock(|when, then| {
        when.method("GET")
            .path("/backend-api/accounts/acct-workspace/remaining_balance");
        then.status(403).body(r#"{"detail":"Forbidden"}"#);
    });

    let now = Timestamp::from_second(1_776_000_000).unwrap();
    let response = OpenAIDashboardFetcher::fetch_dashboard_api_response(
        &broker,
        &endpoint,
        "__Secure-next-auth.session-token=tok",
        None,
        now,
        &Instant::now,
        Some(&https::call()),
    )
    .await
    .unwrap()
    .unwrap();

    assert!(response.api_data.has_usage_data());
    let snapshot = response
        .api_data
        .to_snapshot(Some("member@example.com".to_owned()), now);
    assert!(OpenAIDashboardFetcher::has_returnable_dashboard_data(
        &snapshot
    ));
    let credits = snapshot.to_credits_snapshot().unwrap();
    assert!(!credits.balance_read_succeeded);
    assert_eq!(credits.credits_available, Some(true));
}

// upstream: CodexWorkspaceBalanceDashboardTests.swift:117 missing balance payload never revives a cached workspace amount
#[test]
fn missing_balance_payload_never_revives_a_cached_workspace_amount() {
    let now = Timestamp::from_second(1_776_000_000).unwrap();
    let previous = OpenAIDashboardSnapshot {
        signed_in_email: Some("owner@example.com".to_owned()),
        account_id: Some("acct-workspace".to_owned()),
        credits_remaining: Some(450.0),
        credits_available: Some(true),
        balance_is_workspace: Some(true),
        updated_at: now,
        ..Default::default()
    };

    let api_data = DashboardAPIData {
        account_id: Some("acct-workspace".to_owned()),
        primary_limit: None,
        secondary_limit: None,
        extra_rate_windows: None,
        credits_remaining: None,
        credits_available: Some(true),
        balance_is_workspace: None,
        codex_credit_limit: None,
        account_plan: Some("team".to_owned()),
    };

    let merged = OpenAIDashboardFetcher::snapshot_by_merging_api(
        &api_data,
        Some(&previous),
        Some("owner@example.com"),
        OpenAISubscriptionFetchResult::Unavailable,
        now,
    );

    assert_eq!(merged.credits_remaining, None);
    assert_eq!(merged.credits_available, Some(true));
    assert_eq!(merged.balance_is_workspace, None);
}

// upstream: CodexWorkspaceBalanceDashboardTests.swift:133 merges keep provenance with the selected numeric balance
#[test]
fn merges_keep_provenance_with_the_selected_numeric_balance() {
    let now = Timestamp::from_second(1_776_000_000).unwrap();
    let previous_workspace = OpenAIDashboardSnapshot {
        signed_in_email: Some("owner@example.com".to_owned()),
        account_id: Some("acct-workspace".to_owned()),
        credits_remaining: Some(450.0),
        credits_available: Some(true),
        balance_is_workspace: Some(true),
        updated_at: now,
        ..Default::default()
    };

    let api_personal_balance = DashboardAPIData {
        account_id: Some("acct-workspace".to_owned()),
        primary_limit: None,
        secondary_limit: None,
        extra_rate_windows: None,
        credits_remaining: Some(25.0),
        credits_available: Some(true),
        balance_is_workspace: None,
        codex_credit_limit: None,
        account_plan: Some("team".to_owned()),
    };

    let merged = OpenAIDashboardFetcher::snapshot_by_merging_api(
        &api_personal_balance,
        Some(&previous_workspace),
        Some("owner@example.com"),
        OpenAISubscriptionFetchResult::Unavailable,
        now,
    );

    assert_eq!(merged.credits_remaining, Some(25.0));
    assert_eq!(merged.balance_is_workspace, None);
}

// upstream: CodexWorkspaceBalanceDashboardTests.swift:156 page placeholder cannot replace the API hidden balance state
#[test]
fn page_placeholder_cannot_replace_the_api_hidden_balance_state() {
    let now = Timestamp::from_second(1_776_000_000).unwrap();
    let scrape = DashboardScrapePayload {
        signed_in_email: Some("member@example.com".to_owned()),
        auth_status: Some("logged_in".to_owned()),
        body_text: "Balance\nCredits remaining 0\n5h limit\n80% remaining".to_owned(),
        ..Default::default()
    };
    let api_data = DashboardAPIData {
        account_id: Some("acct-workspace".to_owned()),
        primary_limit: Some(RateWindow {
            used_percent: 20.0,
            window_minutes: Some(300),
            resets_at: None,
            reset_description: None,
            next_regen_percent: None,
            is_synthetic_placeholder: false,
        }),
        secondary_limit: None,
        extra_rate_windows: None,
        credits_remaining: None,
        credits_available: Some(true),
        balance_is_workspace: None,
        codex_credit_limit: None,
        account_plan: Some("team".to_owned()),
    };

    let parsed = OpenAIDashboardFetcher::parse_dashboard_scrape_with_api(
        &scrape,
        Some(&api_data),
        None,
        now,
    )
    .unwrap();

    assert_eq!(parsed.credits_remaining, None);
    assert_eq!(parsed.credits_available, Some(true));
    assert_eq!(parsed.balance_is_workspace, None);
}

// upstream: CodexWorkspaceBalanceDashboardTests.swift:194 dashboard cache preserves provenance and accepts older snapshots
#[test]
fn dashboard_cache_preserves_provenance_and_accepts_older_snapshots() {
    let older_json = r#"{
      "signedInEmail": "owner@example.com",
      "accountId": "acct-workspace",
      "creditEvents": [],
      "dailyBreakdown": [],
      "creditsRemaining": 120.5,
      "updatedAt": "2026-04-12T00:00:00Z"
    }"#;
    let older: OpenAIDashboardSnapshot = serde_json::from_str(older_json).unwrap();
    assert_eq!(older.credits_remaining, Some(120.5));
    assert_eq!(older.credits_available, None);
    assert_eq!(older.balance_is_workspace, None);

    let mut with_provenance = older;
    with_provenance.credits_available = Some(true);
    with_provenance.balance_is_workspace = Some(true);

    let encoded = serde_json::to_vec(&with_provenance).unwrap();
    let decoded: OpenAIDashboardSnapshot = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded.credits_available, Some(true));
    assert_eq!(decoded.balance_is_workspace, Some(true));
    let credits = decoded.to_credits_snapshot().unwrap();
    assert!(credits.balance_is_workspace);
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
}
