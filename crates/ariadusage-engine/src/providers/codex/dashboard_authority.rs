// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexDashboardAuthority.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexCLIDashboardAuthorityContext.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexOpenAIWorkspaceResolver.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexOpenAIWorkspaceIdentityCache.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ariadusage_core::model::snapshot::UsageSnapshot;
use serde::{Deserialize, Serialize};

use crate::fs_private::write_private;

use super::identity::{CodexAuthBackedAccount, CodexIdentity, CodexIdentityResolver};
use super::openai_dashboard_models::OpenAIDashboardSnapshot;

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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CodexDashboardKnownOwnerCandidate {
    pub identity: CodexIdentity,
    pub normalized_email: Option<String>,
    pub source_isolation_identifier: Option<String>,
}

impl CodexDashboardKnownOwnerCandidate {
    pub fn new(
        identity: CodexIdentity,
        email: Option<&str>,
        source_isolation_identifier: Option<&str>,
    ) -> Self {
        let normalized_isolation = source_isolation_identifier
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        Self {
            identity,
            normalized_email: CodexIdentityResolver::normalize_email(email),
            source_isolation_identifier: normalized_isolation,
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
            email,
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
            expected_scoped_email: CodexIdentityResolver::normalize_email(expected_scoped_email),
            trusted_current_usage_email: CodexIdentityResolver::normalize_email(
                trusted_current_usage_email,
            ),
            dashboard_signed_in_email: CodexIdentityResolver::normalize_email(
                dashboard_signed_in_email,
            ),
            dashboard_account_id: normalize_workspace_account_id(dashboard_account_id),
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
            target_email: CodexIdentityResolver::normalize_email(target_email),
            last_known_dashboard_routing_email: CodexIdentityResolver::normalize_email(
                last_known_dashboard_routing_email,
            ),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodexDashboardDecisionReason {
    ExactProviderAccountWithoutCompetingWorkspaces,
    UniqueEmailBackedOwnerMatched,
    UnresolvedWithoutTrustedEvidence,
    SameEmailAmbiguity,
    MissingDashboardSignedInEmail,
    DashboardEmailMismatch { expected: String, actual: String },
    ProviderAccountLacksExactOwnershipProof,
    ProviderAccountCollisionWithoutExactMatch,
    UnresolvedWithKnownOwnersPresent,
    WrongEmail { expected: String, actual: String },
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
    #[error("Codex dashboard policy returned display-only")]
    DisplayOnly(CodexDashboardAuthorityDecision),
}

pub struct CodexDashboardAuthority;

impl CodexDashboardAuthority {
    pub fn evaluate(input: &CodexDashboardAuthorityInput) -> CodexDashboardAuthorityDecision {
        let proof = &input.proof;
        let expected_email = proof
            .expected_scoped_email
            .as_deref()
            .or(proof.trusted_current_usage_email.as_deref());
        let Some(dashboard_email) = proof.dashboard_signed_in_email.as_deref() else {
            return CodexDashboardAuthorityDecision::new(
                CodexDashboardDisposition::FailClosed,
                CodexDashboardDecisionReason::MissingDashboardSignedInEmail,
                BTreeSet::new(),
                Self::fail_closed_cleanup(input.source_kind),
            );
        };

        if let Some(expected_email) = expected_email
            && expected_email != dashboard_email
        {
            let reason = if matches!(input.source_kind, CodexDashboardSourceKind::CachedDashboard)
                && proof.expected_scoped_email.is_none()
            {
                CodexDashboardDecisionReason::DashboardEmailMismatch {
                    expected: expected_email.to_owned(),
                    actual: dashboard_email.to_owned(),
                }
            } else {
                CodexDashboardDecisionReason::WrongEmail {
                    expected: expected_email.to_owned(),
                    actual: dashboard_email.to_owned(),
                }
            };
            return CodexDashboardAuthorityDecision::new(
                CodexDashboardDisposition::FailClosed,
                reason,
                BTreeSet::new(),
                Self::fail_closed_cleanup(input.source_kind),
            );
        }

        if proof.requires_workspace_balance_scope {
            let CodexIdentity::ProviderAccount { id: account_id } = &proof.current_identity else {
                return CodexDashboardAuthorityDecision::new(
                    CodexDashboardDisposition::FailClosed,
                    CodexDashboardDecisionReason::ProviderAccountLacksExactOwnershipProof,
                    BTreeSet::new(),
                    Self::fail_closed_cleanup(input.source_kind),
                );
            };
            let normalized_account_id = normalize_workspace_account_id(Some(account_id.as_str()));
            if proof.dashboard_account_id.is_none()
                || proof.dashboard_account_id != normalized_account_id
            {
                return CodexDashboardAuthorityDecision::new(
                    CodexDashboardDisposition::FailClosed,
                    CodexDashboardDecisionReason::ProviderAccountLacksExactOwnershipProof,
                    BTreeSet::new(),
                    Self::fail_closed_cleanup(input.source_kind),
                );
            }
        }

        let normalized_owners = Self::normalize_known_owners(&proof.known_owners);
        let matching_email_owners: Vec<&CodexDashboardKnownOwnerCandidate> = normalized_owners
            .iter()
            .filter(|c| c.normalized_email.as_deref() == Some(dashboard_email))
            .collect();

        match &proof.current_identity {
            CodexIdentity::ProviderAccount { id: account_id } => Self::evaluate_provider_account(
                account_id,
                dashboard_email,
                &matching_email_owners,
                input,
            ),
            CodexIdentity::EmailOnly { normalized_email } => Self::evaluate_email_only(
                normalized_email,
                dashboard_email,
                &matching_email_owners,
                input,
            ),
            CodexIdentity::Unresolved => Self::evaluate_unresolved(&normalized_owners, input),
        }
    }

    fn evaluate_provider_account(
        account_id: &str,
        dashboard_email: &str,
        matching_email_owners: &[&CodexDashboardKnownOwnerCandidate],
        input: &CodexDashboardAuthorityInput,
    ) -> CodexDashboardAuthorityDecision {
        let Some(expected_email) = input
            .proof
            .expected_scoped_email
            .as_deref()
            .or(input.proof.trusted_current_usage_email.as_deref())
        else {
            return CodexDashboardAuthorityDecision::new(
                CodexDashboardDisposition::FailClosed,
                CodexDashboardDecisionReason::ProviderAccountLacksExactOwnershipProof,
                BTreeSet::new(),
                Self::fail_closed_cleanup(input.source_kind),
            );
        };

        if expected_email != dashboard_email {
            return CodexDashboardAuthorityDecision::new(
                CodexDashboardDisposition::FailClosed,
                CodexDashboardDecisionReason::WrongEmail {
                    expected: expected_email.to_owned(),
                    actual: dashboard_email.to_owned(),
                },
                BTreeSet::new(),
                Self::fail_closed_cleanup(input.source_kind),
            );
        }

        let competing_provider_accounts: BTreeSet<&str> = matching_email_owners
            .iter()
            .filter_map(|candidate| match &candidate.identity {
                CodexIdentity::ProviderAccount { id } => Some(id.as_str()),
                CodexIdentity::EmailOnly { .. } | CodexIdentity::Unresolved => None,
            })
            .collect();

        if competing_provider_accounts.len() > 1 {
            return CodexDashboardAuthorityDecision::new(
                CodexDashboardDisposition::FailClosed,
                CodexDashboardDecisionReason::ProviderAccountCollisionWithoutExactMatch,
                BTreeSet::new(),
                Self::fail_closed_cleanup(input.source_kind),
            );
        }

        if let Some(only_account_id) = competing_provider_accounts.iter().next().copied()
            && only_account_id != account_id
        {
            return CodexDashboardAuthorityDecision::new(
                CodexDashboardDisposition::FailClosed,
                CodexDashboardDecisionReason::ProviderAccountCollisionWithoutExactMatch,
                BTreeSet::new(),
                Self::fail_closed_cleanup(input.source_kind),
            );
        }

        let has_competing_email_only_owner = matching_email_owners
            .iter()
            .any(|c| matches!(c.identity, CodexIdentity::EmailOnly { .. }));

        if has_competing_email_only_owner {
            let disposition = match input.source_kind {
                CodexDashboardSourceKind::LiveWeb => CodexDashboardDisposition::DisplayOnly,
                CodexDashboardSourceKind::CachedDashboard => CodexDashboardDisposition::FailClosed,
            };
            return CodexDashboardAuthorityDecision::new(
                disposition,
                CodexDashboardDecisionReason::SameEmailAmbiguity,
                BTreeSet::new(),
                Self::all_cleanup(),
            );
        }

        CodexDashboardAuthorityDecision::new(
            CodexDashboardDisposition::Attach,
            CodexDashboardDecisionReason::ExactProviderAccountWithoutCompetingWorkspaces,
            Self::attach_effects(input.source_kind),
            BTreeSet::new(),
        )
    }

    fn evaluate_email_only(
        current_email: &str,
        dashboard_email: &str,
        matching_email_owners: &[&CodexDashboardKnownOwnerCandidate],
        input: &CodexDashboardAuthorityInput,
    ) -> CodexDashboardAuthorityDecision {
        if current_email != dashboard_email {
            return CodexDashboardAuthorityDecision::new(
                CodexDashboardDisposition::FailClosed,
                CodexDashboardDecisionReason::WrongEmail {
                    expected: current_email.to_owned(),
                    actual: dashboard_email.to_owned(),
                },
                BTreeSet::new(),
                Self::fail_closed_cleanup(input.source_kind),
            );
        }

        let has_provider_account_for_same_email = matching_email_owners
            .iter()
            .any(|c| matches!(c.identity, CodexIdentity::ProviderAccount { .. }));

        let known_owner_count = matching_email_owners.len();
        if has_provider_account_for_same_email || known_owner_count > 1 {
            let disposition = match input.source_kind {
                CodexDashboardSourceKind::LiveWeb => CodexDashboardDisposition::DisplayOnly,
                CodexDashboardSourceKind::CachedDashboard => CodexDashboardDisposition::FailClosed,
            };
            return CodexDashboardAuthorityDecision::new(
                disposition,
                CodexDashboardDecisionReason::SameEmailAmbiguity,
                BTreeSet::new(),
                Self::all_cleanup(),
            );
        }

        CodexDashboardAuthorityDecision::new(
            CodexDashboardDisposition::Attach,
            CodexDashboardDecisionReason::UniqueEmailBackedOwnerMatched,
            Self::attach_effects(input.source_kind),
            BTreeSet::new(),
        )
    }

    fn evaluate_unresolved(
        normalized_owners: &[CodexDashboardKnownOwnerCandidate],
        input: &CodexDashboardAuthorityInput,
    ) -> CodexDashboardAuthorityDecision {
        let has_any_known_owner = normalized_owners
            .iter()
            .any(|c| c.identity != CodexIdentity::Unresolved || c.normalized_email.is_some());

        if has_any_known_owner {
            return CodexDashboardAuthorityDecision::new(
                CodexDashboardDisposition::FailClosed,
                CodexDashboardDecisionReason::UnresolvedWithKnownOwnersPresent,
                BTreeSet::new(),
                Self::fail_closed_cleanup(input.source_kind),
            );
        }

        if input.proof.expected_scoped_email.is_none()
            && input.proof.trusted_current_usage_email.is_none()
        {
            return CodexDashboardAuthorityDecision::new(
                CodexDashboardDisposition::Attach,
                CodexDashboardDecisionReason::UnresolvedWithoutTrustedEvidence,
                Self::attach_effects(input.source_kind),
                BTreeSet::new(),
            );
        }

        CodexDashboardAuthorityDecision::new(
            CodexDashboardDisposition::Attach,
            CodexDashboardDecisionReason::UniqueEmailBackedOwnerMatched,
            Self::attach_effects(input.source_kind),
            BTreeSet::new(),
        )
    }

    fn normalize_known_owners(
        owners: &[CodexDashboardKnownOwnerCandidate],
    ) -> Vec<CodexDashboardKnownOwnerCandidate> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for owner in owners {
            let normalized_identity = match &owner.identity {
                CodexIdentity::ProviderAccount { id } => {
                    CodexIdentityResolver::resolve(Some(id.as_str()), None)
                }
                CodexIdentity::EmailOnly { normalized_email } => {
                    CodexIdentityResolver::resolve(None, Some(normalized_email.as_str()))
                }
                CodexIdentity::Unresolved => {
                    CodexIdentityResolver::resolve(None, owner.normalized_email.as_deref())
                }
            };
            let source_isolation_identifier = match &normalized_identity {
                CodexIdentity::ProviderAccount { .. } => None,
                CodexIdentity::EmailOnly { .. } | CodexIdentity::Unresolved => {
                    owner.source_isolation_identifier.as_deref()
                }
            };
            let candidate = CodexDashboardKnownOwnerCandidate::new(
                normalized_identity,
                owner.normalized_email.as_deref(),
                source_isolation_identifier,
            );
            if seen.insert(candidate.clone()) {
                out.push(candidate);
            }
        }
        out
    }

    fn attach_effects(
        source_kind: CodexDashboardSourceKind,
    ) -> BTreeSet<CodexDashboardAllowedEffect> {
        let mut effects = BTreeSet::from([
            CodexDashboardAllowedEffect::SubscriptionMetadataAttachment,
            CodexDashboardAllowedEffect::UsageBackfill,
            CodexDashboardAllowedEffect::CreditsAttachment,
            CodexDashboardAllowedEffect::RefreshGuardSeed,
            CodexDashboardAllowedEffect::HistoricalBackfill,
        ]);
        if source_kind == CodexDashboardSourceKind::CachedDashboard {
            effects.insert(CodexDashboardAllowedEffect::CachedDashboardReuse);
        }
        effects
    }

    fn all_cleanup() -> BTreeSet<CodexDashboardCleanup> {
        BTreeSet::from([
            CodexDashboardCleanup::DashboardSnapshot,
            CodexDashboardCleanup::DashboardDerivedUsage,
            CodexDashboardCleanup::DashboardDerivedCredits,
            CodexDashboardCleanup::DashboardRefreshGuardSeed,
            CodexDashboardCleanup::DashboardCache,
        ])
    }

    fn fail_closed_cleanup(
        source_kind: CodexDashboardSourceKind,
    ) -> BTreeSet<CodexDashboardCleanup> {
        match source_kind {
            CodexDashboardSourceKind::LiveWeb => BTreeSet::from([
                CodexDashboardCleanup::DashboardSnapshot,
                CodexDashboardCleanup::DashboardDerivedUsage,
                CodexDashboardCleanup::DashboardDerivedCredits,
                CodexDashboardCleanup::DashboardRefreshGuardSeed,
            ]),
            CodexDashboardSourceKind::CachedDashboard => Self::all_cleanup(),
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
        let normalized_auth_email = effective_auth.normalized_email();
        let requires_workspace_balance_scope = dashboard.balance_is_workspace == Some(true)
            || (dashboard.credits_available == Some(true) && dashboard.credits_remaining.is_none());

        CodexDashboardAuthorityInput::new(
            CodexDashboardSourceKind::LiveWeb,
            CodexDashboardOwnershipProofContext::new(
                effective_auth.identity,
                normalized_auth_email.as_deref(),
                None,
                dashboard.signed_in_email.as_deref(),
                dashboard.account_id.as_deref(),
                requires_workspace_balance_scope,
                known_owners,
            ),
            CodexDashboardRoutingHints::new(
                routing_target_email.or(normalized_auth_email.as_deref()),
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
        let normalized_auth_email = effective_auth.normalized_email();
        let trusted_usage_email = if Self::should_trust_usage_email(source_label) {
            CodexIdentityResolver::normalize_email(
                usage.and_then(|u| u.identity.as_ref()?.account_email.as_deref()),
            )
        } else {
            None
        };
        let current_identity = if effective_auth.identity != CodexIdentity::Unresolved {
            effective_auth.identity
        } else {
            CodexIdentityResolver::resolve(None, trusted_usage_email.as_deref())
        };
        let dashboard_email = dashboard
            .signed_in_email
            .as_deref()
            .unwrap_or(cached_account_email);
        let requires_workspace_balance_scope = dashboard.balance_is_workspace == Some(true)
            || (dashboard.credits_available == Some(true) && dashboard.credits_remaining.is_none());

        CodexDashboardAuthorityInput::new(
            CodexDashboardSourceKind::CachedDashboard,
            CodexDashboardOwnershipProofContext::new(
                current_identity,
                normalized_auth_email.as_deref(),
                trusted_usage_email.as_deref(),
                Some(dashboard_email),
                dashboard.account_id.as_deref(),
                requires_workspace_balance_scope,
                known_owners,
            ),
            CodexDashboardRoutingHints::new(
                normalized_auth_email
                    .as_deref()
                    .or(trusted_usage_email.as_deref()),
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
pub struct CodexOpenAIWorkspaceIdentityCachePayload {
    pub version: i32,
    pub workspace_names_by_account_id: BTreeMap<String, String>,
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
        let trimmed = raw?.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }

    pub fn workspace_label(&self, account_id: Option<&str>) -> Option<String> {
        let normalized_account_id = normalize_workspace_account_id(account_id)?;
        let entries = self.load_entries();
        entries.get(&normalized_account_id).cloned()
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
        if entries.get(&normalized_account_id) == Some(&normalized_label) {
            return Ok(());
        }
        entries.insert(normalized_account_id, normalized_label);
        let payload = CodexOpenAIWorkspaceIdentityCachePayload {
            version: Self::CURRENT_VERSION,
            workspace_names_by_account_id: entries,
        };
        let mut bytes = serde_json::to_vec_pretty(&payload).map_err(std::io::Error::other)?;
        bytes.push(b'\n');
        write_private(path, &bytes)
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
        let Ok(payload) = serde_json::from_slice::<CodexOpenAIWorkspaceIdentityCachePayload>(&bytes)
        else {
            return BTreeMap::new();
        };
        if payload.version != Self::CURRENT_VERSION {
            return BTreeMap::new();
        }
        let mut normalized = BTreeMap::new();
        for (raw_id, raw_name) in payload.workspace_names_by_account_id {
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

pub struct CodexOpenAIWorkspaceResolver;

impl CodexOpenAIWorkspaceResolver {
    pub const PERSONAL_WORKSPACE_NAME: &'static str = "Personal";

    pub fn is_personal_workspace_name(raw: Option<&str>) -> bool {
        let Some(normalized) = CodexOpenAIWorkspaceIdentityCache::normalize_workspace_label(raw)
        else {
            return false;
        };
        normalized.eq_ignore_ascii_case(Self::PERSONAL_WORKSPACE_NAME)
    }

    pub fn resolve_workspace_account_id(raw: Option<&str>) -> Option<String> {
        normalize_workspace_account_id(raw)
    }
}
