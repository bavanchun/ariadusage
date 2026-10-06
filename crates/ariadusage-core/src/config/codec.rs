// Ported from CodexBar Sources/CodexBarCore/Config/CodexBarConfig.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Config/OpaqueConfigJSON.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use serde::de::{MapAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

use super::quota::QuotaWarnings;
use super::types::{
    Config, CookieSource, ProviderConfig, ProviderEntry, TokenAccountMeta, TokenAccountsMeta,
};
use crate::providers::{self, SourceMode};

/// Errors encountered while decoding configuration bytes.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("JSON decode error: {message} at line {line}, column {column}")]
    Decode {
        message: &'static str,
        line: usize,
        column: usize,
    },
    #[error("Duplicate top-level key: {key}")]
    DuplicateKey { key: String },
    #[error("Missing required key: {key}")]
    MissingKey { key: &'static str },
    #[error("Unknown source mode: {0}")]
    UnknownSource(String),
    #[error("Invalid provider ID: {0}")]
    InvalidProviderId(String),
}

struct RawRoot {
    version: Option<u32>,
    providers: Option<Vec<Box<RawValue>>>,
    hooks: Option<Box<RawValue>>,
    settings: Option<Box<RawValue>>,
    extra_top: BTreeMap<String, Box<RawValue>>,
}

struct RawRootVisitor;

impl<'de> Visitor<'de> for RawRootVisitor {
    type Value = RawRoot;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object representing a configuration document")
    }

    fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut seen = HashSet::new();
        let mut version = None;
        let mut providers = None;
        let mut hooks = None;
        let mut settings = None;
        let mut extra_top = BTreeMap::new();

        while let Some(key) = access.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(serde::de::Error::custom(format!(
                    "CONFIG_ERR:Duplicate top-level key: {key}"
                )));
            }
            match key.as_str() {
                "version" => {
                    version = Some(access.next_value::<u32>()?);
                }
                "providers" => {
                    providers = Some(access.next_value::<Vec<Box<RawValue>>>()?);
                }
                "hooks" => {
                    hooks = Some(access.next_value::<Box<RawValue>>()?);
                }
                "settings" => {
                    settings = Some(access.next_value::<Box<RawValue>>()?);
                }
                _ => {
                    let val = access.next_value::<Box<RawValue>>()?;
                    extra_top.insert(key, val);
                }
            }
        }

        let Some(version) = version else {
            return Err(serde::de::Error::custom(
                "CONFIG_ERR:Missing required key: version",
            ));
        };
        let Some(providers) = providers else {
            return Err(serde::de::Error::custom(
                "CONFIG_ERR:Missing required key: providers",
            ));
        };

        Ok(RawRoot {
            version: Some(version),
            providers: Some(providers),
            hooks,
            settings,
            extra_top,
        })
    }
}

impl<'de> Deserialize<'de> for RawRoot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(RawRootVisitor)
    }
}

