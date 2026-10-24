// Ported from CodexBar Sources/CodexBarCore/CostUsageModels.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/CostUsageFetcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use ariadusage_protocol::usage::{
    CostDailyEntry, CostHistory, CostModelBreakdown, CostProvenance, CostWindowSummary,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

use crate::cost::claude::ClaudeUsageRow;
use crate::cost::codex::CodexScanReport;
use crate::cost::timestamp::{self, DayRange};

/// Single day entry in `CostUsageDailyReport` (`CostUsageDailyReport.Entry` in CodexBar).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CostUsageDailyEntry {
    pub date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unmetered_request_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unpriced_request_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priced_request_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models_used: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_breakdowns: Vec<CostModelBreakdown>,
}

impl CostUsageDailyEntry {
    /// Projects this internal daily report entry into Snapshot/IPC v1 `CostDailyEntry`.
    #[must_use]
    pub fn to_protocol_entry(&self) -> CostDailyEntry {
        CostDailyEntry {
            date: self.date.clone(),
            total_tokens: self.total_tokens,
            cost_usd: self.cost_usd,
            total_requests: self.request_count,
            models_used: self.models_used.clone(),
            model_breakdowns: self.model_breakdowns.clone(),
        }
    }
}

/// Summary totals across a `CostUsageDailyReport`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CostUsageDailySummary {
    pub total_input_tokens: Option<u64>,
    pub total_output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub total_cost_usd: Option<f64>,
    pub total_requests: Option<u64>,
}

/// Daily cost & token usage report (`CostUsageDailyReport` in CodexBar).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CostUsageDailyReport {
    pub data: Vec<CostUsageDailyEntry>,
    pub summary: Option<CostUsageDailySummary>,
    #[serde(default)]
    pub priority_data_is_partial: bool,
}

#[derive(Debug, Clone, Default)]
struct ModelAccumulator {
    total_tokens: u64,
    cost_nanos: i64,
    saw_priced: bool,
    saw_unpriced: bool,
    standard_tokens: u64,
    priority_tokens: u64,
    standard_cost_nanos: i64,
    priority_cost_nanos: i64,
    saw_standard_cost: bool,
    saw_priority_cost: bool,
}

impl ModelAccumulator {
    fn add_claude(&mut self, row: &ClaudeUsageRow) {
        self.total_tokens = self
            .total_tokens
            .saturating_add(row.total_tokens().max(0) as u64);
        if row.cost_priced {
            self.cost_nanos = self.cost_nanos.saturating_add(row.cost_nanos);
            self.saw_priced = true;
        } else {
            self.saw_unpriced = true;
        }
    }

    fn add_codex(&mut self, tokens: u64, cost_nanos: Option<i64>, is_priority: bool) {
        self.total_tokens = self.total_tokens.saturating_add(tokens);
        if is_priority {
            self.priority_tokens = self.priority_tokens.saturating_add(tokens);
        } else {
            self.standard_tokens = self.standard_tokens.saturating_add(tokens);
        }
        if let Some(nanos) = cost_nanos {
            self.cost_nanos = self.cost_nanos.saturating_add(nanos);
            self.saw_priced = true;
            if is_priority {
                self.priority_cost_nanos = self.priority_cost_nanos.saturating_add(nanos);
                self.saw_priority_cost = true;
            } else {
                self.standard_cost_nanos = self.standard_cost_nanos.saturating_add(nanos);
                self.saw_standard_cost = true;
            }
        } else {
            self.saw_unpriced = true;
        }
    }

    fn add_breakdown(&mut self, breakdown: &CostModelBreakdown) {
        if let Some(tokens) = breakdown.total_tokens {
            self.total_tokens = self.total_tokens.saturating_add(tokens);
        }
        if let Some(cost) = breakdown.cost_usd
            && cost.is_finite()
            && cost >= 0.0
        {
            self.cost_nanos = self
                .cost_nanos
                .saturating_add((cost * 1_000_000_000.0).round() as i64);
            self.saw_priced = true;
        } else {
            self.saw_unpriced = true;
        }
    }

