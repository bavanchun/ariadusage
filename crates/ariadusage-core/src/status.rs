// Ported from CodexBar Sources/CodexBarCore/ProviderStatus.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/ProviderStatusFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/StatuspageSummaryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/GoogleWorkspaceStatusTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashMap;
use std::sync::LazyLock;

use ariadusage_protocol::{
    ProviderStatus, StatusComponent, StatusIndicator as ProviderStatusIndicator,
};
use jiff::Timestamp;
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Error returned when a provider status payload cannot be decoded.
#[derive(Debug, Error)]
pub enum StatusParseError {
    #[error("status JSON decode failed: {0}")]
    Json(#[from] serde_json::Error),
}

/// Combined status indicator and component breakdown for a provider status page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderStatusSummary {
    pub status: ProviderStatus,
    #[serde(default)]
    pub components: Vec<StatuspageComponentSummary>,
}

impl ProviderStatusSummary {
    pub fn new(status: ProviderStatus, components: Vec<StatuspageComponentSummary>) -> Self {
        Self { status, components }
    }
}

/// Summary of one Statuspage or incident.io component or component group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatuspageComponentSummary {
    pub id: String,
    pub name: String,
    pub status: String,
    pub indicator: ProviderStatusIndicator,
    #[serde(default)]
    pub children: Vec<StatuspageComponentSummary>,
}

impl StatuspageComponentSummary {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        status: impl Into<String>,
        indicator: ProviderStatusIndicator,
        children: Vec<StatuspageComponentSummary>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            status: status.into(),
            indicator,
            children,
        }
    }

    pub fn to_status_component(&self, updated_at: Option<Timestamp>) -> StatusComponent {
        StatusComponent {
            id: self.id.clone(),
            name: self.name.clone(),
            status: self.status.clone(),
            indicator: self.indicator,
            updated_at,
            children: self
                .children
                .iter()
                .map(|child| child.to_status_component(updated_at))
                .collect(),
        }
    }
}

/// Severity rank used when choosing the worst indicator across components or incidents.
pub fn status_indicator_rank(indicator: ProviderStatusIndicator) -> u8 {
    match indicator {
        ProviderStatusIndicator::None => 0,
        ProviderStatusIndicator::Maintenance | ProviderStatusIndicator::Unknown => 1,
        ProviderStatusIndicator::Minor => 2,
        ProviderStatusIndicator::Major => 3,
        ProviderStatusIndicator::Critical => 4,
    }
}

/// Whether the indicator represents an active issue or maintenance (`ProviderStatusIndicator.hasIssue`).
pub fn status_indicator_has_issue(indicator: ProviderStatusIndicator) -> bool {
    matches!(
        indicator,
        ProviderStatusIndicator::Minor
            | ProviderStatusIndicator::Major
            | ProviderStatusIndicator::Critical
            | ProviderStatusIndicator::Maintenance
    )
}

/// Human-readable fallback label for a status indicator (`ProviderStatusIndicator.label`).
pub fn status_indicator_label(indicator: ProviderStatusIndicator) -> &'static str {
    match indicator {
        ProviderStatusIndicator::None => "Operational",
        ProviderStatusIndicator::Minor => "Partial outage",
        ProviderStatusIndicator::Major => "Major outage",
        ProviderStatusIndicator::Critical => "Critical issue",
        ProviderStatusIndicator::Maintenance => "Maintenance",
        ProviderStatusIndicator::Unknown => "Status unknown",
    }
}

/// Maps a raw Statuspage / incident.io component status string to a `ProviderStatusIndicator`.
pub fn indicator_for_statuspage_status(raw: &str) -> ProviderStatusIndicator {
    match raw.trim().to_ascii_lowercase().as_str() {
        "operational" => ProviderStatusIndicator::None,
        "degraded_performance" => ProviderStatusIndicator::Minor,
        "partial_outage" => ProviderStatusIndicator::Major,
        "major_outage" | "full_outage" => ProviderStatusIndicator::Critical,
        "under_maintenance" => ProviderStatusIndicator::Maintenance,
        _ => ProviderStatusIndicator::Unknown,
    }
}

fn parse_indicator(raw: &str) -> ProviderStatusIndicator {
    match raw.trim().to_ascii_lowercase().as_str() {
        "none" => ProviderStatusIndicator::None,
        "minor" => ProviderStatusIndicator::Minor,
        "major" => ProviderStatusIndicator::Major,
        "critical" => ProviderStatusIndicator::Critical,
        "maintenance" => ProviderStatusIndicator::Maintenance,
        _ => ProviderStatusIndicator::Unknown,
    }
}

