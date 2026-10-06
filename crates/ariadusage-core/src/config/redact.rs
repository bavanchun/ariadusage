// Ported from CodexBar Sources/CodexBarCore/Config/CodexBarConfig.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/TokenAccounts.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;

use serde_json::value::RawValue;

use super::types::{Config, ProviderEntry};

pub const REDACTED_PLACEHOLDER: &str = "[REDACTED]";

fn redacted_raw() -> Box<RawValue> {
    RawValue::from_string(format!("\"{REDACTED_PLACEHOLDER}\""))
        .expect("static placeholder is valid JSON")
}

/// Sanitizes configuration for export or dump inspection:
/// - In typed provider entries, `apiKey`, `cookieHeader`, and `secretKey` become `"[REDACTED]"`.
/// - Each value in `pluginSecrets` becomes `"[REDACTED]"` if an object; any non-object value becomes `"[REDACTED]"`.
/// - In token accounts, the `token` field becomes `"[REDACTED]"`.
/// - In opaque provider entries, every value is redacted except the real `id` and `enabled`.
///   If an opaque entry's raw value is not a JSON object, the whole entry is redacted.
/// - Hooks, settings, and unknown top-level keys hold no secrets by contract and are passed through unchanged.
/// - If `show_secrets` is true, the configuration is returned unmodified.
pub fn sanitized_for_dump(config: &Config, show_secrets: bool) -> Config {
    if show_secrets {
        return config.clone();
    }

    let mut copy = config.clone();

    for entry in &mut copy.providers {
        match entry {
            ProviderEntry::Typed(cfg) => {
                if cfg.extra.contains_key("apiKey") {
                    cfg.extra.insert("apiKey".to_string(), redacted_raw());
                }
                if cfg.extra.contains_key("cookieHeader") {
                    cfg.extra.insert("cookieHeader".to_string(), redacted_raw());
                }
                if cfg.extra.contains_key("secretKey") {
                    cfg.extra.insert("secretKey".to_string(), redacted_raw());
                }
                if let Some(raw_ps) = cfg.extra.get("pluginSecrets") {
                    let new_raw = match serde_json::from_str::<serde_json::Value>(raw_ps.get()) {
                        Ok(serde_json::Value::Object(map)) => {
                            let redacted: BTreeMap<String, String> = map
                                .into_iter()
                                .map(|(k, _)| (k, REDACTED_PLACEHOLDER.to_string()))
                                .collect();
                            serde_json::to_string(&redacted)
                                .ok()
                                .and_then(|s| RawValue::from_string(s).ok())
                                .unwrap_or_else(redacted_raw)
                        }
                        _ => redacted_raw(),
                    };
                    cfg.extra.insert("pluginSecrets".to_string(), new_raw);
                }

                if let Some(ref mut ta) = cfg.token_accounts {
                    for account in &mut ta.accounts {
                        if account.extra.contains_key("token") {
                            account.extra.insert("token".to_string(), redacted_raw());
                        }
                    }
                }
            }
            ProviderEntry::Opaque { id, enabled, raw } => {
                let new_raw = if let Ok(map) =
                    serde_json::from_str::<BTreeMap<String, Box<RawValue>>>(raw.get())
                {
                    let mut redacted = BTreeMap::new();
                    for key in map.keys() {
                        redacted.insert(key.clone(), redacted_raw());
                    }
                    if let Ok(escaped_id) = serde_json::to_string(id)
                        && let Ok(raw_id) = RawValue::from_string(escaped_id)
                    {
                        redacted.insert("id".to_string(), raw_id);
                    }
                    if let Ok(raw_enabled) = RawValue::from_string(if *enabled {
                        "true".to_string()
                    } else {
                        "false".to_string()
                    }) {
                        redacted.insert("enabled".to_string(), raw_enabled);
                    }

                    serde_json::to_string(&redacted)
                        .ok()
                        .and_then(|s| RawValue::from_string(s).ok())
                        .unwrap_or_else(redacted_raw)
                } else {
                    redacted_raw()
                };
                *raw = new_raw;
            }
        }
    }

    copy
}
