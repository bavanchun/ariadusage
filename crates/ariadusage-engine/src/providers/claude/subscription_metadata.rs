// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeWeb/ClaudeSubscriptionMetadata.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::model::UsageSnapshot;
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use reqwest::Method;
use reqwest::header::{ACCEPT, COOKIE, HeaderValue};
use serde_json::Value;

use crate::brokers::call::BrokerCall;
use crate::brokers::net::{NetBroker, NetRequest};

use super::identity::ClaudeVerifiedAccountOwner;
use super::scoped_weekly::parse_iso8601_timestamp;
use super::usage_fetcher::{ClaudeOAuthEndpointConfig, ClaudeOAuthUsageFetcher};
use super::web_fetcher::{ClaudeWebAPIFetcher, ClaudeWebEndpointConfig, SESSION_KEY_COOKIE_NAME};

pub const DEFAULT_SUBSCRIPTION_METADATA_TIMEOUT: Duration = Duration::from_secs(2);

/// One parsed billing date from Claude's `subscription_details` response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BillingDate {
    pub date: Timestamp,
    pub is_date_only: bool,
}

impl BillingDate {
    /// Converts this billing date into the Snapshot v1 wire type.
    pub fn to_protocol_billing_date(self) -> ariadusage_protocol::BillingDate {
        ariadusage_protocol::BillingDate {
            at: self.date,
            date_only: self.is_date_only,
        }
    }
}

impl From<BillingDate> for ariadusage_protocol::BillingDate {
    fn from(value: BillingDate) -> Self {
        value.to_protocol_billing_date()
    }
}

/// Error returned when `subscription_details` payload does not match the recognized schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClaudeSubscriptionParseError {
    #[error("unrecognized Claude subscription_details payload")]
    Unrecognized,
}

/// Optional dates from Claude's authenticated `subscription_details` response, separate from quota resets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSubscriptionMetadata {
    pub renews: Option<BillingDate>,
    pub expires: Option<BillingDate>,
}

impl ClaudeSubscriptionMetadata {
    /// Applies this subscription metadata (including authoritative `None` clearing) to `snapshot`.
    pub fn applying(&self, snapshot: &UsageSnapshot) -> UsageSnapshot {
        snapshot.clone().with_subscription_metadata(
            self.expires.map(|d| d.date),
            self.renews.map(|d| d.date),
            self.expires.is_some_and(|d| d.is_date_only),
            self.renews.is_some_and(|d| d.is_date_only),
        )
    }

    /// Converts this subscription metadata into the Snapshot v1 wire type.
    pub fn to_subscription_dates(&self) -> ariadusage_protocol::SubscriptionDates {
        ariadusage_protocol::SubscriptionDates {
            renews: self.renews.map(BillingDate::to_protocol_billing_date),
            expires: self.expires.map(BillingDate::to_protocol_billing_date),
        }
    }

    /// Parses a raw `subscription_details` JSON payload.
    pub fn parse(data: &[u8]) -> Result<Self, ClaudeSubscriptionParseError> {
        let value: Value =
            serde_json::from_slice(data).map_err(|_| ClaudeSubscriptionParseError::Unrecognized)?;
        let fields = value
            .as_object()
            .ok_or(ClaudeSubscriptionParseError::Unrecognized)?;

        let status = fields
            .get("status")
            .and_then(Value::as_str)
            .ok_or(ClaudeSubscriptionParseError::Unrecognized)?;
        if !matches!(status, "active" | "trialing" | "canceled") {
            return Err(ClaudeSubscriptionParseError::Unrecognized);
        }

        for required_key in [
            "next_charge_at",
            "next_charge_date",
            "plan_ending_at",
            "plan_ending_before",
        ] {
            if !fields.contains_key(required_key) {
                return Err(ClaudeSubscriptionParseError::Unrecognized);
            }
        }

        let end = match parse_billing_date(fields.get("plan_ending_at"))? {
            Some(d) => Some(d),
            None => parse_billing_date(fields.get("plan_ending_before"))?,
        };

        // A scheduled ending takes precedence over a next-charge value left in the same response.
        let renewal = if end.is_none() && matches!(status, "active" | "trialing") {
            match parse_billing_date(fields.get("next_charge_at"))? {
                Some(d) => Some(d),
                None => parse_billing_date(fields.get("next_charge_date"))?,
            }
        } else {
            None
        };

        Ok(Self {
            renews: renewal,
            expires: end,
        })
    }
}

