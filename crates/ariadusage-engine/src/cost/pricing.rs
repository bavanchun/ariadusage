// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsagePricing.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsagePricing+CodexResolver.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsagePricing+Overlay.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsagePricing+Provider.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsagePricingKey.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageCustomPricing.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/ModelsDevPricing.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/ModelsDevPricingTargetResolver.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/OpenCodexUsage/OpenCodexRouteDispatcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

pub const CODEX_PRIORITY_INPUT_TOKEN_LIMIT: i64 = 272_000;
pub const CODEX_UNATTRIBUTED_MODEL: &str = "unknown";
pub const COST_SCALE_NANOS: f64 = 1_000_000_000.0;

pub const CODEX_GPT56_PRICING_CUTOFF_SECS: i64 = 1_785_369_600;
pub const CODEX_SOL_PRICING_CUTOFF_SECS: i64 = 1_787_270_400;
pub const CLAUDE_FULL_CONTEXT_STANDARD_PRICING_CUTOFF_SECS: i64 = 1_773_360_000;

pub const CODEX_MODELS_DEV_PROVIDER_ID: &str = "openai";
pub const CLAUDE_MODELS_DEV_PROVIDER_ID: &str = "anthropic";

pub const CODEX_MODELS_DEV_PROVIDER_IDS: &[&str] = &[
    "deepseek",
    "kimi-coding",
    "kimi-for-coding",
    "openai",
    "opencode",
    "opencode-free",
    "opencode-go",
];

pub const CLAUDE_FIRST_PARTY_MODELS_DEV_PROVIDER_IDS: &[&str] = &[
    "anthropic",
    "openai",
    "google",
    "moonshot",
    "kimi-for-coding",
    "minimax",
    "deepseek",
];

/// Pricing definition for a Codex model (USD per token).
#[derive(Debug, Clone, PartialEq)]
pub struct CodexPricing {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
    pub cache_read_input_cost_per_token: Option<f64>,
    pub cache_write_input_cost_per_token: Option<f64>,
    pub display_label: Option<&'static str>,
    pub threshold_tokens: Option<i64>,
    pub input_cost_per_token_above_threshold: Option<f64>,
    pub output_cost_per_token_above_threshold: Option<f64>,
    pub cache_read_input_cost_per_token_above_threshold: Option<f64>,
    pub cache_write_input_cost_per_token_above_threshold: Option<f64>,
}

impl CodexPricing {
    pub const fn standard(input: f64, output: f64, cached: Option<f64>) -> Self {
        Self {
            input_cost_per_token: input,
            output_cost_per_token: output,
            cache_read_input_cost_per_token: cached,
            cache_write_input_cost_per_token: None,
            display_label: None,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
            cache_write_input_cost_per_token_above_threshold: None,
        }
    }

    pub const fn long_context(
        input: f64,
        cached: Option<f64>,
        write: Option<f64>,
        output_standard: f64,
        output_long: f64,
    ) -> Self {
        Self {
            input_cost_per_token: input,
            output_cost_per_token: output_standard,
            cache_read_input_cost_per_token: cached,
            cache_write_input_cost_per_token: write,
            display_label: None,
            threshold_tokens: Some(272_000),
            input_cost_per_token_above_threshold: Some(input * 2.0),
            output_cost_per_token_above_threshold: Some(output_long),
            cache_read_input_cost_per_token_above_threshold: match cached {
                Some(c) => Some(c * 2.0),
                None => None,
            },
            cache_write_input_cost_per_token_above_threshold: match write {
                Some(w) => Some(w * 2.0),
                None => None,
            },
        }
    }
}

/// Pricing definition for a Claude model (USD per token).
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudePricing {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
    pub cache_creation_input_cost_per_token: f64,
    pub cache_read_input_cost_per_token: f64,
    pub threshold_tokens: Option<i64>,
    pub input_cost_per_token_above_threshold: Option<f64>,
    pub output_cost_per_token_above_threshold: Option<f64>,
    pub cache_creation_input_cost_per_token_above_threshold: Option<f64>,
    pub cache_read_input_cost_per_token_above_threshold: Option<f64>,
}

