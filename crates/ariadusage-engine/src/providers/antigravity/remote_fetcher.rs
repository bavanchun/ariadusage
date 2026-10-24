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
use super::quota_summary::{AntigravityQuotaSummary, parse_quota_summary_response_with_source};
use super::status_probe::{
    AntigravityModelQuota, AntigravitySnapshotSource, AntigravityStatusSnapshot,
};

pub const LOAD_CODE_ASSIST_ENDPOINT: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
pub const ONBOARD_USER_ENDPOINT: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:onboardUser";
pub const RETRIEVE_USER_QUOTA_SUMMARY_ENDPOINT: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
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
    #[error("Antigravity Google auth not found. Use Antigravity login to authenticate.")]
    NotLoggedIn,
    #[error(
        "This Antigravity account was signed in with an OAuth client that cannot read its quota. Sign in to it again."
    )]
    ReauthenticationRequired,
    #[error("Antigravity remote API permission denied: {0}")]
    PermissionDenied(String),
    #[error("Antigravity OAuth token refresh failed: {0}")]
    RefreshFailed(String),
    #[error("Antigravity remote API error: {0}")]
    ApiError(String),
    #[error("Could not parse Antigravity remote usage: {0}")]
    ParseFailed(String),
    #[error("Antigravity remote network error: {0}")]
    NetworkError(String),
}

impl AntigravityRemoteFetchError {
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::NetworkError(msg) if msg.trim().eq_ignore_ascii_case("request cancelled"))
    }

    pub fn into_classified_error(self) -> ClassifiedError {
        let transport = match &self {
            Self::NetworkError(msg) if msg.trim().eq_ignore_ascii_case("request cancelled") => {
                Some(TransportClass::Cancelled)
            }
            Self::NetworkError(msg) if msg.trim().eq_ignore_ascii_case("request timed out") => {
                Some(TransportClass::Timeout)
            }
            _ => None,
        };
        let kind = match &self {
            Self::NotLoggedIn => ProviderErrorKind::MissingCredential,
            Self::ReauthenticationRequired | Self::RefreshFailed(_) => {
                ProviderErrorKind::AuthenticationExpired
            }
            Self::PermissionDenied(_) | Self::ApiError(_) => ProviderErrorKind::ApiFailure,
            Self::ParseFailed(_) => ProviderErrorKind::ParseFailure,
            Self::NetworkError(_) => ProviderErrorKind::NetworkFailure,
        };
        let classified = ClassifiedError::new(kind, self.to_string());
        if let Some(transport) = transport {
            classified.with_transport(transport)
        } else {
            classified
        }
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

pub type AsyncSleeper = Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>;

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
    sleeper: AsyncSleeper,
}

impl AntigravityRemoteUsageFetcher {
    pub fn new(timeout: Duration, home_directory: impl Into<PathBuf>) -> Self {
        let home_directory = home_directory.into();
        let env = BTreeMap::new();
        let resolver_env = env.clone();
        Self {
            timeout,
            home_directory,
            environment: env,
            net: None,
            data_loader: None,
            credentials_update_handler: None,
            oauth_client_resolver: Arc::new(move || {
                AntigravityOAuthConfig::resolve_client_from_env(&resolver_env)
            }),
            clock: Arc::new(Timestamp::now),
            sleeper: Arc::new(|d| Box::pin(tokio::time::sleep(d))),
        }
    }

    pub fn with_installed_client_discovery(mut self) -> Self {
        let resolver_home = self.home_directory.clone();
        let resolver_env = self.environment.clone();
        self.oauth_client_resolver = Arc::new(move || {
            AntigravityOAuthConfig::resolve_client_from_env(&resolver_env)
                .or_else(|| AntigravityOAuthConfig::installed_client_cached(Some(&resolver_home)))
        });
        self
    }

    pub fn with_net(mut self, net: Arc<NetBroker>) -> Self {
        self.net = Some(net);
        self
    }

