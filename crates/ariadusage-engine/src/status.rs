// Ported from CodexBar Sources/CodexBarCore/ProviderStatus.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/ProviderStatusFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+ProviderStatus.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::hosts::ProviderHosts;
use ariadusage_core::pipeline::{BoxFuture, TransportClass};
use ariadusage_core::providers::ProviderDescriptor;
pub use ariadusage_core::status::{
    ProviderStatusSummary, StatusParseError, StatuspageComponentSummary,
    indicator_for_statuspage_status, overlay_incident_io_with_statuspage,
    parse_google_workspace_status, parse_incident_io_summary, parse_statuspage_components,
    parse_statuspage_status, status_indicator_has_issue, status_indicator_label,
    status_indicator_rank, workspace_summary,
};
use ariadusage_protocol::{ProviderId, ProviderStatus};
use reqwest::header::{ACCEPT, HeaderValue};
use reqwest::{Method, Url};
use thiserror::Error;

use crate::brokers::call::BrokerCall;
use crate::brokers::net::{DeclaredOrigins, NetBroker, NetError, NetRequest};

/// Default per-request timeout for status feed fetches (`ProviderStatusFetcher.swift`: 10s).
pub const DEFAULT_STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// Default Google Workspace status incidents endpoint used for Antigravity.
pub const GOOGLE_WORKSPACE_INCIDENTS_URL: &str =
    "https://www.google.com/appsstatus/dashboard/incidents.json";

/// Returns the canonical declared hostnames for first-party provider status feeds.
pub fn status_declared_hosts() -> ProviderHosts {
    ProviderHosts::new([
        "status.claude.com",
        "status.anthropic.com",
        "status.openai.com",
        "www.google.com",
    ])
    .expect("valid status feed host declarations")
}

/// Returns the declared HTTPS origins for first-party provider status feeds.
pub fn status_declared_origins() -> DeclaredOrigins {
    DeclaredOrigins::from_provider_hosts(&status_declared_hosts())
}

/// Errors returned while fetching or decoding a provider status feed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StatusFetchError {
    #[error("status request cancelled")]
    Cancelled,
    #[error("invalid status feed URL")]
    InvalidUrl,
    #[error("status feed origin is not declared")]
    OriginNotDeclared,
    #[error("status feed returned HTTP {status}")]
    Http { status: u16 },
    #[error("status feed network failure")]
    Network(Option<TransportClass>),
    #[error("status feed parse failure: {0}")]
    Parse(String),
}

impl From<StatusParseError> for StatusFetchError {
    fn from(err: StatusParseError) -> Self {
        Self::Parse(err.to_string())
    }
}

fn map_net_error(err: NetError) -> StatusFetchError {
    match err {
        NetError::Cancelled => StatusFetchError::Cancelled,
        NetError::InvalidUrl => StatusFetchError::InvalidUrl,
        NetError::OriginNotDeclared => StatusFetchError::OriginNotDeclared,
        NetError::Http { status } => StatusFetchError::Http { status },
        NetError::Transport(transport) => StatusFetchError::Network(Some(transport)),
        _ => StatusFetchError::Network(None),
    }
}

/// Optional async override for status fetches in tests (`UsageStore._test_providerStatusFetchOverride`).
pub type StatusFetchOverrideFn = Arc<
    dyn Fn(
            ProviderId,
        ) -> BoxFuture<'static, Result<Option<ProviderStatusSummary>, StatusFetchError>>
        + Send
        + Sync,
>;

/// Configuration and fetcher for Statuspage / incident.io and Google Workspace status feeds,
/// routed exclusively through `NetBroker` with declared origins.
#[derive(Clone)]
pub struct ProviderStatusFetcher {
    declared_origins: DeclaredOrigins,
    workspace_incidents_url: Url,
    timeout: Duration,
    base_url_overrides: HashMap<String, Url>,
    proxy_host_overrides: HashMap<String, String>,
    fetch_override: Option<StatusFetchOverrideFn>,
}

impl fmt::Debug for ProviderStatusFetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderStatusFetcher")
            .field("declared_origins", &self.declared_origins)
            .field("workspace_incidents_url", &self.workspace_incidents_url)
            .field("timeout", &self.timeout)
            .field("base_url_overrides", &self.base_url_overrides)
            .field("proxy_host_overrides", &self.proxy_host_overrides)
            .field("has_fetch_override", &self.fetch_override.is_some())
            .finish()
    }
}

