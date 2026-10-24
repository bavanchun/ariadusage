// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityOAuthCredentialsStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_protocol::secret::SecretString;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use jiff::Timestamp;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::brokers::call::BrokerCall;
use crate::brokers::credential_file::{self, CredentialDecl, CredentialFileError};
use crate::private_file::{WriteHooks, write_private};

use super::remote_fetcher::AntigravityRemoteFetchError;

pub trait EnvValue {
    fn as_env_str(&self) -> &str;
}

impl EnvValue for String {
    fn as_env_str(&self) -> &str {
        self.as_str()
    }
}

impl EnvValue for &str {
    fn as_env_str(&self) -> &str {
        self
    }
}

impl EnvValue for SecretString {
    fn as_env_str(&self) -> &str {
        self.expose_secret()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct AntigravityOAuthCredentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expiry_date: Option<Timestamp>,
    pub id_token: Option<String>,
    pub email: Option<String>,
    pub project_id: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

impl fmt::Debug for AntigravityOAuthCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AntigravityOAuthCredentials")
            .field("has_access_token", &!self.access_token.is_empty())
            .field("has_refresh_token", &self.refresh_token.is_some())
            .field("expiry_date", &self.expiry_date)
            .field("has_id_token", &self.id_token.is_some())
            .field("has_email", &self.email.is_some())
            .field("has_project_id", &self.project_id.is_some())
            .field("has_client_id", &self.client_id.is_some())
            .field("has_client_secret", &self.client_secret.is_some())
            .finish()
    }
}

impl AntigravityOAuthCredentials {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        access_token: impl Into<String>,
        refresh_token: Option<String>,
        expiry_date: Option<Timestamp>,
        id_token: Option<String>,
        email: Option<String>,
        project_id: Option<String>,
        client_id: Option<String>,
        client_secret: Option<String>,
    ) -> Self {
        Self {
            access_token: access_token.into(),
            refresh_token,
            expiry_date,
            id_token,
            email,
            project_id,
            client_id,
            client_secret,
        }
    }

    pub fn needs_refresh(&self) -> bool {
        self.needs_refresh_at(Timestamp::now())
    }

    pub fn needs_refresh_at(&self, now: Timestamp) -> bool {
        let Some(expiry) = self.expiry_date else {
            return false;
        };
        let threshold = now
            .saturating_add(jiff::Span::new().seconds(60))
            .unwrap_or(now);
        threshold >= expiry
    }

    pub fn resolved_account_email(&self) -> Option<String> {
        AntigravityOAuthConfig::extract_email_from_id_token(self.id_token.as_deref()).or_else(
            || {
                self.email
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            },
        )
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AntigravityOAuthClient {
    pub client_id: String,
    pub client_secret: String,
}

impl fmt::Debug for AntigravityOAuthClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AntigravityOAuthClient")
            .field("has_client_id", &!self.client_id.is_empty())
            .field("has_client_secret", &!self.client_secret.is_empty())
            .finish()
    }
}

impl AntigravityOAuthClient {
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
        }
    }
}

pub struct AntigravityOAuthConfig;

const MAX_INSTALLED_ARTIFACT_BYTES: u64 = 128 * 1024 * 1024;
static INSTALLED_CLIENT_CACHE: OnceLock<Mutex<BTreeMap<Option<PathBuf>, AntigravityOAuthClient>>> =
    OnceLock::new();

impl AntigravityOAuthConfig {
    pub const DEFAULT_SCOPES: &[&str] = &[
        "https://www.googleapis.com/auth/cloud-platform",
        "https://www.googleapis.com/auth/userinfo.email",
    ];

    pub const CLIENT_ID_ENV_KEY: &str = "ANTIGRAVITY_OAUTH_CLIENT_ID";
    pub const CLIENT_SECRET_ENV_KEY: &str = "ANTIGRAVITY_OAUTH_CLIENT_SECRET";
    pub const CONSUMER_CLIENT_ID: &str =
        "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";

    const CANDIDATE_APP_NAMES: &[&str] = &["Gemini.app", "Antigravity.app"];
    const RELATIVE_CLIENT_CONFIG_PATHS: &[&str] = &[
        "Contents/Resources/app/extensions/antigravity/bin/language_server_macos_arm",
        "Contents/Resources/app/extensions/antigravity/bin/language_server_macos_x64",
        "Contents/Resources/app/extensions/antigravity/bin/language_server_macos",
        "Contents/Resources/app/out/main.js",
        "Contents/Resources/bin/language_server",
        "Contents/Resources/bin/language_server_macos",
        "Contents/Resources/app/extensions/antigravity/dist/extension.js",
        "resources/app/extensions/antigravity/bin/language_server_linux_x64",
        "resources/app/extensions/antigravity/bin/language_server_linux_arm",
        "resources/app/extensions/antigravity/bin/language_server_linux",
        "resources/app/out/main.js",
        "resources/app/extensions/antigravity/dist/extension.js",
        "Contents/MacOS/Antigravity",
        "Contents/MacOS/Gemini",
        "antigravity",
        "bin/antigravity",
    ];

    pub fn resolve_client_from_env(
        environment: &BTreeMap<String, String>,
    ) -> Option<AntigravityOAuthClient> {
        let client_id = environment
            .get(Self::CLIENT_ID_ENV_KEY)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        let client_secret = environment
            .get(Self::CLIENT_SECRET_ENV_KEY)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        if let (Some(id), Some(secret)) = (client_id, client_secret) {
            return Some(AntigravityOAuthClient::new(id, secret));
        }
        None
    }

    pub fn resolve_client(
        environment: &BTreeMap<String, String>,
        app_bundle_url: Option<&Path>,
        home_directory: Option<&Path>,
    ) -> Option<AntigravityOAuthClient> {
        if let Some(client) = Self::resolve_client_from_env(environment) {
            return Some(client);
        }

        Self::installed_client(app_bundle_url, home_directory)
    }

