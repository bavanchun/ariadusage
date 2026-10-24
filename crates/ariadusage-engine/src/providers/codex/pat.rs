// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexPAT/CodexCLIUserAgent.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexPAT/CodexPATUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use ariadusage_protocol::secret::SecretString;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use reqwest::{Method, Url};
use serde::Deserialize;

use crate::brokers::call::BrokerCall;
use crate::brokers::net::{NetBroker, NetRequest, is_loopback_host};

use super::credentials::CodexPATCredentials;
use super::reconciled::CodexPATWhoami;
use super::usage_fetcher::{
    CHATGPT_USAGE_PATH, CODEX_USAGE_PATH, CodexOAuthFetchError, CodexOAuthUsageFetcher,
    CodexUsageResponse,
};

pub const DEFAULT_PAT_WHOAMI_URL: &str =
    "https://auth.openai.com/api/accounts/v1/user-auth-credential/whoami";
pub const PAT_WHOAMI_PATH: &str = "/api/accounts/v1/user-auth-credential/whoami";

pub struct CodexCLIUserAgent;

impl CodexCLIUserAgent {
    pub const ORIGINATOR: &'static str = "codex_cli_rs";

    pub fn make(cli_version: Option<&str>) -> String {
        let platform = Self::platform_name();
        let os_version = Self::operating_system_version_string();
        let architecture = Self::architecture();
        if let Some(version) = Self::normalized_cli_version(cli_version) {
            format!("codex_cli_rs/{version} ({platform} {os_version}; {architecture})")
        } else {
            format!("codex_cli_rs ({platform} {os_version}; {architecture})")
        }
    }

    pub fn normalized_cli_version(version_string: Option<&str>) -> Option<String> {
        let raw = version_string?.trim();
        if raw.is_empty() {
            return None;
        }
        let parts: Vec<&str> = raw.split_whitespace().collect();
        if parts.len() >= 2 && parts[0].eq_ignore_ascii_case("codex-cli") {
            let version = parts[1].trim();
            return (!version.is_empty()).then(|| version.to_owned());
        }
        let token = parts.first().copied().map(str::trim).unwrap_or(raw);
        (!token.is_empty()).then(|| token.to_owned())
    }