/// Decodes configuration bytes into a `Config` instance.
///
/// Strips a leading UTF-8 BOM if present. Blank input (only spaces, tabs,
/// newlines, carriage returns) returns `Ok(None)`.
pub fn decode(bytes: &[u8]) -> Result<Option<Config>, ConfigError> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);

    if bytes
        .iter()
        .all(|&b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
    {
        return Ok(None);
    }

    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let raw_root: RawRoot = match RawRoot::deserialize(&mut deserializer) {
        Ok(root) => root,
        Err(e) => {
            let msg = e.to_string();
            if let Some(pos) = msg.find("CONFIG_ERR:") {
                let rest = &msg[pos + "CONFIG_ERR:".len()..];
                if let Some(key_part) = rest.strip_prefix("Duplicate top-level key: ") {
                    let key = key_part.split(" at line").next().unwrap_or(key_part).trim();
                    return Err(ConfigError::DuplicateKey {
                        key: key.to_string(),
                    });
                }
                if let Some(key_part) = rest.strip_prefix("Missing required key: ") {
                    let key = key_part.split(" at line").next().unwrap_or(key_part).trim();
                    let static_key = match key {
                        "version" => "version",
                        "providers" => "providers",
                        _ => "unknown",
                    };
                    return Err(ConfigError::MissingKey { key: static_key });
                }
            }
            return Err(ConfigError::Decode {
                message: "Malformed JSON or schema error",
                line: e.line(),
                column: e.column(),
            });
        }
    };

    let version = raw_root.version.unwrap_or(1);
    let raw_providers = raw_root.providers.unwrap_or_default();

    let mut providers = Vec::with_capacity(raw_providers.len());
    for raw_entry in raw_providers {
        let entry_str = raw_entry.get();
        let mut map: BTreeMap<String, Box<RawValue>> =
            serde_json::from_str(entry_str).map_err(|e| ConfigError::Decode {
                message: "Provider entry is not a valid JSON object",
                line: e.line(),
                column: e.column(),
            })?;

        let raw_id = map.remove("id");
        let id_str = match raw_id {
            Some(ref r) => serde_json::from_str::<String>(r.get()).unwrap_or_default(),
            None => String::new(),
        };

        if let Some(descriptor) = providers::find_by_id_str(&id_str) {
            let enabled = if let Some(raw_enabled) = map.remove("enabled") {
                Some(
                    serde_json::from_str::<bool>(raw_enabled.get()).map_err(|e| {
                        ConfigError::Decode {
                            message: "Invalid enabled field",
                            line: e.line(),
                            column: e.column(),
                        }
                    })?,
                )
            } else {
                None
            };

            let source = if let Some(raw_source) = map.remove("source") {
                let s_str = serde_json::from_str::<String>(raw_source.get()).map_err(|e| {
                    ConfigError::Decode {
                        message: "Invalid source field",
                        line: e.line(),
                        column: e.column(),
                    }
                })?;
                let mode = match s_str.as_str() {
                    "auto" => SourceMode::Auto,
                    "web" => SourceMode::Web,
                    "cli" => SourceMode::Cli,
                    "oauth" => SourceMode::Oauth,
                    "api" => SourceMode::Api,
                    _ => return Err(ConfigError::UnknownSource(s_str)),
                };
                Some(mode)
            } else {
                None
            };

            let extras_enabled = if let Some(raw_ee) = map.remove("extrasEnabled") {
                Some(serde_json::from_str::<bool>(raw_ee.get()).map_err(|e| {
                    ConfigError::Decode {
                        message: "Invalid extrasEnabled field",
                        line: e.line(),
                        column: e.column(),
                    }
                })?)
            } else {
                None
            };

            let cookie_source = if let Some(raw_cs) = map.remove("cookieSource") {
                Some(
                    serde_json::from_str::<CookieSource>(raw_cs.get()).map_err(|e| {
                        ConfigError::Decode {
                            message: "Invalid cookieSource field",
                            line: e.line(),
                            column: e.column(),
                        }
                    })?,
                )
            } else {
                None
            };

            let region = if let Some(raw_reg) = map.remove("region") {
                Some(serde_json::from_str::<String>(raw_reg.get()).map_err(|e| {
                    ConfigError::Decode {
                        message: "Invalid region field",
                        line: e.line(),
                        column: e.column(),
                    }
                })?)
            } else {
                None
            };

            let workspace_id = if let Some(raw_ws) = map.remove("workspaceID") {
                Some(serde_json::from_str::<String>(raw_ws.get()).map_err(|e| {
                    ConfigError::Decode {
                        message: "Invalid workspaceID field",
                        line: e.line(),
                        column: e.column(),
                    }
                })?)
            } else {
                None
            };

            let enterprise_host = if let Some(raw_eh) = map.remove("enterpriseHost") {
                Some(serde_json::from_str::<String>(raw_eh.get()).map_err(|e| {
                    ConfigError::Decode {
                        message: "Invalid enterpriseHost field",
                        line: e.line(),
                        column: e.column(),
                    }
                })?)
            } else {
                None
            };

            let token_accounts = if let Some(raw_ta) = map.remove("tokenAccounts") {
                Some(decode_token_accounts(raw_ta.get())?)
            } else {
                None
            };

            let quota_warnings = if let Some(raw_qw) = map.remove("quotaWarnings") {
                Some(
                    serde_json::from_str::<QuotaWarnings>(raw_qw.get()).map_err(|e| {
                        ConfigError::Decode {
                            message: "Invalid quotaWarnings field",
                            line: e.line(),
                            column: e.column(),
                        }
                    })?,
                )
            } else {
                None
            };

            let accent_color = if let Some(raw_ac) = map.remove("accentColor") {
                Some(serde_json::from_str::<String>(raw_ac.get()).map_err(|e| {
                    ConfigError::Decode {
                        message: "Invalid accentColor field",
                        line: e.line(),
                        column: e.column(),
                    }
                })?)
            } else {
                None
            };

            let hidden_usage_item_ids = if let Some(raw_ids) = map.remove("hiddenUsageItemIDs") {
                Some(
                    serde_json::from_str::<Vec<String>>(raw_ids.get()).map_err(|e| {
                        ConfigError::Decode {
                            message: "Invalid hiddenUsageItemIDs field",
                            line: e.line(),
                            column: e.column(),
                        }
                    })?,
                )
            } else {
                None
            };

            // Null-valued extra keys are dropped per CodexBar parity, except secret keys
            // which must be retained so secret_in_config flags them and dump redacts them.
            let mut extra = BTreeMap::new();
            for (k, v) in map {
                let is_secret_key = matches!(
                    k.as_str(),
                    "apiKey" | "cookieHeader" | "secretKey" | "pluginSecrets"
                );
                if is_secret_key || v.get().trim() != "null" {
                    extra.insert(k, v);
                }
            }

            providers.push(ProviderEntry::Typed(ProviderConfig {
                id: descriptor.id.clone(),
                enabled,
                source,
                extras_enabled,
                cookie_source,
                region,
                workspace_id,
                enterprise_host,
                token_accounts,
                quota_warnings,
                accent_color,
                hidden_usage_item_ids,
                extra,
            }));
        } else {
            let enabled = map
                .get("enabled")
                .and_then(|e| serde_json::from_str::<bool>(e.get()).ok())
                .unwrap_or(false);

            providers.push(ProviderEntry::Opaque {
                id: id_str,
                enabled,
                raw: raw_entry,
            });
        }
    }

    Ok(Some(Config {
        version,
        providers,
        hooks: raw_root.hooks,
        settings: raw_root.settings,
        extra_top: raw_root.extra_top,
    }))
}