    pub fn installed_client_cached(
        home_directory: Option<&Path>,
    ) -> Option<AntigravityOAuthClient> {
        let key = home_directory.map(Path::to_path_buf);
        let cache = INSTALLED_CLIENT_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
        if let Ok(guard) = cache.lock()
            && let Some(cached) = guard.get(&key)
        {
            return Some(cached.clone());
        }
        let discovered = Self::installed_client_uncached(None, home_directory)?;
        if let Ok(mut guard) = cache.lock() {
            guard.insert(key, discovered.clone());
        }
        Some(discovered)
    }

    pub fn installed_client(
        app_bundle_url: Option<&Path>,
        home_directory: Option<&Path>,
    ) -> Option<AntigravityOAuthClient> {
        if app_bundle_url.is_none() && home_directory.is_none() {
            return Self::installed_client_cached(None);
        }
        Self::installed_client_uncached(app_bundle_url, home_directory)
    }

    fn installed_client_uncached(
        app_bundle_url: Option<&Path>,
        home_directory: Option<&Path>,
    ) -> Option<AntigravityOAuthClient> {
        let candidate_bundle_urls: Vec<PathBuf> = match app_bundle_url {
            Some(url) => {
                if !Self::is_allowed_explicit_bundle(url) {
                    return None;
                }
                vec![url.to_path_buf()]
            }
            None => {
                let mut roots = vec![PathBuf::from("/Applications")];
                if let Some(home) = home_directory
                    && !home.to_string_lossy().trim().is_empty()
                {
                    roots.push(home.join("Applications"));
                }
                let mut urls = Self::candidate_antigravity_app_bundle_urls(&roots);
                for install_url in Self::installed_app_urls(home_directory) {
                    if install_url.extension().and_then(|e| e.to_str()) != Some("app") {
                        urls.push(install_url);
                    }
                }
                urls
            }
        };

        for bundle_url in candidate_bundle_urls {
            for relative_path in Self::RELATIVE_CLIENT_CONFIG_PATHS {
                let candidate_url = bundle_url.join(relative_path);
                let Some(data) = Self::read_bounded_artifact(&candidate_url) else {
                    continue;
                };
                if let Some(client) = Self::parse_client_from_installed_artifact_data(&data) {
                    return Some(client);
                }
            }
        }
        None
    }

    pub fn discover_client_from_installed_app<P: AsRef<Path>>(
        application_roots: &[P],
    ) -> Option<AntigravityOAuthClient> {
        for candidate_url in Self::candidate_oauth_client_artifact_urls(application_roots) {
            let Some(data) = Self::read_bounded_artifact(&candidate_url) else {
                continue;
            };
            if let Some(client) = Self::parse_client_from_installed_artifact_data(&data) {
                return Some(client);
            }
        }
        None
    }

    pub fn candidate_oauth_client_artifact_urls<P: AsRef<Path>>(
        application_roots: &[P],
    ) -> Vec<PathBuf> {
        let app_bundle_urls = Self::candidate_antigravity_app_bundle_urls(application_roots);
        let mut urls = Vec::new();
        for bundle_url in app_bundle_urls {
            for relative_path in Self::RELATIVE_CLIENT_CONFIG_PATHS {
                urls.push(bundle_url.join(relative_path));
            }
        }
        urls
    }

