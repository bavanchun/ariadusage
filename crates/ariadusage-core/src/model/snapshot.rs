// Ported from CodexBar Sources/CodexBarCore/UsageFetcher.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/UsageSnapshot+AccountLabel.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_protocol::{Confidence, DetailRow, DetailSection, ProviderId};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::ModelError;
use crate::model::cost::ProviderCostSnapshot;
use crate::model::identity::ProviderIdentitySnapshot;
use crate::model::window::{NamedRateWindow, RateWindow};

/// Normalized provider usage snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSnapshot {
    pub primary: Option<RateWindow>,
    pub secondary: Option<RateWindow>,
    pub tertiary: Option<RateWindow>,
    pub extra_rate_windows: Option<Vec<NamedRateWindow>>,
    pub provider_cost: Option<ProviderCostSnapshot>,
    pub details: Vec<DetailSection>,
    pub subscription_expires_at: Option<jiff::Timestamp>,
    pub subscription_renews_at: Option<jiff::Timestamp>,
    pub updated_at: jiff::Timestamp,
    pub identity: Option<ProviderIdentitySnapshot>,
    pub data_confidence: Confidence,
}

impl UsageSnapshot {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        primary: Option<RateWindow>,
        secondary: Option<RateWindow>,
        tertiary: Option<RateWindow>,
        extra_rate_windows: Option<Vec<NamedRateWindow>>,
        provider_cost: Option<ProviderCostSnapshot>,
        details: Vec<DetailSection>,
        subscription_expires_at: Option<jiff::Timestamp>,
        subscription_renews_at: Option<jiff::Timestamp>,
        updated_at: jiff::Timestamp,
        identity: Option<ProviderIdentitySnapshot>,
        data_confidence: Confidence,
    ) -> Result<Self, ModelError> {
        DetailSection::validate_sections(&details)
            .map_err(|_| ModelError::TooManySections(details.len()))?;

        Ok(Self {
            primary,
            secondary,
            tertiary,
            extra_rate_windows,
            provider_cost,
            details,
            subscription_expires_at,
            subscription_renews_at,
            updated_at,
            identity,
            data_confidence,
        })
    }

    pub fn identity_for(&self, provider_id: &ProviderId) -> Option<&ProviderIdentitySnapshot> {
        self.identity
            .as_ref()
            .filter(|id| id.provider_id.as_ref() == Some(provider_id))
    }

    pub fn with_account_label(&self, label: &str, provider_id: &ProviderId) -> Self {
        let label = label.trim();
        if label.is_empty() {
            return self.clone();
        }
        let existing = self.identity_for(provider_id);
        let email = existing
            .and_then(|id| id.account_email.as_deref())
            .map(str::trim)
            .filter(|e| !e.is_empty());
        let resolved_email = email
            .map(str::to_string)
            .unwrap_or_else(|| label.to_string());
        let new_identity = ProviderIdentitySnapshot {
            provider_id: Some(provider_id.clone()),
            account_email: Some(resolved_email),
            account_organization: existing.and_then(|id| id.account_organization.clone()),
            login_method: existing.and_then(|id| id.login_method.clone()),
            account_id: existing.and_then(|id| id.account_id.clone()),
        };
        self.clone().with_identity(Some(new_identity))
    }

    pub fn scoped_to(&self, provider_id: ProviderId) -> Self {
        match &self.identity {
            Some(id) if id.provider_id != Some(provider_id.clone()) => {
                let scoped = id.scoped_to(provider_id);
                self.clone().with_identity(Some(scoped))
            }
            _ => self.clone(),
        }
    }

    pub fn with_extra_rate_windows(mut self, extra: Option<Vec<NamedRateWindow>>) -> Self {
        self.extra_rate_windows = extra;
        self
    }

    pub fn with_details(mut self, details: Vec<DetailSection>) -> Result<Self, ModelError> {
        DetailSection::validate_sections(&details)
            .map_err(|_| ModelError::TooManySections(details.len()))?;
        self.details = details;
        Ok(self)
    }

    pub fn with_subscription_metadata(
        mut self,
        expires_at: Option<jiff::Timestamp>,
        renews_at: Option<jiff::Timestamp>,
    ) -> Self {
        self.subscription_expires_at = expires_at;
        self.subscription_renews_at = renews_at;
        self
    }

    pub fn with_rate_windows(
        mut self,
        primary: Option<RateWindow>,
        secondary: Option<RateWindow>,
    ) -> Self {
        self.primary = primary;
        self.secondary = secondary;
        self
    }

    pub fn with_tertiary(mut self, tertiary: Option<RateWindow>) -> Self {
        self.tertiary = tertiary;
        self
    }

    pub fn with_provider_cost(mut self, provider_cost: Option<ProviderCostSnapshot>) -> Self {
        self.provider_cost = provider_cost;
        self
    }

    pub fn with_identity(mut self, identity: Option<ProviderIdentitySnapshot>) -> Self {
        self.identity = identity;
        self
    }

    pub fn with_data_confidence(mut self, confidence: Confidence) -> Self {
        self.data_confidence = confidence;
        self
    }

    pub fn backfilling_reset_times(
        &self,
        cached: Option<&UsageSnapshot>,
        now: jiff::Timestamp,
    ) -> UsageSnapshot {
        crate::model::backfill::backfilling_reset_times(self, cached, now)
    }

    pub fn detail_row(&self, label: &str) -> Option<&DetailRow> {
        self.details
            .iter()
            .flat_map(|s| &s.rows)
            .find(|r| r.label == label)
    }

    pub fn detail_row_by_id(&self, id: &str) -> Option<&DetailRow> {
        self.details
            .iter()
            .flat_map(|s| &s.rows)
            .find(|r| r.id.as_deref() == Some(id))
    }
}

