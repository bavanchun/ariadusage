// Ported from CodexBar Sources/CodexBarCLI/CLIServeAccounts.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
//
// Metadata-only account discovery payloads and `/accounts` + `/accounts/<id>`
// route handlers for `serve`.

use std::path::{Path, PathBuf};

use ariadusage_core::config::{
    Config, ProviderConfig, ProviderEntry, TokenAccountMeta, TokenAccountsMeta,
};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use uuid::Uuid;

use super::auth::serve_unauthorized_response;
use super::dashboard::{DashboardIdentityMode, DashboardSnapshotBuilder};
use super::http_server::{CliLocalHttpRequest, CliLocalHttpResponse};
use super::{
    ServeRuntime, adding_no_store, resolve_dashboard_identity_mode, serve_error, serve_json,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliServeAccountsPayload {
    pub schema_version: i32,
    pub accounts: Vec<CliServeAccountPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliServeAccountPayload {
    pub id: String,
    pub provider: String,
    pub source: String,
    pub label: String,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<CliServeAccountIdentityPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliServeAccountIdentityPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
}

/// Persisted Codex active account source (`CodexActiveSource.swift`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexActiveSource {
    LiveSystem,
    ManagedAccount { id: Uuid },
    ProfileHome { path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawCodexActiveSource {
    kind: String,
    #[serde(rename = "accountID", skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(rename = "homePath", skip_serializing_if = "Option::is_none")]
    home_path: Option<String>,
}

impl CodexActiveSource {
    #[must_use]
    pub fn from_provider_config(config: &ProviderConfig) -> Self {
        let Some(raw) = config.extra.get("codexActiveSource") else {
            return Self::LiveSystem;
        };
        let Ok(parsed) = serde_json::from_str::<RawCodexActiveSource>(raw.get()) else {
            return Self::LiveSystem;
        };
        match parsed.kind.as_str() {
            "managedAccount" => parsed
                .account_id
                .and_then(|s| Uuid::parse_str(&s).ok())
                .map_or(Self::LiveSystem, |id| Self::ManagedAccount { id }),
            "profileHome" => {
                let path = parsed.home_path.unwrap_or_default();
                if path.trim().is_empty() {
                    Self::LiveSystem
                } else {
                    Self::ProfileHome { path }
                }
            }
            "liveSystem" => {
                if let Some(path) = parsed.home_path
                    && !path.trim().is_empty()
                {
                    return Self::ProfileHome { path };
                }
                Self::LiveSystem
            }
            _ => Self::LiveSystem,
        }
    }

    pub fn apply_to_provider_config(&self, config: &mut ProviderConfig) {
        let raw_obj = match self {
            Self::LiveSystem => RawCodexActiveSource {
                kind: "liveSystem".to_string(),
                account_id: None,
                home_path: None,
            },
            Self::ManagedAccount { id } => RawCodexActiveSource {
                kind: "managedAccount".to_string(),
                account_id: Some(id.hyphenated().to_string().to_ascii_uppercase()),
                home_path: None,
            },
            Self::ProfileHome { path } => RawCodexActiveSource {
                kind: "profileHome".to_string(),
                account_id: None,
                home_path: Some(path.clone()),
            },
        };
        if let Ok(json_str) = serde_json::to_string(&raw_obj)
            && let Ok(raw_val) = RawValue::from_string(json_str)
        {
            config
                .extra
                .insert("codexActiveSource".to_string(), raw_val);
        }
    }
}

#[must_use]
pub fn codex_active_source_from_config(config: &Config) -> CodexActiveSource {
    config.provider_config("codex").map_or(
        CodexActiveSource::LiveSystem,
        CodexActiveSource::from_provider_config,
    )
}

/// Metadata for a CodexBar/AriadUsage managed Codex account (`ManagedCodexAccount`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedCodexAccount {
    #[serde(with = "serde_uuid")]
    pub id: Uuid,
    pub email: String,
    #[serde(
        rename = "providerAccountID",
        alias = "providerAccountId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub provider_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_label: Option<String>,
    #[serde(
        rename = "workspaceAccountID",
        alias = "workspaceAccountId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub workspace_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_fingerprint: Option<String>,
    pub managed_home_path: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub last_authenticated_at: i64,
}

impl ManagedCodexAccount {
    #[must_use]
    pub fn normalize_email(email: &str) -> String {
        email.trim().to_ascii_lowercase()
    }

    #[must_use]
    pub fn normalize_workspace_account_id(workspace_account_id: Option<&str>) -> Option<String> {
        let trimmed = workspace_account_id?.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_ascii_lowercase())
        }
    }

    #[must_use]
    pub fn effective_workspace_account_id(&self) -> Option<String> {
        Self::normalize_workspace_account_id(
            self.workspace_account_id
                .as_deref()
                .or(self.provider_account_id.as_deref()),
        )
    }
}

mod serde_uuid {
    use serde::{Deserialize, Deserializer, Serializer};
    use uuid::Uuid;