fn non_blank_string(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

fn parse_rfc3339_timestamp(raw: Option<&str>) -> Option<Timestamp> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    raw.parse::<Timestamp>().ok()
}

#[derive(Deserialize)]
struct StatuspageStatusResponse {
    page: Option<StatuspagePage>,
    status: StatuspageStatusInfo,
}

#[derive(Deserialize)]
struct StatuspagePage {
    updated_at: Option<String>,
}

#[derive(Deserialize)]
struct StatuspageStatusInfo {
    indicator: String,
    description: Option<String>,
}

/// Parses a Statuspage `api/v2/status.json` or `api/v2/summary.json` payload into a `ProviderStatus`.
pub fn parse_statuspage_status(bytes: &[u8]) -> Result<ProviderStatus, StatusParseError> {
    let decoded: StatuspageStatusResponse = serde_json::from_slice(bytes)?;
    let indicator = parse_indicator(&decoded.status.indicator);
    let description = non_blank_string(decoded.status.description);
    let updated_at = parse_rfc3339_timestamp(
        decoded
            .page
            .as_ref()
            .and_then(|page| page.updated_at.as_deref()),
    );
    Ok(ProviderStatus {
        indicator,
        description,
        updated_at,
        page_url: None,
        components: Vec::new(),
    })
}

#[derive(Deserialize)]
struct StatuspageComponentsResponse {
    #[serde(default)]
    components: Vec<StatuspageComponentPayload>,
}

#[derive(Deserialize)]
struct StatuspageComponentPayload {
    id: String,
    name: String,
    status: String,
    position: Option<i64>,
    group_id: Option<String>,
    group: Option<bool>,
}

/// Parses a Statuspage `api/v2/components.json` payload into a sorted, grouped component tree.
pub fn parse_statuspage_components(
    bytes: &[u8],
) -> Result<Vec<StatuspageComponentSummary>, StatusParseError> {
    let decoded: StatuspageComponentsResponse = serde_json::from_slice(bytes)?;
    let mut visible: Vec<(usize, StatuspageComponentPayload)> = decoded
        .components
        .into_iter()
        .filter_map(|mut component| {
            let trimmed = component.name.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                component.name = trimmed;
                Some(component)
            }
        })
        .enumerate()
        .collect();
    visible.sort_by_key(|(idx, component)| (component.position.unwrap_or(0), *idx));

    let mut children_by_group_id: HashMap<String, Vec<StatuspageComponentSummary>> = HashMap::new();
    for (_, component) in &visible {
        if component.group == Some(true) {
            continue;
        }
        if let Some(group_id) = &component.group_id {
            children_by_group_id
                .entry(group_id.clone())
                .or_default()
                .push(StatuspageComponentSummary {
                    id: component.id.clone(),
                    name: component.name.clone(),
                    status: component.status.clone(),
                    indicator: indicator_for_statuspage_status(&component.status),
                    children: Vec::new(),
                });
        }
    }

    let mut summaries = Vec::new();
    for (_, component) in visible {
        if component.group != Some(true) && component.group_id.is_some() {
            continue;
        }
        let children = if component.group == Some(true) {
            children_by_group_id
                .remove(&component.id)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let indicator = indicator_for_statuspage_status(&component.status);
        summaries.push(StatuspageComponentSummary {
            id: component.id,
            name: component.name,
            status: component.status,
            indicator,
            children,
        });
    }

    Ok(summaries)
}

#[derive(Deserialize)]
struct IncidentIOSummaryEnvelope {
    summary: IncidentIOSummaryPayload,
}

#[derive(Deserialize)]
struct IncidentIOSummaryPayload {
    #[serde(default)]
    affected_components: Vec<IncidentIOAffectedComponent>,
    structure: Option<IncidentIOStructure>,
}

#[derive(Deserialize)]
struct IncidentIOAffectedComponent {
    component_id: String,
    status: String,
}

#[derive(Deserialize)]
struct IncidentIOStructure {
    #[serde(default)]
    items: Vec<IncidentIOStructureItem>,
}

#[derive(Deserialize)]
struct IncidentIOStructureItem {
    group: Option<IncidentIOComponentGroup>,
    component: Option<IncidentIOComponent>,
}

