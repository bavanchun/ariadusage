// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeRateLimitResetCredits.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_protocol::usage::{DetailRow, DetailSection, RowProgress};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::scoped_weekly::parse_iso8601_timestamp;

/// Decoded `cedar_ember` grant entry (intentionally omits `id` and `label`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthCedarEmberGrant {
    pub paused: Option<bool>,
    #[serde(rename = "resets_left")]
    pub resets_left: Option<i64>,
    #[serde(rename = "resets_total")]
    pub resets_total: Option<i64>,
    #[serde(rename = "starts_at")]
    pub starts_at: Option<String>,
    #[serde(rename = "ends_at")]
    pub ends_at: Option<String>,
    #[serde(rename = "usable_now")]
    pub usable_now: Option<bool>,
}

impl OAuthCedarEmberGrant {
    fn from_json_value(value: &Value) -> Option<Self> {
        let obj = value.as_object()?;
        let paused = obj.get("paused")?.as_bool()?;
        let resets_left = obj.get("resets_left")?.as_i64()?;
        if resets_left < 0 {
            return None;
        }
        let resets_total = match obj.get("resets_total") {
            Some(Value::Null) | None => None,
            Some(v) => {
                let total = v.as_i64()?;
                if total < resets_left {
                    return None;
                }
                Some(total)
            }
        };
        let starts_at = match obj.get("starts_at") {
            Some(Value::Null) | None => None,
            Some(v) => {
                let s = v.as_str()?;
                parse_iso8601_timestamp(s)?;
                Some(s.to_owned())
            }
        };
        let ends_at = match obj.get("ends_at") {
            Some(Value::Null) | None => None,
            Some(v) => {
                let s = v.as_str()?;
                parse_iso8601_timestamp(s)?;
                Some(s.to_owned())
            }
        };
        let usable_now = obj.get("usable_now").and_then(Value::as_bool);
        Some(Self {
            paused: Some(paused),
            resets_left: Some(resets_left),
            resets_total,
            starts_at,
            ends_at,
            usable_now,
        })
    }
}

/// Decoded `cedar_ember` rate-limit reset credits payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthCedarEmberPayload {
    pub eligible: Option<bool>,
    pub grants: Option<Vec<OAuthCedarEmberGrant>>,
}

impl OAuthCedarEmberPayload {
    pub const MAXIMUM_GRANT_RECORDS: usize = 200;

    /// Decodes a `cedar_ember` JSON object, rejecting grant arrays larger than 200 records
    /// and skipping individual malformed grant entries.
    pub fn from_json_value(value: &Value) -> Option<Self> {
        let obj = value.as_object()?;
        let eligible = match obj.get("eligible") {
            Some(Value::Bool(b)) => Some(*b),
            Some(Value::Null) | None => None,
            Some(_) => return None,
        };
        let grants = match obj.get("grants") {
            Some(Value::Array(items)) => {
                if items.len() > Self::MAXIMUM_GRANT_RECORDS {
                    return None;
                }
                let mut valid_grants = Vec::with_capacity(items.len());
                for item in items {
                    if let Some(grant) = OAuthCedarEmberGrant::from_json_value(item) {
                        valid_grants.push(grant);
                    }
                }
                Some(valid_grants)
            }
            Some(Value::Null) | None => None,
            Some(_) => return None,
        };
        Some(Self { eligible, grants })
    }
}

/// Normalized Claude rate-limit reset credits (`cedar_ember`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeRateLimitResetCredits {
    pub resets_left: i64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub expires_at: Option<Timestamp>,
    #[serde(skip)]
    pub expirations: Vec<Option<Timestamp>>,
}

impl ClaudeRateLimitResetCredits {
    pub const DETAIL_ROW_LABEL: &'static str = "Limit Reset Credits";
    pub const MAXIMUM_RESETS: i64 = 50;

    pub fn new(resets_left: i64, expires_at: Option<Timestamp>) -> Option<Self> {
        if !(1..=Self::MAXIMUM_RESETS).contains(&resets_left) {
            return None;
        }
        let count = usize::try_from(resets_left).ok()?;
        Some(Self {
            resets_left,
            expires_at,
            expirations: vec![expires_at; count],
        })
    }