    pub fn candidate_antigravity_app_bundle_urls<P: AsRef<Path>>(
        application_roots: &[P],
    ) -> Vec<PathBuf> {
        let mut urls = Vec::new();
        for root_ref in application_roots {
            let root = root_ref.as_ref();
            let antigravity_url = root.join("Antigravity.app");
            if Self::is_allowed_explicit_bundle(&antigravity_url) {
                urls.push(antigravity_url);
            }

            // "Gemini.app" is a generic name, so unlike "Antigravity.app" it is
            // only accepted when the bundle identifier confirms the renamed app.
            let gemini_url = root.join("Gemini.app");
            if Self::is_antigravity_app_bundle(&gemini_url) {
                urls.push(gemini_url);
            }

            if let Ok(entries) = std::fs::read_dir(root) {
                let mut app_paths: Vec<PathBuf> = entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|path| {
                        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                            return false;
                        };
                        !name.starts_with('.')
                            && path.extension().and_then(|ext| ext.to_str()) == Some("app")
                    })
                    .collect();
                app_paths.sort();
                for app_url in app_paths {
                    if Self::is_antigravity_app_bundle(&app_url) {
                        urls.push(app_url);
                    }
                }
            }
        }

        let mut seen = BTreeSet::new();
        let mut deduped = Vec::new();
        for url in urls {
            let clean = normalize_lexical_path(&url);
            let key = clean.to_string_lossy().into_owned();
            if seen.insert(key) {
                deduped.push(clean);
            }
        }
        deduped
    }

    pub fn is_antigravity_app_bundle(bundle_url: &Path) -> bool {
        Self::bundle_identifier(bundle_url)
            .is_some_and(|id| Self::is_known_bundle_identifier(&id))
    }

    pub fn is_known_bundle_identifier(bundle_id: &str) -> bool {
        matches!(
            bundle_id.trim(),
            "com.google.antigravity"
                | "com.google.antigravity-ide"
                | "com.google.GeminiMacOS"
        )
    }

    pub fn bundle_identifier(bundle_url: &Path) -> Option<String> {
        let info_plist_path = bundle_url.join("Contents").join("Info.plist");
        let data = Self::read_bounded_artifact(&info_plist_path)?;
        let text = std::str::from_utf8(&data).ok()?;
        Self::parse_bundle_identifier_from_plist_xml(text)
    }

    fn parse_bundle_identifier_from_plist_xml(xml: &str) -> Option<String> {
        let mut rest = xml;
        while let Some(key_start) = rest.find("<key>") {
            let after_key_open = &rest[key_start + "<key>".len()..];
            let key_end = after_key_open.find("</key>")?;
            let key_name = after_key_open[..key_end].trim();
            let after_key_close = &after_key_open[key_end + "</key>".len()..];
            if key_name == "CFBundleIdentifier" {
                let trimmed = after_key_close.trim_start();
                let after_string_open = trimmed.strip_prefix("<string>")?;
                let string_end = after_string_open.find("</string>")?;
                let value = after_string_open[..string_end].trim();
                if value.is_empty() {
                    return None;
                }
                return Some(value.to_owned());
            }
            rest = after_key_close;
        }
        None
    }

    fn is_allowed_explicit_bundle(bundle_url: &Path) -> bool {
        if bundle_url.file_name().and_then(|n| n.to_str()) == Some("Gemini.app") {
            return Self::is_antigravity_app_bundle(bundle_url);
        }
        match Self::bundle_identifier(bundle_url) {
            Some(id) => Self::is_known_bundle_identifier(&id),
            None => true,
        }
    }

    fn read_bounded_artifact(path: &Path) -> Option<Vec<u8>> {
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_file() || metadata.len() > MAX_INSTALLED_ARTIFACT_BYTES {
            return None;
        }
        let file = std::fs::File::open(path).ok()?;
        let mut buf = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_INSTALLED_ARTIFACT_BYTES)
            .read_to_end(&mut buf)
            .ok()?;
        Some(buf)
    }

    pub fn parse_client_from_installed_artifact_data(
        data: &[u8],
    ) -> Option<AntigravityOAuthClient> {
        if let Ok(text) = std::str::from_utf8(data)
            && let Some(client) = Self::parse_client_from_installed_artifact_text(text)
        {
            return Some(client);
        }
        Self::binary_client(data).or_else(|| Self::unambiguous_client_in_bytes(data))
    }

    pub fn parse_client_from_installed_artifact_text(text: &str) -> Option<AntigravityOAuthClient> {
        const MARKER: &str = "vs/platform/cloudCode/common/oauthClient.js";
        let start = text.find(MARKER).unwrap_or(0);
        let mut end = start;
        for (char_count, (byte_idx, ch)) in text[start..].char_indices().enumerate() {
            if char_count >= 4000 {
                break;
            }
            end = start + byte_idx + ch.len_utf8();
        }
        let windowed = &text[start..end];

        let pair_matches = Self::extract_js_object_client_pairs(windowed);
        if !pair_matches.is_empty() {
            let ids: Vec<String> = pair_matches.iter().map(|p| p.0.clone()).collect();
            let secrets: Vec<String> = pair_matches.iter().map(|p| p.1.clone()).collect();
            return Self::unambiguous_client(&ids, &secrets);
        }

        let client_ids = Self::extract_client_ids(windowed);
        let client_secrets = Self::extract_client_secrets(windowed);
        Self::unambiguous_client(&client_ids, &client_secrets)
    }

    fn unambiguous_client_in_bytes(data: &[u8]) -> Option<AntigravityOAuthClient> {
        let text = String::from_utf8_lossy(data);
        let client_ids = Self::extract_client_ids(&text);
        let client_secrets = Self::extract_client_secrets(&text);
        Self::unambiguous_client(&client_ids, &client_secrets)
    }

    pub fn unambiguous_client(
        client_ids: &[String],
        client_secrets: &[String],
    ) -> Option<AntigravityOAuthClient> {
        let unique_ids: BTreeSet<&String> = client_ids.iter().collect();
        let unique_secrets: BTreeSet<&String> = client_secrets.iter().collect();
        if unique_ids.len() == 1 && unique_secrets.len() == 1 {
            let id = (*unique_ids.iter().next()?).clone();
            let secret = (*unique_secrets.iter().next()?).clone();
            Some(AntigravityOAuthClient::new(id, secret))
        } else {
            None
        }
    }

    pub fn unambiguous_clients(
        clients: &[AntigravityOAuthClient],
    ) -> Option<AntigravityOAuthClient> {
        let unique: BTreeSet<&AntigravityOAuthClient> = clients.iter().collect();
        if unique.len() == 1 {
            unique.into_iter().next().cloned()
        } else {
            None
        }
    }

    pub fn installed_app_urls(home_directory: Option<&Path>) -> Vec<PathBuf> {
        let mut urls = Vec::new();
        let mut seen = BTreeSet::new();
        let mut append_unique = |path: PathBuf| {
            let clean = normalize_lexical_path(&path);
            let key = clean.to_string_lossy().into_owned();
            if seen.insert(key) {
                urls.push(clean);
            }
        };

        for app_name in Self::CANDIDATE_APP_NAMES {
            append_unique(PathBuf::from("/Applications").join(app_name));
        }

        if let Some(home) = home_directory {
            let home_trimmed = home.to_string_lossy();
            if !home_trimmed.trim().is_empty() {
                for app_name in Self::CANDIDATE_APP_NAMES {
                    append_unique(home.join("Applications").join(app_name));
                }
                append_unique(home.join(".local/share/antigravity"));
                append_unique(home.join("opt/antigravity"));
            }
        }

        for linux_path in [
            "/opt/Antigravity",
            "/opt/antigravity",
            "/usr/share/antigravity",
            "/usr/local/share/antigravity",
            "/usr/lib/antigravity",
            "/usr/local/lib/antigravity",
            "/snap/antigravity/current",
        ] {
            append_unique(PathBuf::from(linux_path));
        }

        urls
    }

    pub fn extract_email_from_id_token(id_token: Option<&str>) -> Option<String> {
        let payload = Self::decode_jwt_payload(id_token?)?;
        let email = payload.get("email")?.as_str()?.trim();
        if email.is_empty() {
            None
        } else {
            Some(email.to_owned())
        }
    }

    pub fn extract_hosted_domain_from_id_token(id_token: Option<&str>) -> Option<String> {
        let payload = Self::decode_jwt_payload(id_token?)?;
        let hd = payload.get("hd")?.as_str()?.trim();
        if hd.is_empty() {
            None
        } else {
            Some(hd.to_owned())
        }
    }

    fn decode_jwt_payload(id_token: &str) -> Option<Map<String, Value>> {
        let parts: Vec<&str> = id_token.split('.').collect();
        if parts.len() < 2 {
            return None;
        }
        let mut payload_part = parts[1].replace('-', "+").replace('_', "/");
        let remainder = payload_part.len() % 4;
        if remainder > 0 {
            payload_part.push_str(&"=".repeat(4 - remainder));
        }
        let decoded = STANDARD.decode(payload_part.as_bytes()).ok()?;
        serde_json::from_slice(&decoded).ok()
    }

    // MARK: - Text Artifact Scanners

    fn extract_js_object_client_pairs(text: &str) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        let mut cursor = 0;
        while let Some(rel_idx) = text[cursor..].find("clientId") {
            let start = cursor + rel_idx;
            let after_key = start + "clientId".len();
            cursor = after_key;

            let Some((client_id, after_id)) = Self::parse_colon_quoted_value(&text[after_key..])
            else {
                continue;
            };
            if !client_id.ends_with(".apps.googleusercontent.com") {
                continue;
            }
            let scan_start = after_key + after_id;
            let remaining = &text[scan_start..];
            // Scan up to 400 chars without crossing `{` or `}`
            let mut window_end = 0;
            for (char_count, (byte_idx, ch)) in remaining.char_indices().enumerate() {
                if ch == '{' || ch == '}' || char_count >= 400 {
                    break;
                }
                window_end = byte_idx + ch.len_utf8();
            }
            let window = &remaining[..window_end];
            if let Some(sec_rel) = window.find("clientSecret") {
                let after_sec_key = sec_rel + "clientSecret".len();
                if let Some((client_secret, _)) =
                    Self::parse_colon_quoted_value(&window[after_sec_key..])
                    && client_secret.starts_with("GOCSPX-")
                {
                    pairs.push((client_id, client_secret));
                }
            }
        }
        pairs
    }

    fn parse_colon_quoted_value(s: &str) -> Option<(String, usize)> {
        let trimmed_start = s.trim_start();
        let ws1 = s.len() - trimmed_start.len();
        let after_colon = trimmed_start.strip_prefix(':')?;
        let after_colon_trimmed = after_colon.trim_start();
        let ws2 = after_colon.len() - after_colon_trimmed.len();
        let mut chars = after_colon_trimmed.chars();
        let quote = chars.next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let content = chars.as_str();
        let end_quote_idx = content.find(quote)?;
        let value = &content[..end_quote_idx];
        if value.is_empty() {
            return None;
        }
        let consumed = ws1 + 1 + ws2 + 1 + end_quote_idx + 1;
        Some((value.to_owned(), consumed))
    }

    fn extract_client_ids(text: &str) -> Vec<String> {
        const SUFFIX: &str = ".apps.googleusercontent.com";
        let mut results = Vec::new();
        let mut cursor = 0;
        while let Some(rel_idx) = text[cursor..].find(SUFFIX) {
            let suffix_start = cursor + rel_idx;
            let suffix_end = suffix_start + SUFFIX.len();
            cursor = suffix_end;

            // Walk backwards up to 256 bytes over `[A-Za-z0-9_-]`, then find `[0-9]+-[A-Za-z0-9_-]+`
            let before = &text.as_bytes()[..suffix_start];
            let min_pos = before.len().saturating_sub(256);
            let mut pos = before.len();
            while pos > min_pos
                && (before[pos - 1].is_ascii_alphanumeric()
                    || before[pos - 1] == b'_'
                    || before[pos - 1] == b'-')
            {
                pos -= 1;
            }
            while pos < before.len() && !before[pos].is_ascii_digit() {
                pos += 1;
            }
            let digit_start = pos;
            while pos < before.len() && before[pos].is_ascii_digit() {
                pos += 1;
            }
            let digit_len = pos - digit_start;
            if digit_len >= 1 && pos < before.len() && before[pos] == b'-' && pos + 1 < before.len()
            {
                results.push(text[digit_start..suffix_end].to_owned());
            }
        }
        results
    }

    fn extract_client_secrets(text: &str) -> Vec<String> {
        const PREFIX: &str = "GOCSPX-";
        let mut results = Vec::new();
        let mut cursor = 0;
        while let Some(rel_idx) = text[cursor..].find(PREFIX) {
            let start = cursor + rel_idx;
            let after_prefix = start + PREFIX.len();
            let bytes = &text.as_bytes()[after_prefix..];
            let mut len = 0;
            while len < bytes.len()
                && (bytes[len].is_ascii_alphanumeric() || bytes[len] == b'_' || bytes[len] == b'-')
            {
                len += 1;
            }
            cursor = after_prefix + len.max(1);
            if len > 0 {
                results.push(text[start..after_prefix + len].to_owned());
            }
        }
        results
    }

    fn is_exact_client_id(s: &str) -> bool {
        let extracted = Self::extract_client_ids(s);
        extracted.len() == 1 && extracted[0] == s
    }

    fn is_exact_client_secret(s: &str) -> bool {
        let extracted = Self::extract_client_secrets(s);
        extracted.len() == 1 && extracted[0] == s
    }

    // MARK: - Mach-O Binary Client Extraction

    pub fn binary_client(data: &[u8]) -> Option<AntigravityOAuthClient> {
        if let Some(slices) = Self::mach_o_slices(data) {
            let clients: Vec<AntigravityOAuthClient> =
                slices.into_iter().filter_map(Self::binary_client).collect();
            return Self::unambiguous_clients(&clients);
        }

        if let Some(client) = Self::binary_client_from_instructions(data) {
            return Some(client);
        }

        let strings = Self::mach_o_64_strings(data)?;
        let mut paired_clients = Vec::new();
        for index in 0..strings.len() {
            if Self::is_exact_client_id(&strings[index])
                && index + 1 < strings.len()
                && Self::is_exact_client_secret(&strings[index + 1])
            {
                paired_clients.push(AntigravityOAuthClient::new(
                    strings[index].clone(),
                    strings[index + 1].clone(),
                ));
            }
        }
        if let Some(client) = Self::unambiguous_clients(&paired_clients) {
            return Some(client);
        }

        let all_ids: Vec<String> = strings
            .iter()
            .filter(|s| Self::is_exact_client_id(s))
            .cloned()
            .collect();
        let all_secrets: Vec<String> = strings
            .iter()
            .filter(|s| Self::is_exact_client_secret(s))
            .cloned()
            .collect();
        Self::unambiguous_client(&all_ids, &all_secrets)
    }

    /// Reads adjacent ClientID/ClientSecret Go string fields from `__TEXT,__text` instructions.
    /// Prefers `CONSUMER_CLIENT_ID` when a bundle pairs several; otherwise the first record wins.
    fn binary_client_from_instructions(data: &[u8]) -> Option<AntigravityOAuthClient> {
        let word = |offset: usize| -> Option<u32> { Self::read_u32(data, offset, false) };
        if data.len() < 32 || word(0)? != 0xFEED_FACF {
            return None;
        }
        let cputype = word(4)?;
        if cputype != 0x0100_000C && cputype != 0x0100_0007 {
            return None;
        }
        let sizeofcmds = word(20)? as usize;
        if sizeofcmds > data.len() - 32 {
            return None;
        }
        let commands_end = 32 + sizeofcmds;
        let mut command = 32usize;
        let ncmds = (word(16)? as usize).min(4096);
        let mut first_client: Option<AntigravityOAuthClient> = None;

        for _ in 0..ncmds {
            if command + 8 > commands_end {
                return None;
            }
            let size = word(command + 4)? as usize;
            if size < 8 || size > commands_end - command {
                return None;
            }
            let next_command = command + size;
            if word(command)? == 0x19
                && size >= 72
                && data
                    .get(command + 8..command + 24)
                    .is_some_and(|s| s.starts_with(b"__TEXT\0"))
                && (word(command + 24)? & 4095) == 0
                && word(command + 40)? == 0
                && word(command + 44)? == 0
                && word(command + 52)? == 0
                && (word(command + 48)? as usize) <= data.len()
            {
                let text_end = word(command + 48)? as usize;
                let sections = word(command + 64)? as usize;
                if sections > (size - 72) / 80 {
                    return None;
                }
                for index in 0..sections {
                    let section = command + 72 + index * 80;
                    if !data
                        .get(section..section + 16)
                        .is_some_and(|s| s.starts_with(b"__text\0"))
                        || word(section + 44)? != 0
                    {
                        continue;
                    }
                    let start = word(section + 48)? as usize;
                    let length = word(section + 40)? as usize;
                    if start < commands_end || start > text_end || length > text_end - start {
                        return None;
                    }
                    let intel = cputype == 0x0100_0007;
                    let marker: &[u8] = if intel {
                        &[0x48, 0xC7, 0x40, 0x08]
                    } else {
                        &[0x01, 0x08, 0x00, 0xA9]
                    };
                    let section_end = start + length;
                    let mut search_pos = start;
                    while search_pos + marker.len() <= section_end {
                        let Some(rel_match) = data[search_pos..section_end]
                            .windows(marker.len())
                            .position(|w| w == marker)
                        else {
                            break;
                        };
                        let match_lower = search_pos + rel_match;
                        search_pos = match_lower + marker.len();
                        let min_record_len = if intel { 37usize } else { 32usize };
                        let record_offset_delta = if intel { 0usize } else { 12usize };
                        if match_lower < start + record_offset_delta {
                            continue;
                        }
                        let record = match_lower - record_offset_delta;
                        if record + min_record_len > section_end {
                            continue;
                        }

                        let reference = |offset: usize| -> Option<(usize, usize)> {
                            if intel {
                                if (word(offset)? & 0x00FF_FFFF) != 0x000D_8D48 {
                                    return None;
                                }
                                let disp = word(offset + 3)? as i32 as isize;
                                let target = (offset as isize).checked_add(7)?.checked_add(disp)?;
                                usize::try_from(target).ok().map(|t| (t, 0))
                            } else {
                                let page = word(offset)?;
                                let add = word(offset + 4)?;
                                let count = word(offset + 8)?;
                                if !offset.is_multiple_of(4)
                                    || (page & 0x9F00_001F) != 0x9000_0001
                                    || (add & 0xFFC0_03FF) != 0x9100_0021
                                    || (count & 0xFFE0_001F) != 0xD280_0002
                                {
                                    return None;
                                }
                                let immediate = (((page >> 5) & 0x7FFFF) << 2) | ((page >> 29) & 3);
                                let pages = ((immediate << 11) as i32 >> 11) as isize;
                                let base_page = (offset & !4095) as isize;
                                let target = base_page
                                    .checked_add(pages.checked_mul(4096)?)?
                                    .checked_add(((add >> 10) & 4095) as isize)?;
                                let len = ((count >> 5) & 0xFFFF) as usize;
                                Some((usize::try_from(target).ok()?, len))
                            }
                        };

                        let (id_ref, secret_ref) = if intel {
                            let Some(id_r) = reference(record + 8) else {
                                continue;
                            };
                            let Some(sec_r) = reference(record + 26) else {
                                continue;
                            };
                            if data.get(record + 15..record + 22)
                                != Some(&[0x48, 0x89, 0x08, 0x48, 0xC7, 0x40, 0x18])
                                || word(record + 33) != Some(0x1048_8948)
                            {
                                continue;
                            }
                            let id_len = word(record + 4)? as usize;
                            let sec_len = word(record + 22)? as usize;
                            ((id_r.0, id_len), (sec_r.0, sec_len))
                        } else {
                            let Some(id_r) = reference(record) else {
                                continue;
                            };
                            let Some(sec_r) = reference(record + 16) else {
                                continue;
                            };
                            if word(record + 28) != Some(0xA901_0801) {
                                continue;
                            }
                            (id_r, sec_r)
                        };

                        if !(30..=256).contains(&id_ref.1)
                            || secret_ref.1 != 35
                            || id_ref.0 < commands_end
                            || id_ref.0 + id_ref.1 > text_end
                            || secret_ref.0 < commands_end
                            || secret_ref.0 + secret_ref.1 > text_end
                        {
                            continue;
                        }
                        let Some(client_id) = data
                            .get(id_ref.0..id_ref.0 + id_ref.1)
                            .and_then(|s| std::str::from_utf8(s).ok())
                        else {
                            continue;
                        };
                        let Some(client_secret) = data
                            .get(secret_ref.0..secret_ref.0 + secret_ref.1)
                            .and_then(|s| std::str::from_utf8(s).ok())
                        else {
                            continue;
                        };
                        if !Self::is_exact_client_id(client_id)
                            || !Self::is_exact_client_secret(client_secret)
                        {
                            continue;
                        }
                        let client = AntigravityOAuthClient::new(client_id, client_secret);
                        if client_id == Self::CONSUMER_CLIENT_ID {
                            return Some(client);
                        }
                        if first_client.is_none() {
                            first_client = Some(client);
                        }
                    }
                }
            }
            command = next_command;
        }
        first_client
    }

    fn mach_o_slices(data: &[u8]) -> Option<Vec<&[u8]>> {
        let magic = Self::read_u32(data, 0, false)?;
        let (is_64_bit, byte_swapped) = match magic {
            0xCAFE_BABE => (false, false),
            0xBEBA_FECA => (false, true),
            0xCAFE_BABF => (true, false),
            0xBFBA_FECA => (true, true),
            _ => return None,
        };

        let count = Self::read_u32(data, 4, byte_swapped)? as usize;
        let arch_size = if is_64_bit { 32usize } else { 20usize };
        let header_size = 8usize;
        if count == 0 || data.len() < header_size.checked_add(count.checked_mul(arch_size)?)? {
            return None;
        }

        let mut slices = Vec::new();
        for index in 0..count {
            let entry_offset = header_size + index * arch_size;
            let (slice_offset, slice_size) = if is_64_bit {
                let offset = Self::read_u64(data, entry_offset + 8, byte_swapped)? as usize;
                let size = Self::read_u64(data, entry_offset + 16, byte_swapped)? as usize;
                (offset, size)
            } else {
                let offset = Self::read_u32(data, entry_offset + 8, byte_swapped)? as usize;
                let size = Self::read_u32(data, entry_offset + 12, byte_swapped)? as usize;
                (offset, size)
            };
            let end = slice_offset.checked_add(slice_size)?;
            if slice_size > 0 && end <= data.len() {
                slices.push(&data[slice_offset..end]);
            }
        }

        if slices.is_empty() {
            None
        } else {
            Some(slices)
        }
    }

    fn mach_o_64_strings(data: &[u8]) -> Option<Vec<String>> {
        let magic = Self::read_u32(data, 0, false)?;
        let byte_swapped = match magic {
            0xFEED_FACF => false,
            0xCFFA_EDFE => true,
            _ => return None,
        };

        let ncmds = Self::read_u32(data, 16, byte_swapped)? as usize;
        let sizeofcmds = Self::read_u32(data, 20, byte_swapped)? as usize;
        let header_size = 32usize;
        if data.len() < header_size.checked_add(sizeofcmds)? {
            return None;
        }

        let mut segments = Vec::new();
        let mut sections = Vec::new();
        let mut command_offset = header_size;
        for _ in 0..ncmds {
            if command_offset + 8 > header_size + sizeofcmds {
                return None;
            }
            let cmd = Self::read_u32(data, command_offset, byte_swapped)?;
            let cmdsize = Self::read_u32(data, command_offset + 4, byte_swapped)? as usize;
            if cmdsize < 8 || command_offset + cmdsize > header_size + sizeofcmds {
                return None;
            }

            if cmd == 0x19 && cmdsize >= 72 {
                let vmaddr = Self::read_u64(data, command_offset + 24, byte_swapped)?;
                let vmsize = Self::read_u64(data, command_offset + 32, byte_swapped)?;
                let fileoff = Self::read_u64(data, command_offset + 40, byte_swapped)?;
                let filesize = Self::read_u64(data, command_offset + 48, byte_swapped)?;
                let nsects = Self::read_u32(data, command_offset + 64, byte_swapped)? as usize;
                segments.push(MachOSegment64 {
                    vmaddr,
                    vmsize,
                    fileoff,
                    filesize,
                });

                let mut section_offset = command_offset + 72;
                for _ in 0..nsects {
                    if section_offset + 80 > command_offset + cmdsize {
                        break;
                    }
                    let sectname = Self::read_fixed_string(data, section_offset, 16);
                    let segname = Self::read_fixed_string(data, section_offset + 16, 16);
                    let size = Self::read_u64(data, section_offset + 40, byte_swapped)?;
                    let offset = Self::read_u32(data, section_offset + 48, byte_swapped)?;
                    sections.push(MachOSection64 {
                        sectname,
                        segname,
                        size,
                        offset,
                    });
                    section_offset += 80;
                }
            }
            command_offset += cmdsize;
        }

        let mut strings = Vec::new();
        for section in &sections {
            if section.segname == "__TEXT" && section.sectname == "__cstring" {
                strings.extend(Self::read_c_strings(data, section));
            } else if (section.segname == "__DATA" || section.segname == "__DATA_CONST")
                && section.sectname == "__const"
            {
                strings.extend(Self::read_pointer_length_strings(
                    data,
                    section,
                    &segments,
                    byte_swapped,
                ));
            }
        }
        Some(strings)
    }

    fn read_c_strings(data: &[u8], section: &MachOSection64) -> Vec<String> {
        let Some(slice) = Self::section_slice(data, section) else {
            return Vec::new();
        };
        slice
            .split(|&b| b == 0)
            .filter(|part| !part.is_empty())
            .filter_map(|part| std::str::from_utf8(part).ok().map(str::to_owned))
            .collect()
    }

    fn read_pointer_length_strings(
        data: &[u8],
        section: &MachOSection64,
        segments: &[MachOSegment64],
        byte_swapped: bool,
    ) -> Vec<String> {
        let Some(slice) = Self::section_slice(data, section) else {
            return Vec::new();
        };
        if slice.len() < 16 {
            return Vec::new();
        }
        let mut strings = Vec::new();
        let mut offset = 0usize;
        while offset + 16 <= slice.len() {
            let Some(raw_pointer) = Self::read_u64(slice, offset, byte_swapped) else {
                break;
            };
            let Some(length) = Self::read_u64(slice, offset + 8, byte_swapped) else {
                break;
            };
            offset += 16;

            let pointer = raw_pointer & ((1u64 << 48) - 1);
            if length == 0 || length > 256 || pointer == 0 || usize::try_from(length).is_err() {
                continue;
            }
            let len_usize = length as usize;
            let Some(file_offset) = Self::file_offset_for_vmaddr(pointer, segments) else {
                continue;
            };
            let Some(end) = file_offset.checked_add(len_usize) else {
                continue;
            };
            if end > data.len() {
                continue;
            }
            if let Ok(value) = std::str::from_utf8(&data[file_offset..end]) {
                strings.push(value.to_owned());
            }
        }
        strings
    }

    fn section_slice<'a>(data: &'a [u8], section: &MachOSection64) -> Option<&'a [u8]> {
        let start = section.offset as usize;
        let size = usize::try_from(section.size).ok()?;
        let end = start.checked_add(size)?;
        if size == 0 || end > data.len() {
            None
        } else {
            Some(&data[start..end])
        }
    }

    fn file_offset_for_vmaddr(vmaddr: u64, segments: &[MachOSegment64]) -> Option<usize> {
        for segment in segments {
            let seg_end = segment.vmaddr.checked_add(segment.vmsize)?;
            if vmaddr >= segment.vmaddr && vmaddr < seg_end {
                let relative = vmaddr - segment.vmaddr;
                if relative >= segment.filesize {
                    continue;
                }
                return usize::try_from(segment.fileoff.checked_add(relative)?).ok();
            }
        }
        None
    }

    fn read_fixed_string(data: &[u8], offset: usize, length: usize) -> String {
        let Some(end) = offset.checked_add(length) else {
            return String::new();
        };
        if end > data.len() {
            return String::new();
        }
        let slice = &data[offset..end];
        let trimmed = slice.split(|&b| b == 0).next().unwrap_or(slice);
        String::from_utf8_lossy(trimmed).into_owned()
    }

    fn read_u32(data: &[u8], offset: usize, byte_swapped: bool) -> Option<u32> {
        let bytes: [u8; 4] = data.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
        Some(if byte_swapped {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        })
    }

    fn read_u64(data: &[u8], offset: usize, byte_swapped: bool) -> Option<u64> {
        let bytes: [u8; 8] = data.get(offset..offset.checked_add(8)?)?.try_into().ok()?;
        Some(if byte_swapped {
            u64::from_be_bytes(bytes)
        } else {
            u64::from_le_bytes(bytes)
        })
    }
}