fn is_unknown_confidence(c: &Confidence) -> bool {
    matches!(c, Confidence::Unknown)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageSnapshotSerialize<'a> {
    primary: &'a Option<RateWindow>,
    secondary: &'a Option<RateWindow>,
    tertiary: &'a Option<RateWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra_rate_windows: &'a Option<Vec<NamedRateWindow>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_cost: &'a Option<ProviderCostSnapshot>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    details: &'a Vec<DetailSection>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    subscription_expires_at: &'a Option<jiff::Timestamp>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    subscription_renews_at: &'a Option<jiff::Timestamp>,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    updated_at: &'a jiff::Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    identity: &'a Option<ProviderIdentitySnapshot>,
    #[serde(skip_serializing_if = "is_unknown_confidence")]
    data_confidence: Confidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_email: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_organization: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    login_method: Option<&'a str>,
}

impl Serialize for UsageSnapshot {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let s = UsageSnapshotSerialize {
            primary: &self.primary,
            secondary: &self.secondary,
            tertiary: &self.tertiary,
            extra_rate_windows: &self.extra_rate_windows,
            provider_cost: &self.provider_cost,
            details: &self.details,
            subscription_expires_at: &self.subscription_expires_at,
            subscription_renews_at: &self.subscription_renews_at,
            updated_at: &self.updated_at,
            identity: &self.identity,
            data_confidence: self.data_confidence,
            account_email: self
                .identity
                .as_ref()
                .and_then(|i| i.account_email.as_deref()),
            account_organization: self
                .identity
                .as_ref()
                .and_then(|i| i.account_organization.as_deref()),
            login_method: self
                .identity
                .as_ref()
                .and_then(|i| i.login_method.as_deref()),
        };
        s.serialize(serializer)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageSnapshotRaw {
    #[serde(default)]
    primary: Option<RateWindow>,
    #[serde(default)]
    secondary: Option<RateWindow>,
    #[serde(default)]
    tertiary: Option<RateWindow>,
    #[serde(default)]
    extra_rate_windows: Option<Vec<NamedRateWindow>>,
    #[serde(default)]
    provider_cost: Option<ProviderCostSnapshot>,
    #[serde(default)]
    details: Vec<DetailSection>,
    #[serde(default, with = "ariadusage_protocol::time::rfc3339_opt")]
    subscription_expires_at: Option<jiff::Timestamp>,
    #[serde(default, with = "ariadusage_protocol::time::rfc3339_opt")]
    subscription_renews_at: Option<jiff::Timestamp>,
    #[serde(with = "ariadusage_protocol::time::rfc3339")]
    updated_at: jiff::Timestamp,
    #[serde(default)]
    identity: Option<ProviderIdentitySnapshot>,
    #[serde(default)]
    data_confidence: Option<Confidence>,
    #[serde(default)]
    account_email: Option<String>,
    #[serde(default)]
    account_organization: Option<String>,
    #[serde(default)]
    login_method: Option<String>,
}

impl<'de> Deserialize<'de> for UsageSnapshot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = UsageSnapshotRaw::deserialize(deserializer)?;
        DetailSection::validate_sections(&raw.details).map_err(serde::de::Error::custom)?;

        let identity = if let Some(id) = raw.identity {
            Some(id)
        } else if raw.account_email.is_some()
            || raw.account_organization.is_some()
            || raw.login_method.is_some()
        {
            Some(ProviderIdentitySnapshot {
                provider_id: None,
                account_email: raw.account_email,
                account_organization: raw.account_organization,
                login_method: raw.login_method,
                account_id: None,
            })
        } else {
            None
        };

        Ok(Self {
            primary: raw.primary,
            secondary: raw.secondary,
            tertiary: raw.tertiary,
            extra_rate_windows: raw.extra_rate_windows,
            provider_cost: raw.provider_cost,
            details: raw.details,
            subscription_expires_at: raw.subscription_expires_at,
            subscription_renews_at: raw.subscription_renews_at,
            updated_at: raw.updated_at,
            identity,
            data_confidence: raw.data_confidence.unwrap_or(Confidence::Unknown),
        })
    }
}