    /// Expiry of each reset still available at `date`, soonest first; resets without an expiry sort last.
    pub fn available_expirations(&self, date: Timestamp) -> Vec<Option<Timestamp>> {
        let mut out: Vec<Option<Timestamp>> = self
            .expirations
            .iter()
            .copied()
            .filter(|expires_at| expires_at.is_none_or(|ts| ts > date))
            .collect();
        out.sort_by(|lhs, rhs| match (lhs, rhs) {
            (Some(l), Some(r)) => l.cmp(r),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        out
    }

    /// Evaluates `cedar_ember` grants against `updated_at` and aggregates available resets.
    ///
    /// Returns `None` when the total available resets across active grants exceeds `MAXIMUM_RESETS` (50).
    pub fn from_payload(
        payload: Option<&OAuthCedarEmberPayload>,
        updated_at: Timestamp,
    ) -> Option<Self> {
        let payload = payload?;
        if payload.eligible != Some(true) {
            return None;
        }
        let grants = payload.grants.as_deref()?;
        if grants.is_empty() {
            return None;
        }

        let max_resets = usize::try_from(Self::MAXIMUM_RESETS).ok()?;
        let mut expirations: Vec<Option<Timestamp>> = Vec::new();

        for grant in grants {
            let Some((resets_left, ends_at)) = Self::available_grant(grant, updated_at) else {
                continue;
            };
            let count = usize::try_from(resets_left).ok()?;
            if count > max_resets.saturating_sub(expirations.len()) {
                return None;
            }
            expirations.extend(std::iter::repeat_n(ends_at, count));
        }

        if expirations.is_empty() {
            return None;
        }

        let mut sorted = expirations.clone();
        sorted.sort_by(|lhs, rhs| match (lhs, rhs) {
            (Some(l), Some(r)) => l.cmp(r),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        let earliest_expiry = sorted.first().copied().flatten();
        let resets_left = i64::try_from(expirations.len()).ok()?;

        Some(Self {
            resets_left,
            expires_at: earliest_expiry,
            expirations,
        })
    }

    /// Converts these rate-limit reset credits into the Snapshot v1 `ResetCredits` struct.
    ///
    /// Note: Snapshot v1 projects Claude's `cedar_ember` reset credits into `UsageSnapshot.details`
    /// via [`Self::detail_sections`] so grants remain visible when no `Credits` balance metric exists.
    pub fn to_protocol_reset_credits(&self) -> Option<ariadusage_protocol::usage::ResetCredits> {
        let available_count = u32::try_from(self.resets_left).ok()?;
        Some(ariadusage_protocol::usage::ResetCredits {
            available_count,
            next_expires_at: self.expires_at,
        })
    }

    /// Projects reset credits to a bounded `DetailSection` for `UsageSnapshot.details`.
    pub fn detail_sections(&self) -> Vec<DetailSection> {
        if !(1..=Self::MAXIMUM_RESETS).contains(&self.resets_left) {
            return Vec::new();
        }
        let value = if self.resets_left == 1 {
            "1 available".to_owned()
        } else {
            format!("{} available", self.resets_left)
        };
        let secondary_value = self.expires_at.map(format_expires_timestamp);

        let Ok(row) = DetailRow::new(
            None::<&str>,
            Self::DETAIL_ROW_LABEL,
            value,
            secondary_value,
            None,
            None,
        ) else {
            return Vec::new();
        };
        let Ok(section) = DetailSection::new(None::<&str>, vec![row], None) else {
            return Vec::new();
        };
        vec![section]
    }

    fn available_grant(
        grant: &OAuthCedarEmberGrant,
        now: Timestamp,
    ) -> Option<(i64, Option<Timestamp>)> {
        if grant.paused != Some(false) {
            return None;
        }
        let resets_left = grant.resets_left?;
        if resets_left <= 0 {
            return None;
        }
        if let Some(total) = grant.resets_total
            && total < resets_left
        {
            return None;
        }
        if let Some(starts_raw) = Self::non_empty(grant.starts_at.as_deref()) {
            let starts_at = parse_iso8601_timestamp(starts_raw)?;
            if starts_at > now {
                return None;
            }
        }
        let ends_at = if let Some(ends_raw) = Self::non_empty(grant.ends_at.as_deref()) {
            let parsed = parse_iso8601_timestamp(ends_raw)?;
            if parsed <= now {
                return None;
            }
            Some(parsed)
        } else {
            None
        };
        Some((resets_left, ends_at))
    }

    fn non_empty(raw: Option<&str>) -> Option<&str> {
        let trimmed = raw?.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    }
}

/// Decoded `iguana_necktie` cloud credits payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OAuthIguanaNecktiePayload {
    #[serde(rename = "limit_dollars")]
    pub limit_dollars: Option<f64>,
    #[serde(rename = "used_dollars")]
    pub used_dollars: Option<f64>,
    #[serde(rename = "remaining_dollars")]
    pub remaining_dollars: Option<f64>,
    #[serde(rename = "resets_at")]
    pub resets_at: Option<String>,
    #[serde(rename = "locked_reason")]
    pub locked_reason: Option<String>,
}

/// Normalized Claude cloud credits (`iguana_necktie`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaudeCloudCreditsSnapshot {
    pub limit_dollars: f64,
    pub remaining_dollars: f64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "ariadusage_protocol::time::rfc3339_opt"
    )]
    pub resets_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked_reason: Option<String>,
}

