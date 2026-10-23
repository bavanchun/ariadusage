// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexOAuth/CodexOAuthCredentials.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexOAuth/CodexTokenRefresher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/CodexHomeScope.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/CodexCredentialFileAccess.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_protocol::secret::SecretString;
use jiff::Timestamp;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Method, Url};
use serde_json::{Map, Value};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::brokers::call::BrokerCall;
use crate::brokers::credential_file::{self, CredentialDecl, CredentialFileError};
use crate::brokers::net::{DeclaredOrigins, NetBody, NetBroker, NetError, NetRequest};
#[cfg(any(test, feature = "test-hooks"))]
use crate::private_file::{WriteHooks, write_private};

pub const CODEX_HOME_ENV_KEY: &str = "CODEX_HOME";
pub const XDG_DATA_HOME_ENV_KEY: &str = "XDG_DATA_HOME";
pub const DEFAULT_REFRESH_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
pub const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodexOAuthCredentialSource {
    CodexHome,
    LegacyCodexHome,
    OpenCode,
}

impl CodexOAuthCredentialSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodexHome => "codex-home",
            Self::LegacyCodexHome => "legacy-codex-home",
            Self::OpenCode => "opencode",
        }
    }

    pub const fn allows_refresh(self) -> bool {
        false
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct CodexOAuthCredentials {
    pub access_token: SecretString,
    pub refresh_token: SecretString,
    pub id_token: Option<SecretString>,
    pub account_id: Option<String>,
    pub last_refresh: Option<Timestamp>,
    pub expires_at: Option<Timestamp>,
    pub source: CodexOAuthCredentialSource,
    pub is_api_key: bool,
}

impl fmt::Debug for CodexOAuthCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexOAuthCredentials")
            .field("access_token", &"[redacted]")
            .field("refresh_token", &"[redacted]")
            .field("has_id_token", &self.id_token.is_some())
            .field("has_account_id", &self.account_id.is_some())
            .field("last_refresh", &self.last_refresh)
            .field("expires_at", &self.expires_at)
            .field("source", &self.source)
            .field("is_api_key", &self.is_api_key)
            .finish()
    }
}

impl CodexOAuthCredentials {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        access_token: impl Into<SecretString>,
        refresh_token: impl Into<SecretString>,
        id_token: Option<SecretString>,
        account_id: Option<String>,
        last_refresh: Option<Timestamp>,
        expires_at: Option<Timestamp>,
        source: CodexOAuthCredentialSource,
        is_api_key: bool,
    ) -> Self {
        Self {
            access_token: access_token.into(),
            refresh_token: refresh_token.into(),
            id_token,
            account_id,
            last_refresh,
            expires_at,
            source,
            is_api_key,
        }
    }

    pub fn access_token_str(&self) -> &str {
        self.access_token.expose_secret()
    }

    pub fn refresh_token_str(&self) -> &str {
        self.refresh_token.expose_secret()
    }

    pub fn id_token_str(&self) -> Option<&str> {
        self.id_token.as_ref().map(SecretString::expose_secret)
    }

    pub fn needs_refresh(&self) -> bool {
        self.needs_refresh_at(Timestamp::now())
    }

    pub fn needs_refresh_at(&self, now: Timestamp) -> bool {
        if self.is_api_key {
            return false;
        }
        let refresh_window_ms: i64 = match self.source {
            CodexOAuthCredentialSource::CodexHome => 5 * 60 * 1000,
            CodexOAuthCredentialSource::LegacyCodexHome | CodexOAuthCredentialSource::OpenCode => {
                60 * 1000
            }
        };
        if let Some(expires_at) = self.expires_at {
            return (expires_at.as_millisecond() - now.as_millisecond()) <= refresh_window_ms;
        }
        let Some(last_refresh) = self.last_refresh else {
            return true;
        };
        let eight_days_ms: i64 = 8 * 24 * 60 * 60 * 1000;
        (now.as_millisecond() - last_refresh.as_millisecond()) > eight_days_ms
    }

    pub fn with_account_id(&self, account_id: Option<String>) -> Self {
        Self {
            access_token: self.access_token.clone(),
            refresh_token: self.refresh_token.clone(),
            id_token: self.id_token.clone(),
            account_id,
            last_refresh: self.last_refresh,
            expires_at: self.expires_at,
            source: self.source,
            is_api_key: self.is_api_key,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CodexOAuthCredentialsError {
    #[error("Codex OAuth credentials not found. Run `codex` to authenticate.")]
    NotFound,
    #[error("Failed to decode Codex OAuth credentials: {0}")]
    DecodeFailed(String),
    #[error("Codex OAuth credentials are missing tokens.")]
    MissingTokens,
    #[error(
        "Codex OAuth credentials in the active Codex home expired. Run `codex` or `codex login` to refresh them."
    )]
    NativeRefreshRequired,
    #[error(
        "External Codex OAuth credentials expired. Sign in with `codex login` to refresh them."
    )]
    ReadOnlySource,
    #[error("Codex OAuth credentials could not be read. Check `~/.codex/auth.json` permissions.")]
    Unreadable,
    #[error("Codex OAuth credentials could not be saved.")]
    WriteFailed,
    #[error("Codex OAuth credential read was cancelled.")]
    Cancelled,
}

