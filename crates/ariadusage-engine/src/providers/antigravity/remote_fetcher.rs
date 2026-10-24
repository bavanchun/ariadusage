// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityRemoteUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::pipeline::{BoxFuture, ClassifiedError, FetchInteraction, TransportClass};
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::ProviderErrorKind;
use jiff::Timestamp;
use reqwest::Method;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::brokers::call::BrokerCall;
use crate::brokers::net::{DeclaredOrigins, NetBody, NetBroker, NetError, NetRequest};

use super::oauth_store::{
    AntigravityOAuthClient, AntigravityOAuthConfig, AntigravityOAuthCredentials,
    AntigravityOAuthCredentialsStore,
};
use super::quota_summary::{
    AntigravityQuotaSummary, AntigravityQuotaSummaryResponse, quota_summary_from_response,
};
use super::status_probe::{
    AntigravityModelQuota, AntigravitySnapshotSource, AntigravityStatusSnapshot,
};

pub const LOAD_CODE_ASSIST_ENDPOINT: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
pub const ONBOARD_USER_ENDPOINT: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:onboardUser";
pub const RETRIEVE_USER_QUOTA_SUMMARY_ENDPOINT: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
pub const FETCH_AVAILABLE_MODELS_ENDPOINT: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels";
pub const RETRIEVE_USER_QUOTA_ENDPOINT: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
pub const TOKEN_REFRESH_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

pub const ANTIGRAVITY_GOOGLE_ORIGINS: &[&str] = &[
    "https://cloudcode-pa.googleapis.com:443",
    "https://daily-cloudcode-pa.googleapis.com:443",
    "https://oauth2.googleapis.com:443",
    "https://www.googleapis.com:443",
    "https://accounts.google.com:443",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AntigravityRemoteFetchError {
    #[error("Not logged in to Antigravity OAuth. Click \"Sign in to Antigravity...\" in the menu.")]
    NotLoggedIn,
    #[error(
        "Antigravity OAuth needs a new sign-in. Click \"Sign in to Antigravity...\" in the menu."
    )]
    ReauthenticationRequired,
    #[error("Antigravity OAuth token refresh failed: {0}")]
    RefreshFailed(String),
    #[error("Antigravity remote API error: {0}")]
    ApiError(String),
    #[error("Could not parse Antigravity remote quota: {0}")]
    ParseFailed(String),
    #[error("Antigravity remote network error: {0}")]
    NetworkError(String),
}

impl AntigravityRemoteFetchError {
    pub fn into_classified_error(self) -> ClassifiedError {
        let kind = match &self {
            Self::NotLoggedIn => ProviderErrorKind::MissingCredential,
            Self::ReauthenticationRequired | Self::RefreshFailed(_) => {
                ProviderErrorKind::AuthenticationExpired
            }
            Self::ApiError(_) => ProviderErrorKind::ApiFailure,
            Self::ParseFailed(_) => ProviderErrorKind::ParseFailure,
            Self::NetworkError(_) => ProviderErrorKind::NetworkFailure,
        };
        ClassifiedError::new(kind, self.to_string())
    }
}

#[derive(Clone)]
pub struct RemoteHttpRequest {
    pub url: reqwest::Url,
    pub method: Method,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Vec<u8>>,
    pub timeout: Duration,
}

impl std::fmt::Debug for RemoteHttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteHttpRequest")
            .field("url", &self.url.as_str())
            .field("method", &self.method)
            .field("headers_count", &self.headers.len())
            .field("has_body", &self.body.is_some())
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl RemoteHttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase() == lower)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone)]
pub struct RemoteHttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl std::fmt::Debug for RemoteHttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteHttpResponse")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .finish()
    }
}

pub type RemoteDataLoader = Arc<
    dyn Fn(
            RemoteHttpRequest,
        ) -> BoxFuture<'static, Result<RemoteHttpResponse, AntigravityRemoteFetchError>>
        + Send
        + Sync,
>;

pub type CredentialsUpdateHandler = Arc<
    dyn Fn(
            AntigravityOAuthCredentials,
        ) -> BoxFuture<'static, Result<(), AntigravityRemoteFetchError>>
        + Send
        + Sync,
>;

pub type OAuthClientResolver = Arc<dyn Fn() -> Option<AntigravityOAuthClient> + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialSource {
    SharedFile,
    SelectedAccount,
}

