// Ported from CodexBar Sources/CodexBarCore/Providers/Antigravity/AntigravityLocalReader.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/CostUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use jiff::Timestamp;
use jiff::tz::{Offset, TimeZone};
use serde::{Deserialize, Serialize};

use super::local_scan::{Budget, Context, Limits, ScanFailure, Statistics};
use super::proto_reader::{CheckedSum, ParsedTurn, ParsedUsage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct DailyReportResult {
    pub report: CostUsageDailyReport,
    pub coverage: Coverage,
    pub statistics: Statistics,
    /// Two sources disagreed about the same request's payload, so the rows that survived may
    /// be wrong rather than merely incomplete. Unlike a truncated scan, this evidence cannot
    /// be published as a lower bound.
    pub evidence_is_contradicted: bool,
    /// An immutable SQLite read observed the database change beneath it, so its rows cannot
    /// be established as one valid snapshot and must not be published.
    pub evidence_is_unstable: bool,
}

impl DailyReportResult {
    #[must_use]
    pub fn new(
        report: CostUsageDailyReport,
        coverage: Coverage,
        statistics: Statistics,
        evidence_is_contradicted: bool,
        evidence_is_unstable: bool,
    ) -> Self {
        Self {
            report,
            coverage,
            statistics,
            evidence_is_contradicted,
            evidence_is_unstable,
        }
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.coverage == Coverage::Complete
    }

    #[must_use]
    pub fn is_available(&self) -> bool {
        self.coverage == Coverage::Complete
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Event {
    pub session: String,
    pub row: i64,
    pub turn: ParsedTurn,
    pub cache_write: usize,
    pub input: usize,
    pub total: usize,
}

impl std::fmt::Debug for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Event")
            .field("has_session", &!self.session.is_empty())
            .field("row", &self.row)
            .field("turn", &self.turn)
            .field("cache_write", &self.cache_write)
            .field("input", &self.input)
            .field("total", &self.total)
            .finish()
    }
}

impl Event {
    #[must_use]
    pub fn new(session: String, row: i64, turn: ParsedTurn, cache_write: usize) -> Option<Self> {
        let usage = turn.usage.as_ref()?;
        turn.timestamp_ms?;
        let total = CheckedSum::integers(&[
            usage.new_input,
            usage.output,
            usage.cache_read,
            cache_write,
            usage.reasoning,
        ])?;
        if total > usize::try_from(i64::MAX).unwrap_or(usize::MAX) {
            return None;
        }
        let input = usage.new_input;
        Some(Self {
            session,
            row,
            turn,
            cache_write,
            input,
            total,
        })
    }
}

#[derive(Debug, Clone)]
pub struct SourceResult {
    pub events: Vec<Event>,
    pub is_complete: bool,
    pub contains_history_source: bool,
    pub evidence_is_unstable: bool,
}

impl Default for SourceResult {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            is_complete: true,
            contains_history_source: false,
            evidence_is_unstable: false,
        }
    }
}