    fn into_breakdown(self, model_name: String) -> CostModelBreakdown {
        let cost_usd = if self.saw_priced && !self.saw_unpriced {
            Some(self.cost_nanos as f64 / 1_000_000_000.0)
        } else {
            None
        };
        let has_mode_split =
            !self.saw_unpriced && (self.saw_priority_cost || self.priority_tokens > 0);
        CostModelBreakdown {
            model_name,
            cost_usd,
            total_tokens: Some(self.total_tokens),
            standard_cost_usd: if has_mode_split && self.saw_standard_cost {
                Some(self.standard_cost_nanos as f64 / 1_000_000_000.0)
            } else {
                None
            },
            priority_cost_usd: if has_mode_split && self.saw_priority_cost {
                Some(self.priority_cost_nanos as f64 / 1_000_000_000.0)
            } else {
                None
            },
            standard_tokens: if has_mode_split && self.standard_tokens > 0 {
                Some(self.standard_tokens)
            } else {
                None
            },
            priority_tokens: if has_mode_split && self.priority_tokens > 0 {
                Some(self.priority_tokens)
            } else {
                None
            },
        }
    }
}

/// Sorts model breakdowns matching `CostUsageDailyReport.sortedModelBreakdowns`:
/// descending by `cost_usd` (`None` -> `-1`), then descending by `total_tokens` (`None` -> `-1`),
/// then descending by `model_name`.
pub fn sort_model_breakdowns(breakdowns: &mut [CostModelBreakdown]) {
    breakdowns.sort_by(|lhs, rhs| {
        let l_cost = lhs.cost_usd.unwrap_or(-1.0);
        let r_cost = rhs.cost_usd.unwrap_or(-1.0);
        if (l_cost - r_cost).abs() > 1e-12 {
            return r_cost.partial_cmp(&l_cost).unwrap_or(Ordering::Equal);
        }
        let l_tok = lhs.total_tokens.map_or(-1i128, i128::from);
        let r_tok = rhs.total_tokens.map_or(-1i128, i128::from);
        if l_tok != r_tok {
            return r_tok.cmp(&l_tok);
        }
        rhs.model_name.cmp(&lhs.model_name)
    });
}

/// Computes the top model across a slice of daily entries.
#[must_use]
pub fn compute_top_model(entries: &[CostDailyEntry]) -> Option<CostModelBreakdown> {
    let mut by_model: BTreeMap<String, ModelAccumulator> = BTreeMap::new();
    for entry in entries {
        for breakdown in &entry.model_breakdowns {
            by_model
                .entry(breakdown.model_name.clone())
                .or_default()
                .add_breakdown(breakdown);
        }
    }
    if by_model.is_empty() {
        return None;
    }
    let mut list: Vec<CostModelBreakdown> = by_model
        .into_iter()
        .map(|(name, acc)| acc.into_breakdown(name))
        .collect();
    sort_model_breakdowns(&mut list);
    list.into_iter().next()
}

/// Builds a `CostUsageDailyReport` from deduplicated Claude usage rows within `range`.
#[must_use]
pub fn build_claude_daily_report(
    rows: &[ClaudeUsageRow],
    range: &DayRange,
) -> CostUsageDailyReport {
    let mut by_day: BTreeMap<String, Vec<&ClaudeUsageRow>> = BTreeMap::new();
    for row in rows {
        if range.contains_day_str(&row.day_key) {
            by_day.entry(row.day_key.clone()).or_default().push(row);
        }
    }

    let mut data = Vec::with_capacity(by_day.len());
    for (day, day_rows) in by_day {
        let mut input: u64 = 0;
        let mut cache_read: u64 = 0;
        let mut cache_create: u64 = 0;
        let mut output: u64 = 0;
        let mut models: BTreeMap<String, ModelAccumulator> = BTreeMap::new();

        for row in &day_rows {
            if !row.is_incomplete {
                input = input.saturating_add(row.input.max(0) as u64);
                cache_read = cache_read.saturating_add(row.cache_read.max(0) as u64);
                cache_create = cache_create.saturating_add(row.cache_create.max(0) as u64);
                output = output.saturating_add(row.output.max(0) as u64);
                models.entry(row.model.clone()).or_default().add_claude(row);
            }
        }

        let total_tokens = input
            .saturating_add(cache_read)
            .saturating_add(cache_create)
            .saturating_add(output);
        let mut model_breakdowns: Vec<CostModelBreakdown> = models
            .into_iter()
            .map(|(name, acc)| acc.into_breakdown(name))
            .collect();
        sort_model_breakdowns(&mut model_breakdowns);

        let mut models_used: Vec<String> = model_breakdowns
            .iter()
            .map(|b| b.model_name.clone())
            .collect();
        models_used.sort();

        let all_priced =
            !model_breakdowns.is_empty() && model_breakdowns.iter().all(|b| b.cost_usd.is_some());
        let cost_usd = if all_priced {
            Some(
                model_breakdowns
                    .iter()
                    .filter_map(|b| b.cost_usd)
                    .sum::<f64>(),
            )
        } else {
            None
        };

        let request_count = day_rows.iter().filter(|r| !r.is_incomplete).count() as u64;
        let priced_count = day_rows
            .iter()
            .filter(|r| !r.is_incomplete && r.cost_priced)
            .count() as u64;
        let unpriced_count = request_count.saturating_sub(priced_count);

        data.push(CostUsageDailyEntry {
            date: day,
            input_tokens: Some(input),
            output_tokens: Some(output),
            cache_read_tokens: Some(cache_read),
            cache_creation_tokens: Some(cache_create),
            total_tokens: Some(total_tokens),
            request_count: Some(request_count),
            unmetered_request_count: None,
            unpriced_request_count: (unpriced_count > 0).then_some(unpriced_count),
            priced_request_count: Some(priced_count),
            cost_usd,
            models_used,
            model_breakdowns,
        });
    }

    let summary = compute_daily_summary(&data);
    CostUsageDailyReport {
        data,
        summary,
        priority_data_is_partial: false,
    }
}