pub type CredentialReadFn =
    Arc<dyn Fn(&Path) -> Result<Vec<u8>, CodexOAuthCredentialsError> + Send + Sync>;

pub struct CodexHomeScope;

impl CodexHomeScope {
    pub fn ambient_home_path(env: &BTreeMap<String, String>, home_directory: &Path) -> PathBuf {
        if let Some(raw) = env.get(CODEX_HOME_ENV_KEY) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
        home_directory.join(".codex")
    }

    pub fn ambient_home_url(
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
    ) -> Option<PathBuf> {
        if let Some(raw) = env.get(CODEX_HOME_ENV_KEY) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Some(PathBuf::from(trimmed));
            }
        }
        let home = home_dir
            .map(Path::to_path_buf)
            .or_else(|| env.get("HOME").map(PathBuf::from))?;
        Some(home.join(".codex"))
    }

    pub fn scoped_home_path(
        env: &BTreeMap<String, String>,
        codex_home_override: Option<&str>,
        home_directory: &Path,
    ) -> PathBuf {
        if let Some(raw) = codex_home_override {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
        Self::ambient_home_path(env, home_directory)
    }

    pub fn scoped_environment(
        base: &BTreeMap<String, String>,
        codex_home: Option<&str>,
    ) -> BTreeMap<String, String> {
        let Some(raw) = codex_home else {
            return base.clone();
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return base.clone();
        }
        let mut copy = base.clone();
        copy.insert(CODEX_HOME_ENV_KEY.to_owned(), trimmed.to_owned());
        copy
    }
}

pub fn parse_jwt_payload(token: &str) -> Option<Map<String, Value>> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload_part = parts.next()?;
    let bytes = decode_base64url(payload_part.as_bytes())?;
    match serde_json::from_slice::<Value>(&bytes).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn decode_base64url(input: &[u8]) -> Option<Vec<u8>> {
    let mut filtered = Vec::with_capacity(input.len());
    for &b in input {
        if b == b'=' || b.is_ascii_whitespace() {
            continue;
        }
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        filtered.push(val);
    }
    if filtered.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(filtered.len() * 3 / 4);
    for chunk in filtered.chunks(4) {
        let b0 = chunk[0];
        let b1 = chunk[1];
        out.push((b0 << 2) | (b1 >> 4));
        if chunk.len() >= 3 {
            let b2 = chunk[2];
            out.push((b1 << 4) | (b2 >> 2));
            if chunk.len() == 4 {
                let b3 = chunk[3];
                out.push((b2 << 6) | b3);
            }
        }
    }
    Some(out)
}

pub struct CodexOAuthCredentialsStore;

impl CodexOAuthCredentialsStore {
    pub fn auth_file_path(
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
    ) -> Option<PathBuf> {
        Some(CodexHomeScope::ambient_home_url(env, home_dir)?.join("auth.json"))
    }

