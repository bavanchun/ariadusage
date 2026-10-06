//! RFC 3339 timestamp helpers with whole-second output and flexible decoding.

use std::fmt;

use serde::{Deserialize, Deserializer, Serializer};

/// Format a [`jiff::Timestamp`] as whole-second UTC RFC 3339 (`YYYY-MM-DDTHH:MM:SSZ`).
pub fn format_rfc3339_whole_seconds(ts: &jiff::Timestamp) -> String {
    // Truncate subsecond precision to whole seconds in UTC.
    let second = ts.as_second();
    let truncated = jiff::Timestamp::from_second(second).unwrap_or(*ts);
    // Format in UTC with 'Z'
    truncated.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Parse an RFC 3339 string accepting fractional seconds, UTC 'Z', or numeric offsets.
pub fn parse_rfc3339(s: &str) -> Result<jiff::Timestamp, jiff::Error> {
    s.parse::<jiff::Timestamp>()
}

/// Serde module for [`jiff::Timestamp`] with whole-second RFC 3339 serialization.
pub mod rfc3339 {
    use super::*;

    pub fn serialize<S: Serializer>(
        ts: &jiff::Timestamp,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format_rfc3339_whole_seconds(ts))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<jiff::Timestamp, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = jiff::Timestamp;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an RFC 3339 timestamp string")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                parse_rfc3339(v).map_err(serde::de::Error::custom)
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

/// Serde module for [`Option<jiff::Timestamp>`] with whole-second RFC 3339 serialization.
pub mod rfc3339_opt {
    use super::*;

    pub fn serialize<S: Serializer>(
        ts: &Option<jiff::Timestamp>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match ts {
            Some(t) => serializer.serialize_some(&format_rfc3339_whole_seconds(t)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<jiff::Timestamp>, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Option<jiff::Timestamp>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an optional RFC 3339 timestamp string")
            }

            fn visit_some<D2: Deserializer<'de>>(
                self,
                deserializer: D2,
            ) -> Result<Self::Value, D2::Error> {
                let s = String::deserialize(deserializer)?;
                let ts = parse_rfc3339(&s).map_err(serde::de::Error::custom)?;
                Ok(Some(ts))
            }

            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                let ts = parse_rfc3339(v).map_err(serde::de::Error::custom)?;
                Ok(Some(ts))
            }
        }
        deserializer.deserialize_option(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct TimeRecord {
        #[serde(with = "rfc3339")]
        pub required: jiff::Timestamp,
        #[serde(default, with = "rfc3339_opt")]
        pub optional: Option<jiff::Timestamp>,
    }

    #[test]
    fn serializes_whole_seconds() {
        let ts = "2026-10-06T12:34:56.789Z"
            .parse::<jiff::Timestamp>()
            .unwrap();
        let record = TimeRecord {
            required: ts,
            optional: Some(ts),
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"required\":\"2026-10-06T12:34:56Z\""));
        assert!(json.contains("\"optional\":\"2026-10-06T12:34:56Z\""));
    }

    #[test]
    fn deserializes_fractional_and_offset() {
        let json = r#"{"required":"2026-10-06T19:30:00.123456+07:00","optional":null}"#;
        let record: TimeRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.required.as_second(), 1791289800);
        assert_eq!(record.optional, None);
    }
}