#[derive(Deserialize)]
struct IncidentIOComponentGroup {
    id: String,
    name: String,
    hidden: Option<bool>,
    #[serde(default)]
    components: Vec<IncidentIOComponent>,
}

#[derive(Deserialize)]
struct IncidentIOComponent {
    id: String,
    name: String,
    hidden: Option<bool>,
}

/// Parses an incident.io `/proxy/<host>` summary payload into a `ProviderStatusSummary`.
pub fn parse_incident_io_summary(bytes: &[u8]) -> Result<ProviderStatusSummary, StatusParseError> {
    let decoded: IncidentIOSummaryEnvelope = serde_json::from_slice(bytes)?;
    let mut status_by_component_id: HashMap<String, String> = HashMap::new();
    for affected in decoded.summary.affected_components {
        status_by_component_id
            .entry(affected.component_id)
            .or_insert(affected.status);
    }

    let mut components = Vec::new();
    let mut leaf_indicators = Vec::new();

    if let Some(structure) = decoded.summary.structure {
        for item in structure.items {
            if let Some(group) = item.group
                && group.hidden != Some(true)
            {
                let mut children = Vec::new();
                for child in group.components {
                    if child.hidden == Some(true) {
                        continue;
                    }
                    let raw_status = status_by_component_id
                        .get(&child.id)
                        .map(String::as_str)
                        .unwrap_or("operational");
                    let indicator = indicator_for_statuspage_status(raw_status);
                    leaf_indicators.push(indicator);
                    children.push(StatuspageComponentSummary {
                        id: child.id,
                        name: child.name,
                        status: raw_status.to_string(),
                        indicator,
                        children: Vec::new(),
                    });
                }

                let (group_status, group_indicator) = worst_component_state(&children);
                components.push(StatuspageComponentSummary {
                    id: group.id,
                    name: group.name,
                    status: group_status,
                    indicator: group_indicator,
                    children,
                });
            } else if let Some(component) = item.component
                && component.hidden != Some(true)
            {
                let raw_status = status_by_component_id
                    .get(&component.id)
                    .map(String::as_str)
                    .unwrap_or("operational");
                let indicator = indicator_for_statuspage_status(raw_status);
                leaf_indicators.push(indicator);
                components.push(StatuspageComponentSummary {
                    id: component.id,
                    name: component.name,
                    status: raw_status.to_string(),
                    indicator,
                    children: Vec::new(),
                });
            }
        }
    }

    let overall_indicator = leaf_indicators
        .into_iter()
        .max_by_key(|indicator| status_indicator_rank(*indicator))
        .unwrap_or(ProviderStatusIndicator::None);

    let protocol_components = components
        .iter()
        .map(|c| c.to_status_component(None))
        .collect();

    Ok(ProviderStatusSummary {
        status: ProviderStatus {
            indicator: overall_indicator,
            description: None,
            updated_at: None,
            page_url: None,
            components: protocol_components,
        },
        components,
    })
}