fn decode_token_accounts(json: &str) -> Result<TokenAccountsMeta, ConfigError> {
    #[derive(Deserialize)]
    struct RawMeta {
        version: u32,
        #[serde(rename = "activeIndex")]
        active_index: i64,
        accounts: Vec<Box<RawValue>>,
    }

    let meta: RawMeta = serde_json::from_str(json).map_err(|e| ConfigError::Decode {
        message: "Invalid tokenAccounts structure",
        line: e.line(),
        column: e.column(),
    })?;

    let mut accounts = Vec::with_capacity(meta.accounts.len());
    for raw_acc in meta.accounts {
        let mut map: BTreeMap<String, Box<RawValue>> = serde_json::from_str(raw_acc.get())
            .map_err(|e| ConfigError::Decode {
                message: "Invalid token account item",
                line: e.line(),
                column: e.column(),
            })?;

        let id = map
            .remove("id")
            .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
            .unwrap_or_default();
        let label = map
            .remove("label")
            .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
            .unwrap_or_default();
        let added_at = map
            .remove("addedAt")
            .and_then(|v| serde_json::from_str::<f64>(v.get()).ok())
            .unwrap_or(0.0);
        let last_used = map
            .remove("lastUsed")
            .and_then(|v| serde_json::from_str::<Option<f64>>(v.get()).ok())
            .flatten();
        let external_identifier = map
            .remove("externalIdentifier")
            .and_then(|v| serde_json::from_str::<Option<String>>(v.get()).ok())
            .flatten();
        let usage_scope = map
            .remove("usageScope")
            .and_then(|v| serde_json::from_str::<Option<String>>(v.get()).ok())
            .flatten();
        let organization_id = map
            .remove("organizationId")
            .and_then(|v| serde_json::from_str::<Option<String>>(v.get()).ok())
            .flatten();
        let workspace_id = map
            .remove("workspaceID")
            .and_then(|v| serde_json::from_str::<Option<String>>(v.get()).ok())
            .flatten();
        let seat_credit_entitlement = map
            .remove("seatCreditEntitlement")
            .and_then(|v| serde_json::from_str::<Option<String>>(v.get()).ok())
            .flatten();

        let mut extra = BTreeMap::new();
        for (k, v) in map {
            if k == "token" || v.get().trim() != "null" {
                extra.insert(k, v);
            }
        }

        accounts.push(TokenAccountMeta {
            id,
            label,
            added_at,
            last_used,
            external_identifier,
            usage_scope,
            organization_id,
            workspace_id,
            seat_credit_entitlement,
            extra,
        });
    }

    Ok(TokenAccountsMeta {
        version: meta.version,
        active_index: meta.active_index,
        accounts,
    })
}

