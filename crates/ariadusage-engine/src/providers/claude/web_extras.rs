// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeUsageSnapshot+WebExtras.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::model::{NamedRateWindow, ProviderCostSnapshot, UsageSnapshot};
use ariadusage_core::pipeline::FetchError;
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::secret::SecretString;

use crate::brokers::call::BrokerCall;
use crate::brokers::cookie_cache::CookieCache;
use crate::brokers::net::NetBroker;

use super::identity::ClaudeAccountIdentity;
use super::usage_fetcher::{
    ClaudeOAuthEndpointConfig, ClaudeOAuthMappedUsage, ClaudeOAuthUsageFetcher,
};
use super::web_fetcher::{
    ClaudeWebAPIFetcher, ClaudeWebBrowserFetchGate, ClaudeWebEndpointConfig, ClaudeWebFetchOptions,
    ClaudeWebSessionImporter, WebUsageData,
};

/// Replaces web-enriched fields (`extra_rate_windows` and `provider_cost`) on a `UsageSnapshot`
/// while preserving all primary source identity, confidence, and detail sections.
pub fn replacing_web_extras(
    snapshot: &UsageSnapshot,
    extra_rate_windows: Vec<NamedRateWindow>,
    provider_cost: Option<ProviderCostSnapshot>,
) -> UsageSnapshot {
    let extra_opt = if extra_rate_windows.is_empty() {
        None
    } else {
        Some(extra_rate_windows)
    };
    snapshot
        .clone()
        .with_extra_rate_windows(extra_opt)
        .with_provider_cost(provider_cost)
}

impl ClaudeOAuthMappedUsage {
    /// Replaces web-enriched fields on the inner `UsageSnapshot` while preserving
    /// `history_owner_id`, `reset_credits`, and `cloud_credits`.
    pub fn replacing_web_extras(
        &self,
        extra_rate_windows: Vec<NamedRateWindow>,
        provider_cost: Option<ProviderCostSnapshot>,
    ) -> Self {
        Self {
            snapshot: replacing_web_extras(&self.snapshot, extra_rate_windows, provider_cost),
            reset_credits: self.reset_credits.clone(),
            cloud_credits: self.cloud_credits.clone(),
            history_owner_id: self.history_owner_id.clone(),
        }
    }
}

/// Checks whether a web usage response belongs to the same account and organization
/// as the primary OAuth or CLI usage snapshot.
pub fn web_extras_account_matches(
    snapshot: &UsageSnapshot,
    web_data: &WebUsageData,
    oauth_profile: Option<&ClaudeAccountIdentity>,
) -> bool {
    let snapshot_email = snapshot
        .identity
        .as_ref()
        .and_then(|id| id.account_email.as_deref());
    let primary_email = normalized_account_field(
        oauth_profile
            .and_then(|p| p.email.as_deref())
            .or(snapshot_email),
    );
    let web_email = normalized_account_field(web_data.account_email.as_deref());
    if let (Some(pe), Some(we)) = (&primary_email, &web_email)
        && pe != we
    {
        return false;
    }

    let snapshot_org = snapshot
        .identity
        .as_ref()
        .and_then(|id| id.account_organization.as_deref());
    let primary_organization = normalized_account_field(
        oauth_profile
            .and_then(|p| p.organization_uuid.as_deref())
            .or(snapshot_org),
    );
    let web_organization = normalized_account_field(if oauth_profile.is_none() {
        web_data.account_organization.as_deref()
    } else {
        web_data.account_organization_id.as_deref()
    });
    if let (Some(po), Some(wo)) = (&primary_organization, &web_organization)
        && po != wo
    {
        return false;
    }

    let email_matches = primary_email.is_some() && primary_email == web_email;
    let oauth_organization =
        normalized_account_field(oauth_profile.and_then(|p| p.organization_uuid.as_deref()));
    let organization_matches =
        oauth_organization.is_some() && oauth_organization == web_organization;
    email_matches || organization_matches
}