    pub fn serialize<S>(id: &Uuid, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&id.hyphenated().to_string().to_ascii_uppercase())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Uuid, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Uuid::parse_str(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedCodexAccountSet {
    #[serde(default = "default_managed_accounts_version")]
    pub version: i32,
    #[serde(default)]
    pub accounts: Vec<ManagedCodexAccount>,
}

const fn default_managed_accounts_version() -> i32 {
    3
}

/// File-backed metadata store for managed Codex accounts.
#[derive(Debug, Clone)]
pub struct FileManagedCodexAccountStore {
    file_path: Option<PathBuf>,
}

impl FileManagedCodexAccountStore {
    pub const CURRENT_VERSION: i32 = 3;

    #[must_use]
    pub fn new(file_path: impl Into<PathBuf>) -> Self {
        Self {
            file_path: Some(file_path.into()),
        }
    }

    #[must_use]
    pub fn with_optional_path(file_path: Option<PathBuf>) -> Self {
        Self { file_path }
    }

    #[must_use]
    pub fn default_path() -> Option<PathBuf> {
        crate::paths::state_dir()
            .ok()
            .map(|p| p.join("managed-codex-accounts.json"))
    }

    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.file_path.as_deref()
    }

    pub fn load_account_metadata(&self) -> Result<ManagedCodexAccountSet, std::io::Error> {
        let Some(file_path) = self.file_path.as_deref() else {
            return Err(std::io::Error::other(
                "managed Codex account state directory unavailable",
            ));
        };
        match std::fs::read(file_path) {
            Ok(bytes) => {
                let accounts: ManagedCodexAccountSet = serde_json::from_slice(&bytes)
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
                if !(1..=Self::CURRENT_VERSION).contains(&accounts.version) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unsupported version: {}", accounts.version),
                    ));
                }
                Ok(accounts)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(ManagedCodexAccountSet {
                version: Self::CURRENT_VERSION,
                accounts: Vec::new(),
            }),
            Err(err) => Err(err),
        }
    }

    pub fn store_accounts(&self, accounts: &ManagedCodexAccountSet) -> Result<(), std::io::Error> {
        let Some(file_path) = self.file_path.as_deref() else {
            return Err(std::io::Error::other(
                "managed Codex account state directory unavailable",
            ));
        };
        let bytes = serde_json::to_vec_pretty(accounts)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        crate::private_file::write_private(
            file_path,
            &bytes,
            &crate::private_file::WriteHooks::default(),
        )
        .map_err(|err| std::io::Error::other(err.to_string()))
    }
}

/// Builds `/accounts` and `/accounts/<id>` discovery payloads without touching credentials.
pub struct CliServeAccountDiscovery;

impl CliServeAccountDiscovery {
    pub const SCHEMA_VERSION: i32 = 1;
    pub const MANAGED_CODEX_SOURCE: &'static str = "codex-managed";
    pub const TOKEN_ACCOUNT_SOURCE: &'static str = "token-account";
    pub const MANAGED_CODEX_PROVIDER: &'static str = "codex";

    #[must_use]
    pub fn make_payload(
        config: &Config,
        managed_codex_accounts: &[ManagedCodexAccount],
        identity_mode: DashboardIdentityMode,
    ) -> CliServeAccountsPayload {
        let mut accounts = Self::token_accounts(config, identity_mode);
        accounts.extend(Self::managed_codex_accounts(
            config,
            managed_codex_accounts,
            identity_mode,
        ));
        accounts.sort_by(|a, b| {
            a.provider
                .cmp(&b.provider)
                .then_with(|| a.source.cmp(&b.source))
                .then_with(|| a.id.cmp(&b.id))
        });
        CliServeAccountsPayload {
            schema_version: Self::SCHEMA_VERSION,
            accounts,
        }
    }

    #[must_use]
    pub fn response(
        payload: &CliServeAccountsPayload,
        requested_id: Option<&str>,
    ) -> CliLocalHttpResponse {
        if let Some(requested_id) = requested_id {
            let Some(account) = payload.accounts.iter().find(|a| a.id == requested_id) else {
                return serve_error(404, "Not Found", "account not found");
            };
            return serve_json(account);
        }
        serve_json(payload)
    }

    fn token_accounts(
        config: &Config,
        identity_mode: DashboardIdentityMode,
    ) -> Vec<CliServeAccountPayload> {
        let mut out = Vec::new();
        for entry in &config.providers {
            let (provider_id, token_accounts_opt) = match entry {
                ProviderEntry::Typed(cfg) => (cfg.id.as_str(), cfg.token_accounts.clone()),
                ProviderEntry::Opaque { id, raw, .. } => {
                    (id.as_str(), parse_opaque_token_accounts(raw.get()))
                }
            };
            let Some(data) = token_accounts_opt else {
                continue;
            };
            let active_index = data.clamped_active_index();
            for (index, account) in data.accounts.iter().enumerate() {
                let opaque_id = account.id.trim().to_ascii_lowercase();
                out.push(CliServeAccountPayload {
                    id: format!("{}:{provider_id}:{opaque_id}", Self::TOKEN_ACCOUNT_SOURCE),
                    provider: provider_id.to_string(),
                    source: Self::TOKEN_ACCOUNT_SOURCE.to_string(),
                    label: Self::presented_label(&account.label, &opaque_id, identity_mode),
                    active: index == active_index,
                    identity: None,
                });
            }
        }
        out
    }

