// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexCLIBackendConfiguration.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/Providers/Codex/CodexConsumerProjection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use super::credentials::CodexHomeScope;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexCLIBackend {
    AmazonBedrock,
    CustomProvider,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodexCLIBackendRateLimitError {
    #[error("{}", .0.error_description())]
    ChatGPTRateLimitsUnavailable(CodexCLIBackend),
}

impl CodexCLIBackend {
    pub fn error_description(self) -> &'static str {
        match self {
            Self::AmazonBedrock => {
                "Codex is configured for Amazon Bedrock; ChatGPT rate limits are unavailable. Disable the Codex usage card or use cost-based tracking."
            }
            Self::CustomProvider => {
                "Codex is configured for a custom provider; ChatGPT rate limits are unavailable. Disable the Codex usage card or use cost-based tracking."
            }
        }
    }
}

impl CodexCLIBackendRateLimitError {
    pub fn error_description(&self) -> &'static str {
        match self {
            Self::ChatGPTRateLimitsUnavailable(backend) => backend.error_description(),
        }
    }

    pub fn classify_with_env(
        error_description: &str,
        env: &BTreeMap<String, String>,
        home_dir: Option<&Path>,
    ) -> Option<Self> {
        let contents = CodexCLIBackendConfiguration::load_contents(env, home_dir)?;
        Self::classify(error_description, &contents)
    }

    pub fn classify(error_description: &str, config_contents: &str) -> Option<Self> {
        let lower = error_description.to_lowercase();
        let is_auth_error = lower.contains("codex account authentication required")
            || lower.contains("account authentication required to read rate limits")
            || lower.contains("requiresopenaiauth");
        if !is_auth_error {
            return None;
        }
        let configuration = CodexCLIBackendConfiguration::parse(config_contents)?;
        if configuration.requires_openai_auth {
            return None;
        }
        Some(Self::ChatGPTRateLimitsUnavailable(configuration.backend))
    }
}

#[derive(Default)]
struct ProviderSettings {
    name: Option<String>,
    requires_openai_auth: Option<bool>,
}

struct CodexCLIBackendConfiguration {
    requires_openai_auth: bool,
    backend: CodexCLIBackend,
}

