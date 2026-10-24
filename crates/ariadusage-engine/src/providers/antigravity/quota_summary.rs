// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityQuotaSummaryParser.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use jiff::Timestamp;
use serde::Deserialize;

use super::status_probe::{
    AntigravitySnapshotSource, AntigravityStatusProbeError, AntigravityStatusSnapshot,
};

#[derive(Debug, Clone, PartialEq)]
pub struct AntigravityQuotaSummary {
    pub groups: Vec<AntigravityQuotaSummaryGroup>,
}

impl AntigravityQuotaSummary {
    pub fn new(groups: Vec<AntigravityQuotaSummaryGroup>) -> Self {
        Self { groups }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AntigravityQuotaSummaryGroup {
    pub name: String,
    pub buckets: Vec<AntigravityQuotaSummaryBucket>,
}

impl AntigravityQuotaSummaryGroup {
    pub fn new(name: impl Into<String>, buckets: Vec<AntigravityQuotaSummaryBucket>) -> Self {
        Self {
            name: name.into(),
            buckets,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AntigravityQuotaSummaryBucket {
    pub id: String,
    pub name: String,
    pub window_label: Option<String>,
    pub window_minutes: Option<u32>,
    pub remaining_fraction: Option<f64>,
    pub reset_time: Option<Timestamp>,
    pub disabled: bool,
}

impl AntigravityQuotaSummaryBucket {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        window_label: Option<String>,
        window_minutes: Option<u32>,
        remaining_fraction: Option<f64>,
        reset_time: Option<Timestamp>,
        disabled: bool,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            window_label,
            window_minutes,
            remaining_fraction,
            reset_time,
            disabled,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AntigravityQuotaSummaryResponse {
    pub groups: Option<Vec<QuotaSummaryGroupWire>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuotaSummaryGroupWire {
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    pub name: Option<String>,
    pub buckets: Option<Vec<QuotaSummaryBucketWire>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuotaSummaryBucketWire {
    #[serde(rename = "bucketId")]
    pub bucket_id: Option<String>,
    pub id: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    pub name: Option<String>,
    pub label: Option<String>,
    pub window: Option<String>,
    #[serde(rename = "windowType")]
    pub window_type: Option<String>,
    pub cadence: Option<String>,
    #[serde(rename = "remainingFraction", alias = "remaining_fraction")]
    pub remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime", alias = "reset_time")]
    pub reset_time: Option<String>,
    pub disabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
struct CliUsageReport {
    status: Option<String>,
    command: Option<CliUsageCommand>,
}

#[derive(Debug, Clone, Deserialize)]
struct CliUsageCommand {
    data: Option<AntigravityQuotaSummaryResponse>,
}

pub fn parse_quota_summary_response(
    data: &[u8],
) -> Result<AntigravityStatusSnapshot, AntigravityStatusProbeError> {
    let response: AntigravityQuotaSummaryResponse = serde_json::from_slice(data)
        .map_err(|e| AntigravityStatusProbeError::ParseFailed(e.to_string()))?;
    let summary = quota_summary_from_response(&response)?;
    Ok(AntigravityStatusSnapshot {
        model_quotas: Vec::new(),
        account_email: None,
        account_plan: None,
        source: AntigravitySnapshotSource::Local,
        quota_summary: Some(summary),
    })
}

pub fn parse_cli_usage_report(
    data: &[u8],
) -> Result<AntigravityStatusSnapshot, AntigravityStatusProbeError> {
    let report: CliUsageReport = serde_json::from_slice(data)
        .map_err(|e| AntigravityStatusProbeError::ParseFailed(e.to_string()))?;
    let status = report.status.as_deref().unwrap_or("").trim().to_lowercase();
    if status != "success" {
        return Err(AntigravityStatusProbeError::ParseFailed(format!(
            "CLI usage report status: {}",
            report.status.as_deref().unwrap_or("missing")
        )));
    }
    let Some(command_data) = report.command.and_then(|c| c.data) else {
        return Err(AntigravityStatusProbeError::ParseFailed(
            "CLI usage report missing command.data".to_owned(),
        ));
    };
    let summary = quota_summary_from_response(&command_data)?;
    Ok(AntigravityStatusSnapshot {
        model_quotas: Vec::new(),
        account_email: None,
        account_plan: None,
        source: AntigravitySnapshotSource::Local,
        quota_summary: Some(summary),
    })
}

pub fn quota_summary_from_response(
    response: &AntigravityQuotaSummaryResponse,
) -> Result<AntigravityQuotaSummary, AntigravityStatusProbeError> {
    let mut groups = Vec::new();
    for (group_index, group) in response.groups.as_deref().unwrap_or(&[]).iter().enumerate() {
        let raw_buckets = group.buckets.as_deref().unwrap_or(&[]);
        if raw_buckets.is_empty() {
            continue;
        }
        let group_name = non_empty(group.display_name.as_deref())
            .or_else(|| non_empty(group.name.as_deref()))
            .unwrap_or_else(|| format!("Group {}", group_index + 1));
        let mut buckets = Vec::with_capacity(raw_buckets.len());
        for (bucket_index, bucket) in raw_buckets.iter().enumerate() {
            let window_label = non_empty(bucket.window.as_deref())
                .or_else(|| non_empty(bucket.window_type.as_deref()))
                .or_else(|| non_empty(bucket.cadence.as_deref()));
            let id = non_empty(bucket.bucket_id.as_deref())
                .or_else(|| non_empty(bucket.id.as_deref()))
                .unwrap_or_else(|| format!("{group_name}-{}", bucket_index + 1));
            let name = non_empty(bucket.display_name.as_deref())
                .or_else(|| non_empty(bucket.name.as_deref()))
                .or_else(|| non_empty(bucket.label.as_deref()))
                .unwrap_or_else(|| id.clone());
            let window_minutes = window_label
                .as_deref()
                .and_then(parse_quota_summary_window_minutes);
            let remaining_fraction = bucket.remaining_fraction.map(|v| v.clamp(0.0, 1.0));
            let reset_time = bucket
                .reset_time
                .as_deref()
                .and_then(decode_quota_summary_date);
            buckets.push(AntigravityQuotaSummaryBucket {
                id,
                name,
                window_label,
                window_minutes,
                remaining_fraction,
                reset_time,
                disabled: bucket.disabled.unwrap_or(false),
            });
        }
        groups.push(AntigravityQuotaSummaryGroup {
            name: group_name,
            buckets,
        });
    }

    if groups.is_empty() {
        return Err(AntigravityStatusProbeError::ParseFailed(
            "No quota summary groups found".to_owned(),
        ));
    }

    Ok(AntigravityQuotaSummary { groups })
}

pub fn decode_quota_summary_date(raw: &str) -> Option<Timestamp> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(ts) = trimmed.parse::<Timestamp>() {
        return Some(ts);
    }
    if let Ok(seconds) = trimmed.parse::<f64>()
        && seconds.is_finite()
    {
        let whole_secs = seconds.trunc() as i64;
        let nanos = ((seconds - seconds.trunc()) * 1_000_000_000.0).round() as i32;
        return Timestamp::new(whole_secs, nanos).ok();
    }
    None
}

pub fn parse_quota_summary_window_minutes(raw: &str) -> Option<u32> {
    let lower = raw.trim().to_lowercase();
    match lower.as_str() {
        "5h" | "5hr" | "5hrs" | "5_hour" | "5-hour" | "five_hour" | "five-hour" | "session" => {
            return Some(300);
        }
        "weekly" | "week" | "1w" | "7d" | "7_day" | "7-day" => return Some(10_080),
        "daily" | "day" | "1d" | "24h" => return Some(1440),
        _ => {}
    }

    if let Some(hours_str) = lower.strip_suffix('h')
        && let Ok(hours) = hours_str.parse::<u32>()
        && hours > 0
    {
        return hours.checked_mul(60);
    }
    if let Some(days_str) = lower.strip_suffix('d')
        && let Ok(days) = days_str.parse::<u32>()
        && days > 0
    {
        return days.checked_mul(1440);
    }
    None
}

fn non_empty(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}