    fn managed_codex_accounts(
        config: &Config,
        accounts: &[ManagedCodexAccount],
        identity_mode: DashboardIdentityMode,
    ) -> Vec<CliServeAccountPayload> {
        let active_source = config.provider_config(Self::MANAGED_CODEX_PROVIDER).map_or(
            CodexActiveSource::LiveSystem,
            CodexActiveSource::from_provider_config,
        );
        accounts
            .iter()
            .map(|account| {
                let opaque_id = account.id.hyphenated().to_string().to_ascii_lowercase();
                let email =
                    DashboardSnapshotBuilder::dashboard_email(Some(&account.email), identity_mode);
                let raw_label = account.workspace_label.as_deref().unwrap_or(&account.email);
                let identity = (identity_mode != DashboardIdentityMode::None).then_some(
                    CliServeAccountIdentityPayload {
                        account_email: email,
                    },
                );
                CliServeAccountPayload {
                    id: format!("{}:{opaque_id}", Self::MANAGED_CODEX_SOURCE),
                    provider: Self::MANAGED_CODEX_PROVIDER.to_string(),
                    source: Self::MANAGED_CODEX_SOURCE.to_string(),
                    label: Self::presented_label(raw_label, &opaque_id, identity_mode),
                    active: active_source == CodexActiveSource::ManagedAccount { id: account.id },
                    identity,
                }
            })
            .collect()
    }

    #[must_use]
    pub fn presented_label(raw: &str, account_id: &str, mode: DashboardIdentityMode) -> String {
        let label = raw.trim();
        if label.is_empty() {
            return format!("Account {account_id}");
        }
        if mode == DashboardIdentityMode::Redacted {
            return format!("Account {account_id}");
        }
        if mode == DashboardIdentityMode::None {
            return if label.contains('@') {
                format!("Account {account_id}")
            } else {
                label.to_string()
            };
        }
        label.to_string()
    }
}

pub fn serve_accounts_route(
    request: &CliLocalHttpRequest,
    id: Option<&str>,
    runtime: &ServeRuntime,
) -> CliLocalHttpResponse {
    if runtime.data_routes_require_auth
        && !runtime
            .dashboard_auth
            .authorizes(request.headers.get("authorization").map(String::as_str))
    {
        return serve_unauthorized_response();
    }
    let config = match runtime.load_config() {
        Ok(cfg) => cfg,
        Err(_) => {
            return adding_no_store(serve_error(
                500,
                "Internal Server Error",
                "could not load accounts",
            ));
        }
    };
    let managed_accounts = match runtime.load_managed_codex_accounts() {
        Ok(accounts) => accounts,
        Err(_) => {
            return adding_no_store(serve_error(
                500,
                "Internal Server Error",
                "could not load accounts",
            ));
        }
    };
    let identity_mode = resolve_dashboard_identity_mode(
        runtime.dashboard_identity_mode,
        runtime.hides_personal_info(&config),
        false,
    );
    let payload = CliServeAccountDiscovery::make_payload(&config, &managed_accounts, identity_mode);
    adding_no_store(CliServeAccountDiscovery::response(&payload, id))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOpaqueProvider {
    #[serde(default)]
    token_accounts: Option<RawTokenAccounts>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTokenAccounts {
    #[serde(default = "default_token_accounts_version")]
    version: u32,
    #[serde(default)]
    active_index: i64,
    #[serde(default)]
    accounts: Vec<RawTokenAccount>,
}

const fn default_token_accounts_version() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTokenAccount {
    id: String,
    label: String,
    #[serde(default)]
    added_at: f64,
    #[serde(default)]
    last_used: Option<f64>,
    #[serde(default)]
    external_identifier: Option<String>,
    #[serde(default)]
    usage_scope: Option<String>,
    #[serde(rename = "organizationID", alias = "organizationId", default)]
    organization_id: Option<String>,
    #[serde(rename = "workspaceID", alias = "workspaceId", default)]
    workspace_id: Option<String>,
    #[serde(default)]
    claude_config_directory: Option<String>,
    #[serde(default)]
    seat_credit_entitlement: Option<String>,
}

pub(crate) fn parse_opaque_token_accounts(raw_json: &str) -> Option<TokenAccountsMeta> {
    let parsed: RawOpaqueProvider = serde_json::from_str(raw_json).ok()?;
    let ta = parsed.token_accounts?;
    Some(TokenAccountsMeta {
        version: ta.version,
        active_index: ta.active_index,
        accounts: ta
            .accounts
            .into_iter()
            .map(|a| TokenAccountMeta {
                id: a.id,
                label: a.label,
                added_at: a.added_at,
                last_used: a.last_used,
                external_identifier: a.external_identifier,
                usage_scope: a.usage_scope,
                organization_id: a.organization_id,
                workspace_id: a.workspace_id,
                claude_config_directory: a.claude_config_directory,
                seat_credit_entitlement: a.seat_credit_entitlement,
                model_rate_limits: None,
                extra: std::collections::BTreeMap::new(),
            })
            .collect(),
    })
}
