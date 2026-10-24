// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexReconciledState.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexPAT/CodexPATUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ariadusage_core::model::identity::ProviderIdentitySnapshot;
use ariadusage_core::model::snapshot::UsageSnapshot;
use ariadusage_core::model::window::{NamedRateWindow, RateWindow};
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::credentials::{CodexOAuthCredentials, parse_jwt_payload};
use super::identity::CodexNativeCredentialOwnerIdentity;
use super::normalizer::{
    CodexAdditionalRateLimitMapper, CodexRateWindowNormalizer, WindowSnapshot,
};
use super::openai_dashboard_models::OpenAIDashboardSnapshot;
use super::usage_fetcher::CodexUsageResponse;

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexPATWhoami {
    #[serde(default)]
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub plan_type: Option<String>,
}

impl fmt::Debug for CodexPATWhoami {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexPATWhoami")
            .field(
                "account_id",
                &self.account_id.as_ref().map(|_| "[redacted]"),
            )
            .field("email", &self.email.as_ref().map(|_| "[redacted]"))
            .field("plan_type", &self.plan_type)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CodexUsageLane {
    Session,
    Weekly,
    Monthly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexReconciledState {
    pub session: Option<RateWindow>,
    pub weekly: Option<RateWindow>,
    pub extra_rate_windows: Vec<NamedRateWindow>,
    pub subscription_expires_at: Option<Timestamp>,
    pub subscription_expires_at_is_date_only: bool,
    pub subscription_renews_at: Option<Timestamp>,
    pub subscription_renews_at_is_date_only: bool,
    pub identity: Option<ProviderIdentitySnapshot>,
    pub updated_at: Timestamp,
}

impl CodexReconciledState {
    pub fn from_cli(
        primary: Option<RateWindow>,
        secondary: Option<RateWindow>,
        identity: Option<ProviderIdentitySnapshot>,
        updated_at: Timestamp,
    ) -> Option<Self> {
        Self::make(
            primary,
            secondary,
            Vec::new(),
            None,
            false,
            None,
            false,
            identity,
            updated_at,
        )
    }

    pub fn from_oauth(
        response: &CodexUsageResponse,
        credentials: &CodexOAuthCredentials,
        updated_at: Timestamp,
    ) -> Option<Self> {
        let primary = response
            .rate_limit
            .as_ref()
            .and_then(|rl| rl.primary_window.as_ref())
            .and_then(WindowSnapshot::to_rate_window_unchecked);
        let secondary = response
            .rate_limit
            .as_ref()
            .and_then(|rl| rl.secondary_window.as_ref())
            .and_then(WindowSnapshot::to_rate_window_unchecked);
        let extra_rate_windows = CodexAdditionalRateLimitMapper::extra_rate_windows(
            response.additional_rate_limits.as_deref(),
            updated_at,
        );
        let identity = Some(Self::oauth_identity(response, credentials));
        Self::make(
            primary,
            secondary,
            extra_rate_windows,
            None,
            false,
            None,
            false,
            identity,
            updated_at,
        )
    }

    pub fn from_pat(
        response: &CodexUsageResponse,
        whoami: Option<&CodexPATWhoami>,
        updated_at: Timestamp,
    ) -> Option<Self> {
        let primary = response
            .rate_limit
            .as_ref()
            .and_then(|rl| rl.primary_window.as_ref())
            .and_then(WindowSnapshot::to_rate_window_unchecked);
        let secondary = response
            .rate_limit
            .as_ref()
            .and_then(|rl| rl.secondary_window.as_ref())
            .and_then(WindowSnapshot::to_rate_window_unchecked);
        let extra_rate_windows = CodexAdditionalRateLimitMapper::extra_rate_windows(
            response.additional_rate_limits.as_deref(),
            updated_at,
        );
        let identity = Some(Self::pat_identity(response, whoami));
        Self::make(
            primary,
            secondary,
            extra_rate_windows,
            None,
            false,
            None,
            false,
            identity,
            updated_at,
        )
    }

    pub fn from_attached_dashboard(
        snapshot: &OpenAIDashboardSnapshot,
        provider: ProviderId,
        account_email: Option<&str>,
        account_plan: Option<&str>,
    ) -> Option<Self> {
        let identity =
            Some(Self::attached_dashboard_identity(snapshot, provider, account_email, account_plan));
        Self::make(
            snapshot.primary_limit.clone(),
            snapshot.secondary_limit.clone(),
            snapshot.extra_rate_windows.clone().unwrap_or_default(),
            snapshot.subscription_expires_at,
            snapshot.subscription_expires_at_is_date_only,
            snapshot.subscription_renews_at,
            snapshot.subscription_renews_at_is_date_only,
            identity,
            snapshot.updated_at,
        )
    }

    pub fn attached_dashboard_identity(
        snapshot: &OpenAIDashboardSnapshot,
        provider: ProviderId,
        account_email: Option<&str>,
        account_plan: Option<&str>,
    ) -> ProviderIdentitySnapshot {
        let resolved_email = account_email
            .map(str::to_owned)
            .or_else(|| snapshot.signed_in_email.clone());
        let resolved_plan = account_plan
            .map(str::to_owned)
            .or_else(|| snapshot.account_plan.clone());
        ProviderIdentitySnapshot::new(Some(provider), resolved_email, None, resolved_plan, None)
    }

    pub fn to_usage_snapshot(&self, confidence: Confidence) -> UsageSnapshot {
        UsageSnapshot::new(
            self.session.clone(),
            self.weekly.clone(),
            None,
            (!self.extra_rate_windows.is_empty()).then(|| self.extra_rate_windows.clone()),
            None,
            Vec::new(),
            self.subscription_expires_at,
            self.subscription_renews_at,
            self.updated_at,
            self.identity.clone(),
            confidence,
        )
        .expect("empty details cannot exceed max sections")
        .with_subscription_metadata(
            self.subscription_expires_at,
            self.subscription_renews_at,
            self.subscription_expires_at_is_date_only,
            self.subscription_renews_at_is_date_only,
        )
    }

    pub fn oauth_identity(
        response: &CodexUsageResponse,
        credentials: &CodexOAuthCredentials,
    ) -> ProviderIdentitySnapshot {
        ProviderIdentitySnapshot::new(
            Some(ProviderId::new("codex").expect("valid provider id")),
            Self::resolve_account_email(credentials),
            None,
            Self::resolve_plan(response, credentials),
            None,
        )
    }

    pub fn pat_identity(
        response: &CodexUsageResponse,
        whoami: Option<&CodexPATWhoami>,
    ) -> ProviderIdentitySnapshot {
        ProviderIdentitySnapshot::new(
            Some(ProviderId::new("codex").expect("valid provider id")),
            whoami.and_then(|w| w.email.clone()),
            None,
            Self::resolve_pat_plan(response, whoami),
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn make(
        primary: Option<RateWindow>,
        secondary: Option<RateWindow>,
        extra_rate_windows: Vec<NamedRateWindow>,
        subscription_expires_at: Option<Timestamp>,
        subscription_expires_at_is_date_only: bool,
        subscription_renews_at: Option<Timestamp>,
        subscription_renews_at_is_date_only: bool,
        identity: Option<ProviderIdentitySnapshot>,
        updated_at: Timestamp,
    ) -> Option<Self> {
        let (session, weekly) = CodexRateWindowNormalizer::normalize(primary, secondary);
        if session.is_none() && weekly.is_none() {
            return None;
        }
        Some(Self {
            session,
            weekly,
            extra_rate_windows,
            subscription_expires_at,
            subscription_expires_at_is_date_only,
            subscription_renews_at,
            subscription_renews_at_is_date_only,
            identity,
            updated_at,
        })
    }

    fn resolve_account_email(credentials: &CodexOAuthCredentials) -> Option<String> {
        CodexNativeCredentialOwnerIdentity::normalized_email_from_id_token(
            credentials.id_token_str(),
        )
    }

    fn resolve_pat_plan(
        response: &CodexUsageResponse,
        whoami: Option<&CodexPATWhoami>,
    ) -> Option<String> {
        if let Some(plan) = response.plan_type.as_ref().map(|p| p.raw_value())
            && !plan.is_empty()
        {
            return Some(plan.to_owned());
        }
        whoami.and_then(|w| w.plan_type.clone())
    }

    fn resolve_plan(
        response: &CodexUsageResponse,
        credentials: &CodexOAuthCredentials,
    ) -> Option<String> {
        if let Some(plan) = response.plan_type.as_ref().map(|p| p.raw_value())
            && !plan.is_empty()
        {
            return Some(plan.to_owned());
        }
        Self::extract_jwt_plan(credentials)
    }

    fn extract_jwt_plan(credentials: &CodexOAuthCredentials) -> Option<String> {
        let id_token = credentials.id_token_str()?;
        let payload = parse_jwt_payload(id_token)?;
        let auth_dict = payload
            .get("https://api.openai.com/auth")
            .and_then(serde_json::Value::as_object);
        let plan = auth_dict
            .and_then(|d| d.get("chatgpt_plan_type"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                payload
                    .get("chatgpt_plan_type")
                    .and_then(serde_json::Value::as_str)
            })?;
        Some(plan.trim().to_owned())
    }
}

pub fn classify_codex_lane(window: &RateWindow, fallback: CodexUsageLane) -> CodexUsageLane {
    match window.window_minutes {
        Some(300) => CodexUsageLane::Session,
        Some(10_080) => CodexUsageLane::Weekly,
        Some(43_200) => CodexUsageLane::Monthly,
        _ => fallback,
    }
}

pub fn windows_by_lane(snapshot: &UsageSnapshot) -> BTreeMap<CodexUsageLane, RateWindow> {
    let mut result = BTreeMap::new();
    if let Some(primary) = &snapshot.primary {
        let lane = classify_codex_lane(primary, CodexUsageLane::Session);
        result.insert(lane, primary.clone());
    }
    if let Some(secondary) = &snapshot.secondary {
        let lane = classify_codex_lane(secondary, CodexUsageLane::Weekly);
        result.insert(lane, secondary.clone());
    }
    result
}

pub fn weekly_caps_session(weekly: Option<&RateWindow>, now: Timestamp) -> bool {
    let Some(weekly) = weekly else {
        return false;
    };
    if weekly.remaining_percent() > 0.0 {
        return false;
    }
    weekly.resets_at.map(|r| r > now).unwrap_or(true)
}

pub fn visible_windows(snapshot: &UsageSnapshot, now: Timestamp) -> Vec<RateWindow> {
    let mut slotted = Vec::new();
    if let Some(primary) = &snapshot.primary {
        slotted.push(classify_codex_lane(primary, CodexUsageLane::Session));
    }
    if let Some(secondary) = &snapshot.secondary {
        slotted.push(classify_codex_lane(secondary, CodexUsageLane::Weekly));
    }
    let by_lane = windows_by_lane(snapshot);
    let weekly = by_lane.get(&CodexUsageLane::Weekly);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for lane in slotted {
        if !seen.insert(lane) {
            continue;
        }
        let Some(mut window) = by_lane.get(&lane).cloned() else {
            continue;
        };
        if lane == CodexUsageLane::Session
            && weekly_caps_session(weekly, now)
            && let Ok(capped) = RateWindow::new(
                100.0,
                window.window_minutes,
                window.resets_at,
                window.reset_description.clone(),
                window.next_regen_percent,
                window.is_synthetic_placeholder,
            )
        {
            window = capped;
        }
        if window.remaining_percent() > 0.0 || window.resets_at.map(|r| r > now) != Some(false) {
            out.push(window);
        }
    }
    out
}

pub fn icon_windows(
    snapshot: &UsageSnapshot,
    now: Timestamp,
) -> (Option<RateWindow>, Option<RateWindow>) {
    let windows = visible_windows(snapshot, now);
    (windows.first().cloned(), windows.get(1).cloned())
}

pub fn plan_utilization_series(snapshot: &UsageSnapshot) -> BTreeSet<CodexUsageLane> {
    windows_by_lane(snapshot).into_keys().collect()
}