struct MachOSegment64 {
    vmaddr: u64,
    vmsize: u64,
    fileoff: u64,
    filesize: u64,
}

struct MachOSection64 {
    sectname: String,
    segname: String,
    size: u64,
    offset: u32,
}

pub struct AntigravityOAuthCredentialsStore;

impl AntigravityOAuthCredentialsStore {
    pub const ENVIRONMENT_CREDENTIALS_KEY: &str = "ANTIGRAVITY_OAUTH_CREDENTIALS_JSON";

    pub fn default_directory_url(home_directory: &Path) -> PathBuf {
        home_directory.join(".ariadusage").join("antigravity")
    }

    pub fn credentials_url(home_directory: &Path) -> PathBuf {
        Self::default_directory_url(home_directory).join("oauth_creds.json")
    }

    pub fn exists(credentials_url: &Path) -> bool {
        credentials_url.is_file()
    }

    pub fn has_selected_account_credentials<V: EnvValue>(
        environment: &BTreeMap<String, V>,
    ) -> bool {
        environment
            .get(Self::ENVIRONMENT_CREDENTIALS_KEY)
            .is_some_and(|raw| !raw.as_env_str().trim().is_empty())
    }

    pub fn load(
        credentials_url: &Path,
    ) -> Result<AntigravityOAuthCredentials, AntigravityRemoteFetchError> {
        let call = BrokerCall {
            interaction: FetchInteraction::UserInitiated,
            cancel: CancellationToken::new(),
            request_id: "antigravity-oauth-load".to_owned(),
        };
        Self::load_with_call(credentials_url, &call)
    }