    pub fn legacy_auth_file_path(home_directory: &Path) -> PathBuf {
        home_directory
            .join(".config")
            .join("codex")
            .join("auth.json")
    }

    pub fn open_code_auth_file_path(
        env: &BTreeMap<String, String>,
        home_directory: &Path,
    ) -> PathBuf {
        if let Some(raw) = env.get(XDG_DATA_HOME_ENV_KEY) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join("opencode").join("auth.json");
            }
        }
        home_directory
            .join(".local")
            .join("share")
            .join("opencode")
            .join("auth.json")
    }

    fn resolve_home_dir(
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
    ) -> Option<PathBuf> {
        home_dir
            .map(Path::to_path_buf)
            .or_else(|| env.get("HOME").map(PathBuf::from))
    }

    pub fn load(
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        Self::load_with_reader_and_call(env, home_dir, None, None)
    }

    pub fn load_with_reader(
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        reader: Option<&CredentialReadFn>,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        Self::load_with_reader_and_call(env, home_dir, reader, None)
    }

    pub fn load_with_reader_and_call(
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        reader: Option<&CredentialReadFn>,
        call: Option<&BrokerCall>,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        let path =
            Self::auth_file_path(env, home_dir).ok_or(CodexOAuthCredentialsError::NotFound)?;
        let data = Self::read_file(&path, reader, call)?;
        Self::parse(&data, CodexOAuthCredentialSource::CodexHome)
    }

    pub fn load_oauth_tokens(
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        Self::load_oauth_tokens_with_reader_and_call(env, home_dir, None, None)
    }

    pub fn load_oauth_tokens_with_reader(
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        reader: Option<&CredentialReadFn>,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        Self::load_oauth_tokens_with_reader_and_call(env, home_dir, reader, None)
    }

    pub fn load_oauth_tokens_with_reader_and_call(
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
        reader: Option<&CredentialReadFn>,
        call: Option<&BrokerCall>,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        let path =
            Self::auth_file_path(env, home_dir).ok_or(CodexOAuthCredentialsError::NotFound)?;
        let data = Self::read_file(&path, reader, call)?;
        Self::parse_oauth_tokens(&data, CodexOAuthCredentialSource::CodexHome)
    }

    pub fn load_for_usage(
        env: &BTreeMap<String, String>,
        allow_external_sources: bool,
        home_dir: Option<&Path>,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        Self::load_for_usage_with_reader_and_call(env, allow_external_sources, home_dir, None, None)
    }

    pub fn load_for_usage_with_reader(
        env: &BTreeMap<String, String>,
        allow_external_sources: bool,
        home_dir: Option<&Path>,
        reader: Option<&CredentialReadFn>,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        Self::load_for_usage_with_reader_and_call(
            env,
            allow_external_sources,
            home_dir,
            reader,
            None,
        )
    }

    pub fn load_for_usage_with_reader_and_call(
        env: &BTreeMap<String, String>,
        allow_external_sources: bool,
        home_dir: Option<&Path>,
        reader: Option<&CredentialReadFn>,
        call: Option<&BrokerCall>,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        match Self::load_with_reader_and_call(env, home_dir, reader, call) {
            Ok(creds) => Ok(creds),
            Err(native_error) => {
                if !allow_external_sources
                    || Self::has_explicit_codex_home(env)
                    || !Self::should_try_external_fallback(&native_error)
                {
                    return Err(native_error);
                }
                let Some(home) = Self::resolve_home_dir(env, home_dir) else {
                    return Err(native_error);
                };
                match Self::load_legacy_codex_home_with_reader(&home, reader, call) {
                    Ok(legacy) => return Ok(legacy),
                    Err(CodexOAuthCredentialsError::Cancelled) => {
                        return Err(CodexOAuthCredentialsError::Cancelled);
                    }
                    Err(_) => {}
                }
                match Self::load_open_code_with_reader(env, &home, reader, call) {
                    Ok(open_code) => return Ok(open_code),
                    Err(CodexOAuthCredentialsError::Cancelled) => {
                        return Err(CodexOAuthCredentialsError::Cancelled);
                    }
                    Err(_) => {}
                }
                Err(native_error)
            }
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn save(
        credentials: &CodexOAuthCredentials,
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
    ) -> Result<(), CodexOAuthCredentialsError> {
        if credentials.source != CodexOAuthCredentialSource::CodexHome {
            return Err(CodexOAuthCredentialsError::ReadOnlySource);
        }
        let path =
            Self::auth_file_path(env, home_dir).ok_or(CodexOAuthCredentialsError::WriteFailed)?;
        let existing_bytes = Self::read_file(&path, None, None).ok();
        let mut json: Map<String, Value> = existing_bytes
            .as_deref()
            .and_then(|bytes| serde_json::from_slice(bytes).ok())
            .unwrap_or_default();

        let mut tokens = Map::new();
        tokens.insert(
            "access_token".to_owned(),
            Value::String(credentials.access_token_str().to_owned()),
        );
        tokens.insert(
            "refresh_token".to_owned(),
            Value::String(credentials.refresh_token_str().to_owned()),
        );
        if let Some(id_token) = credentials.id_token_str() {
            tokens.insert("id_token".to_owned(), Value::String(id_token.to_owned()));
        }
        if let Some(account_id) = &credentials.account_id {
            tokens.insert("account_id".to_owned(), Value::String(account_id.clone()));
        }
        json.insert("tokens".to_owned(), Value::Object(tokens));
        let last_refresh = credentials.last_refresh.unwrap_or_else(Timestamp::now);
        json.insert(
            "last_refresh".to_owned(),
            Value::String(last_refresh.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()),
        );

        let mut data = Zeroizing::new(
            serde_json::to_vec_pretty(&Value::Object(json))
                .map_err(|_| CodexOAuthCredentialsError::WriteFailed)?,
        );
        data.push(b'\n');
        write_private(&path, &data, &WriteHooks::default())
            .map_err(|_| CodexOAuthCredentialsError::WriteFailed)
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn write_private_file_for_testing(
        path: &Path,
        data: &[u8],
    ) -> Result<(), CodexOAuthCredentialsError> {
        write_private(path, data, &WriteHooks::default())
            .map_err(|_| CodexOAuthCredentialsError::WriteFailed)
    }

    pub fn parse(
        data: &[u8],
        source: CodexOAuthCredentialSource,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        let json = Self::decode_json_object(data)?;
        if let Some(api_key) = json
            .get("OPENAI_API_KEY")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Ok(CodexOAuthCredentials::new(
                api_key, "", None, None, None, None, source, true,
            ));
        }
        Self::parse_oauth_tokens_from_map(&json, source)
    }

    pub fn parse_oauth_tokens(
        data: &[u8],
        source: CodexOAuthCredentialSource,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        let json = Self::decode_json_object(data)?;
        Self::parse_oauth_tokens_from_map(&json, source)
    }

    pub fn parse_open_code(
        data: &[u8],
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        let json = Self::decode_json_object(data)?;
        let openai = json
            .get("openai")
            .and_then(Value::as_object)
            .ok_or(CodexOAuthCredentialsError::MissingTokens)?;
        if let Some(raw_type) = openai
            .get("type")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && !raw_type.eq_ignore_ascii_case("oauth")
        {
            return Err(CodexOAuthCredentialsError::MissingTokens);
        }

        let access_token =
            Self::first_non_empty_string(openai, &["access", "access_token", "accessToken"])
                .ok_or(CodexOAuthCredentialsError::MissingTokens)?;
        let refresh_token =
            Self::first_non_empty_string(openai, &["refresh", "refresh_token", "refreshToken"])
                .unwrap_or_default();
        let id_token = Self::first_non_empty_string(openai, &["id", "id_token", "idToken"]);
        let account_id = Self::first_non_empty_string(openai, &["accountId", "account_id"])
            .or_else(|| Self::extract_account_id(id_token.as_deref(), Some(&access_token)));
        let expires_at = Self::parse_epoch_millis(openai.get("expires"));

        Ok(CodexOAuthCredentials::new(
            access_token,
            refresh_token,
            id_token.map(SecretString::new),
            account_id,
            None,
            expires_at,
            CodexOAuthCredentialSource::OpenCode,
            false,
        ))
    }

    fn parse_oauth_tokens_from_map(
        json: &Map<String, Value>,
        source: CodexOAuthCredentialSource,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        let tokens = json
            .get("tokens")
            .and_then(Value::as_object)
            .ok_or(CodexOAuthCredentialsError::MissingTokens)?;
        let access_token = Self::first_non_empty_string(tokens, &["access_token", "accessToken"])
            .ok_or(CodexOAuthCredentialsError::MissingTokens)?;
        let refresh_token =
            Self::first_non_empty_string(tokens, &["refresh_token", "refreshToken"])
                .unwrap_or_default();
        let id_token = tokens
            .get("id_token")
            .or_else(|| tokens.get("idToken"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let account_id = Self::first_non_empty_string(tokens, &["account_id", "accountId"])
            .or_else(|| Self::extract_account_id(id_token.as_deref(), Some(&access_token)));
        let last_refresh =
            Self::parse_last_refresh(json.get("last_refresh").or_else(|| json.get("lastRefresh")));
        let expires_at = if source == CodexOAuthCredentialSource::CodexHome {
            ariadusage_core::jwt::exp_claim(&access_token)
                .and_then(|secs| Timestamp::from_second(secs).ok())
        } else {
            None
        };

        Ok(CodexOAuthCredentials::new(
            access_token,
            refresh_token,
            id_token.map(SecretString::new),
            account_id,
            last_refresh,
            expires_at,
            source,
            false,
        ))
    }

    fn load_legacy_codex_home_with_reader(
        home_directory: &Path,
        reader: Option<&CredentialReadFn>,
        call: Option<&BrokerCall>,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        let path = Self::legacy_auth_file_path(home_directory);
        let data = Self::read_file(&path, reader, call)?;
        Self::parse_oauth_tokens(&data, CodexOAuthCredentialSource::LegacyCodexHome)
    }

    fn load_open_code_with_reader(
        env: &BTreeMap<String, String>,
        home_directory: &Path,
        reader: Option<&CredentialReadFn>,
        call: Option<&BrokerCall>,
    ) -> Result<CodexOAuthCredentials, CodexOAuthCredentialsError> {
        let path = Self::open_code_auth_file_path(env, home_directory);
        let data = Self::read_file(&path, reader, call)?;
        Self::parse_open_code(&data)
    }

    fn read_file(
        path: &Path,
        reader: Option<&CredentialReadFn>,
        call: Option<&BrokerCall>,
    ) -> Result<Zeroizing<Vec<u8>>, CodexOAuthCredentialsError> {
        if let Some(call) = call
            && call.cancel.is_cancelled()
        {
            return Err(CodexOAuthCredentialsError::Cancelled);
        }
        if let Some(reader) = reader {
            return reader(path).map(Zeroizing::new);
        }
        let decl = CredentialDecl::new(path);
        let default_call;
        let effective_call = if let Some(call) = call {
            call
        } else {
            default_call = BrokerCall {
                interaction: FetchInteraction::Background,
                cancel: CancellationToken::new(),
                request_id: "codex-cred-read".to_owned(),
            };
            &default_call
        };
        match credential_file::read(&decl, effective_call) {
            Ok(read) => Ok(read.bytes),
            Err(CredentialFileError::NotFound) => Err(CodexOAuthCredentialsError::NotFound),
            Err(CredentialFileError::Cancelled) => Err(CodexOAuthCredentialsError::Cancelled),
            Err(
                CredentialFileError::Untrusted
                | CredentialFileError::Unreadable
                | CredentialFileError::TooLarge
                | CredentialFileError::Malformed,
            ) => Err(CodexOAuthCredentialsError::Unreadable),
        }
    }

    fn has_explicit_codex_home(env: &BTreeMap<String, String>) -> bool {
        env.get(CODEX_HOME_ENV_KEY)
            .map(String::as_str)
            .map(str::trim)
            .is_some_and(|s| !s.is_empty())
    }

    fn should_try_external_fallback(native_error: &CodexOAuthCredentialsError) -> bool {
        matches!(native_error, CodexOAuthCredentialsError::NotFound)
    }

    fn decode_json_object(data: &[u8]) -> Result<Map<String, Value>, CodexOAuthCredentialsError> {
        match serde_json::from_slice::<Value>(data) {
            Ok(Value::Object(map)) => Ok(map),
            Ok(_) => Err(CodexOAuthCredentialsError::DecodeFailed(
                "Invalid JSON".to_owned(),
            )),
            Err(err) => Err(CodexOAuthCredentialsError::DecodeFailed(err.to_string())),
        }
    }

    fn first_non_empty_string(map: &Map<String, Value>, keys: &[&str]) -> Option<String> {
        for key in keys {
            if let Some(raw) = map.get(*key).and_then(Value::as_str) {
                let trimmed = raw.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_owned());
                }
            }
        }
        None
    }

    fn parse_last_refresh(raw: Option<&Value>) -> Option<Timestamp> {
        let raw = raw?;
        if let Some(text) = raw.as_str() {
            return text.trim().parse::<Timestamp>().ok();
        }
        let secs = raw.as_f64()?;
        if !secs.is_finite() {
            return None;
        }
        let normalized_secs = if secs > 10_000_000_000.0 {
            secs / 1000.0
        } else {
            secs
        };
        let millis = (normalized_secs * 1000.0).round() as i64;
        Timestamp::from_millisecond(millis).ok()
    }

    fn parse_epoch_millis(raw: Option<&Value>) -> Option<Timestamp> {
        let raw = raw?;
        let millis = if let Some(val) = raw.as_f64() {
            if !val.is_finite() {
                return None;
            }
            val
        } else {
            let text = raw.as_str()?;
            let val = text.trim().parse::<f64>().ok()?;
            if !val.is_finite() {
                return None;
            }
            val
        };
        if millis <= 0.0 {
            return None;
        }
        Timestamp::from_millisecond(millis.round() as i64).ok()
    }

    pub fn extract_account_id(
        id_token: Option<&str>,
        access_token: Option<&str>,
    ) -> Option<String> {
        for token in [id_token, access_token].into_iter().flatten() {
            let Some(payload) = parse_jwt_payload(token) else {
                continue;
            };
            if let Some(account_id) = payload
                .get("chatgpt_account_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(account_id.to_owned());
            }
            if let Some(auth_obj) = payload
                .get("https://api.openai.com/auth")
                .and_then(Value::as_object)
                && let Some(account_id) = auth_obj
                    .get("chatgpt_account_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            {
                return Some(account_id.to_owned());
            }
            if let Some(orgs) = payload.get("organizations").and_then(Value::as_array) {
                for org in orgs {
                    if let Some(org_id) = org
                        .as_object()
                        .and_then(|o| o.get("id"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        return Some(org_id.to_owned());
                    }
                }
            }
        }
        None
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CodexRefreshError {
    #[error("Refresh token has expired. Please run `codex` to log in again.")]
    Expired,
    #[error("Refresh token was revoked. Please run `codex` to log in again.")]
    Revoked,
    #[error("Refresh token was already used. Please run `codex` to log in again.")]
    Reused,
    #[error("Network error during token refresh: {0}")]
    NetworkError(String),
    #[error("Invalid refresh response: {0}")]
    InvalidResponse(String),
    #[error("Token refresh was cancelled.")]
    Cancelled,
}

pub struct CodexTokenRefresher;

impl CodexTokenRefresher {
    pub async fn refresh(
        net: &NetBroker,
        call: BrokerCall,
        credentials: &CodexOAuthCredentials,
        refresh_endpoint: &str,
    ) -> Result<CodexOAuthCredentials, CodexRefreshError> {
        if call.cancel.is_cancelled() {
            return Err(CodexRefreshError::Cancelled);
        }
        if credentials.is_api_key || credentials.refresh_token_str().is_empty() {
            return Ok(credentials.clone());
        }

        let url = Url::parse(refresh_endpoint)
            .map_err(|e| CodexRefreshError::InvalidResponse(e.to_string()))?;
        let declared_origins = DeclaredOrigins::from_urls([url.clone()])
            .map_err(|e| CodexRefreshError::InvalidResponse(e.to_string()))?;

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let body_json = serde_json::json!({
            "client_id": CODEX_OAUTH_CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": credentials.refresh_token_str(),
            "scope": "openid profile email",
        });
        let body_bytes = serde_json::to_vec(&body_json)
            .map_err(|e| CodexRefreshError::InvalidResponse(e.to_string()))?;

        let mut req = NetRequest::new(Method::POST, url);
        req.headers = headers;
        req.body = NetBody::secret(body_bytes);
        req.timeout = Some(Duration::from_secs(30));

        let response = net
            .send_response(&call, &declared_origins, req)
            .await
            .map_err(|err| match err {
                NetError::Cancelled => CodexRefreshError::Cancelled,
                other => CodexRefreshError::NetworkError(other.to_string()),
            })?;

        let status = response.status.as_u16();
        let bytes = response.body();
        if status == 401 {
            return Err(Self::parse_refresh_error(bytes).unwrap_or(CodexRefreshError::Expired));
        }
        if !(200..=299).contains(&status) {
            return Err(Self::refresh_failure_error(status, bytes));
        }

        let json: Map<String, Value> = match serde_json::from_slice::<Value>(bytes) {
            Ok(Value::Object(map)) => map,
            _ => {
                return Err(CodexRefreshError::InvalidResponse(
                    "Unable to decode response".to_owned(),
                ));
            }
        };

        let new_access_token = json
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| credentials.access_token_str());
        let new_refresh_token = json
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| credentials.refresh_token_str());
        let new_id_token = json
            .get("id_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(SecretString::new)
            .or_else(|| credentials.id_token.clone());
        let new_account_id = CodexOAuthCredentialsStore::extract_account_id(
            new_id_token.as_ref().map(SecretString::expose_secret),
            Some(new_access_token),
        )
        .or_else(|| credentials.account_id.clone());
        let expires_at = if credentials.source == CodexOAuthCredentialSource::CodexHome {
            ariadusage_core::jwt::exp_claim(new_access_token)
                .and_then(|secs| Timestamp::from_second(secs).ok())
        } else {
            credentials.expires_at
        };

        Ok(CodexOAuthCredentials::new(
            new_access_token,
            new_refresh_token,
            new_id_token,
            new_account_id,
            Some(Timestamp::now()),
            expires_at,
            credentials.source,
            false,
        ))
    }

    pub fn parse_refresh_error(bytes: &[u8]) -> Option<CodexRefreshError> {
        let json = serde_json::from_slice::<Value>(bytes).ok()?;
        let obj = json.as_object()?;
        let code = obj
            .get("error")
            .and_then(|v| {
                v.as_str().or_else(|| {
                    v.as_object()
                        .and_then(|e| e.get("code"))
                        .and_then(Value::as_str)
                })
            })
            .or_else(|| obj.get("code").and_then(Value::as_str))?;
        match code.to_lowercase().as_str() {
            "refresh_token_expired" => Some(CodexRefreshError::Expired),
            "refresh_token_reused" => Some(CodexRefreshError::Reused),
            "refresh_token_invalidated" | "invalid_grant" => Some(CodexRefreshError::Revoked),
            _ => None,
        }
    }

    fn refresh_failure_error(status: u16, bytes: &[u8]) -> CodexRefreshError {
        if let Some(err) = Self::parse_refresh_error(bytes) {
            return err;
        }
        CodexRefreshError::InvalidResponse(format!("HTTP {status}"))
    }

    pub fn refresh_failure_error_for_testing(status: u16, bytes: &[u8]) -> CodexRefreshError {
        Self::refresh_failure_error(status, bytes)
    }
}
