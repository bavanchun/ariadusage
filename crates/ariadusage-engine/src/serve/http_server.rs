// Ported from CodexBar Sources/CodexBarCLI/CLILocalHTTPServer.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
//
// Loopback-first HTTP/1.1 server with pre-auth connection gating, header-read
// deadlines, Host-header validation, and axum/tower/hyper-util dispatch.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use http::{HeaderName, HeaderValue, Request, Response, StatusCode};
use http_body_util::BodyExt;
use hyper::service::Service as HyperService;
use hyper_util::service::TowerToHyperService;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tower::ServiceBuilder;

use super::auth::CliServeSecurity;

pub const MAXIMUM_REQUEST_BYTES: usize = 16_384;
pub const DEFAULT_READ_TIMEOUT_MS: i64 = 5_000;
pub const DEFAULT_TOTAL_READ_TIMEOUT_MS: i64 = 10_000;
pub const DEFAULT_MAXIMUM_CONNECTIONS: usize = 16;

/// Allowed `Host` header policy derived from the server bind address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliLocalHttpAllowedHosts {
    LoopbackOnly,
    LoopbackAnd(BTreeSet<String>),
    Any,
}

impl CliLocalHttpAllowedHosts {
    #[must_use]
    pub fn loopback_and<I, S>(hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::LoopbackAnd(hosts.into_iter().map(Into::into).collect())
    }

    #[must_use]
    pub fn allows(&self, raw_host_header: &str) -> bool {
        let Some(host) = Self::host_without_port(raw_host_header) else {
            return false;
        };
        let is_loopback = matches!(
            host.as_str(),
            "127.0.0.1"
                | "localhost"
                | "localhost."
                | "::1"
                | "[::1]"
                | "0:0:0:0:0:0:0:1"
                | "[0:0:0:0:0:0:0:1]"
        );
        match self {
            Self::LoopbackOnly => is_loopback,
            Self::LoopbackAnd(extra) => {
                if is_loopback || extra.contains(&host) {
                    return true;
                }
                let unbracketed = host
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or(host.as_str());
                let bracketed = format!("[{unbracketed}]");
                extra.contains(unbracketed) || extra.contains(&bracketed)
            }
            Self::Any => true,
        }
    }

    #[must_use]
    pub fn host_without_port(raw_host_header: &str) -> Option<String> {
        let trimmed = raw_host_header.trim();
        if trimmed.is_empty()
            || trimmed.contains(',')
            || trimmed.bytes().any(|b| b.is_ascii_whitespace())
        {
            return None;
        }
        if trimmed.starts_with('[') {
            let close_idx = trimmed.find(']')?;
            let inside = &trimmed[1..close_idx];
            if inside.is_empty() {
                return None;
            }
            let remainder = &trimmed[close_idx + 1..];
            if !remainder.is_empty() {
                let port_str = remainder.strip_prefix(':')?;
                if !Self::is_valid_port(port_str) {
                    return None;
                }
            }
            return Some(trimmed[..=close_idx].to_ascii_lowercase());
        }

        let parts: Vec<&str> = trimmed.split(':').collect();
        if parts.len() > 2 {
            return None;
        }
        let host = parts[0];
        if host.is_empty() {
            return None;
        }
        if parts.len() == 2 && !Self::is_valid_port(parts[1]) {
            return None;
        }
        Some(host.to_ascii_lowercase())
    }

    fn is_valid_port(raw_port: &str) -> bool {
        if raw_port.is_empty() || !raw_port.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        matches!(raw_port.parse::<u16>(), Ok(1..=u16::MAX))
    }
}

/// Request parsing failure classification (`CLILocalHTTPRequestParseError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CliLocalHttpRequestError {
    #[error("invalid HTTP request")]
    InvalidRequest,
    #[error("missing Host header")]
    MissingHost,
    #[error("duplicate Host header")]
    DuplicateHost,
    #[error("disallowed Host header")]
    DisallowedHost,
    #[error("duplicate Authorization header")]
    DuplicateAuthorization,
}

