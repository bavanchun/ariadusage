// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexDashboardAuthority.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexCLIDashboardAuthorityContext.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexOpenAIWorkspaceResolver.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexOpenAIWorkspaceIdentityCache.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ariadusage_core::model::snapshot::UsageSnapshot;
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_protocol::secret::SecretString;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderName, HeaderValue, USER_AGENT};
use reqwest::{Method, Url};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::brokers::call::BrokerCall;
use crate::brokers::net::{DeclaredOrigins, NetBroker, NetRequest};
use crate::private_file::{WriteHooks, write_private};

use super::credentials::CodexOAuthCredentials;
use super::identity::{CodexAuthBackedAccount, CodexIdentity, CodexIdentityResolver};
use super::openai_dashboard_models::OpenAIDashboardSnapshot;
use super::reconciled::is_valid_header_value_str;

pub const DEFAULT_CHATGPT_ACCOUNTS_URL: &str = "https://chatgpt.com/backend-api/accounts";

pub fn normalize_workspace_account_id(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodexDashboardSourceKind {
    LiveWeb,
    CachedDashboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodexDashboardDisposition {
    Attach,
    DisplayOnly,
    FailClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodexDashboardAllowedEffect {
    /// Allows subscription dates to attach to the active Codex snapshot.
    SubscriptionMetadataAttachment,
    UsageBackfill,
    CreditsAttachment,
    RefreshGuardSeed,
    HistoricalBackfill,
    CachedDashboardReuse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodexDashboardCleanup {
    DashboardSnapshot,
    DashboardDerivedUsage,
    DashboardDerivedCredits,
    DashboardRefreshGuardSeed,
    DashboardCache,
}

impl CodexDashboardCleanup {
    pub const ALL: &'static [Self] = &[
        Self::DashboardSnapshot,
        Self::DashboardDerivedUsage,
        Self::DashboardDerivedCredits,
        Self::DashboardRefreshGuardSeed,
        Self::DashboardCache,
    ];

    pub fn all_set() -> BTreeSet<Self> {
        Self::ALL.iter().copied().collect()
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CodexDashboardKnownOwnerCandidate {
    pub identity: CodexIdentity,
    pub normalized_email: Option<String>,
    pub source_isolation_identifier: Option<String>,
}

impl std::fmt::Debug for CodexDashboardKnownOwnerCandidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexDashboardKnownOwnerCandidate")
            .field("identity", &self.identity)
            .field(
                "normalized_email",
                &self.normalized_email.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "source_isolation_identifier",
                &self.source_isolation_identifier,
            )
            .finish()
    }
}

impl CodexDashboardKnownOwnerCandidate {
    pub fn new(
        identity: CodexIdentity,
        normalized_email: Option<&str>,
        source_isolation_identifier: Option<&str>,
    ) -> Self {
        Self {
            identity,
            normalized_email: normalized_email.map(str::to_owned),
            source_isolation_identifier: source_isolation_identifier.map(str::to_owned),
        }
    }

    pub fn from_account_id_and_email(account_id: Option<&str>, email: Option<&str>) -> Self {
        Self::from_account_id_email_and_isolation(account_id, email, None)
    }

    pub fn from_account_id_email_and_isolation(
        account_id: Option<&str>,
        email: Option<&str>,
        source_isolation_identifier: Option<&str>,
    ) -> Self {
        Self::new(
            CodexIdentityResolver::resolve(account_id, email),
            CodexIdentityResolver::normalize_email(email).as_deref(),
            source_isolation_identifier,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexDashboardOwnershipProofContext {
    pub current_identity: CodexIdentity,
    pub expected_scoped_email: Option<String>,
    pub trusted_current_usage_email: Option<String>,
    pub dashboard_signed_in_email: Option<String>,
    pub dashboard_account_id: Option<String>,
    pub requires_workspace_balance_scope: bool,
    pub known_owners: Vec<CodexDashboardKnownOwnerCandidate>,
}

impl CodexDashboardOwnershipProofContext {
    pub fn new(
        current_identity: CodexIdentity,
        expected_scoped_email: Option<&str>,
        trusted_current_usage_email: Option<&str>,
        dashboard_signed_in_email: Option<&str>,
        dashboard_account_id: Option<&str>,
        requires_workspace_balance_scope: bool,
        known_owners: Vec<CodexDashboardKnownOwnerCandidate>,
    ) -> Self {
        Self {
            current_identity,
            expected_scoped_email: expected_scoped_email.map(str::to_owned),
            trusted_current_usage_email: trusted_current_usage_email.map(str::to_owned),
            dashboard_signed_in_email: dashboard_signed_in_email.map(str::to_owned),
            dashboard_account_id: dashboard_account_id.map(str::to_owned),
            requires_workspace_balance_scope,
            known_owners,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexDashboardRoutingHints {
    pub target_email: Option<String>,
    pub last_known_dashboard_routing_email: Option<String>,
}

impl CodexDashboardRoutingHints {
    pub fn new(
        target_email: Option<&str>,
        last_known_dashboard_routing_email: Option<&str>,
    ) -> Self {
        Self {
            target_email: target_email.map(str::to_owned),
            last_known_dashboard_routing_email: last_known_dashboard_routing_email
                .map(str::to_owned),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexDashboardAuthorityInput {
    pub source_kind: CodexDashboardSourceKind,
    pub proof: CodexDashboardOwnershipProofContext,
    pub routing: CodexDashboardRoutingHints,
}

impl CodexDashboardAuthorityInput {
    pub fn new(
        source_kind: CodexDashboardSourceKind,
        proof: CodexDashboardOwnershipProofContext,
        routing: CodexDashboardRoutingHints,
    ) -> Self {
        Self {
            source_kind,
            proof,
            routing,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodexDashboardDecisionReason {
    ExactProviderAccountMatch,
    TrustedEmailMatchNoCompetingOwner,
    TrustedContinuityNoCompetingOwner,
    WrongEmail {
        expected: Option<String>,
        actual: Option<String>,
    },
    SameEmailAmbiguity {
        email: String,
    },
    UnresolvedWithoutTrustedEvidence,
    ProviderAccountMissingScopedEmail,
    ProviderAccountLacksExactOwnershipProof,
    MissingDashboardSignedInEmail,
}

impl std::fmt::Debug for CodexDashboardDecisionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExactProviderAccountMatch => f.write_str("ExactProviderAccountMatch"),
            Self::TrustedEmailMatchNoCompetingOwner => {
                f.write_str("TrustedEmailMatchNoCompetingOwner")
            }
            Self::TrustedContinuityNoCompetingOwner => {
                f.write_str("TrustedContinuityNoCompetingOwner")
            }
            Self::WrongEmail { expected, actual } => f
                .debug_struct("WrongEmail")
                .field("expected", &expected.as_ref().map(|_| "[redacted]"))
                .field("actual", &actual.as_ref().map(|_| "[redacted]"))
                .finish(),
            Self::SameEmailAmbiguity { .. } => f
                .debug_struct("SameEmailAmbiguity")
                .field("email", &"[redacted]")
                .finish(),
            Self::UnresolvedWithoutTrustedEvidence => {
                f.write_str("UnresolvedWithoutTrustedEvidence")
            }
            Self::ProviderAccountMissingScopedEmail => {
                f.write_str("ProviderAccountMissingScopedEmail")
            }
            Self::ProviderAccountLacksExactOwnershipProof => {
                f.write_str("ProviderAccountLacksExactOwnershipProof")
            }
            Self::MissingDashboardSignedInEmail => f.write_str("MissingDashboardSignedInEmail"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexDashboardAuthorityDecision {
    pub disposition: CodexDashboardDisposition,
    pub reason: CodexDashboardDecisionReason,
    pub allowed_effects: BTreeSet<CodexDashboardAllowedEffect>,
    pub cleanup: BTreeSet<CodexDashboardCleanup>,
}

impl CodexDashboardAuthorityDecision {
    pub fn new(
        disposition: CodexDashboardDisposition,
        reason: CodexDashboardDecisionReason,
        allowed_effects: BTreeSet<CodexDashboardAllowedEffect>,
        cleanup: BTreeSet<CodexDashboardCleanup>,
    ) -> Self {
        Self {
            disposition,
            reason,
            allowed_effects,
            cleanup,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodexDashboardPolicyError {
    #[error("Codex dashboard may be displayed, but it cannot be attached to the active account.")]
    DisplayOnly(CodexDashboardAuthorityDecision),
}

pub struct CodexDashboardAuthority;

impl CodexDashboardAuthority {
    pub fn evaluate(input: &CodexDashboardAuthorityInput) -> CodexDashboardAuthorityDecision {
        let proof = &input.proof;
        let current_identity = Self::normalize_identity(&proof.current_identity);
        let expected_scoped_email =
            CodexIdentityResolver::normalize_email(proof.expected_scoped_email.as_deref());
        let trusted_current_usage_email =
            CodexIdentityResolver::normalize_email(proof.trusted_current_usage_email.as_deref());
        let dashboard_signed_in_email =
            CodexIdentityResolver::normalize_email(proof.dashboard_signed_in_email.as_deref());
        let known_owners = Self::normalize_known_owners(&proof.known_owners);

        // Routing hints are intentionally excluded from ownership proof.
        let Some(dashboard_signed_in_email) = dashboard_signed_in_email else {
            return Self::make_decision(
                CodexDashboardDisposition::FailClosed,
                CodexDashboardDecisionReason::MissingDashboardSignedInEmail,
                input.source_kind,
            );
        };

        if let Some(expected_scoped_email) = &expected_scoped_email
            && &dashboard_signed_in_email != expected_scoped_email
        {
            return Self::make_decision(
                CodexDashboardDisposition::FailClosed,
                CodexDashboardDecisionReason::WrongEmail {
                    expected: Some(expected_scoped_email.clone()),
                    actual: Some(dashboard_signed_in_email),
                },
                input.source_kind,
            );
        }

        if proof.requires_workspace_balance_scope {
            let matches_workspace_id = match &current_identity {
                CodexIdentity::ProviderAccount { id } => {
                    normalize_workspace_account_id(proof.dashboard_account_id.as_deref()).as_deref()
                        == Some(id.as_str())
                }
                CodexIdentity::EmailOnly { .. } | CodexIdentity::Unresolved => false,
            };
            if !matches_workspace_id {
                return Self::make_decision(
                    CodexDashboardDisposition::FailClosed,
                    CodexDashboardDecisionReason::ProviderAccountLacksExactOwnershipProof,
                    input.source_kind,
                );
            }
        }

        match current_identity {
            CodexIdentity::ProviderAccount { id } => {
                if expected_scoped_email.is_none() {
                    return Self::make_decision(
                        CodexDashboardDisposition::FailClosed,
                        CodexDashboardDecisionReason::ProviderAccountMissingScopedEmail,
                        input.source_kind,
                    );
                }
                if Self::known_owner_count(&dashboard_signed_in_email, &known_owners) > 1 {
                    return Self::make_decision(
                        CodexDashboardDisposition::DisplayOnly,
                        CodexDashboardDecisionReason::SameEmailAmbiguity {
                            email: dashboard_signed_in_email,
                        },
                        input.source_kind,
                    );
                }
                let exact_match = known_owners.iter().any(|candidate| {
                    candidate.identity == CodexIdentity::ProviderAccount { id: id.clone() }
                        && candidate.normalized_email.as_deref()
                            == Some(dashboard_signed_in_email.as_str())
                });
                if exact_match {
                    return Self::make_decision(
                        CodexDashboardDisposition::Attach,
                        CodexDashboardDecisionReason::ExactProviderAccountMatch,
                        input.source_kind,
                    );
                }
                Self::make_decision(
                    CodexDashboardDisposition::FailClosed,
                    CodexDashboardDecisionReason::ProviderAccountLacksExactOwnershipProof,
                    input.source_kind,
                )
            }
            CodexIdentity::EmailOnly { normalized_email } => {
                if dashboard_signed_in_email != normalized_email {
                    return Self::make_decision(
                        CodexDashboardDisposition::FailClosed,
                        CodexDashboardDecisionReason::WrongEmail {
                            expected: Some(normalized_email),
                            actual: Some(dashboard_signed_in_email),
                        },
                        input.source_kind,
                    );
                }
                if Self::known_owner_count(&normalized_email, &known_owners) > 1 {
                    return Self::make_decision(
                        CodexDashboardDisposition::DisplayOnly,
                        CodexDashboardDecisionReason::SameEmailAmbiguity {
                            email: normalized_email,
                        },
                        input.source_kind,
                    );
                }
                Self::make_decision(
                    CodexDashboardDisposition::Attach,
                    CodexDashboardDecisionReason::TrustedEmailMatchNoCompetingOwner,
                    input.source_kind,
                )
            }
            CodexIdentity::Unresolved => {
                let Some(trusted_current_usage_email) = trusted_current_usage_email else {
                    return Self::make_decision(
                        CodexDashboardDisposition::FailClosed,
                        CodexDashboardDecisionReason::UnresolvedWithoutTrustedEvidence,
                        input.source_kind,
                    );
                };
                if dashboard_signed_in_email != trusted_current_usage_email {
                    return Self::make_decision(
                        CodexDashboardDisposition::FailClosed,
                        CodexDashboardDecisionReason::WrongEmail {
                            expected: Some(trusted_current_usage_email),
                            actual: Some(dashboard_signed_in_email),
                        },
                        input.source_kind,
                    );
                }
                if Self::known_owner_count(&trusted_current_usage_email, &known_owners) > 1 {
                    return Self::make_decision(
                        CodexDashboardDisposition::DisplayOnly,
                        CodexDashboardDecisionReason::SameEmailAmbiguity {
                            email: trusted_current_usage_email,
                        },
                        input.source_kind,
                    );
                }
                Self::make_decision(
                    CodexDashboardDisposition::Attach,
                    CodexDashboardDecisionReason::TrustedContinuityNoCompetingOwner,
                    input.source_kind,
                )
            }
        }
    }

    fn normalize_identity(identity: &CodexIdentity) -> CodexIdentity {
        match identity {
            CodexIdentity::ProviderAccount { id } => {
                if let Some(normalized_id) = normalize_workspace_account_id(Some(id.as_str())) {
                    CodexIdentity::ProviderAccount { id: normalized_id }
                } else {
                    CodexIdentity::Unresolved
                }
            }
            CodexIdentity::EmailOnly { normalized_email } => {
                if let Some(normalized_email) =
                    CodexIdentityResolver::normalize_email(Some(normalized_email.as_str()))
                {
                    CodexIdentity::EmailOnly { normalized_email }
                } else {
                    CodexIdentity::Unresolved
                }
            }
            CodexIdentity::Unresolved => CodexIdentity::Unresolved,
        }
    }

    fn normalize_known_owners(
        candidates: &[CodexDashboardKnownOwnerCandidate],
    ) -> BTreeSet<CodexDashboardKnownOwnerCandidate> {
        candidates
            .iter()
            .map(|candidate| {
                let identity = Self::normalize_identity(&candidate.identity);
                let source_isolation_identifier = match &identity {
                    CodexIdentity::ProviderAccount { .. } => None,
                    CodexIdentity::EmailOnly { .. } | CodexIdentity::Unresolved => {
                        candidate.source_isolation_identifier.clone()
                    }
                };
                let normalized_email =
                    CodexIdentityResolver::normalize_email(candidate.normalized_email.as_deref());
                CodexDashboardKnownOwnerCandidate {
                    identity,
                    normalized_email,
                    source_isolation_identifier,
                }
            })
            .collect()
    }

    fn known_owner_count(
        email: &str,
        candidates: &BTreeSet<CodexDashboardKnownOwnerCandidate>,
    ) -> usize {
        candidates
            .iter()
            .filter(|c| c.normalized_email.as_deref() == Some(email))
            .count()
    }

    fn make_decision(
        disposition: CodexDashboardDisposition,
        reason: CodexDashboardDecisionReason,
        source_kind: CodexDashboardSourceKind,
    ) -> CodexDashboardAuthorityDecision {
        CodexDashboardAuthorityDecision::new(
            disposition,
            reason,
            Self::allowed_effects(disposition, source_kind),
            if disposition == CodexDashboardDisposition::Attach {
                BTreeSet::new()
            } else {
                CodexDashboardCleanup::all_set()
            },
        )
    }

    fn allowed_effects(
        disposition: CodexDashboardDisposition,
        source_kind: CodexDashboardSourceKind,
    ) -> BTreeSet<CodexDashboardAllowedEffect> {
        if disposition != CodexDashboardDisposition::Attach {
            return BTreeSet::new();
        }
        match source_kind {
            CodexDashboardSourceKind::LiveWeb => BTreeSet::from([
                CodexDashboardAllowedEffect::SubscriptionMetadataAttachment,
                CodexDashboardAllowedEffect::UsageBackfill,
                CodexDashboardAllowedEffect::CreditsAttachment,
                CodexDashboardAllowedEffect::RefreshGuardSeed,
                CodexDashboardAllowedEffect::HistoricalBackfill,
            ]),
            CodexDashboardSourceKind::CachedDashboard => {
                BTreeSet::from([CodexDashboardAllowedEffect::CachedDashboardReuse])
            }
        }
    }
}

pub struct CodexCLIDashboardAuthorityContext;

impl CodexCLIDashboardAuthorityContext {
    pub fn make_live_web_input(
        dashboard: &OpenAIDashboardSnapshot,
        auth: Option<&CodexAuthBackedAccount>,
        known_owners: Vec<CodexDashboardKnownOwnerCandidate>,
        routing_target_email: Option<&str>,
    ) -> CodexDashboardAuthorityInput {
        let effective_auth = auth
            .cloned()
            .unwrap_or_else(|| CodexAuthBackedAccount::new(CodexIdentity::Unresolved, None, None));

        CodexDashboardAuthorityInput::new(
            CodexDashboardSourceKind::LiveWeb,
            CodexDashboardOwnershipProofContext::new(
                effective_auth.identity,
                effective_auth.email.as_deref(),
                None,
                dashboard.signed_in_email.as_deref(),
                dashboard.account_id.as_deref(),
                dashboard.requires_workspace_balance_scope(),
                known_owners,
            ),
            CodexDashboardRoutingHints::new(
                CodexIdentityResolver::normalize_email(routing_target_email).as_deref(),
                None,
            ),
        )
    }

    pub fn make_cached_dashboard_input(
        dashboard: &OpenAIDashboardSnapshot,
        cached_account_email: &str,
        usage: Option<&UsageSnapshot>,
        source_label: Option<&str>,
        auth: Option<&CodexAuthBackedAccount>,
        known_owners: Vec<CodexDashboardKnownOwnerCandidate>,
    ) -> CodexDashboardAuthorityInput {
        let effective_auth = auth
            .cloned()
            .unwrap_or_else(|| CodexAuthBackedAccount::new(CodexIdentity::Unresolved, None, None));
        let trusted_usage_email = if Self::should_trust_usage_email(source_label) {
            usage.and_then(|u| u.identity.as_ref()?.account_email.clone())
        } else {
            None
        };

        CodexDashboardAuthorityInput::new(
            CodexDashboardSourceKind::CachedDashboard,
            CodexDashboardOwnershipProofContext::new(
                effective_auth.identity,
                effective_auth.email.as_deref(),
                trusted_usage_email.as_deref(),
                dashboard.signed_in_email.as_deref(),
                dashboard.account_id.as_deref(),
                dashboard.requires_workspace_balance_scope(),
                known_owners,
            ),
            CodexDashboardRoutingHints::new(
                effective_auth.email.as_deref(),
                Some(cached_account_email),
            ),
        )
    }

    pub fn attachment_email(input: &CodexDashboardAuthorityInput) -> Option<String> {
        let email = input
            .proof
            .expected_scoped_email
            .as_deref()
            .or(input.proof.trusted_current_usage_email.as_deref())
            .or(input.proof.dashboard_signed_in_email.as_deref());
        CodexIdentityResolver::normalize_email(email)
    }

    pub fn should_trust_usage_email(source_label: Option<&str>) -> bool {
        let Some(source_label) = source_label else {
            return false;
        };
        matches!(
            source_label.trim().to_ascii_lowercase().as_str(),
            "codex-cli" | "oauth"
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexOpenAIWorkspaceIdentity {
    pub workspace_account_id: String,
    pub workspace_label: Option<String>,
}

impl CodexOpenAIWorkspaceIdentity {
    pub fn new(workspace_account_id: &str, workspace_label: Option<&str>) -> Self {
        Self {
            workspace_account_id: Self::normalize_workspace_account_id(workspace_account_id),
            workspace_label: Self::normalize_workspace_label(workspace_label),
        }
    }

    pub fn normalize_workspace_account_id(value: &str) -> String {
        value.trim().to_ascii_lowercase()
    }

    pub fn normalize_workspace_label(value: Option<&str>) -> Option<String> {
        let trimmed = value?.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodexOpenAIWorkspaceResolverError {
    #[error("OpenAI account lookup returned an invalid response.")]
    InvalidResponse,
}

#[derive(Debug, Deserialize)]
struct AccountsResponse {
    items: Vec<AccountItem>,
}

#[derive(Debug, Deserialize)]
struct AccountItem {
    id: String,
    name: Option<String>,
}

pub struct CodexOpenAIWorkspaceResolver;

impl CodexOpenAIWorkspaceResolver {
    pub const PERSONAL_WORKSPACE_NAME: &'static str = "Personal";

    pub fn normalize_workspace_account_id(value: Option<&str>) -> Option<String> {
        normalize_workspace_account_id(value)
    }

    pub fn is_personal_workspace_name(raw: Option<&str>) -> bool {
        let Some(normalized) = CodexOpenAIWorkspaceIdentity::normalize_workspace_label(raw) else {
            return false;
        };
        normalized.eq_ignore_ascii_case(Self::PERSONAL_WORKSPACE_NAME)
    }

    pub fn resolve_workspace_account_id(raw: Option<&str>) -> Option<String> {
        normalize_workspace_account_id(raw)
    }

    pub async fn resolve(
        net: &NetBroker,
        credentials: &CodexOAuthCredentials,
    ) -> Result<Option<CodexOpenAIWorkspaceIdentity>, CodexOpenAIWorkspaceResolverError> {
        Self::resolve_with_url_and_call(net, credentials, DEFAULT_CHATGPT_ACCOUNTS_URL, None).await
    }

    pub async fn resolve_with_url_and_call(
        net: &NetBroker,
        credentials: &CodexOAuthCredentials,
        accounts_url: &str,
        call: Option<&BrokerCall>,
    ) -> Result<Option<CodexOpenAIWorkspaceIdentity>, CodexOpenAIWorkspaceResolverError> {
        let Some(workspace_account_id) =
            Self::normalize_workspace_account_id(credentials.account_id.as_deref())
        else {
            return Ok(None);
        };
        let identities =
            Self::list_workspaces_with_url_and_call(net, credentials, accounts_url, call).await?;
        if let Some(identity) = identities
            .into_iter()
            .find(|item| item.workspace_account_id == workspace_account_id)
        {
            return Ok(Some(identity));
        }
        Ok(Some(CodexOpenAIWorkspaceIdentity {
            workspace_account_id,
            workspace_label: None,
        }))
    }

    pub async fn list_workspaces(
        net: &NetBroker,
        credentials: &CodexOAuthCredentials,
    ) -> Result<Vec<CodexOpenAIWorkspaceIdentity>, CodexOpenAIWorkspaceResolverError> {
        Self::list_workspaces_with_url_and_call(
            net,
            credentials,
            DEFAULT_CHATGPT_ACCOUNTS_URL,
            None,
        )
        .await
    }

    pub async fn list_workspaces_with_url_and_call(
        net: &NetBroker,
        credentials: &CodexOAuthCredentials,
        accounts_url: &str,
        call: Option<&BrokerCall>,
    ) -> Result<Vec<CodexOpenAIWorkspaceIdentity>, CodexOpenAIWorkspaceResolverError> {
        let url = Url::parse(accounts_url)
            .map_err(|_| CodexOpenAIWorkspaceResolverError::InvalidResponse)?;
        let declared = DeclaredOrigins::from_urls([url.clone()])
            .map_err(|_| CodexOpenAIWorkspaceResolverError::InvalidResponse)?;
        let token = credentials.access_token_str().trim();
        if !is_valid_header_value_str(token) {
            return Err(CodexOpenAIWorkspaceResolverError::InvalidResponse);
        }

        let mut req = NetRequest::new(Method::GET, url);
        req.timeout = Some(Duration::from_secs(20));
        req.headers
            .insert(USER_AGENT, HeaderValue::from_static("codex-cli"));
        req.headers
            .insert(ACCEPT, HeaderValue::from_static("application/json"));
        req.credentials
            .push((AUTHORIZATION, SecretString::from(format!("Bearer {token}"))));
        if let Some(workspace_account_id) =
            Self::normalize_workspace_account_id(credentials.account_id.as_deref())
            && let Ok(val) = HeaderValue::from_str(&workspace_account_id)
        {
            req.headers
                .insert(HeaderName::from_static("chatgpt-account-id"), val);
        }

        let default_call;
        let effective_call = match call {
            Some(c) => c,
            None => {
                default_call = BrokerCall {
                    interaction: FetchInteraction::Background,
                    cancel: CancellationToken::new(),
                    request_id: "codex-workspace-resolver".to_owned(),
                };
                &default_call
            }
        };

        let response = net
            .send_response(effective_call, &declared, req)
            .await
            .map_err(|_| CodexOpenAIWorkspaceResolverError::InvalidResponse)?;

        if !(200..300).contains(&response.status.as_u16()) {
            return Err(CodexOpenAIWorkspaceResolverError::InvalidResponse);
        }

        Self::decode_accounts_response(&response.body)
    }

    pub fn decode_accounts_response(
        data: &[u8],
    ) -> Result<Vec<CodexOpenAIWorkspaceIdentity>, CodexOpenAIWorkspaceResolverError> {
        let decoded: AccountsResponse = serde_json::from_slice(data)
            .map_err(|_| CodexOpenAIWorkspaceResolverError::InvalidResponse)?;
        Ok(decoded
            .items
            .into_iter()
            .filter_map(|account| {
                let workspace_account_id = Self::normalize_workspace_account_id(Some(&account.id))?;
                let workspace_label = account
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .or_else(|| Some(Self::PERSONAL_WORKSPACE_NAME.to_owned()));
                Some(CodexOpenAIWorkspaceIdentity {
                    workspace_account_id,
                    workspace_label,
                })
            })
            .collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexOpenAIWorkspaceIdentityCachePayload {
    pub version: i32,
    #[serde(
        rename = "labelsByWorkspaceAccountID",
        alias = "workspace_names_by_account_id"
    )]
    pub labels_by_workspace_account_id: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct CodexOpenAIWorkspaceIdentityCache {
    pub file_path: Option<PathBuf>,
}

impl CodexOpenAIWorkspaceIdentityCache {
    pub const CURRENT_VERSION: i32 = 1;

    pub fn new(file_path: Option<PathBuf>) -> Self {
        Self { file_path }
    }

    pub fn normalize_workspace_label(raw: Option<&str>) -> Option<String> {
        CodexOpenAIWorkspaceIdentity::normalize_workspace_label(raw)
    }

    pub fn workspace_label(&self, workspace_account_id: Option<&str>) -> Option<String> {
        let normalized = normalize_workspace_account_id(workspace_account_id)?;
        let entries = self.load_entries();
        entries.get(&normalized).cloned()
    }

    pub fn store(&self, identity: &CodexOpenAIWorkspaceIdentity) -> std::io::Result<()> {
        self.store_workspace_label(
            Some(&identity.workspace_account_id),
            identity.workspace_label.as_deref(),
        )
    }

    pub fn store_workspace_label(
        &self,
        account_id: Option<&str>,
        workspace_label: Option<&str>,
    ) -> std::io::Result<()> {
        let (Some(normalized_account_id), Some(normalized_label)) = (
            normalize_workspace_account_id(account_id),
            Self::normalize_workspace_label(workspace_label),
        ) else {
            return Ok(());
        };
        let Some(path) = &self.file_path else {
            return Ok(());
        };
        let mut entries = self.load_entries();
        entries.insert(normalized_account_id, normalized_label);
        let payload = CodexOpenAIWorkspaceIdentityCachePayload {
            version: Self::CURRENT_VERSION,
            labels_by_workspace_account_id: entries,
        };
        let mut bytes = serde_json::to_vec_pretty(&payload).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        write_private(path, &bytes, &WriteHooks::default()).map_err(std::io::Error::other)
    }

    pub fn clear(&self) {
        if let Some(path) = &self.file_path {
            let _ = std::fs::remove_file(path);
        }
    }

    pub fn load_entries(&self) -> BTreeMap<String, String> {
        let Some(path) = &self.file_path else {
            return BTreeMap::new();
        };
        Self::load_entries_from_path(path)
    }

    pub fn load_entries_from_path(path: &Path) -> BTreeMap<String, String> {
        let Ok(bytes) = std::fs::read(path) else {
            return BTreeMap::new();
        };
        let Ok(payload) =
            serde_json::from_slice::<CodexOpenAIWorkspaceIdentityCachePayload>(&bytes)
        else {
            return BTreeMap::new();
        };
        if payload.version != Self::CURRENT_VERSION {
            return BTreeMap::new();
        }
        let mut normalized = BTreeMap::new();
        for (raw_id, raw_name) in payload.labels_by_workspace_account_id {
            if let (Some(id), Some(label)) = (
                normalize_workspace_account_id(Some(&raw_id)),
                Self::normalize_workspace_label(Some(&raw_name)),
            ) {
                normalized.insert(id, label);
            }
        }
        normalized
    }
}
