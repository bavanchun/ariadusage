// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexCLIBackendConfiguration.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

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

impl CodexUIErrorMapper {
    pub fn user_facing_message(raw: Option<&str>) -> Option<String> {
        let raw = raw?.trim();
        if raw.is_empty() {
            return None;
        }
        let lower = raw.to_lowercase();
        if lower.contains("data not available yet")
            || lower.contains("could not parse codex status")
        {
            return Some("Could not parse Codex status; will retry shortly.".to_owned());
        }
        if lower.contains("codex account authentication required")
            || lower.contains("account authentication required to read rate limits")
            || lower.contains("requiresopenaiauth")
        {
            return Some(
                "Codex CLI is not signed in. Run `codex login --device-auth`, then refresh."
                    .to_owned(),
            );
        }
        Some(raw.to_owned())
    }
}