#[derive(Clone)]
pub struct AntigravityRemoteUsageFetcher {
    timeout: Duration,
    home_directory: PathBuf,
    environment: BTreeMap<String, String>,
    net: Option<Arc<NetBroker>>,
    data_loader: Option<RemoteDataLoader>,
    credentials_update_handler: Option<CredentialsUpdateHandler>,
    oauth_client_resolver: OAuthClientResolver,
    clock: Arc<dyn Fn() -> Timestamp + Send + Sync>,
}

impl AntigravityRemoteUsageFetcher {
    pub fn new(timeout: Duration, home_directory: impl Into<PathBuf>) -> Self {
        let home_directory = home_directory.into();
        let env = BTreeMap::new();
        let resolver_home = home_directory.clone();
        let resolver_env = env.clone();
        Self {
            timeout,
            home_directory,
            environment: env,
            net: None,
            data_loader: None,
            credentials_update_handler: None,
            oauth_client_resolver: Arc::new(move || {
                AntigravityOAuthConfig::resolve_client(&resolver_env, None, Some(&resolver_home))
            }),
            clock: Arc::new(Timestamp::now),
        }
    }

    pub fn with_net(mut self, net: Arc<NetBroker>) -> Self {
        self.net = Some(net);
        self
    }

    pub fn with_environment(mut self, environment: BTreeMap<String, String>) -> Self {
        let resolver_home = self.home_directory.clone();
        let resolver_env = environment.clone();
        self.environment = environment;
        self.oauth_client_resolver = Arc::new(move || {
            AntigravityOAuthConfig::resolve_client(&resolver_env, None, Some(&resolver_home))
        });
        self
    }

    pub fn with_secret_environment(self, environment: &BTreeMap<String, SecretString>) -> Self {
        let map = environment
            .iter()
            .map(|(k, v)| (k.clone(), v.expose_secret().to_owned()))
            .collect();
        self.with_environment(map)
    }

    pub fn with_data_loader<F>(mut self, loader: F) -> Self
    where
        F: Fn(
                RemoteHttpRequest,
            )
                -> BoxFuture<'static, Result<RemoteHttpResponse, AntigravityRemoteFetchError>>
            + Send
            + Sync
            + 'static,
    {
        self.data_loader = Some(Arc::new(loader));
        self
    }

    pub fn with_credentials_update_handler<F>(mut self, handler: F) -> Self
    where
        F: Fn(
                AntigravityOAuthCredentials,
            ) -> BoxFuture<'static, Result<(), AntigravityRemoteFetchError>>
            + Send
            + Sync
            + 'static,
    {
        self.credentials_update_handler = Some(Arc::new(handler));
        self
    }

    pub fn with_oauth_client_resolver<F>(mut self, resolver: F) -> Self
    where
        F: Fn() -> Option<AntigravityOAuthClient> + Send + Sync + 'static,
    {
        self.oauth_client_resolver = Arc::new(resolver);
        self
    }

    pub fn with_clock<F>(mut self, clock: F) -> Self
    where
        F: Fn() -> Timestamp + Send + Sync + 'static,
    {
        self.clock = Arc::new(clock);
        self
    }

    pub async fn fetch(&self) -> Result<AntigravityStatusSnapshot, AntigravityRemoteFetchError> {
        let call = BrokerCall {
            interaction: FetchInteraction::UserInitiated,
            cancel: CancellationToken::new(),
            request_id: "antigravity-oauth-fetch".to_owned(),
        };
        self.fetch_with_call(&call).await
    }

    pub async fn fetch_with_call(
        &self,
        call: &BrokerCall,
    ) -> Result<AntigravityStatusSnapshot, AntigravityRemoteFetchError> {
        let (mut credentials, credential_source) = self.resolve_active_credentials(call)?;
        let now = (self.clock)();
        if credentials.needs_refresh_at(now) {
            let Some(refresh_token) = credentials
                .refresh_token
                .clone()
                .filter(|s| !s.trim().is_empty())
            else {
                return Err(AntigravityRemoteFetchError::NotLoggedIn);
            };
            credentials = self
                .refresh_token(call, &credentials, &refresh_token, credential_source)
                .await?;
        }

        let load_response = self
            .load_code_assist(call, &credentials.access_token)
            .await?;
        let project_id = self
            .resolve_project_id(call, &load_response, &credentials, credential_source)
            .await?;
        let quota_summary = self
            .retrieve_user_quota_summary(call, &credentials.access_token, project_id.as_deref())
            .await?;

        let model_quotas = if quota_summary
            .as_ref()
            .is_some_and(Self::quota_summary_has_known_usage)
        {
            Vec::new()
        } else {
            self.fetch_remote_model_quotas(call, &credentials.access_token, project_id.as_deref())
                .await?
        };

        let email = credentials.email.clone().or_else(|| {
            AntigravityOAuthConfig::extract_email_from_id_token(credentials.id_token.as_deref())
        });
        let hosted_domain = AntigravityOAuthConfig::extract_hosted_domain_from_id_token(
            credentials.id_token.as_deref(),
        );
        let plan = Self::resolve_plan(
            &load_response,
            hosted_domain.as_deref(),
            quota_summary.as_ref(),
        );

        Ok(AntigravityStatusSnapshot {
            model_quotas,
            account_email: email,
            account_plan: plan,
            source: AntigravitySnapshotSource::Remote,
            quota_summary,
        })
    }

