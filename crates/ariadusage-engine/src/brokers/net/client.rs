// Ported from CodexBar Sources/CodexBarCore/ProviderHTTPClient.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use ariadusage_protocol::secret::SecretString;
use reqwest::dns::Resolve;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method, Response, StatusCode, Url};
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use rustls::{ClientConfig, RootCertStore};
use zeroize::Zeroizing;

use crate::brokers::call::BrokerCall;

use super::body::NetBody;
use super::classify::transport_class;
use super::error::NetError;
use super::origin::{DeclaredOrigins, Origin};
use super::redirect;

const DEFAULT_BODY_CAP: usize = 5 * 1024 * 1024;

/// HTTP client limits shared by the provider and loopback clients.
#[derive(Clone, Debug)]
pub struct NetConfig {
    pub body_cap: usize,
    pub read_timeout: Duration,
    pub total_timeout: Duration,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            body_cap: DEFAULT_BODY_CAP,
            read_timeout: Duration::from_secs(30),
            total_timeout: Duration::from_secs(90),
        }
    }
}

/// A request sent through the Net broker.
pub struct NetRequest {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    pub credentials: Vec<(HeaderName, ariadusage_protocol::secret::SecretString)>,
    pub body: NetBody,
    pub timeout: Option<Duration>,
    pub body_cap: Option<usize>,
}

impl NetRequest {
    pub fn new(method: Method, url: Url) -> Self {
        Self {
            method,
            url,
            headers: HeaderMap::new(),
            credentials: Vec::new(),
            body: NetBody::Empty,
            timeout: None,
            body_cap: None,
        }
    }
}

impl fmt::Debug for NetRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names = self
            .headers
            .keys()
            .map(HeaderName::as_str)
            .collect::<Vec<_>>();
        let credential_names = self
            .credentials
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        f.debug_struct("NetRequest")
            .field("method", &self.method)
            .field("url", &"[redacted]")
            .field("header_names", &header_names)
            .field("credential_header_names", &credential_names)
            .field("body", &self.body)
            .field("timeout", &self.timeout)
            .field("body_cap", &self.body_cap)
            .finish()
    }
}

/// Headers with values kept out of debug output.
#[derive(Clone, Default)]
pub struct RedactedHeaders(HeaderMap);

impl RedactedHeaders {
    pub fn get(&self, name: &str) -> Option<&HeaderValue> {
        self.0.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&HeaderName, &HeaderValue)> {
        self.0.iter()
    }

    fn from_sensitive(headers: HeaderMap) -> Self {
        let mut headers = headers;
        for (_, value) in headers.iter_mut() {
            value.set_sensitive(true);
        }
        Self(headers)
    }
}

impl fmt::Debug for RedactedHeaders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = self.0.keys().map(HeaderName::as_str).collect::<Vec<_>>();
        f.debug_struct("RedactedHeaders")
            .field("names", &names)
            .finish()
    }
}

/// A response body is zeroized when its value is dropped.
pub struct NetResponse {
    pub status: StatusCode,
    pub headers: RedactedHeaders,
    pub body: Zeroizing<Vec<u8>>,
}

impl NetResponse {
    pub fn body(&self) -> &[u8] {
        self.body.as_slice()
    }
}

impl fmt::Debug for NetResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetResponse")
            .field("status", &self.status.as_u16())
            .field("headers", &self.headers)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// Provider HTTP and literal-loopback clients, constructed once for reuse.
pub struct NetBroker {
    provider_client: Client,
    loopback_client: Client,
    config: NetConfig,
}

impl NetBroker {
    pub fn new(config: NetConfig) -> Result<Self, NetError> {
        Self::build(config, &[], None, false)
    }

    #[cfg(feature = "test-hooks")]
    pub fn new_with_test_ca(config: NetConfig, ca_pem: &[u8]) -> Result<Self, NetError> {
        Self::build(config, &[ca_pem.to_vec()], None, true)
    }

    #[cfg(feature = "test-hooks")]
    pub fn new_with_test_ca_and_system_proxy(
        config: NetConfig,
        ca_pem: &[u8],
    ) -> Result<Self, NetError> {
        Self::build(config, &[ca_pem.to_vec()], None, false)
    }

    #[cfg(feature = "test-hooks")]
    pub fn new_with_resolver(
        config: NetConfig,
        resolver: Arc<dyn Resolve>,
    ) -> Result<Self, NetError> {
        Self::build(config, &[], Some(resolver), true)
    }

    fn build(
        config: NetConfig,
        test_roots: &[Vec<u8>],
        resolver: Option<Arc<dyn Resolve>>,
        disable_provider_proxy: bool,
    ) -> Result<Self, NetError> {
        let provider_client = build_client(
            &config,
            test_roots,
            resolver.clone(),
            false,
            disable_provider_proxy,
        )?;
        let loopback_client = build_client(&config, test_roots, resolver, true, true)?;

        Ok(Self {
            provider_client,
            loopback_client,
            config,
        })
    }

    pub async fn send(
        &self,
        call: &BrokerCall,
        declared_origins: &DeclaredOrigins,
        request: NetRequest,
    ) -> Result<NetResponse, NetError> {
        if call.cancel.is_cancelled() {
            return Err(NetError::Cancelled);
        }

        let origin = Origin::of(&request.url).ok_or(NetError::InvalidUrl)?;
        if !declared_origins.contains(&origin) {
            return Err(NetError::OriginNotDeclared);
        }

        let cap = request.body_cap.unwrap_or(self.config.body_cap);
        let response = send_request(&self.provider_client, call, request).await?;
        collect_response(response, call, cap).await
    }

