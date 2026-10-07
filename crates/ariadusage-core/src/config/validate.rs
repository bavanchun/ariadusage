// Ported from CodexBar Sources/CodexBarCore/Config/CodexBarConfigValidation.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashSet;

use ariadusage_protocol::ProviderId;
use serde::{Deserialize, Serialize};

use super::types::{Config, CookieSource, ProviderEntry};
use crate::providers::{self, SourceMode};

/// Severity level of a configuration validation issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueSeverity {
    Warning,
    Error,
}

/// A validation issue emitted for a configuration document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub severity: IssueSeverity,
    pub provider: Option<ProviderId>,
    pub field: Option<String>,
    pub code: String,
    pub message: String,
}

impl Issue {
    pub fn new(
        severity: IssueSeverity,
        provider: Option<ProviderId>,
        field: Option<impl Into<String>>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            provider,
            field: field.map(Into::into),
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Injected secret presence information indicating which credentials are
/// configured in secure storage (SecretStore), keeping core pure of I/O.
#[derive(Debug, Clone, Default)]
pub struct SecretPresence {
    pub api_keys: HashSet<ProviderId>,
    pub cookie_headers: HashSet<ProviderId>,
    pub token_account_tokens: HashSet<(ProviderId, String)>,
}

impl SecretPresence {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn with_api_key(mut self, provider: ProviderId) -> Self {
        self.api_keys.insert(provider);
        self
    }

    pub fn with_cookie_header(mut self, provider: ProviderId) -> Self {
        self.cookie_headers.insert(provider);
        self
    }

    pub fn with_account_token(
        mut self,
        provider: ProviderId,
        account_id: impl Into<String>,
    ) -> Self {
        self.token_account_tokens
            .insert((provider, account_id.into()));
        self
    }

    pub fn has_api_key(&self, provider: &ProviderId) -> bool {
        self.api_keys.contains(provider)
    }

    pub fn has_cookie_header(&self, provider: &ProviderId) -> bool {
        self.cookie_headers.contains(provider)
    }

    pub fn has_token_for_account(&self, provider: &ProviderId, account_id: &str) -> bool {
        self.token_account_tokens
            .iter()
            .any(|(p, id)| p == provider && id == account_id)
    }
}

/// Validates a configuration document against provider descriptors and secret presence.
pub fn validate(config: &Config, secrets: &SecretPresence) -> Vec<Issue> {
    let mut issues = Vec::new();

    if config.version != Config::CURRENT_VERSION {
        issues.push(Issue::new(
            IssueSeverity::Error,
            None,
            Some("version"),
            "version_mismatch",
            format!("Unsupported config version {}.", config.version),
        ));
    }

    for entry in &config.providers {
        let ProviderEntry::Typed(cfg) = entry else {
            // Opaque entries are not validated
            continue;
        };

        let Some(descriptor) = providers::find_by_id(&cfg.id) else {
            continue;
        };

        let supported_sources = descriptor.source_modes;
        let supports_web = supported_sources.contains(&SourceMode::Auto)
            || supported_sources.contains(&SourceMode::Web);
        let supports_api = supported_sources.contains(&SourceMode::Api);

        if let Some(source) = cfg.source
            && !supported_sources.contains(&source)
        {
            issues.push(Issue::new(
                IssueSeverity::Error,
                Some(cfg.id.clone()),
                Some("source"),
                "unsupported_source",
                format!(
                    "Source {:?} is not supported for {}.",
                    source,
                    cfg.id.as_str()
                ),
            ));
        }

        let has_api_key_in_extra = cfg
            .extra
            .get("apiKey")
            .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);

        if has_api_key_in_extra && !supports_api {
            issues.push(Issue::new(
                IssueSeverity::Warning,
                Some(cfg.id.clone()),
                Some("apiKey"),
                "api_key_unused",
                format!(
                    "apiKey is set but {} does not support api source.",
                    cfg.id.as_str()
                ),
            ));
        }

        if cfg.source == Some(SourceMode::Api) && !supports_api {
            issues.push(Issue::new(
                IssueSeverity::Error,
                Some(cfg.id.clone()),
                Some("source"),
                "api_source_unsupported",
                format!("Source api is not supported for {}.", cfg.id.as_str()),
            ));
        }

        if cfg.source == Some(SourceMode::Api) && descriptor.requires_api_key_for_api_source {
            let has_credential = secrets.has_api_key(&cfg.id)
                || cfg.token_accounts.as_ref().is_some_and(|ta| {
                    ta.accounts
                        .iter()
                        .any(|acc| secrets.has_token_for_account(&cfg.id, &acc.id))
                });
            if !has_credential {
                issues.push(Issue::new(
                    IssueSeverity::Warning,
                    Some(cfg.id.clone()),
                    Some("apiKey"),
                    "api_key_missing",
                    format!(
                        "Source api is selected but apiKey is missing for {}.",
                        cfg.id.as_str()
                    ),
                ));
            }
        }

        if cfg.cookie_source.is_some() && !supports_web {
            issues.push(Issue::new(
                IssueSeverity::Warning,
                Some(cfg.id.clone()),
                Some("cookieSource"),
                "cookie_source_unused",
                format!(
                    "cookieSource is set but {} does not use web cookies.",
                    cfg.id.as_str()
                ),
            ));
        }

        let has_cookie_header_in_extra = cfg
            .extra
            .get("cookieHeader")
            .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);

        if has_cookie_header_in_extra && !supports_web {
            issues.push(Issue::new(
                IssueSeverity::Warning,
                Some(cfg.id.clone()),
                Some("cookieHeader"),
                "cookie_header_unused",
                format!(
                    "cookieHeader is set but {} does not use web cookies.",
                    cfg.id.as_str()
                ),
            ));
        }

        if cfg.cookie_source == Some(CookieSource::Manual) && !secrets.has_cookie_header(&cfg.id) {
            issues.push(Issue::new(
                IssueSeverity::Warning,
                Some(cfg.id.clone()),
                Some("cookieHeader"),
                "cookie_header_missing",
                format!(
                    "cookieSource manual is set but cookieHeader is missing for {}.",
                    cfg.id.as_str()
                ),
            ));
        }

        let has_secret_key_in_extra = cfg
            .extra
            .get("secretKey")
            .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);