impl ClaudePricing {
    pub const fn flat(input: f64, output: f64, cache_creation: f64, cache_read: f64) -> Self {
        Self {
            input_cost_per_token: input,
            output_cost_per_token: output,
            cache_creation_input_cost_per_token: cache_creation,
            cache_read_input_cost_per_token: cache_read,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_creation_input_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub const fn threshold_200k(
        input: f64,
        output: f64,
        cache_creation: f64,
        cache_read: f64,
        input_above: f64,
        output_above: f64,
        cache_creation_above: f64,
        cache_read_above: f64,
    ) -> Self {
        Self {
            input_cost_per_token: input,
            output_cost_per_token: output,
            cache_creation_input_cost_per_token: cache_creation,
            cache_read_input_cost_per_token: cache_read,
            threshold_tokens: Some(200_000),
            input_cost_per_token_above_threshold: Some(input_above),
            output_cost_per_token_above_threshold: Some(output_above),
            cache_creation_input_cost_per_token_above_threshold: Some(cache_creation_above),
            cache_read_input_cost_per_token_above_threshold: Some(cache_read_above),
        }
    }
}

/// Token counts for a single Claude assistant request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClaudeCostTokens {
    pub input: i64,
    pub cache_read: i64,
    pub cache_creation: i64,
    pub cache_creation_1h: i64,
    pub output: i64,
}

const BUILTIN_CODEX_MODELS: &[(&str, CodexPricing)] = &[
    (
        "gpt-5",
        CodexPricing::standard(1.25e-6, 1e-5, Some(1.25e-7)),
    ),
    (
        "gpt-5-codex",
        CodexPricing::standard(1.25e-6, 1e-5, Some(1.25e-7)),
    ),
    (
        "gpt-5-mini",
        CodexPricing::standard(2.5e-7, 2e-6, Some(2.5e-8)),
    ),
    ("gpt-5-nano", CodexPricing::standard(5e-8, 4e-7, Some(5e-9))),
    ("gpt-5-pro", CodexPricing::standard(1.5e-5, 1.2e-4, None)),
    (
        "gpt-5.1",
        CodexPricing::standard(1.25e-6, 1e-5, Some(1.25e-7)),
    ),
    (
        "gpt-5.1-codex",
        CodexPricing::standard(1.25e-6, 1e-5, Some(1.25e-7)),
    ),
    (
        "gpt-5.1-codex-max",
        CodexPricing::standard(1.25e-6, 1e-5, Some(1.25e-7)),
    ),
    (
        "gpt-5.1-codex-mini",
        CodexPricing::standard(2.5e-7, 2e-6, Some(2.5e-8)),
    ),
    (
        "gpt-5.2",
        CodexPricing::standard(1.75e-6, 1.4e-5, Some(1.75e-7)),
    ),
    (
        "gpt-5.2-codex",
        CodexPricing::standard(1.75e-6, 1.4e-5, Some(1.75e-7)),
    ),
    ("gpt-5.2-pro", CodexPricing::standard(2.1e-5, 1.68e-4, None)),
    (
        "gpt-5.3-codex",
        CodexPricing::standard(1.75e-6, 1.4e-5, Some(1.75e-7)),
    ),
    (
        "gpt-5.3-codex-spark",
        CodexPricing {
            input_cost_per_token: 0.0,
            output_cost_per_token: 0.0,
            cache_read_input_cost_per_token: Some(0.0),
            cache_write_input_cost_per_token: None,
            display_label: Some("Research Preview"),
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
            cache_write_input_cost_per_token_above_threshold: None,
        },
    ),
    (
        "gpt-5.4",
        CodexPricing::long_context(2.5e-6, Some(2.5e-7), None, 1.5e-5, 2.25e-5),
    ),
    (
        "gpt-5.4-mini",
        CodexPricing::standard(7.5e-7, 4.5e-6, Some(7.5e-8)),
    ),
    (
        "gpt-5.4-nano",
        CodexPricing::standard(2e-7, 1.25e-6, Some(2e-8)),
    ),
    (
        "gpt-5.4-pro",
        CodexPricing::long_context(3e-5, None, None, 1.8e-4, 2.7e-4),
    ),
    (
        "gpt-5.5",
        CodexPricing::long_context(5e-6, Some(5e-7), None, 3e-5, 4.5e-5),
    ),
    (
        "gpt-5.5-cyber",
        CodexPricing::standard(1.25e-5, 7.5e-5, Some(1.25e-6)),
    ),
    (
        "gpt-5.5-pro",
        CodexPricing::long_context(3e-5, None, None, 1.8e-4, 2.7e-4),
    ),
    (
        "gpt-5.6-cyber",
        CodexPricing {
            input_cost_per_token: 1.25e-5,
            output_cost_per_token: 7.5e-5,
            cache_read_input_cost_per_token: Some(1.25e-6),
            cache_write_input_cost_per_token: Some(1.5625e-5),
            display_label: None,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
            cache_write_input_cost_per_token_above_threshold: None,
        },
    ),
    (
        "gpt-5.6-luna",
        CodexPricing::long_context(2e-7, Some(2e-8), Some(2.5e-7), 1.2e-6, 1.8e-6),
    ),
    (
        "gpt-5.6-sol",
        CodexPricing::long_context(4e-6, Some(4e-7), Some(5e-6), 2e-5, 3e-5),
    ),
    (
        "gpt-5.6-terra",
        CodexPricing::long_context(2e-6, Some(2e-7), Some(2.5e-6), 1.2e-5, 1.8e-5),
    ),
    (
        "gpt-6-astra",
        CodexPricing::long_context(1e-5, Some(1e-6), Some(1.25e-5), 5e-5, 7.5e-5),
    ),
    (
        "gpt-6-luna",
        CodexPricing::long_context(1e-7, Some(1e-8), Some(1.25e-7), 5e-7, 7.5e-7),
    ),
    (
        "gpt-6-sol",
        CodexPricing::long_context(2e-6, Some(2e-7), Some(2.5e-6), 1e-5, 1.5e-5),
    ),
    (
        "gpt-6.1-sol",
        CodexPricing::long_context(2e-6, Some(1e-7), Some(2.5e-6), 1e-5, 1.5e-5),
    ),
];

const BUILTIN_CLAUDE_MODELS: &[(&str, ClaudePricing)] = &[
    (
        "claude-fable-5",
        ClaudePricing::flat(1e-5, 5e-5, 1.25e-5, 1e-6),
    ),
    (
        "claude-haiku-4-5-20251001",
        ClaudePricing::flat(1e-6, 5e-6, 1.25e-6, 1e-7),
    ),
    (
        "claude-haiku-4-5",
        ClaudePricing::flat(1e-6, 5e-6, 1.25e-6, 1e-7),
    ),
    (
        "claude-opus-4-5-20251101",
        ClaudePricing::flat(5e-6, 2.5e-5, 6.25e-6, 5e-7),
    ),
    (
        "claude-opus-4-5",
        ClaudePricing::flat(5e-6, 2.5e-5, 6.25e-6, 5e-7),
    ),
    (
        "claude-opus-4-6-20260205",
        ClaudePricing::flat(5e-6, 2.5e-5, 6.25e-6, 5e-7),
    ),
    (
        "claude-opus-4-6",
        ClaudePricing::flat(5e-6, 2.5e-5, 6.25e-6, 5e-7),
    ),
    (
        "claude-opus-4-7",
        ClaudePricing::flat(5e-6, 2.5e-5, 6.25e-6, 5e-7),
    ),
    (
        "claude-opus-4-8",
        ClaudePricing::flat(5e-6, 2.5e-5, 6.25e-6, 5e-7),
    ),
    (
        "claude-sonnet-4-5",
        ClaudePricing::threshold_200k(3e-6, 1.5e-5, 3.75e-6, 3e-7, 6e-6, 2.25e-5, 7.5e-6, 6e-7),
    ),
    (
        "claude-sonnet-4-6",
        ClaudePricing::flat(3e-6, 1.5e-5, 3.75e-6, 3e-7),
    ),
    (
        "claude-sonnet-4-5-20250929",
        ClaudePricing::threshold_200k(3e-6, 1.5e-5, 3.75e-6, 3e-7, 6e-6, 2.25e-5, 7.5e-6, 6e-7),
    ),
    (
        "claude-opus-4-20250514",
        ClaudePricing::flat(1.5e-5, 7.5e-5, 1.875e-5, 1.5e-6),
    ),
    (
        "claude-opus-4-1",
        ClaudePricing::flat(1.5e-5, 7.5e-5, 1.875e-5, 1.5e-6),
    ),
    (
        "claude-sonnet-4-20250514",
        ClaudePricing::threshold_200k(3e-6, 1.5e-5, 3.75e-6, 3e-7, 6e-6, 2.25e-5, 7.5e-6, 6e-7),
    ),
];

pub fn builtin_codex_pricing(model: &str) -> Option<CodexPricing> {
    BUILTIN_CODEX_MODELS
        .iter()
        .find(|(k, _)| *k == model)
        .map(|(_, v)| v.clone())
}

pub fn builtin_claude_pricing(model: &str) -> Option<ClaudePricing> {
    BUILTIN_CLAUDE_MODELS
        .iter()
        .find(|(k, _)| *k == model)
        .map(|(_, v)| v.clone())
}

fn historical_codex_pricing(model: &str, pricing_epoch_secs: i64) -> Option<CodexPricing> {
    match model {
        "gpt-5.6-sol" if pricing_epoch_secs < CODEX_SOL_PRICING_CUTOFF_SECS => Some(
            CodexPricing::long_context(5e-6, Some(5e-7), Some(6.25e-6), 3e-5, 4.5e-5),
        ),
        "gpt-5.6-terra" if pricing_epoch_secs < CODEX_GPT56_PRICING_CUTOFF_SECS => Some(
            CodexPricing::long_context(2.5e-6, Some(2.5e-7), Some(3.125e-6), 1.5e-5, 2.25e-5),
        ),
        "gpt-5.6-luna" if pricing_epoch_secs < CODEX_GPT56_PRICING_CUTOFF_SECS => Some(
            CodexPricing::long_context(1e-6, Some(1e-7), Some(1.25e-6), 6e-6, 9e-6),
        ),
        _ => None,
    }
}

fn historical_claude_long_context_pricing(model: &str) -> Option<ClaudePricing> {
    match model {
        "claude-opus-4-6" => Some(ClaudePricing::threshold_200k(
            5e-6, 2.5e-5, 6.25e-6, 5e-7, 1e-5, 3.75e-5, 1.25e-5, 1e-6,
        )),
        "claude-sonnet-4-6" => Some(ClaudePricing::threshold_200k(
            3e-6, 1.5e-5, 3.75e-6, 3e-7, 6e-6, 2.25e-5, 7.5e-6, 6e-7,
        )),
        _ => None,
    }
}

fn strip_dashed_date_suffix(s: &str) -> Option<&str> {
    // Matches `-\d{4}-\d{2}-\d{2}$`
    let bytes = s.as_bytes();
    if bytes.len() <= 11 {
        return None;
    }
    let start = bytes.len() - 11;
    let tail = &bytes[start..];
    if tail[0] == b'-'
        && tail[1..5].iter().all(u8::is_ascii_digit)
        && tail[5] == b'-'
        && tail[6..8].iter().all(u8::is_ascii_digit)
        && tail[8] == b'-'
        && tail[9..11].iter().all(u8::is_ascii_digit)
    {
        Some(&s[..start])
    } else {
        None
    }
}

fn strip_compact_date_suffix(s: &str) -> Option<&str> {
    // Matches `-\d{8}$`
    let bytes = s.as_bytes();
    if bytes.len() <= 9 {
        return None;
    }
    let start = bytes.len() - 9;
    let tail = &bytes[start..];
    if tail[0] == b'-' && tail[1..9].iter().all(u8::is_ascii_digit) {
        Some(&s[..start])
    } else {
        None
    }
}

fn strip_version_suffix(s: &str) -> Option<&str> {
    // Matches `-v\d+:\d+$`
    let colon_pos = s.rfind(':')?;
    let after_colon = &s[colon_pos + 1..];
    if after_colon.is_empty() || !after_colon.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let before_colon = &s[..colon_pos];
    let v_pos = before_colon.rfind("-v")?;
    let between = &before_colon[v_pos + 2..];
    if between.is_empty() || !between.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(&s[..v_pos])
}

/// Normalizes a Codex model identifier to its canonical pricing key.
pub fn normalize_codex_model(raw: &str) -> String {
    let mut trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("openai/") {
        trimmed = rest;
    }
    if trimmed == "gpt-5.6" || trimmed == "gpt-daybreak-blue-latest" {
        return "gpt-5.6-sol".to_string();
    }
    if trimmed == "gpt-reserve" {
        return "gpt-5.6-luna".to_string();
    }
    if trimmed == "gpt-daybreak-red-latest" {
        return "gpt-5.6-cyber".to_string();
    }
    if builtin_codex_pricing(trimmed).is_some() {
        return trimmed.to_string();
    }
    if let Some(base) = strip_dashed_date_suffix(trimmed)
        && builtin_codex_pricing(base).is_some()
    {
        return base.to_string();
    }
    trimmed.to_string()
}

/// Returns true when the Codex model identifier is the unattributed placeholder (`"unknown"`).
pub fn is_codex_unattributed_model(raw: &str) -> bool {
    normalize_codex_model(raw) == CODEX_UNATTRIBUTED_MODEL
}

/// Returns the optional display label for a Codex model (e.g., `"Research Preview"` for `gpt-5.3-codex-spark`).
pub fn codex_display_label(model: &str) -> Option<&'static str> {
    let key = normalize_codex_model(model);
    builtin_codex_pricing(&key).and_then(|p| p.display_label)
}