/// Overlays `description` and `updated_at` from `api/v2/status.json` onto an incident.io summary.
pub fn overlay_incident_io_with_statuspage(
    summary: ProviderStatusSummary,
    status_page: Option<ProviderStatus>,
) -> ProviderStatusSummary {
    let trimmed_description = status_page
        .as_ref()
        .and_then(|sp| sp.description.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let description = trimmed_description.or(summary.status.description);
    let updated_at = status_page
        .and_then(|sp| sp.updated_at)
        .or(summary.status.updated_at);
    let protocol_components = summary
        .components
        .iter()
        .map(|c| c.to_status_component(updated_at))
        .collect();

    ProviderStatusSummary {
        status: ProviderStatus {
            indicator: summary.status.indicator,
            description,
            updated_at,
            page_url: summary.status.page_url,
            components: protocol_components,
        },
        components: summary.components,
    }
}

fn worst_component_state(
    children: &[StatuspageComponentSummary],
) -> (String, ProviderStatusIndicator) {
    let Some(worst) = children
        .iter()
        .max_by_key(|child| status_indicator_rank(child.indicator))
    else {
        return ("operational".to_string(), ProviderStatusIndicator::None);
    };
    (worst.status.clone(), worst.indicator)
}

#[derive(Deserialize)]
struct WorkspaceIncident {
    begin: Option<String>,
    end: Option<String>,
    modified: Option<String>,
    external_desc: Option<String>,
    status_impact: Option<String>,
    severity: Option<String>,
    affected_products: Option<Vec<WorkspaceProduct>>,
    currently_affected_products: Option<Vec<WorkspaceProduct>>,
    most_recent_update: Option<WorkspaceUpdate>,
    updates: Option<Vec<WorkspaceUpdate>>,
}

#[derive(Deserialize)]
struct WorkspaceProduct {
    id: Option<String>,
}

#[derive(Deserialize)]
struct WorkspaceUpdate {
    when: Option<String>,
    status: Option<String>,
    text: Option<String>,
}

impl WorkspaceIncident {
    fn is_active(&self) -> bool {
        self.end.is_none()
    }

    fn is_relevant(&self, product_id: &str) -> bool {
        if let Some(current) = &self.currently_affected_products {
            return current
                .iter()
                .any(|product| product.id.as_deref() == Some(product_id));
        }
        if let Some(affected) = &self.affected_products {
            return affected
                .iter()
                .any(|product| product.id.as_deref() == Some(product_id));
        }
        false
    }
}

fn workspace_indicator(status: Option<&str>, severity: Option<&str>) -> ProviderStatusIndicator {
    if let Some(raw) = status {
        match raw.trim().to_ascii_uppercase().as_str() {
            "AVAILABLE" => return ProviderStatusIndicator::None,
            "SERVICE_INFORMATION" => return ProviderStatusIndicator::Minor,
            "SERVICE_DISRUPTION" => return ProviderStatusIndicator::Major,
            "SERVICE_OUTAGE" => return ProviderStatusIndicator::Critical,
            "SERVICE_MAINTENANCE" | "SCHEDULED_MAINTENANCE" => {
                return ProviderStatusIndicator::Maintenance;
            }
            _ => {}
        }
    }
    if let Some(raw) = severity {
        match raw.trim().to_ascii_lowercase().as_str() {
            "low" => return ProviderStatusIndicator::Minor,
            "medium" => return ProviderStatusIndicator::Major,
            "high" => return ProviderStatusIndicator::Critical,
            _ => {}
        }
    }
    ProviderStatusIndicator::Minor
}

static MARKDOWN_LINK_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\([^)]+\)").expect("valid regex"));

/// Extracts a clean one-line summary from a Google Workspace incident update or description.
pub fn workspace_summary(text: Option<&str>) -> Option<String> {
    let text = text?;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for raw_line in normalized.split('\n') {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("**summary")
            || lower.starts_with("**description")
            || lower == "summary"
        {
            continue;
        }
        let without_bold = trimmed.replace("**", "");
        let without_links = MARKDOWN_LINK_REGEX
            .replace_all(&without_bold, "$1")
            .into_owned();
        let mut cleaned = without_links.trim();
        if let Some(stripped) = cleaned.strip_prefix("- ") {
            cleaned = stripped.trim();
        }
        if !cleaned.is_empty() {
            return Some(cleaned.to_string());
        }
    }
    None
}