    pub fn with_environment(mut self, environment: BTreeMap<String, String>) -> Self {
        let resolver_env = environment.clone();
        self.environment = environment;
        self.oauth_client_resolver =
            Arc::new(move || AntigravityOAuthConfig::resolve_client_from_env(&resolver_env));
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
        self.sleeper = Arc::new(|_| Box::pin(async {}));
        self
    }

    pub fn with_sleeper<F>(mut self, sleeper: F) -> Self
    where
        F: Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    {
        self.sleeper = Arc::new(sleeper);
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

    pub fn user_agent() -> &'static str {
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            "antigravity/hub/2.9.1 linux/arm64"
        }
        #[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
        {
            "antigravity/hub/2.9.1 linux/amd64"
        }
        #[cfg(all(not(target_os = "linux"), target_arch = "aarch64"))]
        {
            "antigravity/hub/2.9.1 darwin/arm64"
        }
        #[cfg(all(not(target_os = "linux"), not(target_arch = "aarch64")))]
        {
            "antigravity/hub/2.9.1 darwin/amd64"
        }
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
        if credentials.access_token.trim().is_empty() {
            return Err(AntigravityRemoteFetchError::NotLoggedIn);
        }

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
            if credential_source == CredentialSource::SharedFile {
                let cred_url =
                    AntigravityOAuthCredentialsStore::credentials_url(&self.home_directory);
                if let Ok(reloaded) =
                    AntigravityOAuthCredentialsStore::load_with_call(&cred_url, call)
                    && !reloaded.access_token.trim().is_empty()
                {
                    credentials = reloaded;
                }
            }
        }

        let email = credentials.resolved_account_email();
        let hosted_domain = AntigravityOAuthConfig::extract_hosted_domain_from_id_token(
            credentials.id_token.as_deref(),
        );

        let load_response = self
            .load_code_assist(call, &credentials.access_token)
            .await?;

        let stored_project_id = credentials
            .project_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        if stored_project_id.is_none() && Self::should_require_reauthentication(&load_response) {
            return Err(AntigravityRemoteFetchError::ReauthenticationRequired);
        }

        let project_id = self
            .resolve_project_id(
                call,
                &credentials.access_token,
                stored_project_id.as_deref(),
                &load_response,
            )
            .await?;

        if let Some(pid) = &project_id
            && stored_project_id.as_deref() != Some(pid.as_str())
        {
            credentials.project_id = Some(pid.clone());
            let _ = self
                .persist_updated_credentials(&credentials, credential_source)
                .await;
        }

        let plan = Self::resolve_plan(&load_response, hosted_domain.as_deref(), None);

        match self
            .retrieve_user_quota_summary(
                call,
                &credentials.access_token,
                project_id.as_deref(),
                email.clone(),
                plan.clone(),
            )
            .await
        {
            Ok(Some(snapshot)) => return Ok(snapshot),
            Ok(None) => {
                if call.cancel.is_cancelled() {
                    return Err(AntigravityRemoteFetchError::NetworkError(
                        "Request cancelled".to_owned(),
                    ));
                }
            }
            Err(AntigravityRemoteFetchError::NotLoggedIn) => {
                return Err(AntigravityRemoteFetchError::NotLoggedIn);
            }
            Err(err) => {
                if err.is_cancelled() || call.cancel.is_cancelled() {
                    return Err(AntigravityRemoteFetchError::NetworkError(
                        "Request cancelled".to_owned(),
                    ));
                }
            }
        }

        let model_quotas = self
            .fetch_remote_model_quotas(call, &credentials.access_token, project_id.as_deref())
            .await?;

        Ok(AntigravityStatusSnapshot {
            model_quotas,
            account_email: email,
            account_plan: plan,
            source: AntigravitySnapshotSource::Remote,
            quota_summary: None,
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
            return Err(AntigravityRemoteFetchError::NotLoggedIn);
        }

        let refresh_response: TokenRefreshResponse = serde_json::from_slice(&response.body)
            .map_err(|_| {
                AntigravityRemoteFetchError::ParseFailed(
                    "Could not parse refresh response".to_owned(),
                )
            })?;

        let now = (self.clock)();
        let expiry_date = refresh_response
            .expires_in
            .and_then(|secs| {
                let rounded = secs.round() as i64;
                now.saturating_add(jiff::Span::new().seconds(rounded)).ok()
            })
            .or(credentials.expiry_date);
        let id_token = refresh_response
            .id_token
            .or_else(|| credentials.id_token.clone());

        let updated = AntigravityOAuthCredentials {
            access_token: refresh_response.access_token,
            refresh_token: credentials.refresh_token.clone(),
            expiry_date,
            id_token,
            email: credentials.email.clone(),
            project_id: credentials.project_id.clone(),
            client_id: credentials.client_id.clone(),
            client_secret: credentials.client_secret.clone(),
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
            "Could not discover a matching Antigravity OAuth client. Update or install Antigravity.app, or set {} and {} before logging in.",
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
            .perform_json_post(
                call,
                LOAD_CODE_ASSIST_ENDPOINT,
                access_token,
                &body,
                self.timeout,
            )
            .await?;
        serde_json::from_slice(&data)
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))
    }

    async fn resolve_project_id(
        &self,
        call: &BrokerCall,
        access_token: &str,
        stored_project_id: Option<&str>,
        initial_response: &LoadCodeAssistResponse,
    ) -> Result<Option<String>, AntigravityRemoteFetchError> {
        if let Some(pid) = stored_project_id
            .map(str::to_owned)
            .or_else(|| initial_response.project_id())
        {
            return Ok(Some(pid));
        }

        let Some(tier_id) = Self::pick_onboard_tier_from_response(initial_response) else {
            return Ok(None);
        };

        if let Ok(Some(onboarded)) = self.onboard_user(call, access_token, &tier_id).await {
            return Ok(Some(onboarded));
        }

        for _ in 0..5 {
            (self.sleeper)(Duration::from_millis(2000)).await;
            let refreshed = self.load_code_assist(call, access_token).await?;
            if let Some(pid) = refreshed.project_id() {
                return Ok(Some(pid));
            }
        }

        Ok(None)
    }

    pub fn should_require_reauthentication(load_response: &LoadCodeAssistResponse) -> bool {
        if load_response.current_tier.is_some() || load_response.project_id().is_some() {
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
                    .is_some_and(|code| code == "GOOGLE_TOS_NOT_SUPPORTED_BY_CLIENT")
            })
    }

    pub fn pick_onboard_tier(allowed_tiers: Option<&[AllowedTier]>) -> Option<String> {
        let tiers = allowed_tiers?;
        if let Some(default_tier) = tiers.iter().find(|t| t.is_default == Some(true))
            && let Some(id) = default_tier
                .id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        {
            return Some(id.to_owned());
        }
        tiers
            .iter()
            .filter_map(|t| t.id.as_deref().map(str::trim).filter(|s| !s.is_empty()))
            .next()
            .map(str::to_owned)
    }

    pub fn pick_onboard_tier_from_response(response: &LoadCodeAssistResponse) -> Option<String> {
        Self::pick_onboard_tier(response.allowed_tiers.as_deref())
            .or_else(|| {
                response
                    .paid_tier
                    .as_ref()
                    .and_then(|t| t.id.as_deref())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            })
            .or_else(|| {
                response
                    .current_tier
                    .as_ref()
                    .and_then(|t| t.id.as_deref())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            })
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
            .perform_json_post(
                call,
                ONBOARD_USER_ENDPOINT,
                access_token,
                &body,
                self.timeout,
            )
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
        account_email: Option<String>,
        account_plan: Option<String>,
    ) -> Result<Option<AntigravityStatusSnapshot>, AntigravityRemoteFetchError> {
        let mut payload = Map::new();
        if let Some(pid) = project_id.map(str::trim).filter(|s| !s.is_empty()) {
            payload.insert("project".to_owned(), Value::String(pid.to_owned()));
        }
        let data = self
            .perform_json_post(
                call,
                RETRIEVE_USER_QUOTA_SUMMARY_ENDPOINT,
                access_token,
                &Value::Object(payload),
                self.timeout.min(Duration::from_secs(2)),
            )
            .await?;

        if call.cancel.is_cancelled() {
            return Err(AntigravityRemoteFetchError::NetworkError(
                "Request cancelled".to_owned(),
            ));
        }

        let mut summary =
            parse_quota_summary_response_with_source(&data, AntigravitySnapshotSource::Remote)
                .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))?;

        summary.account_email = account_email.clone();
        summary.account_plan = account_plan.clone();

        if !summary.has_known_quota_summary() {
            return Ok(None);
        }

        if account_plan
            .as_deref()
            .is_some_and(|p| p.eq_ignore_ascii_case("free"))
            && Self::has_measured_five_hour_quota(summary.quota_summary.as_ref())
        {
            summary.account_plan = None;
        }

        Ok(Some(summary))
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
                    let verified_quotas = self
                        .fetch_quota_buckets_if_permitted(call, access_token, project_id)
                        .await?;
                    let Some(verified_quotas) = verified_quotas else {
                        return Ok(Vec::new());
                    };
                    if !verified_quotas
                        .iter()
                        .any(|q| q.remaining_fraction.is_some())
                    {
                        return Ok(Vec::new());
                    }
                    Ok(Self::merge_verified_quotas(
                        &available_models,
                        &verified_quotas,
                    ))
                } else {
                    Ok(available_models)
                }
            }
            Err(AntigravityRemoteFetchError::PermissionDenied(_)) => Ok(self
                .fetch_quota_buckets_if_permitted(call, access_token, project_id)
                .await?
                .unwrap_or_default()),
            Err(err) => Err(err),
        }
    }

    async fn fetch_quota_buckets_if_permitted(
        &self,
        call: &BrokerCall,
        access_token: &str,
        project_id: Option<&str>,
    ) -> Result<Option<Vec<AntigravityModelQuota>>, AntigravityRemoteFetchError> {
        match self
            .retrieve_user_quota(call, access_token, project_id)
            .await
        {
            Ok(quotas) => Ok(Some(quotas)),
            Err(AntigravityRemoteFetchError::PermissionDenied(_)) => Ok(None),
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
                self.timeout,
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
                self.timeout,
            )
            .await?;
        let response: RetrieveUserQuotaResponse = serde_json::from_slice(&data)
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))?;

        let Some(buckets) = response.buckets else {
            return Err(AntigravityRemoteFetchError::ParseFailed(
                "No quota buckets in response".to_owned(),
            ));
        };

        let mut model_quota_map: BTreeMap<String, (Option<f64>, Option<String>)> = BTreeMap::new();
        for bucket in buckets {
            let Some(model_id) = bucket
                .model_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let next = (bucket.remaining_fraction, bucket.reset_time);
            if let Some(existing) = model_quota_map.get(model_id) {
                let existing_frac = existing.0.unwrap_or(f64::MAX);
                let next_frac = next.0.unwrap_or(f64::MAX);
                if existing_frac <= next_frac {
                    continue;
                }
            }
            model_quota_map.insert(model_id.to_owned(), next);
        }

        let quotas = model_quota_map
            .into_iter()
            .map(|(model_id, (fraction, reset_raw))| {
                let reset_time = reset_raw.as_deref().and_then(Self::parse_reset_date);
                AntigravityModelQuota {
                    label: model_id.clone(),
                    model_id,
                    remaining_fraction: fraction,
                    reset_time,
                    reset_description: None,
                }
            })
            .collect();
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
        let mut verified_by_id: BTreeMap<String, AntigravityModelQuota> = BTreeMap::new();
        for quota in verified_quotas {
            verified_by_id.insert(quota.model_id.trim().to_lowercase(), quota.clone());
        }

        let mut merged: Vec<AntigravityModelQuota> = models
            .iter()
            .filter_map(|model| {
                let key = model.model_id.trim().to_lowercase();
                let verified = verified_by_id.remove(&key)?;
                Some(AntigravityModelQuota {
                    label: model.label.clone(),
                    model_id: model.model_id.clone(),
                    remaining_fraction: verified.remaining_fraction.or(model.remaining_fraction),
                    reset_time: verified.reset_time.or(model.reset_time),
                    reset_description: verified
                        .reset_description
                        .or_else(|| model.reset_description.clone()),
                })
            })
            .collect();

        let mut unmatched: Vec<AntigravityModelQuota> = verified_by_id
            .into_values()
            .filter(|q| q.remaining_fraction.is_some())
            .collect();
        unmatched.sort_by(|lhs, rhs| {
            lhs.model_id
                .to_lowercase()
                .cmp(&rhs.model_id.to_lowercase())
        });
        merged.extend(unmatched);
        merged
    }

    pub fn resolve_plan(
        load_response: &LoadCodeAssistResponse,
        hosted_domain: Option<&str>,
        quota_summary: Option<&AntigravityQuotaSummary>,
    ) -> Option<String> {
        if let Some(paid_tier) = &load_response.paid_tier {
            let name = paid_tier
                .name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let id = paid_tier
                .id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if name.is_some() || id.is_some() {
                return Some(name.unwrap_or("Paid").to_owned());
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
            .trim();
        let tier_name = load_response
            .current_tier
            .as_ref()
            .and_then(|t| t.name.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        match tier_id {
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
        let snapshot = AntigravityStatusSnapshot::from_quota_summary(
            summary.clone(),
            None,
            None,
            AntigravitySnapshotSource::Remote,
        );
        snapshot
            .to_usage_snapshot()
            .ok()
            .and_then(|u| u.extra_rate_windows)
            .is_some_and(|ws| {
                ws.iter()
                    .any(|w| w.usage_known && w.window.window_minutes == Some(300))
            })
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
        timeout: Duration,
    ) -> Result<Vec<u8>, AntigravityRemoteFetchError> {
        let url = reqwest::Url::parse(endpoint).map_err(|e| {
            AntigravityRemoteFetchError::ApiError(format!("Invalid endpoint URL: {e}"))
        })?;
        let body_bytes = serde_json::to_vec(body)
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))?;

        let mut headers = BTreeMap::new();
        headers.insert("Authorization".to_owned(), format!("Bearer {access_token}"));
        headers.insert("Content-Type".to_owned(), "application/json".to_owned());
        headers.insert("User-Agent".to_owned(), Self::user_agent().to_owned());

        let response = self
            .perform_request(
                call,
                RemoteHttpRequest {
                    url,
                    method: Method::POST,
                    headers,
                    body: Some(body_bytes),
                    timeout,
                },
                Some(access_token),
            )
            .await?;

        match response.status {
            200 => Ok(response.body),
            401 => Err(AntigravityRemoteFetchError::NotLoggedIn),
            403 => Err(AntigravityRemoteFetchError::PermissionDenied(
                "HTTP 403".to_owned(),
            )),
            status => Err(AntigravityRemoteFetchError::ApiError(format!(
                "HTTP {status}"
            ))),
        }
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
                    NetError::Cancelled | NetError::Transport(TransportClass::Cancelled) => {
                        AntigravityRemoteFetchError::NetworkError("Request cancelled".to_owned())
                    }
                    NetError::Transport(TransportClass::Timeout) => {
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

impl LoadCodeAssistResponse {
    pub fn project_id(&self) -> Option<String> {
        self.cloudaicompanion_project
            .as_ref()
            .and_then(CloudAiCompanionProject::resolved_project_id)
    }

    pub fn rejects_client_for_consumer_tier(&self) -> bool {
        AntigravityRemoteUsageFetcher::should_require_reauthentication(self)
    }
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
            self.plan_type.as_deref(),
            self.plan_display_name.as_deref(),
            self.display_name.as_deref(),
            self.product_name.as_deref(),
            self.plan_name.as_deref(),
            self.plan_short_name.as_deref(),
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
    Object {
        id: Option<String>,
        #[serde(rename = "projectId")]
        project_id: Option<String>,
    },
}

impl CloudAiCompanionProject {
    pub fn resolved_project_id(&self) -> Option<String> {
        let raw = match self {
            Self::Id(id) => Some(id.as_str()),
            Self::Object { id, project_id } => id.as_deref().or(project_id.as_deref()),
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