impl SourceResult {
    #[must_use]
    pub fn incomplete() -> Self {
        Self {
            is_complete: false,
            ..Self::default()
        }
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct RowIdentity {
    session: String,
    row: i64,
}

impl std::fmt::Debug for RowIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowIdentity")
            .field("has_session", &!self.session.is_empty())
            .field("row", &self.row)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ResponseIdentity {
    session: String,
    response: String,
}

impl std::fmt::Debug for ResponseIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseIdentity")
            .field("has_session", &!self.session.is_empty())
            .field("has_response", &!self.response.is_empty())
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct LabelIdentity {
    session: String,
    label: String,
}

impl std::fmt::Debug for LabelIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelIdentity")
            .field("has_session", &!self.session.is_empty())
            .field("has_label", &!self.label.is_empty())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CostProvenance {
    ListPriceEstimate,
    VendorMetered,
    Mixed,
    Unknown,
}

impl CostProvenance {
    #[must_use]
    pub fn for_window(snapshot: Self, has_window_costs: bool, includes_metered: bool) -> Self {
        match snapshot {
            Self::VendorMetered => {
                if includes_metered || has_window_costs {
                    Self::VendorMetered
                } else {
                    Self::Unknown
                }
            }
            Self::Mixed => match (includes_metered, has_window_costs) {
                (true, true) => Self::Mixed,
                (true, false) => Self::VendorMetered,
                (false, true) => Self::ListPriceEstimate,
                (false, false) => Self::Unknown,
            },
            Self::ListPriceEstimate => {
                if has_window_costs {
                    Self::ListPriceEstimate
                } else {
                    Self::Unknown
                }
            }
            Self::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostUsageCoverageCounts {
    pub priced: usize,
    pub unpriced: usize,
    pub unmetered: usize,
    pub estimated: usize,
}

impl CostUsageCoverageCounts {
    #[must_use]
    pub fn new(priced: usize, unpriced: usize, unmetered: usize, estimated: usize) -> Self {
        Self {
            priced,
            unpriced,
            unmetered,
            estimated,
        }
    }

    pub fn merge(&mut self, other: Self) {
        self.priced = self.priced.saturating_add(other.priced);
        self.unpriced = self.unpriced.saturating_add(other.unpriced);
        self.unmetered = self.unmetered.saturating_add(other.unmetered);
        self.estimated = self.estimated.saturating_add(other.estimated);
    }
}

/// Token-class mix. `None` means the source did not establish that class — never treat as zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostUsageTokenMix {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<usize>,
    #[serde(skip)]
    overflowed_classes: u8,
}

impl CostUsageTokenMix {
    #[must_use]
    pub fn new(
        input_tokens: Option<usize>,
        output_tokens: Option<usize>,
        cache_read_tokens: Option<usize>,
        cache_creation_tokens: Option<usize>,
        reasoning_tokens: Option<usize>,
    ) -> Self {
        Self {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
            reasoning_tokens,
            overflowed_classes: 0,
        }
    }

    #[must_use]
    pub fn from_entry(entry: &CostUsageDailyReportEntry) -> Self {
        Self::new(
            entry.input_tokens,
            entry.output_tokens,
            entry.cache_read_tokens,
            entry.cache_creation_tokens,
            entry.reasoning_tokens,
        )
    }

    pub fn merge(&mut self, other: Self) {
        self.overflowed_classes |= other.overflowed_classes;
        self.input_tokens = self.add(self.input_tokens, other.input_tokens, 1);
        self.output_tokens = self.add(self.output_tokens, other.output_tokens, 2);
        self.cache_read_tokens = self.add(self.cache_read_tokens, other.cache_read_tokens, 4);
        self.cache_creation_tokens =
            self.add(self.cache_creation_tokens, other.cache_creation_tokens, 8);
        self.reasoning_tokens = self.add(self.reasoning_tokens, other.reasoning_tokens, 16);
    }

    fn add(&mut self, lhs: Option<usize>, rhs: Option<usize>, bit: u8) -> Option<usize> {
        if self.overflowed_classes & bit != 0 {
            return None;
        }
        match (lhs, rhs) {
            (Some(left), Some(right)) => {
                if let Some(sum) = AntigravityLocalReader::checked_add(left, right) {
                    Some(sum)
                } else {
                    self.overflowed_classes |= bit;
                    None
                }
            }
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostUsageModelBreakdown {
    pub model_name: String,
    pub cost_usd: Option<f64>,
    pub total_tokens: Option<usize>,
    pub request_count: Option<usize>,
    pub input_tokens: Option<usize>,
    pub output_tokens: Option<usize>,
    pub cache_read_tokens: Option<usize>,
    pub cache_creation_tokens: Option<usize>,
    pub reasoning_tokens: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostUsageDailyReportEntry {
    pub date: String,
    pub input_tokens: Option<usize>,
    pub output_tokens: Option<usize>,
    pub cache_read_tokens: Option<usize>,
    pub cache_creation_tokens: Option<usize>,
    pub reasoning_tokens: Option<usize>,
    pub total_tokens: Option<usize>,
    pub request_count: Option<usize>,
    pub cost_usd: Option<f64>,
    pub models_used: Option<Vec<String>>,
    pub model_breakdowns: Option<Vec<CostUsageModelBreakdown>>,
    pub unpriced_request_count: Option<usize>,
    pub estimated_request_count: Option<usize>,
}

impl CostUsageDailyReportEntry {
    #[must_use]
    pub fn coverage_counts(&self) -> CostUsageCoverageCounts {
        let unpriced = self.unpriced_request_count.unwrap_or(0);
        let estimated = self.estimated_request_count.unwrap_or(0);
        if let Some(requests) = self.request_count
            && requests > 0
        {
            let remainder = requests.saturating_sub(unpriced).saturating_sub(estimated);
            let has_cost = self.cost_usd.is_some();
            return CostUsageCoverageCounts {
                priced: if has_cost { remainder } else { 0 },
                unpriced: if has_cost {
                    unpriced
                } else {
                    unpriced.saturating_add(remainder)
                },
                unmetered: 0,
                estimated,
            };
        }
        if unpriced > 0 || estimated > 0 {
            return CostUsageCoverageCounts {
                priced: 0,
                unpriced,
                unmetered: 0,
                estimated,
            };
        }
        if self.cost_usd.is_some() {
            return CostUsageCoverageCounts {
                priced: 1,
                ..CostUsageCoverageCounts::default()
            };
        }
        if self.total_tokens.unwrap_or(0) > 0 {
            return CostUsageCoverageCounts {
                unpriced: 1,
                ..CostUsageCoverageCounts::default()
            };
        }
        CostUsageCoverageCounts::default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostUsageDailyReportSummary {
    pub total_input_tokens: Option<usize>,
    pub total_output_tokens: Option<usize>,
    pub total_tokens: Option<usize>,
    pub total_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostUsageDailyReport {
    pub data: Vec<CostUsageDailyReportEntry>,
    pub summary: Option<CostUsageDailyReportSummary>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostUsageWindowSummary {
    pub days: usize,
    pub total_tokens: Option<usize>,
    pub total_cost_usd: Option<f64>,
    pub total_requests: Option<usize>,
    pub entry_count: usize,
    pub token_mix: CostUsageTokenMix,
    pub coverage: CostUsageCoverageCounts,
    pub provenance: CostProvenance,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostUsageTokenSnapshot {
    pub session_tokens: Option<usize>,
    pub session_cost_usd: Option<f64>,
    pub session_requests: Option<usize>,
    pub last_30_days_tokens: Option<usize>,
    pub last_30_days_cost_usd: Option<f64>,
    pub last_30_days_requests: Option<usize>,
    pub currency_code: String,
    pub history_days: usize,
    pub history_coverage_is_established: bool,
    pub history_scan_is_partial: bool,
    pub cost_provenance: CostProvenance,
    pub daily: Vec<CostUsageDailyReportEntry>,
    pub updated_at: Timestamp,
}

impl CostUsageTokenSnapshot {
    #[must_use]
    pub fn history_is_fully_scanned(&self) -> bool {
        self.history_coverage_is_established && !self.history_scan_is_partial
    }

    #[must_use]
    pub fn summary(&self, requested_days: usize, tz_offset_seconds: i32) -> CostUsageWindowSummary {
        let days = requested_days.max(1);
        let entries = filter_rolling_entries(&self.daily, days, self.updated_at, tz_offset_seconds);
        let costs: Vec<f64> = entries.iter().filter_map(|e| e.cost_usd).collect();
        let tokens: Vec<usize> = entries.iter().filter_map(|e| e.total_tokens).collect();
        let requests: Vec<usize> = entries.iter().filter_map(|e| e.request_count).collect();
        let mut mix = CostUsageTokenMix::default();
        let mut coverage = CostUsageCoverageCounts::default();
        for entry in &entries {
            mix.merge(CostUsageTokenMix::from_entry(entry));
            coverage.merge(entry.coverage_counts());
        }
        let total_tokens = if tokens.is_empty() {
            None
        } else {
            CheckedSum::integers(&tokens)
        };
        let total_requests = if requests.is_empty() {
            None
        } else {
            CheckedSum::integers(&requests)
        };
        CostUsageWindowSummary {
            days,
            total_tokens,
            total_cost_usd: if costs.is_empty() {
                None
            } else {
                Some(costs.iter().sum())
            },
            total_requests,
            entry_count: entries.len(),
            token_mix: mix,
            coverage,
            provenance: CostProvenance::for_window(self.cost_provenance, !costs.is_empty(), false),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelsDevCost {
    pub input: Option<f64>,
    pub output: Option<f64>,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelsDevModel {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cost: Option<ModelsDevCost>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelsDevProvider {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub models: BTreeMap<String, ModelsDevModel>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelsDevCatalog {
    pub providers: BTreeMap<String, ModelsDevProvider>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ModelsDevCacheArtifact {
    version: usize,
    #[serde(rename = "fetchedAt")]
    fetched_at: String,
    catalog: BTreeMap<String, BTreeMap<String, ModelsDevProvider>>,
}

pub struct ModelsDevCache;

impl ModelsDevCache {
    pub const ARTIFACT_VERSION: usize = 1;

    #[must_use]
    pub fn cache_file_url(cache_root: &Path) -> std::path::PathBuf {
        cache_root
            .join("model-pricing")
            .join(format!("models-dev-v{}.json", Self::ARTIFACT_VERSION))
    }

    pub fn save(catalog: &ModelsDevCatalog, fetched_at: Timestamp, cache_root: &Path) -> bool {
        let url = Self::cache_file_url(cache_root);
        let Some(dir) = url.parent() else {
            return false;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return false;
        }
        let mut wrapped = BTreeMap::new();
        wrapped.insert("providers".to_owned(), catalog.providers.clone());
        let artifact = ModelsDevCacheArtifact {
            version: Self::ARTIFACT_VERSION,
            fetched_at: fetched_at.to_string(),
            catalog: wrapped,
        };
        let Ok(bytes) = serde_json::to_vec(&artifact) else {
            return false;
        };
        std::fs::write(url, bytes).is_ok()
    }

    #[must_use]
    pub fn load(cache_root: &Path) -> Option<ModelsDevCatalog> {
        let url = Self::cache_file_url(cache_root);
        let bytes = std::fs::read(url).ok()?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let version = value.get("version")?.as_u64()?;
        if usize::try_from(version).ok()? != Self::ARTIFACT_VERSION {
            return None;
        }
        let catalog_val = value.get("catalog")?;
        if let Some(providers_val) = catalog_val.get("providers")
            && let Ok(providers) =
                serde_json::from_value::<BTreeMap<String, ModelsDevProvider>>(providers_val.clone())
        {
            return Some(ModelsDevCatalog { providers });
        }
        let providers =
            serde_json::from_value::<BTreeMap<String, ModelsDevProvider>>(catalog_val.clone())
                .ok()?;
        Some(ModelsDevCatalog { providers })
    }
}

#[derive(Debug, Clone, Copy)]
struct ClaudePricing {
    input_cost_per_token: f64,
    output_cost_per_token: f64,
    cache_creation_input_cost_per_token: f64,
    cache_read_input_cost_per_token: f64,
    threshold_tokens: Option<usize>,
    input_cost_per_token_above_threshold: Option<f64>,
    output_cost_per_token_above_threshold: Option<f64>,
    cache_creation_input_cost_per_token_above_threshold: Option<f64>,
    cache_read_input_cost_per_token_above_threshold: Option<f64>,
}

const CLAUDE_FULL_CONTEXT_STANDARD_PRICING_CUTOFF_SECS: i64 = 1_773_360_000;

struct ClaudePricingResolver {
    catalog: ModelsDevCatalog,
}

impl ClaudePricingResolver {
    fn new(cache_root: Option<&Path>) -> Self {
        let catalog = cache_root
            .and_then(ModelsDevCache::load)
            .unwrap_or_default();
        Self { catalog }
    }

    #[expect(clippy::too_many_arguments)]
    fn cost_usd(
        &self,
        model: &str,
        provider_id: Option<&str>,
        input_tokens: usize,
        cache_read_input_tokens: usize,
        cache_creation_input_tokens: usize,
        output_tokens: usize,
        pricing_timestamp_secs: i64,
    ) -> Option<f64> {
        if let Some(provider_id) = provider_id {
            let pricing = self.catalog_lookup(provider_id, model)?;
            return Some(Self::compute_cost(
                pricing,
                input_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
                output_tokens,
            ));
        }

        let normalized = normalize_claude_model(model);
        if let (Some(historical), Some(current)) = (
            bundled_claude_historical_long_context(&normalized),
            bundled_claude_pricing(&normalized),
        ) {
            let pricing =
                if pricing_timestamp_secs < CLAUDE_FULL_CONTEXT_STANDARD_PRICING_CUTOFF_SECS {
                    historical
                } else {
                    current
                };
            return Some(Self::compute_cost(
                pricing,
                input_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
                output_tokens,
            ));
        }

        if let Some(pricing) = self.claude_models_dev_lookup(model) {
            return Some(Self::compute_cost(
                pricing,
                input_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
                output_tokens,
            ));
        }

        let pricing = bundled_claude_pricing(&normalized)?;
        Some(Self::compute_cost(
            pricing,
            input_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens,
            output_tokens,
        ))
    }

    fn catalog_lookup(&self, provider_id: &str, model_id: &str) -> Option<ClaudePricing> {
        let norm_provider = provider_id.trim().to_lowercase();
        let provider = self.catalog.providers.get(&norm_provider)?;
        let candidate = model_id.trim();
        let model = provider
            .models
            .get(candidate)
            .or_else(|| provider.models.values().find(|m| m.id.trim() == candidate))?;
        let cost = model.cost.as_ref()?;
        let input = cost.input? / 1_000_000.0;
        let output = cost.output? / 1_000_000.0;
        let cache_read = cost.cache_read.map_or(input, |c| c / 1_000_000.0);
        let cache_write = cost.cache_write.map_or(input, |c| c / 1_000_000.0);
        Some(ClaudePricing {
            input_cost_per_token: input,
            output_cost_per_token: output,
            cache_creation_input_cost_per_token: cache_write,
            cache_read_input_cost_per_token: cache_read,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_creation_input_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
        })
    }

    fn claude_models_dev_lookup(&self, raw_model: &str) -> Option<ClaudePricing> {
        let trimmed = raw_model.trim();
        let has_explicit_route = trimmed.contains('/');
        let has_preferred_vendor = claude_preferred_provider_ids(trimmed).is_some();
        let mut matches: Vec<(String, ClaudePricing)> = Vec::new();
        for (provider_id, model_id) in
            AntigravityLocalReader::claude_models_dev_pricing_targets(raw_model)
        {
            if let Some(pricing) = self.catalog_lookup(&provider_id, &model_id) {
                if has_explicit_route || has_preferred_vendor {
                    return Some(pricing);
                }
                matches.push((provider_id, pricing));
            }
        }
        let unique_providers: BTreeSet<&str> = matches.iter().map(|(p, _)| p.as_str()).collect();
        if unique_providers.len() == 1 {
            matches.into_iter().next().map(|(_, p)| p)
        } else {
            None
        }
    }

    fn compute_cost(
        pricing: ClaudePricing,
        input_tokens: usize,
        cache_read_tokens: usize,
        cache_creation_tokens: usize,
        output_tokens: usize,
    ) -> f64 {
        let uses_long_context = pricing.threshold_tokens.is_some_and(|threshold| {
            CheckedSum::integers(&[input_tokens, cache_read_tokens, cache_creation_tokens])
                .is_none_or(|sum| sum > threshold)
        });
        let input_rate = if uses_long_context {
            pricing
                .input_cost_per_token_above_threshold
                .unwrap_or(pricing.input_cost_per_token)
        } else {
            pricing.input_cost_per_token
        };
        let cache_read_rate = if uses_long_context {
            pricing
                .cache_read_input_cost_per_token_above_threshold
                .unwrap_or(pricing.cache_read_input_cost_per_token)
        } else {
            pricing.cache_read_input_cost_per_token
        };
        let cache_creation_rate = if uses_long_context {
            pricing
                .cache_creation_input_cost_per_token_above_threshold
                .unwrap_or(pricing.cache_creation_input_cost_per_token)
        } else {
            pricing.cache_creation_input_cost_per_token
        };
        let output_rate = if uses_long_context {
            pricing
                .output_cost_per_token_above_threshold
                .unwrap_or(pricing.output_cost_per_token)
        } else {
            pricing.output_cost_per_token
        };
        (input_tokens as f64) * input_rate
            + (cache_read_tokens as f64) * cache_read_rate
            + (cache_creation_tokens as f64) * cache_creation_rate
            + (output_tokens as f64) * output_rate
    }
}

fn normalize_claude_model(raw: &str) -> String {
    let mut trimmed = raw.trim().to_owned();
    if let Some(rest) = trimmed.strip_prefix("anthropic.") {
        trimmed = rest.to_owned();
    }
    if let Some(last_dot) = trimmed.rfind('.')
        && trimmed.contains("claude-")
    {
        let tail = &trimmed[last_dot + 1..];
        if tail.starts_with("claude-") {
            trimmed = tail.to_owned();
        }
    }
    if let Some(v_idx) = trimmed.rfind("-v") {
        let suffix = &trimmed[v_idx + 2..];
        if let Some((left, right)) = suffix.split_once(':')
            && !left.is_empty()
            && left.bytes().all(|b| b.is_ascii_digit())
            && !right.is_empty()
            && right.bytes().all(|b| b.is_ascii_digit())
        {
            trimmed.truncate(v_idx);
        }
    }
    if trimmed.len() > 9 {
        let split_at = trimmed.len() - 9;
        if trimmed.is_char_boundary(split_at) {
            let suffix = &trimmed[split_at..];
            if suffix.starts_with('-') && suffix[1..].bytes().all(|b| b.is_ascii_digit()) {
                let base = &trimmed[..split_at];
                if bundled_claude_pricing(base).is_some() {
                    return base.to_owned();
                }
            }
        }
    }
    trimmed
}

fn bundled_claude_pricing(model: &str) -> Option<ClaudePricing> {
    let std_pricing = |input: f64, output: f64, creation: f64, read: f64| -> ClaudePricing {
        ClaudePricing {
            input_cost_per_token: input,
            output_cost_per_token: output,
            cache_creation_input_cost_per_token: creation,
            cache_read_input_cost_per_token: read,
            threshold_tokens: None,
            input_cost_per_token_above_threshold: None,
            output_cost_per_token_above_threshold: None,
            cache_creation_input_cost_per_token_above_threshold: None,
            cache_read_input_cost_per_token_above_threshold: None,
        }
    };
    let long_sonnet = ClaudePricing {
        input_cost_per_token: 3e-6,
        output_cost_per_token: 1.5e-5,
        cache_creation_input_cost_per_token: 3.75e-6,
        cache_read_input_cost_per_token: 3e-7,
        threshold_tokens: Some(200_000),
        input_cost_per_token_above_threshold: Some(6e-6),
        output_cost_per_token_above_threshold: Some(2.25e-5),
        cache_creation_input_cost_per_token_above_threshold: Some(7.5e-6),
        cache_read_input_cost_per_token_above_threshold: Some(6e-7),
    };
    match model {
        "claude-fable-5" => Some(std_pricing(1e-5, 5e-5, 1.25e-5, 1e-6)),
        "claude-haiku-4-5-20251001" | "claude-haiku-4-5" => {
            Some(std_pricing(1e-6, 5e-6, 1.25e-6, 1e-7))
        }
        "claude-opus-4-5-20251101"
        | "claude-opus-4-5"
        | "claude-opus-4-6-20260205"
        | "claude-opus-4-6"
        | "claude-opus-4-7"
        | "claude-opus-4-8" => Some(std_pricing(5e-6, 2.5e-5, 6.25e-6, 5e-7)),
        "claude-sonnet-4-6" => Some(std_pricing(3e-6, 1.5e-5, 3.75e-6, 3e-7)),
        "claude-sonnet-4-5" | "claude-sonnet-4-5-20250929" | "claude-sonnet-4-20250514" => {
            Some(long_sonnet)
        }
        "claude-opus-4-20250514" | "claude-opus-4-1" => {
            Some(std_pricing(1.5e-5, 7.5e-5, 1.875e-5, 1.5e-6))
        }
        _ => None,
    }
}

fn bundled_claude_historical_long_context(model: &str) -> Option<ClaudePricing> {
    match model {
        "claude-opus-4-6" => Some(ClaudePricing {
            input_cost_per_token: 5e-6,
            output_cost_per_token: 2.5e-5,
            cache_creation_input_cost_per_token: 6.25e-6,
            cache_read_input_cost_per_token: 5e-7,
            threshold_tokens: Some(200_000),
            input_cost_per_token_above_threshold: Some(1e-5),
            output_cost_per_token_above_threshold: Some(3.75e-5),
            cache_creation_input_cost_per_token_above_threshold: Some(1.25e-5),
            cache_read_input_cost_per_token_above_threshold: Some(1e-6),
        }),
        "claude-sonnet-4-6" => Some(ClaudePricing {
            input_cost_per_token: 3e-6,
            output_cost_per_token: 1.5e-5,
            cache_creation_input_cost_per_token: 3.75e-6,
            cache_read_input_cost_per_token: 3e-7,
            threshold_tokens: Some(200_000),
            input_cost_per_token_above_threshold: Some(6e-6),
            output_cost_per_token_above_threshold: Some(2.25e-5),
            cache_creation_input_cost_per_token_above_threshold: Some(7.5e-6),
            cache_read_input_cost_per_token_above_threshold: Some(6e-7),
        }),
        _ => None,
    }
}

const CLAUDE_FIRST_PARTY_PROVIDER_IDS: &[&str] = &[
    "anthropic",
    "openai",
    "google",
    "moonshot",
    "kimi-for-coding",
    "minimax",
    "deepseek",
];

fn claude_preferred_provider_ids(raw_model: &str) -> Option<&'static [&'static str]> {
    let model = normalize_claude_model(raw_model).to_lowercase();
    if model.starts_with("claude-") {
        return Some(&["anthropic"]);
    }
    let openai_reasoning = ["o1", "o3", "o4"]
        .iter()
        .any(|p| model == *p || model.starts_with(&format!("{p}-")));
    if openai_reasoning
        || ["gpt-", "chatgpt-", "text-embedding-"]
            .iter()
            .any(|p| model.starts_with(p))
    {
        return Some(&["openai"]);
    }
    if ["gemini-", "gemma-", "deep-research-", "veo-", "lyria-"]
        .iter()
        .any(|p| model.starts_with(p))
    {
        return Some(&["google"]);
    }
    if model == "kimi-for-coding" || model == "k3" || model == "k3[1m]" || model.starts_with("k3-")
    {
        return Some(&["kimi-for-coding"]);
    }
    if model.starts_with("kimi-") || model.starts_with("moonshot-") {
        return Some(&["moonshot", "kimi-for-coding"]);
    }
    if model.starts_with("minimax-") {
        return Some(&["minimax"]);
    }
    if model.starts_with("deepseek-") {
        return Some(&["deepseek"]);
    }
    None
}

pub struct AntigravityLocalReader;

const ROUTING_VARIANT_SUFFIXES: &[&str] = &["-tiered", "-low", "-thinking"];

impl AntigravityLocalReader {
    #[must_use]
    pub fn normalize_model_id(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            "unknown".to_owned()
        } else {
            trimmed.to_owned()
        }
    }

    fn pricing_model_alias(lowered: &str) -> Option<&'static str> {
        match lowered {
            "gemini-pro-default"
            | "gemini-pro-agent"
            | "gemini-3.1-pro"
            | "gemini-3.1-pro-high"
            | "gemini-3.1-pro-low" => Some("gemini-3.1-pro-preview"),
            "gemini-3.7-flash-safety-le" => Some("gemini-3.7-flash"),
            _ => None,
        }
    }

    fn explicit_catalog_entry(lowered: &str) -> Option<(&'static str, &'static str)> {
        match lowered {
            "gpt-oss-120b-medium" => Some(("google-vertex", "openai/gpt-oss-120b-maas")),
            _ => None,
        }
    }

    /// Antigravity records routing variants of a vendor model (`-tiered`, `-low`, `-thinking`)
    /// that bill at the base model's public price, and product aliases that name no catalogued
    /// model at all. The alias stays provider-local so shared Claude pricing keeps reporting
    /// unknown Claude variants as unpriced.
    #[must_use]
    pub fn pricing_base_model_id(model: &str) -> Option<String> {
        let lowered = model.to_lowercase();
        if let Some(alias) = Self::pricing_model_alias(&lowered) {
            return Some(alias.to_owned());
        }
        let suffix = ROUTING_VARIANT_SUFFIXES
            .iter()
            .find(|&&s| lowered.ends_with(s))?;
        let base_len = model.len().checked_sub(suffix.len())?;
        let base = &model[..base_len];
        if base.is_empty() {
            return None;
        }
        Some(
            Self::pricing_model_alias(&base.to_lowercase())
                .map_or_else(|| base.to_owned(), str::to_owned),
        )
    }

    #[must_use]
    pub fn claude_models_dev_pricing_targets(raw_model: &str) -> Vec<(String, String)> {
        let trimmed = raw_model.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let normalized = normalize_claude_model(trimmed);
        let model_ids: Vec<String> = if normalized == trimmed {
            vec![trimmed.to_owned()]
        } else {
            vec![trimmed.to_owned(), normalized]
        };
        if let Some((route, rest)) = trimmed.split_once('/') {
            let route_id = route.to_lowercase();
            if route_id.is_empty()
                || rest.is_empty()
                || !CLAUDE_FIRST_PARTY_PROVIDER_IDS.contains(&route_id.as_str())
            {
                return Vec::new();
            }
            let rest_norm = normalize_claude_model(rest);
            let rest_ids = if rest_norm == rest {
                vec![rest.to_owned()]
            } else {
                vec![rest.to_owned(), rest_norm]
            };
            return rest_ids
                .into_iter()
                .map(|m| (route_id.clone(), m))
                .collect();
        }
        let provider_ids =
            claude_preferred_provider_ids(trimmed).unwrap_or(CLAUDE_FIRST_PARTY_PROVIDER_IDS);
        let mut targets = Vec::new();
        for &provider_id in provider_ids {
            for model_id in &model_ids {
                targets.push((provider_id.to_owned(), model_id.clone()));
            }
        }
        targets
    }

    #[must_use]
    pub fn pricing_refresh_targets(model: &str) -> Vec<(String, String)> {
        let mut names = vec![model.to_owned()];
        if let Some(base) = Self::pricing_base_model_id(model) {
            names.push(base);
        }
        let mut targets: Vec<(String, String)> = names
            .iter()
            .flat_map(|name| Self::claude_models_dev_pricing_targets(name))
            .collect();
        if let Some((provider_id, model_id)) = Self::explicit_catalog_entry(&model.to_lowercase()) {
            targets.push((provider_id.to_owned(), model_id.to_owned()));
        }
        targets
    }

    #[must_use]
    pub fn checked_add(lhs: usize, rhs: usize) -> Option<usize> {
        let sum = lhs.checked_add(rhs)?;
        if sum <= usize::try_from(i64::MAX).unwrap_or(usize::MAX) {
            Some(sum)
        } else {
            None
        }
    }

    pub fn make_daily_report_with_status(
        context: &Context,
        tz_offset_seconds: i32,
        estimate_cost: bool,
        pricing_cache_root: Option<&Path>,
        limits: Limits,
        clock: impl Fn() -> f64 + Send + Sync + 'static,
        check_cancellation: impl Fn() -> Result<(), ScanFailure> + Send + Sync + 'static,
    ) -> Result<DailyReportResult, ScanFailure> {
        let budget = Budget::new(limits, clock, check_cancellation);
        let pricing = estimate_cost.then(|| ClaudePricingResolver::new(pricing_cache_root));
        let outcome: Result<DailyReportResult, ScanFailure> = (|| {
            let databases = Self::discover(&context.database_roots(), "db", &budget)?;
            // A discovery error is not absence and never authorizes a cache replacement.
            if !databases.paths.is_empty() || !databases.is_complete {
                let source = Self::read_databases(&databases.paths, &budget)?;
                return Self::aggregate(
                    source,
                    databases.is_complete,
                    tz_offset_seconds,
                    &budget,
                    pricing.as_ref(),
                );
            }
            let cache = Self::discover(&[context.cache_root()], "jsonl", &budget)?;
            if cache.paths.is_empty() && cache.is_complete {
                return Ok(DailyReportResult::new(
                    CostUsageDailyReport::default(),
                    Coverage::Unavailable,
                    budget.statistics(),
                    false,
                    false,
                ));
            }
            let source = Self::read_jsonl(&cache.paths, &budget)?;
            Self::aggregate(
                source,
                cache.is_complete,
                tz_offset_seconds,
                &budget,
                pricing.as_ref(),
            )
        })();
        match outcome {
            Ok(result) => Ok(result),
            Err(ScanFailure::Exhausted | ScanFailure::SchemaExhausted) => {
                Ok(DailyReportResult::new(
                    CostUsageDailyReport::default(),
                    Coverage::Partial,
                    budget.statistics(),
                    false,
                    false,
                ))
            }
            Err(err) => Err(err),
        }
    }

    pub fn load_token_snapshot(
        context: &Context,
        now: Timestamp,
        history_days: usize,
        tz_offset_seconds: i32,
        pricing_cache_root: Option<&Path>,
        check_cancellation: impl Fn() -> Result<(), ScanFailure> + Send + Sync + 'static,
    ) -> Result<CostUsageTokenSnapshot, ScanFailure> {
        let clamped_days = history_days.max(1);
        let epoch = Instant::now();
        let report_result = Self::make_daily_report_with_status(
            context,
            tz_offset_seconds,
            true,
            pricing_cache_root,
            Limits::default(),
            move || epoch.elapsed().as_secs_f64(),
            check_cancellation,
        )?;
        let can_publish = report_result.is_available()
            || (!report_result.report.data.is_empty()
                && !report_result.evidence_is_contradicted
                && !report_result.evidence_is_unstable);
        if !can_publish || (report_result.report.data.is_empty() && !report_result.is_complete()) {
            return Ok(token_snapshot_from_daily(
                &CostUsageDailyReport::default(),
                now,
                clamped_days,
                tz_offset_seconds,
                false,
                false,
                CostProvenance::Unknown,
            ));
        }
        let daily =
            windowed_daily_report(&report_result.report, clamped_days, now, tz_offset_seconds);
        let provenance = if daily
            .summary
            .as_ref()
            .and_then(|s| s.total_cost_usd)
            .is_none()
        {
            CostProvenance::Unknown
        } else {
            CostProvenance::ListPriceEstimate
        };
        Ok(token_snapshot_from_daily(
            &daily,
            now,
            clamped_days,
            tz_offset_seconds,
            report_result.is_complete(),
            !report_result.is_complete(),
            provenance,
        ))
    }

    fn aggregate(
        source: SourceResult,
        discovery_complete: bool,
        tz_offset_seconds: i32,
        budget: &Budget,
        pricing: Option<&ClaudePricingResolver>,
    ) -> Result<DailyReportResult, ScanFailure> {
        let stats = budget.statistics();
        let mut is_complete = discovery_complete
            && source.is_complete
            && stats.sqlite_handles_opened == stats.sqlite_handles_closed;
        let mut contradicted = false;
        if is_complete && !source.contains_history_source {
            return Ok(DailyReportResult::new(
                CostUsageDailyReport::default(),
                Coverage::Unavailable,
                budget.statistics(),
                false,
                false,
            ));
        }
        let mut models: BTreeMap<LabelIdentity, String> = BTreeMap::new();
        let mut conflicts: BTreeSet<LabelIdentity> = BTreeSet::new();
        for event in &source.events {
            budget.check()?;
            if let (Some(label), Some(model)) =
                (event.turn.label.as_ref(), event.turn.model.as_ref())
            {
                let key = LabelIdentity {
                    session: event.session.clone(),
                    label: label.clone(),
                };
                if let Some(prior) = models.get(&key) {
                    if prior != model {
                        conflicts.insert(key);
                    }
                } else {
                    models.insert(key, model.clone());
                }
            }
        }

        let mut rows: BTreeMap<RowIdentity, Event> = BTreeMap::new();
        let mut responses: BTreeMap<ResponseIdentity, Event> = BTreeMap::new();
        let mut entries: BTreeMap<String, CostUsageDailyReportEntry> = BTreeMap::new();
        for event in source.events {
            budget.check()?;
            let row = RowIdentity {
                session: event.session.clone(),
                row: event.row,
            };
            let Some(entry) = Self::entry(&event, &models, &conflicts, tz_offset_seconds, pricing)
            else {
                is_complete = false;
                continue;
            };
            if let Some(prior) = rows.get(&row) {
                // Same filename/session and idx identifies a copied SQLite row, not its token payload.
                if prior != &event {
                    is_complete = false;
                    contradicted = true;
                }
                continue;
            }
            let response = event
                .turn
                .usage
                .as_ref()
                .and_then(|u| u.response_id.as_ref())
                .map(|resp| ResponseIdentity {
                    session: event.session.clone(),
                    response: resp.clone(),
                });
            if let Some(ref response_key) = response
                && let Some(prior) = responses.get(response_key)
            {
                if prior.turn != event.turn || prior.cache_write != event.cache_write {
                    is_complete = false;
                    contradicted = true;
                } else {
                    rows.insert(row, event);
                }
                continue;
            }
            let next = if let Some(prior) = entries.get(&entry.date) {
                let Some(merged) = Self::checked_merge_entry(prior, &entry) else {
                    is_complete = false;
                    continue;
                };
                merged
            } else {
                entry
            };
            entries.insert(next.date.clone(), next);
            // Failed validation or aggregation must never reserve identity.
            if let Some(response_key) = response {
                responses.insert(response_key, event.clone());
            }
            rows.insert(row, event);
        }

        let daily: Vec<CostUsageDailyReportEntry> = entries.into_values().collect();
        let total = if daily.iter().all(|e| e.total_tokens.is_some()) {
            let totals: Vec<usize> = daily.iter().filter_map(|e| e.total_tokens).collect();
            CheckedSum::integers(&totals)
                .filter(|&sum| sum <= usize::try_from(i64::MAX).unwrap_or(usize::MAX))
        } else {
            None
        };
        let costs: Vec<f64> = daily.iter().filter_map(|e| e.cost_usd).collect();
        let summary = (!daily.is_empty()).then(|| CostUsageDailyReportSummary {
            total_input_tokens: None,
            total_output_tokens: None,
            total_tokens: total,
            total_cost_usd: if costs.is_empty() {
                None
            } else {
                Some(costs.iter().sum())
            },
        });
        Ok(DailyReportResult::new(
            CostUsageDailyReport {
                data: daily,
                summary,
            },
            if is_complete {
                Coverage::Complete
            } else {
                Coverage::Partial
            },
            budget.statistics(),
            contradicted,
            source.evidence_is_unstable,
        ))
    }

    fn entry(
        event: &Event,
        models: &BTreeMap<LabelIdentity, String>,
        conflicts: &BTreeSet<LabelIdentity>,
        tz_offset_seconds: i32,
        pricing: Option<&ClaudePricingResolver>,
    ) -> Option<CostUsageDailyReportEntry> {
        let usage = event.turn.usage.as_ref()?;
        let timestamp_ms = event.turn.timestamp_ms?;
        let input = event.input;
        let total = event.total;
        let label = event.turn.label.as_ref().map(|lbl| LabelIdentity {
            session: event.session.clone(),
            label: lbl.clone(),
        });
        let inherited = label.and_then(|key| {
            if conflicts.contains(&key) {
                None
            } else {
                models.get(&key).cloned()
            }
        });
        let raw_model = event
            .turn
            .model
            .as_deref()
            .or(inherited.as_deref())
            .unwrap_or("unknown");
        let model = Self::normalize_model_id(raw_model);
        let day = local_day_key_from_ms(timestamp_ms, tz_offset_seconds);
        let unattributed_zero = total == 0 && model == "unknown";
        let timestamp_secs = timestamp_ms.div_euclid(1000);
        let cost = pricing.and_then(|resolver| {
            if unattributed_zero {
                Some(0.0)
            } else {
                Self::cost_usd(resolver, &model, timestamp_secs, usage, event.cache_write)
            }
        });
        let breakdowns = if unattributed_zero {
            Vec::new()
        } else {
            vec![CostUsageModelBreakdown {
                model_name: model,
                cost_usd: cost,
                total_tokens: Some(total),
                request_count: Some(1),
                input_tokens: Some(input),
                output_tokens: Some(usage.output),
                cache_read_tokens: Some(usage.cache_read),
                cache_creation_tokens: Some(event.cache_write),
                reasoning_tokens: Some(usage.reasoning),
            }]
        };
        Some(CostUsageDailyReportEntry {
            date: day,
            input_tokens: Some(input),
            output_tokens: Some(usage.output),
            cache_read_tokens: Some(usage.cache_read),
            cache_creation_tokens: Some(event.cache_write),
            reasoning_tokens: Some(usage.reasoning),
            total_tokens: Some(total),
            request_count: Some(1),
            cost_usd: cost,
            models_used: None,
            model_breakdowns: Some(breakdowns),
            unpriced_request_count: Some(usize::from(cost.is_none())),
            estimated_request_count: Some(usize::from(cost.is_some())),
        })
    }

    fn cost_usd(
        pricing: &ClaudePricingResolver,
        model: &str,
        timestamp_secs: i64,
        usage: &ParsedUsage,
        cache_write: usize,
    ) -> Option<f64> {
        let resolve = |candidate: &str, provider_id: Option<&str>| -> Option<f64> {
            pricing.cost_usd(
                candidate,
                provider_id,
                usage.new_input,
                usage.cache_read,
                cache_write,
                usage.output.saturating_add(usage.reasoning),
                timestamp_secs,
            )
        };
        if let Some(cost) = resolve(model, None) {
            return Some(cost);
        }
        if let Some(base) = Self::pricing_base_model_id(model)
            && let Some(cost) = resolve(&base, None)
        {
            return Some(cost);
        }
        let (provider_id, model_id) = Self::explicit_catalog_entry(&model.to_lowercase())?;
        resolve(model_id, Some(provider_id))
    }

    fn checked_merge_entry(
        existing: &CostUsageDailyReportEntry,
        new: &CostUsageDailyReportEntry,
    ) -> Option<CostUsageDailyReportEntry> {
        let input = Self::checked_add(
            existing.input_tokens.unwrap_or(0),
            new.input_tokens.unwrap_or(0),
        )?;
        let output = Self::checked_add(
            existing.output_tokens.unwrap_or(0),
            new.output_tokens.unwrap_or(0),
        )?;
        let read = Self::checked_add(
            existing.cache_read_tokens.unwrap_or(0),
            new.cache_read_tokens.unwrap_or(0),
        )?;
        let creation = Self::checked_add(
            existing.cache_creation_tokens.unwrap_or(0),
            new.cache_creation_tokens.unwrap_or(0),
        )?;
        let reason = Self::checked_add(
            existing.reasoning_tokens.unwrap_or(0),
            new.reasoning_tokens.unwrap_or(0),
        )?;
        let total = Self::checked_add(
            existing.total_tokens.unwrap_or(0),
            new.total_tokens.unwrap_or(0),
        )?;
        let requests = Self::checked_add(
            existing.request_count.unwrap_or(0),
            new.request_count.unwrap_or(0),
        )?;
        let breakdowns = match (&existing.model_breakdowns, &new.model_breakdowns) {
            (Some(ex), Some(nw)) => {
                let mut merged = ex.clone();
                for b in nw {
                    merged = Self::checked_merge_breakdown(
                        &merged,
                        &b.model_name,
                        b.cost_usd,
                        b.total_tokens.unwrap_or(0),
                        b.request_count.unwrap_or(1),
                        b.input_tokens,
                        b.output_tokens,
                        b.cache_read_tokens,
                        b.cache_creation_tokens,
                        b.reasoning_tokens,
                    )?;
                }
                Some(merged)
            }
            _ => existing
                .model_breakdowns
                .clone()
                .or_else(|| new.model_breakdowns.clone()),
        };
        let unpriced_request_count = match existing.unpriced_request_count {
            Some(old) => Self::checked_add(old, new.unpriced_request_count.unwrap_or(0)),
            None => None,
        };
        let estimated_request_count = match existing.estimated_request_count {
            Some(old) => Self::checked_add(old, new.estimated_request_count.unwrap_or(0)),
            None => None,
        };
        Some(CostUsageDailyReportEntry {
            date: existing.date.clone(),
            input_tokens: Some(input),
            output_tokens: Some(output),
            cache_read_tokens: Some(read),
            cache_creation_tokens: Some(creation),
            reasoning_tokens: Some(reason),
            total_tokens: Some(total),
            request_count: Some(requests),
            cost_usd: Self::sum_costs(existing.cost_usd, new.cost_usd),
            models_used: None,
            model_breakdowns: breakdowns,
            unpriced_request_count,
            estimated_request_count,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors upstream checkedMergeBreakdown parameter list"
    )]
    fn checked_merge_breakdown(
        ex: &[CostUsageModelBreakdown],
        model: &str,
        cost_usd: Option<f64>,
        tokens: usize,
        request_count: usize,
        input_tokens: Option<usize>,
        output_tokens: Option<usize>,
        cache_read_tokens: Option<usize>,
        cache_creation_tokens: Option<usize>,
        reasoning_tokens: Option<usize>,
    ) -> Option<Vec<CostUsageModelBreakdown>> {
        let mut arr = ex.to_vec();
        if let Some(item) = arr.iter_mut().find(|b| b.model_name == model) {
            let new_total = Self::checked_add(item.total_tokens.unwrap_or(0), tokens)?;
            let new_requests = Self::checked_add(item.request_count.unwrap_or(0), request_count)?;
            let new_input =
                Self::checked_add(item.input_tokens.unwrap_or(0), input_tokens.unwrap_or(0))?;
            let new_output =
                Self::checked_add(item.output_tokens.unwrap_or(0), output_tokens.unwrap_or(0))?;
            let new_read = Self::checked_add(
                item.cache_read_tokens.unwrap_or(0),
                cache_read_tokens.unwrap_or(0),
            )?;
            let new_create = Self::checked_add(
                item.cache_creation_tokens.unwrap_or(0),
                cache_creation_tokens.unwrap_or(0),
            )?;
            let new_reason = Self::checked_add(
                item.reasoning_tokens.unwrap_or(0),
                reasoning_tokens.unwrap_or(0),
            )?;
            *item = CostUsageModelBreakdown {
                model_name: item.model_name.clone(),
                cost_usd: Self::sum_costs(item.cost_usd, cost_usd),
                total_tokens: Some(new_total),
                request_count: Some(new_requests),
                input_tokens: Some(new_input),
                output_tokens: Some(new_output),
                cache_read_tokens: Some(new_read),
                cache_creation_tokens: Some(new_create),
                reasoning_tokens: Some(new_reason),
            };
        } else {
            arr.push(CostUsageModelBreakdown {
                model_name: model.to_owned(),
                cost_usd,
                total_tokens: Some(tokens),
                request_count: Some(request_count),
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                reasoning_tokens,
            });
        }
        Some(arr)
    }

    fn sum_costs(lhs: Option<f64>, rhs: Option<f64>) -> Option<f64> {
        match (lhs, rhs) {
            (Some(l), Some(r)) => Some(l + r),
            (Some(l), None) => Some(l),
            (None, Some(r)) => Some(r),
            (None, None) => None,
        }
    }
}

fn local_day_key_from_ms(timestamp_ms: i64, tz_offset_seconds: i32) -> String {
    let offset = Offset::from_seconds(tz_offset_seconds).unwrap_or(Offset::UTC);
    let Ok(ts) = Timestamp::from_millisecond(timestamp_ms) else {
        return "0000-00-00".to_owned();
    };
    let dt_local = ts.to_zoned(TimeZone::fixed(offset));
    format!(
        "{:04}-{:02}-{:02}",
        dt_local.year(),
        dt_local.month(),
        dt_local.day()
    )
}

fn local_day_key_from_date(now: Timestamp, tz_offset_seconds: i32) -> String {
    local_day_key_from_ms(now.as_millisecond(), tz_offset_seconds)
}

fn filter_rolling_entries(
    entries: &[CostUsageDailyReportEntry],
    history_days: usize,
    now: Timestamp,
    tz_offset_seconds: i32,
) -> Vec<CostUsageDailyReportEntry> {
    let days = i64::try_from(history_days.max(1).saturating_sub(1)).unwrap_or(29);
    let start_ms = now
        .as_millisecond()
        .saturating_sub(days.saturating_mul(86_400_000));
    let start_date = Timestamp::from_millisecond(start_ms).unwrap_or(now);
    let start_key = local_day_key_from_date(start_date, tz_offset_seconds);
    let end_key = local_day_key_from_date(now, tz_offset_seconds);
    entries
        .iter()
        .filter(|e| e.date.as_str() >= start_key.as_str() && e.date.as_str() <= end_key.as_str())
        .cloned()
        .collect()
}

fn windowed_daily_report(
    report: &CostUsageDailyReport,
    history_days: usize,
    now: Timestamp,
    tz_offset_seconds: i32,
) -> CostUsageDailyReport {
    let entries = filter_rolling_entries(&report.data, history_days, now, tz_offset_seconds);
    let costs: Vec<f64> = entries.iter().filter_map(|e| e.cost_usd).collect();
    let totals: Vec<usize> = entries.iter().filter_map(|e| e.total_tokens).collect();
    let summary = (!entries.is_empty()).then(|| CostUsageDailyReportSummary {
        total_input_tokens: None,
        total_output_tokens: None,
        total_tokens: CheckedSum::integers(&totals),
        total_cost_usd: if costs.is_empty() {
            None
        } else {
            Some(costs.iter().sum())
        },
    });
    CostUsageDailyReport {
        data: entries,
        summary,
    }
}

fn token_snapshot_from_daily(
    daily: &CostUsageDailyReport,
    now: Timestamp,
    history_days: usize,
    tz_offset_seconds: i32,
    history_coverage_is_established: bool,
    history_scan_is_partial: bool,
    cost_provenance: CostProvenance,
) -> CostUsageTokenSnapshot {
    let today_key = local_day_key_from_date(now, tz_offset_seconds);
    let session_entry = daily.data.iter().find(|e| e.date == today_key);
    let requests: Vec<usize> = daily.data.iter().filter_map(|e| e.request_count).collect();
    let requests_are_complete = !requests.is_empty() && requests.len() == daily.data.len();
    let established_empty_history =
        history_coverage_is_established && !history_scan_is_partial && daily.data.is_empty();
    let empty_session: Option<usize> =
        (history_coverage_is_established && !history_scan_is_partial).then_some(0);
    let session_cost_usd =
        session_entry.map_or_else(|| empty_session.map(|v| v as f64), |e| e.cost_usd);
    let last_30_days_cost_usd = daily
        .summary
        .as_ref()
        .and_then(|s| s.total_cost_usd)
        .or_else(|| {
            if !daily.data.is_empty() && daily.data.iter().all(|e| e.cost_usd.is_some()) {
                Some(daily.data.iter().filter_map(|e| e.cost_usd).sum())
            } else if established_empty_history {
                Some(0.0)
            } else {
                None
            }
        });
    let last_30_days_tokens = daily
        .summary
        .as_ref()
        .and_then(|s| s.total_tokens)
        .or_else(|| {
            if !daily.data.is_empty() && daily.data.iter().all(|e| e.total_tokens.is_some()) {
                let vals: Vec<usize> = daily.data.iter().filter_map(|e| e.total_tokens).collect();
                CheckedSum::integers(&vals)
            } else if established_empty_history {
                Some(0)
            } else {
                None
            }
        });
    let session_requests = session_entry.map_or_else(
        || {
            if requests.is_empty() {
                None
            } else {
                empty_session
            }
        },
        |e| e.request_count,
    );
    let last_30_days_requests = if requests_are_complete {
        CheckedSum::integers(&requests)
    } else {
        None
    };

    CostUsageTokenSnapshot {
        session_tokens: session_entry.map_or(empty_session, |e| e.total_tokens),
        session_cost_usd,
        session_requests,
        last_30_days_tokens,
        last_30_days_cost_usd,
        last_30_days_requests,
        currency_code: "USD".to_owned(),
        history_days,
        history_coverage_is_established,
        history_scan_is_partial,
        cost_provenance,
        daily: daily.data.clone(),
        updated_at: now,
    }
}