impl CliLocalHttpRequestError {
    #[allow(non_upper_case_globals)]
    pub const MalformedRequest: Self = Self::InvalidRequest;
}

/// Parsed HTTP/1.x request passed to the `serve` router.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliLocalHttpRequest {
    pub method: String,
    pub path: String,
    pub target: String,
    pub query_items: Vec<(String, Option<String>)>,
    pub headers: BTreeMap<String, String>,
}

impl CliLocalHttpRequest {
    pub fn parse(
        data: &[u8],
        allowed_hosts: &CliLocalHttpAllowedHosts,
    ) -> Result<Self, CliLocalHttpRequestError> {
        if data.len() > MAXIMUM_REQUEST_BYTES {
            return Err(CliLocalHttpRequestError::InvalidRequest);
        }
        let text =
            std::str::from_utf8(data).map_err(|_| CliLocalHttpRequestError::InvalidRequest)?;
        let header_block = if let Some((block, _)) = text.split_once("\r\n\r\n") {
            block
        } else if let Some((block, _)) = text.split_once("\n\n") {
            block
        } else {
            text
        };

        let mut lines: Vec<&str> = if header_block.contains("\r\n") {
            header_block.split("\r\n").collect()
        } else {
            header_block.split('\n').collect()
        };
        if lines.is_empty() {
            return Err(CliLocalHttpRequestError::InvalidRequest);
        }
        let request_line = lines.remove(0);
        if request_line.is_empty() {
            return Err(CliLocalHttpRequestError::InvalidRequest);
        }
        let parts: Vec<&str> = request_line.split(' ').collect();
        if parts.len() != 3 {
            return Err(CliLocalHttpRequestError::InvalidRequest);
        }
        let method = parts[0];
        let raw_target = parts[1];
        let version = parts[2];
        if method.is_empty()
            || !raw_target.starts_with('/')
            || (version != "HTTP/1.1" && version != "HTTP/1.0")
        {
            return Err(CliLocalHttpRequestError::InvalidRequest);
        }
        if raw_target
            .bytes()
            .any(|b| b <= 0x20 || b == 0x7f || b == b'#')
        {
            return Err(CliLocalHttpRequestError::InvalidRequest);
        }

        let mut headers = BTreeMap::new();
        let mut host_count = 0usize;
        let mut authorization_count = 0usize;
        for line in lines {
            if line.is_empty() {
                break;
            }
            if line.starts_with(' ') || line.starts_with('\t') {
                return Err(CliLocalHttpRequestError::InvalidRequest);
            }
            let Some((raw_name, raw_value)) = line.split_once(':') else {
                return Err(CliLocalHttpRequestError::InvalidRequest);
            };
            if raw_name.is_empty() || raw_name.bytes().any(|b| b.is_ascii_whitespace()) {
                return Err(CliLocalHttpRequestError::InvalidRequest);
            }
            let name = raw_name.to_ascii_lowercase();
            let value = raw_value.trim().to_string();
            if name == "host" {
                host_count += 1;
                if host_count > 1 {
                    return Err(CliLocalHttpRequestError::DuplicateHost);
                }
            } else if name == "authorization" {
                authorization_count += 1;
                if authorization_count > 1 {
                    return Err(CliLocalHttpRequestError::DuplicateAuthorization);
                }
            }
            headers.insert(name, value);
        }

        if host_count == 0 {
            return Err(CliLocalHttpRequestError::MissingHost);
        }
        let Some(host_header) = headers.get("host") else {
            return Err(CliLocalHttpRequestError::MissingHost);
        };
        if !allowed_hosts.allows(host_header) {
            return Err(CliLocalHttpRequestError::DisallowedHost);
        }

        let (raw_path, raw_query) = match raw_target.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (raw_target, None),
        };
        let path =
            Self::percent_decode(raw_path).ok_or(CliLocalHttpRequestError::InvalidRequest)?;
        if !path.starts_with('/') {
            return Err(CliLocalHttpRequestError::InvalidRequest);
        }