/// Merges a web `ProviderCostSnapshot` (extra usage / prepaid balance) into the primary cost snapshot.
pub fn merge_provider_cost(
    primary: Option<ProviderCostSnapshot>,
    web: Option<ProviderCostSnapshot>,
    include_usage_details: bool,
) -> Option<ProviderCostSnapshot> {
    let Some(web) = web else {
        return primary;
    };
    let Some(mut primary) = primary else {
        if include_usage_details {
            return Some(web);
        }
        let balance = web.balance?;
        let mut snapshot = ProviderCostSnapshot::new(0.0, 0.0, web.currency_code, web.updated_at);
        snapshot.period = web.period;
        snapshot.balance = Some(balance);
        return Some(snapshot);
    };

    if let Some(balance) = web.balance
        && primary
            .currency_code
            .eq_ignore_ascii_case(&web.currency_code)
    {
        primary.balance = Some(balance);
    }
    Some(primary)
}

/// Merges web `extra_rate_windows` into the primary `extra_rate_windows`, deduplicating
/// by window ID and by model-scoped weekly key when unambiguous.
pub fn merge_extra_rate_windows(
    primary: &[NamedRateWindow],
    web: &[NamedRateWindow],
) -> Vec<NamedRateWindow> {
    if primary.is_empty() {
        return web.to_vec();
    }
    if web.is_empty() {
        return primary.to_vec();
    }

    let primary_scoped_keys: BTreeSet<String> = primary
        .iter()
        .filter_map(scoped_extra_rate_window_merge_key)
        .collect();

    let mut web_scoped_ids_by_key: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for window in web {
        if let Some(scoped_key) = scoped_extra_rate_window_merge_key(window) {
            web_scoped_ids_by_key
                .entry(scoped_key)
                .or_default()
                .insert(window.id.clone());
        }
    }

    let mut seen_ids: BTreeSet<String> = primary.iter().map(|w| w.id.clone()).collect();
    let mut merged = primary.to_vec();

    for window in web {
        if !seen_ids.insert(window.id.clone()) {
            continue;
        }
        if let Some(scoped_key) = scoped_extra_rate_window_merge_key(window)
            && primary_scoped_keys.contains(&scoped_key)
            && web_scoped_ids_by_key
                .get(&scoped_key)
                .is_some_and(|ids| ids.len() == 1)
        {
            continue;
        }
        merged.push(window.clone());
    }

    merged
}

/// Parameters for `apply_web_extras_if_needed`.
pub struct WebExtrasApplyParams<'a> {
    pub net: &'a Arc<NetBroker>,
    pub oauth_endpoint: &'a ClaudeOAuthEndpointConfig,
    pub web_endpoint: &'a ClaudeWebEndpointConfig,
    pub cookie_cache: &'a CookieCache,
    pub importer: &'a ClaudeWebSessionImporter,
    pub browser_gate: &'a ClaudeWebBrowserFetchGate,
    pub manual_cookie_header: Option<&'a str>,
    pub oauth_access_token: Option<&'a SecretString>,
    pub oauth_profile: Option<&'a ClaudeAccountIdentity>,
    pub use_web_extras: bool,
    pub include_prepaid_balance: bool,
    pub web_extras_timeout: Option<Duration>,
    pub options: ClaudeWebFetchOptions,
    pub call: &'a BrokerCall,
}