impl ClaudeCloudCreditsSnapshot {
    pub const DETAIL_TITLE: &'static str = "Cloud credits";
    pub const ROW_ID: &'static str = "claude-cloud-credits";

    pub fn from_payload(payload: Option<&OAuthIguanaNecktiePayload>) -> Option<Self> {
        let payload = payload?;
        let limit = payload.limit_dollars?;
        if !limit.is_finite() || limit <= 0.0 {
            return None;
        }
        let remaining = if let Some(rem) = payload.remaining_dollars.filter(|v| v.is_finite()) {
            rem
        } else {
            let used = payload.used_dollars.filter(|v| v.is_finite())?;
            limit - used
        };
        let clamped_remaining = remaining.clamp(0.0, limit);
        let resets_at = payload
            .resets_at
            .as_deref()
            .and_then(parse_iso8601_timestamp);
        let locked_reason = payload
            .locked_reason
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        Some(Self {
            limit_dollars: limit,
            remaining_dollars: clamped_remaining,
            resets_at,
            locked_reason,
        })
    }

    pub fn used_dollars(&self) -> f64 {
        (self.limit_dollars - self.remaining_dollars).clamp(0.0, self.limit_dollars)
    }

    /// Projects cloud credits to a bounded `DetailSection` for `UsageSnapshot.details`.
    pub fn detail_sections(&self, now: Timestamp) -> Vec<DetailSection> {
        let used = self.used_dollars();
        let Ok(progress) = RowProgress::new(used, self.limit_dollars) else {
            return Vec::new();
        };

        let is_expired = self.resets_at.is_some_and(|exp| exp <= now);
        let is_locked = self
            .locked_reason
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| !s.is_empty());

        let value = if is_expired {
            "Expired".to_owned()
        } else if is_locked {
            "Unavailable".to_owned()
        } else {
            format!(
                "${:.2} of ${:.2} remaining",
                self.remaining_dollars, self.limit_dollars
            )
        };

        let secondary_value = if is_expired {
            None
        } else {
            self.resets_at.map(format_expires_timestamp)
        };

        let Ok(row) = DetailRow::new(
            Some(Self::ROW_ID),
            Self::DETAIL_TITLE,
            value,
            secondary_value,
            Some(progress),
            Some(used),
        ) else {
            return Vec::new();
        };
        let Ok(section) = DetailSection::new(Some(Self::DETAIL_TITLE), vec![row], None) else {
            return Vec::new();
        };
        vec![section]
    }
}

fn format_expires_timestamp(ts: Timestamp) -> String {
    let dt = ts.to_zoned(jiff::tz::TimeZone::UTC);
    let month = match dt.month() {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "UTC",
    };
    format!(
        "Expires {month} {}, {:02}:{:02} UTC",
        dt.day(),
        dt.hour(),
        dt.minute()
    )
}