    const fn platform_name() -> &'static str {
        if cfg!(target_os = "macos") {
            "Mac OS"
        } else if cfg!(target_os = "linux") {
            "Linux"
        } else {
            "Darwin"
        }
    }

    fn operating_system_version_string() -> String {
        #[cfg(target_os = "linux")]
        {
            if let Ok(raw) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
                let prefix = raw
                    .trim()
                    .split(|c: char| !c.is_ascii_digit() && c != '.')
                    .next()
                    .unwrap_or("");
                let nums: Vec<u32> = prefix
                    .split('.')
                    .filter(|s| !s.is_empty())
                    .take(3)
                    .filter_map(|s| s.parse::<u32>().ok())
                    .collect();
                if !nums.is_empty() {
                    let major = nums.first().copied().unwrap_or(0);
                    let minor = nums.get(1).copied().unwrap_or(0);
                    let patch = nums.get(2).copied().unwrap_or(0);
                    return format!("{major}.{minor}.{patch}");
                }
            }
        }
        "0.0.0".to_owned()
    }

    const fn architecture() -> &'static str {
        if cfg!(target_arch = "aarch64") {
            "arm64"
        } else if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else {
            "unknown"
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexPATUsageFetch {
    pub usage: CodexUsageResponse,
    pub whoami: Option<CodexPATWhoami>,
}

#[derive(Deserialize)]
struct WhoamiWireResponse {
    #[serde(default)]
    chatgpt_account_id: Option<String>,
    #[serde(default)]
    chatgpt_plan_type: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

impl WhoamiWireResponse {
    fn into_model(self) -> CodexPATWhoami {
        CodexPATWhoami {
            account_id: non_empty(self.chatgpt_account_id),
            email: non_empty(self.email),
            plan_type: non_empty(self.chatgpt_plan_type),
        }
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    let trimmed = value?.trim().to_owned();
    (!trimmed.is_empty()).then_some(trimmed)
}

pub struct CodexPATUsageFetcher;

impl CodexPATUsageFetcher {
    pub fn user_agent_for_testing(cli_version: Option<&str>) -> String {
        CodexCLIUserAgent::make(cli_version)
    }

    pub fn normalized_cli_version_for_testing(version_string: Option<&str>) -> Option<String> {
        CodexCLIUserAgent::normalized_cli_version(version_string)
    }

    pub fn decode_whoami_response(data: &[u8]) -> Result<CodexPATWhoami, CodexOAuthFetchError> {
        let parsed: WhoamiWireResponse =
            serde_json::from_slice(data).map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
        Ok(parsed.into_model())
    }

    pub fn build_pat_request(
        url: Url,
        token: &str,
        user_agent: &str,
        account_id: Option<&str>,
    ) -> Result<NetRequest, CodexOAuthFetchError> {
        HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
        let ua_val =
            HeaderValue::from_str(user_agent).map_err(|_| CodexOAuthFetchError::InvalidResponse)?;

        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, ua_val);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            HeaderName::from_static("originator"),
            HeaderValue::from_static(CodexCLIUserAgent::ORIGINATOR),
        );
        if let Some(acct) = account_id.map(str::trim).filter(|s| !s.is_empty()) {
            let acct_val =
                HeaderValue::from_str(acct).map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
            headers.insert(HeaderName::from_static("chatgpt-account-id"), acct_val);
        }

        let mut req = NetRequest::new(Method::GET, url);
        req.headers = headers;
        req.credentials
            .push((AUTHORIZATION, SecretString::new(format!("Bearer {token}"))));
        req.timeout = Some(Duration::from_secs(30));
        Ok(req)
    }

    pub async fn fetch_usage(
        net: &NetBroker,
        credentials: &CodexPATCredentials,
        cli_version: Option<&str>,
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
    ) -> Result<CodexPATUsageFetch, CodexOAuthFetchError> {
        Self::fetch_usage_with_call(net, credentials, cli_version, env, home_dir, None).await
    }

    pub async fn fetch_usage_with_call(
        net: &NetBroker,
        credentials: &CodexPATCredentials,
        cli_version: Option<&str>,
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        call: Option<&BrokerCall>,
    ) -> Result<CodexPATUsageFetch, CodexOAuthFetchError> {
        let user_agent = CodexCLIUserAgent::make(cli_version);
        let whoami = Self::fetch_whoami_with_call(
            net,
            credentials.token_str(),
            &user_agent,
            env,
            home_dir,
            call,
        )
        .await?;
        // PAT identity comes from the token's whoami payload. A stale managed-workspace
        // ChatGPT-Account-Id would query the wrong account after CODEX_HOME fail-closes.
        let account_id = whoami
            .account_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let usage = Self::fetch_usage_endpoint_with_call(
            net,
            credentials.token_str(),
            account_id,
            &user_agent,
            env,
            home_dir,
            call,
        )
        .await?;
        Ok(CodexPATUsageFetch {
            usage,
            whoami: Some(whoami),
        })
    }

    async fn fetch_whoami_with_call(
        net: &NetBroker,
        token: &str,
        user_agent: &str,
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        call: Option<&BrokerCall>,
    ) -> Result<CodexPATWhoami, CodexOAuthFetchError> {
        let (whoami_url, validated_override) = Self::resolve_whoami_url(env, home_dir)?;
        let req = Self::build_pat_request(whoami_url, token, user_agent, None)?;
        let body =
            CodexOAuthUsageFetcher::perform_request(net, validated_override.as_ref(), req, call)
                .await?;
        Self::decode_whoami_response(&body)
    }

    #[allow(clippy::too_many_arguments)]
    async fn fetch_usage_endpoint_with_call(
        net: &NetBroker,
        token: &str,
        account_id: Option<&str>,
        user_agent: &str,
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        call: Option<&BrokerCall>,
    ) -> Result<CodexUsageResponse, CodexOAuthFetchError> {
        let (normalized, validated_override) =
            CodexOAuthUsageFetcher::resolve_validated_chatgpt_base_url(env, home_dir)?;
        let path = if normalized.contains("/backend-api") {
            CHATGPT_USAGE_PATH
        } else {
            CODEX_USAGE_PATH
        };
        let url = Url::parse(&format!("{normalized}{path}"))
            .map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
        let req = Self::build_pat_request(url, token, user_agent, account_id)?;
        let body =
            CodexOAuthUsageFetcher::perform_request(net, validated_override.as_ref(), req, call)
                .await?;
        CodexOAuthUsageFetcher::decode_usage_response(&body)
    }

    fn resolve_whoami_url(
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
    ) -> Result<(Url, Option<Url>), CodexOAuthFetchError> {
        let (_normalized, validated_override) =
            CodexOAuthUsageFetcher::resolve_validated_chatgpt_base_url(env, home_dir)?;
        if let Some(override_url) = validated_override
            && override_url.host_str().is_some_and(is_loopback_host)
        {
            let mut whoami_url = override_url.clone();
            whoami_url.set_path(PAT_WHOAMI_PATH);
            whoami_url.set_query(None);
            whoami_url.set_fragment(None);
            return Ok((whoami_url, Some(override_url)));
        }
        let default_url = Url::parse(DEFAULT_PAT_WHOAMI_URL)
            .map_err(|_| CodexOAuthFetchError::InvalidResponse)?;
        Ok((default_url, None))
    }
}