/// Parses a Google Workspace `incidents.json` payload for a specific `product_id`.
pub fn parse_google_workspace_status(
    bytes: &[u8],
    product_id: &str,
) -> Result<ProviderStatus, StatusParseError> {
    let incidents: Vec<WorkspaceIncident> = serde_json::from_slice(bytes)?;
    let mut best: Option<(
        &WorkspaceIncident,
        ProviderStatusIndicator,
        Option<&WorkspaceUpdate>,
    )> = None;

    for incident in &incidents {
        if !incident.is_active() || !incident.is_relevant(product_id) {
            continue;
        }
        let update = incident
            .most_recent_update
            .as_ref()
            .or_else(|| incident.updates.as_ref().and_then(|u| u.last()));
        let status_raw = update
            .and_then(|u| u.status.as_deref())
            .or(incident.status_impact.as_deref());
        let indicator = workspace_indicator(status_raw, incident.severity.as_deref());
        let replace = match best {
            None => true,
            Some((_, current_best, _)) => {
                status_indicator_rank(indicator) > status_indicator_rank(current_best)
            }
        };
        if replace {
            best = Some((incident, indicator, update));
        }
    }

    let Some((incident, indicator, update)) = best else {
        return Ok(ProviderStatus {
            indicator: ProviderStatusIndicator::None,
            description: None,
            updated_at: None,
            page_url: None,
            components: Vec::new(),
        });
    };

    let description = workspace_summary(
        update
            .and_then(|u| u.text.as_deref())
            .or(incident.external_desc.as_deref()),
    );
    let updated_raw = update
        .and_then(|u| u.when.as_deref())
        .or(incident.modified.as_deref())
        .or(incident.begin.as_deref());
    let updated_at = parse_rfc3339_timestamp(updated_raw);

    Ok(ProviderStatus {
        indicator,
        description,
        updated_at,
        page_url: None,
        components: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_statuspage_status_with_fractional_timestamp() {
        let json = br#"{
          "page": { "updated_at": "2025-12-04T19:15:23.123Z" },
          "status": { "indicator": "minor", "description": "Partial System Outage" }
        }"#;
        let status = parse_statuspage_status(json).expect("valid statuspage json");
        assert_eq!(status.indicator, ProviderStatusIndicator::Minor);
        assert_eq!(status.description.as_deref(), Some("Partial System Outage"));
        assert!(status.updated_at.is_some());
    }

    #[test]
    fn parses_statuspage_components_into_grouped_summaries() {
        let json = br#"{
          "components": [
            {
              "id": "group-web",
              "name": "claude.ai",
              "status": "degraded_performance",
              "position": 1,
              "group_id": null,
              "group": true
            },
            {
              "id": "child-login",
              "name": "Login",
              "status": "degraded_performance",
              "position": 2,
              "group_id": "group-web",
              "group": false
            },
            {
              "id": "standalone-code",
              "name": "Claude Code",
              "status": "operational",
              "position": 3,
              "group_id": null,
              "group": false
            }
          ]
        }"#;

        let components = parse_statuspage_components(json).expect("valid components json");
        assert_eq!(components.len(), 2);
        assert_eq!(components[0].name, "claude.ai");
        assert_eq!(components[0].indicator, ProviderStatusIndicator::Minor);
        assert_eq!(components[0].children.len(), 1);
        assert_eq!(components[0].children[0].name, "Login");
        assert_eq!(components[1].name, "Claude Code");
        assert_eq!(components[1].indicator, ProviderStatusIndicator::None);
    }

    #[test]
    fn parses_incident_io_summary_into_grouped_components() {
        let json = br#"{
          "summary": {
            "affected_components": [
              {
                "component_id": "comp-claude-code",
                "status": "degraded_performance"
              }
            ],
            "ongoing_incidents": [],
            "scheduled_maintenances": [],
            "structure": {
              "items": [
                {
                  "group": {
                    "id": "group-1",
                    "name": "Public Beta",
                    "hidden": false,
                    "components": [
                      {
                        "id": "comp-claude-ai",
                        "name": "claude.ai",
                        "hidden": false
                      },
                      {
                        "id": "comp-claude-code",
                        "name": "Claude Code",
                        "hidden": false
                      }
                    ]
                  },
                  "component": null
                }
              ]
            }
          }
        }"#;

        let summary = parse_incident_io_summary(json).expect("valid incident.io json");
        assert_eq!(summary.status.indicator, ProviderStatusIndicator::Minor);
        assert_eq!(summary.components.len(), 1);
        assert_eq!(summary.components[0].name, "Public Beta");
        assert_eq!(
            summary.components[0].indicator,
            ProviderStatusIndicator::Minor
        );
        assert_eq!(summary.components[0].children.len(), 2);
        assert_eq!(summary.components[0].children[0].name, "claude.ai");
        assert_eq!(
            summary.components[0].children[0].indicator,
            ProviderStatusIndicator::None
        );
        assert_eq!(summary.components[0].children[1].name, "Claude Code");
        assert_eq!(
            summary.components[0].children[1].indicator,
            ProviderStatusIndicator::Minor
        );
    }

    #[test]
    fn parses_google_workspace_status_for_product() {
        let json = br#"[
          {
            "begin": "2026-03-01T10:00:00Z",
            "end": null,
            "modified": "2026-03-01T11:00:00Z",
            "status_impact": "SERVICE_DISRUPTION",
            "severity": "medium",
            "currently_affected_products": [
              { "id": "npdyhgECDJ6tB66MxXyo" }
            ],
            "most_recent_update": {
              "when": "2026-03-01T11:30:00Z",
              "status": "SERVICE_DISRUPTION",
              "text": "**Summary**\n- Elevated error rates in [Gemini](https://gemini.google.com)."
            }
          }
        ]"#;

        let status =
            parse_google_workspace_status(json, "npdyhgECDJ6tB66MxXyo").expect("valid workspace");
        assert_eq!(status.indicator, ProviderStatusIndicator::Major);
        assert_eq!(
            status.description.as_deref(),
            Some("Elevated error rates in Gemini.")
        );
        assert_eq!(
            status.updated_at.map(|ts| ts.to_string()).as_deref(),
            Some("2026-03-01T11:30:00Z")
        );

        let unaffected =
            parse_google_workspace_status(json, "other-product-id").expect("valid workspace");
        assert_eq!(unaffected.indicator, ProviderStatusIndicator::None);
        assert!(unaffected.description.is_none());
    }

    #[test]
    fn parse_statuspage_components_maps_and_sorts_leaf_rows() {
        let json = br#"{
          "components": [
            {"id": "c-cli", "name": "CLI", "status": "operational", "position": 2},
            {"id": "c-api", "name": "Codex API", "status": "major_outage", "position": 1},
            {"id": "c-fed", "name": "FedRAMP", "status": "degraded_performance", "position": 25}
          ]
        }"#;
        let components = parse_statuspage_components(json).expect("valid components json");
        let names: Vec<&str> = components.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Codex API", "CLI", "FedRAMP"]);
        let indicators: Vec<ProviderStatusIndicator> =
            components.iter().map(|c| c.indicator).collect();
        assert_eq!(
            indicators,
            vec![
                ProviderStatusIndicator::Critical,
                ProviderStatusIndicator::None,
                ProviderStatusIndicator::Minor,
            ]
        );
        assert!(components.iter().all(|c| c.children.is_empty()));
        assert_eq!(components.last().unwrap().status, "degraded_performance");
    }

    #[test]
    fn parse_statuspage_components_tolerates_missing_components_and_drops_blank_names() {
        let empty = parse_statuspage_components(b"{}").expect("tolerates missing components");
        assert!(empty.is_empty());

        let json = br#"{
          "components": [
            {"id": "blank", "name": "   ", "status": "operational", "position": 1},
            {"id": "api", "name": " API ", "status": "operational", "position": 2}
          ]
        }"#;
        let components = parse_statuspage_components(json).expect("valid components json");
        let names: Vec<&str> = components.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["API"]);
    }

    #[test]
    fn parse_workspace_status_selects_worst_incident_and_ignores_resolved_incidents() {
        let product_id = "npdyhgECDJ6tB66MxXyo";
        let worst_json = br#"[
          {
            "id": "inc-1",
            "begin": "2025-12-02T09:00:00+00:00",
            "end": null,
            "affected_products": [
              {"title": "Gemini", "id": "npdyhgECDJ6tB66MxXyo"}
            ],
            "most_recent_update": {
              "when": "2025-12-02T10:00:00+00:00",
              "status": "SERVICE_INFORMATION",
              "text": "**Summary**\nMinor issue.\n"
            }
          },
          {
            "id": "inc-2",
            "begin": "2025-12-02T11:00:00+00:00",
            "end": null,
            "affected_products": [
              {"title": "Gemini", "id": "npdyhgECDJ6tB66MxXyo"}
            ],
            "most_recent_update": {
              "when": "2025-12-02T12:00:00+00:00",
              "status": "SERVICE_OUTAGE",
              "text": "**Summary**\nGemini API error.\n"
            }
          }
        ]"#;
        let status =
            parse_google_workspace_status(worst_json, product_id).expect("worst workspace status");
        assert_eq!(status.indicator, ProviderStatusIndicator::Critical);
        assert_eq!(status.description.as_deref(), Some("Gemini API error."));
        assert!(status.updated_at.is_some());

        let resolved_json = br#"[
          {
            "id": "inc-3",
            "begin": "2025-12-02T08:00:00+00:00",
            "end": "2025-12-02T09:00:00+00:00",
            "affected_products": [
              {"title": "Gemini", "id": "npdyhgECDJ6tB66MxXyo"}
            ],
            "most_recent_update": {
              "when": "2025-12-02T09:00:00+00:00",
              "status": "AVAILABLE",
              "text": "**Summary**\nResolved.\n"
            }
          }
        ]"#;
        let resolved =
            parse_google_workspace_status(resolved_json, product_id).expect("resolved workspace");
        assert_eq!(resolved.indicator, ProviderStatusIndicator::None);
        assert!(resolved.description.is_none());
    }
}
