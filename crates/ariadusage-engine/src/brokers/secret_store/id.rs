use std::collections::BTreeMap;
use std::fmt;

use ariadusage_protocol::{ProviderId, ids::SettingId};

/// The kind of secret addressed by a setting identifier.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretKind {
    ApiKey,
    CookieHeader,
    AccountToken,
}

impl SecretKind {
    pub const fn attribute_value(self) -> &'static str {
        match self {
            Self::ApiKey => "apiKey",
            Self::CookieHeader => "cookieHeader",
            Self::AccountToken => "token",
        }
    }

    const fn label_value(self) -> &'static str {
        match self {
            Self::ApiKey => "API key",
            Self::CookieHeader => "cookie header",
            Self::AccountToken => "account token",
        }
    }
}

impl fmt::Debug for SecretKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.attribute_value())
    }
}

/// A validated identifier for one secret in AriadUsage's SecretStore.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SecretId {
    setting_id: SettingId,
    provider: ProviderId,
    kind: SecretKind,
    account_id: Option<String>,
}

impl SecretId {
    /// Accepts only known providers and the three supported secret-setting shapes.
    pub fn from_setting_id(setting_id: &SettingId) -> Result<Self, SecretIdError> {
        let parts: Vec<_> = setting_id.as_str().split('.').collect();
        let (provider, kind, account_id) = match parts.as_slice() {
            ["providers", provider, "apiKey"] => (provider, SecretKind::ApiKey, None),
            ["providers", provider, "cookieHeader"] => (provider, SecretKind::CookieHeader, None),
            ["providers", provider, "accounts", account_id, "token"]
                if !account_id.is_empty() && account_id.len() <= 64 =>
            {
                (
                    provider,
                    SecretKind::AccountToken,
                    Some((*account_id).to_owned()),
                )
            }
            _ => return Err(SecretIdError),
        };

        let provider = ProviderId::new(*provider).map_err(|_| SecretIdError)?;
        if ariadusage_core::providers::find_by_id(&provider).is_none() {
            return Err(SecretIdError);
        }

        if account_id.as_deref().is_some_and(|id| {
            !id.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        }) {
            return Err(SecretIdError);
        }

        Ok(Self {
            setting_id: setting_id.clone(),
            provider,
            kind,
            account_id,
        })
    }

    pub fn provider(&self) -> &ProviderId {
        &self.provider
    }

    pub const fn kind(&self) -> SecretKind {
        self.kind
    }

    pub fn account_id(&self) -> Option<&str> {
        self.account_id.as_deref()
    }

    pub fn as_setting_id(&self) -> &SettingId {
        &self.setting_id
    }

    /// Stable, non-PII attributes used to find this item in Secret Service.
    pub fn attributes(&self) -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            ("application", "ariadusage".to_owned()),
            ("ariadusage:kind", self.kind.attribute_value().to_owned()),
            ("ariadusage:provider", self.provider.to_string()),
            (
                "ariadusage:account",
                self.account_id.clone().unwrap_or_default(),
            ),
            (
                "xdg:schema",
                "io.github.bavanchun.ariadusage.Secret".to_owned(),
            ),
        ])
    }

    /// A stable display label that never includes an account label or secret value.
    pub fn label(&self) -> String {
        let name = ariadusage_core::providers::find_by_id(&self.provider)
            .map(|descriptor| descriptor.display_name)
            .unwrap_or("Provider");
        format!("AriadUsage {name} {}", self.kind.label_value())
    }
}

impl fmt::Debug for SecretId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretId")
            .field("provider", &self.provider)
            .field("kind", &self.kind)
            .field("account_id_len", &self.account_id.as_ref().map(String::len))
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid secret setting identifier")]
pub struct SecretIdError;