        let mut query_items = Vec::new();
        if let Some(query) = raw_query
            && !query.is_empty()
        {
            for segment in query.split('&') {
                if let Some((raw_k, raw_v)) = segment.split_once('=') {
                    let k = Self::percent_decode(raw_k)
                        .ok_or(CliLocalHttpRequestError::InvalidRequest)?;
                    let v = Self::percent_decode(raw_v)
                        .ok_or(CliLocalHttpRequestError::InvalidRequest)?;
                    query_items.push((k, Some(v)));
                } else {
                    let k = Self::percent_decode(segment)
                        .ok_or(CliLocalHttpRequestError::InvalidRequest)?;
                    query_items.push((k, None));
                }
            }
        }

        Ok(Self {
            method: method.to_ascii_uppercase(),
            path,
            target: raw_target.to_string(),
            query_items,
            headers,
        })
    }

    #[must_use]
    pub fn host(&self) -> &str {
        self.headers.get("host").map_or("", String::as_str)
    }

    #[must_use]
    pub fn authorization(&self) -> Option<&str> {
        self.headers.get("authorization").map(String::as_str)
    }

    #[must_use]
    pub fn query_value(&self, name: &str) -> Option<&str> {
        self.query_items
            .iter()
            .find_map(|(k, v)| (k == name).then_some(v.as_deref()).flatten())
    }

    #[must_use]
    pub fn query_values(&self, name: &str) -> Vec<&str> {
        self.query_items
            .iter()
            .filter_map(|(k, v)| (k == name).then_some(v.as_deref()).flatten())
            .collect()
    }

    fn percent_decode(input: &str) -> Option<String> {
        let bytes = input.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                if i + 2 >= bytes.len() {
                    return None;
                }
                let hi = Self::hex_val(bytes[i + 1])?;
                let lo = Self::hex_val(bytes[i + 2])?;
                let decoded = (hi << 4) | lo;
                if decoded == 0 {
                    return None;
                }
                out.push(decoded);
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).ok()
    }

    fn hex_val(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
}

/// Serialized HTTP/1.1 response returned by `serve` routes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliLocalHttpResponse {
    pub status_code: u16,
    pub reason_phrase: String,
    pub content_type: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    pub usage_cache_keys: Option<Vec<Option<String>>>,
}

impl CliLocalHttpResponse {
    #[must_use]
    pub fn json(status_code: u16, reason_phrase: &str, body: Vec<u8>) -> Self {
        Self {
            status_code,
            reason_phrase: reason_phrase.to_string(),
            content_type: "application/json; charset=utf-8".to_string(),
            headers: BTreeMap::new(),
            body,
            usage_cache_keys: None,
        }
    }

    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    #[must_use]
    pub fn with_headers(mut self, headers: BTreeMap<String, String>) -> Self {
        self.headers.extend(headers);
        self
    }

    #[must_use]
    pub fn with_usage_cache_keys(mut self, keys: Option<Vec<Option<String>>>) -> Self {
        self.usage_cache_keys = keys;
        self
    }

    #[must_use]
    pub fn bad_request() -> Self {
        Self::json(
            400,
            "Bad Request",
            br#"{"error":"invalid request"}"#.to_vec(),
        )
        .with_header("Cache-Control", "no-store")
    }

    #[must_use]
    pub fn forbidden_host() -> Self {
        Self::json(403, "Forbidden", br#"{"error":"forbidden host"}"#.to_vec())
            .with_header("Cache-Control", "no-store")
    }

    #[must_use]
    pub fn serialized(&self) -> Vec<u8> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n",
            self.status_code,
            self.reason_phrase,
            self.content_type,
            self.body.len()
        );
        for (name, value) in &self.headers {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        let mut out = head.into_bytes();
        out.extend_from_slice(&self.body);
        out
    }
}