    fn resolve_active_credentials(
        &self,
        call: &BrokerCall,
    ) -> Result<(AntigravityOAuthCredentials, CredentialSource), AntigravityRemoteFetchError> {
        if let Some(selected) =
            AntigravityOAuthCredentialsStore::load_selected_account_credentials(&self.environment)?
        {
            return Ok((selected, CredentialSource::SelectedAccount));
        }

        let cred_url = AntigravityOAuthCredentialsStore::credentials_url(&self.home_directory);
        let shared = AntigravityOAuthCredentialsStore::load_with_call(&cred_url, call)?;
        Ok((shared, CredentialSource::SharedFile))
    }

    async fn refresh_token(
        &self,
        call: &BrokerCall,
        credentials: &AntigravityOAuthCredentials,
        refresh_token: &str,
        source: CredentialSource,
    ) -> Result<AntigravityOAuthCredentials, AntigravityRemoteFetchError> {
        let client = self.resolve_oauth_client_for_refresh(credentials)?;
        let url = reqwest::Url::parse(TOKEN_REFRESH_ENDPOINT)
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))?;

        let mut headers = BTreeMap::new();
        headers.insert(
            "Content-Type".to_owned(),
            "application/x-www-form-urlencoded".to_owned(),
        );
        let body = Self::form_urlencoded_body(&[
            ("client_id", &client.client_id),
            ("client_secret", &client.client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ]);

        let response = self
            .perform_request(
                call,
                RemoteHttpRequest {
                    url,
                    method: Method::POST,
                    headers,
                    body: Some(body),
                    timeout: self.timeout,
                },
                None,
            )
            .await?;

        if response.status != 200 {
            return Err(AntigravityRemoteFetchError::RefreshFailed(format!(
                "HTTP {}",
                response.status
            )));
        }

        let refresh_response: TokenRefreshResponse = serde_json::from_slice(&response.body)
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))?;

        let now = (self.clock)();
        let expires_in_secs = refresh_response.expires_in.unwrap_or(3600.0).round() as i64;
        let expiry_date = now
            .saturating_add(jiff::Span::new().seconds(expires_in_secs))
            .ok();
        let id_token = refresh_response
            .id_token
            .or_else(|| credentials.id_token.clone());
        let email = AntigravityOAuthConfig::extract_email_from_id_token(id_token.as_deref())
            .or_else(|| credentials.email.clone());

        let updated = AntigravityOAuthCredentials {
            access_token: refresh_response.access_token,
            refresh_token: credentials.refresh_token.clone(),
            expiry_date,
            id_token,
            email,
            project_id: credentials.project_id.clone(),
            client_id: Some(client.client_id),
            client_secret: Some(client.client_secret),
        };

        self.persist_updated_credentials(&updated, source).await?;
        Ok(updated)
    }

    fn resolve_oauth_client_for_refresh(
        &self,
        credentials: &AntigravityOAuthCredentials,
    ) -> Result<AntigravityOAuthClient, AntigravityRemoteFetchError> {
        let stored_id = credentials
            .client_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let stored_secret = credentials
            .client_secret
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if let (Some(id), Some(secret)) = (stored_id, stored_secret) {
            return Ok(AntigravityOAuthClient::new(id, secret));
        }

        if let Some(client) = (self.oauth_client_resolver)() {
            return Ok(client);
        }

        Err(AntigravityRemoteFetchError::ApiError(format!(
            "Antigravity OAuth client is not configured. Ensure Antigravity.app is installed or set {} and {}.",
            AntigravityOAuthConfig::CLIENT_ID_ENV_KEY,
            AntigravityOAuthConfig::CLIENT_SECRET_ENV_KEY
        )))
    }

    async fn persist_updated_credentials(
        &self,
        credentials: &AntigravityOAuthCredentials,
        source: CredentialSource,
    ) -> Result<(), AntigravityRemoteFetchError> {
        match source {
            CredentialSource::SharedFile => {
                let cred_url =
                    AntigravityOAuthCredentialsStore::credentials_url(&self.home_directory);
                AntigravityOAuthCredentialsStore::save(credentials, &cred_url)
            }
            CredentialSource::SelectedAccount => {
                if let Some(handler) = &self.credentials_update_handler {
                    handler(credentials.clone()).await?;
                }
                Ok(())
            }
        }
    }

    async fn persist_project_id_if_needed(
        &self,
        project_id: Option<&str>,
        credentials: &AntigravityOAuthCredentials,
        source: CredentialSource,
    ) {
        let Some(trimmed_project_id) = project_id.map(str::trim).filter(|s| !s.is_empty()) else {
            return;
        };
        if credentials.project_id.as_deref() == Some(trimmed_project_id) {
            return;
        }
        let mut updated = credentials.clone();
        updated.project_id = Some(trimmed_project_id.to_owned());

        match source {
            CredentialSource::SharedFile => {
                let cred_url =
                    AntigravityOAuthCredentialsStore::credentials_url(&self.home_directory);
                let _ = AntigravityOAuthCredentialsStore::save(&updated, &cred_url);
            }
            CredentialSource::SelectedAccount => {
                if let Some(handler) = &self.credentials_update_handler {
                    let _ = handler(updated).await;
                }
            }
        }
    }

    async fn load_code_assist(
        &self,
        call: &BrokerCall,
        access_token: &str,
    ) -> Result<LoadCodeAssistResponse, AntigravityRemoteFetchError> {
        let body = json!({
            "metadata": {
                "ideType": "ANTIGRAVITY",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI",
            }
        });
        let data = self
            .perform_json_post(call, LOAD_CODE_ASSIST_ENDPOINT, access_token, &body)
            .await?;
        serde_json::from_slice(&data)
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))
    }

    async fn resolve_project_id(
        &self,
        call: &BrokerCall,
        load_response: &LoadCodeAssistResponse,
        credentials: &AntigravityOAuthCredentials,
        source: CredentialSource,
    ) -> Result<Option<String>, AntigravityRemoteFetchError> {
        if let Some(project_id) = load_response
            .cloudaicompanion_project
            .as_ref()
            .and_then(CloudAiCompanionProject::resolved_project_id)
        {
            self.persist_project_id_if_needed(Some(&project_id), credentials, source)
                .await;
            return Ok(Some(project_id));
        }
        if let Some(stored) = credentials
            .project_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Ok(Some(stored.to_owned()));
        }
        if Self::should_require_reauthentication(load_response) {
            return Err(AntigravityRemoteFetchError::ReauthenticationRequired);
        }
        if let Some(tier_id) = Self::pick_onboard_tier(load_response.allowed_tiers.as_deref()) {
            let onboarded = self
                .onboard_user(call, &credentials.access_token, &tier_id)
                .await?;
            self.persist_project_id_if_needed(onboarded.as_deref(), credentials, source)
                .await;
            return Ok(onboarded);
        }
        Ok(None)
    }

    pub fn should_require_reauthentication(load_response: &LoadCodeAssistResponse) -> bool {
        if load_response.current_tier.is_some()
            || load_response
                .cloudaicompanion_project
                .as_ref()
                .and_then(CloudAiCompanionProject::resolved_project_id)
                .is_some()
        {
            return false;
        }
        load_response
            .ineligible_tiers
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .any(|tier| {
                tier.reason_code
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|code| {
                        code.eq_ignore_ascii_case("GOOGLE_TOS_NOT_SUPPORTED_BY_CLIENT")
                    })
            })
    }

    pub fn pick_onboard_tier(allowed_tiers: Option<&[AllowedTier]>) -> Option<String> {
        let tiers = allowed_tiers?;
        if tiers.is_empty() {
            return None;
        }
        if let Some(default_tier) = tiers.iter().find(|t| t.is_default == Some(true))
            && let Some(id) = default_tier
                .id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        {
            return Some(id.to_owned());
        }
        if let Some(first_id) = tiers
            .iter()
            .filter_map(|t| t.id.as_deref().map(str::trim).filter(|s| !s.is_empty()))
            .next()
        {
            return Some(first_id.to_owned());
        }
        Some("LEGACY".to_owned())
    }

    async fn onboard_user(
        &self,
        call: &BrokerCall,
        access_token: &str,
        tier_id: &str,
    ) -> Result<Option<String>, AntigravityRemoteFetchError> {
        let body = json!({
            "tierId": tier_id,
            "metadata": {
                "ideType": "ANTIGRAVITY",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI",
            }
        });
        let data = self
            .perform_json_post(call, ONBOARD_USER_ENDPOINT, access_token, &body)
            .await?;
        let response: OnboardUserResponse = serde_json::from_slice(&data)
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))?;
        Ok(response
            .response
            .and_then(|r| r.cloudaicompanion_project)
            .and_then(|p| p.resolved_project_id()))
    }

    async fn retrieve_user_quota_summary(
        &self,
        call: &BrokerCall,
        access_token: &str,
        project_id: Option<&str>,
    ) -> Result<Option<AntigravityQuotaSummary>, AntigravityRemoteFetchError> {
        let mut payload = Map::new();
        if let Some(pid) = project_id.map(str::trim).filter(|s| !s.is_empty()) {
            payload.insert("project".to_owned(), Value::String(pid.to_owned()));
        }
        let body = serde_json::to_vec(&Value::Object(payload))
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))?;
        let url = reqwest::Url::parse(RETRIEVE_USER_QUOTA_SUMMARY_ENDPOINT)
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))?;

        let mut headers = BTreeMap::new();
        headers.insert("Authorization".to_owned(), format!("Bearer {access_token}"));
        headers.insert("Content-Type".to_owned(), "application/json".to_owned());

        let response = self
            .perform_request(
                call,
                RemoteHttpRequest {
                    url,
                    method: Method::POST,
                    headers,
                    body: Some(body),
                    timeout: self.timeout,
                },
                Some(access_token),
            )
            .await?;

        if matches!(response.status, 404 | 405 | 501 | 403) {
            return Ok(None);
        }
        if response.status == 401 {
            return Err(AntigravityRemoteFetchError::NotLoggedIn);
        }
        if response.status != 200 {
            return Err(AntigravityRemoteFetchError::ApiError(format!(
                "HTTP {}",
                response.status
            )));
        }

        let Ok(decoded) = serde_json::from_slice::<AntigravityQuotaSummaryResponse>(&response.body)
        else {
            return Ok(None);
        };
        Ok(quota_summary_from_response(&decoded).ok())
    }

    fn quota_summary_has_known_usage(summary: &AntigravityQuotaSummary) -> bool {
        summary
            .groups
            .iter()
            .flat_map(|g| &g.buckets)
            .any(|b| !b.disabled && b.remaining_fraction.is_some())
    }

    async fn fetch_remote_model_quotas(
        &self,
        call: &BrokerCall,
        access_token: &str,
        project_id: Option<&str>,
    ) -> Result<Vec<AntigravityModelQuota>, AntigravityRemoteFetchError> {
        match self
            .fetch_available_models(call, access_token, project_id)
            .await
        {
            Ok(available_models) => {
                if Self::should_verify_full_remote_quotas(&available_models) {
                    match self
                        .retrieve_user_quota(call, access_token, project_id)
                        .await
                    {
                        Ok(verified_quotas) => Ok(Self::merge_verified_quotas(
                            &available_models,
                            &verified_quotas,
                        )),
                        Err(err) if Self::is_permission_denied(&err) => Ok(available_models),
                        Err(err) => Err(err),
                    }
                } else {
                    Ok(available_models)
                }
            }
            Err(err) if Self::is_permission_denied(&err) => {
                match self
                    .retrieve_user_quota(call, access_token, project_id)
                    .await
                {
                    Ok(quotas) => Ok(quotas),
                    Err(quota_err) if Self::is_permission_denied(&quota_err) => Ok(Vec::new()),
                    Err(quota_err) => Err(quota_err),
                }
            }
            Err(err) => Err(err),
        }
    }

    async fn fetch_available_models(
        &self,
        call: &BrokerCall,
        access_token: &str,
        project_id: Option<&str>,
    ) -> Result<Vec<AntigravityModelQuota>, AntigravityRemoteFetchError> {
        let mut payload = Map::new();
        if let Some(pid) = project_id.map(str::trim).filter(|s| !s.is_empty()) {
            payload.insert("project".to_owned(), Value::String(pid.to_owned()));
        }
        let data = self
            .perform_json_post(
                call,
                FETCH_AVAILABLE_MODELS_ENDPOINT,
                access_token,
                &Value::Object(payload),
            )
            .await?;
        let response: FetchAvailableModelsResponse = serde_json::from_slice(&data)
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))?;

        let mut quotas = Vec::new();
        for (model_id, entry) in response.models.unwrap_or_default() {
            let Some(quota_info) = entry.quota_info else {
                continue;
            };
            let reset_time = quota_info
                .reset_time
                .as_deref()
                .and_then(Self::parse_reset_date);
            if quota_info.remaining_fraction.is_none() && reset_time.is_none() {
                continue;
            }
            let label = entry
                .display_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    entry
                        .label
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or(&model_id)
                .to_owned();
            quotas.push(AntigravityModelQuota {
                label,
                model_id,
                remaining_fraction: quota_info.remaining_fraction,
                reset_time,
                reset_description: None,
            });
        }
        Ok(quotas)
    }

    async fn retrieve_user_quota(
        &self,
        call: &BrokerCall,
        access_token: &str,
        project_id: Option<&str>,
    ) -> Result<Vec<AntigravityModelQuota>, AntigravityRemoteFetchError> {
        let mut payload = Map::new();
        if let Some(pid) = project_id.map(str::trim).filter(|s| !s.is_empty()) {
            payload.insert("project".to_owned(), Value::String(pid.to_owned()));
        }
        let data = self
            .perform_json_post(
                call,
                RETRIEVE_USER_QUOTA_ENDPOINT,
                access_token,
                &Value::Object(payload),
            )
            .await?;
        let response: RetrieveUserQuotaResponse = serde_json::from_slice(&data)
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))?;

        let mut quotas = Vec::new();
        for bucket in response.buckets.unwrap_or_default() {
            let Some(model_id) = bucket
                .model_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let reset_time = bucket
                .reset_time
                .as_deref()
                .and_then(Self::parse_reset_date);
            if bucket.remaining_fraction.is_none() && reset_time.is_none() {
                continue;
            }
            quotas.push(AntigravityModelQuota {
                label: model_id.to_owned(),
                model_id: model_id.to_owned(),
                remaining_fraction: bucket.remaining_fraction,
                reset_time,
                reset_description: None,
            });
        }
        Ok(quotas)
    }

    pub fn should_verify_full_remote_quotas(models: &[AntigravityModelQuota]) -> bool {
        !models.is_empty()
            && models
                .iter()
                .all(|m| m.remaining_fraction.unwrap_or(0.0) >= 0.999)
    }

    pub fn merge_verified_quotas(
        models: &[AntigravityModelQuota],
        verified_quotas: &[AntigravityModelQuota],
    ) -> Vec<AntigravityModelQuota> {
        if verified_quotas.is_empty() {
            return Vec::new();
        }
        let mut verified_by_id = BTreeMap::new();
        for quota in verified_quotas {
            verified_by_id
                .entry(quota.model_id.to_lowercase())
                .or_insert_with(|| quota.clone());
        }
        if !verified_by_id
            .values()
            .any(|q| q.remaining_fraction.is_some())
        {
            return Vec::new();
        }

        models
            .iter()
            .filter_map(|model| {
                let verified = verified_by_id.get(&model.model_id.to_lowercase())?;
                Some(AntigravityModelQuota {
                    label: model.label.clone(),
                    model_id: model.model_id.clone(),
                    remaining_fraction: verified.remaining_fraction.or(model.remaining_fraction),
                    reset_time: verified.reset_time.or(model.reset_time),
                    reset_description: verified
                        .reset_description
                        .clone()
                        .or_else(|| model.reset_description.clone()),
                })
            })
            .collect()
    }

    fn is_permission_denied(error: &AntigravityRemoteFetchError) -> bool {
        match error {
            AntigravityRemoteFetchError::ApiError(msg) => {
                let lower = msg.to_lowercase();
                lower.contains("403")
                    || lower.contains("permission_denied")
                    || lower.contains("permission denied")
            }
            _ => false,
        }
    }

    pub fn resolve_plan(
        load_response: &LoadCodeAssistResponse,
        hosted_domain: Option<&str>,
        quota_summary: Option<&AntigravityQuotaSummary>,
    ) -> Option<String> {
        if let Some(paid_name) = load_response
            .paid_tier
            .as_ref()
            .and_then(|t| t.name.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(paid_name.to_owned());
        }
        if let Some(paid_id) = load_response
            .paid_tier
            .as_ref()
            .and_then(|t| t.id.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let lower = paid_id.to_lowercase();
            if lower != "free" && lower != "free-tier" {
                return Some("Paid".to_owned());
            }
        }
        if let Some(plan_info) = &load_response.plan_info
            && let Some(preferred) = plan_info.preferred_name()
        {
            if preferred.trim().eq_ignore_ascii_case("free")
                && Self::has_measured_five_hour_quota(quota_summary)
            {
                return None;
            }
            return Some(preferred);
        }

        let tier_id = load_response
            .current_tier
            .as_ref()
            .and_then(|t| t.id.as_deref())
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let tier_name = load_response
            .current_tier
            .as_ref()
            .and_then(|t| t.name.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        match tier_id.as_str() {
            "standard-tier" => Some("Paid".to_owned()),
            "free-tier" => {
                if Self::has_measured_five_hour_quota(quota_summary) {
                    None
                } else if hosted_domain.is_some_and(|hd| !hd.trim().is_empty()) {
                    Some("Workspace".to_owned())
                } else {
                    Some("Free".to_owned())
                }
            }
            "legacy-tier" => Some("Legacy".to_owned()),
            _ => tier_name.map(str::to_owned),
        }
    }

    fn has_measured_five_hour_quota(summary: Option<&AntigravityQuotaSummary>) -> bool {
        let Some(summary) = summary else {
            return false;
        };
        summary
            .groups
            .iter()
            .flat_map(|g| &g.buckets)
            .any(|b| !b.disabled && b.remaining_fraction.is_some() && b.window_minutes == Some(300))
    }

    pub fn parse_reset_date(raw: &str) -> Option<Timestamp> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Ok(ts) = trimmed.parse::<Timestamp>() {
            return Some(ts);
        }
        if let Ok(seconds) = trimmed.parse::<f64>()
            && seconds.is_finite()
        {
            let whole_secs = seconds.trunc() as i64;
            let nanos = ((seconds - seconds.trunc()) * 1_000_000_000.0).round() as i32;
            return Timestamp::new(whole_secs, nanos).ok();
        }
        None
    }

    async fn perform_json_post(
        &self,
        call: &BrokerCall,
        endpoint: &str,
        access_token: &str,
        body: &Value,
    ) -> Result<Vec<u8>, AntigravityRemoteFetchError> {
        let url = reqwest::Url::parse(endpoint)
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))?;
        let body_bytes = serde_json::to_vec(body)
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))?;

        let mut headers = BTreeMap::new();
        headers.insert("Authorization".to_owned(), format!("Bearer {access_token}"));
        headers.insert("Content-Type".to_owned(), "application/json".to_owned());

        let response = self
            .perform_request(
                call,
                RemoteHttpRequest {
                    url,
                    method: Method::POST,
                    headers,
                    body: Some(body_bytes),
                    timeout: self.timeout,
                },
                Some(access_token),
            )
            .await?;

        if response.status == 401 {
            return Err(AntigravityRemoteFetchError::NotLoggedIn);
        }
        if response.status != 200 {
            let body_text = String::from_utf8_lossy(&response.body);
            let status_hint = if body_text.contains("PERMISSION_DENIED") {
                "PERMISSION_DENIED"
            } else {
                ""
            };
            let suffix = if status_hint.is_empty() {
                String::new()
            } else {
                format!(": {status_hint}")
            };
            return Err(AntigravityRemoteFetchError::ApiError(format!(
                "HTTP {}{suffix}",
                response.status
            )));
        }

        Ok(response.body)
    }

    async fn perform_request(
        &self,
        call: &BrokerCall,
        request: RemoteHttpRequest,
        bearer_token: Option<&str>,
    ) -> Result<RemoteHttpResponse, AntigravityRemoteFetchError> {
        if let Some(loader) = &self.data_loader {
            return loader(request).await;
        }
        let Some(net) = &self.net else {
            return Err(AntigravityRemoteFetchError::NetworkError(
                "No NetBroker or data loader configured".to_owned(),
            ));
        };

        let urls = ANTIGRAVITY_GOOGLE_ORIGINS
            .iter()
            .filter_map(|raw| reqwest::Url::parse(raw).ok());
        let origins = DeclaredOrigins::from_urls(urls)
            .map_err(|e| AntigravityRemoteFetchError::NetworkError(e.to_string()))?;

        let mut header_map = HeaderMap::new();
        for (k, v) in &request.headers {
            if k.eq_ignore_ascii_case("authorization") {
                continue;
            }
            if k.eq_ignore_ascii_case("content-type") {
                if let Ok(val) = HeaderValue::from_str(v) {
                    header_map.insert(CONTENT_TYPE, val);
                }
            } else if let (Ok(name), Ok(val)) = (
                HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                header_map.insert(name, val);
            }
        }

        let mut credentials = Vec::new();
        if let Some(token) = bearer_token {
            credentials.push((AUTHORIZATION, SecretString::new(format!("Bearer {token}"))));
        }

        let net_req = NetRequest {
            method: request.method,
            url: request.url,
            headers: header_map,
            credentials,
            body: request.body.map_or(NetBody::Empty, NetBody::secret),
            timeout: Some(request.timeout),
            body_cap: None,
        };

        let response =
            net.send_response(call, &origins, net_req)
                .await
                .map_err(|err| match err {
                    NetError::Transport(TransportClass::Timeout) | NetError::Cancelled => {
                        AntigravityRemoteFetchError::NetworkError("Request timed out".to_owned())
                    }
                    other => AntigravityRemoteFetchError::NetworkError(other.to_string()),
                })?;

        Ok(RemoteHttpResponse {
            status: response.status.as_u16(),
            body: response.body.to_vec(),
        })
    }

    pub fn form_urlencoded_body(fields: &[(&str, &str)]) -> Vec<u8> {
        fields
            .iter()
            .map(|(k, v)| {
                format!(
                    "{}={}",
                    Self::form_urlencoded_component(k),
                    Self::form_urlencoded_component(v)
                )
            })
            .collect::<Vec<_>>()
            .join("&")
            .into_bytes()
    }

    pub fn form_urlencoded_component(value: &str) -> String {
        let mut out = String::with_capacity(value.len());
        for &b in value.as_bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        out
    }
}

