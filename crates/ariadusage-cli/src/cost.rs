// Ported from CodexBar Sources/CodexBarCLI/CLICostCommand.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/CostUsageModels.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/CostReportingPeriod.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/CostProvenance.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/CostUsageSessionBreakdown.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/UsageFormatter.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use ariadusage_engine::cost::claude::{ClaudeLogProviderFilter, ClaudeScanOptions};
use ariadusage_engine::cost::codex::{CodexParseResult, CodexScanOptions, CodexScanReport};
use ariadusage_engine::cost::ledger::{self, CostUsageDailyEntry as EngineDailyEntry};
use ariadusage_engine::cost::pricing::{self, ModelsDevCatalog};
use ariadusage_engine::cost::store::{self, CostUsageStore};
use ariadusage_engine::cost::timestamp::{self, DayKey, DayRange};
use ariadusage_protocol::cli_cost_config::{
    CostDailyEntryPayload, CostErrorPayload, CostModelBreakdownPayload, CostPayload,
    CostProjectPayload, CostProjectSourcePayload, CostTotalsPayload, CostUsageCoverageCounts,
};
use clap::Args;
use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::config::{self, EXIT_ARGS, OutputFormat, parse_output_format};

/// Exit code for unsupported remote/identified `cost` flags.
pub const EXIT_COST_UNSUPPORTED: i32 = 21;

pub const COST_ESTIMATE_HINT: &str = "Estimated from local logs · may differ from your bill";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CostProvider {
    Claude,
    Codex,
    Antigravity,
    Cursor,
    OpenCodeGo,
    Pi,
    VertexAi,
}