    pub fn load_with_call(
        credentials_url: &Path,
        call: &BrokerCall,
    ) -> Result<AntigravityOAuthCredentials, AntigravityRemoteFetchError> {
        let decl = CredentialDecl::new(credentials_url);
        let read = credential_file::read(&decl, call).map_err(|err| match err {
            CredentialFileError::NotFound => AntigravityRemoteFetchError::NotLoggedIn,
            other => AntigravityRemoteFetchError::ParseFailed(other.to_string()),
        })?;
        Self::decode_credentials(&read.bytes)
    }

    pub fn load_selected_account_credentials<V: EnvValue>(
        environment: &BTreeMap<String, V>,
    ) -> Result<Option<AntigravityOAuthCredentials>, AntigravityRemoteFetchError> {
        let Some(raw) = environment
            .get(Self::ENVIRONMENT_CREDENTIALS_KEY)
            .map(|s| s.as_env_str().trim())
            .filter(|s| !s.is_empty())
        else {
            return Ok(None);
        };
        match Self::decode_credentials(raw.as_bytes()) {
            Ok(creds) => Ok(Some(creds)),
            Err(_) => Err(AntigravityRemoteFetchError::ParseFailed(
                "Could not decode Antigravity selected account credentials.".to_owned(),
            )),
        }
    }

