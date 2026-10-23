// Ported from CodexBar Sources/CodexBarCore/ProviderIdentitySnapshot.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::fmt;

use ariadusage_protocol::ProviderId;
use serde::{Deserialize, Serialize};

/// Snapshot of provider-associated account identity.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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

impl fmt::Debug for ProviderIdentitySnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderIdentitySnapshot")
            .field("provider_id", &self.provider_id)
            .field("has_account_email", &self.account_email.is_some())
            .field(
                "has_account_organization",
                &self.account_organization.is_some(),
            )
            .field("login_method", &self.login_method)
            .field("has_account_id", &self.account_id.is_some())
            .finish()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_email_organization_and_account_id() {
        let snap = ProviderIdentitySnapshot::new(
            Some(ProviderId::new("claude").unwrap()),
            Some("sensitive-user@example.invalid".to_owned()),
            Some("Sensitive Org".to_owned()),
            Some("Claude Max 5x".to_owned()),
            Some("claude-owner-v1:123456".to_owned()),
        );
        let debug = format!("{snap:?}");
        assert!(debug.contains("Claude Max 5x"));
        assert!(debug.contains("has_account_email: true"));
        assert!(debug.contains("has_account_organization: true"));
        assert!(debug.contains("has_account_id: true"));
        assert!(!debug.contains("sensitive-user@example.invalid"));
        assert!(!debug.contains("Sensitive Org"));
        assert!(!debug.contains("claude-owner-v1:123456"));
    }
}
