//! Unverified JWT expiration claim reader.
//!
//! Extracts the top-level `exp` claim from an unverified JWT without validating
//! cryptographic signatures, following CodexBar parity.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;
use std::fmt;

/// Minimum allowed epoch seconds (-262143-01-01T00:00:00Z).
pub const MIN_CODEX_EXP: i64 = -8_334_601_228_800;

/// Maximum allowed epoch seconds (262142-12-31T23:59:59Z).
pub const MAX_CODEX_EXP: i64 = 8_210_266_876_799;

/// Parses the unverified `exp` timestamp claim from a JWT token.
///
/// Returns `Some(exp_seconds)` if:
/// - The token has exactly 3 non-empty segments separated by `.`.
/// - The payload segment is valid base64url (padding-indifferent).
/// - The decoded payload is a UTF-8 JSON object.
/// - The JSON object contains a unique top-level `exp` claim matching `-?(0|[1-9][0-9]*)`.
/// - The integer value fits within `-8_334_601_228_800..=8_210_266_876_799`.
///
/// Returns `None` for any malformed, missing, nested, ambiguous, or out-of-range claim.
pub fn exp_claim(token: &str) -> Option<i64> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|s| s.is_empty()) {
        return None;
    }

    let mut payload_b64 = parts[1].replace('-', "+").replace('_', "/");
    match payload_b64.len() % 4 {
        2 => payload_b64.push_str("=="),
        3 => payload_b64.push('='),
        _ => {}
    }

    let payload_bytes = STANDARD.decode(payload_b64.as_bytes()).ok()?;
    let payload_str = std::str::from_utf8(&payload_bytes).ok()?;

    let parsed: ExpPayload = serde_json::from_str(payload_str).ok()?;
    parsed.0
}

fn parse_exp_raw(raw: &str) -> Option<i64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let digits = s.strip_prefix('-').unwrap_or(s);
    if digits.is_empty() {
        return None;
    }
    if digits == "0" {
        return Some(0);
    }
    if digits.starts_with('0') || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let val: i64 = s.parse().ok()?;
    if (MIN_CODEX_EXP..=MAX_CODEX_EXP).contains(&val) {
        Some(val)
    } else {
        None
    }
}

struct ExpPayload(Option<i64>);

impl<'de> serde::Deserialize<'de> for ExpPayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(ExpVisitor).map(ExpPayload)
    }
}

struct ExpVisitor;

impl<'de> Visitor<'de> for ExpVisitor {
    type Value = Option<i64>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut seen_exp = false;
        let mut duplicate_exp = false;
        let mut exp_result = None;

        while let Some(key) = access.next_key::<String>()? {
            if key == "exp" {
                if seen_exp {
                    duplicate_exp = true;
                    let _ = access.next_value::<&RawValue>()?;
                } else {
                    seen_exp = true;
                    let raw: &RawValue = access.next_value()?;
                    exp_result = parse_exp_raw(raw.get());
                }
            } else {
                let _ = access.next_value::<de::IgnoredAny>()?;
            }
        }

        if duplicate_exp {
            Ok(None)
        } else {
            Ok(exp_result)
        }
    }
}
