//! Settings descriptors and pages for client-agnostic configuration UI.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::ids::{ActionId, ProviderId, SettingId};

/// Target scope of a settings page.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SettingsScope {
    App,
    Provider(ProviderId),
}

impl SettingsScope {
    pub fn app() -> Self {
        Self::App
    }

    pub fn provider(id: ProviderId) -> Self {
        Self::Provider(id)
    }
}

/// A condition gating visibility or enablement of a setting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingCondition {
    pub setting_id: SettingId,
    pub equals: serde_json::Value,
}

impl SettingCondition {
    pub fn equals(setting_id: SettingId, value: impl Into<serde_json::Value>) -> Self {
        Self {
            setting_id,
            equals: value.into(),
        }
    }
}

/// An option in a single-choice picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChoiceOption {
    pub id: String,
    pub label: String,
}

impl ChoiceOption {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// An entry in a multi-choice checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MultiChoiceEntry {
    pub id: String,
    pub label: String,
    pub locked: bool,
    pub enabled: bool,
}

impl MultiChoiceEntry {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        locked: bool,
        enabled: bool,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            locked,
            enabled,
        }
    }
}

/// Configuration for numeric inputs.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NumberConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// Configuration for text inputs.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_length: Option<usize>,
}

/// Confirmation prompt displayed before running an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActionConfirmation {
    pub title: String,
    pub message: String,
    pub confirm_label: String,
}

/// Visual style of an action element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ActionStyle {
    Button,
    Link,
}

/// An executable action in an action list descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActionItem {
    pub id: ActionId,
    pub label: String,
    pub style: ActionStyle,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<ActionConfirmation>,
}

/// Account row representation in token account manager descriptors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenAccountRow {
    pub id: String,
    pub label: String,
    pub active: bool,
    pub token_is_set: bool,
}

/// Capabilities and account list for token account management.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TokenAccountsConfig {
    pub accounts: Vec<TokenAccountRow>,
    pub supports_add: bool,
    pub supports_remove: bool,
    pub supports_activate: bool,
}

/// Specialized descriptor kind definitions.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum DescriptorKind {
    #[serde(rename_all = "camelCase")]
    Toggle { value: bool },
    #[serde(rename_all = "camelCase")]
    Choice {
        selected: String,
        options: Vec<ChoiceOption>,
    },
    #[serde(rename_all = "camelCase")]
    MultiChoice { entries: Vec<MultiChoiceEntry> },
    #[serde(rename_all = "camelCase")]
    Number {
        value: f64,
        #[serde(flatten)]
        config: NumberConfig,
    },
    #[serde(rename_all = "camelCase")]
    Text {
        value: String,
        #[serde(flatten)]
        config: TextConfig,
    },
    #[serde(rename_all = "camelCase")]
    Secret {
        is_set: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    PathList { paths: Vec<String> },
    #[serde(rename_all = "camelCase")]
    Color { value: String, default: String },
    #[serde(rename_all = "camelCase")]
    Actions { actions: Vec<ActionItem> },
    #[serde(rename_all = "camelCase")]
    TokenAccounts {
        #[serde(flatten)]
        config: TokenAccountsConfig,
    },
    #[serde(rename_all = "camelCase")]
    Unknown { kind: String },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
enum DescriptorKindHelper {
    #[serde(rename_all = "camelCase")]
    Toggle { value: bool },
    #[serde(rename_all = "camelCase")]
    Choice {
        selected: String,
        options: Vec<ChoiceOption>,
    },
    #[serde(rename_all = "camelCase")]
    MultiChoice { entries: Vec<MultiChoiceEntry> },
    #[serde(rename_all = "camelCase")]
    Number {
        value: f64,
        #[serde(flatten)]
        config: NumberConfig,
    },
    #[serde(rename_all = "camelCase")]
    Text {
        value: String,
        #[serde(flatten)]
        config: TextConfig,
    },
    #[serde(rename_all = "camelCase")]
    Secret {
        is_set: bool,
        #[serde(default)]
        source: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    PathList { paths: Vec<String> },
    #[serde(rename_all = "camelCase")]
    Color { value: String, default: String },
    #[serde(rename_all = "camelCase")]
    Actions { actions: Vec<ActionItem> },
    #[serde(rename_all = "camelCase")]
    TokenAccounts {
        #[serde(flatten)]
        config: TokenAccountsConfig,
    },
}

impl<'de> Deserialize<'de> for DescriptorKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if let Some(type_str) = value.get("type").and_then(|v| v.as_str()) {
            match type_str {
                "toggle" | "choice" | "multiChoice" | "number" | "text" | "secret" | "pathList"
                | "color" | "actions" | "tokenAccounts" => {
                    let helper = DescriptorKindHelper::deserialize(value)
                        .map_err(serde::de::Error::custom)?;
                    Ok(match helper {
                        DescriptorKindHelper::Toggle { value } => DescriptorKind::Toggle { value },
                        DescriptorKindHelper::Choice { selected, options } => {
                            DescriptorKind::Choice { selected, options }
                        }
                        DescriptorKindHelper::MultiChoice { entries } => {
                            DescriptorKind::MultiChoice { entries }
                        }
                        DescriptorKindHelper::Number { value, config } => {
                            DescriptorKind::Number { value, config }
                        }
                        DescriptorKindHelper::Text { value, config } => {
                            DescriptorKind::Text { value, config }
                        }
                        DescriptorKindHelper::Secret { is_set, source } => {
                            DescriptorKind::Secret { is_set, source }
                        }
                        DescriptorKindHelper::PathList { paths } => {
                            DescriptorKind::PathList { paths }
                        }
                        DescriptorKindHelper::Color { value, default } => {
                            DescriptorKind::Color { value, default }
                        }
                        DescriptorKindHelper::Actions { actions } => {
                            DescriptorKind::Actions { actions }
                        }
                        DescriptorKindHelper::TokenAccounts { config } => {
                            DescriptorKind::TokenAccounts { config }
                        }
                    })
                }
                other => Ok(DescriptorKind::Unknown {
                    kind: other.to_string(),
                }),
            }
        } else {
            Ok(DescriptorKind::Unknown {
                kind: "missing_type".to_string(),
            })
        }
    }
}

