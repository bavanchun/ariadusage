//! Identifiers for providers, settings, actions, and requests.

use std::fmt;
use std::ops::Deref;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Error returned when an identifier fails validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdError {
    /// Provider ID must contain 1-64 lowercase ASCII letters, digits, or hyphens.
    InvalidProviderId(String),
    /// Setting ID must contain 1-64 ASCII letters, digits, hyphens, underscores, or dots.
    InvalidSettingId(String),
    /// Action ID must contain 1-64 ASCII letters, digits, hyphens, underscores, or dots.
    InvalidActionId(String),
    /// Request ID must contain 1-64 non-empty ASCII characters without whitespace.
    InvalidRequestId(String),
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProviderId(id) => write!(
                f,
                "provider ID must contain 1-64 lowercase ASCII letters, digits, or hyphens; got '{id}'"
            ),
            Self::InvalidSettingId(id) => write!(
                f,
                "setting ID must contain 1-64 ASCII characters ([a-zA-Z0-9_.-]); got '{id}'"
            ),
            Self::InvalidActionId(id) => write!(
                f,
                "action ID must contain 1-64 ASCII characters ([a-zA-Z0-9_.-]); got '{id}'"
            ),
            Self::InvalidRequestId(id) => {
                write!(
                    f,
                    "request ID must contain 1-64 ASCII characters; got '{id}'"
                )
            }
        }
    }
}

impl std::error::Error for IdError {}

fn is_valid_provider_id(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn is_valid_symbol_id(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

fn is_valid_request_id(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_graphic() && !b.is_ascii_control())
}

/// A unique identifier for a provider instance (1–64 characters, `[a-z0-9-]`).
///
/// Follows the same grammar and length restrictions as CodexBar's `ProviderInstanceID`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
#[schemars(transparent)]
pub struct ProviderId(#[schemars(regex(pattern = r"^[a-z0-9-]{1,64}$"))] String);

impl ProviderId {
    /// Validate and construct a new [`ProviderId`].
    pub fn new(id: impl Into<String>) -> Result<Self, IdError> {
        let s = id.into();
        if is_valid_provider_id(&s) {
            Ok(Self(s))
        } else {
            Err(IdError::InvalidProviderId(s))
        }
    }

    /// Returns a string slice of the provider ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProviderId({:?})", self.0)
    }
}

impl Deref for ProviderId {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for ProviderId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for ProviderId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for ProviderId {
    type Error = IdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<&str> for ProviderId {
    type Error = IdError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl Serialize for ProviderId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ProviderId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

/// A unique identifier for a setting descriptor (1–64 characters, `[a-zA-Z0-9_.-]`).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
#[schemars(transparent)]
pub struct SettingId(#[schemars(regex(pattern = r"^[a-zA-Z0-9_.-]{1,64}$"))] String);

impl SettingId {
    /// Validate and construct a new [`SettingId`].
    pub fn new(id: impl Into<String>) -> Result<Self, IdError> {
        let s = id.into();
        if is_valid_symbol_id(&s) {
            Ok(Self(s))
        } else {
            Err(IdError::InvalidSettingId(s))
        }
    }

    /// Returns a string slice of the setting ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SettingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for SettingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SettingId({:?})", self.0)
    }
}

impl Deref for SettingId {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for SettingId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for SettingId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for SettingId {
    type Error = IdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<&str> for SettingId {
    type Error = IdError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl Serialize for SettingId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SettingId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

/// A unique identifier for a provider action (1–64 characters, `[a-zA-Z0-9_.-]`).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
#[schemars(transparent)]
pub struct ActionId(#[schemars(regex(pattern = r"^[a-zA-Z0-9_.-]{1,64}$"))] String);

impl ActionId {
    /// Validate and construct a new [`ActionId`].
    pub fn new(id: impl Into<String>) -> Result<Self, IdError> {
        let s = id.into();
        if is_valid_symbol_id(&s) {
            Ok(Self(s))
        } else {
            Err(IdError::InvalidActionId(s))
        }
    }

    /// Returns a string slice of the action ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for ActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ActionId({:?})", self.0)
    }
}

impl Deref for ActionId {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for ActionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for ActionId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for ActionId {
    type Error = IdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<&str> for ActionId {
    type Error = IdError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl Serialize for ActionId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ActionId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

/// A unique identifier for a client request (1–64 characters, ASCII graphic characters).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
#[schemars(transparent)]
pub struct RequestId(#[schemars(regex(pattern = r"^[\x21-\x7E]{1,64}$"))] String);

impl RequestId {
    /// Validate and construct a new [`RequestId`].
    pub fn new(id: impl Into<String>) -> Result<Self, IdError> {
        let s = id.into();
        if is_valid_request_id(&s) {
            Ok(Self(s))
        } else {
            Err(IdError::InvalidRequestId(s))
        }
    }

    /// Returns a string slice of the request ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RequestId({:?})", self.0)
    }
}

impl Deref for RequestId {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<str> for RequestId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for RequestId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for RequestId {
    type Error = IdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl TryFrom<&str> for RequestId {
    type Error = IdError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl Serialize for RequestId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::new(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_id_accepts_valid() {
        assert!(ProviderId::new("claude").is_ok());
        assert!(ProviderId::new("codex-1").is_ok());
        assert!(ProviderId::new("a".repeat(64)).is_ok());
    }

    #[test]
    fn provider_id_rejects_invalid() {
        assert!(ProviderId::new("").is_err());
        assert!(ProviderId::new("Claude").is_err());
        assert!(ProviderId::new("claude_code").is_err());
        assert!(ProviderId::new("a".repeat(65)).is_err());
    }

    #[test]
    fn setting_id_accepts_valid() {
        assert!(SettingId::new("sourceMode").is_ok());
        assert!(SettingId::new("providers.claude.api_key").is_ok());
    }

    #[test]
    fn request_id_accepts_valid() {
        assert!(RequestId::new("req-42").is_ok());
        assert!(RequestId::new("123e4567-e89b-12d3-a456-426614174000").is_ok());
        assert!(RequestId::new("has space").is_err());
    }
}