/// Builds a `CostUsageDailyReport` from a `CodexScanReport` within `range`.
#[must_use]
pub fn build_codex_daily_report(scan: &CodexScanReport, range: &DayRange) -> CostUsageDailyReport {
    let mut all_days = BTreeSet::new();
    for row in &scan.rows {
        if range.contains_day_str(&row.day) {
            all_days.insert(row.day.clone());
        }
    }
    for (day, &unmetered) in &scan.unmetered_requests_by_day {
        if unmetered > 0 && range.contains_day_str(day) {
            all_days.insert(day.clone());
        }
    }
    for (day, &unpriced) in &scan.unpriced_requests_by_day {
        if unpriced > 0 && range.contains_day_str(day) {
            all_days.insert(day.clone());
        }
    }

    let mut data = Vec::with_capacity(all_days.len());
    for day in all_days {
        let day_rows: Vec<_> = scan.rows.iter().filter(|r| r.day == day).collect();
        let unmetered = scan
            .unmetered_requests_by_day
            .get(&day)
            .copied()
            .filter(|&c| c > 0);
        let unpriced_from_map = scan
            .unpriced_requests_by_day
            .get(&day)
            .copied()
            .unwrap_or(0);

        if day_rows.is_empty() {
            data.push(CostUsageDailyEntry {
                date: day,
                input_tokens: Some(0),
                output_tokens: Some(0),
                cache_read_tokens: Some(0),
                cache_creation_tokens: None,
                total_tokens: Some(0),
                request_count: None,
                unmetered_request_count: unmetered,
                unpriced_request_count: (unpriced_from_map > 0).then_some(unpriced_from_map),
                priced_request_count: None,
                cost_usd: None,
                models_used: Vec::new(),
                model_breakdowns: Vec::new(),
            });
            continue;
        }

        let mut input: u64 = 0;
        let mut cached: u64 = 0;
        let mut output: u64 = 0;
        let mut priced_rows: u64 = 0;
        let mut unpriced_rows: u64 = 0;
        let mut models: BTreeMap<String, ModelAccumulator> = BTreeMap::new();

        for row in &day_rows {
            let row_in = row.input.max(0) as u64;
            let row_cached = row.cached.max(0) as u64;
            let row_out = row.output.max(0) as u64;
            let row_tok = row_in.saturating_add(row_out);
            let is_priority = row.pricing_mode.as_deref() == Some("priority");
            input = input.saturating_add(row_in);
            cached = cached.saturating_add(row_cached);
            output = output.saturating_add(row_out);
            if row.known_cost_nanos.is_some() {
                priced_rows = priced_rows.saturating_add(1);
            } else {
                unpriced_rows = unpriced_rows.saturating_add(1);
            }
            models.entry(row.model.clone()).or_default().add_codex(
                row_tok,
                row.known_cost_nanos,
                is_priority,
            );
        }

        let total_tokens = input.saturating_add(output);
        let mut model_breakdowns: Vec<CostModelBreakdown> = models
            .into_iter()
            .map(|(name, acc)| acc.into_breakdown(name))
            .collect();
        sort_model_breakdowns(&mut model_breakdowns);

        let mut models_used: Vec<String> = model_breakdowns
            .iter()
            .map(|b| b.model_name.clone())
            .collect();
        models_used.sort();

        let total_unpriced = unpriced_rows
            .max(unpriced_from_map)
            .max(u64::from(scan.priority_data_is_partial && !day_rows.is_empty()));
        let unpriced_request_count = (total_unpriced > 0).then_some(total_unpriced);
        let all_priced = !model_breakdowns.is_empty()
            && model_breakdowns.iter().all(|b| b.cost_usd.is_some())
            && unmetered.is_none()
            && unpriced_request_count.is_none()
            && !scan.priority_data_is_partial;
        let cost_usd = if all_priced {
            Some(
                model_breakdowns
                    .iter()
                    .filter_map(|b| b.cost_usd)
                    .sum::<f64>(),
            )
        } else {
            None
        };

        data.push(CostUsageDailyEntry {
            date: day,
            input_tokens: Some(input),
            output_tokens: Some(output),
            cache_read_tokens: Some(cached),
            cache_creation_tokens: None,
            total_tokens: Some(total_tokens),
            request_count: Some(day_rows.len() as u64),
            unmetered_request_count: unmetered,
            unpriced_request_count,
            priced_request_count: Some(priced_rows),
            cost_usd,
            models_used,
            model_breakdowns,
        });
    }

    let summary = compute_daily_summary(&data);
    CostUsageDailyReport {
        data,
        summary,
        priority_data_is_partial: scan.priority_data_is_partial,
    }
}

