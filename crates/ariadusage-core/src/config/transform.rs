// Ported from CodexBar Sources/CodexBarCore/Config/CodexBarConfig.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;

use serde_json::value::RawValue;

use super::types::{Config, ProviderConfig, ProviderEntry};
use crate::providers;

/// Toggles or sets the enablement state for a provider entry in the configuration.
///
/// If the provider is already present, its enablement is updated in-place (updating
/// raw bytes if opaque). If the provider is not present, a new entry is appended.
pub fn set_provider_enabled(config: &mut Config, id: &str, enabled: bool) {
    if let Some(entry) = config.providers.iter_mut().find(|p| p.id() == id) {
        match entry {
            ProviderEntry::Typed(cfg) => {
                cfg.enabled = Some(enabled);
            }
            ProviderEntry::Opaque {
                enabled: e, raw, ..
            } => {
                *e = enabled;
                let update = || -> Option<Box<RawValue>> {
                    let mut map: BTreeMap<String, Box<RawValue>> =
                        serde_json::from_str(raw.get()).ok()?;
                    let serialized_val = serde_json::to_string(&enabled).ok()?;
                    let raw_val = RawValue::from_string(serialized_val).ok()?;
                    map.insert("enabled".to_string(), raw_val);
                    let serialized_map = serde_json::to_string(&map).ok()?;
                    RawValue::from_string(serialized_map).ok()
                };
                if let Some(new_raw) = update() {
                    *raw = new_raw;
                } else if let Ok(new_raw) = RawValue::from_string(
                    serde_json::json!({
                        "id": id,
                        "enabled": enabled,
                    })
                    .to_string(),
                ) {
                    *raw = new_raw;
                }
            }
        }
    } else if let Some(descriptor) = providers::find_by_id_str(id) {
        let mut cfg = ProviderConfig::new(descriptor.id.clone());
        cfg.enabled = Some(enabled);
        config.providers.push(ProviderEntry::Typed(cfg));
    } else {
        let json_str = serde_json::json!({
            "id": id,
            "enabled": enabled,
        })
        .to_string();
        if let Ok(raw) = RawValue::from_string(json_str) {
            config.providers.push(ProviderEntry::Opaque {
                id: id.to_string(),
                enabled,
                raw,
            });
        }
    }
}