impl From<&ClaudeSubscriptionMetadata> for ariadusage_protocol::SubscriptionDates {
    fn from(value: &ClaudeSubscriptionMetadata) -> Self {
        value.to_subscription_dates()
    }
}

impl From<ClaudeSubscriptionMetadata> for ariadusage_protocol::SubscriptionDates {
    fn from(value: ClaudeSubscriptionMetadata) -> Self {
        value.to_subscription_dates()
    }
}

fn parse_billing_date(
    value: Option<&Value>,
) -> Result<Option<BillingDate>, ClaudeSubscriptionParseError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let string = value
        .as_str()
        .ok_or(ClaudeSubscriptionParseError::Unrecognized)?;
    if string.len() == 10 {
        let date =
            parse_strict_calendar_date(string).ok_or(ClaudeSubscriptionParseError::Unrecognized)?;
        return Ok(Some(BillingDate {
            date,
            is_date_only: true,
        }));
    }
    let parsed =
        parse_iso8601_timestamp(string).ok_or(ClaudeSubscriptionParseError::Unrecognized)?;
    Ok(Some(BillingDate {
        date: parsed,
        is_date_only: false,
    }))
}

fn parse_strict_calendar_date(raw: &str) -> Option<Timestamp> {
    if raw.len() != 10 {
        return None;
    }
    let bytes = raw.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    if !bytes[0..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || !bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let civil_date: jiff::civil::Date = raw.parse().ok()?;
    if civil_date.to_string() != raw {
        return None;
    }
    let zoned = civil_date
        .at(0, 0, 0, 0)
        .to_zoned(jiff::tz::TimeZone::UTC)
        .ok()?;
    Some(zoned.timestamp())
}

/// Outcome of an optional subscription metadata fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeSubscriptionFetchResult {
    Unavailable,
    /// A recognized response with two `None` dates is authoritative empty metadata.
    Available(ClaudeSubscriptionMetadata),
}

/// Fetches optional subscription billing metadata using an existing, verified web-owner binding.
pub struct ClaudeSubscriptionMetadataFetcher;

impl ClaudeSubscriptionMetadataFetcher {
    /// Resolves ownership from the OAuth token used for the accepted usage capture.
    pub async fn oauth_owner(
        net: &Arc<NetBroker>,
        oauth_endpoint: &ClaudeOAuthEndpointConfig,
        access_token: &SecretString,
        call: &BrokerCall,
    ) -> Option<String> {
        let identity = ClaudeOAuthUsageFetcher::fetch_profile_identity(
            net,
            oauth_endpoint,
            access_token,
            call,
        )
        .await
        .ok()??;
        identity.verified_owner_id()
    }

    /// Uses ONLY an existing, verified web-owner binding. Performs no cookie discovery or cache mutation.
    #[allow(clippy::too_many_arguments)]
    pub async fn fetch(
        net: &Arc<NetBroker>,
        web_endpoint: &ClaudeWebEndpointConfig,
        oauth_endpoint: &ClaudeOAuthEndpointConfig,
        cookie_header: &SecretString,
        expected_owner_id: Option<&str>,
        oauth_access_token: Option<&SecretString>,
        timeout: Duration,
        call: &BrokerCall,
    ) -> ClaudeSubscriptionFetchResult {
        if timeout.is_zero() || call.cancel.is_cancelled() {
            return ClaudeSubscriptionFetchResult::Unavailable;
        }

        let child_cancel = call.cancel.child_token();
        let sub_call = BrokerCall {
            interaction: call.interaction,
            cancel: child_cancel.clone(),
            request_id: call.request_id.clone(),
        };

        let fut = Self::fetch_verified(
            net,
            web_endpoint,
            oauth_endpoint,
            cookie_header,
            expected_owner_id,
            oauth_access_token,
            &sub_call,
        );

        match tokio::time::timeout(timeout, fut).await {
            Ok(res) => res,
            Err(_) => {
                child_cancel.cancel();
                ClaudeSubscriptionFetchResult::Unavailable
            }
        }
    }