/// Returns the long-context input token threshold for a Codex model when defined in the built-in table.
pub fn codex_long_context_threshold(model: &str) -> Option<i64> {
    let key = normalize_codex_model(model);
    builtin_codex_pricing(&key).and_then(|p| p.threshold_tokens)
}

/// Normalizes a Claude model identifier to its canonical pricing key.
pub fn normalize_claude_model(raw: &str) -> String {
    let mut trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("anthropic.") {
        trimmed = rest;
    }
    if trimmed.contains("claude-")
        && let Some(dot_idx) = trimmed.rfind('.')
    {
        let tail = &trimmed[dot_idx + 1..];
        if tail.starts_with("claude-") {
            trimmed = tail;
        }
    }
    if let Some(without_v) = strip_version_suffix(trimmed) {
        trimmed = without_v;
    }
    if let Some(base) = strip_compact_date_suffix(trimmed)
        && builtin_claude_pricing(base).is_some()
    {
        return base.to_string();
    }
    trimmed.to_string()
}

/// Returns true when `model_name` routes to the OpenAI/Codex subscription bucket.
pub fn counts_toward_codex_subscription(model_name: &str) -> bool {
    let trimmed = model_name.trim();
    let Some(slash_idx) = trimmed.find('/') else {
        return true;
    };
    let prefix = &trimmed[..slash_idx];
    if prefix.is_empty() {
        return false;
    }
    prefix.trim().eq_ignore_ascii_case("openai")
}

/// Public API Fast multiplier relative to Standard API pricing.
pub fn codex_api_fast_multiplier(model: &str) -> Option<f64> {
    match normalize_codex_model(model).as_str() {
        "gpt-5.4" | "gpt-5.4-mini" | "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna"
        | "gpt-6-astra" => Some(2.0),
        "gpt-5.5" => Some(2.5),
        _ => None,
    }
}

pub fn codex_api_fast_allows_long_context(model: &str) -> bool {
    normalize_codex_model(model) == "gpt-6-astra"
}

fn format_optional_f64_g17(val: Option<f64>) -> String {
    match val {
        None => "nil".to_string(),
        Some(v) => format!("{v}"),
    }
}

/// Returns a deterministic fingerprint of the built-in Codex pricing table and API Fast definition.
pub fn codex_built_in_pricing_fingerprint() -> String {
    let mut parts = vec![
        format!("priorityInputTokenLimit={CODEX_PRIORITY_INPUT_TOKEN_LIMIT}"),
        "fastPricingDefinition=api-fast-usd-v1".to_string(),
    ];
    let mut sorted: Vec<(&str, &CodexPricing)> =
        BUILTIN_CODEX_MODELS.iter().map(|(k, v)| (*k, v)).collect();
    sorted.sort_by_key(|(k, _)| *k);
    for (model, pricing) in sorted {
        parts.push(
            [
                format!("model={model}"),
                format_optional_f64_g17(Some(pricing.input_cost_per_token)),
                format_optional_f64_g17(Some(pricing.output_cost_per_token)),
                format_optional_f64_g17(pricing.cache_read_input_cost_per_token),
                format_optional_f64_g17(pricing.cache_write_input_cost_per_token),
                pricing.display_label.unwrap_or("nil").to_string(),
                pricing
                    .threshold_tokens
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "nil".to_string()),
                format_optional_f64_g17(pricing.input_cost_per_token_above_threshold),
                format_optional_f64_g17(pricing.output_cost_per_token_above_threshold),
                format_optional_f64_g17(pricing.cache_read_input_cost_per_token_above_threshold),
                format_optional_f64_g17(pricing.cache_write_input_cost_per_token_above_threshold),
                format_optional_f64_g17(codex_api_fast_multiplier(model)),
                format!(
                    "fastLongContext={}",
                    codex_api_fast_allows_long_context(model)
                ),
            ]
            .join("|"),
        );
    }
    parts.join("\n")
}

// MARK: - Models.dev Catalog & Target Resolution

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelsDevPricingTarget {
    pub provider_id: String,
    pub model_id: String,
}

impl ModelsDevPricingTarget {
    pub fn resolve(raw_provider_id: &str, raw_model_id: &str) -> Vec<Self> {
        let provider_id = Self::normalized_provider_id(raw_provider_id);
        let model_id = raw_model_id.trim();
        if provider_id.is_empty() || !Self::is_valid_model_id(model_id) {
            return Vec::new();
        }
        let resolved_model_id = Self::model_id_for_provider(model_id, &provider_id);
        if !Self::is_valid_model_id(&resolved_model_id) {
            return Vec::new();
        }
        let mut provider_ids = vec![provider_id.clone()];
        match provider_id.as_str() {
            "kimi-coding" => provider_ids.push("kimi-for-coding".to_string()),
            "opencode-free" => provider_ids.push("opencode".to_string()),
            _ => {}
        }
        provider_ids
            .into_iter()
            .map(|pid| Self {
                provider_id: pid,
                model_id: resolved_model_id.clone(),
            })
            .collect()
    }