/// Enriches a primary OAuth or CLI `UsageSnapshot` with web extras / prepaid balance when enabled
/// and the web account matches the primary account.
pub async fn apply_web_extras_if_needed(
    snapshot: UsageSnapshot,
    params: WebExtrasApplyParams<'_>,
) -> Result<UsageSnapshot, FetchError> {
    if !params.use_web_extras && !params.include_prepaid_balance {
        return Ok(snapshot);
    }
    let Some(timeout) = params.web_extras_timeout else {
        return Ok(snapshot);
    };
    if params.call.cancel.is_cancelled() {
        return Err(FetchError::Cancelled);
    }

    let child_cancel = params.call.cancel.child_token();
    let sub_call = BrokerCall {
        interaction: params.call.interaction,
        cancel: child_cancel.clone(),
        request_id: params.call.request_id.clone(),
    };

    let mut fetch_options = params.options.clone();
    fetch_options.include_usage_details = params.use_web_extras;
    fetch_options.include_prepaid_balance = params.include_prepaid_balance;

    let work = async {
        let resolved_profile = match params.oauth_profile {
            Some(existing) => Some(existing.clone()),
            None => {
                if let Some(token) = params.oauth_access_token {
                    Some(
                        ClaudeOAuthUsageFetcher::fetch_profile_identity_strict(
                            params.net,
                            params.oauth_endpoint,
                            token,
                            &sub_call,
                        )
                        .await
                        .map_err(|e| e.into_fetch_error())?,
                    )
                } else {
                    None
                }
            }
        };

        let web_data = if let Some(header) = params.manual_cookie_header {
            ClaudeWebAPIFetcher::fetch_usage_with_cookie_header(
                params.net,
                params.web_endpoint,
                header,
                &fetch_options,
                &sub_call,
            )
            .await
            .map_err(|e| e.into_fetch_error())?
        } else {
            ClaudeWebAPIFetcher::fetch_usage_serialized(
                params.net,
                params.web_endpoint,
                params.cookie_cache,
                params.importer,
                params.browser_gate,
                &fetch_options,
                &sub_call,
            )
            .await
            .map_err(|e| e.into_fetch_error())?
        };

        Ok::<(WebUsageData, Option<ClaudeAccountIdentity>), FetchError>((
            web_data,
            resolved_profile,
        ))
    };

    match tokio::time::timeout(timeout, work).await {
        Ok(Ok((web_data, resolved_profile))) => {
            if params.call.cancel.is_cancelled() {
                return Err(FetchError::Cancelled);
            }
            let snapshot = with_profile_email(
                snapshot,
                resolved_profile.as_ref().and_then(|p| p.email.as_deref()),
            );
            if !web_extras_account_matches(&snapshot, &web_data, resolved_profile.as_ref()) {
                return Ok(snapshot);
            }
            let current_extras = snapshot.extra_rate_windows.as_deref().unwrap_or(&[]);
            let merged_extra_windows = if params.use_web_extras {
                merge_extra_rate_windows(current_extras, &web_data.extra_rate_windows)
            } else {
                current_extras.to_vec()
            };
            let merged_cost = merge_provider_cost(
                snapshot.provider_cost.clone(),
                web_data.extra_usage_cost,
                params.use_web_extras,
            );
            Ok(replacing_web_extras(
                &snapshot,
                merged_extra_windows,
                merged_cost,
            ))
        }
        Ok(Err(FetchError::Cancelled)) => Err(FetchError::Cancelled),
        Ok(Err(_)) => {
            if params.call.cancel.is_cancelled() {
                Err(FetchError::Cancelled)
            } else {
                Ok(snapshot)
            }
        }
        Err(_) => {
            child_cancel.cancel();
            if params.call.cancel.is_cancelled() {
                Err(FetchError::Cancelled)
            } else {
                Ok(snapshot)
            }
        }
    }
}

fn with_profile_email(snapshot: UsageSnapshot, email: Option<&str>) -> UsageSnapshot {
    let Some(email) = email.map(str::trim).filter(|s| !s.is_empty()) else {
        return snapshot;
    };
    let mut updated = snapshot;
    match &mut updated.identity {
        Some(identity) => {
            identity.account_email = Some(email.to_owned());
        }
        None => {
            updated.identity = Some(ariadusage_core::model::ProviderIdentitySnapshot {
                provider_id: ProviderId::new("claude").ok(),
                account_email: Some(email.to_owned()),
                account_organization: None,
                login_method: None,
                account_id: None,
            });
        }
    }
    updated
}

fn normalized_account_field(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim().to_lowercase();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn strip_trailing_only(title: &str) -> &str {
    let trimmed = title.trim_end();
    if trimmed.len() > 4 {
        let (head, tail) = trimmed.split_at(trimmed.len() - 4);
        if tail.eq_ignore_ascii_case("only")
            && head.chars().next_back().is_some_and(char::is_whitespace)
        {
            return head.trim_end();
        }
    }
    title
}

fn scoped_extra_rate_window_merge_key(window: &NamedRateWindow) -> Option<String> {
    if !window.id.starts_with("claude-weekly-scoped-") {
        return None;
    }
    let without_only = strip_trailing_only(&window.title);
    let normalized: String = without_only
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}