/// A descriptor representing a single configurable setting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingDescriptor {
    pub id: SettingId,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<SettingCondition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_when: Option<SettingCondition>,
    pub kind: DescriptorKind,
}

impl SettingDescriptor {
    pub fn new(id: SettingId, label: impl Into<String>, kind: DescriptorKind) -> Self {
        Self {
            id,
            label: label.into(),
            help: None,
            visible_when: None,
            enabled_when: None,
            kind,
        }
    }
}

/// A titled section of settings descriptors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSection {
    pub id: String,
    pub title: String,
    pub descriptors: Vec<SettingDescriptor>,
}

impl SettingsSection {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        descriptors: Vec<SettingDescriptor>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            descriptors,
        }
    }
}

/// A complete settings page for app or provider configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPage {
    pub scope: SettingsScope,
    pub sections: Vec<SettingsSection>,
}

impl SettingsPage {
    pub fn new(scope: SettingsScope, sections: Vec<SettingsSection>) -> Self {
        Self { scope, sections }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_descriptor_never_serializes_value() {
        let descriptor = SettingDescriptor::new(
            SettingId::new("apiKey").unwrap(),
            "API Key",
            DescriptorKind::Secret {
                is_set: true,
                source: Some("environment".to_string()),
            },
        );

        let json = serde_json::to_string(&descriptor).expect("serializes");
        assert!(!json.contains("\"value\""));
        assert!(json.contains("\"isSet\":true"));
        assert!(json.contains("\"source\":\"environment\""));
        assert!(json.contains("\"type\":\"secret\""));
    }

    #[test]
    fn unknown_descriptor_kind_preserves_id_and_label() {
        let json = r#"{
            "id": "futureSetting",
            "label": "Future Experimental Feature",
            "kind": {
                "type": "quantumSlider",
                "qubits": 42
            }
        }"#;

        let descriptor: SettingDescriptor = serde_json::from_str(json).expect("deserializes");
        assert_eq!(descriptor.id.as_str(), "futureSetting");
        assert_eq!(descriptor.label, "Future Experimental Feature");
        assert_eq!(
            descriptor.kind,
            DescriptorKind::Unknown {
                kind: "quantumSlider".to_string()
            }
        );
    }
}