#[derive(Clone, Deserialize)]
struct TokenRefreshResponse {
    access_token: String,
    expires_in: Option<f64>,
    id_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoadCodeAssistResponse {
    #[serde(rename = "cloudaicompanionProject")]
    pub cloudaicompanion_project: Option<CloudAiCompanionProject>,
    #[serde(rename = "currentTier")]
    pub current_tier: Option<Tier>,
    #[serde(rename = "paidTier")]
    pub paid_tier: Option<Tier>,
    #[serde(rename = "allowedTiers")]
    pub allowed_tiers: Option<Vec<AllowedTier>>,
    #[serde(rename = "ineligibleTiers")]
    pub ineligible_tiers: Option<Vec<IneligibleTier>>,
    #[serde(rename = "planInfo")]
    pub plan_info: Option<PlanInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tier {
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AllowedTier {
    pub id: Option<String>,
    #[serde(rename = "isDefault")]
    pub is_default: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IneligibleTier {
    #[serde(rename = "reasonCode")]
    pub reason_code: Option<String>,
    #[serde(rename = "reasonMessage")]
    pub reason_message: Option<String>,
    #[serde(rename = "tierId")]
    pub tier_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlanInfo {
    #[serde(rename = "planName")]
    pub plan_name: Option<String>,
    #[serde(rename = "planDisplayName")]
    pub plan_display_name: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(rename = "productName")]
    pub product_name: Option<String>,
    #[serde(rename = "planShortName")]
    pub plan_short_name: Option<String>,
    #[serde(rename = "planType")]
    pub plan_type: Option<String>,
}

impl PlanInfo {
    pub fn preferred_name(&self) -> Option<String> {
        let candidates = [
            self.plan_display_name.as_deref(),
            self.display_name.as_deref(),
            self.product_name.as_deref(),
            self.plan_name.as_deref(),
            self.plan_short_name.as_deref(),
            self.plan_type.as_deref(),
        ];
        for candidate in candidates.into_iter().flatten() {
            let trimmed = candidate.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
        None
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum CloudAiCompanionProject {
    Id(String),
    Object { id: Option<String> },
}

impl CloudAiCompanionProject {
    pub fn resolved_project_id(&self) -> Option<String> {
        let raw = match self {
            Self::Id(id) => Some(id.as_str()),
            Self::Object { id } => id.as_deref(),
        }?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OnboardUserResponse {
    response: Option<OnboardUserInner>,
}

#[derive(Debug, Clone, Deserialize)]
struct OnboardUserInner {
    #[serde(rename = "cloudaicompanionProject")]
    cloudaicompanion_project: Option<CloudAiCompanionProject>,
}

#[derive(Debug, Clone, Deserialize)]
struct FetchAvailableModelsResponse {
    models: Option<BTreeMap<String, AvailableModelEntry>>,
}

#[derive(Debug, Clone, Deserialize)]
struct AvailableModelEntry {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    label: Option<String>,
    #[serde(rename = "quotaInfo")]
    quota_info: Option<AvailableModelQuotaInfo>,
}

#[derive(Debug, Clone, Deserialize)]
struct AvailableModelQuotaInfo {
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RetrieveUserQuotaResponse {
    buckets: Option<Vec<QuotaBucket>>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuotaBucket {
    #[serde(rename = "modelId")]
    model_id: Option<String>,
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}