    pub fn token_account_value(
        credentials: &AntigravityOAuthCredentials,
    ) -> Result<String, AntigravityRemoteFetchError> {
        let data = Self::encode_credentials(credentials)?;
        String::from_utf8(data).map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))
    }

    pub fn credentials_from_token_account_value(
        value: &str,
    ) -> Option<AntigravityOAuthCredentials> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        Self::decode_credentials(trimmed.as_bytes()).ok()
    }

    pub fn resolved_account_email(credentials: &AntigravityOAuthCredentials) -> Option<String> {
        credentials.resolved_account_email()
    }

    pub fn save(
        credentials: &AntigravityOAuthCredentials,
        credentials_url: &Path,
    ) -> Result<(), AntigravityRemoteFetchError> {
        let data = Self::encode_credentials(credentials)?;
        write_private(credentials_url, &data, &WriteHooks::default())
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))
    }

    pub fn delete(credentials_url: &Path) -> Result<(), std::io::Error> {
        if credentials_url.exists() {
            std::fs::remove_file(credentials_url)?;
        }
        Ok(())
    }

    pub fn encode_credentials(
        credentials: &AntigravityOAuthCredentials,
    ) -> Result<Vec<u8>, AntigravityRemoteFetchError> {
        let mut payload = Map::new();
        if !credentials.access_token.is_empty() {
            payload.insert(
                "access_token".to_owned(),
                Value::String(credentials.access_token.clone()),
            );
        }
        if let Some(refresh_token) = &credentials.refresh_token {
            payload.insert(
                "refresh_token".to_owned(),
                Value::String(refresh_token.clone()),
            );
        }
        if let Some(expiry_date) = credentials.expiry_date {
            let millis = expiry_date.as_millisecond() as f64;
            if let Some(num) = serde_json::Number::from_f64(millis) {
                payload.insert("expiry_date".to_owned(), Value::Number(num));
            }
        }
        if let Some(id_token) = &credentials.id_token {
            payload.insert("id_token".to_owned(), Value::String(id_token.clone()));
        }
        if let Some(email) = &credentials.email {
            payload.insert("email".to_owned(), Value::String(email.clone()));
        }
        if let Some(project_id) = &credentials.project_id {
            payload.insert("project_id".to_owned(), Value::String(project_id.clone()));
        }
        if let Some(client_id) = &credentials.client_id {
            payload.insert("client_id".to_owned(), Value::String(client_id.clone()));
        }
        if let Some(client_secret) = &credentials.client_secret {
            payload.insert(
                "client_secret".to_owned(),
                Value::String(client_secret.clone()),
            );
        }

        let mut data = serde_json::to_vec_pretty(&Value::Object(payload))
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))?;
        data.push(b'\n');
        Ok(data)
    }

    pub fn decode_credentials(
        data: &[u8],
    ) -> Result<AntigravityOAuthCredentials, AntigravityRemoteFetchError> {
        let json: Map<String, Value> = serde_json::from_slice(data)
            .map_err(|e| AntigravityRemoteFetchError::ParseFailed(e.to_string()))?;

        let access_token =
            Self::non_empty_field_dual(&json, "access_token", "accessToken").unwrap_or_default();
        let refresh_token = Self::non_empty_field_dual(&json, "refresh_token", "refreshToken");
        let id_token = Self::non_empty_field_dual(&json, "id_token", "idToken");
        let email = Self::non_empty_field(&json, "email");
        let project_id = Self::non_empty_field_dual(&json, "project_id", "projectId");
        let client_id = Self::non_empty_field_dual(&json, "client_id", "clientId");
        let client_secret = Self::non_empty_field_dual(&json, "client_secret", "clientSecret");
        let expiry_date = Self::parse_expiry_millis(&json);

        Ok(AntigravityOAuthCredentials {
            access_token,
            refresh_token,
            expiry_date,
            id_token,
            email,
            project_id,
            client_id,
            client_secret,
        })
    }

    fn non_empty_field(json: &Map<String, Value>, key: &str) -> Option<String> {
        let s = json.get(key)?.as_str()?.trim();
        if s.is_empty() {
            None
        } else {
            Some(s.to_owned())
        }
    }

    fn non_empty_field_dual(
        json: &Map<String, Value>,
        snake_key: &str,
        camel_key: &str,
    ) -> Option<String> {
        Self::non_empty_field(json, snake_key).or_else(|| Self::non_empty_field(json, camel_key))
    }

    fn parse_expiry_millis(json: &Map<String, Value>) -> Option<Timestamp> {
        let val = json.get("expiry_date").or_else(|| json.get("expiresAt"))?;
        let millis = match val {
            Value::Number(num) => num.as_f64()?,
            Value::String(s) => s.trim().parse::<f64>().ok()?,
            _ => return None,
        };
        if !millis.is_finite() {
            return None;
        }
        let seconds = millis / 1000.0;
        let whole_secs = seconds.trunc() as i64;
        let nanos = ((seconds - seconds.trunc()) * 1_000_000_000.0).round() as i32;
        Timestamp::new(whole_secs, nanos).ok()
    }
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        normalized.push(component.as_os_str());
    }
    normalized
}