impl CodexCLIBackendConfiguration {
    const MAXIMUM_CONFIG_BYTES: usize = 256 * 1024;
    const OPENAI_PROVIDER_IDS: &'static [&'static str] = &["openai", "chatgpt"];
    const AMAZON_BEDROCK_PROVIDER_IDS: &'static [&'static str] =
        &["amazon-bedrock", "amazon-bedrock-runtime"];

    fn parse(contents: &str) -> Option<Self> {
        let mut active_provider_id: Option<String> = None;
        let mut current_provider_id: Option<String> = None;
        let mut provider_settings: HashMap<String, ProviderSettings> = HashMap::new();

        for raw_line in contents.lines() {
            let line = Self::removing_comment(raw_line).trim().to_owned();
            if line.is_empty() {
                continue;
            }

            if line.starts_with('[') {
                current_provider_id = Self::model_provider_id_from_table_header(&line);
                continue;
            }

            let Some((key_raw, val_raw)) = line.split_once('=') else {
                continue;
            };
            let key = key_raw.trim();

            if current_provider_id.is_none() && key == "model_provider" {
                active_provider_id = Self::toml_string(val_raw);
                continue;
            }

            let Some(ref provider_id) = current_provider_id else {
                continue;
            };
            match key {
                "name" => {
                    provider_settings
                        .entry(provider_id.clone())
                        .or_default()
                        .name = Self::toml_string(val_raw);
                }
                "requires_openai_auth" => {
                    provider_settings
                        .entry(provider_id.clone())
                        .or_default()
                        .requires_openai_auth = Self::toml_bool(val_raw);
                }
                _ => {}
            }
        }

        let active_provider_id = active_provider_id?;
        let active_provider_id = active_provider_id.trim();
        if active_provider_id.is_empty() {
            return None;
        }

        let normalized_id = active_provider_id.to_lowercase();
        if Self::OPENAI_PROVIDER_IDS.contains(&normalized_id.as_str()) {
            return Some(Self {
                requires_openai_auth: true,
                backend: CodexCLIBackend::CustomProvider,
            });
        }

        let settings = provider_settings.get(active_provider_id).or_else(|| {
            provider_settings
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(active_provider_id))
                .map(|(_, v)| v)
        });
        let requires_openai_auth = settings
            .and_then(|s| s.requires_openai_auth)
            .unwrap_or(false);
        let normalized_name = settings
            .and_then(|s| s.name.as_deref())
            .unwrap_or("")
            .to_lowercase();
        let backend = if Self::AMAZON_BEDROCK_PROVIDER_IDS.contains(&normalized_id.as_str())
            || normalized_name.contains("bedrock")
        {
            CodexCLIBackend::AmazonBedrock
        } else {
            CodexCLIBackend::CustomProvider
        };

        Some(Self {
            requires_openai_auth,
            backend,
        })
    }

    fn load_contents(env: &BTreeMap<String, String>, home_dir: Option<&Path>) -> Option<String> {
        let root = CodexHomeScope::ambient_home_url(env, home_dir)?;
        let config_path = root.join("config.toml");
        let file = File::open(config_path).ok()?;
        let mut buf = Vec::new();
        file.take(Self::MAXIMUM_CONFIG_BYTES as u64)
            .read_to_end(&mut buf)
            .ok()?;
        if buf.is_empty() {
            return None;
        }
        String::from_utf8(buf).ok()
    }

    fn model_provider_id_from_table_header(line: &str) -> Option<String> {
        if !line.starts_with('[') || !line.ends_with(']') || line.starts_with("[[") {
            return None;
        }
        let header = line[1..line.len() - 1].trim();
        let prefix = "model_providers.";
        let raw_id = header.strip_prefix(prefix)?.trim();
        if raw_id.is_empty() {
            return None;
        }
        Self::toml_string(raw_id).or_else(|| {
            if raw_id.contains('.') {
                None
            } else {
                Some(raw_id.to_owned())
            }
        })
    }

    fn removing_comment(line: &str) -> &str {
        let mut quote: Option<char> = None;
        let mut escaped = false;
        for (idx, ch) in line.char_indices() {
            if let Some(active_quote) = quote {
                if active_quote == '"' && ch == '\\' && !escaped {
                    escaped = true;
                    continue;
                }
                if ch == active_quote && !escaped {
                    quote = None;
                }
                escaped = false;
                continue;
            }
            if ch == '"' || ch == '\'' {
                quote = Some(ch);
            } else if ch == '#' {
                return &line[..idx];
            }
        }
        line
    }

    fn toml_string(raw_value: &str) -> Option<String> {
        let value = raw_value.trim();
        let quote = value.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        if quote == '\'' {
            let rest = &value[1..];
            let end = rest.find('\'')?;
            return Some(rest[..end].to_owned());
        }

        let mut escaped = false;
        for (offset, ch) in value[1..].char_indices() {
            let idx = offset + 1;
            if ch == '"' && !escaped {
                let token = &value[..=idx];
                return serde_json::from_str::<String>(token).ok();
            }
            if ch == '\\' {
                escaped = !escaped;
            } else {
                escaped = false;
            }
        }
        None
    }

    fn toml_bool(raw_value: &str) -> Option<bool> {
        match raw_value.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }
}

pub struct CodexUIErrorMapper;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CodexUserFacingErrors {
    pub usage: Option<String>,
    pub credits: Option<String>,
    pub dashboard: Option<String>,
}