    fn normalized_provider_id(raw: &str) -> String {
        let norm = raw.trim().to_ascii_lowercase();
        if norm == "x-ai" {
            "xai".to_string()
        } else {
            norm
        }
    }

    fn model_id_for_provider(model_id: &str, provider_id: &str) -> String {
        let Some(slash_idx) = model_id.find('/') else {
            return model_id.to_string();
        };
        let prefix = &model_id[..slash_idx];
        let remainder = &model_id[slash_idx + 1..];
        if remainder.is_empty() {
            return model_id.to_string();
        }
        if provider_id == "openrouter" {
            if Self::normalized_provider_id(prefix) == "openrouter" {
                return remainder.to_string();
            }
            return model_id.to_string();
        }
        if Self::normalized_provider_id(prefix) == provider_id {
            remainder.to_string()
        } else {
            model_id.to_string()
        }
    }

    fn is_valid_model_id(model_id: &str) -> bool {
        !model_id.is_empty() && !model_id.starts_with('/') && !model_id.ends_with('/')
    }
}

pub fn codex_models_dev_pricing_targets(raw_model: &str) -> Vec<(String, String)> {
    let trimmed = raw_model.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Some(slash_idx) = trimmed.find('/') {
        let route_id = trimmed[..slash_idx].to_ascii_lowercase();
        let model_id = &trimmed[slash_idx + 1..];
        if route_id.is_empty()
            || model_id.is_empty()
            || !CODEX_MODELS_DEV_PROVIDER_IDS.contains(&route_id.as_str())
        {
            return Vec::new();
        }
        let mut targets: Vec<(String, String)> =
            ModelsDevPricingTarget::resolve(&route_id, trimmed)
                .into_iter()
                .map(|t| (t.provider_id, t.model_id))
                .collect();
        if route_id == CODEX_MODELS_DEV_PROVIDER_ID {
            let normalized = normalize_codex_model(model_id);
            if normalized != model_id {
                targets.push((CODEX_MODELS_DEV_PROVIDER_ID.to_string(), normalized));
            }
        }
        return targets;
    }

    let normalized = normalize_codex_model(trimmed);
    let mut targets = vec![(
        CODEX_MODELS_DEV_PROVIDER_ID.to_string(),
        trimmed.to_string(),
    )];
    if normalized != trimmed {
        targets.push((CODEX_MODELS_DEV_PROVIDER_ID.to_string(), normalized));
    }
    targets
}

pub fn claude_models_dev_pricing_targets(raw_model: &str) -> Vec<(String, String)> {
    let mut targets = claude_unaliased_models_dev_pricing_targets(raw_model);
    if targets
        .iter()
        .any(|(pid, mid)| pid == "kimi-for-coding" && mid.trim() == "k3[1m]")
    {
        targets.push(("kimi-for-coding".to_string(), "k3".to_string()));
    }
    targets
}

fn claude_unaliased_models_dev_pricing_targets(raw_model: &str) -> Vec<(String, String)> {
    let trimmed = raw_model.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Some(slash_idx) = trimmed.find('/') {
        let codex_targets = codex_models_dev_pricing_targets(trimmed);
        if !codex_targets.is_empty() {
            return codex_targets;
        }
        let route_id = trimmed[..slash_idx].to_ascii_lowercase();
        let model_id = &trimmed[slash_idx + 1..];
        if route_id.is_empty()
            || model_id.is_empty()
            || !CLAUDE_FIRST_PARTY_MODELS_DEV_PROVIDER_IDS.contains(&route_id.as_str())
        {
            return Vec::new();
        }
        return claude_models_dev_model_ids(model_id)
            .into_iter()
            .map(|mid| (route_id.clone(), mid))
            .collect();
    }

    let provider_ids: Vec<&str> = claude_first_party_models_dev_preferred_provider_ids(trimmed)
        .unwrap_or_else(|| CLAUDE_FIRST_PARTY_MODELS_DEV_PROVIDER_IDS.to_vec());
    let model_ids = claude_models_dev_model_ids(trimmed);
    let mut out = Vec::with_capacity(provider_ids.len() * model_ids.len());
    for pid in provider_ids {
        for mid in &model_ids {
            out.push((pid.to_string(), mid.clone()));
        }
    }
    out
}

fn claude_models_dev_model_ids(raw_model: &str) -> Vec<String> {
    let normalized = normalize_claude_model(raw_model);
    if normalized == raw_model {
        vec![raw_model.to_string()]
    } else {
        vec![raw_model.to_string(), normalized]
    }
}

fn claude_first_party_models_dev_preferred_provider_ids(
    raw_model: &str,
) -> Option<Vec<&'static str>> {
    let model = normalize_claude_model(raw_model).to_ascii_lowercase();
    if model.starts_with("claude-") {
        return Some(vec![CLAUDE_MODELS_DEV_PROVIDER_ID]);
    }
    let openai_reasoning = ["o1", "o3", "o4"]
        .iter()
        .any(|p| model == *p || model.starts_with(&format!("{p}-")));
    if openai_reasoning
        || ["gpt-", "chatgpt-", "text-embedding-"]
            .iter()
            .any(|p| model.starts_with(p))
    {
        return Some(vec!["openai"]);
    }
    if ["gemini-", "gemma-", "deep-research-", "veo-", "lyria-"]
        .iter()
        .any(|p| model.starts_with(p))
    {
        return Some(vec!["google"]);
    }
    if model == "kimi-for-coding" || model == "k3" || model == "k3[1m]" || model.starts_with("k3-")
    {
        return Some(vec!["kimi-for-coding"]);
    }
    if model.starts_with("kimi-") || model.starts_with("moonshot-") {
        return Some(vec!["moonshot", "kimi-for-coding"]);
    }
    if model.starts_with("minimax-") {
        return Some(vec!["minimax"]);
    }
    if model.starts_with("deepseek-") {
        return Some(vec!["deepseek"]);
    }
    None
}

