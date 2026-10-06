//! Metric envelope and honesty invariants.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

/// State of a metric value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MetricState {
    Value,
    Loading,
    Stale,
    Error,
    #[serde(other)]
    Unknown,
}

/// Confidence level in a metric value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Confidence {
    Exact,
    Estimated,
    PercentOnly,
    #[serde(other)]
    Unknown,
}

/// Strategy kind that acquired the metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SourceKind {
    Cli,
    Web,
    Oauth,
    Api,
    LocalProbe,
    Dashboard,
    #[serde(other)]
    Unknown,
}

/// Provenance information for a metric.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetricSource {
    pub kind: SourceKind,
    pub label: String,
}

impl MetricSource {
    /// Construct a new [`MetricSource`].
    pub fn new(kind: SourceKind, label: impl Into<String>) -> Self {
        Self {
            kind,
            label: label.into(),
        }
    }
}

/// Diagnostic error attached to a metric.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetricError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
}

impl MetricError {
    /// Construct a new [`MetricError`] without retry information.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retry_after_seconds: None,
        }
    }

    /// Construct a new [`MetricError`] with retry information.
    pub fn with_retry(
        code: impl Into<String>,
        message: impl Into<String>,
        retry_after_seconds: u64,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retry_after_seconds: Some(retry_after_seconds),
        }
    }
}

/// Error returned when metric state and value presence violate the honesty invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetricInvariantError {
    /// State is `value` or `stale` but `value` is `None`.
    MissingValueForState(MetricState),
    /// State is not `value` or `stale` but `value` is `Some`.
    UnexpectedValueForState(MetricState),
}

impl fmt::Display for MetricInvariantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingValueForState(state) => {
                write!(f, "metric with state '{state:?}' must have a value")
            }
            Self::UnexpectedValueForState(state) => {
                write!(f, "metric with state '{state:?}' must not have a value")
            }
        }
    }
}

impl std::error::Error for MetricInvariantError {}

/// Envelope for metric values that carries freshness, honesty, and provenance.
///
/// Invariant: `value` is present if and only if `state` is `MetricState::Value` or `MetricState::Stale`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Metric<T> {
    pub state: MetricState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<T>,
    #[schemars(with = "Option<String>")]
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::time::rfc3339_opt"
    )]
    pub as_of: Option<jiff::Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<MetricSource>,
    pub confidence: Confidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<MetricError>,
}

impl<T> Metric<T> {
    /// Construct a new `Metric` verifying the honesty invariant.
    pub fn new(
        state: MetricState,
        value: Option<T>,
        as_of: Option<jiff::Timestamp>,
        source: Option<MetricSource>,
        confidence: Confidence,
        error: Option<MetricError>,
    ) -> Result<Self, MetricInvariantError> {
        let is_value_state = matches!(state, MetricState::Value | MetricState::Stale);
        if is_value_state && value.is_none() {
            return Err(MetricInvariantError::MissingValueForState(state));
        }
        if !is_value_state && value.is_some() {
            return Err(MetricInvariantError::UnexpectedValueForState(state));
        }
        Ok(Self {
            state,
            value,
            as_of,
            source,
            confidence,
            error,
        })
    }

    /// Construct a metric in the `value` state.
    pub fn value(
        value: T,
        as_of: Option<jiff::Timestamp>,
        source: Option<MetricSource>,
        confidence: Confidence,
    ) -> Self {
        Self {
            state: MetricState::Value,
            value: Some(value),
            as_of,
            source,
            confidence,
            error: None,
        }
    }

    /// Construct a metric in the `stale` state.
    pub fn stale(
        value: T,
        as_of: Option<jiff::Timestamp>,
        source: Option<MetricSource>,
        confidence: Confidence,
        error: Option<MetricError>,
    ) -> Self {
        Self {
            state: MetricState::Stale,
            value: Some(value),
            as_of,
            source,
            confidence,
            error,
        }
    }