impl CodexUserFacingErrors {
    #[must_use]
    pub fn credits_hint_copy_text(&self) -> Option<&str> {
        self.dashboard.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexProviderErrorDisplay {
    pub preview: String,
    pub full: String,
}

impl CodexUIErrorMapper {
    pub const CODEX_CLI_NOT_SIGNED_IN_MESSAGE: &'static str =
        "Codex CLI is not signed in. Run `codex login --device-auth`, then refresh.";
    pub const CODEX_CLI_MISSING_MESSAGE: &'static str =
        "Codex CLI missing. Install via `npm i -g @openai/codex` (or bun install) and restart.";
    pub const CODEX_SESSION_EXPIRED_MESSAGE: &'static str = "Codex session expired. Sign in again.";
    pub const OPENAI_WEB_INTERRUPTED_MESSAGE: &'static str =
        "OpenAI web refresh was interrupted. Refresh OpenAI cookies and try again.";
    pub const OPENAI_WEB_TIMEOUT_MESSAGE: &'static str =
        "OpenAI web refresh timed out. Refresh OpenAI cookies and try again.";
    pub const OPENAI_WEB_NETWORK_ERROR_MESSAGE: &'static str = "OpenAI web refresh hit a network error. Check your connection, then refresh OpenAI cookies and try again.";
    pub const CODEX_TEMPORARILY_UNAVAILABLE_MESSAGE: &'static str =
        "Codex usage is temporarily unavailable. Try refreshing.";

    pub fn user_facing_message(raw: Option<&str>) -> Option<String> {
        let trimmed = raw?.trim();
        if trimmed.is_empty() {
            return None;
        }
        let lower = trimmed.to_lowercase();
        if lower.contains("data not available yet")
            || lower.contains("could not parse codex status")
        {
            return Some("Could not parse Codex status; will retry shortly.".to_owned());
        }
        if Self::is_already_user_facing(&lower) {
            return Some(trimmed.to_owned());
        }
        if let Some(cached) = Self::cached_message(trimmed, &lower) {
            return Some(cached);
        }
        if Self::looks_codex_cli_missing(&lower) {
            return Some(Self::CODEX_CLI_MISSING_MESSAGE.to_owned());
        }
        if Self::looks_codex_cli_login_required(&lower) {
            return Some(Self::CODEX_CLI_NOT_SIGNED_IN_MESSAGE.to_owned());
        }
        if Self::looks_expired(&lower) {
            return Some(Self::CODEX_SESSION_EXPIRED_MESSAGE.to_owned());
        }
        if lower.contains("frame load interrupted") {
            return Some(Self::OPENAI_WEB_INTERRUPTED_MESSAGE.to_owned());
        }
        if Self::looks_openai_web_timeout(&lower) {
            return Some(Self::OPENAI_WEB_TIMEOUT_MESSAGE.to_owned());
        }
        if Self::looks_openai_web_network_error(&lower) {
            return Some(Self::OPENAI_WEB_NETWORK_ERROR_MESSAGE.to_owned());
        }
        if Self::looks_internal_transport(&lower) {
            return Some(Self::CODEX_TEMPORARILY_UNAVAILABLE_MESSAGE.to_owned());
        }
        Some(trimmed.to_owned())
    }

    pub fn user_facing_error_for_provider(provider_id: &str, raw: Option<&str>) -> Option<String> {
        if provider_id.eq_ignore_ascii_case("codex") {
            Self::user_facing_message(raw)
        } else {
            raw.map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        }
    }

    pub fn user_facing_errors(
        raw_usage_error: Option<&str>,
        raw_credits_error: Option<&str>,
        raw_dashboard_error: Option<&str>,
        allows_live_adjuncts: bool,
    ) -> CodexUserFacingErrors {
        CodexUserFacingErrors {
            usage: Self::user_facing_message(raw_usage_error),
            credits: allows_live_adjuncts
                .then(|| Self::user_facing_message(raw_credits_error))
                .flatten(),
            dashboard: allows_live_adjuncts
                .then(|| Self::user_facing_message(raw_dashboard_error))
                .flatten(),
        }
    }

    pub fn provider_error_display(
        provider_id: &str,
        raw: Option<&str>,
    ) -> Option<CodexProviderErrorDisplay> {
        let full = raw?.trim();
        if full.is_empty() {
            return None;
        }
        let preview = Self::user_facing_error_for_provider(provider_id, Some(full))
            .unwrap_or_else(|| full.to_owned());
        Some(CodexProviderErrorDisplay {
            preview,
            full: full.to_owned(),
        })
    }

    fn cached_message(raw: &str, lower: &str) -> Option<String> {
        const CACHED_MARKER: &str = " Cached values from ";
        let suffix_idx = raw.find(CACHED_MARKER)?;
        let raw_prefix = &raw[..suffix_idx];
        let stamp = Self::cached_stamp(&raw[suffix_idx + CACHED_MARKER.len()..]);

        if lower.starts_with("last codex credits refresh failed:") {
            let base = Self::user_facing_message(Some(&Self::failure_message(
                raw_prefix,
                "Last Codex credits refresh failed:",
            )))?;
            return Some(format!("{base} Cached values from {stamp}."));
        }
        if lower.starts_with("last openai dashboard refresh failed:") {
            let base = Self::user_facing_message(Some(&Self::failure_message(
                raw_prefix,
                "Last OpenAI dashboard refresh failed:",
            )))?;
            return Some(format!("{base} Cached values from {stamp}."));
        }
        None
    }

    fn failure_message(raw_prefix: &str, prefix: &str) -> String {
        let dropped = if raw_prefix
            .get(..prefix.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
        {
            &raw_prefix[prefix.len()..]
        } else {
            raw_prefix
        };
        let trimmed = dropped.trim();
        trimmed.strip_suffix('.').unwrap_or(trimmed).to_owned()
    }

    fn cached_stamp(after_marker: &str) -> &str {
        let trimmed = after_marker.trim();
        trimmed.strip_suffix('.').unwrap_or(trimmed)
    }

    fn is_already_user_facing(lower: &str) -> bool {
        lower.contains("openai cookies are for")
            || lower.contains("sign in to chatgpt.com")
            || lower.contains("requires a signed-in chatgpt.com session")
            || lower.contains("managed codex account data is unavailable")
            || lower.contains("selected managed codex account is unavailable")
            || lower.contains("codex credits are still loading")
            || lower.contains("codex account changed; importing browser cookies")
            || lower.contains("codex cli is not signed in.")
            || lower.contains("chatgpt rate limits are unavailable.")
            || lower.contains("codex session expired. sign in again.")
            || lower.contains("openai web refresh timed out. refresh openai cookies and try again.")
            || lower.contains(
                "openai web refresh hit a network error. check your connection, then refresh openai cookies and try again.",
            )
            || lower.contains("codex usage is temporarily unavailable. try refreshing.")
    }

    fn looks_codex_cli_missing(lower: &str) -> bool {
        lower.contains("codex cli missing")
            || lower.contains("codex cli not found")
            || lower.contains("missing cli codex")
            || lower.contains("missing cli 'codex'")
            || lower.contains("missing cli \"codex\"")
            || (lower.contains("binary not found") && lower.contains("codex"))
    }

    fn looks_codex_cli_login_required(lower: &str) -> bool {
        lower.contains("codex account authentication required")
            || lower.contains("account authentication required to read rate limits")
            || lower.contains("requiresopenaiauth")
    }

    fn looks_expired(lower: &str) -> bool {
        lower.contains("token_expired")
            || lower.contains("authentication token is expired")
            || lower.contains("oauth token has expired")
            || lower.contains("provided authentication token is expired")
            || lower.contains("please try signing in again")
            || lower.contains("please sign in again")
            || (lower.contains("401") && lower.contains("unauthorized"))
    }

    fn looks_internal_transport(lower: &str) -> bool {
        lower.contains("codex connection failed")
            || lower.contains("failed to fetch codex rate limits")
            || lower.contains("/backend-api/")
            || lower.contains("content-type=")
            || lower.contains("body={")
            || lower.contains("body=")
            || lower.contains("get https://")
            || lower.contains("get http://")
            || lower.contains("returned invalid data")
    }

    fn looks_openai_web_timeout(lower: &str) -> bool {
        lower.contains("nsurlerrordomain")
            && (lower.contains("timed out") || lower.contains("error -1001"))
    }

    fn looks_openai_web_network_error(lower: &str) -> bool {
        lower.contains("nsurlerrordomain")
    }
}