/// Candidate model ID normalizer for `models.dev` lookups.
pub fn models_dev_candidates(raw: &str, preserve_dated_snapshots: bool) -> Vec<String> {
    fn push(candidates: &mut Vec<String>, val: &str) {
        let norm = val.trim();
        if !norm.is_empty() && !candidates.iter().any(|c| c == norm) {
            candidates.push(norm.to_string());
        }
    }

    let mut candidates: Vec<String> = Vec::new();
    let trimmed = raw.trim();
    push(&mut candidates, trimmed);

    if let Some(rest) = trimmed.strip_prefix("openai/") {
        push(&mut candidates, rest);
    }
    if let Some(rest) = trimmed.strip_prefix("anthropic.") {
        push(&mut candidates, rest);
    }
    if trimmed.contains("claude-")
        && let Some(dot_idx) = trimmed.rfind('.')
    {
        let tail = &trimmed[dot_idx + 1..];
        if tail.starts_with("claude-") {
            push(&mut candidates, tail);
        }
    }

    let mut idx = 0;
    while idx < candidates.len() {
        let candidate = candidates[idx].clone();
        if let Some(at_idx) = candidate.find('@') {
            let base = &candidate[..at_idx];
            let suffix = &candidate[at_idx + 1..];
            if suffix.len() == 8 && suffix.bytes().all(|b| b.is_ascii_digit()) {
                push(&mut candidates, &format!("{base}-{suffix}"));
            }
            push(&mut candidates, base);
        } else if candidate.starts_with("claude-") && !candidate.ends_with("@default") {
            push(&mut candidates, &format!("{candidate}@default"));
        }

        if !preserve_dated_snapshots {
            if let Some(base) = strip_dashed_date_suffix(&candidate) {
                push(&mut candidates, base);
            }
            if let Some(base) = strip_compact_date_suffix(&candidate) {
                push(&mut candidates, base);
            }
        }
        if let Some(base) = strip_version_suffix(&candidate) {
            push(&mut candidates, base);
        }
        idx += 1;
    }

    candidates
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelsDevContextTier {
    #[serde(rename = "type")]
    pub tier_type: String,
    pub size: i64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelsDevContextOver200KCost {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<ModelsDevContextTier>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    #[serde(rename = "cache_read", skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(rename = "cache_write", skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelsDevCost {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    #[serde(rename = "cache_read", skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(rename = "cache_write", skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    #[serde(rename = "context_over_200k", skip_serializing_if = "Option::is_none")]
    pub context_over_200k: Option<ModelsDevContextOver200KCost>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tiers: Option<Vec<ModelsDevContextOver200KCost>>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelsDevLimit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelsDevModel {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<ModelsDevCost>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<ModelsDevLimit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelsDevPricingInfo {
    pub provider_id: String,
    pub provider_name: Option<String>,
    pub model_id: String,
    pub model_name: Option<String>,
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
    pub cache_read_input_cost_per_token: Option<f64>,
    pub cache_creation_input_cost_per_token: Option<f64>,
    pub context_window: Option<i64>,
    pub threshold_tokens: Option<i64>,
    pub input_cost_per_token_above_threshold: Option<f64>,
    pub output_cost_per_token_above_threshold: Option<f64>,
    pub cache_read_input_cost_per_token_above_threshold: Option<f64>,
    pub cache_creation_input_cost_per_token_above_threshold: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelsDevPricingLookup {
    pub pricing: ModelsDevPricingInfo,
    pub normalized_model_id: String,
}

impl ModelsDevModel {
    pub fn is_priceable(&self) -> bool {
        self.cost
            .as_ref()
            .is_some_and(|c| c.input.is_some() && c.output.is_some())
    }

    pub fn pricing(
        &self,
        provider_id: &str,
        provider_name: Option<&str>,
    ) -> Option<ModelsDevPricingInfo> {
        let cost = self.cost.as_ref()?;
        let input = cost.input?;
        let output = cost.output?;
        let unit = 1_000_000.0;
        let normalized_provider_id = provider_id.trim().to_ascii_lowercase();
        let context_over_200k = cost.context_over_200k.as_ref();

        let context_threshold = cost.tiers.as_ref().and_then(|tiers| {
            tiers.iter().find_map(|tier_entry| {
                let tier = tier_entry.tier.as_ref()?;
                if tier.tier_type != "context" || tier.size <= 0 {
                    return None;
                }
                let mut stripped = tier_entry.clone();
                stripped.tier = None;
                if Some(&stripped) == context_over_200k {
                    Some(tier.size)
                } else {
                    None
                }
            })
        });

        let legacy_threshold = if normalized_provider_id == CODEX_MODELS_DEV_PROVIDER_ID {
            codex_long_context_threshold(&self.id)
        } else {
            None
        };

        let threshold_tokens = if context_over_200k.is_none() {
            None
        } else {
            Some(context_threshold.or(legacy_threshold).unwrap_or(200_000))
        };

        Some(ModelsDevPricingInfo {
            provider_id: normalized_provider_id,
            provider_name: provider_name.map(ToString::to_string),
            model_id: self.id.clone(),
            model_name: self.name.clone(),
            input_cost_per_token: input / unit,
            output_cost_per_token: output / unit,
            cache_read_input_cost_per_token: cost.cache_read.map(|v| v / unit),
            cache_creation_input_cost_per_token: cost.cache_write.map(|v| v / unit),
            context_window: self.limit.as_ref().and_then(|l| l.context),
            threshold_tokens,
            input_cost_per_token_above_threshold: context_over_200k
                .and_then(|c| c.input)
                .map(|v| v / unit),
            output_cost_per_token_above_threshold: context_over_200k
                .and_then(|c| c.output)
                .map(|v| v / unit),
            cache_read_input_cost_per_token_above_threshold: context_over_200k
                .and_then(|c| c.cache_read)
                .map(|v| v / unit),
            cache_creation_input_cost_per_token_above_threshold: context_over_200k
                .and_then(|c| c.cache_write)
                .map(|v| v / unit),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ModelsDevProvider {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub models: BTreeMap<String, ModelsDevModel>,
    #[serde(skip)]
    pub map_key: Option<String>,
}

impl ModelsDevProvider {
    pub fn pricing(
        &self,
        raw_model_id: &str,
        exact_model_id: bool,
    ) -> Option<ModelsDevPricingLookup> {
        let candidates = if exact_model_id {
            vec![raw_model_id.trim().to_string()]
        } else {
            models_dev_candidates(raw_model_id, false)
        };
        let provider_id = self.id.as_deref().or(self.map_key.as_deref()).unwrap_or("");
        let provider_name = self.name.as_deref();

        for candidate in &candidates {
            if let Some(model) = self.models.get(candidate)
                && let Some(pricing) = model.pricing(provider_id, provider_name)
            {
                return Some(ModelsDevPricingLookup {
                    pricing,
                    normalized_model_id: candidate.clone(),
                });
            }
            for model in self.models.values() {
                if !model.is_priceable() {
                    continue;
                }
                let norm_id = model.id.trim();
                if norm_id == candidate
                    && let Some(pricing) = model.pricing(provider_id, provider_name)
                {
                    return Some(ModelsDevPricingLookup {
                        pricing,
                        normalized_model_id: norm_id.to_string(),
                    });
                }
            }
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelsDevCatalog {
    pub providers: BTreeMap<String, ModelsDevProvider>,
}

impl ModelsDevCatalog {
    pub fn new(providers: BTreeMap<String, ModelsDevProvider>) -> Self {
        Self { providers }
    }

    pub fn from_json_str(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn pricing(
        &self,
        raw_provider_id: &str,
        raw_model_id: &str,
        exact_model_id: bool,
    ) -> Option<ModelsDevPricingLookup> {
        let provider_id = raw_provider_id.trim().to_ascii_lowercase();
        self.providers
            .get(&provider_id)?
            .pricing(raw_model_id, exact_model_id)
    }

    /// Loads a cached `ModelsDevCatalog` from `<cache_root>/model-pricing/models-dev-v1.json` if present.
    pub fn load_from_cache_root(cache_root: Option<&Path>) -> Option<Self> {
        let root = cache_root?;
        let path = root.join("model-pricing").join("models-dev-v1.json");
        let bytes = std::fs::read(&path).ok()?;
        let val: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        if let Some(obj) = val.as_object()
            && let Some(version) = obj.get("version").and_then(|v| v.as_i64())
        {
            if version != 1 {
                return None;
            }
            if let Some(catalog_val) = obj.get("catalog") {
                return serde_json::from_value(catalog_val.clone()).ok();
            }
        }
        serde_json::from_value(val).ok()
    }
}

impl Serialize for ModelsDevCatalog {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry("providers", &self.providers)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for ModelsDevCatalog {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw_val = serde_json::Value::deserialize(deserializer)?;
        let Some(obj) = raw_val.as_object() else {
            return Ok(Self::default());
        };
        let target_map =
            if let Some(providers_obj) = obj.get("providers").and_then(|v| v.as_object()) {
                providers_obj
            } else {
                obj
            };
        let mut providers = BTreeMap::new();
        for (key, value) in target_map {
            if let Ok(mut provider) = serde_json::from_value::<ModelsDevProvider>(value.clone()) {
                provider.map_key = Some(key.clone());
                let pid = provider
                    .id
                    .as_deref()
                    .unwrap_or(key.as_str())
                    .trim()
                    .to_ascii_lowercase();
                providers.insert(pid, provider);
            }
        }
        Ok(Self { providers })
    }
}

// MARK: - Custom Pricing Overlay (`CostUsageCustomPricing.swift`)

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CustomModelPricingEntry {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
    #[serde(default)]
    pub cache_read_input_cost_per_token: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CustomModelPricing {
    pub models: BTreeMap<String, CustomModelPricingEntry>,
}

impl CustomModelPricing {
    pub fn get(&self, model: &str) -> Option<&CustomModelPricingEntry> {
        let trimmed = model.trim();
        self.models
            .get(trimmed)
            .or_else(|| self.models.get(&normalize_codex_model(trimmed)))
    }
}

// MARK: - Codex & Claude Cost Calculation

pub fn codex_models_dev_lookup(
    raw_model: &str,
    catalog: Option<&ModelsDevCatalog>,
    cache_root: Option<&Path>,
) -> Option<ModelsDevPricingLookup> {
    let loaded_catalog;
    let catalog_ref = match catalog {
        Some(c) => Some(c),
        None => {
            loaded_catalog = ModelsDevCatalog::load_from_cache_root(cache_root);
            loaded_catalog.as_ref()
        }
    }?;

    for (provider_id, model_id) in codex_models_dev_pricing_targets(raw_model) {
        if let Some(lookup) = catalog_ref.pricing(&provider_id, &model_id, false) {
            return Some(lookup);
        }
    }
    None
}

pub fn claude_models_dev_lookup(
    raw_model: &str,
    catalog: Option<&ModelsDevCatalog>,
    cache_root: Option<&Path>,
) -> Option<ModelsDevPricingLookup> {
    let loaded_catalog;
    let catalog_ref = match catalog {
        Some(c) => Some(c),
        None => {
            loaded_catalog = ModelsDevCatalog::load_from_cache_root(cache_root);
            loaded_catalog.as_ref()
        }
    }?;

    let trimmed = raw_model.trim();
    let has_explicit_route = trimmed.contains('/');
    let has_preferred_vendor =
        claude_first_party_models_dev_preferred_provider_ids(trimmed).is_some();
    let mut matches: Vec<ModelsDevPricingLookup> = Vec::new();
    for (provider_id, model_id) in claude_models_dev_pricing_targets(raw_model) {
        if let Some(lookup) = catalog_ref.pricing(&provider_id, &model_id, false) {
            if has_explicit_route || has_preferred_vendor {
                return Some(lookup);
            }
            matches.push(lookup);
        }
    }
    let matched_providers: HashSet<&str> = matches
        .iter()
        .map(|m| m.pricing.provider_id.as_str())
        .collect();
    if matched_providers.len() != 1 {
        return None;
    }
    matches.into_iter().next()
}

pub fn resolved_codex_pricing(
    model: &str,
    pricing_epoch_secs: Option<i64>,
    catalog: Option<&ModelsDevCatalog>,
    cache_root: Option<&Path>,
) -> Option<CodexPricing> {
    let key = normalize_codex_model(model);
    if key == CODEX_UNATTRIBUTED_MODEL {
        return None;
    }
    if let Some(epoch) = pricing_epoch_secs
        && let Some(historical) = historical_codex_pricing(&key, epoch)
    {
        return Some(historical);
    }

    if let Some(lookup) = codex_models_dev_lookup(model, catalog, cache_root) {
        let bundled = if lookup.pricing.provider_id == CODEX_MODELS_DEV_PROVIDER_ID {
            builtin_codex_pricing(&key)
        } else {
            None
        };
        let bundled_long_context = if lookup.pricing.threshold_tokens.is_none() {
            bundled.as_ref()
        } else {
            None
        };
        let cache_read_above = lookup
            .pricing
            .cache_read_input_cost_per_token_above_threshold
            .or_else(|| {
                if lookup.pricing.threshold_tokens.is_some() {
                    lookup
                        .pricing
                        .cache_read_input_cost_per_token
                        .or(lookup.pricing.input_cost_per_token_above_threshold)
                        .or(Some(lookup.pricing.input_cost_per_token))
                } else {
                    bundled_long_context
                        .and_then(|b| b.cache_read_input_cost_per_token_above_threshold)
                }
            });
        let cache_write_above = lookup
            .pricing
            .cache_creation_input_cost_per_token_above_threshold
            .or_else(|| {
                if lookup.pricing.threshold_tokens.is_some() {
                    lookup
                        .pricing
                        .cache_creation_input_cost_per_token
                        .or(lookup.pricing.input_cost_per_token_above_threshold)
                        .or(Some(lookup.pricing.input_cost_per_token))
                } else {
                    bundled_long_context
                        .and_then(|b| b.cache_write_input_cost_per_token_above_threshold)
                }
            });
        return Some(CodexPricing {
            input_cost_per_token: lookup.pricing.input_cost_per_token,
            output_cost_per_token: lookup.pricing.output_cost_per_token,
            cache_read_input_cost_per_token: lookup
                .pricing
                .cache_read_input_cost_per_token
                .or_else(|| {
                    bundled
                        .as_ref()
                        .and_then(|b| b.cache_read_input_cost_per_token)
                }),
            cache_write_input_cost_per_token: lookup
                .pricing
                .cache_creation_input_cost_per_token
                .or_else(|| {
                    bundled
                        .as_ref()
                        .and_then(|b| b.cache_write_input_cost_per_token)
                }),
            display_label: bundled.as_ref().and_then(|b| b.display_label),
            threshold_tokens: lookup
                .pricing
                .threshold_tokens
                .or_else(|| bundled.as_ref().and_then(|b| b.threshold_tokens)),
            input_cost_per_token_above_threshold: lookup
                .pricing
                .input_cost_per_token_above_threshold
                .or_else(|| {
                    bundled_long_context.and_then(|b| b.input_cost_per_token_above_threshold)
                }),
            output_cost_per_token_above_threshold: lookup
                .pricing
                .output_cost_per_token_above_threshold
                .or_else(|| {
                    bundled_long_context.and_then(|b| b.output_cost_per_token_above_threshold)
                }),
            cache_read_input_cost_per_token_above_threshold: cache_read_above,
            cache_write_input_cost_per_token_above_threshold: cache_write_above,
        });
    }

    if model.contains('/') {
        let trimmed = model.trim();
        if let Some(slash_idx) = trimmed.find('/') {
            let prefix = &trimmed[..slash_idx];
            if !prefix.eq_ignore_ascii_case("openai") {
                return None;
            }
        }
    }

    builtin_codex_pricing(&key)
}

pub fn codex_cost_usd_with_pricing(
    pricing: &CodexPricing,
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_input_tokens: i64,
    output_tokens: i64,
) -> f64 {
    let total_input = input_tokens.max(0);
    let cached = cached_input_tokens.max(0).min(total_input);
    let remaining_after_cache = total_input - cached;
    let cache_write = cache_write_input_tokens.max(0).min(remaining_after_cache);
    let non_cached = remaining_after_cache - cache_write;
    let cached_rate = pricing
        .cache_read_input_cost_per_token
        .unwrap_or(pricing.input_cost_per_token);

    let uses_long_context_rates = pricing
        .threshold_tokens
        .is_some_and(|threshold| total_input > threshold);
    let input_rate = if uses_long_context_rates {
        pricing
            .input_cost_per_token_above_threshold
            .unwrap_or(pricing.input_cost_per_token)
    } else {
        pricing.input_cost_per_token
    };
    let cached_input_rate = if uses_long_context_rates {
        pricing
            .cache_read_input_cost_per_token_above_threshold
            .or(pricing.cache_read_input_cost_per_token)
            .unwrap_or(input_rate)
    } else {
        cached_rate
    };
    let cache_write_rate = if uses_long_context_rates {
        pricing
            .cache_write_input_cost_per_token_above_threshold
            .or(pricing.cache_write_input_cost_per_token)
            .unwrap_or(input_rate)
    } else {
        pricing
            .cache_write_input_cost_per_token
            .unwrap_or(input_rate)
    };
    let output_rate = if uses_long_context_rates {
        pricing
            .output_cost_per_token_above_threshold
            .unwrap_or(pricing.output_cost_per_token)
    } else {
        pricing.output_cost_per_token
    };

    (non_cached as f64 * input_rate)
        + (cached as f64 * cached_input_rate)
        + (cache_write as f64 * cache_write_rate)
        + (output_tokens.max(0) as f64 * output_rate)
}

/// Computes the standard Codex USD cost for a single request (or safe below-threshold aggregate).
#[allow(clippy::too_many_arguments)]
pub fn codex_cost_usd(
    model: &str,
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
    cache_write_input_tokens: i64,
    pricing_epoch_secs: Option<i64>,
    catalog: Option<&ModelsDevCatalog>,
    cache_root: Option<&Path>,
    custom_pricing: Option<&CustomModelPricing>,
    aggregate: bool,
) -> Option<f64> {
    if let Some(custom) = custom_pricing
        && let Some(entry) = custom.get(model)
    {
        let p = CodexPricing::standard(
            entry.input_cost_per_token,
            entry.output_cost_per_token,
            entry.cache_read_input_cost_per_token,
        );
        return Some(codex_cost_usd_with_pricing(
            &p,
            input_tokens,
            cached_input_tokens,
            cache_write_input_tokens,
            output_tokens,
        ));
    }

    let pricing = resolved_codex_pricing(model, pricing_epoch_secs, catalog, cache_root)?;
    if aggregate
        && let Some(threshold) = pricing.threshold_tokens
        && input_tokens.max(0) > threshold
    {
        return None;
    }

    Some(codex_cost_usd_with_pricing(
        &pricing,
        input_tokens,
        cached_input_tokens,
        cache_write_input_tokens,
        output_tokens,
    ))
}

/// Service tier for Codex requests (`Standard`, `Priority`, `Fast`, `Flex`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodexServiceTier {
    Standard,
    Priority,
    Fast,
    Flex,
}

/// Normalizes an optional Codex service-tier string (`"priority"`, `"fast"`, `"standard"`, `"default"`, `"flex"`).
#[must_use]
pub fn normalize_codex_service_tier(raw: Option<&str>) -> Option<CodexServiceTier> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "priority" => Some(CodexServiceTier::Priority),
        "fast" => Some(CodexServiceTier::Fast),
        "standard" | "default" => Some(CodexServiceTier::Standard),
        "flex" => Some(CodexServiceTier::Flex),
        _ => None,
    }
}

/// Computes the Codex USD cost for `(input_tokens, cached_input_tokens, output_tokens)`
/// and an optional `service_tier`.
#[must_use]
pub fn codex_cost_usd_with_tier(
    model: &str,
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
    service_tier: Option<CodexServiceTier>,
    catalog: Option<&ModelsDevCatalog>,
) -> Option<f64> {
    codex_cost_usd_with_tier_and_epoch(
        model,
        input_tokens,
        cached_input_tokens,
        output_tokens,
        service_tier,
        None,
        catalog,
    )
}

/// Computes the Codex USD cost for `(input_tokens, cached_input_tokens, output_tokens)`,
/// an optional `service_tier`, and an optional `pricing_epoch_secs` timestamp.
#[must_use]
pub fn codex_cost_usd_with_tier_and_epoch(
    model: &str,
    input_tokens: i64,
    cached_input_tokens: i64,
    output_tokens: i64,
    service_tier: Option<CodexServiceTier>,
    pricing_epoch_secs: Option<i64>,
    catalog: Option<&ModelsDevCatalog>,
) -> Option<f64> {
    let norm = normalize_codex_model(model);
    if norm == "gpt-5.3-codex-spark" || norm == CODEX_UNATTRIBUTED_MODEL {
        return None;
    }
    let total_input = input_tokens.max(0);
    match service_tier {
        Some(CodexServiceTier::Priority | CodexServiceTier::Fast) => codex_priority_cost_usd(
            model,
            total_input,
            cached_input_tokens.max(0),
            0,
            output_tokens.max(0),
            pricing_epoch_secs,
            catalog,
            None,
            None,
        ),
        _ => codex_cost_usd(
            model,
            total_input,
            cached_input_tokens.max(0),
            output_tokens.max(0),
            0,
            pricing_epoch_secs,
            catalog,
            None,
            None,
            false,
        ),
    }
}

/// Computes the Priority / API Fast Codex USD cost for a single request.
#[allow(clippy::too_many_arguments)]
pub fn codex_priority_cost_usd(
    model: &str,
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_input_tokens: i64,
    output_tokens: i64,
    pricing_epoch_secs: Option<i64>,
    catalog: Option<&ModelsDevCatalog>,
    cache_root: Option<&Path>,
    custom_pricing: Option<&CustomModelPricing>,
) -> Option<f64> {
    let multiplier = codex_api_fast_multiplier(model)?;
    if input_tokens.max(0) > CODEX_PRIORITY_INPUT_TOKEN_LIMIT
        && !codex_api_fast_allows_long_context(model)
    {
        return None;
    }
    codex_cost_usd(
        model,
        input_tokens,
        cached_input_tokens,
        output_tokens,
        cache_write_input_tokens,
        pricing_epoch_secs,
        catalog,
        cache_root,
        custom_pricing,
        false,
    )
    .map(|c| c * multiplier)
}

pub fn claude_cost_usd_with_pricing(pricing: &ClaudePricing, tokens: ClaudeCostTokens) -> f64 {
    let input = tokens.input.max(0);
    let cache_read = tokens.cache_read.max(0);
    let cache_creation_total = tokens.cache_creation.max(0);
    let cache_creation_1h = tokens.cache_creation_1h.max(0).min(cache_creation_total);
    let cache_creation_5m = cache_creation_total - cache_creation_1h;

    let uses_long_context_rates = pricing.threshold_tokens.is_some_and(|threshold| {
        match input
            .checked_add(cache_read)
            .and_then(|s| s.checked_add(cache_creation_total))
        {
            Some(sum) => sum > threshold,
            None => true,
        }
    });

    let input_rate = if uses_long_context_rates {
        pricing
            .input_cost_per_token_above_threshold
            .unwrap_or(pricing.input_cost_per_token)
    } else {
        pricing.input_cost_per_token
    };
    let cache_read_rate = if uses_long_context_rates {
        pricing
            .cache_read_input_cost_per_token_above_threshold
            .unwrap_or(pricing.cache_read_input_cost_per_token)
    } else {
        pricing.cache_read_input_cost_per_token
    };
    let cache_creation_5m_rate = if uses_long_context_rates {
        pricing
            .cache_creation_input_cost_per_token_above_threshold
            .unwrap_or(pricing.cache_creation_input_cost_per_token)
    } else {
        pricing.cache_creation_input_cost_per_token
    };
    let output_rate = if uses_long_context_rates {
        pricing
            .output_cost_per_token_above_threshold
            .unwrap_or(pricing.output_cost_per_token)
    } else {
        pricing.output_cost_per_token
    };

    (input as f64 * input_rate)
        + (cache_read as f64 * cache_read_rate)
        + (cache_creation_5m as f64 * cache_creation_5m_rate)
        + (cache_creation_1h as f64 * input_rate * 2.0)
        + (tokens.output.max(0) as f64 * output_rate)
}

pub fn claude_cost_usd_with_models_dev_info(
    pricing: &ModelsDevPricingInfo,
    tokens: ClaudeCostTokens,
) -> f64 {
    let bundled_threshold = if pricing.provider_id == CODEX_MODELS_DEV_PROVIDER_ID {
        codex_long_context_threshold(&pricing.model_id)
    } else {
        None
    };
    let claude_pricing = ClaudePricing {
        input_cost_per_token: pricing.input_cost_per_token,
        output_cost_per_token: pricing.output_cost_per_token,
        cache_creation_input_cost_per_token: pricing
            .cache_creation_input_cost_per_token
            .unwrap_or(pricing.input_cost_per_token),
        cache_read_input_cost_per_token: pricing
            .cache_read_input_cost_per_token
            .unwrap_or(pricing.input_cost_per_token),
        threshold_tokens: bundled_threshold.or(pricing.threshold_tokens),
        input_cost_per_token_above_threshold: pricing.input_cost_per_token_above_threshold,
        output_cost_per_token_above_threshold: pricing.output_cost_per_token_above_threshold,
        cache_creation_input_cost_per_token_above_threshold: pricing
            .cache_creation_input_cost_per_token_above_threshold,
        cache_read_input_cost_per_token_above_threshold: pricing
            .cache_read_input_cost_per_token_above_threshold,
    };
    claude_cost_usd_with_pricing(&claude_pricing, tokens)
}

/// Computes the Claude USD cost for a single assistant message.
#[allow(clippy::too_many_arguments)]
pub fn claude_cost_usd(
    model: &str,
    tokens: ClaudeCostTokens,
    pricing_epoch_secs: Option<i64>,
    catalog: Option<&ModelsDevCatalog>,
    cache_root: Option<&Path>,
) -> Option<f64> {
    let key = normalize_claude_model(model);
    if let Some(epoch) = pricing_epoch_secs
        && let Some(historical) = historical_claude_long_context_pricing(&key)
        && let Some(current) = builtin_claude_pricing(&key)
    {
        let selected = if epoch < CLAUDE_FULL_CONTEXT_STANDARD_PRICING_CUTOFF_SECS {
            &historical
        } else {
            &current
        };
        return Some(claude_cost_usd_with_pricing(selected, tokens));
    }

    if let Some(lookup) = claude_models_dev_lookup(model, catalog, cache_root) {
        return Some(claude_cost_usd_with_models_dev_info(
            &lookup.pricing,
            tokens,
        ));
    }

    let pricing = builtin_claude_pricing(&key)?;
    Some(claude_cost_usd_with_pricing(&pricing, tokens))
}

/// Converts a USD cost to rounded integer nano-dollars (`1e-9` USD).
pub fn usd_to_nanos(cost_usd: f64) -> i64 {
    if !cost_usd.is_finite() || cost_usd <= 0.0 {
        return 0;
    }
    (cost_usd * COST_SCALE_NANOS).round() as i64
}

/// Converts integer nano-dollars (`1e-9` USD) back to `f64` USD.
pub fn nanos_to_usd(nanos: i64) -> f64 {
    (nanos as f64) / COST_SCALE_NANOS
}

/// Deterministic pricing key fingerprint for cache invalidation.
pub fn codex_pricing_key(catalog: Option<&ModelsDevCatalog>, formula_version: u32) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("formula={formula_version}\n").as_bytes());
    hasher.update(codex_built_in_pricing_fingerprint().as_bytes());
    if let Some(cat) = catalog
        && let Ok(bytes) = serde_json::to_vec(cat)
    {
        hasher.update(b"\ncatalog=");
        hasher.update(&bytes);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{b:02x}");
    }
    hex
}

/// Per-scan memoized Claude pricing resolver (`CostUsagePricing.ClaudeResolver`).
#[derive(Debug, Default)]
pub struct ClaudePricingResolver {
    catalog: ModelsDevCatalog,
    normalized_models: HashMap<String, String>,
    lookups: HashMap<String, Option<ModelsDevPricingLookup>>,
}

impl ClaudePricingResolver {
    pub const MEMO_ENTRY_LIMIT: usize = 1024;

    pub fn new(catalog: Option<ModelsDevCatalog>, cache_root: Option<&Path>) -> Self {
        let resolved = catalog
            .or_else(|| ModelsDevCatalog::load_from_cache_root(cache_root))
            .unwrap_or_default();
        Self {
            catalog: resolved,
            normalized_models: HashMap::new(),
            lookups: HashMap::new(),
        }
    }

    pub fn normalize(&mut self, model: &str) -> String {
        if let Some(existing) = self.normalized_models.get(model) {
            return existing.clone();
        }
        let norm = normalize_claude_model(model);
        if self.normalized_models.len() < Self::MEMO_ENTRY_LIMIT {
            self.normalized_models
                .insert(model.to_string(), norm.clone());
        }
        norm
    }

    pub fn lookup(&mut self, model: &str) -> Option<ModelsDevPricingLookup> {
        if let Some(existing) = self.lookups.get(model) {
            return existing.clone();
        }
        let value = claude_models_dev_lookup(model, Some(&self.catalog), None);
        if self.lookups.len() < Self::MEMO_ENTRY_LIMIT {
            self.lookups.insert(model.to_string(), value.clone());
        }
        value
    }

    pub fn cost_usd(
        &mut self,
        model: &str,
        provider_id: Option<&str>,
        tokens: ClaudeCostTokens,
        pricing_epoch_secs: Option<i64>,
    ) -> Option<f64> {
        if let Some(pid) = provider_id {
            let lookup = self.catalog.pricing(pid, model, false)?;
            return Some(claude_cost_usd_with_models_dev_info(
                &lookup.pricing,
                tokens,
            ));
        }
        let key = self.normalize(model);
        if let Some(epoch) = pricing_epoch_secs
            && let Some(historical) = historical_claude_long_context_pricing(&key)
            && let Some(current) = builtin_claude_pricing(&key)
        {
            let selected = if epoch < CLAUDE_FULL_CONTEXT_STANDARD_PRICING_CUTOFF_SECS {
                &historical
            } else {
                &current
            };
            return Some(claude_cost_usd_with_pricing(selected, tokens));
        }
        if let Some(lookup) = self.lookup(model) {
            return Some(claude_cost_usd_with_models_dev_info(
                &lookup.pricing,
                tokens,
            ));
        }
        let pricing = builtin_claude_pricing(&key)?;
        Some(claude_cost_usd_with_pricing(&pricing, tokens))
    }
}