fn compute_daily_summary(entries: &[CostUsageDailyEntry]) -> Option<CostUsageDailySummary> {
    if entries.is_empty() {
        return None;
    }
    let total_input_tokens = checked_sum_u64(entries.iter().filter_map(|e| e.input_tokens));
    let total_output_tokens = checked_sum_u64(entries.iter().filter_map(|e| e.output_tokens));
    let cache_read_tokens = checked_sum_u64(entries.iter().filter_map(|e| e.cache_read_tokens));
    let cache_creation_tokens =
        checked_sum_u64(entries.iter().filter_map(|e| e.cache_creation_tokens));
    let total_tokens = if entries.iter().all(|e| e.total_tokens.is_some()) {
        checked_sum_u64(entries.iter().filter_map(|e| e.total_tokens))
    } else {
        None
    };
    let total_requests = if entries.iter().all(|e| e.request_count.is_some()) {
        checked_sum_u64(entries.iter().filter_map(|e| e.request_count))
    } else {
        None
    };
    let total_cost_usd = if entries.iter().all(|e| e.cost_usd.is_some()) {
        let sum: f64 = entries.iter().filter_map(|e| e.cost_usd).sum();
        if sum.is_finite() { Some(sum) } else { None }
    } else {
        None
    };

    Some(CostUsageDailySummary {
        total_input_tokens,
        total_output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
        total_tokens,
        total_cost_usd,
        total_requests,
    })
}

fn checked_sum_u64<I: Iterator<Item = u64>>(iter: I) -> Option<u64> {
    let mut count = 0usize;
    let mut sum = 0u64;
    for val in iter {
        count += 1;
        sum = sum.checked_add(val)?;
    }
    if count == 0 { None } else { Some(sum) }
}

/// Computes a rolling calendar window summary over `entries` ending at `now`.
///
/// Mirrors `CostUsageTokenSnapshot.summary(forLastDays:calendar:)`.
#[must_use]
pub fn summary_for_last_days(
    entries: &[CostDailyEntry],
    requested_days: u32,
    now: Timestamp,
    timezone: Option<TimeZone>,
) -> CostWindowSummary {
    let days = requested_days.max(1);
    let until_key = timestamp::day_key_from_utc(now, timezone);
    let since_key = until_key.add_days(-((days as i32) - 1));

    let matching: Vec<&CostDailyEntry> = entries
        .iter()
        .filter(|entry| {
            let trimmed = entry.date.trim();
            trimmed >= since_key.as_str() && trimmed <= until_key.as_str()
        })
        .collect();

    let mut costs = Vec::new();
    let mut cost_overflow = false;
    for e in &matching {
        if let Some(c) = e.cost_usd {
            if c.is_finite() && c >= 0.0 {
                costs.push(c);
            } else {
                cost_overflow = true;
            }
        }
    }
    let cost_usd = if costs.is_empty() || cost_overflow {
        None
    } else {
        let s: f64 = costs.iter().sum();
        if s.is_finite() { Some(s) } else { None }
    };

    let has_tokens = matching.iter().any(|e| e.total_tokens.is_some());
    let total_tokens = if has_tokens {
        checked_sum_u64(matching.iter().filter_map(|e| e.total_tokens))
    } else {
        None
    };

    let has_requests = matching.iter().any(|e| e.total_requests.is_some());
    let total_requests = if has_requests {
        checked_sum_u64(matching.iter().filter_map(|e| e.total_requests))
    } else {
        None
    };

    CostWindowSummary {
        days,
        cost_usd,
        metered_cost_usd: None,
        total_tokens,
        total_requests,
        incomplete_request_count: None,
        entry_count: Some(matching.len() as u32),
        provenance: None,
    }
}