    async fn fetch_verified(
        net: &Arc<NetBroker>,
        web_endpoint: &ClaudeWebEndpointConfig,
        oauth_endpoint: &ClaudeOAuthEndpointConfig,
        cookie_header: &SecretString,
        expected_owner_id: Option<&str>,
        oauth_access_token: Option<&SecretString>,
        call: &BrokerCall,
    ) -> ClaudeSubscriptionFetchResult {
        let owner = if let Some(token) = oauth_access_token {
            let Some(resolved) = Self::oauth_owner(net, oauth_endpoint, token, call).await else {
                return ClaudeSubscriptionFetchResult::Unavailable;
            };
            if let Some(expected) = expected_owner_id
                && resolved != expected
            {
                return ClaudeSubscriptionFetchResult::Unavailable;
            }
            resolved
        } else {
            let Some(expected) = expected_owner_id else {
                return ClaudeSubscriptionFetchResult::Unavailable;
            };
            expected.to_owned()
        };

        let Ok(session) = ClaudeWebAPIFetcher::session_key_info(cookie_header.expose_secret())
        else {
            return ClaudeSubscriptionFetchResult::Unavailable;
        };

        let Some(first_account_data) =
            Self::get_json(net, web_endpoint, "/account", &session.key, call).await
        else {
            return ClaudeSubscriptionFetchResult::Unavailable;
        };
        let Some(org) = resolve_matching_organization(&first_account_data, &owner) else {
            return ClaudeSubscriptionFetchResult::Unavailable;
        };

        let details_path = format!("/organizations/{org}/subscription_details");
        let Some(details_data) =
            Self::get_json(net, web_endpoint, &details_path, &session.key, call).await
        else {
            return ClaudeSubscriptionFetchResult::Unavailable;
        };
        let Ok(metadata) = ClaudeSubscriptionMetadata::parse(&details_data) else {
            return ClaudeSubscriptionFetchResult::Unavailable;
        };

        // Recheck the authenticated principal + organization after the optional request.
        let Some(second_account_data) =
            Self::get_json(net, web_endpoint, "/account", &session.key, call).await
        else {
            return ClaudeSubscriptionFetchResult::Unavailable;
        };
        if resolve_matching_organization(&second_account_data, &owner).as_deref()
            != Some(org.as_str())
        {
            return ClaudeSubscriptionFetchResult::Unavailable;
        }

        if let Some(token) = oauth_access_token
            && Self::oauth_owner(net, oauth_endpoint, token, call)
                .await
                .as_deref()
                != Some(owner.as_str())
        {
            return ClaudeSubscriptionFetchResult::Unavailable;
        }

        ClaudeSubscriptionFetchResult::Available(metadata)
    }

    async fn get_json(
        net: &Arc<NetBroker>,
        web_endpoint: &ClaudeWebEndpointConfig,
        path: &str,
        session_key: &SecretString,
        call: &BrokerCall,
    ) -> Option<Vec<u8>> {
        if call.cancel.is_cancelled() {
            return None;
        }
        let url = web_endpoint.api_url(path).ok()?;
        let mut req = NetRequest::new(Method::GET, url);
        req.headers
            .insert(ACCEPT, HeaderValue::from_static("application/json"));
        let hdr_value = format!(
            "{}={}",
            SESSION_KEY_COOKIE_NAME,
            session_key.expose_secret()
        );
        req.credentials.push((COOKIE, SecretString::new(hdr_value)));

        let resp = net
            .send_response(call, &web_endpoint.declared_origins, req)
            .await
            .ok()?;
        if call.cancel.is_cancelled() || resp.status.as_u16() != 200 {
            return None;
        }
        Some(resp.body().to_vec())
    }
}

fn resolve_matching_organization(data: &[u8], expected_owner: &str) -> Option<String> {
    let value: Value = serde_json::from_slice(data).ok()?;
    let account = value.as_object()?;
    let email = account.get("email_address")?.as_str()?;
    let memberships = account.get("memberships")?.as_array()?;
    let account_uuid = account.get("uuid").and_then(Value::as_str);

    let mut matches = Vec::new();
    for membership in memberships {
        let Some(org) = membership.get("organization").and_then(Value::as_object) else {
            continue;
        };
        let Some(id) = org.get("uuid").and_then(Value::as_str) else {
            continue;
        };
        if !is_valid_uuid_string(id) {
            continue;
        }
        let candidate_without_account_uuid =
            ClaudeVerifiedAccountOwner::owner_id(None, Some(email), Some(id));
        let candidate_with_account_uuid = account_uuid
            .and_then(|au| ClaudeVerifiedAccountOwner::owner_id(Some(au), Some(email), Some(id)));

        if candidate_without_account_uuid.as_deref() == Some(expected_owner)
            || candidate_with_account_uuid.as_deref() == Some(expected_owner)
        {
            matches.push(id.to_owned());
        }
    }

    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn is_valid_uuid_string(raw: &str) -> bool {
    if raw.len() != 36 {
        return false;
    }
    for (idx, b) in raw.bytes().enumerate() {
        if matches!(idx, 8 | 13 | 18 | 23) {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}