impl Default for ProviderStatusFetcher {
    fn default() -> Self {
        Self {
            declared_origins: status_declared_origins(),
            workspace_incidents_url: Url::parse(GOOGLE_WORKSPACE_INCIDENTS_URL)
                .expect("valid Google Workspace incidents URL"),
            timeout: DEFAULT_STATUS_TIMEOUT,
            base_url_overrides: HashMap::new(),
            proxy_host_overrides: HashMap::new(),
            fetch_override: None,
        }
    }
}

impl ProviderStatusFetcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_fetch_override(mut self, fetch_override: StatusFetchOverrideFn) -> Self {
        self.fetch_override = Some(fetch_override);
        self
    }

    pub fn with_declared_origins(mut self, declared_origins: DeclaredOrigins) -> Self {
        self.declared_origins = declared_origins;
        self
    }

    pub fn with_workspace_incidents_url(mut self, url: Url) -> Self {
        self.workspace_incidents_url = url;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_provider_base_url(mut self, provider_id: impl Into<String>, base_url: Url) -> Self {
        self.base_url_overrides.insert(provider_id.into(), base_url);
        self
    }

    pub fn with_provider_proxy_host(
        mut self,
        provider_id: impl Into<String>,
        proxy_host: impl Into<String>,
    ) -> Self {
        self.proxy_host_overrides
            .insert(provider_id.into(), proxy_host.into());
        self
    }

    pub fn declared_origins(&self) -> &DeclaredOrigins {
        &self.declared_origins
    }

    /// Fetches the status summary for a provider descriptor if it supports status polling.
    pub async fn fetch_for_descriptor(
        &self,
        net: &NetBroker,
        call: &BrokerCall,
        descriptor: &ProviderDescriptor,
    ) -> Result<Option<ProviderStatusSummary>, StatusFetchError> {
        let default_page_url = descriptor
            .status_page_url
            .or(descriptor.status_link_url)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if let Some(override_fn) = &self.fetch_override {
            let mut summary = override_fn(descriptor.id.clone()).await?;
            if let Some(ref mut s) = summary
                && s.status.page_url.is_none()
            {
                s.status.page_url = default_page_url;
            }
            return Ok(summary);
        }
        let provider_id = descriptor.id.as_str();
        if let Some(override_url) = self.base_url_overrides.get(provider_id) {
            let proxy_host = self
                .proxy_host_overrides
                .get(provider_id)
                .map(String::as_str);
            let mut summary = self
                .fetch_status_summary_with_proxy_host(net, call, override_url, proxy_host)
                .await?;
            if summary.status.page_url.is_none() {
                summary.status.page_url = default_page_url;
            }
            return Ok(Some(summary));
        }

        if let Some(raw_url) = descriptor.status_page_url {
            let trimmed = raw_url.trim();
            if !trimmed.is_empty() {
                let base_url = Url::parse(trimmed).map_err(|_| StatusFetchError::InvalidUrl)?;
                let proxy_host = self
                    .proxy_host_overrides
                    .get(provider_id)
                    .map(String::as_str);
                let mut summary = self
                    .fetch_status_summary_with_proxy_host(net, call, &base_url, proxy_host)
                    .await?;
                if summary.status.page_url.is_none() {
                    summary.status.page_url = default_page_url;
                }
                return Ok(Some(summary));
            }
        }

        if let Some(product_id) = descriptor.status_workspace_product_id {
            let trimmed = product_id.trim();
            if !trimmed.is_empty() {
                let mut status = self.fetch_workspace_status(net, call, trimmed).await?;
                if status.page_url.is_none() {
                    status.page_url = default_page_url;
                }
                return Ok(Some(ProviderStatusSummary::new(status, Vec::new())));
            }
        }

        Ok(None)
    }

    /// Fetches only the overall Statuspage `api/v2/status.json` for `base_url`.
    pub async fn fetch_status(
        &self,
        net: &NetBroker,
        call: &BrokerCall,
        base_url: &Url,
    ) -> Result<ProviderStatus, StatusFetchError> {
        let normalized = ensure_trailing_slash(base_url);
        let api_url = normalized
            .join("api/v2/status.json")
            .map_err(|_| StatusFetchError::InvalidUrl)?;
        let bytes = self.get_bytes(net, call, api_url).await?;
        Ok(parse_statuspage_status(&bytes)?)
    }

    /// Fetches the full status summary (incident.io `/proxy/<host>` with `api/v2/status.json`
    /// overlay, falling back to Statuspage `api/v2/summary.json` + `api/v2/components.json`).
    pub async fn fetch_status_summary(
        &self,
        net: &NetBroker,
        call: &BrokerCall,
        base_url: &Url,
    ) -> Result<ProviderStatusSummary, StatusFetchError> {
        self.fetch_status_summary_with_proxy_host(net, call, base_url, None)
            .await
    }

    pub async fn fetch_status_summary_with_proxy_host(
        &self,
        net: &NetBroker,
        call: &BrokerCall,
        base_url: &Url,
        proxy_host_override: Option<&str>,
    ) -> Result<ProviderStatusSummary, StatusFetchError> {
        let normalized = ensure_trailing_slash(base_url);
        let host = proxy_host_override.or_else(|| normalized.host_str());

        if let Some(host) = host
            && let Ok(proxy_url) = normalized.join(&format!("proxy/{host}"))
        {
            match self.fetch_incident_io_summary(net, call, proxy_url).await {
                Ok(summary) => {
                    let status_page = match normalized.join("api/v2/status.json") {
                        Ok(status_url) => match self.get_bytes(net, call, status_url).await {
                            Ok(bytes) => parse_statuspage_status(&bytes).ok(),
                            Err(StatusFetchError::Cancelled) => {
                                return Err(StatusFetchError::Cancelled);
                            }
                            Err(_) => None,
                        },
                        Err(_) => None,
                    };
                    return Ok(overlay_incident_io_with_statuspage(summary, status_page));
                }
                Err(StatusFetchError::Cancelled | StatusFetchError::OriginNotDeclared) => {
                    if call.cancel.is_cancelled() {
                        return Err(StatusFetchError::Cancelled);
                    }
                    return Err(StatusFetchError::OriginNotDeclared);
                }
                Err(_) => {
                    // Fall through to standard Statuspage endpoints when the incident.io feed is unavailable.
                }
            }
        }

        let summary_url = normalized
            .join("api/v2/summary.json")
            .map_err(|_| StatusFetchError::InvalidUrl)?;
        let summary_bytes = self.get_bytes(net, call, summary_url).await?;
        let status = parse_statuspage_status(&summary_bytes)?;

        let components = match normalized.join("api/v2/components.json") {
            Ok(components_url) => match self.get_bytes(net, call, components_url).await {
                Ok(bytes) => parse_statuspage_components(&bytes).unwrap_or_default(),
                Err(StatusFetchError::Cancelled) => return Err(StatusFetchError::Cancelled),
                Err(_) => Vec::new(),
            },
            Err(_) => Vec::new(),
        };

        Ok(ProviderStatusSummary::new(status, components))
    }

    /// Fetches Google Workspace status for `product_id` (`Antigravity`).
    pub async fn fetch_workspace_status(
        &self,
        net: &NetBroker,
        call: &BrokerCall,
        product_id: &str,
    ) -> Result<ProviderStatus, StatusFetchError> {
        let bytes = self
            .get_bytes(net, call, self.workspace_incidents_url.clone())
            .await?;
        Ok(parse_google_workspace_status(&bytes, product_id)?)
    }

    async fn fetch_incident_io_summary(
        &self,
        net: &NetBroker,
        call: &BrokerCall,
        url: Url,
    ) -> Result<ProviderStatusSummary, StatusFetchError> {
        let bytes = self.get_bytes(net, call, url).await?;
        Ok(parse_incident_io_summary(&bytes)?)
    }

    async fn get_bytes(
        &self,
        net: &NetBroker,
        call: &BrokerCall,
        url: Url,
    ) -> Result<Vec<u8>, StatusFetchError> {
        let mut request = NetRequest::new(Method::GET, url);
        request
            .headers
            .insert(ACCEPT, HeaderValue::from_static("application/json"));
        request.timeout = Some(self.timeout);
        let response = net
            .send(call, &self.declared_origins, request)
            .await
            .map_err(map_net_error)?;
        Ok(response.body().to_vec())
    }
}

fn ensure_trailing_slash(url: &Url) -> Url {
    let mut normalized = url.clone();
    if !normalized.path().ends_with('/') {
        let new_path = format!("{}/", normalized.path());
        normalized.set_path(&new_path);
    }
    normalized
}