    /// Construct a metric in the `unknown` state.
    pub fn unknown() -> Self {
        Self {
            state: MetricState::Unknown,
            value: None,
            as_of: None,
            source: None,
            confidence: Confidence::Unknown,
            error: None,
        }
    }

    /// Construct a metric in the `loading` state.
    pub fn loading(source: Option<MetricSource>) -> Self {
        Self {
            state: MetricState::Loading,
            value: None,
            as_of: None,
            source,
            confidence: Confidence::Unknown,
            error: None,
        }
    }

    /// Construct a metric in the `error` state.
    pub fn error(error: MetricError, source: Option<MetricSource>) -> Self {
        Self {
            state: MetricState::Error,
            value: None,
            as_of: None,
            source,
            confidence: Confidence::Unknown,
            error: Some(error),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", bound(deserialize = "T: Deserialize<'de>"))]
struct MetricRaw<T> {
    state: MetricState,
    #[serde(default)]
    value: Option<T>,
    #[serde(default, with = "crate::time::rfc3339_opt")]
    as_of: Option<jiff::Timestamp>,
    #[serde(default)]
    source: Option<MetricSource>,
    #[serde(default = "default_unknown_confidence")]
    confidence: Confidence,
    #[serde(default)]
    error: Option<MetricError>,
}

fn default_unknown_confidence() -> Confidence {
    Confidence::Unknown
}

impl<T> TryFrom<MetricRaw<T>> for Metric<T> {
    type Error = MetricInvariantError;

    fn try_from(raw: MetricRaw<T>) -> Result<Self, Self::Error> {
        Self::new(
            raw.state,
            raw.value,
            raw.as_of,
            raw.source,
            raw.confidence,
            raw.error,
        )
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Metric<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = MetricRaw::<T>::deserialize(deserializer)?;
        Self::try_from(raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_value_state() {
        let metric = Metric::value(42, None, None, Confidence::Exact);
        assert_eq!(metric.state, MetricState::Value);
        assert_eq!(metric.value, Some(42));
    }

    #[test]
    fn valid_stale_state() {
        let metric = Metric::stale(42, None, None, Confidence::Estimated, None);
        assert_eq!(metric.state, MetricState::Stale);
        assert_eq!(metric.value, Some(42));
    }

    #[test]
    fn valid_unknown_state() {
        let metric: Metric<i32> = Metric::unknown();
        assert_eq!(metric.state, MetricState::Unknown);
        assert_eq!(metric.value, None);
    }

    #[test]
    fn rejects_missing_value_for_value_state() {
        let res: Result<Metric<i32>, _> = Metric::new(
            MetricState::Value,
            None,
            None,
            None,
            Confidence::Exact,
            None,
        );
        assert_eq!(
            res,
            Err(MetricInvariantError::MissingValueForState(
                MetricState::Value
            ))
        );
    }

    #[test]
    fn rejects_value_for_unknown_state() {
        let res = Metric::new(
            MetricState::Unknown,
            Some(10),
            None,
            None,
            Confidence::Unknown,
            None,
        );
        assert_eq!(
            res,
            Err(MetricInvariantError::UnexpectedValueForState(
                MetricState::Unknown
            ))
        );
    }

    #[test]
    fn deserialization_enforces_invariant() {
        let valid_json = r#"{"state":"value","value":100,"confidence":"exact"}"#;
        let metric: Metric<i32> = serde_json::from_str(valid_json).unwrap();
        assert_eq!(metric.value, Some(100));

        let invalid_missing_value = r#"{"state":"value","confidence":"exact"}"#;
        assert!(serde_json::from_str::<Metric<i32>>(invalid_missing_value).is_err());

        let invalid_unexpected_value = r#"{"state":"unknown","value":100,"confidence":"unknown"}"#;
        assert!(serde_json::from_str::<Metric<i32>>(invalid_unexpected_value).is_err());
    }
}