impl CostProvider {
    #[must_use]
    pub const fn raw_value(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
            Self::Cursor => "cursor",
            Self::OpenCodeGo => "opencodego",
            Self::Pi => "pi",
            Self::VertexAi => "vertexai",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Antigravity => "Antigravity",
            Self::Cursor => "Cursor",
            Self::OpenCodeGo => "OpenCode Go",
            Self::Pi => "Pi",
            Self::VertexAi => "Vertex AI",
        }
    }

    #[must_use]
    pub const fn supports_cost_command(self) -> bool {
        matches!(self, Self::Claude | Self::Codex)
    }

    #[must_use]
    pub const fn tokens_only_presentation(self) -> bool {
        false
    }

    #[must_use]
    pub fn from_raw(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "antigravity" => Some(Self::Antigravity),
            "cursor" => Some(Self::Cursor),
            "opencodego" | "opencode-go" => Some(Self::OpenCodeGo),
            "pi" => Some(Self::Pi),
            "vertexai" | "vertex-ai" => Some(Self::VertexAi),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CostGroupBy {
    #[default]
    None,
    Project,
    Session,
}

impl CostGroupBy {
    #[must_use]
    pub const fn requires_codex_local_sessions(self) -> bool {
        match self {
            Self::None => false,
            Self::Project | Self::Session => true,
        }
    }

    #[must_use]
    pub fn from_raw(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "project" => Some(Self::Project),
            "session" => Some(Self::Session),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CostReportingPeriod {
    Rolling { days: usize },
    MonthToDate,
    AllTime,
}

impl CostReportingPeriod {
    pub const DEFAULTS_KEY: &'static str = "costReportingPeriod";
    pub const LEGACY_DAYS_KEY: &'static str = "tokenCostUsageHistoryDays";

    #[must_use]
    pub fn from_raw(raw_value: &str) -> Option<Self> {
        match raw_value {
            "month-to-date" => Some(Self::MonthToDate),
            "all" => Some(Self::AllTime),
            _ => {
                let rest = raw_value.strip_prefix("rolling:")?;
                let days: usize = rest.parse().ok()?;
                if days == 0 {
                    return None;
                }
                Some(Self::Rolling {
                    days: days.min(365),
                })
            }
        }
    }

    #[must_use]
    pub fn raw_value(self) -> String {
        match self {
            Self::Rolling { days } => format!("rolling:{days}"),
            Self::MonthToDate => "month-to-date".to_owned(),
            Self::AllTime => "all".to_owned(),
        }
    }

    #[must_use]
    pub fn migrated(raw_value: Option<&str>, legacy_days: Option<usize>) -> Self {
        raw_value
            .and_then(Self::from_raw)
            .unwrap_or_else(|| Self::Rolling {
                days: legacy_days.unwrap_or(30).clamp(1, 365),
            })
    }

    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Rolling { days: 1 } => "Today".to_owned(),
            Self::Rolling { days } => format!("Last {days} days"),
            Self::MonthToDate => "Month to date".to_owned(),
            Self::AllTime => "All".to_owned(),
        }
    }

    #[must_use]
    pub fn bounds(
        self,
        now: Timestamp,
        timezone: &TimeZone,
        earliest: Option<Timestamp>,
    ) -> (DayKey, DayKey) {
        let end = timestamp::day_key_from_utc(now, Some(timezone.clone()));
        let start = match self {
            Self::Rolling { days } => {
                let offset = -((days.max(1) as i32) - 1);
                end.add_days(offset)
            }
            Self::MonthToDate => {
                if let Some((y, m, _)) = timestamp::parse_day_key(end.as_str()) {
                    DayKey(timestamp::format_day_key(y, m, 1))
                } else {
                    end.clone()
                }
            }
            Self::AllTime => {
                if let Some(earliest_ts) = earliest {
                    timestamp::day_key_from_utc(earliest_ts, Some(timezone.clone()))
                } else {
                    DayKey(timestamp::format_day_key(1970, 1, 1))
                }
            }
        };
        if start <= end {
            (start, end)
        } else {
            (end.clone(), end)
        }
    }

    #[must_use]
    pub fn days(self, now: Timestamp, timezone: &TimeZone, earliest: Option<Timestamp>) -> usize {
        let (start, end) = self.bounds(now, timezone, earliest);
        let Some(start_date) = timestamp::parse_day_key_date(start.as_str()) else {
            return 1;
        };
        let Some(end_date) = timestamp::parse_day_key_date(end.as_str()) else {
            return 1;
        };
        let diff = end_date
            .since(start_date)
            .map(|s| s.get_days())
            .unwrap_or(0);
        (diff.max(0) as usize) + 1
    }

    #[must_use]
    pub fn entries(
        self,
        entries: &[CostUsageDailyReportEntry],
        now: Timestamp,
        timezone: &TimeZone,
    ) -> Vec<CostUsageDailyReportEntry> {
        let (start, end) = self.bounds(now, timezone, None);
        let start_str = start.as_str();
        let end_str = end.as_str();
        entries
            .iter()
            .filter(|entry| {
                let Some(day) = CostUsageTokenSnapshot::local_day_key(&entry.date, timezone) else {
                    return false;
                };
                day.as_str() >= start_str && day.as_str() <= end_str
            })
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn identity(self, now: Timestamp, timezone: &TimeZone) -> String {
        let (start, end) = self.bounds(now, timezone, None);
        let tz_id = timezone.iana_name().unwrap_or("UTC");
        format!(
            "{}|{}|{}|{}",
            self.raw_value(),
            tz_id,
            start.as_str(),
            end.as_str()
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CostProvenance {
    ListPriceEstimate,
    VendorMetered,
    Mixed,
    #[default]
    Unknown,
}

impl CostProvenance {
    #[must_use]
    pub const fn raw_value(self) -> &'static str {
        match self {
            Self::ListPriceEstimate => "listPriceEstimate",
            Self::VendorMetered => "vendorMetered",
            Self::Mixed => "mixed",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub const fn for_window(
        snapshot: Self,
        has_window_costs: bool,
        includes_metered: bool,
    ) -> Self {
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CostUsageTokenMix {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    overflowed_classes: u8,
}

impl CostUsageTokenMix {
    #[must_use]
    pub fn new(
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
        cache_read_tokens: Option<i64>,
        cache_creation_tokens: Option<i64>,
        reasoning_tokens: Option<i64>,
    ) -> Self {
        Self {
            input_tokens: Self::nonnegative(input_tokens),
            output_tokens: Self::nonnegative(output_tokens),
            cache_read_tokens: Self::nonnegative(cache_read_tokens),
            cache_creation_tokens: Self::nonnegative(cache_creation_tokens),
            reasoning_tokens: Self::nonnegative(reasoning_tokens),
            overflowed_classes: 0,
        }
    }

    #[must_use]
    pub const fn has_any_class(&self) -> bool {
        self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.cache_read_tokens.is_some()
            || self.cache_creation_tokens.is_some()
            || self.reasoning_tokens.is_some()
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

    pub fn merge(&mut self, other: &Self) {
        self.overflowed_classes |= other.overflowed_classes;
        self.input_tokens = self.add(self.input_tokens, other.input_tokens, 1);
        self.output_tokens = self.add(self.output_tokens, other.output_tokens, 2);
        self.cache_read_tokens = self.add(self.cache_read_tokens, other.cache_read_tokens, 4);
        self.cache_creation_tokens =
            self.add(self.cache_creation_tokens, other.cache_creation_tokens, 8);
        self.reasoning_tokens = self.add(self.reasoning_tokens, other.reasoning_tokens, 16);
    }

    fn nonnegative(value: Option<i64>) -> Option<i64> {
        value.filter(|&v| v >= 0)
    }

    fn add(&mut self, lhs: Option<i64>, rhs: Option<i64>, bit: u8) -> Option<i64> {
        if self.overflowed_classes & bit != 0 {
            return None;
        }
        match (lhs, rhs) {
            (Some(left), Some(right)) => {
                if let Some(res) = left.checked_add(right) {
                    Some(res)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostUsageCoverageDetail {
    Exact,
    Requests,
    Rows,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostUsageCoverageAccumulator {
    pub exact: Option<CostUsageCoverageCounts>,
    inferred: Option<CostUsageCoverageCounts>,
    rows: CostUsageCoverageCounts,
}

impl Default for CostUsageCoverageAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl CostUsageCoverageAccumulator {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            exact: Some(CostUsageCoverageCounts::new(0, 0, 0, 0)),
            inferred: Some(CostUsageCoverageCounts::new(0, 0, 0, 0)),
            rows: CostUsageCoverageCounts::new(0, 0, 0, 0),
        }
    }

    #[must_use]
    pub fn counts(&self) -> CostUsageCoverageCounts {
        self.exact.or(self.inferred).unwrap_or(self.rows)
    }

    pub fn add(&mut self, entry: &CostUsageDailyReportEntry) {
        self.rows
            .merge(entry.coverage_counts_with_detail(CostUsageCoverageDetail::Rows));
        self.inferred = Self::adding(
            self.inferred,
            Some(entry.coverage_counts_with_detail(CostUsageCoverageDetail::Requests)),
        );
        if self.exact.is_none() {
            return;
        }
        let explicit = CostUsageCoverageCounts::new(
            entry.priced_request_count.unwrap_or(0),
            entry.unpriced_request_count.unwrap_or(0),
            entry.unmetered_request_count.unwrap_or(0),
            entry.estimated_request_count.unwrap_or(0),
        );
        if Self::adding(Some(CostUsageCoverageCounts::default()), Some(explicit)).is_none() {
            self.exact = None;
            return;
        }
        self.exact = Self::adding(self.exact, Some(entry.coverage_counts()));
    }

    pub fn merge(&mut self, other: &Self) {
        self.exact = Self::adding(self.exact, other.exact);
        self.inferred = Self::adding(self.inferred, other.inferred);
        self.rows.merge(other.rows);
    }

    fn adding(
        lhs: Option<CostUsageCoverageCounts>,
        rhs: Option<CostUsageCoverageCounts>,
    ) -> Option<CostUsageCoverageCounts> {
        let lhs = lhs?;
        let rhs = rhs?;
        let priced = lhs.priced.checked_add(rhs.priced)?;
        let unpriced = lhs.unpriced.checked_add(rhs.unpriced)?;
        let unmetered = lhs.unmetered.checked_add(rhs.unmetered)?;
        let estimated = lhs.estimated.checked_add(rhs.estimated)?;
        let first = priced.checked_add(unpriced)?;
        let second = first.checked_add(unmetered)?;
        let _total = second.checked_add(estimated)?;
        Some(CostUsageCoverageCounts::new(
            priced, unpriced, unmetered, estimated,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CostUsageModelBreakdown {
    pub model_name: String,
    pub cost_usd: Option<f64>,
    pub total_tokens: Option<i64>,
    pub request_count: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub standard_cost_usd: Option<f64>,
    pub priority_cost_usd: Option<f64>,
    pub standard_tokens: Option<i64>,
    pub priority_tokens: Option<i64>,
    pub incomplete_request_count: Option<i64>,
}

impl CostUsageModelBreakdown {
    #[must_use]
    pub fn new(
        model_name: impl Into<String>,
        cost_usd: Option<f64>,
        total_tokens: Option<i64>,
    ) -> Self {
        Self {
            model_name: model_name.into(),
            cost_usd,
            total_tokens,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CostUsageDailyReportEntry {
    pub date: String,
    pub input_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub request_count: Option<i64>,
    pub cost_usd: Option<f64>,
    pub models_used: Option<Vec<String>>,
    pub model_breakdowns: Option<Vec<CostUsageModelBreakdown>>,
    pub unpriced_request_count: Option<i64>,
    pub priced_request_count: Option<i64>,
    pub unmetered_request_count: Option<i64>,
    pub estimated_request_count: Option<i64>,
}

impl CostUsageDailyReportEntry {
    #[must_use]
    pub fn incomplete_request_count(&self) -> i64 {
        let Some(ref breakdowns) = self.model_breakdowns else {
            return 0;
        };
        sum_incomplete_requests(breakdowns.iter().filter_map(|b| b.incomplete_request_count))
    }

    #[must_use]
    pub fn coverage_counts(&self) -> CostUsageCoverageCounts {
        self.coverage_counts_with_detail(CostUsageCoverageDetail::Exact)
    }

    #[must_use]
    pub fn coverage_counts_with_detail(
        &self,
        detail: CostUsageCoverageDetail,
    ) -> CostUsageCoverageCounts {
        let unpriced = if detail == CostUsageCoverageDetail::Exact {
            self.unpriced_request_count.unwrap_or(0).max(0)
        } else {
            0
        };
        let unmetered = if detail == CostUsageCoverageDetail::Exact {
            self.unmetered_request_count.unwrap_or(0).max(0)
        } else {
            0
        };
        let estimated = if detail == CostUsageCoverageDetail::Exact {
            self.estimated_request_count.unwrap_or(0).max(0)
        } else {
            0
        };
        if detail == CostUsageCoverageDetail::Exact
            && let Some(priced) = self.priced_request_count
        {
            return CostUsageCoverageCounts::new(priced.max(0), unpriced, unmetered, estimated);
        }
        if detail != CostUsageCoverageDetail::Rows
            && let Some(requests) = self.request_count
            && requests > 0
        {
            let remainder = [unpriced, unmetered, estimated]
                .into_iter()
                .fold(requests, |acc, v| (acc - v).max(0));
            let has_cost = self.cost_usd.is_some();
            return CostUsageCoverageCounts::new(
                if has_cost { remainder } else { 0 },
                if has_cost {
                    unpriced
                } else {
                    unpriced.saturating_add(remainder)
                },
                unmetered,
                estimated,
            );
        }
        if unpriced > 0 || unmetered > 0 || estimated > 0 {
            return CostUsageCoverageCounts::new(0, unpriced, unmetered, estimated);
        }
        if self.cost_usd.is_some() {
            return CostUsageCoverageCounts::new(1, 0, 0, 0);
        }
        if self.total_tokens.unwrap_or(0) > 0
            || [
                self.input_tokens,
                self.output_tokens,
                self.cache_read_tokens,
                self.cache_creation_tokens,
            ]
            .into_iter()
            .any(|v| v.unwrap_or(0) > 0)
        {
            return CostUsageCoverageCounts::new(0, 1, 0, 0);
        }
        CostUsageCoverageCounts::default()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CostUsageDailyReportSummary {
    pub total_input_tokens: Option<i64>,
    pub total_output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub total_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CostUsageDailyReport {
    pub data: Vec<CostUsageDailyReportEntry>,
    pub summary: Option<CostUsageDailyReportSummary>,
}

#[derive(Debug, Clone, Default)]
struct OptionalCountAccumulator {
    value: Option<i64>,
    overflowed: bool,
}

impl OptionalCountAccumulator {
    fn add(&mut self, incoming: Option<i64>) {
        let Some(incoming) = incoming else {
            return;
        };
        if incoming < 0 || self.overflowed {
            return;
        }
        let cur = self.value.unwrap_or(0);
        if let Some(sum) = cur.checked_add(incoming) {
            self.value = Some(sum);
        } else {
            self.value = None;
            self.overflowed = true;
        }
    }
}

#[derive(Debug, Clone)]
struct BreakdownAccumulator {
    token_mix: CostUsageTokenMix,
    request_count: OptionalCountAccumulator,
    total_tokens: OptionalCountAccumulator,
    total_tokens_are_complete: bool,
    cost_usd: f64,
    saw_cost: bool,
    cost_is_valid: bool,
    standard_cost_usd: f64,
    saw_standard_cost: bool,
    priority_cost_usd: f64,
    saw_priority_cost: bool,
    standard_tokens: OptionalCountAccumulator,
    priority_tokens: OptionalCountAccumulator,
    incomplete_request_count: i64,
}

impl Default for BreakdownAccumulator {
    fn default() -> Self {
        Self {
            token_mix: CostUsageTokenMix::default(),
            request_count: OptionalCountAccumulator::default(),
            total_tokens: OptionalCountAccumulator::default(),
            total_tokens_are_complete: true,
            cost_usd: 0.0,
            saw_cost: false,
            cost_is_valid: true,
            standard_cost_usd: 0.0,
            saw_standard_cost: false,
            priority_cost_usd: 0.0,
            saw_priority_cost: false,
            standard_tokens: OptionalCountAccumulator::default(),
            priority_tokens: OptionalCountAccumulator::default(),
            incomplete_request_count: 0,
        }
    }
}

impl BreakdownAccumulator {
    fn add(&mut self, breakdown: &CostUsageModelBreakdown) {
        self.token_mix.merge(&CostUsageTokenMix::new(
            breakdown.input_tokens,
            breakdown.output_tokens,
            breakdown.cache_read_tokens,
            breakdown.cache_creation_tokens,
            breakdown.reasoning_tokens,
        ));
        self.request_count.add(breakdown.request_count);
        self.incomplete_request_count = sum_incomplete_requests([
            self.incomplete_request_count,
            breakdown.incomplete_request_count.unwrap_or(0),
        ]);
        self.total_tokens.add(breakdown.total_tokens);
        if let Some(total_tokens) = breakdown.total_tokens {
            self.total_tokens_are_complete = self.total_tokens_are_complete && total_tokens >= 0;
        } else if breakdown.request_count.unwrap_or(0) > 0
            || breakdown.cost_usd.unwrap_or(0.0) > 0.0
        {
            self.total_tokens_are_complete = false;
        }
        if let Some(cost_usd) = breakdown.cost_usd {
            if !cost_usd.is_finite() || cost_usd < 0.0 {
                self.cost_is_valid = false;
            } else {
                let sum = self.cost_usd + cost_usd;
                if sum.is_finite() {
                    self.cost_usd = sum;
                    self.saw_cost = true;
                } else {
                    self.cost_is_valid = false;
                }
            }
        }
        if let Some(standard_cost_usd) = breakdown.standard_cost_usd {
            self.standard_cost_usd += standard_cost_usd;
            self.saw_standard_cost = true;
        }
        if let Some(priority_cost_usd) = breakdown.priority_cost_usd {
            self.priority_cost_usd += priority_cost_usd;
            self.saw_priority_cost = true;
        }
        self.standard_tokens.add(breakdown.standard_tokens);
        self.priority_tokens.add(breakdown.priority_tokens);
    }

    fn build(self, model_name: String, include_activity: bool) -> CostUsageModelBreakdown {
        CostUsageModelBreakdown {
            model_name,
            cost_usd: (self.saw_cost && self.cost_is_valid).then_some(self.cost_usd),
            total_tokens: if self.total_tokens_are_complete {
                self.total_tokens.value
            } else {
                None
            },
            request_count: include_activity
                .then_some(self.request_count.value)
                .flatten(),
            input_tokens: include_activity
                .then_some(self.token_mix.input_tokens)
                .flatten(),
            output_tokens: include_activity
                .then_some(self.token_mix.output_tokens)
                .flatten(),
            cache_read_tokens: include_activity
                .then_some(self.token_mix.cache_read_tokens)
                .flatten(),
            cache_creation_tokens: include_activity
                .then_some(self.token_mix.cache_creation_tokens)
                .flatten(),
            reasoning_tokens: include_activity
                .then_some(self.token_mix.reasoning_tokens)
                .flatten(),
            standard_cost_usd: self.saw_standard_cost.then_some(self.standard_cost_usd),
            priority_cost_usd: self.saw_priority_cost.then_some(self.priority_cost_usd),
            standard_tokens: self.standard_tokens.value,
            priority_tokens: self.priority_tokens.value,
            incomplete_request_count: (self.incomplete_request_count > 0)
                .then_some(self.incomplete_request_count),
        }
    }
}

#[derive(Debug, Clone)]
struct EntryAccumulator {
    token_mix: CostUsageTokenMix,
    request_count: OptionalCountAccumulator,
    coverage: CostUsageCoverageAccumulator,
    entry_count: usize,
    has_explicit_coverage: bool,
    total_tokens: OptionalCountAccumulator,
    saw_explicit_total_tokens: bool,
    total_tokens_are_complete: bool,
    cost_usd: f64,
    saw_cost: bool,
    cost_is_complete: bool,
    models_used: BTreeSet<String>,
    breakdowns: BTreeMap<String, BreakdownAccumulator>,
}

impl Default for EntryAccumulator {
    fn default() -> Self {
        Self {
            token_mix: CostUsageTokenMix::default(),
            request_count: OptionalCountAccumulator::default(),
            coverage: CostUsageCoverageAccumulator::new(),
            entry_count: 0,
            has_explicit_coverage: false,
            total_tokens: OptionalCountAccumulator::default(),
            saw_explicit_total_tokens: false,
            total_tokens_are_complete: true,
            cost_usd: 0.0,
            saw_cost: false,
            cost_is_complete: true,
            models_used: BTreeSet::new(),
            breakdowns: BTreeMap::new(),
        }
    }
}

impl EntryAccumulator {
    fn add(&mut self, entry: &CostUsageDailyReportEntry) {
        self.token_mix.merge(&CostUsageTokenMix::from_entry(entry));
        self.request_count.add(entry.request_count);
        self.coverage.add(entry);
        self.entry_count += 1;
        self.has_explicit_coverage = self.has_explicit_coverage
            || entry.priced_request_count.is_some()
            || entry.unpriced_request_count.is_some()
            || entry.unmetered_request_count.is_some()
            || entry.estimated_request_count.is_some();
        let components = [
            entry.input_tokens,
            entry.cache_read_tokens,
            entry.cache_creation_tokens,
            entry.output_tokens,
        ];
        let has_active_breakdown = entry.model_breakdowns.as_ref().is_some_and(|bds| {
            bds.iter().any(|b| {
                b.total_tokens.unwrap_or(0) > 0
                    || b.request_count.unwrap_or(0) > 0
                    || b.cost_usd.unwrap_or(0.0) > 0.0
            })
        });
        let has_activity = entry.total_tokens.unwrap_or(0) > 0
            || components.into_iter().any(|c| c.unwrap_or(0) > 0)
            || entry.request_count.unwrap_or(0) > 0
            || entry.cost_usd.unwrap_or(0.0) > 0.0
            || has_active_breakdown
            || entry.models_used.as_ref().is_some_and(|m| !m.is_empty());
        if components.into_iter().any(|c| c.unwrap_or(0) < 0)
            || entry.total_tokens.unwrap_or(0) < 0
            || (entry.total_tokens.is_none()
                && !components.into_iter().any(|c| c.is_some())
                && has_activity)
        {
            self.total_tokens_are_complete = false;
        }
        if let Some(total_tokens) = entry.total_tokens {
            self.total_tokens.add(Some(total_tokens));
            self.saw_explicit_total_tokens = true;
        } else {
            for value in components {
                self.total_tokens.add(value);
            }
        }
        if let Some(cost_usd) = entry.cost_usd {
            if !cost_usd.is_finite() || cost_usd < 0.0 {
                self.cost_is_complete = false;
            } else {
                let sum = self.cost_usd + cost_usd;
                if sum.is_finite() {
                    self.cost_usd = sum;
                    self.saw_cost = true;
                } else {
                    self.cost_is_complete = false;
                }
            }
        } else if has_activity {
            self.cost_is_complete = false;
        }
        if let Some(ref models_used) = entry.models_used {
            for model in models_used {
                self.models_used.insert(model.clone());
            }
        }
        if let Some(ref breakdowns) = entry.model_breakdowns {
            for breakdown in breakdowns {
                self.breakdowns
                    .entry(breakdown.model_name.clone())
                    .or_default()
                    .add(breakdown);
                self.models_used.insert(breakdown.model_name.clone());
            }
        }
    }

    fn resolved_total_tokens(&self) -> Option<i64> {
        if self.saw_explicit_total_tokens || self.total_tokens.value != Some(0) {
            self.total_tokens.value
        } else {
            None
        }
    }

    fn resolved_cost_usd(&self) -> Option<f64> {
        (self.saw_cost && self.cost_is_complete).then_some(self.cost_usd)
    }

    fn build(self, date: String) -> CostUsageDailyReportEntry {
        let resolved_tokens = self.resolved_total_tokens();
        let resolved_cost = self.resolved_cost_usd();
        let model_breakdowns = if self.breakdowns.is_empty() {
            None
        } else {
            let mut list: Vec<CostUsageModelBreakdown> = self
                .breakdowns
                .into_iter()
                .map(|(name, acc)| acc.build(name, true))
                .collect();
            CostUsageDailyReport::sort_model_breakdowns(&mut list);
            Some(list)
        };
        let models_used = if self.models_used.is_empty() {
            None
        } else {
            Some(self.models_used.into_iter().collect())
        };
        let include_coverage = self.entry_count > 1 || self.has_explicit_coverage;
        CostUsageDailyReportEntry {
            date,
            input_tokens: self.token_mix.input_tokens,
            output_tokens: self.token_mix.output_tokens,
            cache_read_tokens: self.token_mix.cache_read_tokens,
            cache_creation_tokens: self.token_mix.cache_creation_tokens,
            reasoning_tokens: self.token_mix.reasoning_tokens,
            total_tokens: if self.total_tokens_are_complete {
                resolved_tokens
            } else {
                None
            },
            request_count: self.request_count.value,
            cost_usd: resolved_cost,
            models_used,
            model_breakdowns,
            unpriced_request_count: include_coverage
                .then(|| self.coverage.exact.map(|c| c.unpriced))
                .flatten(),
            unmetered_request_count: include_coverage
                .then(|| self.coverage.exact.map(|c| c.unmetered))
                .flatten(),
            estimated_request_count: include_coverage
                .then(|| self.coverage.exact.map(|c| c.estimated))
                .flatten(),
            priced_request_count: include_coverage
                .then(|| self.coverage.exact.map(|c| c.priced))
                .flatten(),
        }
    }
}

impl CostUsageDailyReport {
    #[must_use]
    pub fn merged(reports: &[Self], timezone: &TimeZone) -> Self {
        let mut day_accumulators: BTreeMap<String, EntryAccumulator> = BTreeMap::new();
        for report in reports {
            for entry in &report.data {
                let raw_date = entry.date.trim();
                let day_key = CostUsageTokenSnapshot::local_day_key(raw_date, timezone)
                    .unwrap_or_else(|| raw_date.to_owned());
                day_accumulators.entry(day_key).or_default().add(entry);
            }
        }
        let data: Vec<CostUsageDailyReportEntry> = day_accumulators
            .into_iter()
            .map(|(date, acc)| acc.build(date))
            .collect();
        if data.is_empty() {
            return Self {
                data: Vec::new(),
                summary: None,
            };
        }
        let mut totals = EntryAccumulator::default();
        for report in reports {
            for entry in &report.data {
                totals.add(entry);
            }
        }
        let summary = Some(CostUsageDailyReportSummary {
            total_input_tokens: totals.token_mix.input_tokens,
            total_output_tokens: totals.token_mix.output_tokens,
            cache_read_tokens: totals.token_mix.cache_read_tokens,
            cache_creation_tokens: totals.token_mix.cache_creation_tokens,
            reasoning_tokens: totals.token_mix.reasoning_tokens,
            total_tokens: totals.resolved_total_tokens(),
            total_cost_usd: totals.resolved_cost_usd(),
        });
        Self { data, summary }
    }

    pub fn sort_model_breakdowns(breakdowns: &mut [CostUsageModelBreakdown]) {
        breakdowns.sort_by(|lhs, rhs| {
            let l_cost = lhs.cost_usd.unwrap_or(-1.0);
            let r_cost = rhs.cost_usd.unwrap_or(-1.0);
            if (l_cost - r_cost).abs() > 1e-12 {
                return r_cost.partial_cmp(&l_cost).unwrap_or(Ordering::Equal);
            }
            let l_tok = lhs.total_tokens.unwrap_or(-1);
            let r_tok = rhs.total_tokens.unwrap_or(-1);
            if l_tok != r_tok {
                return r_tok.cmp(&l_tok);
            }
            rhs.model_name.cmp(&lhs.model_name)
        });
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CostUsageProjectSourceBreakdown {
    pub name: String,
    pub path: Option<String>,
    pub total_tokens: Option<i64>,
    pub total_cost_usd: Option<f64>,
    pub daily: Vec<CostUsageDailyReportEntry>,
    pub model_breakdowns: Option<Vec<CostUsageModelBreakdown>>,
}

impl CostUsageProjectSourceBreakdown {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        path: Option<String>,
        total_tokens: Option<i64>,
        total_cost_usd: Option<f64>,
        daily: Vec<CostUsageDailyReportEntry>,
        model_breakdowns: Option<Vec<CostUsageModelBreakdown>>,
    ) -> Self {
        let raw_name = name.into();
        let trimmed_name = raw_name.trim();
        let clean_name = if trimmed_name.is_empty() {
            CostUsageProjectBreakdown::UNKNOWN_PROJECT_NAME.to_owned()
        } else {
            raw_name
        };
        let clean_path = path.map(|p| p.trim().to_owned()).filter(|p| !p.is_empty());
        Self {
            name: clean_name,
            path: clean_path,
            total_tokens,
            total_cost_usd,
            daily,
            model_breakdowns,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CostUsageProjectBreakdown {
    pub name: String,
    pub is_projectless: bool,
    pub path: Option<String>,
    pub total_tokens: Option<i64>,
    pub total_cost_usd: Option<f64>,
    pub daily: Vec<CostUsageDailyReportEntry>,
    pub model_breakdowns: Option<Vec<CostUsageModelBreakdown>>,
    pub sources: Vec<CostUsageProjectSourceBreakdown>,
}

impl CostUsageProjectBreakdown {
    pub const UNKNOWN_PROJECT_NAME: &'static str = "Unknown project";

    #[must_use]
    pub fn new(
        name: impl Into<String>,
        path: Option<String>,
        total_tokens: Option<i64>,
        total_cost_usd: Option<f64>,
        daily: Vec<CostUsageDailyReportEntry>,
        model_breakdowns: Option<Vec<CostUsageModelBreakdown>>,
        sources: Vec<CostUsageProjectSourceBreakdown>,
    ) -> Self {
        let raw_name = name.into();
        let trimmed_name = raw_name.trim();
        let clean_name = if trimmed_name.is_empty() {
            Self::UNKNOWN_PROJECT_NAME.to_owned()
        } else {
            raw_name
        };
        let clean_path = path.map(|p| p.trim().to_owned()).filter(|p| !p.is_empty());
        Self {
            name: clean_name,
            is_projectless: false,
            path: clean_path,
            total_tokens,
            total_cost_usd,
            daily,
            model_breakdowns,
            sources,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostUsageSessionBreakdown {
    pub session_id: String,
    pub last_activity: Timestamp,
    pub input_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub request_count: Option<i64>,
    pub cost_usd: Option<f64>,
    pub model_breakdowns: Vec<CostUsageModelBreakdown>,
    pub project_path: Option<String>,
    pub project_name: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostUsageWindowSummary {
    pub days: usize,
    pub total_tokens: Option<i64>,
    pub total_cost_usd: Option<f64>,
    pub total_requests: Option<i64>,
    pub entry_count: usize,
    pub incomplete_request_count: i64,
    pub token_mix: CostUsageTokenMix,
    pub coverage: CostUsageCoverageCounts,
    pub provenance: CostProvenance,
    pub metered_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostUsageTokenSnapshot {
    pub session_tokens: Option<i64>,
    pub session_cost_usd: Option<f64>,
    pub session_requests: Option<i64>,
    pub last_30_days_tokens: Option<i64>,
    pub last_30_days_cost_usd: Option<f64>,
    pub last_30_days_requests: Option<i64>,
    pub currency_code: String,
    pub history_days: usize,
    pub history_coverage_is_established: bool,
    pub history_scan_is_partial: bool,
    pub history_label: Option<String>,
    pub reporting_period: Option<CostReportingPeriod>,
    pub metered_cost_usd: Option<f64>,
    pub cost_provenance: CostProvenance,
    pub daily: Vec<CostUsageDailyReportEntry>,
    pub details_are_complete: bool,
    pub projects: Vec<CostUsageProjectBreakdown>,
    pub sessions: Vec<CostUsageSessionBreakdown>,
    pub updated_at: Timestamp,
}

impl Default for CostUsageTokenSnapshot {
    fn default() -> Self {
        Self {
            session_tokens: None,
            session_cost_usd: None,
            session_requests: None,
            last_30_days_tokens: None,
            last_30_days_cost_usd: None,
            last_30_days_requests: None,
            currency_code: "USD".to_owned(),
            history_days: 30,
            history_coverage_is_established: true,
            history_scan_is_partial: false,
            history_label: None,
            reporting_period: None,
            metered_cost_usd: None,
            cost_provenance: CostProvenance::Unknown,
            daily: Vec::new(),
            details_are_complete: false,
            projects: Vec::new(),
            sessions: Vec::new(),
            updated_at: Timestamp::UNIX_EPOCH,
        }
    }
}

impl CostUsageTokenSnapshot {
    #[must_use]
    pub const fn history_is_fully_scanned(&self) -> bool {
        self.history_coverage_is_established && !self.history_scan_is_partial
    }

    #[must_use]
    pub fn period_label(&self) -> String {
        self.history_label
            .clone()
            .or_else(|| self.reporting_period.map(CostReportingPeriod::label))
            .unwrap_or_else(|| {
                CostReportingPeriod::Rolling {
                    days: self.history_days,
                }
                .label()
            })
    }

    #[must_use]
    pub fn reporting(mut self, period: CostReportingPeriod) -> Self {
        if matches!(period, CostReportingPeriod::Rolling { .. }) {
            return self;
        }
        self.reporting_period = Some(period);
        self.history_label = Some(period.label());
        self
    }

    #[must_use]
    pub fn summary(&self, requested_days: usize, timezone: &TimeZone) -> CostUsageWindowSummary {
        let days = requested_days.max(1);
        let entries =
            CostReportingPeriod::Rolling { days }.entries(&self.daily, self.updated_at, timezone);
        let costs: Vec<f64> = entries.iter().filter_map(|e| e.cost_usd).collect();
        let tokens: Vec<i64> = entries.iter().filter_map(|e| e.total_tokens).collect();
        let requests: Vec<i64> = entries.iter().filter_map(|e| e.request_count).collect();
        let mut mix = CostUsageTokenMix::default();
        let mut coverage = CostUsageCoverageAccumulator::new();
        for entry in &entries {
            mix.merge(&CostUsageTokenMix::from_entry(entry));
            coverage.add(entry);
        }
        let covers_full_history = days >= self.history_days;
        let window_metered = covers_full_history
            .then_some(self.metered_cost_usd)
            .flatten();
        let total_tokens = if tokens.is_empty() {
            None
        } else {
            checked_sum_i64(tokens.into_iter())
        };
        let total_requests = if requests.is_empty() {
            None
        } else {
            checked_sum_i64(requests.into_iter())
        };
        let total_cost_usd = if costs.is_empty() {
            None
        } else {
            Some(costs.into_iter().sum())
        };
        let incomplete_request_count = sum_incomplete_requests(
            entries
                .iter()
                .map(CostUsageDailyReportEntry::incomplete_request_count),
        );
        CostUsageWindowSummary {
            days,
            total_tokens,
            total_cost_usd,
            total_requests,
            entry_count: entries.len(),
            incomplete_request_count,
            token_mix: mix,
            coverage: coverage.counts(),
            provenance: CostProvenance::for_window(
                self.cost_provenance,
                total_cost_usd.is_some(),
                window_metered.is_some(),
            ),
            metered_cost_usd: window_metered,
        }
    }

    #[must_use]
    pub fn local_day_key(raw_date: &str, timezone: &TimeZone) -> Option<String> {
        let trimmed = raw_date.trim();
        if trimmed.len() >= 10 {
            let bytes = trimmed.as_bytes();
            if bytes[4] == b'-'
                && bytes[7] == b'-'
                && bytes[0..4].iter().all(u8::is_ascii_digit)
                && bytes[5..7].iter().all(u8::is_ascii_digit)
                && bytes[8..10].iter().all(u8::is_ascii_digit)
            {
                return Some(trimmed[..10].to_owned());
            }
        }
        let parsed: Timestamp = trimmed.parse().ok()?;
        Some(
            timestamp::day_key_from_utc(parsed, Some(timezone.clone()))
                .as_str()
                .to_owned(),
        )
    }
}

pub fn sum_incomplete_requests<I: IntoIterator<Item = i64>>(counts: I) -> i64 {
    checked_sum_i64(counts.into_iter().map(|c| c.max(0))).unwrap_or(i64::MAX)
}

fn checked_sum_i64<I: Iterator<Item = i64>>(iter: I) -> Option<i64> {
    let mut sum = 0i64;
    for val in iter {
        sum = sum.checked_add(val)?;
    }
    Some(sum)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorCookieSource {
    Auto,
    Manual,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorProviderSettings {
    pub cookie_source: CursorCookieSource,
    pub manual_cookie_header: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CursorCostAvailabilityError {
    #[error("Cursor cost is unavailable because the Cursor cookie source is set to Off.")]
    CookieSourceOff,
    #[error("Cursor cost requires a non-empty Manual cookie header.")]
    ManualCookieMissing,
    #[error("{0}")]
    Resolution(String),
}

#[must_use]
pub fn normalize_cookie_header(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let stripped = raw
        .strip_prefix("Cookie:")
        .or_else(|| raw.strip_prefix("cookie:"))
        .map_or(raw, str::trim);
    (!stripped.is_empty()).then(|| stripped.to_owned())
}

#[must_use]
pub fn cursor_cost_availability_error(
    provider: CostProvider,
    settings: Option<&CursorProviderSettings>,
    resolution_error: Option<&str>,
) -> Option<CursorCostAvailabilityError> {
    if provider != CostProvider::Cursor {
        return None;
    }
    if let Some(err) = resolution_error {
        return Some(CursorCostAvailabilityError::Resolution(err.to_owned()));
    }
    let settings = settings?;
    match settings.cookie_source {
        CursorCookieSource::Off => Some(CursorCostAvailabilityError::CookieSourceOff),
        CursorCookieSource::Manual
            if normalize_cookie_header(settings.manual_cookie_header.as_deref()).is_none() =>
        {
            Some(CursorCostAvailabilityError::ManualCookieMissing)
        }
        _ => None,
    }
}

#[must_use]
pub fn cursor_cost_header_override(
    provider: CostProvider,
    settings: Option<&CursorProviderSettings>,
) -> Option<String> {
    if provider != CostProvider::Cursor {
        return None;
    }
    let settings = settings?;
    if settings.cookie_source != CursorCookieSource::Manual {
        return None;
    }
    normalize_cookie_header(settings.manual_cookie_header.as_deref())
}

#[must_use]
pub const fn decode_cost_include_pi_sessions(provider_native_only: bool) -> bool {
    !provider_native_only
}

#[must_use]
pub fn decode_cost_group_by(raw: Option<&str>) -> CostGroupBy {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return CostGroupBy::None;
    };
    CostGroupBy::from_raw(raw).unwrap_or(CostGroupBy::None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostCliError {
    pub code: i32,
    pub kind: &'static str,
    pub message: String,
}

pub fn decode_cost_reporting_period(
    days: Option<usize>,
    period: Option<&str>,
    remote: Option<&str>,
    summary_only: bool,
    saved: CostReportingPeriod,
) -> Result<CostReportingPeriod, CostCliError> {
    if days.is_none()
        && let Some(raw) = period
        && CostReportingPeriod::from_raw(raw).is_none()
    {
        return Err(CostCliError {
            code: EXIT_ARGS,
            kind: "args",
            message: "Error: --period must be month-to-date or all.".to_owned(),
        });
    }
    if let Some(days) = days {
        return Ok(CostReportingPeriod::Rolling {
            days: days.clamp(1, 365),
        });
    }
    let resolved = period
        .and_then(CostReportingPeriod::from_raw)
        .unwrap_or(saved);
    if resolved == CostReportingPeriod::AllTime && (remote.is_some() || summary_only) {
        return Err(CostCliError {
            code: EXIT_ARGS,
            kind: "args",
            message: "Error: host summaries support 1...365 days; override All with --days N."
                .to_owned(),
        });
    }
    Ok(resolved)
}

#[must_use]
pub fn cost_providers(
    providers: &[CostProvider],
    group_by: CostGroupBy,
    format: OutputFormat,
) -> Vec<CostProvider> {
    providers
        .iter()
        .copied()
        .filter(|&p| {
            !group_by.requires_codex_local_sessions()
                || p == CostProvider::Codex
                || format == OutputFormat::Json
        })
        .collect()
}

#[must_use]
pub fn cost_include_pi_sessions(
    provider: CostProvider,
    selected_providers: &[CostProvider],
    group_by: CostGroupBy,
    format: OutputFormat,
    include_pi_sessions: bool,
) -> bool {
    if (provider == CostProvider::Claude || provider == CostProvider::Codex)
        && selected_providers.contains(&CostProvider::Pi)
    {
        return false;
    }
    if provider == CostProvider::Codex
        && group_by == CostGroupBy::Session
        && format == OutputFormat::Text
    {
        return false;
    }
    include_pi_sessions
}

#[must_use]
pub fn session_grouping_pi_omission_warning(
    provider: CostProvider,
    group_by: CostGroupBy,
    format: OutputFormat,
    include_pi_sessions: bool,
) -> Option<&'static str> {
    if provider == CostProvider::Codex
        && group_by == CostGroupBy::Session
        && format == OutputFormat::Text
        && include_pi_sessions
    {
        Some(
            "Session grouping shows native Codex conversations only; Pi/OMP usage is omitted from this view. Use the default cost view for merged totals.",
        )
    } else {
        None
    }
}

#[must_use]
pub fn short_session_id(session_id: &str) -> String {
    let trimmed = session_id.trim();
    let count = trimmed.chars().count();
    if count <= 12 {
        return trimmed.to_owned();
    }
    let prefix: String = trimmed.chars().take(4).collect();
    let suffix: String = trimmed.chars().skip(count - 8).collect();
    format!("{prefix}...{suffix}")
}

#[must_use]
pub fn session_model_label(models: &[String]) -> String {
    let labels: Vec<String> = models.iter().map(|m| model_display_name(m)).collect();
    match labels.len() {
        0 => "Unknown model".to_owned(),
        1 => labels[0].clone(),
        2 => format!("{} +1 model", labels[0]),
        n => format!("{} +{} models", labels[0], n - 1),
    }
}

#[must_use]
pub fn session_timestamp_string(timestamp: Timestamp, timezone: &TimeZone) -> String {
    let zdt = timestamp.to_zoned(timezone.clone());
    let month = match zdt.month() {
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
        _ => "Jan",
    };
    format!(
        "{month} {}, {:02}:{:02}",
        zdt.day(),
        zdt.hour(),
        zdt.minute()
    )
}

#[must_use]
pub fn cost_estimate_hint(provider: CostProvider) -> &'static str {
    match provider {
        CostProvider::Codex => {
            "Not a subscription bill or plan value · local usage × public API prices"
        }
        CostProvider::Claude => {
            "Estimated from local Claude logs at API rates; token totals include cache read/write tokens and may differ from Claude Code /status."
        }
        CostProvider::Antigravity => {
            "Local usage × public API prices · not Antigravity charges or credits"
        }
        CostProvider::Cursor => {
            "From Cursor's usage dashboard at vendor token rates; may differ from your invoice."
        }
        _ => COST_ESTIMATE_HINT,
    }
}

#[must_use]
pub fn token_count_string(value: i64) -> String {
    let abs_value = value.unsigned_abs();
    let sign = if value < 0 { "-" } else { "" };

    let units: [(u64, f64, &str); 3] = [
        (999_500_000, 1_000_000_000.0, "B"),
        (999_500, 1_000_000.0, "M"),
        (1_000, 1_000.0, "K"),
    ];

    for (threshold, divisor, suffix) in units {
        if abs_value >= threshold {
            let scaled = (abs_value as f64) / divisor;
            let formatted = if scaled >= 10.0 {
                format!("{scaled:.0}")
            } else {
                let s = format!("{scaled:.1}");
                s.strip_suffix(".0").unwrap_or(&s).to_owned()
            };
            return format!("{sign}{formatted}{suffix}");
        }
    }

    value.to_string()
}

#[must_use]
pub fn currency_string(value: f64, currency_code: &str) -> String {
    let code = currency_code.trim().to_ascii_uppercase();
    let sign = if value < 0.0 { "-" } else { "" };
    let abs = value.abs();
    let rounded_cents = (abs * 100.0).round() as u64;
    let whole = rounded_cents / 100;
    let cents = rounded_cents % 100;

    let whole_str = format_with_commas(whole);
    let symbol = match code.as_str() {
        "USD" | "" => "$",
        "EUR" => "€",
        "GBP" => "£",
        _ => "",
    };
    if symbol.is_empty() {
        format!("{sign}{code} {whole_str}.{cents:02}")
    } else {
        format!("{sign}{symbol}{whole_str}.{cents:02}")
    }
}

fn format_with_commas(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (idx, &b) in bytes.iter().enumerate() {
        if idx > 0 && (bytes.len() - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

#[must_use]
pub fn model_display_name(raw: &str) -> String {
    let mut cleaned = raw.trim().to_owned();
    if cleaned.is_empty() {
        return raw.to_owned();
    }
    if cleaned == "codex-auto-review" {
        return "Codex Auto Review".to_owned();
    }
    if pricing::is_codex_unattributed_model(&cleaned) {
        return "Unknown model".to_owned();
    }

    if let Some(stripped) = strip_trailing_date_suffix(&cleaned) {
        cleaned = stripped;
    }

    let trimmed = cleaned.trim_end_matches([' ', '\t', '-']);
    if trimmed.is_empty() {
        raw.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn strip_trailing_date_suffix(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    // Pattern 1: (?:-|\s)\d{8}$
    if bytes.len() >= 9 {
        let split = bytes.len() - 9;
        if matches!(bytes[split], b'-' | b' ') && bytes[split + 1..].iter().all(u8::is_ascii_digit)
        {
            return Some(s[..split].to_owned());
        }
    }
    // Pattern 2: (?:-|\s)\d{4}-\d{2}-\d{2}$
    if bytes.len() >= 11 {
        let split = bytes.len() - 11;
        let tail = &bytes[split + 1..];
        if matches!(bytes[split], b'-' | b' ')
            && tail[0..4].iter().all(u8::is_ascii_digit)
            && tail[4] == b'-'
            && tail[5..7].iter().all(u8::is_ascii_digit)
            && tail[7] == b'-'
            && tail[8..10].iter().all(u8::is_ascii_digit)
        {
            return Some(s[..split].to_owned());
        }
    }
    // Pattern 3: \s\d{4}\s\d{4}$
    if bytes.len() >= 10 {
        let split = bytes.len() - 10;
        let tail = &bytes[split..];
        if tail[0] == b' '
            && tail[1..5].iter().all(u8::is_ascii_digit)
            && tail[5] == b' '
            && tail[6..10].iter().all(u8::is_ascii_digit)
        {
            return Some(s[..split].to_owned());
        }
    }
    None
}

fn cost_header_line(header: &str, use_color: bool) -> String {
    if use_color {
        format!("\u{001b}[1;36m{header}\u{001b}[0m")
    } else {
        header.to_owned()
    }
}

#[must_use]
pub fn render_cost_text(
    provider: CostProvider,
    snapshot: &CostUsageTokenSnapshot,
    group_by: CostGroupBy,
    use_color: bool,
    timezone: &TimeZone,
    include_breakdown: bool,
) -> String {
    let name = provider.display_name();
    let cost_is_entirely_unknown = provider == CostProvider::Antigravity
        && (snapshot.last_30_days_cost_usd.is_none()
            || (snapshot.daily.is_empty() && snapshot.last_30_days_cost_usd == Some(0.0)));
    if provider.tokens_only_presentation() || cost_is_entirely_unknown {
        return render_local_token_history_text(name, snapshot, use_color);
    }

    let title = if provider == CostProvider::Codex {
        format!("{name} API-equivalent estimate (not billed)")
    } else {
        format!("{name} Cost (API-rate estimate)")
    };
    let header = cost_header_line(&title, use_color);
    let history_notice = (!snapshot.history_is_fully_scanned())
        .then_some("Partial local history · recorded token subtotal");

    if group_by == CostGroupBy::Project && provider == CostProvider::Codex {
        let project_header = if let Some(notice) = history_notice {
            format!("{header}\n{notice}")
        } else {
            header
        };
        return render_project_cost_text(&project_header, snapshot);
    }
    if group_by == CostGroupBy::Session && provider == CostProvider::Codex {
        return render_session_cost_text(&header, snapshot, timezone);
    }

    let today_incomplete = snapshot.summary(1, timezone).incomplete_request_count;
    let incomplete = sum_incomplete_requests(
        snapshot
            .daily
            .iter()
            .map(CostUsageDailyReportEntry::incomplete_request_count),
    );
    let unpriced: i64 = snapshot
        .daily
        .iter()
        .map(|d| d.unpriced_request_count.unwrap_or(0).max(0))
        .sum();
    let today_cost = snapshot
        .session_cost_usd
        .map(|c| currency_string(c, &snapshot.currency_code))
        .unwrap_or_else(|| "—".to_owned());
    let today_tokens = snapshot.session_tokens.map(token_count_string);
    let today_line = match today_tokens {
        Some(tok) => format!("Today: {today_cost} · {tok} tokens"),
        None => format!("Today: {today_cost}"),
    };

    let month_cost = snapshot
        .last_30_days_cost_usd
        .map(|c| currency_string(c, &snapshot.currency_code))
        .unwrap_or_else(|| "—".to_owned());
    let month_tokens = snapshot.last_30_days_tokens.map(token_count_string);
    let history_label = snapshot.period_label();
    let month_line = match month_tokens {
        Some(tok) => format!("{history_label}: {month_cost} · {tok} tokens"),
        None => format!("{history_label}: {month_cost}"),
    };

    let metered_line = snapshot.metered_cost_usd.map(|m| {
        let amount = currency_string(m, &snapshot.currency_code);
        format!(
            "Cursor-metered: {amount} ({})",
            history_label.to_ascii_lowercase()
        )
    });

    let mut lines: Vec<String> = vec![
        header,
        format!(
            "{today_line}{}",
            if today_incomplete > 0 {
                " · Incomplete"
            } else {
                ""
            }
        ),
        format!(
            "{month_line}{}",
            if incomplete > 0 { " · Incomplete" } else { "" }
        ),
    ];
    if let Some(metered) = metered_line {
        lines.push(metered);
    }
    if incomplete > 0 {
        lines.push(format!(
            "Incomplete: {incomplete} requests lacked final usage and were excluded from tokens and cost."
        ));
    }
    if unpriced > 0 {
        let suffix = if unpriced == 1 { "" } else { "s" };
        lines.push(format!(
            "Partial estimate: {unpriced} recorded request{suffix} had no price."
        ));
    }
    if let Some(notice) = history_notice {
        lines.push(notice.to_owned());
    }
    if include_breakdown
        && (provider == CostProvider::Claude || provider == CostProvider::OpenCodeGo)
        && !snapshot.daily.is_empty()
    {
        lines.extend(token_detail_lines(snapshot, timezone, use_color));
    }
    lines.push(cost_estimate_hint(provider).to_owned());
    lines.join("\n")
}

fn token_detail_lines(
    snapshot: &CostUsageTokenSnapshot,
    timezone: &TimeZone,
    use_color: bool,
) -> Vec<String> {
    let (entries, period_label) = recent_cost_entries(snapshot, timezone);
    if entries.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    out.extend(daily_cost_lines(
        &entries,
        snapshot,
        &period_label,
        use_color,
    ));
    out.extend(top_cost_model_lines(
        &entries,
        snapshot,
        &period_label,
        use_color,
    ));
    out
}

fn recent_cost_entries(
    snapshot: &CostUsageTokenSnapshot,
    timezone: &TimeZone,
) -> (Vec<CostUsageDailyReportEntry>, String) {
    if snapshot.daily.is_empty() {
        return (Vec::new(), String::new());
    }
    let today = timestamp::day_key_from_utc(snapshot.updated_at, Some(timezone.clone()));
    let interval_days = snapshot.history_days.clamp(1, 7);
    let recent_day_keys: BTreeSet<String> = (0..interval_days)
        .map(|offset| today.add_days(-(offset as i32)).as_str().to_owned())
        .collect();
    let mut recent: Vec<CostUsageDailyReportEntry> = snapshot
        .daily
        .iter()
        .filter(|e| recent_day_keys.contains(e.date.as_str()))
        .cloned()
        .collect();
    recent.sort_by(|a, b| b.date.cmp(&a.date));
    if !recent.is_empty() {
        recent.truncate(interval_days);
        let plural = if interval_days == 1 { "" } else { "s" };
        let label = format!("last {interval_days} calendar day{plural}");
        return (recent, label);
    }
    let mut recorded = snapshot.daily.clone();
    recorded.sort_by(|a, b| b.date.cmp(&a.date));
    recorded.truncate(interval_days);
    let count = recorded.len();
    let plural = if count == 1 { "" } else { "s" };
    let label = format!("last {count} recorded day{plural}");
    (recorded, label)
}

fn daily_cost_lines(
    entries: &[CostUsageDailyReportEntry],
    snapshot: &CostUsageTokenSnapshot,
    period_label: &str,
    use_color: bool,
) -> Vec<String> {
    let title = format!("Daily breakdown ({period_label}):");
    let styled_title = if use_color {
        format!("\u{001b}[1m{title}\u{001b}[0m")
    } else {
        title
    };
    let mut out = vec![String::new(), styled_title];
    for entry in entries.iter().rev() {
        let has_unpriced_usage = entry.unpriced_request_count.unwrap_or(0) > 0
            || entry.model_breakdowns.as_ref().is_some_and(|bds| {
                bds.iter()
                    .any(|b| b.cost_usd.is_none() && b.total_tokens.unwrap_or(0) > 0)
            });
        let cost = if has_unpriced_usage {
            None
        } else {
            entry.cost_usd
        }
        .map(|c| currency_string(c, &snapshot.currency_code))
        .unwrap_or_else(|| "—".to_owned());
        let total = entry
            .total_tokens
            .map(token_count_string)
            .unwrap_or_else(|| "—".to_owned());
        let mut parts = Vec::new();
        if let Some(v) = entry.input_tokens {
            parts.push(format!("in {}", token_count_string(v)));
        }
        if let Some(v) = entry.output_tokens {
            parts.push(format!("out {}", token_count_string(v)));
        }
        if let Some(v) = entry.cache_read_tokens
            && v > 0
        {
            parts.push(format!("cacheRead {}", token_count_string(v)));
        }
        if let Some(v) = entry.cache_creation_tokens
            && v > 0
        {
            parts.push(format!("cacheCreate {}", token_count_string(v)));
        }
        let mix = if parts.is_empty() {
            String::new()
        } else {
            format!(" ({})", parts.join(", "))
        };
        let models = match entry.models_used.as_ref() {
            Some(models) if !models.is_empty() => {
                let head: Vec<&str> = models.iter().take(2).map(String::as_str).collect();
                let extra = if models.len() > 2 {
                    format!(" +{}", models.len() - 2)
                } else {
                    String::new()
                };
                format!(" · {}{extra}", head.join(", "))
            }
            _ => String::new(),
        };
        let incomplete = if entry.incomplete_request_count() > 0 {
            " · Incomplete"
        } else {
            ""
        };
        out.push(format!(
            "{}: {cost} · {total} tokens{mix}{models}{incomplete}",
            entry.date
        ));
    }
    out
}

fn top_cost_model_lines(
    entries: &[CostUsageDailyReportEntry],
    snapshot: &CostUsageTokenSnapshot,
    period_label: &str,
    use_color: bool,
) -> Vec<String> {
    #[derive(Clone)]
    struct ModelAggregate {
        cost: Option<f64>,
        tokens: Option<i64>,
        days: BTreeSet<String>,
        incomplete: i64,
        has_usage: bool,
    }

    impl Default for ModelAggregate {
        fn default() -> Self {
            Self {
                cost: Some(0.0),
                tokens: Some(0),
                days: BTreeSet::new(),
                incomplete: 0,
                has_usage: false,
            }
        }
    }

    let mut has_unattributed_day = false;
    let mut model_agg: BTreeMap<String, ModelAggregate> = BTreeMap::new();
    for entry in entries {
        if entry.unpriced_request_count.unwrap_or(0) > 0 || entry.incomplete_request_count() > 0 {
            has_unattributed_day = true;
        }
        let Some(ref breakdowns) = entry.model_breakdowns else {
            let has_usage =
                entry.cost_usd.unwrap_or(0.0) != 0.0 || entry.total_tokens.unwrap_or(0) > 0;
            if has_usage {
                has_unattributed_day = true;
            }
            continue;
        };
        if breakdowns.is_empty() {
            let has_usage =
                entry.cost_usd.unwrap_or(0.0) != 0.0 || entry.total_tokens.unwrap_or(0) > 0;
            if has_usage {
                has_unattributed_day = true;
            }
            continue;
        }
        for breakdown in breakdowns {
            let cur = model_agg.entry(breakdown.model_name.clone()).or_default();
            cur.incomplete = sum_incomplete_requests([
                cur.incomplete,
                breakdown.incomplete_request_count.unwrap_or(0),
            ]);
            cur.days.insert(entry.date.clone());
            let excluded_only = breakdown.incomplete_request_count.unwrap_or(0) > 0
                && breakdown.cost_usd.is_none()
                && breakdown.total_tokens.is_none();
            if excluded_only {
                continue;
            }
            cur.has_usage = true;
            if let Some(existing_cost) = cur.cost {
                if let Some(c) = breakdown.cost_usd {
                    cur.cost = Some(existing_cost + c);
                } else {
                    cur.cost = None;
                }
            }
            if let Some(existing_tokens) = cur.tokens {
                if let Some(t) = breakdown.total_tokens {
                    cur.tokens = existing_tokens.checked_add(t);
                } else {
                    cur.tokens = None;
                }
            }
        }
    }
    if model_agg.is_empty() {
        return Vec::new();
    }
    let is_partial = !snapshot.history_is_fully_scanned()
        || has_unattributed_day
        || model_agg
            .values()
            .any(|v| !v.has_usage || v.cost.is_none() || v.tokens.is_none());
    let mut sorted: Vec<(String, ModelAggregate)> = model_agg.into_iter().collect();
    sorted.sort_by(|lhs, rhs| {
        let l_cost = if lhs.1.has_usage {
            lhs.1.cost.unwrap_or(-1.0)
        } else {
            -1.0
        };
        let r_cost = if rhs.1.has_usage {
            rhs.1.cost.unwrap_or(-1.0)
        } else {
            -1.0
        };
        if (l_cost - r_cost).abs() > 1e-12 {
            return r_cost.partial_cmp(&l_cost).unwrap_or(Ordering::Equal);
        }
        let l_tok = if lhs.1.has_usage {
            lhs.1.tokens.unwrap_or(-1)
        } else {
            -1
        };
        let r_tok = if rhs.1.has_usage {
            rhs.1.tokens.unwrap_or(-1)
        } else {
            -1
        };
        if l_tok != r_tok {
            return r_tok.cmp(&l_tok);
        }
        lhs.0.cmp(&rhs.0)
    });
    let mut out = vec![String::new()];
    let title = if is_partial {
        format!("Top models ({period_label} — partial):")
    } else {
        format!("Top models ({period_label}):")
    };
    out.push(if use_color {
        format!("\u{001b}[1m{title}\u{001b}[0m")
    } else {
        title
    });
    for (idx, (model_key, agg)) in sorted.into_iter().take(5).enumerate() {
        let cost_str = agg
            .has_usage
            .then_some(agg.cost)
            .flatten()
            .map(|c| currency_string(c, &snapshot.currency_code))
            .unwrap_or_else(|| "—".to_owned());
        let tokens_str = agg
            .has_usage
            .then_some(agg.tokens)
            .flatten()
            .map(token_count_string)
            .unwrap_or_else(|| "—".to_owned());
        let days = agg.days.len();
        let display = model_display_name(&model_key);
        let day_label = if days == 1 { "day" } else { "days" };
        let incomplete = if agg.incomplete > 0 {
            " · Incomplete"
        } else {
            ""
        };
        let usage = format!("{cost_str} · {tokens_str} tokens ({days} {day_label}){incomplete}");
        out.push(format!("{}. {display} — {usage}", idx + 1));
    }
    if is_partial {
        out.push(
            "Note: Ranking is partial (incomplete history or unattributed cost/tokens).".to_owned(),
        );
    }
    out
}

fn render_local_token_history_text(
    name: &str,
    snapshot: &CostUsageTokenSnapshot,
    use_color: bool,
) -> String {
    let header = cost_header_line(&format!("{name} Token History"), use_color);
    let hint = "Local token history · dollar costs unavailable";
    if !snapshot.history_coverage_is_established && snapshot.last_30_days_tokens.is_none() {
        return [
            header,
            "Local token history is unavailable or incomplete.".to_owned(),
            hint.to_owned(),
        ]
        .join("\n");
    }
    let today = snapshot
        .session_tokens
        .map(|t| format!("{} tokens", token_count_string(t)))
        .unwrap_or_else(|| "—".to_owned());
    let total = snapshot
        .last_30_days_tokens
        .map(|t| format!("{} tokens", token_count_string(t)))
        .unwrap_or_else(|| "—".to_owned());
    let history_label = snapshot.period_label();
    let mut lines = vec![header, format!("Today: {today}")];
    if snapshot.history_days != 1 {
        lines.push(format!("{history_label}: {total}"));
    }
    if snapshot.daily.is_empty() && snapshot.history_coverage_is_established {
        lines.push("No token usage found in the selected period.".to_owned());
    }
    if !snapshot.history_is_fully_scanned() {
        lines.push("Partial local history · recorded token subtotal".to_owned());
    }
    lines.push(hint.to_owned());
    lines.join("\n")
}

fn render_project_cost_text(header: &str, snapshot: &CostUsageTokenSnapshot) -> String {
    let history_label = snapshot.period_label();
    let mut lines = vec![header.to_owned(), format!("Projects ({history_label}):")];
    if snapshot.projects.is_empty() {
        lines.push("—".to_owned());
        lines.push(cost_estimate_hint(CostProvider::Codex).to_owned());
        return lines.join("\n");
    }
    for project in &snapshot.projects {
        let cost = project
            .total_cost_usd
            .map(|c| currency_string(c, &snapshot.currency_code))
            .unwrap_or_else(|| "—".to_owned());
        let tokens = project.total_tokens.map(token_count_string);
        let summary = match tokens {
            Some(tok) => format!("{cost} · {tok} tokens"),
            None => cost,
        };
        lines.push(format!("{}: {summary}", project.name));
        if let Some(ref path) = project.path {
            lines.push(format!("  {path}"));
        }
        for source in &project.sources {
            let source_cost = source
                .total_cost_usd
                .map(|c| currency_string(c, &snapshot.currency_code))
                .unwrap_or_else(|| "—".to_owned());
            let source_tokens = source.total_tokens.map(token_count_string);
            let source_summary = match source_tokens {
                Some(tok) => format!("{source_cost} · {tok} tokens"),
                None => source_cost,
            };
            lines.push(format!("  - {}: {source_summary}", source.name));
            if let Some(ref path) = source.path {
                lines.push(format!("    {path}"));
            }
        }
    }
    lines.push(cost_estimate_hint(CostProvider::Codex).to_owned());
    lines.join("\n")
}

fn render_session_cost_text(
    header: &str,
    snapshot: &CostUsageTokenSnapshot,
    timezone: &TimeZone,
) -> String {
    let history_label = snapshot.period_label();
    let mut lines = vec![
        header.to_owned(),
        format!("Conversations ({history_label}):"),
    ];
    let history_incomplete = !snapshot.history_coverage_is_established;
    if history_incomplete {
        lines
            .push("Conversation history is incomplete while the local scan catches up.".to_owned());
    }
    if snapshot.sessions.is_empty() {
        if !history_incomplete {
            lines.push("—".to_owned());
        }
        lines.push(cost_estimate_hint(CostProvider::Codex).to_owned());
        return lines.join("\n");
    }
    for session in &snapshot.sessions {
        let cost = session
            .cost_usd
            .map(|c| currency_string(c, &snapshot.currency_code))
            .unwrap_or_else(|| "—".to_owned());
        let mut summary = vec![cost];
        if let Some(tokens) = session.total_tokens {
            summary.push(format!("{} tokens", token_count_string(tokens)));
        }
        if let Some(requests) = session.request_count {
            summary.push(format!("{requests} requests"));
        }
        lines.push(format!(
            "Session {}: {}",
            short_session_id(&session.session_id),
            summary.join(" · ")
        ));
        let model_names: Vec<String> = session
            .model_breakdowns
            .iter()
            .map(|b| b.model_name.clone())
            .collect();
        let model_label = session_model_label(&model_names);
        lines.push(format!(
            "{model_label} · {}",
            session_timestamp_string(session.last_activity, timezone)
        ));
    }
    lines.push(cost_estimate_hint(CostProvider::Codex).to_owned());
    lines.join("\n")
}

#[must_use]
pub fn make_cost_payload(
    provider: CostProvider,
    snapshot: Option<&CostUsageTokenSnapshot>,
    error: Option<&str>,
    timezone: &TimeZone,
) -> CostPayload {
    let daily = snapshot
        .map(|s| s.daily.iter().map(cost_daily_payload).collect())
        .unwrap_or_default();
    let summary = snapshot.map(|s| s.summary(s.history_days, timezone));
    let last_30_days = snapshot.map(|s| last_30_days_totals(s, timezone));
    let projects = if provider == CostProvider::Codex {
        snapshot
            .map(|s| {
                s.projects
                    .iter()
                    .map(|project| CostProjectPayload {
                        name: project.name.clone(),
                        path: project.path.clone(),
                        total_tokens: project.total_tokens,
                        total_cost_usd: project.total_cost_usd,
                        daily: project.daily.iter().map(cost_daily_payload).collect(),
                        model_breakdowns: project
                            .model_breakdowns
                            .as_ref()
                            .map(|bds| bds.iter().map(cost_model_breakdown_payload).collect()),
                        sources: project
                            .sources
                            .iter()
                            .map(|source| CostProjectSourcePayload {
                                name: source.name.clone(),
                                path: source.path.clone(),
                                total_tokens: source.total_tokens,
                                total_cost_usd: source.total_cost_usd,
                                daily: source.daily.iter().map(cost_daily_payload).collect(),
                                model_breakdowns: source.model_breakdowns.as_ref().map(|bds| {
                                    bds.iter().map(cost_model_breakdown_payload).collect()
                                }),
                            })
                            .collect(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let incomplete_sum = snapshot.map(|s| {
        sum_incomplete_requests(
            s.daily
                .iter()
                .map(CostUsageDailyReportEntry::incomplete_request_count),
        )
    });

    CostPayload {
        provider: provider.raw_value().to_owned(),
        source: if provider == CostProvider::Cursor {
            "web".to_owned()
        } else {
            "local".to_owned()
        },
        updated_at: snapshot.map(|s| s.updated_at).or_else(|| {
            if error.is_some() {
                Some(Timestamp::now())
            } else {
                None
            }
        }),
        currency_code: snapshot.map(|s| s.currency_code.clone()),
        session_tokens: snapshot.and_then(|s| s.session_tokens),
        session_cost_usd: snapshot.and_then(|s| s.session_cost_usd),
        history_days: snapshot.map(|s| s.history_days as i64),
        reporting_period: snapshot.map(|s| {
            s.reporting_period
                .unwrap_or(CostReportingPeriod::Rolling {
                    days: s.history_days,
                })
                .raw_value()
        }),
        history_label: snapshot.map(CostUsageTokenSnapshot::period_label),
        history_coverage_is_established: snapshot.map(|s| s.history_coverage_is_established),
        last_30_days_tokens: last_30_days.and_then(|t| t.0),
        last_30_days_cost_usd: last_30_days.and_then(|t| t.1),
        metered_cost_usd: snapshot.and_then(|s| s.metered_cost_usd),
        daily,
        projects,
        totals: snapshot.and_then(|s| cost_totals(s, timezone)),
        provenance: summary
            .as_ref()
            .map(|s| s.provenance.raw_value().to_owned()),
        coverage: summary.map(|s| s.coverage),
        error: error.map(|msg| CostErrorPayload {
            code: 1,
            kind: Some("provider".to_owned()),
            message: msg.to_owned(),
        }),
        incomplete_request_count: incomplete_sum.filter(|&c| c > 0),
    }
}

#[must_use]
pub fn make_open_codex_cost_payload(
    snapshot: &CostUsageTokenSnapshot,
    timezone: &TimeZone,
) -> CostPayload {
    let summary = snapshot.summary(snapshot.history_days, timezone);
    let last_30_days = last_30_days_totals(snapshot, timezone);
    CostPayload {
        provider: "opencodex".to_owned(),
        source: "opencodex".to_owned(),
        updated_at: Some(snapshot.updated_at),
        currency_code: Some(snapshot.currency_code.clone()),
        session_tokens: snapshot.session_tokens,
        session_cost_usd: snapshot.session_cost_usd,
        history_days: Some(snapshot.history_days as i64),
        reporting_period: Some(
            snapshot
                .reporting_period
                .unwrap_or(CostReportingPeriod::Rolling {
                    days: snapshot.history_days,
                })
                .raw_value(),
        ),
        history_label: Some(snapshot.period_label()),
        history_coverage_is_established: Some(snapshot.history_coverage_is_established),
        last_30_days_tokens: last_30_days.0,
        last_30_days_cost_usd: last_30_days.1,
        metered_cost_usd: None,
        daily: snapshot.daily.iter().map(cost_daily_payload).collect(),
        projects: Vec::new(),
        totals: cost_totals(snapshot, timezone),
        provenance: Some(CostProvenance::ListPriceEstimate.raw_value().to_owned()),
        coverage: Some(summary.coverage),
        error: None,
        incomplete_request_count: None,
    }
}

fn last_30_days_totals(
    snapshot: &CostUsageTokenSnapshot,
    timezone: &TimeZone,
) -> (Option<i64>, Option<f64>) {
    if snapshot.history_days <= 30 {
        return (snapshot.last_30_days_tokens, snapshot.last_30_days_cost_usd);
    }
    let summary = snapshot.summary(30, timezone);
    if summary.entry_count == 0 && snapshot.history_is_fully_scanned() {
        return (
            snapshot.last_30_days_tokens.map(|_| 0),
            snapshot.last_30_days_cost_usd.map(|_| 0.0),
        );
    }
    (summary.total_tokens, summary.total_cost_usd)
}

fn cost_daily_payload(entry: &CostUsageDailyReportEntry) -> CostDailyEntryPayload {
    let incomplete = entry.incomplete_request_count();
    CostDailyEntryPayload {
        date: entry.date.clone(),
        input_tokens: entry.input_tokens,
        output_tokens: entry.output_tokens,
        cache_read_tokens: entry.cache_read_tokens,
        cache_creation_tokens: entry.cache_creation_tokens,
        reasoning_tokens: entry.reasoning_tokens,
        total_tokens: entry.total_tokens,
        cost_usd: entry.cost_usd,
        models_used: entry.models_used.clone(),
        incomplete_request_count: (incomplete > 0).then_some(incomplete),
        model_breakdowns: entry
            .model_breakdowns
            .as_ref()
            .map(|bds| bds.iter().map(cost_model_breakdown_payload).collect()),
    }
}

fn cost_model_breakdown_payload(breakdown: &CostUsageModelBreakdown) -> CostModelBreakdownPayload {
    CostModelBreakdownPayload {
        model_name: breakdown.model_name.clone(),
        cost_usd: breakdown.cost_usd,
        total_tokens: breakdown.total_tokens,
        incomplete_request_count: breakdown.incomplete_request_count.filter(|&c| c > 0),
    }
}

fn cost_totals(
    snapshot: &CostUsageTokenSnapshot,
    timezone: &TimeZone,
) -> Option<CostTotalsPayload> {
    let entries = &snapshot.daily;
    if entries.is_empty()
        && snapshot.last_30_days_tokens.is_none()
        && snapshot.last_30_days_cost_usd.is_none()
    {
        return None;
    }

    let sum_field = |f: fn(&CostUsageDailyReportEntry) -> Option<i64>| -> Option<i64> {
        let values: Vec<i64> = entries.iter().filter_map(f).collect();
        if values.is_empty() {
            None
        } else {
            checked_sum_i64(values.into_iter())
        }
    };

    let costs: Vec<f64> = entries.iter().filter_map(|e| e.cost_usd).collect();
    let summary = (!entries.is_empty()).then(|| snapshot.summary(snapshot.history_days, timezone));
    let incomplete = sum_incomplete_requests(
        entries
            .iter()
            .map(CostUsageDailyReportEntry::incomplete_request_count),
    );
    Some(CostTotalsPayload {
        total_input_tokens: sum_field(|e| e.input_tokens),
        total_output_tokens: sum_field(|e| e.output_tokens),
        cache_read_tokens: sum_field(|e| e.cache_read_tokens),
        cache_creation_tokens: sum_field(|e| e.cache_creation_tokens),
        reasoning_tokens: sum_field(|e| e.reasoning_tokens),
        total_tokens: sum_field(|e| e.total_tokens).or(snapshot.last_30_days_tokens),
        total_cost_usd: if costs.is_empty() {
            snapshot.last_30_days_cost_usd
        } else {
            Some(costs.into_iter().sum())
        },
        provenance: summary
            .as_ref()
            .map(|s| s.provenance.raw_value().to_owned()),
        incomplete_request_count: (incomplete > 0).then_some(incomplete),
        coverage: summary.map(|s| s.coverage),
    })
}

/// CLI options for `ariadusage cost`.
#[derive(Debug, Clone, Args, Default)]
pub struct CostCommandArgs {
    /// Provider to query (codex | claude | antigravity | all)
    #[arg(short = 'p', long = "provider")]
    pub provider: Option<String>,

    /// Output format: text | json
    #[arg(long = "format")]
    pub format: Option<String>,

    /// Shorthand for `--format json`
    #[arg(short = 'j', long = "json")]
    pub json: bool,

    /// Emit JSON-only output (errors also emitted as JSON)
    #[arg(long = "json-only")]
    pub json_only: bool,

    /// Pretty-print JSON output
    #[arg(long = "pretty")]
    pub pretty: bool,

    /// Disable ANSI colors in text output
    #[arg(long = "no-color")]
    pub no_color: bool,

    /// Force refresh by ignoring cached scans
    #[arg(long = "refresh")]
    pub refresh: bool,

    /// Show daily and model breakdowns in Claude text output
    #[arg(long = "breakdown")]
    pub breakdown: bool,

    /// Experimental: exclude pi and OMP session mirrors from Claude/Codex cost history
    #[arg(long = "provider-native-only")]
    pub provider_native_only: bool,

    /// Cost period: month-to-date or all; --days overrides this selection
    #[arg(long = "period")]
    pub period: Option<String>,

    /// Cost history window in days (1...365)
    #[arg(long = "days")]
    pub days: Option<usize>,

    /// Group text output by: project | session
    #[arg(long = "group-by")]
    pub group_by: Option<String>,

    /// Read one SSH host; separate summaries unless --identified-only is selected
    #[arg(long = "remote")]
    pub remote: Option<String>,

    /// Versioned native Codex JSON totals without account or session details
    #[arg(long = "summary-only")]
    pub summary_only: bool,

    /// Native Codex identified-request subtotal JSON; prices unavailable
    #[arg(long = "identified-only")]
    pub identified_only: bool,

    /// Identified subtotal window end, inclusive Unix milliseconds (UTC)
    #[arg(long = "identified-end")]
    pub identified_end: Option<i64>,

    /// Log level
    #[arg(long = "log-level")]
    pub log_level: Option<String>,

    /// Enable verbose logging
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,

    /// Emit JSON log lines on stderr
    #[arg(long = "json-output")]
    pub json_output: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CostRunEnvironment {
    pub claude_projects_roots: Option<Vec<PathBuf>>,
    pub codex_home: Option<PathBuf>,
    pub cache_root: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub now: Option<Timestamp>,
    pub timezone: Option<TimeZone>,
    pub catalog: Option<ModelsDevCatalog>,
}

pub fn run_cost_command(args: &CostCommandArgs) -> i32 {
    run_cost_command_with_env(args, &CostRunEnvironment::default())
}

pub fn run_cost_command_with_env(
    args: &CostCommandArgs,
    env_overrides: &CostRunEnvironment,
) -> i32 {
    let format = match parse_output_format(args.format.as_deref(), args.json, args.json_only) {
        Ok(f) => f,
        Err(msg) => {
            eprintln!("Error: {msg}");
            return EXIT_ARGS;
        }
    };
    let json_only = args.json_only;
    let pretty = args.pretty;

    if args.identified_only || args.identified_end.is_some() {
        return emit_cost_cli_error(
            EXIT_COST_UNSUPPORTED,
            "args",
            "Error: --identified-only and --identified-end are not supported in local cost mode.",
            format,
            json_only,
            pretty,
        );
    }

    let period = match decode_cost_reporting_period(
        args.days,
        args.period.as_deref(),
        args.remote.as_deref(),
        args.summary_only,
        CostReportingPeriod::Rolling { days: 30 },
    ) {
        Ok(p) => p,
        Err(err) => {
            return emit_cost_cli_error(
                err.code,
                err.kind,
                &err.message,
                format,
                json_only,
                pretty,
            );
        }
    };

    if args.remote.is_some() || args.summary_only {
        return emit_cost_cli_error(
            EXIT_COST_UNSUPPORTED,
            "args",
            "Error: --remote and --summary-only are not supported in local cost mode.",
            format,
            json_only,
            pretty,
        );
    }

    let selected = match resolve_cost_provider_selection(args.provider.as_deref(), env_overrides) {
        Ok(sel) => sel,
        Err(msg) => {
            return emit_cost_cli_error(EXIT_ARGS, "args", &msg, format, json_only, pretty);
        }
    };

    let mut unsupported: Vec<&str> = selected
        .iter()
        .filter(|p| !p.supports_cost_command())
        .map(|p| p.display_name())
        .collect();
    unsupported.sort_unstable();
    if !unsupported.is_empty() && !json_only {
        eprintln!(
            "Skipping providers without local cost usage: {}",
            unsupported.join(", ")
        );
    }

    let providers: Vec<CostProvider> = selected
        .into_iter()
        .filter(|p| p.supports_cost_command())
        .collect();
    if providers.is_empty() {
        return emit_cost_cli_error(
            1,
            "args",
            "Error: cost is only supported for Claude, Codex.",
            format,
            json_only,
            pretty,
        );
    }

    let include_pi_sessions = decode_cost_include_pi_sessions(args.provider_native_only);
    let group_by = decode_cost_group_by(args.group_by.as_deref());
    warn_skipped_grouping_providers(group_by, &providers, json_only);
    if providers.contains(&CostProvider::Codex)
        && !json_only
        && let Some(warning) = session_grouping_pi_omission_warning(
            CostProvider::Codex,
            group_by,
            format,
            include_pi_sessions,
        )
    {
        eprintln!("Warning: {warning}");
    }

    let output_providers = cost_providers(&providers, group_by, format);
    let use_color = should_use_color(args.no_color, format);
    let now = env_overrides.now.unwrap_or_else(Timestamp::now);
    let timezone = env_overrides
        .timezone
        .clone()
        .unwrap_or_else(TimeZone::system);
    let history_days = period.days(now, &timezone, None);

    let mut sections: Vec<String> = Vec::new();
    let mut payloads: Vec<CostPayload> = Vec::new();

    for provider in &output_providers {
        let snapshot = load_local_provider_snapshot(
            *provider,
            period,
            history_days,
            now,
            &timezone,
            args.refresh,
            env_overrides,
        );
        match format {
            OutputFormat::Text => {
                sections.push(render_cost_text(
                    *provider,
                    &snapshot,
                    group_by,
                    use_color,
                    &timezone,
                    args.breakdown,
                ));
            }
            OutputFormat::Json => {
                payloads.push(make_cost_payload(
                    *provider,
                    Some(&snapshot),
                    None,
                    &timezone,
                ));
            }
        }
    }

    match format {
        OutputFormat::Text => {
            if !sections.is_empty() {
                println!("{}", sections.join("\n\n"));
            }
        }
        OutputFormat::Json => {
            if !payloads.is_empty() {
                let rendered = if pretty {
                    serde_json::to_string_pretty(&payloads)
                } else {
                    serde_json::to_string(&payloads)
                }
                .unwrap_or_else(|_| "[]".to_owned());
                println!("{rendered}");
            }
        }
    }

    0
}

fn resolve_cost_provider_selection(
    raw_provider: Option<&str>,
    env_overrides: &CostRunEnvironment,
) -> Result<Vec<CostProvider>, String> {
    if let Some(raw) = raw_provider {
        let trimmed = raw.trim().to_ascii_lowercase();
        if trimmed == "all" || trimmed == "both" {
            return Ok(vec![
                CostProvider::Codex,
                CostProvider::Claude,
                CostProvider::Antigravity,
            ]);
        }
        let Some(p) = CostProvider::from_raw(&trimmed) else {
            return Err(format!("Error: Unknown provider '{raw}'."));
        };
        return Ok(vec![p]);
    }

    // Default to enabled providers in config (or defaults if config is absent/unreadable).
    let store = env_overrides
        .config_path
        .clone()
        .or_else(|| config::resolve_cli_config_path().ok())
        .map(ariadusage_engine::config_store::ConfigStore::new);
    if let Some(store) = store
        && let Ok(cfg) = store.load_effective()
    {
        let statuses = config::config_provider_statuses(&cfg);
        let enabled: Vec<CostProvider> = statuses
            .into_iter()
            .filter(|s| s.enabled)
            .filter_map(|s| CostProvider::from_raw(&s.provider))
            .collect();
        if !enabled.is_empty() {
            return Ok(enabled);
        }
    }
    Ok(vec![CostProvider::Codex, CostProvider::Claude])
}

fn warn_skipped_grouping_providers(
    group_by: CostGroupBy,
    providers: &[CostProvider],
    json_only: bool,
) {
    if json_only {
        return;
    }
    let mut unsupported: Vec<&str> = providers
        .iter()
        .filter(|&&p| p != CostProvider::Codex)
        .map(|p| p.display_name())
        .collect();
    if unsupported.is_empty() {
        return;
    }
    unsupported.sort_unstable();
    let names = unsupported.join(", ");
    match group_by {
        CostGroupBy::Project => {
            eprintln!(
                "Skipping project grouping for providers without Codex project data: {names}"
            );
        }
        CostGroupBy::Session => {
            eprintln!(
                "Skipping session grouping for providers without Codex session data: {names}"
            );
        }
        CostGroupBy::None => {}
    }
}

fn should_use_color(no_color: bool, format: OutputFormat) -> bool {
    if no_color || format != OutputFormat::Text {
        return false;
    }
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var("TERM").is_ok_and(|t| t == "dumb") {
        return false;
    }
    std::io::stdout().is_terminal()
}

fn emit_cost_cli_error(
    code: i32,
    kind: &str,
    message: &str,
    format: OutputFormat,
    json_only: bool,
    pretty: bool,
) -> i32 {
    if format == OutputFormat::Json || json_only {
        let payload = serde_json::json!([{
            "error": {
                "code": code,
                "kind": kind,
                "message": message,
            }
        }]);
        let rendered = if pretty {
            serde_json::to_string_pretty(&payload)
        } else {
            serde_json::to_string(&payload)
        }
        .unwrap_or_else(|_| "[]".to_owned());
        println!("{rendered}");
    } else {
        eprintln!("{message}");
    }
    code
}

fn load_local_provider_snapshot(
    provider: CostProvider,
    period: CostReportingPeriod,
    history_days: usize,
    now: Timestamp,
    timezone: &TimeZone,
    force_refresh: bool,
    env_overrides: &CostRunEnvironment,
) -> CostUsageTokenSnapshot {
    let (since_key, until_key) = period.bounds(now, timezone, None);
    let range = DayRange::from_day_keys(
        since_key.as_str(),
        until_key.as_str(),
        timestamp::BucketTimeZone::from_tz(timezone.clone()),
    );
    let cache_root = env_overrides
        .cache_root
        .clone()
        .unwrap_or_else(store::default_cache_root);

    match provider {
        CostProvider::Claude => {
            let roots = env_overrides
                .claude_projects_roots
                .clone()
                .unwrap_or_default();
            let claude_opts = ClaudeScanOptions {
                claude_projects_roots: roots,
                provider_filter: ClaudeLogProviderFilter::ExcludeVertexAi,
                timezone: Some(timezone.clone()),
                catalog: env_overrides.catalog.clone(),
            };
            let rows = ariadusage_engine::cost::claude::scan_claude_projects(&range, &claude_opts);
            let engine_report = ledger::build_claude_daily_report(&rows, &range);
            let mut store = CostUsageStore::new(Some(&cache_root));
            let _ = store.save_provider_report("claude", &engine_report, now.as_millisecond());
            snapshot_from_engine_report(
                &engine_report,
                Vec::new(),
                Vec::new(),
                period,
                history_days,
                now,
                timezone,
            )
        }
        CostProvider::Codex => {
            let persisted_cursor = if force_refresh {
                None
            } else {
                let mut store = CostUsageStore::new(Some(&cache_root));
                store.load_metadata().and_then(|m| m.priority_turns_cursor)
            };
            let codex_opts = CodexScanOptions {
                codex_home: env_overrides.codex_home.clone(),
                codex_sessions_root: None,
                priority_db_path: None,
                temp_dir: Some(store::default_sqlite_copy_parent(Some(&cache_root))),
                max_sqlite_bytes: None,
                priority_deadline: None,
                persisted_priority_cursor: persisted_cursor,
                force_rescan: force_refresh,
                timezone: Some(timezone.clone()),
                catalog: env_overrides.catalog.clone(),
            };
            let scan = ariadusage_engine::cost::codex::scan_codex_home(&range, &codex_opts);
            let mut store = CostUsageStore::new(Some(&cache_root));
            let engine_report = store.save_codex_scan(&scan, &range, now.as_millisecond());
            let (projects, sessions) = build_codex_projects_and_sessions(&scan, &range, timezone);
            snapshot_from_engine_report(
                &engine_report,
                projects,
                sessions,
                period,
                history_days,
                now,
                timezone,
            )
        }
        _ => CostUsageTokenSnapshot {
            updated_at: now,
            history_days,
            ..CostUsageTokenSnapshot::default()
        }
        .reporting(period),
    }
}

fn convert_engine_entry(entry: &EngineDailyEntry) -> CostUsageDailyReportEntry {
    CostUsageDailyReportEntry {
        date: entry.date.clone(),
        input_tokens: entry.input_tokens.and_then(|v| i64::try_from(v).ok()),
        output_tokens: entry.output_tokens.and_then(|v| i64::try_from(v).ok()),
        cache_read_tokens: entry.cache_read_tokens.and_then(|v| i64::try_from(v).ok()),
        cache_creation_tokens: entry
            .cache_creation_tokens
            .and_then(|v| i64::try_from(v).ok()),
        reasoning_tokens: None,
        total_tokens: entry.total_tokens.and_then(|v| i64::try_from(v).ok()),
        request_count: entry.request_count.and_then(|v| i64::try_from(v).ok()),
        cost_usd: entry.cost_usd,
        models_used: (!entry.models_used.is_empty()).then(|| entry.models_used.clone()),
        model_breakdowns: (!entry.model_breakdowns.is_empty()).then(|| {
            entry
                .model_breakdowns
                .iter()
                .map(|b| CostUsageModelBreakdown {
                    model_name: b.model_name.clone(),
                    cost_usd: b.cost_usd,
                    total_tokens: b.total_tokens.and_then(|v| i64::try_from(v).ok()),
                    standard_cost_usd: b.standard_cost_usd,
                    priority_cost_usd: b.priority_cost_usd,
                    standard_tokens: b.standard_tokens.and_then(|v| i64::try_from(v).ok()),
                    priority_tokens: b.priority_tokens.and_then(|v| i64::try_from(v).ok()),
                    ..CostUsageModelBreakdown::default()
                })
                .collect()
        }),
        unpriced_request_count: entry
            .unpriced_request_count
            .and_then(|v| i64::try_from(v).ok()),
        priced_request_count: entry
            .priced_request_count
            .and_then(|v| i64::try_from(v).ok()),
        unmetered_request_count: entry
            .unmetered_request_count
            .and_then(|v| i64::try_from(v).ok()),
        estimated_request_count: None,
    }
}

fn snapshot_from_engine_report(
    report: &ledger::CostUsageDailyReport,
    projects: Vec<CostUsageProjectBreakdown>,
    sessions: Vec<CostUsageSessionBreakdown>,
    period: CostReportingPeriod,
    history_days: usize,
    now: Timestamp,
    timezone: &TimeZone,
) -> CostUsageTokenSnapshot {
    let daily: Vec<CostUsageDailyReportEntry> =
        report.data.iter().map(convert_engine_entry).collect();
    let today_key = timestamp::day_key_from_utc(now, Some(timezone.clone()));
    let today_entry = daily.iter().find(|e| e.date.trim() == today_key.as_str());
    let scan_is_partial = report.priority_data_is_partial;
    let established = true;
    let fully_scanned = established && !scan_is_partial;

    let session_tokens = today_entry
        .and_then(|e| e.total_tokens)
        .or_else(|| fully_scanned.then_some(0));
    let session_cost_usd = match today_entry {
        Some(entry) => entry.cost_usd,
        None => fully_scanned.then_some(0.0),
    };
    let last_30_days_tokens = if !daily.is_empty() && daily.iter().all(|e| e.total_tokens.is_some())
    {
        checked_sum_i64(daily.iter().filter_map(|e| e.total_tokens))
    } else if daily.is_empty() && fully_scanned {
        Some(0)
    } else {
        None
    };
    let last_30_days_cost_usd = if !daily.is_empty() && daily.iter().all(|e| e.cost_usd.is_some()) {
        Some(daily.iter().filter_map(|e| e.cost_usd).sum())
    } else if daily.is_empty() && fully_scanned {
        Some(0.0)
    } else {
        None
    };

    let effective_history_days = if period == CostReportingPeriod::AllTime {
        let earliest = daily
            .iter()
            .filter_map(|e| timestamp::parse_day_key_date(&e.date))
            .min()
            .and_then(|d| {
                d.at(0, 0, 0, 0)
                    .to_zoned(timezone.clone())
                    .ok()
                    .map(|z| z.timestamp())
            })
            .unwrap_or(now);
        period.days(now, timezone, Some(earliest))
    } else {
        history_days.max(1)
    };

    CostUsageTokenSnapshot {
        session_tokens,
        session_cost_usd,
        session_requests: None,
        last_30_days_tokens,
        last_30_days_cost_usd,
        last_30_days_requests: None,
        currency_code: "USD".to_owned(),
        history_days: effective_history_days,
        history_coverage_is_established: established,
        history_scan_is_partial: scan_is_partial,
        history_label: None,
        reporting_period: None,
        metered_cost_usd: None,
        cost_provenance: CostProvenance::ListPriceEstimate,
        daily,
        details_are_complete: !scan_is_partial,
        projects,
        sessions,
        updated_at: now,
    }
    .reporting(period)
}

fn build_codex_projects_and_sessions(
    scan: &CodexScanReport,
    range: &DayRange,
    timezone: &TimeZone,
) -> (
    Vec<CostUsageProjectBreakdown>,
    Vec<CostUsageSessionBreakdown>,
) {
    let mut latest_sessions: BTreeMap<String, (&CodexParseResult, CostUsageDailyReport, i64)> =
        BTreeMap::new();
    let mut reports_by_project: BTreeMap<String, BTreeMap<String, Vec<CostUsageDailyReport>>> =
        BTreeMap::new();

    for (file_path, usage) in &scan.files {
        let single_scan = CodexScanReport {
            rows: usage.rows.clone(),
            unmetered_requests_by_day: BTreeMap::new(),
            unpriced_requests_by_day: BTreeMap::new(),
            priority_data_is_partial: scan.priority_data_is_partial,
            validation_pending: false,
            persisted_priority_cursor: None,
            files: BTreeMap::new(),
        };
        let engine_report = ledger::build_codex_daily_report(&single_scan, range);
        if engine_report.data.is_empty() {
            continue;
        }
        let report = CostUsageDailyReport {
            data: engine_report
                .data
                .iter()
                .map(convert_engine_entry)
                .collect(),
            summary: engine_report.summary.map(|s| CostUsageDailyReportSummary {
                total_input_tokens: s.total_input_tokens.and_then(|v| i64::try_from(v).ok()),
                total_output_tokens: s.total_output_tokens.and_then(|v| i64::try_from(v).ok()),
                cache_read_tokens: s.cache_read_tokens.and_then(|v| i64::try_from(v).ok()),
                cache_creation_tokens: s.cache_creation_tokens.and_then(|v| i64::try_from(v).ok()),
                reasoning_tokens: None,
                total_tokens: s.total_tokens.and_then(|v| i64::try_from(v).ok()),
                total_cost_usd: s.total_cost_usd,
            }),
        };

        let session_id = usage
            .session_id
            .clone()
            .or_else(|| usage.codex_session.as_ref().map(|m| m.session_id.clone()))
            .unwrap_or_else(|| {
                Path::new(file_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_owned()
            });
        let mtime_ms = usage
            .codex_session
            .as_ref()
            .and_then(|m| m.latest_activity_unix_ms.or(m.started_at_unix_ms))
            .unwrap_or(0);
        if !session_id.is_empty()
            && latest_sessions
                .get(&session_id)
                .is_none_or(|existing| existing.2 < mtime_ms)
        {
            latest_sessions.insert(session_id, (usage, report.clone(), mtime_ms));
        }

        let source_path = usage
            .codex_session
            .as_ref()
            .and_then(|m| m.cwd.clone())
            .unwrap_or_default();
        let project_key = source_path.clone();
        reports_by_project
            .entry(project_key)
            .or_default()
            .entry(source_path)
            .or_default()
            .push(report);
    }

    let mut projects: Vec<CostUsageProjectBreakdown> = reports_by_project
        .into_iter()
        .map(|(project_path, by_source)| {
            let all_reports: Vec<CostUsageDailyReport> =
                by_source.values().flatten().cloned().collect();
            let merged = CostUsageDailyReport::merged(&all_reports, timezone);
            let resolved_path = (!project_path.is_empty()).then_some(project_path);
            let name = codex_project_name(resolved_path.as_deref());
            let mut sources: Vec<CostUsageProjectSourceBreakdown> = by_source
                .into_iter()
                .map(|(source_path, reports)| {
                    let source_merged = CostUsageDailyReport::merged(&reports, timezone);
                    let resolved_source = (!source_path.is_empty()).then_some(source_path);
                    let source_name = codex_project_name(resolved_source.as_deref());
                    let model_breakdowns = codex_project_model_breakdowns(&source_merged.data);
                    CostUsageProjectSourceBreakdown::new(
                        source_name,
                        resolved_source,
                        source_merged.summary.as_ref().and_then(|s| s.total_tokens),
                        source_merged
                            .summary
                            .as_ref()
                            .and_then(|s| s.total_cost_usd),
                        source_merged.data,
                        model_breakdowns,
                    )
                })
                .collect();
            sources.sort_by(|lhs, rhs| {
                compare_cost_token_name(
                    lhs.total_cost_usd,
                    lhs.total_tokens,
                    &lhs.name,
                    rhs.total_cost_usd,
                    rhs.total_tokens,
                    &rhs.name,
                )
            });
            let model_breakdowns = codex_project_model_breakdowns(&merged.data);
            CostUsageProjectBreakdown::new(
                name,
                resolved_path,
                merged.summary.as_ref().and_then(|s| s.total_tokens),
                merged.summary.as_ref().and_then(|s| s.total_cost_usd),
                merged.data,
                model_breakdowns,
                sources,
            )
        })
        .collect();
    projects.sort_by(|lhs, rhs| {
        compare_cost_token_name(
            lhs.total_cost_usd,
            lhs.total_tokens,
            &lhs.name,
            rhs.total_cost_usd,
            rhs.total_tokens,
            &rhs.name,
        )
    });

    let mut sessions: Vec<CostUsageSessionBreakdown> = latest_sessions
        .into_iter()
        .map(|(id, (usage, report, mtime_ms))| {
            let summary = report.summary.as_ref();
            let request_counts: Vec<i64> =
                report.data.iter().filter_map(|e| e.request_count).collect();
            let project_path = usage
                .codex_session
                .as_ref()
                .and_then(|m| m.cwd.clone())
                .filter(|s| !s.is_empty());
            let project_name = project_path.as_deref().map(|p| codex_project_name(Some(p)));
            let last_activity =
                Timestamp::from_millisecond(mtime_ms).unwrap_or(Timestamp::UNIX_EPOCH);
            CostUsageSessionBreakdown {
                session_id: id,
                last_activity,
                input_tokens: summary.and_then(|s| s.total_input_tokens),
                cached_input_tokens: summary.and_then(|s| s.cache_read_tokens),
                output_tokens: summary.and_then(|s| s.total_output_tokens),
                reasoning_tokens: summary.and_then(|s| s.reasoning_tokens),
                total_tokens: summary.and_then(|s| s.total_tokens),
                request_count: (!request_counts.is_empty())
                    .then(|| request_counts.into_iter().sum()),
                cost_usd: summary.and_then(|s| s.total_cost_usd),
                model_breakdowns: codex_project_model_breakdowns(&report.data).unwrap_or_default(),
                project_path,
                project_name,
                title: usage.codex_session.as_ref().and_then(|m| m.title.clone()),
            }
        })
        .collect();
    sessions.sort_by(|lhs, rhs| {
        if lhs.last_activity != rhs.last_activity {
            return rhs.last_activity.cmp(&lhs.last_activity);
        }
        rhs.session_id.cmp(&lhs.session_id)
    });

    (projects, sessions)
}

fn codex_project_name(path: Option<&str>) -> String {
    let Some(path) = path.filter(|s| !s.is_empty()) else {
        return CostUsageProjectBreakdown::UNKNOWN_PROJECT_NAME.to_owned();
    };
    Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(path)
        .to_owned()
}

fn codex_project_model_breakdowns(
    entries: &[CostUsageDailyReportEntry],
) -> Option<Vec<CostUsageModelBreakdown>> {
    let mut accumulators: BTreeMap<String, BreakdownAccumulator> = BTreeMap::new();
    for entry in entries {
        if let Some(ref breakdowns) = entry.model_breakdowns {
            for breakdown in breakdowns {
                accumulators
                    .entry(breakdown.model_name.clone())
                    .or_default()
                    .add(breakdown);
            }
        }
    }
    if accumulators.is_empty() {
        return None;
    }
    let mut summaries: Vec<CostUsageModelBreakdown> = accumulators
        .into_iter()
        .map(|(name, acc)| acc.build(name, false))
        .collect();
    CostUsageDailyReport::sort_model_breakdowns(&mut summaries);
    Some(summaries)
}

fn compare_cost_token_name(
    l_cost: Option<f64>,
    l_tok: Option<i64>,
    l_name: &str,
    r_cost: Option<f64>,
    r_tok: Option<i64>,
    r_name: &str,
) -> Ordering {
    let lc = l_cost.unwrap_or(-1.0);
    let rc = r_cost.unwrap_or(-1.0);
    if (lc - rc).abs() > 1e-12 {
        return rc.partial_cmp(&lc).unwrap_or(Ordering::Equal);
    }
    let lt = l_tok.unwrap_or(-1);
    let rt = r_tok.unwrap_or(-1);
    if lt != rt {
        return rt.cmp(&lt);
    }
    l_name.cmp(r_name)
}
