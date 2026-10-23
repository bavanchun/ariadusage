// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexIdentity.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/UsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::fmt;

use super::credentials::{CodexOAuthCredentials, parse_jwt_payload};

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum CodexIdentity {
    ProviderAccount { id: String },
    EmailOnly { normalized_email: String },
    Unresolved,
}

impl fmt::Debug for CodexIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderAccount { .. } => f
                .debug_struct("ProviderAccount")
                .field("id", &"[redacted]")
                .finish(),
            Self::EmailOnly { .. } => f
                .debug_struct("EmailOnly")
                .field("normalized_email", &"[redacted]")
                .finish(),
            Self::Unresolved => f.write_str("Unresolved"),
        }
    }
}

pub struct CodexIdentityResolver;

impl CodexIdentityResolver {
    pub fn resolve(account_id: Option<&str>, email: Option<&str>) -> CodexIdentity {
        if let Some(id) = Self::normalize_account_id(account_id) {
            return CodexIdentity::ProviderAccount { id };
        }
        if let Some(normalized_email) = Self::normalize_email(email) {
            return CodexIdentity::EmailOnly { normalized_email };
        }
        CodexIdentity::Unresolved
    }

    pub fn normalize_account_id(value: Option<&str>) -> Option<String> {
        let trimmed = value?.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(trimmed.to_owned())
    }

    pub fn normalize_email(value: Option<&str>) -> Option<String> {
        let trimmed = value?.trim().to_lowercase();
        if trimmed.is_empty() {
            return None;
        }
        Some(trimmed)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct CodexNativeCredentialOwnerIdentity {
    pub account_id: Option<String>,
    pub identity: CodexIdentity,
    pub email: Option<String>,
}

impl fmt::Debug for CodexNativeCredentialOwnerIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexNativeCredentialOwnerIdentity")
            .field(
                "account_id",
                &self.account_id.as_ref().map(|_| "[redacted]"),
            )
            .field("identity", &self.identity)
            .field("email", &self.email.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

impl CodexNativeCredentialOwnerIdentity {
    pub fn new(account_id: Option<&str>, email: Option<&str>) -> Self {
        let normalized_acct = CodexIdentityResolver::normalize_account_id(account_id);
        let normalized_email = CodexIdentityResolver::normalize_email(email);
        let identity =
            CodexIdentityResolver::resolve(normalized_acct.as_deref(), normalized_email.as_deref());
        Self {
            account_id: normalized_acct,
            identity,
            email: normalized_email,
        }
    }

    pub fn matches(&self, other: &Self) -> bool {
        if let (Some(a), Some(b)) = (&self.account_id, &other.account_id) {
            return a == b;
        }
        if self.account_id.is_some() || other.account_id.is_some() {
            return false;
        }
        if let (Some(a), Some(b)) = (&self.email, &other.email) {
            return a == b;
        }
        false
    }

    pub fn from_credentials(credentials: Option<&CodexOAuthCredentials>) -> Self {
        let Some(credentials) = credentials else {
            return Self::new(None, None);
        };
        let email = Self::normalized_email_from_id_token(credentials.id_token_str());
        Self::new(credentials.account_id.as_deref(), email.as_deref())
    }

    pub fn normalized_email_from_id_token(id_token: Option<&str>) -> Option<String> {
        let obj = parse_jwt_payload(id_token?)?;

        let top_level_email = match obj.get("email") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(raw)) => {
                CodexIdentityResolver::normalize_email(Some(raw))
            }
            Some(_) => return None,
        };

        let profile_email = match obj.get("https://api.openai.com/profile") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::Object(profile)) => match profile.get("email") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(raw)) => {
                    CodexIdentityResolver::normalize_email(Some(raw))
                }
                Some(_) => return None,
            },
            Some(_) => return None,
        };

        if let (Some(top), Some(profile)) = (&top_level_email, &profile_email) {
            return (top == profile).then(|| top.clone());
        }
        top_level_email.or(profile_email)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct CodexAuthBackedAccount {
    pub identity: CodexIdentity,
    pub email: Option<String>,
    pub plan: Option<String>,
}

impl fmt::Debug for CodexAuthBackedAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexAuthBackedAccount")
            .field("identity", &self.identity)
            .field("email", &self.email.as_ref().map(|_| "[redacted]"))
            .field("plan", &self.plan)
            .finish()
    }
}

impl CodexAuthBackedAccount {
    pub fn new(identity: CodexIdentity, email: Option<String>, plan: Option<String>) -> Self {
        Self {
            identity,
            email: email.map(|e| e.trim().to_owned()).filter(|e| !e.is_empty()),
            plan: plan.map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.identity == CodexIdentity::Unresolved && self.email.is_none() && self.plan.is_none()
    }

    pub fn from_credentials(credentials: Option<&CodexOAuthCredentials>) -> Self {
        let Some(credentials) = credentials else {
            return Self::new(CodexIdentity::Unresolved, None, None);
        };
        let email = CodexNativeCredentialOwnerIdentity::normalized_email_from_id_token(
            credentials.id_token_str(),
        );
        let payload = credentials.id_token_str().and_then(parse_jwt_payload);
        let auth_dict = payload
            .as_ref()
            .and_then(|p| p.get("https://api.openai.com/auth"))
            .and_then(serde_json::Value::as_object);
        let jwt_account_id = auth_dict
            .and_then(|d| d.get("chatgpt_account_id"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                payload
                    .as_ref()
                    .and_then(|p| p.get("chatgpt_account_id"))
                    .and_then(serde_json::Value::as_str)
            });
        let account_id = credentials.account_id.as_deref().or(jwt_account_id);
        let plan = auth_dict
            .and_then(|d| d.get("chatgpt_plan_type"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                payload
                    .as_ref()
                    .and_then(|p| p.get("chatgpt_plan_type"))
                    .and_then(serde_json::Value::as_str)
            })
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let identity = CodexIdentityResolver::resolve(account_id, email.as_deref());
        Self::new(identity, email, plan)
    }
}