/// Pre-auth connection concurrency limiter (`CLILocalHTTPConnectionGate`).
#[derive(Debug, Clone)]
pub struct CliLocalHttpConnectionGate {
    maximum: usize,
    active: Arc<Mutex<usize>>,
}

impl CliLocalHttpConnectionGate {
    #[must_use]
    pub fn new(maximum: usize) -> Self {
        Self {
            maximum: maximum.max(1),
            active: Arc::new(Mutex::new(0)),
        }
    }

    #[must_use]
    pub fn try_acquire(&self) -> bool {
        let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if *guard >= self.maximum {
            return false;
        }
        *guard += 1;
        true
    }

    #[must_use]
    pub fn try_acquire_permit(&self) -> Option<CliLocalHttpConnectionPermit> {
        self.try_acquire()
            .then(|| CliLocalHttpConnectionPermit { gate: self.clone() })
    }

    pub fn release(&self) {
        let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        *guard = guard.saturating_sub(1);
    }

    #[must_use]
    pub fn active_count(&self) -> usize {
        *self.active.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// RAII permit that releases a [`CliLocalHttpConnectionGate`] slot when dropped.
#[derive(Debug)]
pub struct CliLocalHttpConnectionPermit {
    gate: CliLocalHttpConnectionGate,
}

impl Drop for CliLocalHttpConnectionPermit {
    fn drop(&mut self) {
        self.gate.release();
    }
}

/// Computes the next chunk read timeout in milliseconds given the remaining total
/// header-read budget and the per-read timeout.
#[must_use]
pub fn header_read_chunk_timeout_ms(
    remaining_budget_ms: Option<i64>,
    per_read_timeout_ms: i64,
) -> Option<i64> {
    let clamped_per_read = per_read_timeout_ms.max(1);
    let Some(remaining_budget_ms) = remaining_budget_ms else {
        return Some(clamped_per_read);
    };
    if remaining_budget_ms <= 0 {
        return None;
    }
    Some(clamped_per_read.min(remaining_budget_ms))
}

pub type ServeRequestHandler = Arc<
    dyn Fn(CliLocalHttpRequest) -> Pin<Box<dyn Future<Output = CliLocalHttpResponse> + Send>>
        + Send
        + Sync,
>;

/// Configuration and handle for a running `CliLocalHttpServer`.
pub struct CliLocalHttpServer {
    listener: TcpListener,
    pub port: u16,
    allowed_hosts: CliLocalHttpAllowedHosts,
    read_timeout_ms: i64,
    total_read_timeout_ms: i64,
    connection_gate: CliLocalHttpConnectionGate,
}

impl CliLocalHttpServer {
    pub async fn bind(
        host: &str,
        port: u16,
        read_timeout_ms: i64,
        total_read_timeout_ms: i64,
        maximum_connections: usize,
    ) -> Result<Self, String> {
        let normalized_host = CliServeSecurity::bind_host(Some(host));
        if !CliServeSecurity::is_supported_ipv4_bind_host(&normalized_host) {
            return Err(format!(
                "Invalid --host '{normalized_host}'. Use 127.0.0.1, localhost, or an IPv4 address."
            ));
        }
        let addr_str = format!("{normalized_host}:{port}");
        let addr: SocketAddr = addr_str.parse().map_err(|_| {
            format!(
                "Invalid --host '{normalized_host}'. Use 127.0.0.1, localhost, or an IPv4 address."
            )
        })?;
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|err| format!("Failed to bind {normalized_host}:{port}: {err}"))?;
        let local_port = listener
            .local_addr()
            .map_err(|err| format!("Failed to query local address: {err}"))?
            .port();
        let allowed_hosts = CliServeSecurity::allowed_hosts(&normalized_host);
        Ok(Self {
            listener,
            port: local_port,
            allowed_hosts,
            read_timeout_ms: read_timeout_ms.max(1),
            total_read_timeout_ms: total_read_timeout_ms.max(1),
            connection_gate: CliLocalHttpConnectionGate::new(maximum_connections),
        })
    }

    #[must_use]
    pub fn with_allowed_hosts(mut self, allowed_hosts: CliLocalHttpAllowedHosts) -> Self {
        self.allowed_hosts = allowed_hosts;
        self
    }

    pub async fn serve(self, mut shutdown_rx: watch::Receiver<bool>, handler: ServeRequestHandler) {
        let app = Self::build_axum_router(handler);
        let tower_service = ServiceBuilder::new().service(app);

        loop {
            tokio::select! {
                biased;
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
                accepted = self.listener.accept() => {
                    let Ok((stream, _peer)) = accepted else {
                        continue;
                    };
                    let Some(permit) = self.connection_gate.try_acquire_permit() else {
                        drop(stream);
                        continue;
                    };
                    let allowed_hosts = self.allowed_hosts.clone();
                    let read_timeout_ms = self.read_timeout_ms;
                    let total_read_timeout_ms = self.total_read_timeout_ms;
                    let hyper_service = TowerToHyperService::new(tower_service.clone());
                    tokio::spawn(async move {
                        let _permit = permit;
                        let _ = Self::handle_connection(
                            stream,
                            allowed_hosts,
                            read_timeout_ms,
                            total_read_timeout_ms,
                            hyper_service,
                        )
                        .await;
                    });
                }
            }
        }
    }

    fn build_axum_router(handler: ServeRequestHandler) -> Router {
        Router::new().fallback(move |req: Request<Body>| {
            let handler = Arc::clone(&handler);
            async move {
                let parsed = req
                    .extensions()
                    .get::<CliLocalHttpRequest>()
                    .cloned()
                    .unwrap_or_else(|| CliLocalHttpRequest {
                        method: req.method().to_string(),
                        path: req.uri().path().to_string(),
                        target: req.uri().to_string(),
                        query_items: Vec::new(),
                        headers: BTreeMap::new(),
                    });
                let resp = handler(parsed).await;
                Self::into_http_response(resp)
            }
        })
    }

    fn into_http_response(resp: CliLocalHttpResponse) -> Response<Body> {
        let status = StatusCode::from_u16(resp.status_code).unwrap_or(StatusCode::OK);
        let mut builder = Response::builder().status(status);
        if let Ok(ct) = HeaderValue::from_str(&resp.content_type) {
            builder = builder.header(http::header::CONTENT_TYPE, ct);
        }
        for (name, value) in &resp.headers {
            if let (Ok(hn), Ok(hv)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                builder = builder.header(hn, hv);
            }
        }
        let mut http_resp = builder
            .body(Body::from(resp.body))
            .unwrap_or_else(|_| Response::new(Body::empty()));
        http_resp
            .extensions_mut()
            .insert(ReasonPhraseExt(resp.reason_phrase, resp.usage_cache_keys));
        http_resp
    }

    async fn handle_connection(
        mut stream: TcpStream,
        allowed_hosts: CliLocalHttpAllowedHosts,
        read_timeout_ms: i64,
        total_read_timeout_ms: i64,
        hyper_service: TowerToHyperService<Router>,
    ) -> std::io::Result<()> {
        let _ = stream.set_nodelay(true);
        let response =
            match Self::read_request_headers(&mut stream, read_timeout_ms, total_read_timeout_ms)
                .await
            {
                Some(raw_bytes) => match CliLocalHttpRequest::parse(&raw_bytes, &allowed_hosts) {
                    Ok(parsed) => Self::dispatch_through_hyper(parsed, hyper_service).await,
                    Err(CliLocalHttpRequestError::DisallowedHost) => {
                        CliLocalHttpResponse::forbidden_host()
                    }
                    Err(_) => CliLocalHttpResponse::bad_request(),
                },
                None => CliLocalHttpResponse::bad_request(),
            };
        let bytes = response.serialized();
        let _ = stream.write_all(&bytes).await;
        let _ = stream.shutdown().await;
        Ok(())
    }

    async fn dispatch_through_hyper(
        parsed: CliLocalHttpRequest,
        hyper_service: TowerToHyperService<Router>,
    ) -> CliLocalHttpResponse {
        let mut req_builder = Request::builder()
            .method(parsed.method.as_str())
            .uri(parsed.target.as_str());
        for (k, v) in &parsed.headers {
            if let (Ok(hn), Ok(hv)) = (
                HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                req_builder = req_builder.header(hn, hv);
            }
        }
        let Ok(mut req) = req_builder.body(Body::empty()) else {
            return CliLocalHttpResponse::bad_request();
        };
        req.extensions_mut().insert(parsed);

        let resp = match hyper_service.call(req).await {
            Ok(resp) => resp,
            Err(never) => match never {},
        };
        let status_code = resp.status().as_u16();
        let ext = resp.extensions().get::<ReasonPhraseExt>().cloned();
        let reason_phrase = ext
            .as_ref()
            .map(|r| r.0.clone())
            .or_else(|| resp.status().canonical_reason().map(ToString::to_string))
            .unwrap_or_else(|| "OK".to_string());
        let usage_cache_keys = ext.and_then(|r| r.1);
        let content_type = resp
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json; charset=utf-8")
            .to_string();
        let mut headers = BTreeMap::new();
        for (name, value) in resp.headers() {
            if name == http::header::CONTENT_TYPE || name == http::header::CONTENT_LENGTH {
                continue;
            }
            if let Ok(val_str) = value.to_str() {
                headers.insert(canonical_header_name(name.as_str()), val_str.to_string());
            }
        }
        let body = match resp.into_body().collect().await {
            Ok(collected) => collected.to_bytes().to_vec(),
            Err(_) => Vec::new(),
        };
        CliLocalHttpResponse {
            status_code,
            reason_phrase,
            content_type,
            headers,
            body,
            usage_cache_keys,
        }
    }

    async fn read_request_headers(
        stream: &mut TcpStream,
        read_timeout_ms: i64,
        total_read_timeout_ms: i64,
    ) -> Option<Vec<u8>> {
        let start = Instant::now();
        let mut buffer = Vec::with_capacity(1024);
        let mut chunk = [0u8; 4096];

        loop {
            let elapsed_ms = i64::try_from(start.elapsed().as_millis()).unwrap_or(i64::MAX);
            let remaining_ms = total_read_timeout_ms.saturating_sub(elapsed_ms);
            let timeout_ms = header_read_chunk_timeout_ms(Some(remaining_ms), read_timeout_ms)?;
            let read_res = tokio::time::timeout(
                Duration::from_millis(u64::try_from(timeout_ms).unwrap_or(1)),
                stream.read(&mut chunk),
            )
            .await;

            let Ok(Ok(n)) = read_res else {
                return None;
            };
            if n == 0 {
                return None;
            }
            buffer.extend_from_slice(&chunk[..n]);
            if buffer.len() > MAXIMUM_REQUEST_BYTES {
                return None;
            }
            if contains_header_terminator(&buffer) {
                return Some(buffer);
            }
        }
    }
}

#[derive(Debug, Clone)]
struct ReasonPhraseExt(String, Option<Vec<Option<String>>>);

fn contains_header_terminator(buf: &[u8]) -> bool {
    buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.windows(2).any(|w| w == b"\n\n")
}

fn canonical_header_name(lower: &str) -> String {
    match lower {
        "cache-control" => "Cache-Control".to_string(),
        "content-security-policy" => "Content-Security-Policy".to_string(),
        "www-authenticate" => "WWW-Authenticate".to_string(),
        "content-type" => "Content-Type".to_string(),
        "content-length" => "Content-Length".to_string(),
        other => other.to_string(),
    }
}