/// Computes unique, sorted comparison window summaries bounded by `history_days`.
///
/// Mirrors `CostUsageTokenSnapshot.comparisonSummaries(periods:calendar:)`.
#[must_use]
pub fn comparison_summaries(
    entries: &[CostDailyEntry],
    periods: &[u32],
    history_days: u32,
    now: Timestamp,
    timezone: Option<TimeZone>,
) -> Vec<CostWindowSummary> {
    let unique_sorted: BTreeSet<u32> = periods
        .iter()
        .map(|&d| d.max(1))
        .filter(|&d| d < history_days)
        .collect();
    unique_sorted
        .into_iter()
        .map(|days| summary_for_last_days(entries, days, now, timezone.clone()))
        .collect()
}

/// Builds a Snapshot/IPC v1 `CostHistory` from a `CostUsageDailyReport`.
///
/// Mirrors `CostUsageFetcher.tokenSnapshot(from:now:historyDays:...)`.
#[must_use]
pub fn build_cost_history(
    report: &CostUsageDailyReport,
    now: Timestamp,
    history_days: u32,
    timezone: Option<TimeZone>,
    history_coverage_established: bool,
    cost_provenance: CostProvenance,
) -> CostHistory {
    let clamped_days = history_days.max(1);
    let today_key = timestamp::day_key_from_utc(now, timezone.clone());
    let since_key = today_key.add_days(-((clamped_days as i32) - 1));

    let daily: Vec<CostDailyEntry> = report
        .data
        .iter()
        .filter(|e| {
            let d = e.date.trim();
            d >= since_key.as_str() && d <= today_key.as_str()
        })
        .map(CostUsageDailyEntry::to_protocol_entry)
        .collect();

    let today_entry = daily.iter().find(|e| e.date.trim() == today_key.as_str());
    let established_empty = history_coverage_established && daily.is_empty();
    let empty_session_u64 = if history_coverage_established {
        Some(0u64)
    } else {
        None
    };

    let session_tokens = today_entry
        .and_then(|e| e.total_tokens)
        .or(empty_session_u64);
    let session_cost_usd = match today_entry {
        Some(entry) => entry.cost_usd,
        None => empty_session_u64.map(|v| v as f64),
    };
    let requests: Vec<u64> = daily.iter().filter_map(|e| e.total_requests).collect();
    let requests_are_complete = !requests.is_empty() && requests.len() == daily.len();
    let session_requests = today_entry.and_then(|e| e.total_requests).or({
        if requests.is_empty() && !established_empty {
            None
        } else {
            empty_session_u64
        }
    });

    let last_30_days_tokens = if !daily.is_empty() && daily.iter().all(|e| e.total_tokens.is_some())
    {
        checked_sum_u64(daily.iter().filter_map(|e| e.total_tokens))
    } else if established_empty {
        Some(0)
    } else {
        None
    };

    let last_30_days_cost_usd = if !daily.is_empty() && daily.iter().all(|e| e.cost_usd.is_some()) {
        let sum: f64 = daily.iter().filter_map(|e| e.cost_usd).sum();
        if sum.is_finite() { Some(sum) } else { None }
    } else if established_empty {
        Some(0.0)
    } else {
        None
    };

    let last_30_days_requests = if requests_are_complete {
        checked_sum_u64(requests.into_iter())
    } else if established_empty {
        Some(0)
    } else {
        None
    };

    let comparison_windows =
        comparison_summaries(&daily, &[7, 30, 90], clamped_days, now, timezone);
    let top_model = compute_top_model(&daily);

    CostHistory {
        session_tokens,
        session_cost_usd,
        session_requests,
        last_30_days_tokens,
        last_30_days_cost_usd,
        last_30_days_requests,
        history_days: Some(clamped_days),
        history_label: None,
        history_coverage_is_established: history_coverage_established,
        history_scan_is_partial: report.priority_data_is_partial,
        metered_cost_usd: None,
        currency_code: Some("USD".to_owned()),
        cost_provenance: Some(cost_provenance),
        daily,
        comparison_windows,
        top_model,
        hint_lines: Vec::new(),
    }
}