    pub async fn send_loopback(
        &self,
        call: &BrokerCall,
        request: NetRequest,
    ) -> Result<NetResponse, NetError> {
        if call.cancel.is_cancelled() {
            return Err(NetError::Cancelled);
        }

        let (expected_ip, expected_port) =
            loopback_target(&request.url).ok_or(NetError::LoopbackOnly)?;
        let cap = request.body_cap.unwrap_or(self.config.body_cap);
        let response = send_request(&self.loopback_client, call, request).await?;
        let remote = response.remote_addr().ok_or(NetError::LoopbackOnly)?;
        if remote.ip() != expected_ip || remote.port() != expected_port {
            return Err(NetError::LoopbackOnly);
        }

        collect_response(response, call, cap).await
    }
}

fn build_client(
    config: &NetConfig,
    test_roots: &[Vec<u8>],
    resolver: Option<Arc<dyn Resolve>>,
    loopback: bool,
    disable_proxy: bool,
) -> Result<Client, NetError> {
    let tls = tls_config(test_roots)?;
    let mut builder = Client::builder()
        .tls_backend_preconfigured(tls)
        .redirect(if loopback {
            reqwest::redirect::Policy::none()
        } else {
            redirect::policy()
        })
        .timeout(config.total_timeout)
        .read_timeout(config.read_timeout)
        .referer(false)
        .https_only(!loopback);

    if loopback || disable_proxy {
        builder = builder.no_proxy();
    }
    if let Some(resolver) = resolver {
        builder = builder.dns_resolver(resolver);
    }

    builder.build().map_err(|error| reqwest_error(&error))
}

fn tls_config(test_roots: &[Vec<u8>]) -> Result<ClientConfig, NetError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier: Arc<dyn ServerCertVerifier> = if test_roots.is_empty() {
        Arc::new(
            rustls_platform_verifier::Verifier::new(provider.clone())
                .map_err(|_| NetError::Other)?,
        )
    } else {
        let mut root_store = RootCertStore::empty();
        let mut root_count = 0;
        for pem in test_roots {
            for certificate in CertificateDer::pem_slice_iter(pem) {
                root_store
                    .add(certificate.map_err(|_| NetError::Other)?)
                    .map_err(|_| NetError::Other)?;
                root_count += 1;
            }
        }
        if root_count == 0 {
            return Err(NetError::Other);
        }
        rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(root_store),
            provider.clone(),
        )
        .build()
        .map_err(|_| NetError::Other)?
    };

    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| NetError::Other)?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

async fn send_request(
    client: &Client,
    call: &BrokerCall,
    request: NetRequest,
) -> Result<Response, NetError> {
    let mut headers = request.headers;
    for (name, secret) in request.credentials {
        headers.insert(name, sensitive_credential_value(&secret)?);
    }
    for (_, value) in headers.iter_mut() {
        value.set_sensitive(true);
    }

    let mut builder = client.request(request.method, request.url).headers(headers);
    if let Some(timeout) = request.timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(body) = request.body.into_request_body() {
        builder = builder.body(body);
    }

    tokio::select! {
        biased;
        _ = call.cancel.cancelled() => Err(NetError::Cancelled),
        response = builder.send() => response.map_err(|error| reqwest_error(&error)),
    }
}

fn sensitive_credential_value(secret: &SecretString) -> Result<HeaderValue, NetError> {
    let mut value =
        HeaderValue::from_str(secret.expose_secret()).map_err(|_| NetError::InvalidHeader)?;
    value.set_sensitive(true);
    Ok(value)
}

#[cfg(feature = "test-hooks")]
pub fn test_credential_header_value(secret: &SecretString) -> Result<HeaderValue, NetError> {
    sensitive_credential_value(secret)
}

async fn collect_response(
    mut response: Response,
    call: &BrokerCall,
    cap: usize,
) -> Result<NetResponse, NetError> {
    let status = response.status();
    let headers = RedactedHeaders::from_sensitive(response.headers().clone());
    let mut body = Zeroizing::new(Vec::with_capacity(cap.min(64 * 1024)));

    loop {
        let chunk = tokio::select! {
            biased;
            _ = call.cancel.cancelled() => return Err(NetError::Cancelled),
            chunk = response.chunk() => chunk.map_err(|error| reqwest_error(&error))?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        if body.len().saturating_add(chunk.len()) > cap {
            return Err(NetError::BodyTooLarge { cap });
        }
        body.extend_from_slice(&chunk);
    }

    if status.is_client_error() || status.is_server_error() {
        return Err(NetError::Http {
            status: status.as_u16(),
        });
    }

    Ok(NetResponse {
        status,
        headers,
        body,
    })
}

fn reqwest_error(error: &reqwest::Error) -> NetError {
    if error.is_redirect() {
        return NetError::Redirect;
    }
    transport_class(error).map_or(NetError::Other, NetError::Transport)
}

fn loopback_target(url: &Url) -> Option<(IpAddr, u16)> {
    if (!url.scheme().eq_ignore_ascii_case("http") && !url.scheme().eq_ignore_ascii_case("https"))
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }

    let host = url.host_str()?;
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    let address = host.parse::<IpAddr>().ok()?;
    let is_allowed = match address {
        IpAddr::V4(ip) => ip.octets() == [127, 0, 0, 1],
        IpAddr::V6(ip) => ip.is_loopback(),
    };
    if is_allowed {
        Some((address, url.port_or_known_default()?))
    } else {
        None
    }
}
