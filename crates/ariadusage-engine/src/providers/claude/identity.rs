// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeVerifiedAccountOwner.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::fmt::{self, Write};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Verified account identity decoded from `/api/oauth/profile`.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct ClaudeAccountIdentity {
    pub account_uuid: Option<String>,
    pub email: Option<String>,
    pub organization_uuid: Option<String>,
    pub organization_name: Option<String>,
}

impl fmt::Debug for ClaudeAccountIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaudeAccountIdentity")
            .field("has_account_uuid", &self.account_uuid.is_some())
            .field("has_email", &self.email.is_some())
            .field("has_organization_uuid", &self.organization_uuid.is_some())
            .field("has_organization_name", &self.organization_name.is_some())
            .finish()
    }
}

impl ClaudeAccountIdentity {
    pub fn verified_owner_id(&self) -> Option<String> {
        ClaudeVerifiedAccountOwner::owner_id(
            self.account_uuid.as_deref(),
            self.email.as_deref(),
            self.organization_uuid.as_deref(),
        )
    }

    /// Decodes `/api/oauth/profile` JSON response bytes.
    pub fn decode_profile(data: &[u8]) -> Option<Self> {
        let raw: OAuthProfileResponseRaw = serde_json::from_slice(data).ok()?;
        let account_uuid = clean_token(
            raw.account
                .as_ref()
                .and_then(|a| a.uuid.as_deref().or(a.account_uuid.as_deref()))
                .or(raw.account_uuid.as_deref())
                .or(raw.uuid.as_deref()),
        );
        let email = clean_token(
            raw.account
                .as_ref()
                .and_then(|a| a.email_address.as_deref().or(a.email.as_deref()))
                .or(raw.email_address.as_deref())
                .or(raw.email.as_deref()),
        );
        let organization_uuid = clean_token(
            raw.organization
                .as_ref()
                .and_then(|o| o.uuid.as_deref().or(o.organization_uuid.as_deref()))
                .or(raw.organization_uuid.as_deref()),
        );
        let organization_name = clean_token(
            raw.organization
                .as_ref()
                .and_then(|o| o.name.as_deref().or(o.organization_name.as_deref()))
                .or(raw.organization_name.as_deref()),
        );

        if account_uuid.is_none()
            && email.is_none()
            && organization_uuid.is_none()
            && organization_name.is_none()
        {
            return None;
        }

        Some(Self {
            account_uuid,
            email,
            organization_uuid,
            organization_name,
        })
    }
}

/// Computes the one-way verified account owner identifier (`claude-owner-v1:<sha256>`).
pub struct ClaudeVerifiedAccountOwner;

impl ClaudeVerifiedAccountOwner {
    pub fn owner_id(
        account_uuid: Option<&str>,
        email: Option<&str>,
        organization_uuid: Option<&str>,
    ) -> Option<String> {
        let org = Self::normalized_token(organization_uuid)?;
        let (principal_kind, principal_value) =
            if let Some(uuid) = Self::normalized_token(account_uuid) {
                ("uuid", uuid)
            } else {
                let addr = Self::normalized_token(email)?;
                ("email", addr)
            };
        let parts = [principal_kind, &principal_value, &org];
        let payload = serde_json::to_vec(&parts).ok()?;

        let mut hasher = Sha256::new();
        hasher.update(&payload);
        let digest = hasher.finalize();
        let mut hex = String::with_capacity(64);
        for byte in digest {
            let _ = write!(hex, "{byte:02x}");
        }
        Some(format!("claude-owner-v1:{hex}"))
    }

    fn normalized_token(raw: Option<&str>) -> Option<String> {
        let trimmed = raw?.trim().to_lowercase();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }
}

fn clean_token(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

#[derive(Deserialize)]
struct OAuthProfileResponseRaw {
    account: Option<ProfileAccountRaw>,
    organization: Option<ProfileOrgRaw>,
    #[serde(rename = "accountUuid", alias = "account_uuid")]
    account_uuid: Option<String>,
    uuid: Option<String>,
    #[serde(rename = "emailAddress", alias = "email_address")]
    email_address: Option<String>,
    email: Option<String>,
    #[serde(rename = "organizationUuid", alias = "organization_uuid")]
    organization_uuid: Option<String>,
    #[serde(rename = "organizationName", alias = "organization_name")]
    organization_name: Option<String>,
}

#[derive(Deserialize)]
struct ProfileAccountRaw {
    uuid: Option<String>,
    #[serde(rename = "accountUuid", alias = "account_uuid")]
    account_uuid: Option<String>,
    #[serde(rename = "emailAddress", alias = "email_address")]
    email_address: Option<String>,
    email: Option<String>,
}

#[derive(Deserialize)]
struct ProfileOrgRaw {
    uuid: Option<String>,
    #[serde(rename = "organizationUuid", alias = "organization_uuid")]
    organization_uuid: Option<String>,
    name: Option<String>,
    #[serde(rename = "organizationName", alias = "organization_name")]
    organization_name: Option<String>,
}
