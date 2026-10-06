//! Secure string wrapper that redacts in debug representations and zeroizes memory on drop.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A secret string that redacts itself in [`fmt::Debug`], does not implement [`fmt::Display`],
/// and zeroizes its memory buffer when dropped.
///
/// Serialized and deserialized as a standard string for wire transport (e.g. `setSecret` frame).
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop, JsonSchema)]
#[schemars(transparent)]
pub struct SecretString(String);

impl SecretString {
    /// Construct a new [`SecretString`].
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    /// Expose the secret value as a string slice.
    ///
    /// Use with care: never log or expose the returned slice to untrusted sinks.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

// NOTE: fmt::Display is deliberately NOT implemented for SecretString to prevent accidental leaks.

impl From<String> for SecretString {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for SecretString {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl Serialize for SecretString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::new(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_is_redacted() {
        let secret = SecretString::new("my-super-secret-api-key-12345");
        let debug = format!("{secret:?}");
        assert_eq!(debug, "[redacted]");
        assert!(!debug.contains("my-super-secret-api-key-12345"));
    }

    #[test]
    fn expose_secret_returns_value() {
        let secret = SecretString::new("correct-horse-battery-staple");
        assert_eq!(secret.expose_secret(), "correct-horse-battery-staple");
    }

    #[test]
    fn serde_round_trip() {
        let secret = SecretString::new("synthetic-secret-token");
        let json = serde_json::to_string(&secret).unwrap();
        assert_eq!(json, "\"synthetic-secret-token\"");

        let deserialized: SecretString = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.expose_secret(), "synthetic-secret-token");
        assert_eq!(format!("{deserialized:?}"), "[redacted]");
    }
}