        if has_secret_key_in_extra && !descriptor.uses_secret_key {
            issues.push(Issue::new(
                IssueSeverity::Warning,
                Some(cfg.id.clone()),
                Some("secretKey"),
                "secret_key_unused",
                "secretKey is set but only bedrock and doubao use secretKey.",
            ));
        }

        if cfg.sanitized_region().is_some() && !descriptor.uses_region {
            issues.push(Issue::new(
                IssueSeverity::Warning,
                Some(cfg.id.clone()),
                Some("region"),
                "region_unused",
                format!(
                    "region is set but {} does not use regions.",
                    cfg.id.as_str()
                ),
            ));
        }

        if cfg.sanitized_workspace_id().is_some() && !descriptor.supports_workspace {
            issues.push(Issue::new(
                IssueSeverity::Warning,
                Some(cfg.id.clone()),
                Some("workspaceID"),
                "workspace_unused",
                format!(
                    "workspaceID is set but {} does not support workspaceID.",
                    cfg.id.as_str()
                ),
            ));
        }

        if cfg.sanitized_enterprise_host().is_some() && !descriptor.supports_enterprise_host {
            issues.push(Issue::new(
                IssueSeverity::Warning,
                Some(cfg.id.clone()),
                Some("enterpriseHost"),
                "enterprise_host_unused",
                format!(
                    "enterpriseHost is set but {} does not support enterpriseHost.",
                    cfg.id.as_str()
                ),
            ));
        }

        if cfg
            .token_accounts
            .as_ref()
            .is_some_and(|ta| !ta.accounts.is_empty())
            && !descriptor.token_account_support
        {
            issues.push(Issue::new(
                IssueSeverity::Warning,
                Some(cfg.id.clone()),
                Some("tokenAccounts"),
                "token_accounts_unused",
                format!(
                    "tokenAccounts are set but {} does not support token accounts.",
                    cfg.id.as_str()
                ),
            ));
        }

        // AriadUsage secret_in_config warning
        if cfg.extra.contains_key("apiKey") {
            issues.push(Issue::new(
                IssueSeverity::Warning,
                Some(cfg.id.clone()),
                Some("apiKey"),
                "secret_in_config",
                "Secret found in config file; secrets should be stored using 'ariadusage secret set'.",
            ));
        }
        if cfg.extra.contains_key("cookieHeader") {
            issues.push(Issue::new(
                IssueSeverity::Warning,
                Some(cfg.id.clone()),
                Some("cookieHeader"),
                "secret_in_config",
                "Secret found in config file; secrets should be stored using 'ariadusage secret set'.",
            ));
        }
        if cfg.extra.contains_key("secretKey") {
            issues.push(Issue::new(
                IssueSeverity::Warning,
                Some(cfg.id.clone()),
                Some("secretKey"),
                "secret_in_config",
                "Secret found in config file; secrets should be stored using 'ariadusage secret set'.",
            ));
        }
        if cfg.extra.contains_key("pluginSecrets") {
            issues.push(Issue::new(
                IssueSeverity::Warning,
                Some(cfg.id.clone()),
                Some("pluginSecrets"),
                "secret_in_config",
                "Secret found in config file; secrets should be stored using 'ariadusage secret set'.",
            ));
        }
        if cfg
            .token_accounts
            .as_ref()
            .is_some_and(|ta| ta.accounts.iter().any(|a| a.extra.contains_key("token")))
        {
            issues.push(Issue::new(
                IssueSeverity::Warning,
                Some(cfg.id.clone()),
                Some("tokenAccounts"),
                "secret_in_config",
                "Secret found in config file; secrets should be stored using 'ariadusage secret set'.",
            ));
        }
    }

    issues
}
