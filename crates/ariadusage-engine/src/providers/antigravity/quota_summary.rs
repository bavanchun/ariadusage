// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityQuotaSummaryParser.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize};

use super::status_probe::{
    AntigravitySnapshotSource, AntigravityStatusProbe, AntigravityStatusProbeError,
    AntigravityStatusSnapshot,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AntigravityQuotaSummary {
    pub raw_description: Option<String>,
    pub groups: Vec<AntigravityQuotaSummaryGroup>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AntigravityQuotaSummaryGroup {
    pub key: String,
    pub label: String,
    pub buckets: Vec<AntigravityQuotaSummaryBucket>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AntigravityQuotaSummaryBucket {
    pub bucket_id: String,
    pub display_name: Option<String>,
    pub window: Option<String>,
    pub disabled: bool,
    pub remaining_fraction: Option<f64>,
    pub reset_time: Option<Timestamp>,
    pub reset_description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuotaSummaryResponseWire {
    code: Option<QuotaSummaryCodeValue>,
    response: Option<QuotaSummaryPayloadWire>,
    summary: Option<QuotaSummaryPayloadWire>,
    description: Option<String>,
    groups: Option<QuotaSummaryGroupsWire>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuotaSummaryPayloadWire {
    description: Option<String>,
    groups: Option<QuotaSummaryGroupsWire>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum QuotaSummaryGroupsWire {
    List(Vec<QuotaSummaryGroupWire>),
    Map(BTreeMap<String, QuotaSummaryGroupWire>),
}

#[derive(Debug, Clone, Deserialize)]
struct CliUsageReportWire {
    status: Option<String>,
    command: Option<CliUsageCommandWire>,
}

#[derive(Debug, Clone, Deserialize)]
struct CliUsageCommandWire {
    name: Option<String>,
    data: Option<QuotaSummaryPayloadWire>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuotaSummaryGroupWire {
    #[serde(rename = "displayName", alias = "display_name")]
    display_name: Option<String>,
    name: Option<String>,
    label: Option<String>,
    #[allow(dead_code)]
    description: Option<String>,
    buckets: Option<Vec<QuotaSummaryBucketWire>>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuotaSummaryBucketWire {
    #[serde(rename = "bucketId", alias = "bucket_id")]
    bucket_id: Option<String>,
    id: Option<String>,
    #[serde(rename = "displayName", alias = "display_name")]
    display_name: Option<String>,
    name: Option<String>,
    #[serde(alias = "resetDescription", alias = "reset_description")]
    description: Option<String>,
    window: Option<String>,
    disabled: Option<bool>,
    #[serde(rename = "remainingFraction", alias = "remaining_fraction")]
    remaining_fraction: Option<f64>,
    remaining: Option<QuotaSummaryRemainingWire>,
    #[serde(rename = "resetTime", alias = "reset_time")]
    reset_time: Option<String>,
}

impl QuotaSummaryBucketWire {
    fn resolved_remaining_fraction(&self) -> Option<f64> {
        self.remaining_fraction
            .or_else(|| self.remaining.as_ref().and_then(|r| r.remaining_fraction))
    }
}

#[derive(Debug, Clone)]
struct QuotaSummaryRemainingWire {
    remaining_fraction: Option<f64>,
}

impl<'de> Deserialize<'de> for QuotaSummaryRemainingWire {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawRemaining {
            #[serde(rename = "remainingFraction", alias = "remaining_fraction")]
            remaining_fraction: Option<f64>,
            #[serde(rename = "case")]
            case_name: Option<String>,
            value: Option<f64>,
        }

        let raw = RawRemaining::deserialize(deserializer)?;
        if let Some(remaining_fraction) = raw.remaining_fraction {
            return Ok(Self {
                remaining_fraction: Some(remaining_fraction),
            });
        }
        if raw.case_name.as_deref() == Some("remainingFraction")
            || raw.case_name.as_deref() == Some("remaining_fraction")
        {
            return Ok(Self {
                remaining_fraction: raw.value,
            });
        }
        Ok(Self {
            remaining_fraction: None,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum QuotaSummaryCodeValue {
    Int(i64),
    String(String),
}

impl QuotaSummaryCodeValue {
    fn is_ok(&self) -> bool {
        match self {
            Self::Int(v) => *v == 0,
            Self::String(v) => {
                let lower = v.to_lowercase();
                lower == "ok" || lower == "success" || v == "0"
            }
        }
    }

    fn raw_value(&self) -> String {
        match self {
            Self::Int(v) => v.to_string(),
            Self::String(v) => v.clone(),
        }
    }
}

pub fn parse_quota_summary_response(
    data: &[u8],
) -> Result<AntigravityStatusSnapshot, AntigravityStatusProbeError> {
    parse_quota_summary_response_with_source(data, AntigravitySnapshotSource::Local)
}

pub fn parse_quota_summary_response_with_source(
    data: &[u8],
    source: AntigravitySnapshotSource,
) -> Result<AntigravityStatusSnapshot, AntigravityStatusProbeError> {
    let response: QuotaSummaryResponseWire = serde_json::from_slice(data)
        .map_err(|e| AntigravityStatusProbeError::ParseFailed(e.to_string()))?;

    if let Some(code) = &response.code
        && !code.is_ok()
    {
        return Err(AntigravityStatusProbeError::ApiError(code.raw_value()));
    }

    let root = response.groups.map(|groups| QuotaSummaryPayloadWire {
        description: response.description,
        groups: Some(groups),
    });
    let Some(payload) = response.response.or(response.summary).or(root) else {
        return Err(AntigravityStatusProbeError::ParseFailed(
            "Missing quota summary".to_owned(),
        ));
    };

    let Some(groups) = payload.groups else {
        return Err(AntigravityStatusProbeError::ParseFailed(
            "Missing quota groups".to_owned(),
        ));
    };

    let summary = build_quota_summary(payload.description, groups)?;
    Ok(AntigravityStatusSnapshot {
        model_quotas: Vec::new(),
        account_email: None,
        account_plan: None,
        source,
        quota_summary: Some(summary),
    })
}

pub fn parse_cli_usage_report(
    data: &[u8],
) -> Result<AntigravityStatusSnapshot, AntigravityStatusProbeError> {
    let report: CliUsageReportWire = serde_json::from_slice(data)
        .map_err(|e| AntigravityStatusProbeError::ParseFailed(e.to_string()))?;

    let Some(command) = report.command else {
        return Err(AntigravityStatusProbeError::ParseFailed(
            "Unsuccessful CLI usage report".to_owned(),
        ));
    };
    if report.status.as_deref() != Some("SUCCESS")
        || command.name.as_deref() != Some("usage")
        || command.data.is_none()
    {
        return Err(AntigravityStatusProbeError::ParseFailed(
            "Unsuccessful CLI usage report".to_owned(),
        ));
    }
    let cli_data = command.data.expect("checked Some above");
    let Some(groups) = cli_data.groups else {
        return Err(AntigravityStatusProbeError::ParseFailed(
            "Missing quota groups".to_owned(),
        ));
    };
    let summary = build_quota_summary(cli_data.description, groups)?;
    let snapshot = AntigravityStatusSnapshot {
        model_quotas: Vec::new(),
        account_email: None,
        account_plan: None,
        source: AntigravitySnapshotSource::Local,
        quota_summary: Some(summary),
    };
    if !snapshot.has_known_quota_summary() {
        return Err(AntigravityStatusProbeError::ParseFailed(
            "CLI usage report has no known quota".to_owned(),
        ));
    }
    Ok(snapshot)
}

fn build_quota_summary(
    description: Option<String>,
    groups: QuotaSummaryGroupsWire,
) -> Result<AntigravityQuotaSummary, AntigravityStatusProbeError> {
    let parsed_groups: Vec<AntigravityQuotaSummaryGroup> = match groups {
        QuotaSummaryGroupsWire::List(list) => list
            .into_iter()
            .filter_map(|group| build_quota_summary_group(None, group))
            .collect(),
        QuotaSummaryGroupsWire::Map(map) => map
            .into_iter()
            .filter_map(|(key, group)| build_quota_summary_group(Some(key), group))
            .collect(),
    };

    if parsed_groups.is_empty() {
        return Err(AntigravityStatusProbeError::ParseFailed(
            "Missing quota groups".to_owned(),
        ));
    }

    Ok(AntigravityQuotaSummary {
        raw_description: non_empty(description),
        groups: parsed_groups,
    })
}

fn build_quota_summary_group(
    fallback_key: Option<String>,
    group: QuotaSummaryGroupWire,
) -> Option<AntigravityQuotaSummaryGroup> {
    let buckets: Vec<AntigravityQuotaSummaryBucket> = group
        .buckets
        .unwrap_or_default()
        .into_iter()
        .filter_map(build_quota_summary_bucket)
        .collect();
    if buckets.is_empty() {
        return None;
    }
    let label = non_empty(group.display_name.or(group.name).or(group.label))
        .or_else(|| fallback_key.clone())
        .unwrap_or_else(|| "Quota".to_owned());
    let key = fallback_key.unwrap_or_else(|| label.clone());
    Some(AntigravityQuotaSummaryGroup {
        key,
        label,
        buckets,
    })
}

fn build_quota_summary_bucket(
    bucket: QuotaSummaryBucketWire,
) -> Option<AntigravityQuotaSummaryBucket> {
    let remaining_fraction = bucket.resolved_remaining_fraction();
    let bucket_id = non_empty(bucket.bucket_id.or(bucket.id))?;
    let display_name =
        non_empty(bucket.display_name.or(bucket.name)).or_else(|| Some(bucket_id.clone()));
    let reset_time = bucket
        .reset_time
        .as_deref()
        .and_then(AntigravityStatusProbe::parse_date);
    let reset_description = non_empty(bucket.description);

    Some(AntigravityQuotaSummaryBucket {
        bucket_id,
        display_name,
        window: non_empty(bucket.window),
        disabled: bucket.disabled.unwrap_or(false),
        remaining_fraction,
        reset_time,
        reset_description,
    })
}

fn non_empty(raw: Option<String>) -> Option<String> {
    let trimmed = raw?.trim().to_owned();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}