#[derive(Clone)]
enum Node<'a> {
    Value(serde_json::Value),
    Raw(&'a RawValue),
    Map(BTreeMap<&'a str, Node<'a>>),
    List(Vec<Node<'a>>),
}

impl<'a> Serialize for Node<'a> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Value(v) => v.serialize(serializer),
            Self::Raw(r) => r.serialize(serializer),
            Self::Map(m) => {
                let mut map = serializer.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
            Self::List(l) => {
                let mut seq = serializer.serialize_seq(Some(l.len()))?;
                for item in l {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
        }
    }
}

/// Encodes configuration into pretty-printed, key-sorted JSON bytes without a trailing newline.
pub fn encode(config: &Config) -> Vec<u8> {
    let mut top_map = BTreeMap::new();

    top_map.insert("version", Node::Value(config.version.into()));

    let mut provider_nodes = Vec::with_capacity(config.providers.len());
    for entry in &config.providers {
        match entry {
            ProviderEntry::Typed(cfg) => {
                let mut p_map = BTreeMap::new();
                p_map.insert("id", Node::Value(cfg.id.as_str().into()));

                if let Some(ac) = &cfg.accent_color {
                    p_map.insert("accentColor", Node::Value(ac.as_str().into()));
                }
                if let Some(cs) = cfg.cookie_source {
                    p_map.insert(
                        "cookieSource",
                        Node::Value(serde_json::to_value(cs).unwrap_or(serde_json::Value::Null)),
                    );
                }
                if let Some(enabled) = cfg.enabled {
                    p_map.insert("enabled", Node::Value(enabled.into()));
                }
                if let Some(eh) = &cfg.enterprise_host {
                    p_map.insert("enterpriseHost", Node::Value(eh.as_str().into()));
                }
                if let Some(ee) = cfg.extras_enabled {
                    p_map.insert("extrasEnabled", Node::Value(ee.into()));
                }
                if let Some(ids) = &cfg.hidden_usage_item_ids {
                    p_map.insert(
                        "hiddenUsageItemIDs",
                        Node::Value(serde_json::to_value(ids).unwrap_or(serde_json::Value::Null)),
                    );
                }
                if let Some(qw) = &cfg.quota_warnings {
                    p_map.insert(
                        "quotaWarnings",
                        Node::Value(serde_json::to_value(qw).unwrap_or(serde_json::Value::Null)),
                    );
                }
                if let Some(reg) = &cfg.region {
                    p_map.insert("region", Node::Value(reg.as_str().into()));
                }
                if let Some(src) = cfg.source {
                    p_map.insert(
                        "source",
                        Node::Value(serde_json::to_value(src).unwrap_or(serde_json::Value::Null)),
                    );
                }
                if let Some(ta) = &cfg.token_accounts {
                    p_map.insert("tokenAccounts", encode_token_accounts(ta));
                }
                if let Some(ws) = &cfg.workspace_id {
                    p_map.insert("workspaceID", Node::Value(ws.as_str().into()));
                }

                for (k, v) in &cfg.extra {
                    p_map.insert(k.as_str(), Node::Raw(v));
                }

                provider_nodes.push(Node::Map(p_map));
            }
            ProviderEntry::Opaque { raw, .. } => {
                provider_nodes.push(Node::Raw(raw));
            }
        }
    }

    top_map.insert("providers", Node::List(provider_nodes));

    if let Some(hooks) = &config.hooks {
        top_map.insert("hooks", Node::Raw(hooks));
    }
    if let Some(settings) = &config.settings {
        top_map.insert("settings", Node::Raw(settings));
    }

    for (k, v) in &config.extra_top {
        top_map.insert(k.as_str(), Node::Raw(v));
    }

    serde_json::to_vec_pretty(&Node::Map(top_map)).unwrap_or_default()
}

fn encode_token_accounts<'a>(ta: &'a TokenAccountsMeta) -> Node<'a> {
    let mut map = BTreeMap::new();
    map.insert("version", Node::Value(ta.version.into()));
    map.insert("activeIndex", Node::Value(ta.active_index.into()));

    let mut accounts = Vec::with_capacity(ta.accounts.len());
    for acc in &ta.accounts {
        let mut a_map = BTreeMap::new();
        a_map.insert("id", Node::Value(acc.id.as_str().into()));
        a_map.insert("label", Node::Value(acc.label.as_str().into()));
        a_map.insert("addedAt", Node::Value(serde_json::json!(acc.added_at)));

        if let Some(last) = acc.last_used {
            a_map.insert("lastUsed", Node::Value(serde_json::json!(last)));
        }
        if let Some(ext) = &acc.external_identifier {
            a_map.insert("externalIdentifier", Node::Value(ext.as_str().into()));
        }
        if let Some(scope) = &acc.usage_scope {
            a_map.insert("usageScope", Node::Value(scope.as_str().into()));
        }
        if let Some(org) = &acc.organization_id {
            a_map.insert("organizationId", Node::Value(org.as_str().into()));
        }
        if let Some(ws) = &acc.workspace_id {
            a_map.insert("workspaceID", Node::Value(ws.as_str().into()));
        }
        if let Some(seat) = &acc.seat_credit_entitlement {
            a_map.insert("seatCreditEntitlement", Node::Value(seat.as_str().into()));
        }

        for (k, v) in &acc.extra {
            a_map.insert(k.as_str(), Node::Raw(v));
        }

        accounts.push(Node::Map(a_map));
    }

    map.insert("accounts", Node::List(accounts));
    Node::Map(map)
}
