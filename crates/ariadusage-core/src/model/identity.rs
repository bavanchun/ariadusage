// Ported from CodexBar Sources/CodexBarCore/ProviderIdentitySnapshot.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_protocol::ProviderId;
use serde::{Deserialize, Serialize};

/// Snapshot of provider-associated account identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderIdentitySnapshot {
    #[serde(rename = "providerID", skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<ProviderId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_organization: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_method: Option<String>,
    #[serde(rename = "accountID", skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

impl ProviderIdentitySnapshot {
    pub fn new(
        provider_id: Option<ProviderId>,
        account_email: Option<String>,
        account_organization: Option<String>,
        login_method: Option<String>,
        account_id: Option<String>,
    ) -> Self {
        Self {
            provider_id,
            account_email,
            account_organization,
            login_method,
            account_id,
        }
    }

    pub fn scoped_to(&self, provider_id: ProviderId) -> Self {
        if self.provider_id == Some(provider_id.clone()) {
            self.clone()
        } else {
            Self {
                provider_id: Some(provider_id),
                account_email: self.account_email.clone(),
                account_organization: self.account_organization.clone(),
                login_method: self.login_method.clone(),
                account_id: self.account_id.clone(),
            }
        }
    }
}
