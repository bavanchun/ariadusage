// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CodexSubagentRolloutShape.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner+ForkCoverage.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner+RequestIdentity.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner+ReportReconciliation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cost::codex_priority::{self, CodexPriorityTurnMetadata};
use crate::cost::pricing::{self, CodexServiceTier, ModelsDevCatalog};
use crate::cost::timestamp::{self, DayKey, DayRange};

/// Maximum line bytes for `session_meta` head scanning (`8 MiB`, matching `codexSessionMetadataMaxLineBytes`).
pub const CODEX_SESSION_METADATA_MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// Maximum buffered raw-totals entries for exact re-emission suppression (`64`, matching `seenRawTotalsLimit`).
pub const SEEN_RAW_TOTALS_LIMIT: usize = 64;

/// Maximum millisecond gap for delayed `token_usage_record` / `token_count` mirror pairing (`5000 ms`).
pub const NEAR_MIRROR_MAX_DELAY_MS: i64 = 5_000;

/// Cumulative or delta token vector for Codex usage accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CodexTotals {
    pub input: i64,
    pub cached: i64,
    pub output: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<i64>,
}

impl CodexTotals {
    #[must_use]
    pub const fn new(input: i64, cached: i64, output: i64, reasoning: Option<i64>) -> Self {
        Self {
            input,
            cached,
            output,
            reasoning,
        }
    }

    #[must_use]
    pub const fn is_zero(&self) -> bool {
        self.input <= 0 && self.cached <= 0 && self.output <= 0
    }

    #[must_use]
    pub const fn scanner_units(&self) -> i64 {
        self.input
            .saturating_add(self.cached)
            .saturating_add(self.output)
    }
}

#[must_use]
pub fn codex_totals_equal(lhs: Option<CodexTotals>, rhs: Option<CodexTotals>) -> bool {
    match (lhs, rhs) {
        (Some(l), Some(r)) => l.input == r.input && l.cached == r.cached && l.output == r.output,
        (None, None) => true,
        _ => false,
    }
}

#[must_use]
pub fn codex_totals_at_least(lhs: CodexTotals, rhs: CodexTotals) -> bool {
    lhs.input >= rhs.input && lhs.cached >= rhs.cached && lhs.output >= rhs.output
}

#[must_use]
pub fn codex_totals_at_most(lhs: CodexTotals, rhs: CodexTotals) -> bool {
    lhs.input <= rhs.input && lhs.cached <= rhs.cached && lhs.output <= rhs.output
}

#[must_use]
pub fn codex_looks_like_stale_regression(
    current: CodexTotals,
    previous: CodexTotals,
    last: CodexTotals,
) -> bool {
    let reasoning_regressed = match (current.reasoning, previous.reasoning) {
        (Some(cur), Some(prev)) => cur < prev,
        _ => false,
    };
    if current.input >= previous.input
        && current.cached >= previous.cached
        && current.output >= previous.output
        && !reasoning_regressed
    {
        return false;
    }
    let previous_total = previous
        .input
        .saturating_add(previous.output)
        .saturating_add(previous.cached)
        .saturating_add(previous.reasoning.unwrap_or(0));
    let current_total = current
        .input
        .saturating_add(current.output)
        .saturating_add(current.cached)
        .saturating_add(current.reasoning.unwrap_or(0));
    let last_total = last
        .input
        .saturating_add(last.output)
        .saturating_add(last.cached)
        .saturating_add(last.reasoning.unwrap_or(0));
    if previous_total <= 0 || current_total <= 0 || last_total <= 0 {
        return false;
    }
    current_total.saturating_mul(100) >= previous_total.saturating_mul(98)
        || current_total.saturating_add(last_total.saturating_mul(2)) >= previous_total
}

#[must_use]
pub fn codex_should_prefer_total_delta(
    raw_baseline: Option<CodexTotals>,
    current_total: CodexTotals,
    total_delta: CodexTotals,
    last_delta: CodexTotals,
    saw_divergent_totals: bool,
) -> bool {
    if saw_divergent_totals {
        return false;
    }
    let Some(raw_baseline) = raw_baseline else {
        return false;
    };
    codex_totals_at_least(current_total, raw_baseline)
        && codex_totals_at_most(total_delta, last_delta)
}

#[must_use]
pub fn codex_add_totals(lhs: CodexTotals, rhs: CodexTotals) -> CodexTotals {
    CodexTotals {
        input: lhs.input.saturating_add(rhs.input),
        cached: lhs.cached.saturating_add(rhs.cached),
        output: lhs.output.saturating_add(rhs.output),
        reasoning: match (lhs.reasoning, rhs.reasoning) {
            (Some(l), Some(r)) => Some(l.saturating_add(r)),
            _ => None,
        },
    }
}

#[must_use]
pub fn codex_min_totals(lhs: CodexTotals, rhs: CodexTotals) -> CodexTotals {
    CodexTotals {
        input: lhs.input.min(rhs.input),
        cached: lhs.cached.min(rhs.cached),
        output: lhs.output.min(rhs.output),
        reasoning: match (lhs.reasoning, rhs.reasoning) {
            (Some(l), Some(r)) => Some(l.min(r)),
            _ => None,
        },
    }
}

#[must_use]
pub fn codex_max_totals(lhs: Option<CodexTotals>, rhs: CodexTotals) -> CodexTotals {
    let Some(lhs) = lhs else {
        return rhs;
    };
    CodexTotals {
        input: lhs.input.max(rhs.input),
        cached: lhs.cached.max(rhs.cached),
        output: lhs.output.max(rhs.output),
        reasoning: match (lhs.reasoning, rhs.reasoning) {
            (Some(l), Some(r)) => Some(l.max(r)),
            (Some(l), None) => Some(l),
            (None, Some(r)) => Some(r),
            (None, None) => None,
        },
    }
}

#[must_use]
pub fn codex_total_delta(baseline: Option<CodexTotals>, current: CodexTotals) -> CodexTotals {
    let reasoning = match (baseline, current.reasoning) {
        (_, None) => None,
        (None, Some(cur)) => Some(cur),
        (Some(base), Some(cur)) => base.reasoning.map(|b| (cur - b).max(0)),
    };
    let base = baseline.unwrap_or_default();
    CodexTotals {
        input: (current.input - base.input).max(0),
        cached: (current.cached - base.cached).max(0),
        output: (current.output - base.output).max(0),
        reasoning,
    }
}

#[must_use]
pub fn codex_divergent_total_delta(
    raw_baseline: Option<CodexTotals>,
    counted_baseline: Option<CodexTotals>,
    current: CodexTotals,
) -> CodexTotals {
    let raw = raw_baseline.unwrap_or_default();
    let counted = counted_baseline.unwrap_or_default();
    let delta_component = |r: i64, c: i64, cur: i64| -> i64 {
        if cur >= r {
            (cur - r).max(0)
        } else {
            (cur - c).max(0)
        }
    };
    let reasoning = match (raw.reasoning, counted.reasoning, current.reasoning) {
        (Some(r), Some(c), Some(cur)) => Some(delta_component(r, c, cur)),
        _ => None,
    };
    CodexTotals {
        input: delta_component(raw.input, counted.input, current.input),
        cached: delta_component(raw.cached, counted.cached, current.cached),
        output: delta_component(raw.output, counted.output, current.output),
        reasoning,
    }
}

/// Post-latch totals containment for interleaved cumulative counters (issue #2037).
#[must_use]
pub fn codex_contained_total_delta(
    watermark: Option<CodexTotals>,
    counted: Option<CodexTotals>,
    current: CodexTotals,
) -> CodexTotals {
    let water = watermark.unwrap_or_default();
    let cnt = counted.unwrap_or_default();
    let component = |w: i64, c: i64, cur: i64| -> i64 {
        if cur >= w {
            (cur - w.max(c)).max(0)
        } else {
            (cur - c).max(0)
        }
    };
    let reasoning = match (water.reasoning, cnt.reasoning, current.reasoning) {
        (Some(w), Some(c), Some(cur)) => Some(component(w, c, cur)),
        _ => None,
    };
    CodexTotals {
        input: component(water.input, cnt.input, current.input),
        cached: component(water.cached, cnt.cached, current.cached),
        output: component(water.output, cnt.output, current.output),
        reasoning,
    }
}

/// Post-latch event delta: contained totals growth, optionally capped by `adjusted_last`.
#[must_use]
pub fn codex_post_latch_event_delta(
    watermark: Option<CodexTotals>,
    counted: Option<CodexTotals>,
    current: CodexTotals,
    adjusted_last: Option<CodexTotals>,
) -> CodexTotals {
    let contained = codex_contained_total_delta(watermark, counted, current);
    match adjusted_last {
        Some(last) => codex_min_totals(last, contained),
        None => contained,
    }
}

/// High-watermark and exact re-emission tracker for Codex cumulative counters (Issue #2037).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexTotalsTracker {
    pub watermark: Option<CodexTotals>,
    pub seen_raw_totals: Vec<CodexTotals>,
    pub saw_interleaved_totals: bool,
}

impl CodexTotalsTracker {
    #[must_use]
    pub fn is_seen(&self, totals: CodexTotals) -> bool {
        self.seen_raw_totals
            .iter()
            .any(|seen| codex_totals_equal(Some(*seen), Some(totals)))
    }

    pub fn latch_if_below_watermark(&mut self, totals: CodexTotals) {
        let Some(watermark) = self.watermark else {
            return;
        };
        if totals.input < watermark.input
            || totals.cached < watermark.cached
            || totals.output < watermark.output
        {
            self.saw_interleaved_totals = true;
        }
    }

    pub fn commit_observed(&mut self, totals: CodexTotals) {
        self.raise_watermark(totals);
        if !self.is_seen(totals) {
            self.seen_raw_totals.push(totals);
            if self.seen_raw_totals.len() > SEEN_RAW_TOTALS_LIMIT {
                let excess = self.seen_raw_totals.len() - SEEN_RAW_TOTALS_LIMIT;
                self.seen_raw_totals.drain(0..excess);
            }
        }
    }

    pub fn raise_watermark(&mut self, totals: CodexTotals) {
        self.watermark = Some(codex_max_totals(self.watermark, totals));
    }
}

/// Cumulative-totals accumulator used both for parent-session baseline resolution and
/// non-fork file scanning.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexSnapshotAccumulator {
    pub counted_totals: Option<CodexTotals>,
    pub raw_totals_baseline: Option<CodexTotals>,
    pub saw_divergent_totals: bool,
    pub tracker: CodexTotalsTracker,
}

impl CodexSnapshotAccumulator {
    /// Applies one `token_count` event and returns `(counted_delta, new_counted_totals)`.
    pub fn apply_event(
        &mut self,
        last: Option<CodexTotals>,
        total: Option<CodexTotals>,
        preserve_initial_origin: bool,
    ) -> (CodexTotals, CodexTotals) {
        if preserve_initial_origin
            && self.counted_totals.is_none()
            && let (Some(total), Some(last)) = (total, last)
        {
            self.counted_totals = Some(codex_total_delta(Some(last), total));
        }

        let has_reasoning =
            last.and_then(|l| l.reasoning).is_some() || total.and_then(|t| t.reasoning).is_some();
        let base = self.counted_totals.unwrap_or(CodexTotals {
            input: 0,
            cached: 0,
            output: 0,
            reasoning: if has_reasoning { Some(0) } else { None },
        });

        if let Some(total) = total {
            if self.tracker.is_seen(total) {
                return (CodexTotals::default(), base);
            }
            let stale_baseline = self.tracker.watermark.or(self.raw_totals_baseline);
            if let Some(previous_total) = stale_baseline
                && codex_looks_like_stale_regression(
                    total,
                    previous_total,
                    last.unwrap_or_default(),
                )
            {
                return (CodexTotals::default(), base);
            }
            self.tracker.latch_if_below_watermark(total);
        }

        let watermark_baseline = self.tracker.watermark.or(self.raw_totals_baseline);

        let result = if let Some(last) = last {
            let mut counted_delta = last;
            if let Some(total) = total {
                if self.tracker.saw_interleaved_totals {
                    counted_delta = codex_post_latch_event_delta(
                        watermark_baseline,
                        self.counted_totals,
                        total,
                        Some(last),
                    );
                } else {
                    let total_delta = codex_total_delta(watermark_baseline, total);
                    if codex_should_prefer_total_delta(
                        watermark_baseline,
                        total,
                        total_delta,
                        last,
                        self.saw_divergent_totals,
                    ) {
                        counted_delta = total_delta;
                    }
                }
                let next = codex_add_totals(base, counted_delta);
                self.counted_totals = Some(next);
                self.raw_totals_baseline = Some(total);
                if !codex_totals_equal(Some(total), Some(next)) {
                    self.saw_divergent_totals = true;
                }
                (counted_delta, next)
            } else {
                let next = codex_add_totals(base, counted_delta);
                self.counted_totals = Some(next);
                self.raw_totals_baseline = Some(next);
                self.tracker.raise_watermark(next);
                (counted_delta, next)
            }
        } else if let Some(total) = total {
            let delta = if self.tracker.saw_interleaved_totals {
                codex_contained_total_delta(watermark_baseline, self.counted_totals, total)
            } else if self.saw_divergent_totals {
                codex_divergent_total_delta(watermark_baseline, self.counted_totals, total)
            } else {
                codex_total_delta(watermark_baseline, total)
            };
            let next = codex_add_totals(base, delta);
            self.counted_totals = Some(next);
            self.raw_totals_baseline = Some(total);
            if !codex_totals_equal(Some(total), Some(next)) {
                self.saw_divergent_totals = true;
            }
            (delta, next)
        } else {
            (CodexTotals::default(), base)
        };

        if let Some(total) = total {
            self.tracker.commit_observed(total);
        }

        result
    }
}

/// Indexed `token_count` snapshot from a Codex session file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexTokenSnapshot {
    pub timestamp: String,
    pub timestamp_unix_ms: Option<i64>,
    pub day: Option<String>,
    pub last: Option<CodexTotals>,
    pub total: Option<CodexTotals>,
    pub end_offset: Option<u64>,
}

/// Subagent rollout shape classification (ported from `CodexSubagentRolloutShape.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodexSubagentCounterSemantics {
    Independent,
    CopiedPrefix,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexSubagentOwnedSuffix {
    pub start_line_index: usize,
    pub first_token_line_index: Option<usize>,
    pub raw_totals_baseline: CodexTotals,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexSubagentOwnedSuffixCandidate {
    pub suffix: CodexSubagentOwnedSuffix,
    pub parent_totals_at_boundary: CodexTotals,
    pub is_locally_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexSubagentRolloutShape {
    pub semantics: CodexSubagentCounterSemantics,
    pub owned_suffix: Option<CodexSubagentOwnedSuffix>,
    pub owned_suffix_candidate: Option<CodexSubagentOwnedSuffixCandidate>,
    pub inferred_parent_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexSubagentObservationKind {
    SessionMetadata {
        session_id: Option<String>,
    },
    TurnContext,
    InterAgentCommunication {
        trigger_turn: bool,
    },
    TokenCount {
        last: Option<CodexTotals>,
        total: Option<CodexTotals>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexSubagentObservation {
    pub line_index: usize,
    pub kind: CodexSubagentObservationKind,
}

impl CodexSubagentObservation {
    #[must_use]
    pub const fn session_metadata(line_index: usize, session_id: Option<String>) -> Self {
        Self {
            line_index,
            kind: CodexSubagentObservationKind::SessionMetadata { session_id },
        }
    }

    #[must_use]
    pub const fn turn_context(line_index: usize) -> Self {
        Self {
            line_index,
            kind: CodexSubagentObservationKind::TurnContext,
        }
    }

    #[must_use]
    pub const fn inter_agent_communication(line_index: usize, trigger_turn: bool) -> Self {
        Self {
            line_index,
            kind: CodexSubagentObservationKind::InterAgentCommunication { trigger_turn },
        }
    }

    #[must_use]
    pub const fn token_count(
        line_index: usize,
        last: Option<CodexTotals>,
        total: Option<CodexTotals>,
    ) -> Self {
        Self {
            line_index,
            kind: CodexSubagentObservationKind::TokenCount { last, total },
        }
    }
}

impl CodexSubagentRolloutShape {
    #[must_use]
    pub fn classify_session_ids(
        leaf_session_id: Option<&str>,
        observed_session_ids: &[Option<String>],
    ) -> Self {
        let normalized_leaf_id = Self::normalized_session_id(leaf_session_id);
        let ancestor_ids: Vec<Option<String>> = observed_session_ids
            .iter()
            .map(|id| Self::normalized_session_id(id.as_deref()))
            .filter(|id| id.as_deref() != normalized_leaf_id.as_deref())
            .collect();
        let has_embedded_ancestor = !ancestor_ids.is_empty()
            || (normalized_leaf_id.is_none() && observed_session_ids.len() > 1);
        let distinct_ancestor_ids: BTreeSet<String> = ancestor_ids.into_iter().flatten().collect();

        Self {
            semantics: if has_embedded_ancestor {
                CodexSubagentCounterSemantics::CopiedPrefix
            } else {
                CodexSubagentCounterSemantics::Independent
            },
            owned_suffix: None,
            owned_suffix_candidate: None,
            inferred_parent_session_id: if distinct_ancestor_ids.len() == 1 {
                distinct_ancestor_ids.into_iter().next()
            } else {
                None
            },
        }
    }

    #[must_use]
    pub fn classify(
        leaf_session_id: Option<&str>,
        observations: &[CodexSubagentObservation],
        has_explicit_parent: bool,
    ) -> Self {
        let metadata_ids: Vec<Option<String>> = observations
            .iter()
            .filter_map(|obs| match &obs.kind {
                CodexSubagentObservationKind::SessionMetadata { session_id } => {
                    Some(session_id.clone())
                }
                _ => None,
            })
            .collect();
        let metadata_shape = Self::classify_session_ids(leaf_session_id, &metadata_ids);
        let can_propose_parent_confirmed_suffix = metadata_shape.semantics
            == CodexSubagentCounterSemantics::Independent
            && has_explicit_parent;
        if metadata_shape.semantics != CodexSubagentCounterSemantics::CopiedPrefix
            && !can_propose_parent_confirmed_suffix
        {
            return metadata_shape;
        }

        let normalized_leaf_id = Self::normalized_session_id(leaf_session_id);
        let mut last_raw_totals: Option<CodexTotals> = None;
        let mut pending_turn_context: Option<(usize, CodexTotals)> = None;
        let mut owned_suffix: Option<CodexSubagentOwnedSuffix> = None;
        let mut parent_totals_at_boundary: Option<CodexTotals> = None;
        let mut locally_confirmed_boundary = false;
        let mut inspected_owned_suffix_first_total = false;
        let mut observed_authoritative_metadata = false;
        let mut observed_turn_context = false;
        let mut inherited_opening = false;

        for observation in observations {
            match &observation.kind {
                CodexSubagentObservationKind::SessionMetadata { session_id } => {
                    let normalized_id = Self::normalized_session_id(session_id.as_deref());
                    let is_embedded_ancestor = if !observed_authoritative_metadata {
                        false
                    } else if let Some(ref leaf_id) = normalized_leaf_id {
                        normalized_id.as_deref() != Some(leaf_id.as_str())
                    } else {
                        true
                    };
                    observed_authoritative_metadata = true;
                    if is_embedded_ancestor {
                        // A later ancestor meta proves that any earlier candidate boundary was replay.
                        owned_suffix = None;
                        parent_totals_at_boundary = None;
                        locally_confirmed_boundary = false;
                        inspected_owned_suffix_first_total = false;
                    }
                    pending_turn_context = None;
                }
                CodexSubagentObservationKind::TurnContext => {
                    let is_first_turn_context = !observed_turn_context;
                    observed_turn_context = true;
                    let accepts_boundary = metadata_shape.semantics
                        == CodexSubagentCounterSemantics::CopiedPrefix
                        || (can_propose_parent_confirmed_suffix && is_first_turn_context);
                    pending_turn_context = if accepts_boundary {
                        last_raw_totals.map(|totals| (observation.line_index, totals))
                    } else {
                        None
                    };
                    if inherited_opening
                        && is_first_turn_context
                        && let Some((line_idx, baseline)) = pending_turn_context
                    {
                        owned_suffix = Some(CodexSubagentOwnedSuffix {
                            start_line_index: line_idx,
                            first_token_line_index: None,
                            raw_totals_baseline: baseline,
                        });
                        inspected_owned_suffix_first_total = false;
                    }
                }
                CodexSubagentObservationKind::InterAgentCommunication { trigger_turn } => {
                    if owned_suffix.is_none()
                        && *trigger_turn
                        && let Some((ptc_line, ptc_baseline)) = pending_turn_context
                        && observation.line_index == ptc_line + 1
                        && (metadata_shape.semantics == CodexSubagentCounterSemantics::CopiedPrefix
                            || Self::totals_contain_usage(ptc_baseline))
                    {
                        owned_suffix = Some(CodexSubagentOwnedSuffix {
                            start_line_index: ptc_line,
                            first_token_line_index: None,
                            raw_totals_baseline: ptc_baseline,
                        });
                        parent_totals_at_boundary = Some(ptc_baseline);
                        locally_confirmed_boundary = false;
                        inspected_owned_suffix_first_total = false;
                    }
                    pending_turn_context = None;
                }
                CodexSubagentObservationKind::TokenCount { last, total } => {
                    if last_raw_totals.is_none()
                        && can_propose_parent_confirmed_suffix
                        && !observed_turn_context
                        && let (Some(total), Some(last)) = (*total, *last)
                        && Self::totals_contain_usage(total)
                        && !Self::totals_contain_usage(last)
                    {
                        // A zero-component opening event carries inherited context, not child usage.
                        inherited_opening = true;
                        owned_suffix = Some(CodexSubagentOwnedSuffix {
                            start_line_index: observation.line_index,
                            first_token_line_index: None,
                            raw_totals_baseline: total,
                        });
                        parent_totals_at_boundary = Some(total);
                        locally_confirmed_boundary = true;
                    }
                    if inherited_opening
                        && !observed_turn_context
                        && let (Some(total), Some(last)) = (*total, *last)
                        && Self::totals_contain_usage(last)
                        && !codex_totals_equal(Some(total), last_raw_totals)
                    {
                        inherited_opening = false;
                        owned_suffix = Some(CodexSubagentOwnedSuffix {
                            start_line_index: observation.line_index,
                            first_token_line_index: None,
                            raw_totals_baseline: last_raw_totals.unwrap_or(total),
                        });
                        inspected_owned_suffix_first_total = false;
                    }
                    if !inspected_owned_suffix_first_total
                        && let Some(suffix) = owned_suffix.clone()
                        && let Some(total) = *total
                        && !codex_totals_equal(Some(total), Some(suffix.raw_totals_baseline))
                    {
                        inspected_owned_suffix_first_total = true;
                        if let Some(last) = *last {
                            let copied_snapshot =
                                Self::totals_contain_usage(suffix.raw_totals_baseline)
                                    && codex_totals_equal(Some(total), Some(last))
                                    && codex_totals_at_least(total, suffix.raw_totals_baseline);
                            owned_suffix = Some(CodexSubagentOwnedSuffix {
                                start_line_index: suffix.start_line_index,
                                raw_totals_baseline: if copied_snapshot {
                                    total
                                } else {
                                    codex_total_delta(Some(last), total)
                                },
                                first_token_line_index: Some(observation.line_index),
                            });
                            inspected_owned_suffix_first_total = !copied_snapshot;
                            locally_confirmed_boundary = true;
                        }
                    }
                    if let Some(total) = *total {
                        last_raw_totals = Some(total);
                    }
                    pending_turn_context = None;
                }
            }
        }

        if metadata_shape.semantics == CodexSubagentCounterSemantics::CopiedPrefix {
            return Self {
                semantics: CodexSubagentCounterSemantics::CopiedPrefix,
                owned_suffix,
                owned_suffix_candidate: None,
                inferred_parent_session_id: metadata_shape.inferred_parent_session_id,
            };
        }

        let candidate = match (owned_suffix, parent_totals_at_boundary) {
            (Some(suffix), Some(parent_totals_at_boundary)) => {
                Some(CodexSubagentOwnedSuffixCandidate {
                    suffix,
                    parent_totals_at_boundary,
                    is_locally_confirmed: locally_confirmed_boundary,
                })
            }
            _ => None,
        };
        Self {
            semantics: CodexSubagentCounterSemantics::Independent,
            owned_suffix: None,
            owned_suffix_candidate: candidate,
            inferred_parent_session_id: metadata_shape.inferred_parent_session_id,
        }
    }

    #[must_use]
    pub fn same_concrete_session_id(lhs: Option<&str>, rhs: Option<&str>) -> bool {
        let Some(lhs) = Self::normalized_session_id(lhs) else {
            return false;
        };
        let Some(rhs) = Self::normalized_session_id(rhs) else {
            return false;
        };
        lhs == rhs
    }

    #[must_use]
    pub fn totals_contain_usage(totals: CodexTotals) -> bool {
        totals.input > 0 || totals.cached > 0 || totals.output > 0
    }

    fn normalized_session_id(session_id: Option<&str>) -> Option<String> {
        let trimmed = session_id?.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }
}

pub type CodexSubagentClassification = CodexSubagentRolloutShape;

/// Session metadata parsed from `session_meta`.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexSessionMetadata {
    pub session_id: String,
    pub forked_from_id: Option<String>,
    pub fork_timestamp: Option<String>,
    pub is_subagent_thread: bool,
    pub subagent_history_start_ordinal: Option<i64>,
    pub history_base_thread_id: Option<String>,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub started_at_unix_ms: Option<i64>,
    pub latest_activity_unix_ms: Option<i64>,
}

impl std::fmt::Debug for CodexSessionMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexSessionMetadata")
            .field("session_id", &self.session_id)
            .field("forked_from_id", &self.forked_from_id)
            .field("fork_timestamp", &self.fork_timestamp)
            .field("is_subagent_thread", &self.is_subagent_thread)
            .field(
                "subagent_history_start_ordinal",
                &self.subagent_history_start_ordinal,
            )
            .field("history_base_thread_id", &self.history_base_thread_id)
            .field("cwd", &self.cwd.as_ref().map(|_| "[redacted]"))
            .field("title", &self.title.as_ref().map(|_| "[redacted]"))
            .field("started_at_unix_ms", &self.started_at_unix_ms)
            .field("latest_activity_unix_ms", &self.latest_activity_unix_ms)
            .finish()
    }
}

/// Single billed Codex usage row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexUsageRow {
    pub day: String,
    pub model: String,
    pub raw_model: Option<String>,
    pub turn_id: Option<String>,
    pub event_index: Option<usize>,
    pub timestamp_unix_ms: Option<i64>,
    pub input: i64,
    pub cached: i64,
    pub output: i64,
    pub reasoning: Option<i64>,
    pub response_id: Option<String>,
    pub request_mirror_keys: Option<Vec<String>>,
    pub known_cost_nanos: Option<i64>,
    pub unpriced_tokens: Option<i64>,
    pub pricing_model: Option<String>,
    pub pricing_mode: Option<String>,
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_identity: Option<String>,
}

impl CodexUsageRow {
    #[must_use]
    pub fn with_clamped_reasoning(mut self) -> Self {
        self.reasoning = self.reasoning.map(|r| r.max(0).min(self.output.max(0)));
        self
    }
}

/// Day + normalized model key (`CostUsageScanner.CodexDayModelKey`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CodexDayModelKey {
    pub day: String,
    pub model: String,
}

impl CodexDayModelKey {
    #[must_use]
    pub fn new(day: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            day: day.into(),
            model: model.into(),
        }
    }
}

/// Result of reconciling persisted Codex rows against per-day/model canonical token totals
/// (`CostUsageScanner.CodexCanonicalPricingRows`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodexCanonicalPricingRows {
    pub rows: Vec<CodexUsageRow>,
    pub unresolved_groups: BTreeSet<CodexDayModelKey>,
}

/// Reconciles persisted `rows` against canonical `days` token totals (`CostUsageScanner.codexCanonicalPricingRows`).
///
/// Rejects groups where row token sums do not exactly equal the canonical day/model target, or where
/// leading zero-token rows carry non-zero `known_cost_nanos` before the first token-bearing row.
#[must_use]
pub fn codex_canonical_pricing_rows(
    days: &BTreeMap<String, BTreeMap<String, Vec<i64>>>,
    persisted_rows: &[CodexUsageRow],
) -> CodexCanonicalPricingRows {
    let mut rows_by_group: BTreeMap<CodexDayModelKey, Vec<CodexUsageRow>> = BTreeMap::new();
    for row in persisted_rows {
        rows_by_group
            .entry(CodexDayModelKey::new(&row.day, &row.model))
            .or_default()
            .push(row.clone());
    }

    let mut canonical_rows = Vec::new();
    let mut unresolved_groups = BTreeSet::new();

    for (day, models) in days {
        for (model, packed) in models {
            let key = CodexDayModelKey::new(day, model);
            let target_in = packed.first().copied().unwrap_or(0).max(0);
            let target_cached = packed.get(1).copied().unwrap_or(0).max(0);
            let target_out = packed.get(2).copied().unwrap_or(0).max(0);
            let group_rows = rows_by_group.get(&key).map(Vec::as_slice).unwrap_or(&[]);

            if let Some(reconciled) =
                reconciled_codex_pricing_rows(group_rows, (target_in, target_cached, target_out))
            {
                canonical_rows.extend(reconciled);
            } else {
                unresolved_groups.insert(key);
            }
        }
    }

    CodexCanonicalPricingRows {
        rows: canonical_rows,
        unresolved_groups,
    }
}

fn reconciled_codex_pricing_rows(
    rows: &[CodexUsageRow],
    target: (i64, i64, i64),
) -> Option<Vec<CodexUsageRow>> {
    let mut sum_in: i64 = 0;
    let mut sum_cached: i64 = 0;
    let mut sum_out: i64 = 0;
    for row in rows {
        sum_in = sum_in.checked_add(row.input.max(0))?;
        sum_cached = sum_cached.checked_add(row.cached.max(0))?;
        sum_out = sum_out.checked_add(row.output.max(0))?;
    }
    if target == (0, 0, 0) {
        return Some(Vec::new());
    }
    if (sum_in, sum_cached, sum_out) == target {
        let first_token_row = rows
            .iter()
            .position(|r| r.input > 0 || r.cached > 0 || r.output > 0);
        if let Some(idx) = first_token_row
            && rows[..idx]
                .iter()
                .any(|r| r.known_cost_nanos.unwrap_or(0) != 0)
        {
            return None;
        }
        return Some(rows.to_vec());
    }
    None
}

/// Per-file parse result for Codex JSONL session logs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexParseResult {
    /// Day -> NormalizedModel -> `[input, cached, output, cost_nanos, standard_tokens, priority_tokens, standard_in, standard_cached, standard_out, priority_in, priority_cached, priority_out, unpriced_tokens, requests]`
    pub days: BTreeMap<String, BTreeMap<String, Vec<i64>>>,
    pub rows: Vec<CodexUsageRow>,
    pub token_snapshots: Vec<CodexTokenSnapshot>,
    pub parsed_bytes: u64,
    pub last_model: Option<String>,
    pub last_totals: Option<CodexTotals>,
    pub last_codex_turn_id: Option<String>,
    pub session_id: Option<String>,
    pub forked_from_id: Option<String>,
    pub fork_timestamp: Option<String>,
    pub depends_on_parent_totals: bool,
    pub has_unresolved_fork_baseline: bool,
    pub codex_session: Option<CodexSessionMetadata>,
}

/// Resolution result for a fork parent's cumulative baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexForkBaseline {
    Resolved(Option<CodexTotals>),
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NearMirror {
    turn_id: Option<String>,
    timestamp_unix_ms: Option<i64>,
    usage: CodexTotals,
    counter_advanced_strictly_more_than_usage: bool,
}

impl NearMirror {
    fn matches(&self, other: &Self, allow_delayed: bool) -> bool {
        if self.usage.input != other.usage.input
            || self.usage.cached != other.usage.cached
            || self.usage.output != other.usage.output
        {
            return false;
        }
        if self.counter_advanced_strictly_more_than_usage
            || other.counter_advanced_strictly_more_than_usage
        {
            return false;
        }
        if let (Some(t1), Some(t2)) = (&self.turn_id, &other.turn_id)
            && t1 != t2
        {
            return false;
        }
        if allow_delayed {
            match (self.timestamp_unix_ms, other.timestamp_unix_ms) {
                (Some(ms1), Some(ms2)) => (ms1 - ms2).abs() <= NEAR_MIRROR_MAX_DELAY_MS,
                _ => true,
            }
        } else {
            true
        }
    }
}

#[derive(Debug, Default)]
struct CodexRequestLedgerState {
    response_ids: BTreeSet<String>,
    mirrored_snapshots: BTreeMap<String, String>,
    pending_ledger_mirrors: Option<BTreeSet<String>>,
    pending_ledger_response_id: Option<String>,
    pending_ledger_near: Option<NearMirror>,
    pending_legacy_mirrors: Option<BTreeSet<String>>,
    pending_legacy_row_index: Option<usize>,
    pending_legacy_near: Option<NearMirror>,
    last_ledger_total: Option<CodexTotals>,
    last_legacy_total: Option<CodexTotals>,
}

impl CodexRequestLedgerState {
    fn clear_pending(&mut self) {
        self.pending_ledger_mirrors = None;
        self.pending_ledger_response_id = None;
        self.pending_ledger_near = None;
        self.pending_legacy_mirrors = None;
        self.pending_legacy_row_index = None;
        self.pending_legacy_near = None;
    }
}

/// Maximum line bytes for normal JSONL line scanning in `parse_codex_file` (`256 KiB`).
pub const CODEX_DEFAULT_MAX_LINE_BYTES: usize = 256 * 1024;

/// Parses the leaf `session_meta` identifier from a Codex JSONL file, skipping lines larger than
/// `CODEX_SESSION_METADATA_MAX_LINE_BYTES` (`8 MiB`).
pub fn parse_codex_session_identifier(file_path: &Path) -> std::io::Result<Option<String>> {
    let file = crate::cost::claude::open_regular_file_nofollow(file_path)?;
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let (bytes, truncated) = read_bounded_line_with_prefix(
            &mut reader,
            &mut buf,
            CODEX_SESSION_METADATA_MAX_LINE_BYTES,
        )?;
        if bytes == 0 {
            break;
        }
        if truncated {
            continue;
        }
        let Some(line_bytes) = buf.as_slice().strip_suffix(b"\n").or(Some(buf.as_slice())) else {
            continue;
        };
        let line_bytes = line_bytes.strip_suffix(b"\r").unwrap_or(line_bytes);
        let Ok(text) = std::str::from_utf8(line_bytes) else {
            continue;
        };
        if !text.contains("\"session_meta\"") {
            continue;
        }
        let Ok(val) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        if val.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        if let Some(id) = val
            .get("payload")
            .and_then(|p| p.get("id").or_else(|| p.get("session_id")))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            return Ok(Some(id.to_owned()));
        }
    }
    Ok(None)
}

/// Reads a single line up to `max_bytes`, keeping the first `max_bytes` bytes in `out` when
/// truncated and returning `(total_bytes_consumed, was_truncated)`.
fn read_bounded_line_with_prefix<R: BufRead>(
    reader: &mut R,
    out: &mut Vec<u8>,
    max_bytes: usize,
) -> std::io::Result<(usize, bool)> {
    let mut total_read = 0usize;
    let mut truncated = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok((total_read, truncated));
        }
        if let Some(newline_pos) = available.iter().position(|&b| b == b'\n') {
            let chunk_len = newline_pos + 1;
            let content_len = newline_pos;
            if out.len() < max_bytes {
                let keep = (max_bytes - out.len()).min(content_len);
                out.extend_from_slice(&available[..keep]);
            }
            if total_read + content_len > max_bytes {
                truncated = true;
            }
            reader.consume(chunk_len);
            total_read += chunk_len;
            return Ok((total_read, truncated));
        }
        let chunk_len = available.len();
        if out.len() < max_bytes {
            let keep = (max_bytes - out.len()).min(chunk_len);
            out.extend_from_slice(&available[..keep]);
        }
        if total_read + chunk_len > max_bytes {
            truncated = true;
        }
        reader.consume(chunk_len);
        total_read += chunk_len;
    }
}

/// File metadata for a Codex session log (`CostUsageScanner.CodexFileMetadata`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexFileMetadata {
    pub mtime_unix_ms: i64,
    pub size: u64,
    pub file_id: Option<String>,
}

/// Reads file metadata (`mtime_unix_ms`, `size`, and `file_id` = `dev:ino` on Unix)
/// without following symlinks on the target path.
#[must_use]
pub fn codex_file_metadata(file_path: &Path) -> CodexFileMetadata {
    let Ok(meta) = fs::symlink_metadata(file_path) else {
        return CodexFileMetadata {
            mtime_unix_ms: 0,
            size: 0,
            file_id: None,
        };
    };
    if meta.file_type().is_symlink() {
        return CodexFileMetadata {
            mtime_unix_ms: 0,
            size: 0,
            file_id: None,
        };
    }
    let mtime_unix_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));
    let size = meta.len();
    #[cfg(unix)]
    let file_id = {
        use std::os::unix::fs::MetadataExt;
        Some(format!("{}:{}", meta.dev(), meta.ino()))
    };
    #[cfg(not(unix))]
    let file_id = Some(file_path.to_string_lossy().into_owned());
    CodexFileMetadata {
        mtime_unix_ms,
        size,
        file_id,
    }
}

/// Parses a single Codex JSONL session file.
///
/// Mirrors `CostUsageScanner.parseCodexFile`.
#[allow(clippy::too_many_arguments)]
pub fn parse_codex_file<F>(
    file_path: &Path,
    range: &DayRange,
    start_offset: u64,
    initial_model: Option<&str>,
    initial_totals: Option<CodexTotals>,
    initial_turn_id: Option<&str>,
    priority_turns: &BTreeMap<String, CodexPriorityTurnMetadata>,
    session_tiers: &BTreeMap<String, CodexServiceTier>,
    catalog: Option<&ModelsDevCatalog>,
    mut inherited_totals_resolver: F,
) -> CodexParseResult
where
    F: FnMut(&str, Option<&str>) -> CodexForkBaseline,
{
    let file_identity = codex_file_metadata(file_path)
        .file_id
        .unwrap_or_else(|| file_path.to_string_lossy().into_owned());
    let Ok(mut file) = crate::cost::claude::open_regular_file_nofollow(file_path) else {
        return CodexParseResult::default();
    };
    if start_offset > 0 && file.seek(SeekFrom::Start(start_offset)).is_err() {
        return CodexParseResult::default();
    }

    let mut reader = BufReader::new(file);
    let mut offset = start_offset;
    let mut raw_lines: Vec<(u64, String, bool)> = Vec::new();
    let mut byte_buf: Vec<u8> = Vec::with_capacity(4096);

    loop {
        byte_buf.clear();
        let Ok((bytes_read, was_truncated)) =
            read_bounded_line_with_prefix(&mut reader, &mut byte_buf, CODEX_DEFAULT_MAX_LINE_BYTES)
        else {
            break;
        };
        if bytes_read == 0 {
            break;
        }
        offset += bytes_read as u64;
        let trimmed_bytes = byte_buf.strip_suffix(b"\r").unwrap_or(byte_buf.as_slice());
        if trimmed_bytes.is_empty() && !was_truncated {
            continue;
        }
        let text = String::from_utf8_lossy(trimmed_bytes).into_owned();
        raw_lines.push((offset, text, was_truncated));
    }

    // First pass: inspect leaf `session_meta` (and enrich from same-leaf `session_meta` lines)
    let mut session_meta: Option<CodexSessionMetadata> = None;
    let mut did_capture_leaf_metadata = false;
    let mut parsed_lines: Vec<PredecodedCodexLine> = Vec::with_capacity(raw_lines.len());

    for (idx, (end_offset, raw, was_truncated)) in raw_lines.into_iter().enumerate() {
        let val = if was_truncated {
            None
        } else {
            parse_codex_line_json(&raw)
        };
        let ordinal = val
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|o| o.get("ordinal"))
            .and_then(Value::as_i64);

        if let Some(ref v) = val
            && v.get("type").and_then(Value::as_str) == Some("session_meta")
            && let Some(meta) = extract_session_metadata(v)
        {
            if did_capture_leaf_metadata {
                if let Some(ref mut leaf) = session_meta
                    && CodexSubagentRolloutShape::same_concrete_session_id(
                        Some(&leaf.session_id),
                        Some(&meta.session_id),
                    )
                {
                    enrich_same_leaf_metadata(leaf, &meta);
                }
            } else {
                did_capture_leaf_metadata = true;
                session_meta = Some(meta);
            }
        }
        parsed_lines.push((idx, end_offset, ordinal, was_truncated, raw, val));
    }

    let mut subagent_counter_semantics: Option<CodexSubagentCounterSemantics> = None;
    let mut owned_suffix: Option<CodexSubagentOwnedSuffix> = None;
    let mut uses_local_subagent_boundary = false;
    let mut candidate_boundary_depends_on_parent_totals = false;
    let mut suppress_unowned_copied_prefix = false;
    let mut cached_parent_resolution: Option<CodexForkBaseline> = None;

    if let Some(ref mut meta) = session_meta
        && meta.is_subagent_thread
    {
        let observations = build_subagent_observations(&parsed_lines);
        let leaf_id_opt = Some(meta.session_id.as_str()).filter(|s| !s.is_empty());
        let shape = CodexSubagentRolloutShape::classify(
            leaf_id_opt,
            &observations,
            meta.forked_from_id.is_some(),
        );
        subagent_counter_semantics = Some(shape.semantics);
        if meta.forked_from_id.is_none()
            && let Some(ref inferred) = shape.inferred_parent_session_id
        {
            meta.forked_from_id = Some(inferred.clone());
        }

        let explicit_start_ordinal = meta.subagent_history_start_ordinal.filter(|&o| o >= 0);
        let explicit_owned_suffix = explicit_start_ordinal
            .and_then(|start_ord| compute_explicit_ordinal_suffix(&parsed_lines, start_ord));

        let has_explicit_boundary = explicit_start_ordinal.is_some();
        owned_suffix = if has_explicit_boundary {
            explicit_owned_suffix
        } else {
            shape.owned_suffix
        };

        if has_explicit_boundary {
            subagent_counter_semantics = Some(CodexSubagentCounterSemantics::CopiedPrefix);
        } else if let Some(cand) = shape.owned_suffix_candidate {
            if cand.is_locally_confirmed {
                subagent_counter_semantics = Some(CodexSubagentCounterSemantics::CopiedPrefix);
                owned_suffix = Some(cand.suffix);
            } else if let Some(ref parent_session_id) = meta.forked_from_id {
                candidate_boundary_depends_on_parent_totals = true;
                let res =
                    inherited_totals_resolver(parent_session_id, meta.fork_timestamp.as_deref());
                if let CodexForkBaseline::Resolved(parent_totals) = &res
                    && codex_totals_equal(*parent_totals, Some(cand.parent_totals_at_boundary))
                {
                    subagent_counter_semantics = Some(CodexSubagentCounterSemantics::CopiedPrefix);
                    owned_suffix = Some(cand.suffix);
                }
                cached_parent_resolution = Some(res);
            }
        }

        uses_local_subagent_boundary = has_explicit_boundary || owned_suffix.is_some();
        suppress_unowned_copied_prefix = subagent_counter_semantics
            == Some(CodexSubagentCounterSemantics::CopiedPrefix)
            && owned_suffix.is_none()
            && (has_explicit_boundary || meta.forked_from_id.is_none());
    }

    let replay_start_line_index = owned_suffix.as_ref().map_or(0, |s| s.start_line_index);
    let skip_token_lines_before = owned_suffix
        .as_ref()
        .and_then(|s| s.first_token_line_index)
        .unwrap_or(0);
    let subagent_raw_baseline = owned_suffix.as_ref().map(|s| s.raw_totals_baseline);

    let mut inherited_totals: Option<CodexTotals> = None;
    let mut has_unresolved_fork_baseline = false;

    if let Some(ref meta) = session_meta
        && let Some(ref parent_id) = meta.forked_from_id
        && subagent_counter_semantics != Some(CodexSubagentCounterSemantics::Independent)
        && !uses_local_subagent_boundary
    {
        let res = cached_parent_resolution.unwrap_or_else(|| {
            inherited_totals_resolver(parent_id, meta.fork_timestamp.as_deref())
        });
        match res {
            CodexForkBaseline::Resolved(resolved) => {
                inherited_totals = resolved;
                has_unresolved_fork_baseline = false;
            }
            CodexForkBaseline::Unresolved => {
                has_unresolved_fork_baseline = true;
            }
        }
    }

    let depends_on_parent_totals = session_meta
        .as_ref()
        .and_then(|m| m.forked_from_id.as_ref())
        .is_some()
        && (candidate_boundary_depends_on_parent_totals
            || (subagent_counter_semantics != Some(CodexSubagentCounterSemantics::Independent)
                && !uses_local_subagent_boundary));

    let is_forked_session = session_meta
        .as_ref()
        .and_then(|m| m.forked_from_id.as_ref())
        .is_some();

    let mut current_model: Option<String> = if owned_suffix.is_some() {
        None
    } else {
        initial_model.map(String::from)
    };
    let mut current_turn_id: Option<String> = if owned_suffix.is_some() {
        None
    } else {
        initial_turn_id.map(String::from)
    };
    let mut current_service_tier: Option<CodexServiceTier> = session_meta
        .as_ref()
        .and_then(|m| session_tiers.get(&m.session_id).copied());
    let mut thread_priority: Option<bool> = None;
    let mut priority_turn_ids: BTreeSet<String> = BTreeSet::new();
    let mut last_accepted_day_key: Option<DayKey> = None;
    let mut last_accepted_ts_ms: Option<i64> = None;

    let initial_raw_baseline = subagent_raw_baseline.or(initial_totals);
    let mut accumulator = CodexSnapshotAccumulator {
        counted_totals: if owned_suffix.is_some() {
            None
        } else {
            initial_totals
        },
        raw_totals_baseline: initial_raw_baseline,
        saw_divergent_totals: false,
        tracker: CodexTotalsTracker {
            watermark: initial_raw_baseline,
            seen_raw_totals: Vec::new(),
            saw_interleaved_totals: false,
        },
    };
    let mut raised_continued_counter = false;
    let mut remaining_inherited_totals: Option<CodexTotals> = inherited_totals;
    let mut fork_raw_baseline: Option<CodexTotals> = initial_raw_baseline;
    let mut fork_counted_totals: Option<CodexTotals> = if owned_suffix.is_some() {
        None
    } else {
        initial_totals
    };
    let mut fork_saw_divergent = false;

    let mut days: BTreeMap<String, BTreeMap<String, Vec<i64>>> = BTreeMap::new();
    let mut rows: Vec<CodexUsageRow> = Vec::new();
    let mut token_snapshots: Vec<CodexTokenSnapshot> = Vec::new();
    let mut ledger = CodexRequestLedgerState::default();
    let mut event_index = 0usize;

    for (line_idx, end_offset, _ordinal, was_truncated, raw, val_opt) in parsed_lines {
        if !was_truncated
            && let Some(ref val) = val_opt
            && let Some(obj) = val.as_object()
            && obj.get("type").and_then(Value::as_str) == Some("event_msg")
            && let Some(payload) = obj.get("payload").and_then(Value::as_object)
            && payload.get("type").and_then(Value::as_str) == Some("token_count")
            && let Some(info) = payload.get("info").and_then(Value::as_object)
        {
            let last_usage = info
                .get("last_token_usage")
                .and_then(Value::as_object)
                .and_then(parse_codex_totals_map);
            let total_usage = info
                .get("total_token_usage")
                .and_then(Value::as_object)
                .and_then(parse_codex_totals_map);
            if last_usage.is_some() || total_usage.is_some() {
                let snap_ts_str = obj.get("timestamp").and_then(Value::as_str);
                let snap_day_and_ms = snap_ts_str
                    .and_then(|ts| timestamp::day_key_from_timestamp(ts, range.timezone.clone()));
                if snap_day_and_ms.is_some() {
                    token_snapshots.push(CodexTokenSnapshot {
                        timestamp: snap_ts_str.unwrap_or_default().to_owned(),
                        timestamp_unix_ms: snap_day_and_ms.as_ref().map(|(_, ms)| *ms),
                        day: snap_day_and_ms
                            .as_ref()
                            .map(|(dk, _)| dk.as_str().to_owned()),
                        last: last_usage,
                        total: total_usage,
                        end_offset: Some(end_offset),
                    });
                }
            }
        }

        if line_idx < replay_start_line_index {
            continue;
        }
        if was_truncated {
            if let Some(m) = extract_truncated_turn_context_model(&raw) {
                current_model = Some(m);
            }
            continue;
        }
        let Some(val) = val_opt else {
            continue;
        };
        let Some(obj) = val.as_object() else {
            continue;
        };

        let line_ts_str = obj.get("timestamp").and_then(Value::as_str);
        let line_day_and_ms = line_ts_str
            .and_then(|ts| timestamp::day_key_from_timestamp(ts, range.timezone.clone()));

        let record_type = obj.get("type").and_then(Value::as_str);
        if matches!(
            record_type,
            Some("turn_context" | "inter_agent_communication_metadata")
        ) && line_day_and_ms.is_none()
        {
            // Upstream `requiresValidTimestamp`: `turn_context` and `inter_agent_communication_metadata`
            // without a valid root timestamp are rejected before mutating parser state.
            continue;
        }

        if let Some((ref dk, ms)) = line_day_and_ms {
            last_accepted_day_key = Some(dk.clone());
            last_accepted_ts_ms = Some(ms);
            if let Some(ref mut meta) = session_meta {
                meta.latest_activity_unix_ms =
                    Some(meta.latest_activity_unix_ms.map_or(ms, |prev| prev.max(ms)));
                if meta.started_at_unix_ms.is_none() {
                    meta.started_at_unix_ms = Some(ms);
                }
            }
        }

        match record_type {
            Some("session_meta") => {
                if let Some(payload) = obj.get("payload").and_then(Value::as_object)
                    && let Some(tier) = pricing::normalize_codex_service_tier(
                        payload.get("service_tier").and_then(Value::as_str),
                    )
                {
                    current_service_tier = Some(tier);
                }
            }
            Some("turn_context") => {
                if let Some(payload) = obj.get("payload").and_then(Value::as_object) {
                    if let Some(m) = extract_model_from_map(payload) {
                        current_model = Some(m);
                    }
                    if let Some(tid) = payload
                        .get("turn_id")
                        .or_else(|| payload.get("id"))
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        current_turn_id = Some(tid.to_owned());
                    }
                    if let Some(tier) = pricing::normalize_codex_service_tier(
                        payload.get("service_tier").and_then(Value::as_str),
                    ) {
                        current_service_tier = Some(tier);
                    }
                }
            }
            Some("token_usage_record") => {
                if suppress_unowned_copied_prefix || has_unresolved_fork_baseline {
                    continue;
                }
                process_token_usage_record(
                    obj,
                    line_day_and_ms.as_ref(),
                    last_accepted_day_key.as_ref(),
                    last_accepted_ts_ms,
                    session_meta.as_ref(),
                    &file_identity,
                    current_model.as_deref(),
                    current_turn_id.as_deref(),
                    current_service_tier,
                    &priority_turn_ids,
                    priority_turns,
                    catalog,
                    &mut ledger,
                    &mut days,
                    &mut rows,
                );
            }
            Some("event_msg") => {
                let Some(payload) = obj.get("payload").and_then(Value::as_object) else {
                    continue;
                };
                let payload_type = payload.get("type").and_then(Value::as_str);
                if payload_type == Some("thread_settings_applied") {
                    let tier_raw = payload
                        .get("thread_settings")
                        .and_then(Value::as_object)
                        .and_then(|ts| ts.get("service_tier"))
                        .or_else(|| payload.get("service_tier"))
                        .and_then(Value::as_str);
                    let is_prio = matches!(
                        pricing::normalize_codex_service_tier(tier_raw),
                        Some(CodexServiceTier::Priority | CodexServiceTier::Fast)
                    );
                    thread_priority = Some(is_prio);
                    continue;
                }
                if payload_type == Some("task_started") || payload_type == Some("turn_started") {
                    if let Some(tid) = payload
                        .get("turn_id")
                        .or_else(|| payload.get("id"))
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        current_turn_id = Some(tid.to_owned());
                        if let Some(is_prio) = thread_priority {
                            if is_prio {
                                priority_turn_ids.insert(tid.to_owned());
                            } else {
                                priority_turn_ids.remove(tid);
                            }
                        }
                    }
                    continue;
                }
                if payload_type == Some("token_usage_record") {
                    if suppress_unowned_copied_prefix || has_unresolved_fork_baseline {
                        continue;
                    }
                    process_token_usage_record(
                        payload,
                        line_day_and_ms.as_ref(),
                        last_accepted_day_key.as_ref(),
                        last_accepted_ts_ms,
                        session_meta.as_ref(),
                        &file_identity,
                        current_model.as_deref(),
                        current_turn_id.as_deref(),
                        current_service_tier,
                        &priority_turn_ids,
                        priority_turns,
                        catalog,
                        &mut ledger,
                        &mut days,
                        &mut rows,
                    );
                    continue;
                }
                if payload_type != Some("token_count") {
                    continue;
                }
                if line_idx < skip_token_lines_before {
                    continue;
                }

                let Some(info) = payload.get("info").and_then(Value::as_object) else {
                    continue;
                };
                let record_model = extract_model_from_map(info)
                    .or_else(|| extract_model_from_map(payload))
                    .or_else(|| extract_model_from_map(obj));
                if let Some(tier) = pricing::normalize_codex_service_tier(
                    info.get("service_tier")
                        .or_else(|| payload.get("service_tier"))
                        .and_then(Value::as_str),
                ) {
                    current_service_tier = Some(tier);
                }
                let turn_id = info
                    .get("turn_id")
                    .or_else(|| payload.get("turn_id"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .or_else(|| current_turn_id.clone());

                let last_usage = info
                    .get("last_token_usage")
                    .and_then(Value::as_object)
                    .and_then(parse_codex_totals_map);
                let total_usage = info
                    .get("total_token_usage")
                    .and_then(Value::as_object)
                    .and_then(parse_codex_totals_map);

                if last_usage.is_none() && total_usage.is_none() {
                    continue;
                }

                let this_event_index = event_index;
                event_index += 1;

                if suppress_unowned_copied_prefix || has_unresolved_fork_baseline {
                    continue;
                }

                // Continued counter adjustment when history_base.thread_id != forked_from_id
                if is_forked_session
                    && !raised_continued_counter
                    && let Some(ref meta) = session_meta
                {
                    raised_continued_counter = true;
                    if let (Some(hb_id), Some(fork_id)) = (
                        meta.history_base_thread_id.as_deref(),
                        meta.forked_from_id.as_deref(),
                    ) && !CodexSubagentRolloutShape::same_concrete_session_id(
                        Some(hb_id),
                        Some(fork_id),
                    ) && let (Some(total), Some(last)) = (total_usage, last_usage)
                        && codex_totals_at_least(total, last)
                    {
                        let origin = codex_total_delta(Some(last), total);
                        let current_inherited = inherited_totals.unwrap_or_default();
                        if !origin.is_zero()
                            && codex_totals_at_least(origin, current_inherited)
                            && !codex_totals_equal(Some(origin), Some(current_inherited))
                        {
                            inherited_totals = Some(origin);
                            remaining_inherited_totals = Some(origin);
                        }
                    }
                }

                let delta = if is_forked_session {
                    compute_forked_event_delta(
                        last_usage,
                        total_usage,
                        inherited_totals,
                        &mut remaining_inherited_totals,
                        &mut accumulator.tracker,
                        &mut fork_raw_baseline,
                        &mut fork_counted_totals,
                        &mut fork_saw_divergent,
                    )
                } else {
                    let (d, _) = accumulator.apply_event(last_usage, total_usage, false);
                    d
                };

                if delta.is_zero() {
                    continue;
                }

                let Some(day_key) = line_day_and_ms
                    .as_ref()
                    .map(|(dk, _)| dk.clone())
                    .or_else(|| last_accepted_day_key.clone())
                else {
                    continue;
                };
                let ts_ms = line_day_and_ms
                    .as_ref()
                    .map(|(_, ms)| *ms)
                    .or(last_accepted_ts_ms);

                // Check request ledger mirror deduplication
                let snapshot_key = make_legacy_snapshot_key(
                    turn_id.as_deref(),
                    total_usage,
                    delta,
                    this_event_index,
                );
                let mirror_keys = make_mirror_keys(turn_id.as_deref(), total_usage, delta);
                let prev_legacy_total = ledger.last_legacy_total;
                if let Some(total) = total_usage {
                    ledger.last_legacy_total = Some(total);
                }
                let counter_advanced_more = match (prev_legacy_total, total_usage) {
                    (Some(prev), Some(cur)) => {
                        let diff = codex_total_delta(Some(prev), cur);
                        diff.input > delta.input
                            || diff.cached > delta.cached
                            || diff.output > delta.output
                    }
                    _ => false,
                };
                let near = NearMirror {
                    turn_id: turn_id.clone(),
                    timestamp_unix_ms: ts_ms,
                    usage: delta,
                    counter_advanced_strictly_more_than_usage: counter_advanced_more,
                };

                if is_matched_by_pending_ledger(&mut ledger, &snapshot_key, &mirror_keys, &near) {
                    continue;
                }

                let raw_model = current_model
                    .as_deref()
                    .or(record_model.as_deref())
                    .map(String::from)
                    .unwrap_or_else(|| {
                        resolve_codex_turn_raw_model(None, turn_id.as_deref(), priority_turns)
                    });
                let normalized_model = pricing::normalize_codex_model(&raw_model);
                let pricing_model = resolve_codex_priority_pricing_model(
                    &normalized_model,
                    turn_id.as_deref(),
                    priority_turns,
                );
                let session_id_opt = session_meta
                    .as_ref()
                    .map(|m| m.session_id.as_str())
                    .filter(|s| !s.is_empty());
                let is_fast = is_codex_turn_priority(
                    current_service_tier,
                    turn_id.as_deref(),
                    session_id_opt,
                    &priority_turn_ids,
                    priority_turns,
                );

                let row_idx = rows.len();
                ledger.pending_legacy_mirrors = Some(mirror_keys.iter().cloned().collect());
                ledger.pending_legacy_row_index = Some(row_idx);
                ledger.pending_legacy_near = Some(near);

                record_codex_row(
                    day_key.as_str(),
                    &normalized_model,
                    &pricing_model,
                    Some(&raw_model),
                    turn_id.as_deref(),
                    Some(this_event_index),
                    ts_ms,
                    delta,
                    is_fast,
                    None,
                    Some(mirror_keys),
                    session_id_opt,
                    Some(&file_identity),
                    catalog,
                    &mut days,
                    &mut rows,
                );
            }
            _ => {
                // Check for bare usage row (`usage`, `data.usage`, `response.usage`, `result.usage`)
                if suppress_unowned_copied_prefix || has_unresolved_fork_baseline {
                    continue;
                }
                if let Some((bare_delta, bare_model)) = extract_bare_usage_row(obj) {
                    if bare_delta.is_zero() {
                        continue;
                    }
                    let Some(day_key) = line_day_and_ms
                        .as_ref()
                        .map(|(dk, _)| dk.clone())
                        .or_else(|| last_accepted_day_key.clone())
                    else {
                        continue;
                    };
                    let ts_ms = line_day_and_ms
                        .as_ref()
                        .map(|(_, ms)| *ms)
                        .or(last_accepted_ts_ms);
                    let effective_raw = bare_model
                        .as_deref()
                        .or(current_model.as_deref())
                        .unwrap_or("unknown");
                    let normalized_model = pricing::normalize_codex_model(effective_raw);
                    let this_event_index = event_index;
                    event_index += 1;

                    let session_id_opt = session_meta
                        .as_ref()
                        .map(|m| m.session_id.as_str())
                        .filter(|s| !s.is_empty());
                    record_codex_row(
                        day_key.as_str(),
                        &normalized_model,
                        &normalized_model,
                        Some(effective_raw),
                        current_turn_id.as_deref(),
                        Some(this_event_index),
                        ts_ms,
                        bare_delta,
                        false,
                        None,
                        None,
                        session_id_opt,
                        Some(&file_identity),
                        catalog,
                        &mut days,
                        &mut rows,
                    );
                }
            }
        }
    }

    CodexParseResult {
        days,
        rows,
        token_snapshots,
        parsed_bytes: offset,
        last_model: current_model,
        last_totals: accumulator.raw_totals_baseline.or(fork_raw_baseline),
        last_codex_turn_id: current_turn_id,
        session_id: session_meta
            .as_ref()
            .map(|m| m.session_id.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from),
        forked_from_id: session_meta.as_ref().and_then(|m| m.forked_from_id.clone()),
        fork_timestamp: session_meta.as_ref().and_then(|m| m.fork_timestamp.clone()),
        depends_on_parent_totals,
        has_unresolved_fork_baseline,
        codex_session: session_meta,
    }
}

#[allow(clippy::too_many_arguments)]
fn compute_forked_event_delta(
    last: Option<CodexTotals>,
    total: Option<CodexTotals>,
    inherited_totals: Option<CodexTotals>,
    remaining_inherited_totals: &mut Option<CodexTotals>,
    tracker: &mut CodexTotalsTracker,
    raw_baseline: &mut Option<CodexTotals>,
    counted_totals: &mut Option<CodexTotals>,
    saw_divergent: &mut bool,
) -> CodexTotals {
    let mut adjust_last_delta = |raw_delta: CodexTotals| -> CodexTotals {
        let Some(mut rem) = *remaining_inherited_totals else {
            return raw_delta;
        };
        let adjusted = CodexTotals {
            input: (raw_delta.input - rem.input).max(0),
            cached: (raw_delta.cached - rem.cached).max(0),
            output: (raw_delta.output - rem.output).max(0),
            reasoning: match (raw_delta.reasoning, rem.reasoning) {
                (Some(r), Some(rem_r)) => Some((r - rem_r).max(0)),
                (Some(r), None) => Some(r),
                _ => None,
            },
        };
        rem.input = (rem.input - raw_delta.input).max(0);
        rem.cached = (rem.cached - raw_delta.cached).max(0);
        rem.output = (rem.output - raw_delta.output).max(0);
        rem.reasoning = match (rem.reasoning, raw_delta.reasoning) {
            (Some(rem_r), Some(r)) => Some((rem_r - r).max(0)),
            (rem_r, _) => rem_r,
        };
        *remaining_inherited_totals = if rem.input == 0 && rem.cached == 0 && rem.output == 0 {
            None
        } else {
            Some(rem)
        };
        adjusted
    };

    if let Some(total) = total {
        let adjusted_total = match inherited_totals {
            Some(inherited_base) => codex_total_delta(Some(inherited_base), total),
            None => total,
        };
        if tracker.is_seen(adjusted_total) {
            return CodexTotals::default();
        }
        let stale_baseline = tracker.watermark.or(*raw_baseline);
        if let Some(prev_water) = stale_baseline
            && codex_looks_like_stale_regression(
                adjusted_total,
                prev_water,
                last.unwrap_or_default(),
            )
        {
            return CodexTotals::default();
        }
        tracker.latch_if_below_watermark(adjusted_total);
        let watermark_baseline = tracker.watermark.or(*raw_baseline);

        let delta = if tracker.saw_interleaved_totals {
            let adj_last = last.map(&mut adjust_last_delta);
            codex_post_latch_event_delta(
                watermark_baseline,
                *counted_totals,
                adjusted_total,
                adj_last,
            )
        } else if *saw_divergent {
            codex_divergent_total_delta(watermark_baseline, *counted_totals, adjusted_total)
        } else {
            codex_total_delta(watermark_baseline, adjusted_total)
        };

        let base = counted_totals.unwrap_or_default();
        let next = codex_add_totals(base, delta);
        *counted_totals = Some(next);
        *raw_baseline = Some(adjusted_total);
        if !codex_totals_equal(Some(adjusted_total), Some(next)) {
            *saw_divergent = true;
        }
        *remaining_inherited_totals = None;
        tracker.commit_observed(adjusted_total);
        return delta;
    }

    if let Some(last) = last {
        let adjusted_delta = adjust_last_delta(last);
        let base = counted_totals.unwrap_or_default();
        let next = codex_add_totals(base, adjusted_delta);
        *counted_totals = Some(next);
        *raw_baseline = Some(next);
        tracker.raise_watermark(next);
        return adjusted_delta;
    }

    CodexTotals::default()
}

#[allow(clippy::too_many_arguments)]
fn process_token_usage_record(
    obj: &serde_json::Map<String, Value>,
    line_day_and_ms: Option<&(DayKey, i64)>,
    last_accepted_day_key: Option<&DayKey>,
    last_accepted_ts_ms: Option<i64>,
    session_meta: Option<&CodexSessionMetadata>,
    file_identity: &str,
    current_model: Option<&str>,
    current_turn_id: Option<&str>,
    current_service_tier: Option<CodexServiceTier>,
    priority_turn_ids: &BTreeSet<String>,
    priority_turns: &BTreeMap<String, CodexPriorityTurnMetadata>,
    catalog: Option<&ModelsDevCatalog>,
    ledger: &mut CodexRequestLedgerState,
    days: &mut BTreeMap<String, BTreeMap<String, Vec<i64>>>,
    rows: &mut Vec<CodexUsageRow>,
) {
    let payload = obj.get("payload").and_then(Value::as_object).unwrap_or(obj);
    let record_thread_id = payload
        .get("thread_id")
        .or_else(|| payload.get("session_id"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let record_session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());

    if let Some(meta) = session_meta {
        if let Some(tid) = record_thread_id
            && tid != meta.session_id
        {
            return;
        }
        if let Some(sid) = record_session_id
            && sid != meta.session_id
        {
            return;
        }
    }

    let response_id = payload
        .get("response_id")
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let is_replay = response_id
        .as_ref()
        .is_some_and(|rid| !ledger.response_ids.insert(rid.clone()));

    let usage_map = payload
        .get("usage")
        .or_else(|| payload.get("last_token_usage"))
        .and_then(Value::as_object);
    let Some(delta) = usage_map.and_then(parse_codex_totals_map) else {
        return;
    };
    if delta.is_zero() {
        return;
    }

    let total_usage = payload
        .get("thread_token_usage")
        .or_else(|| payload.get("total_token_usage"))
        .and_then(Value::as_object)
        .and_then(parse_codex_totals_map);
    let turn_total = payload
        .get("turn_token_usage")
        .and_then(Value::as_object)
        .and_then(parse_codex_totals_map);
    let turn_id = payload
        .get("turn_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| current_turn_id.map(String::from));

    let Some(day_key) = line_day_and_ms
        .map(|(dk, _)| dk.clone())
        .or_else(|| last_accepted_day_key.cloned())
    else {
        return;
    };
    let ts_ms = line_day_and_ms.map(|(_, ms)| *ms).or(last_accepted_ts_ms);

    let mut mirror_keys = make_mirror_keys(turn_id.as_deref(), total_usage, delta);
    if let Some(tt) = turn_total {
        mirror_keys.extend(make_mirror_keys(turn_id.as_deref(), Some(tt), delta));
    }
    let prev_ledger_total = ledger.last_ledger_total;
    if !is_replay && let Some(total) = total_usage {
        ledger.last_ledger_total = Some(total);
    }
    let counter_advanced_more = match (prev_ledger_total, total_usage) {
        (Some(prev), Some(cur)) => {
            let diff = codex_total_delta(Some(prev), cur);
            diff.input > delta.input || diff.cached > delta.cached || diff.output > delta.output
        }
        _ => false,
    };
    let near = NearMirror {
        turn_id: turn_id.clone(),
        timestamp_unix_ms: ts_ms,
        usage: delta,
        counter_advanced_strictly_more_than_usage: counter_advanced_more,
    };

    // If a legacy `token_count` mirror was emitted right before this `token_usage_record`, replace it!
    if let Some(legacy_row_idx) = take_matching_pending_legacy(ledger, &mirror_keys, &near)
        && legacy_row_idx < rows.len()
    {
        let old_row = rows.remove(legacy_row_idx);
        subtract_codex_row_from_days(&old_row, days);
    } else if let Some(ref rid) = response_id {
        ledger.pending_ledger_mirrors = Some(mirror_keys.iter().cloned().collect());
        ledger.pending_ledger_response_id = Some(rid.clone());
        ledger.pending_ledger_near = Some(near);
    }

    if is_replay {
        return;
    }

    let raw_model = extract_model_from_map(payload)
        .or_else(|| current_model.map(String::from))
        .unwrap_or_else(|| {
            resolve_codex_turn_raw_model(current_model, turn_id.as_deref(), priority_turns)
        });
    let normalized_model = pricing::normalize_codex_model(&raw_model);
    let pricing_model =
        resolve_codex_priority_pricing_model(&normalized_model, turn_id.as_deref(), priority_turns);
    let tier =
        pricing::normalize_codex_service_tier(payload.get("service_tier").and_then(Value::as_str))
            .or(current_service_tier);
    let is_fast = is_codex_turn_priority(
        tier,
        turn_id.as_deref(),
        session_meta.map(|m| m.session_id.as_str()),
        priority_turn_ids,
        priority_turns,
    );

    record_codex_row(
        day_key.as_str(),
        &normalized_model,
        &pricing_model,
        Some(&raw_model),
        turn_id.as_deref(),
        None,
        ts_ms,
        delta,
        is_fast,
        response_id,
        Some(mirror_keys),
        session_meta.map(|m| m.session_id.as_str()),
        Some(file_identity),
        catalog,
        days,
        rows,
    );
}

fn make_legacy_snapshot_key(
    turn_id: Option<&str>,
    total: Option<CodexTotals>,
    delta: CodexTotals,
    event_index: usize,
) -> String {
    let tid = turn_id.unwrap_or("");
    if let Some(t) = total {
        format!("snap|{tid}|{}|{}|{}", t.input, t.cached, t.output)
    } else {
        format!(
            "snap_idx|{tid}|{event_index}|{}|{}|{}",
            delta.input, delta.cached, delta.output
        )
    }
}

fn make_mirror_keys(
    turn_id: Option<&str>,
    total: Option<CodexTotals>,
    delta: CodexTotals,
) -> Vec<String> {
    let mut keys = Vec::new();
    let tid = turn_id.unwrap_or("");
    keys.push(format!(
        "delta|{tid}|{}|{}|{}",
        delta.input, delta.cached, delta.output
    ));
    if let Some(t) = total {
        keys.push(format!("total|{tid}|{}|{}|{}", t.input, t.cached, t.output));
    }
    keys
}

fn is_matched_by_pending_ledger(
    ledger: &mut CodexRequestLedgerState,
    snapshot_key: &str,
    mirror_keys: &[String],
    near: &NearMirror,
) -> bool {
    let Some(ref pending_keys) = ledger.pending_ledger_mirrors else {
        return false;
    };
    let Some(ref response_id) = ledger.pending_ledger_response_id else {
        ledger.clear_pending();
        return false;
    };
    if let Some(owner) = ledger.mirrored_snapshots.get(snapshot_key)
        && owner != response_id
    {
        ledger.clear_pending();
        return false;
    }
    let key_overlap = mirror_keys.iter().any(|k| pending_keys.contains(k));
    let near_match = ledger
        .pending_ledger_near
        .as_ref()
        .is_some_and(|p| near.matches(p, true));
    let matched = key_overlap || near_match;
    if matched {
        ledger
            .mirrored_snapshots
            .insert(snapshot_key.to_owned(), response_id.clone());
    }
    ledger.clear_pending();
    matched
}

fn take_matching_pending_legacy(
    ledger: &mut CodexRequestLedgerState,
    mirror_keys: &[String],
    near: &NearMirror,
) -> Option<usize> {
    let pending_keys = ledger.pending_legacy_mirrors.as_ref()?;
    let row_idx = ledger.pending_legacy_row_index?;
    let key_overlap = mirror_keys.iter().any(|k| pending_keys.contains(k));
    let near_match = ledger
        .pending_legacy_near
        .as_ref()
        .is_some_and(|p| near.matches(p, true));
    let matched = key_overlap || near_match;
    ledger.clear_pending();
    if matched { Some(row_idx) } else { None }
}

fn resolve_codex_turn_raw_model(
    current_model: Option<&str>,
    turn_id: Option<&str>,
    priority_turns: &BTreeMap<String, CodexPriorityTurnMetadata>,
) -> String {
    if let Some(tid) = turn_id
        && let Some(meta) = priority_turns.get(tid)
        && let Some(ref priority_model) = meta.model
        && !priority_model.is_empty()
    {
        return priority_model.clone();
    }
    current_model.unwrap_or("gpt-5").to_owned()
}

fn resolve_codex_priority_pricing_model(
    row_model: &str,
    turn_id: Option<&str>,
    priority_turns: &BTreeMap<String, CodexPriorityTurnMetadata>,
) -> String {
    if let Some(tid) = turn_id
        && let Some(meta) = priority_turns.get(tid)
        && let Some(ref priority_model) = meta.model
        && !priority_model.is_empty()
        && pricing::codex_api_fast_multiplier(priority_model).is_some()
    {
        return pricing::normalize_codex_model(priority_model);
    }
    row_model.to_owned()
}

fn is_codex_turn_priority(
    current_tier: Option<CodexServiceTier>,
    turn_id: Option<&str>,
    _session_id: Option<&str>,
    priority_turn_ids: &BTreeSet<String>,
    priority_turns: &BTreeMap<String, CodexPriorityTurnMetadata>,
) -> bool {
    if matches!(
        current_tier,
        Some(CodexServiceTier::Priority | CodexServiceTier::Fast)
    ) {
        return true;
    }
    if let Some(tid) = turn_id
        && priority_turn_ids.contains(tid)
    {
        return true;
    }
    if let Some(tid) = turn_id
        && priority_turns.contains_key(tid)
    {
        return true;
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn record_codex_row(
    day_key: &str,
    normalized_model: &str,
    pricing_model: &str,
    raw_model: Option<&str>,
    turn_id: Option<&str>,
    event_index: Option<usize>,
    timestamp_unix_ms: Option<i64>,
    delta: CodexTotals,
    is_fast: bool,
    response_id: Option<String>,
    request_mirror_keys: Option<Vec<String>>,
    session_id: Option<&str>,
    file_identity: Option<&str>,
    catalog: Option<&ModelsDevCatalog>,
    days: &mut BTreeMap<String, BTreeMap<String, Vec<i64>>>,
    rows: &mut Vec<CodexUsageRow>,
) {
    let tier = if is_fast {
        Some(CodexServiceTier::Priority)
    } else {
        Some(CodexServiceTier::Standard)
    };
    let cost_usd = pricing::codex_cost_usd_with_tier(
        pricing_model,
        delta.input,
        delta.cached,
        delta.output,
        tier,
        catalog,
    );
    let cost_nanos = cost_usd.map(|c| (c * 1_000_000_000.0).round() as i64);
    let total_tokens = delta.scanner_units();
    let unpriced_tokens = if cost_nanos.is_none() {
        Some(total_tokens)
    } else {
        None
    };

    let row = CodexUsageRow {
        day: day_key.to_owned(),
        model: normalized_model.to_owned(),
        raw_model: raw_model.map(String::from),
        turn_id: turn_id.map(String::from),
        event_index,
        timestamp_unix_ms,
        input: delta.input,
        cached: delta.cached,
        output: delta.output,
        reasoning: delta.reasoning,
        response_id,
        request_mirror_keys,
        known_cost_nanos: cost_nanos,
        unpriced_tokens,
        pricing_model: Some(pricing_model.to_owned()),
        pricing_mode: Some(if is_fast { "priority" } else { "standard" }.to_owned()),
        session_id: session_id.map(String::from),
        file_identity: file_identity.map(String::from),
    }
    .with_clamped_reasoning();

    add_codex_row_to_days(&row, days);
    rows.push(row);
}

fn add_codex_row_to_days(
    row: &CodexUsageRow,
    days: &mut BTreeMap<String, BTreeMap<String, Vec<i64>>>,
) {
    let packed = days
        .entry(row.day.clone())
        .or_default()
        .entry(row.model.clone())
        .or_insert_with(|| vec![0; 14]);
    if packed.len() < 14 {
        packed.resize(14, 0);
    }
    let is_priority = row.pricing_mode.as_deref() == Some("priority");
    let tokens = row.input + row.cached + row.output;
    packed[0] += row.input;
    packed[1] += row.cached;
    packed[2] += row.output;
    packed[3] += row.known_cost_nanos.unwrap_or(0);
    if is_priority {
        packed[5] += tokens;
        packed[9] += row.input;
        packed[10] += row.cached;
        packed[11] += row.output;
    } else {
        packed[4] += tokens;
        packed[6] += row.input;
        packed[7] += row.cached;
        packed[8] += row.output;
    }
    packed[12] += row.unpriced_tokens.unwrap_or(0);
    packed[13] += 1;
}

fn subtract_codex_row_from_days(
    row: &CodexUsageRow,
    days: &mut BTreeMap<String, BTreeMap<String, Vec<i64>>>,
) {
    let Some(day_map) = days.get_mut(&row.day) else {
        return;
    };
    let Some(packed) = day_map.get_mut(&row.model) else {
        return;
    };
    if packed.len() < 14 {
        return;
    }
    let is_priority = row.pricing_mode.as_deref() == Some("priority");
    let tokens = row.input + row.cached + row.output;
    packed[0] = (packed[0] - row.input).max(0);
    packed[1] = (packed[1] - row.cached).max(0);
    packed[2] = (packed[2] - row.output).max(0);
    packed[3] = (packed[3] - row.known_cost_nanos.unwrap_or(0)).max(0);
    if is_priority {
        packed[5] = (packed[5] - tokens).max(0);
        packed[9] = (packed[9] - row.input).max(0);
        packed[10] = (packed[10] - row.cached).max(0);
        packed[11] = (packed[11] - row.output).max(0);
    } else {
        packed[4] = (packed[4] - tokens).max(0);
        packed[6] = (packed[6] - row.input).max(0);
        packed[7] = (packed[7] - row.cached).max(0);
        packed[8] = (packed[8] - row.output).max(0);
    }
    packed[12] = (packed[12] - row.unpriced_tokens.unwrap_or(0)).max(0);
    packed[13] = (packed[13] - 1).max(0);
}

fn parse_codex_line_json(raw: &str) -> Option<Value> {
    serde_json::from_str::<Value>(raw).ok()
}

fn extract_session_metadata(val: &Value) -> Option<CodexSessionMetadata> {
    let obj = val.as_object()?;
    let payload = obj.get("payload").and_then(Value::as_object);
    let session_id = [
        payload.and_then(|p| p.get("id")),
        obj.get("id"),
        payload.and_then(|p| p.get("session_id")),
        payload.and_then(|p| p.get("sessionId")),
        obj.get("session_id"),
        obj.get("sessionId"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .find(|s| !s.is_empty())
    .unwrap_or("")
    .to_owned();

    let forked_from_id = payload
        .and_then(|p| {
            p.get("forked_from_id")
                .or_else(|| p.get("forkedFromId"))
                .or_else(|| p.get("parent_session_id"))
                .or_else(|| p.get("parentSessionId"))
        })
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let fork_timestamp = payload
        .and_then(|p| p.get("timestamp"))
        .or_else(|| obj.get("timestamp"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let is_subagent_thread = match payload.and_then(|p| p.get("source")) {
        Some(Value::String(s)) => s.trim().eq_ignore_ascii_case("subagent"),
        Some(Value::Object(map)) => map.contains_key("subagent"),
        _ => false,
    };
    let subagent_history_start_ordinal = payload
        .and_then(|p| p.get("subagent_history_start_ordinal"))
        .and_then(Value::as_i64);
    let history_base_thread_id = payload
        .and_then(|p| p.get("history_base").or_else(|| p.get("historyBase")))
        .and_then(Value::as_object)
        .and_then(|hb| hb.get("thread_id").or_else(|| hb.get("threadId")))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let cwd = payload
        .and_then(|p| p.get("cwd"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let title = payload
        .and_then(|p| p.get("title"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let started_at_unix_ms = fork_timestamp
        .as_deref()
        .and_then(|ts| timestamp::day_key_from_timestamp(ts, None))
        .map(|(_, ms)| ms);

    Some(CodexSessionMetadata {
        session_id,
        forked_from_id,
        fork_timestamp,
        is_subagent_thread,
        subagent_history_start_ordinal,
        history_base_thread_id,
        cwd,
        title,
        started_at_unix_ms,
        latest_activity_unix_ms: started_at_unix_ms,
    })
}

fn enrich_same_leaf_metadata(leaf: &mut CodexSessionMetadata, extra: &CodexSessionMetadata) {
    if leaf.forked_from_id.is_none() {
        leaf.forked_from_id = extra.forked_from_id.clone();
    }
    if leaf.fork_timestamp.is_none() {
        leaf.fork_timestamp = extra.fork_timestamp.clone();
    }
    if !leaf.is_subagent_thread && extra.is_subagent_thread {
        leaf.is_subagent_thread = true;
    }
    if leaf.subagent_history_start_ordinal.is_none() {
        leaf.subagent_history_start_ordinal = extra.subagent_history_start_ordinal;
    }
    if leaf.history_base_thread_id.is_none() {
        leaf.history_base_thread_id = extra.history_base_thread_id.clone();
    }
    if leaf.cwd.is_none() {
        leaf.cwd = extra.cwd.clone();
    }
    if leaf.title.is_none() {
        leaf.title = extra.title.clone();
    }
}

fn extract_truncated_session_id(raw_prefix: &str) -> Option<(bool, Option<String>)> {
    let trimmed = raw_prefix.trim_start();
    if !trimmed.starts_with('{') {
        return None;
    }
    let payload_idx = trimmed.find(r#""payload""#)?;
    let before_payload = &trimmed[..payload_idx];
    if !before_payload.contains(r#""type":"session_meta""#)
        && !before_payload.contains(r#""type": "session_meta""#)
    {
        return None;
    }
    let after_payload = &trimmed[payload_idx..];
    let id_val = extract_json_string_field(after_payload, "id")
        .or_else(|| extract_json_string_field(after_payload, "session_id"));
    Some((true, id_val))
}

fn extract_truncated_turn_context_model(raw_prefix: &str) -> Option<String> {
    let trimmed = raw_prefix.trim_start();
    if !trimmed.starts_with('{') {
        return None;
    }
    let payload_idx = trimmed.find(r#""payload""#)?;
    let before_payload = &trimmed[..payload_idx];
    if !before_payload.contains(r#""type":"turn_context""#)
        && !before_payload.contains(r#""type": "turn_context""#)
    {
        return None;
    }
    let ts = extract_json_string_field(before_payload, "timestamp")?;
    timestamp::day_key_from_timestamp(&ts, None)?;
    let after_payload = &trimmed[payload_idx..];
    extract_json_string_field(after_payload, "model")
}

fn extract_json_string_field(text: &str, key: &str) -> Option<String> {
    for pattern in [format!(r#""{key}":""#), format!(r#""{key}": ""#)] {
        if let Some(pos) = text.find(&pattern) {
            let tail = &text[pos + pattern.len()..];
            let end = tail.find('"')?;
            let val = &tail[..end];
            if !val.is_empty() {
                return Some(val.to_owned());
            }
        }
    }
    None
}

type PredecodedCodexLine = (usize, u64, Option<i64>, bool, String, Option<Value>);

fn build_subagent_observations(
    parsed_lines: &[PredecodedCodexLine],
) -> Vec<CodexSubagentObservation> {
    let mut observations = Vec::new();
    for (idx, _end_offset, _ordinal, was_truncated, raw, val_opt) in parsed_lines {
        if *was_truncated {
            if let Some((true, sid)) = extract_truncated_session_id(raw) {
                observations.push(CodexSubagentObservation::session_metadata(*idx, sid));
            }
            continue;
        }
        let Some(val) = val_opt else {
            continue;
        };
        let Some(obj) = val.as_object() else {
            continue;
        };
        let record_type = obj.get("type").and_then(Value::as_str);
        match record_type {
            Some("session_meta") => {
                let sid = extract_session_metadata(val)
                    .map(|m| m.session_id)
                    .filter(|s| !s.is_empty());
                observations.push(CodexSubagentObservation::session_metadata(*idx, sid));
            }
            Some("turn_context") => {
                let has_valid_ts = obj
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(|ts| timestamp::day_key_from_timestamp(ts, None))
                    .is_some();
                if has_valid_ts {
                    observations.push(CodexSubagentObservation::turn_context(*idx));
                }
            }
            Some("inter_agent_communication_metadata") => {
                let has_valid_ts = obj
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(|ts| timestamp::day_key_from_timestamp(ts, None))
                    .is_some();
                if has_valid_ts {
                    let trigger_turn = obj
                        .get("payload")
                        .and_then(Value::as_object)
                        .and_then(|p| p.get("trigger_turn"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    observations.push(CodexSubagentObservation::inter_agent_communication(
                        *idx,
                        trigger_turn,
                    ));
                }
            }
            Some("token_usage_record") => {
                if let Some((_, last, total)) = extract_boundary_token_count_from_value(val) {
                    observations.push(CodexSubagentObservation::token_count(*idx, last, total));
                }
            }
            Some("event_msg") => {
                if is_trigger_turn_communication(val) {
                    observations.push(CodexSubagentObservation::inter_agent_communication(
                        *idx, true,
                    ));
                } else if let Some((_, last, total)) = extract_boundary_token_count_from_value(val)
                {
                    observations.push(CodexSubagentObservation::token_count(*idx, last, total));
                }
            }
            _ => {}
        }
    }
    observations
}

fn compute_explicit_ordinal_suffix(
    parsed_lines: &[PredecodedCodexLine],
    start_ordinal: i64,
) -> Option<CodexSubagentOwnedSuffix> {
    let first_owned_line_idx = parsed_lines
        .iter()
        .find(|(_, _, ord, _, _, _)| ord.unwrap_or(i64::MIN) >= start_ordinal)
        .map(|(idx, _, _, _, _, _)| *idx)?;

    let mut inherited_total: Option<CodexTotals> = None;
    for (_, _, ord, _, _, val_opt) in parsed_lines
        .iter()
        .take_while(|(_, _, ord, _, _, _)| ord.unwrap_or(i64::MIN) < start_ordinal)
    {
        let _ = ord;
        if let Some(val) = val_opt
            && let Some((_, _, Some(total))) = extract_boundary_token_count_from_value(val)
        {
            inherited_total = Some(total);
        }
    }

    let mut first_owned_token: Option<(usize, Option<CodexTotals>, Option<CodexTotals>)> = None;
    for (idx, _, ord, _, _, val_opt) in parsed_lines {
        if ord.unwrap_or(i64::MIN) < start_ordinal {
            continue;
        }
        let Some(val) = val_opt else {
            continue;
        };
        let Some((is_usage_record, last, total)) = extract_boundary_token_count_from_value(val)
        else {
            continue;
        };
        if is_usage_record {
            first_owned_token = Some((*idx, last, total));
            break;
        }
        if let (Some(total), Some(baseline)) = (total, inherited_total)
            && (codex_totals_equal(Some(total), Some(baseline))
                || (CodexSubagentRolloutShape::totals_contain_usage(baseline)
                    && codex_totals_equal(Some(total), last)
                    && codex_totals_at_least(total, baseline)))
        {
            inherited_total = Some(total);
            continue;
        }
        if total.is_some() || last.is_some() {
            first_owned_token = Some((*idx, last, total));
            break;
        }
    }

    let inferred_total = first_owned_token.and_then(|(_, last, total)| match (total, last) {
        (Some(total), Some(last)) => Some(codex_total_delta(Some(last), total)),
        (None, Some(_)) => Some(CodexTotals::default()),
        _ => None,
    });

    let raw_totals_baseline = inferred_total.or(inherited_total)?;
    Some(CodexSubagentOwnedSuffix {
        start_line_index: first_owned_line_idx,
        first_token_line_index: Some(
            first_owned_token.map_or(usize::MAX, |(line_idx, _, _)| line_idx),
        ),
        raw_totals_baseline,
    })
}

fn extract_boundary_token_count_from_value(
    val: &Value,
) -> Option<(bool, Option<CodexTotals>, Option<CodexTotals>)> {
    let obj = val.as_object()?;
    let has_valid_ts = obj
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|ts| timestamp::day_key_from_timestamp(ts, None))
        .is_some();
    if !has_valid_ts {
        return None;
    }
    let record_type = obj.get("type").and_then(Value::as_str)?;
    if record_type == "token_usage_record" {
        let payload = obj.get("payload")?.as_object()?;
        let last = payload
            .get("usage")
            .or_else(|| payload.get("last_token_usage"))
            .and_then(Value::as_object)
            .and_then(parse_codex_totals_map);
        let total = payload
            .get("thread_token_usage")
            .or_else(|| payload.get("total_token_usage"))
            .and_then(Value::as_object)
            .and_then(parse_codex_totals_map);
        return if last.is_some() || total.is_some() {
            Some((true, last, total))
        } else {
            None
        };
    }
    if record_type == "event_msg" {
        let payload = obj.get("payload")?.as_object()?;
        if payload.get("type").and_then(Value::as_str) == Some("token_usage_record") {
            let last = payload
                .get("usage")
                .or_else(|| payload.get("last_token_usage"))
                .and_then(Value::as_object)
                .and_then(parse_codex_totals_map);
            let total = payload
                .get("thread_token_usage")
                .or_else(|| payload.get("total_token_usage"))
                .and_then(Value::as_object)
                .and_then(parse_codex_totals_map);
            return if last.is_some() || total.is_some() {
                Some((true, last, total))
            } else {
                None
            };
        }
        let (last, total) = extract_token_count_from_value(val)?;
        return Some((false, last, total));
    }
    None
}

fn is_trigger_turn_communication(val: &Value) -> bool {
    let Some(obj) = val.as_object() else {
        return false;
    };
    let record_type = obj.get("type").and_then(Value::as_str);
    let Some(payload) = obj.get("payload").and_then(Value::as_object) else {
        return false;
    };
    if record_type == Some("inter_agent_communication_metadata") {
        let has_valid_ts = obj
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|ts| timestamp::day_key_from_timestamp(ts, None))
            .is_some();
        return has_valid_ts
            && payload
                .get("trigger_turn")
                .and_then(Value::as_bool)
                .unwrap_or(false);
    }
    if record_type == Some("event_msg")
        && payload.get("type").and_then(Value::as_str) == Some("inter_agent_communication")
        && payload.get("trigger_turn").and_then(Value::as_bool) == Some(true)
    {
        return true;
    }
    false
}

fn extract_token_count_from_value(
    val: &Value,
) -> Option<(Option<CodexTotals>, Option<CodexTotals>)> {
    let obj = val.as_object()?;
    if obj.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let payload = obj.get("payload")?.as_object()?;
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let info = payload.get("info")?.as_object()?;
    let last = info
        .get("last_token_usage")
        .and_then(Value::as_object)
        .and_then(parse_codex_totals_map);
    let total = info
        .get("total_token_usage")
        .and_then(Value::as_object)
        .and_then(parse_codex_totals_map);
    if last.is_none() && total.is_none() {
        None
    } else {
        Some((last, total))
    }
}

fn parse_codex_totals_map(map: &serde_json::Map<String, Value>) -> Option<CodexTotals> {
    let input = json_non_negative_i64(map.get("input_tokens")).unwrap_or(0);
    let cached_a = json_non_negative_i64(map.get("cached_input_tokens")).unwrap_or(0);
    let cached_b = json_non_negative_i64(map.get("cache_read_input_tokens")).unwrap_or(0);
    let cached = cached_a.max(cached_b);
    let output = json_non_negative_i64(map.get("output_tokens")).unwrap_or(0);
    let reasoning = json_non_negative_i64(map.get("reasoning_output_tokens"));
    Some(CodexTotals {
        input,
        cached,
        output,
        reasoning,
    })
}

fn extract_bare_usage_row(
    obj: &serde_json::Map<String, Value>,
) -> Option<(CodexTotals, Option<String>)> {
    let usage_val = obj
        .get("usage")
        .or_else(|| obj.get("data").and_then(|d| d.get("usage")))
        .or_else(|| obj.get("response").and_then(|r| r.get("usage")))
        .or_else(|| obj.get("result").and_then(|r| r.get("usage")))?;
    let usage_map = usage_val.as_object()?;

    let cached = [
        "cached_input_tokens",
        "cache_read_input_tokens",
        "cached_tokens",
    ]
    .into_iter()
    .filter_map(|k| json_non_negative_i64(usage_map.get(k)))
    .max()
    .unwrap_or(0);

    let input = if let Some(inp) = json_non_negative_i64(usage_map.get("input_tokens"))
        .or_else(|| json_non_negative_i64(usage_map.get("input")))
    {
        inp
    } else if let Some(prompt) = json_non_negative_i64(usage_map.get("prompt_tokens")) {
        (prompt - cached).max(0)
    } else {
        0
    };

    let output = json_non_negative_i64(usage_map.get("output_tokens"))
        .or_else(|| json_non_negative_i64(usage_map.get("completion_tokens")))
        .or_else(|| json_non_negative_i64(usage_map.get("output")))
        .unwrap_or(0);

    let reasoning = json_non_negative_i64(usage_map.get("reasoning_output_tokens"));
    if input <= 0 && cached <= 0 && output <= 0 {
        return None;
    }

    let model = extract_model_from_map(obj)
        .or_else(|| {
            obj.get("response")
                .and_then(Value::as_object)
                .and_then(extract_model_from_map)
        })
        .or_else(|| {
            obj.get("data")
                .and_then(Value::as_object)
                .and_then(extract_model_from_map)
        });

    Some((
        CodexTotals {
            input,
            cached,
            output,
            reasoning,
        },
        model,
    ))
}

fn extract_model_from_map(map: &serde_json::Map<String, Value>) -> Option<String> {
    map.get("model")
        .or_else(|| map.get("model_name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn json_non_negative_i64(val: Option<&Value>) -> Option<i64> {
    let val = val?;
    match val {
        Value::Number(num) => num
            .as_i64()
            .or_else(|| num.as_u64().and_then(|u| i64::try_from(u).ok()))
            .filter(|&n| n >= 0),
        _ => None,
    }
}

/// Computes inherited totals from a parent session's `token_snapshots` at or before `cutoff_timestamp`.
#[must_use]
pub fn resolve_parent_inherited_totals(
    parent_snapshots: &[CodexTokenSnapshot],
    parent_initial_inherited: Option<CodexTotals>,
    cutoff_timestamp: Option<&str>,
) -> Option<CodexTotals> {
    resolve_parent_inherited_totals_with_origin(
        parent_snapshots,
        parent_initial_inherited,
        cutoff_timestamp,
    )
    .0
}

#[must_use]
fn resolve_parent_inherited_totals_with_origin(
    parent_snapshots: &[CodexTokenSnapshot],
    parent_initial_inherited: Option<CodexTotals>,
    cutoff_timestamp: Option<&str>,
) -> (Option<CodexTotals>, Option<CodexTotals>) {
    let cutoff_ms = cutoff_timestamp
        .and_then(|ts| timestamp::day_key_from_timestamp(ts, None))
        .map(|(_, ms)| ms);

    let is_at_or_before = |snap: &CodexTokenSnapshot| -> bool {
        let Some(cutoff_ts) = cutoff_timestamp else {
            return true;
        };
        if cutoff_ts.is_empty() {
            return true;
        }
        if let (Some(snap_ms), Some(c_ms)) = (snap.timestamp_unix_ms, cutoff_ms) {
            return snap_ms <= c_ms;
        }
        snap.timestamp.as_str() <= cutoff_ts
    };

    let mut accumulator = CodexSnapshotAccumulator::default();
    let mut inherited = parent_initial_inherited;
    let mut parent_fork_origin: Option<CodexTotals> = None;
    for (idx, snap) in parent_snapshots.iter().enumerate() {
        if idx == 0
            && let (Some(total), Some(last)) = (snap.total, snap.last)
        {
            parent_fork_origin = Some(codex_total_delta(Some(last), total));
        }
        let (_, counted) = accumulator.apply_event(snap.last, snap.total, true);
        if is_at_or_before(snap) {
            inherited = Some(counted);
        }
    }
    (inherited, parent_fork_origin)
}

/// Canonical row identity key for Codex usage rows (`CostUsageScanner.codexUsageRowKey`).
#[must_use]
pub fn codex_usage_row_key(
    session_id: Option<&str>,
    file_identity: Option<&str>,
    row: &CodexUsageRow,
) -> String {
    let sid = session_id
        .filter(|s| !s.is_empty())
        .or_else(|| row.session_id.as_deref().filter(|s| !s.is_empty()));
    let fid = file_identity
        .filter(|s| !s.is_empty())
        .or_else(|| row.file_identity.as_deref().filter(|s| !s.is_empty()));
    if let Some(ref response_id) = row.response_id {
        let scope = sid.or(fid).unwrap_or("");
        return format!("{scope}\x1Fresponse\x1F{response_id}");
    }
    let owner = match sid {
        Some(s) => format!("session:{s}"),
        None => format!("file:{}", fid.unwrap_or("")),
    };
    format!(
        "{owner}\x1F{}\x1F{}\x1F{}\x1F{}\x1F{}\x1F{}\x1F{}",
        row.turn_id.as_deref().unwrap_or(""),
        row.event_index.map_or_else(String::new, |i| i.to_string()),
        row.day,
        row.model,
        row.input,
        row.cached,
        row.output,
    )
}

/// Cross-file deduplication key for Codex usage rows (`CostUsageScanner.codexCrossFileRowKey`).
#[must_use]
pub fn codex_cross_file_row_key(
    session_id: Option<&str>,
    file_identity: Option<&str>,
    row: &CodexUsageRow,
) -> String {
    let key = codex_usage_row_key(session_id, file_identity, row);
    if row.response_id.is_some() {
        key
    } else {
        format!(
            "{key}\x1F{}",
            row.timestamp_unix_ms
                .map_or_else(String::new, |ms| ms.to_string())
        )
    }
}

/// Deduplicates Codex rows across live + archived session copies.
///
/// Mirrors `CostUsageScanner.uniqueCodexRows` and `reconcileCodexRequestMirrors`.
#[must_use]
pub fn deduplicate_codex_rows(rows: Vec<CodexUsageRow>) -> Vec<CodexUsageRow> {
    let mut seen_cross_keys: BTreeSet<String> = BTreeSet::new();
    let mut seen_mirror_keys: BTreeSet<(String, String)> = BTreeSet::new();

    let scope_for_row = |row: &CodexUsageRow| -> String {
        if let Some(sid) = row.session_id.as_deref().filter(|s| !s.is_empty()) {
            sid.to_owned()
        } else if let Some(fid) = row.file_identity.as_deref().filter(|s| !s.is_empty()) {
            format!("file:{fid}")
        } else {
            String::new()
        }
    };

    // First pass: collect all mirror keys owned by `token_usage_record` rows (those with `response_id`)
    for row in &rows {
        if row.response_id.is_some()
            && let Some(ref mkeys) = row.request_mirror_keys
        {
            let scope = scope_for_row(row);
            for mk in mkeys {
                seen_mirror_keys.insert((scope.clone(), mk.clone()));
            }
        }
    }

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let scope = scope_for_row(&row);
        if row.response_id.is_none()
            && let Some(ref mkeys) = row.request_mirror_keys
            && mkeys
                .iter()
                .any(|mk| seen_mirror_keys.contains(&(scope.clone(), mk.clone())))
        {
            continue;
        }
        let cross_key = codex_cross_file_row_key(
            row.session_id.as_deref(),
            row.file_identity.as_deref(),
            &row,
        );
        if !seen_cross_keys.insert(cross_key) {
            continue;
        }
        out.push(row);
    }
    out
}

/// Options for scanning a Codex home or sessions directory.
#[derive(Debug, Clone, Default)]
pub struct CodexScanOptions {
    pub codex_home: Option<PathBuf>,
    pub codex_sessions_root: Option<PathBuf>,
    pub priority_db_path: Option<PathBuf>,
    pub timezone: Option<TimeZone>,
    pub catalog: Option<ModelsDevCatalog>,
}

/// Aggregate result of scanning a Codex home directory across `sessions/` and `archived_sessions/`.
#[derive(Debug, Clone, Default)]
pub struct CodexScanReport {
    pub rows: Vec<CodexUsageRow>,
    pub unmetered_requests_by_day: BTreeMap<String, u64>,
    pub files: BTreeMap<String, CodexParseResult>,
}

/// Discovers and scans all Codex JSONL session files under `sessions/` and `archived_sessions/`,
/// resolving fork baselines and priority trace turns.
#[must_use]
pub fn scan_codex_home(range: &DayRange, options: &CodexScanOptions) -> CodexScanReport {
    let roots = resolve_codex_roots(options);
    let codex_home = options.codex_home.clone().or_else(|| {
        roots
            .first()
            .and_then(|r| r.parent().map(Path::to_path_buf))
    });

    let priority_turns = if let Some(ref db_path) = options.priority_db_path {
        codex_priority::codex_priority_turns(
            db_path,
            Some(range.scan_since_key.as_str()),
            Some(range.scan_until_key.as_str()),
            range.timezone.clone(),
        )
    } else if let Some(ref home) = codex_home {
        codex_priority::load_codex_priority_turns_for_home(
            home,
            Some(range.scan_since_key.as_str()),
            Some(range.scan_until_key.as_str()),
            range.timezone.clone(),
        )
    } else {
        BTreeMap::new()
    };

    let session_tiers = codex_home
        .as_deref()
        .map(codex_priority::codex_session_service_tiers)
        .unwrap_or_default();

    let mut all_files = Vec::new();
    let mut seen_paths = BTreeSet::new();
    for root in &roots {
        for file in collect_jsonl_files_recursive(root) {
            let key = file.to_string_lossy().into_owned();
            if seen_paths.insert(key) {
                all_files.push(file);
            }
        }
    }
    all_files.sort();

    // Build session_id -> PathBuf index
    let mut session_index: BTreeMap<String, PathBuf> = BTreeMap::new();
    for file in &all_files {
        if let Ok(Some(sid)) = parse_codex_session_identifier(file) {
            session_index.entry(sid).or_insert_with(|| file.clone());
        }
    }

    let mut parsed_by_session: BTreeMap<String, CodexParseResult> = BTreeMap::new();
    let mut parsed_by_path: BTreeMap<String, CodexParseResult> = BTreeMap::new();
    let mut visiting = BTreeSet::new();

    for file in &all_files {
        let path_key = file.to_string_lossy().into_owned();
        if parsed_by_path.contains_key(&path_key) {
            continue;
        }
        let parsed = parse_codex_file_with_recursive_parents(
            file,
            range,
            &priority_turns,
            &session_tiers,
            options.catalog.as_ref(),
            &session_index,
            &mut parsed_by_session,
            &mut visiting,
        );
        if let Some(ref sid) = parsed.session_id {
            parsed_by_session
                .entry(sid.clone())
                .or_insert_with(|| parsed.clone());
        }
        parsed_by_path.insert(path_key, parsed);
    }

    let mut all_rows = Vec::new();
    let mut unmetered_requests_by_day: BTreeMap<String, u64> = BTreeMap::new();
    let mut unmetered_sessions_seen = BTreeSet::new();

    for (path_key, parsed) in &parsed_by_path {
        if parsed.has_unresolved_fork_baseline && parsed.rows.is_empty() {
            let session_dedup_key = parsed
                .session_id
                .clone()
                .unwrap_or_else(|| path_key.clone());
            if unmetered_sessions_seen.insert(session_dedup_key)
                && let Some(day_key) = unresolved_fork_day_key(Path::new(path_key), parsed, range)
                && range.contains_day_str(day_key.as_str())
            {
                *unmetered_requests_by_day
                    .entry(day_key.as_str().to_owned())
                    .or_insert(0) += 1;
            }
        }
        for row in &parsed.rows {
            if range.contains_day_str(&row.day) {
                all_rows.push(row.clone());
            }
        }
    }

    let deduped_rows = deduplicate_codex_rows(all_rows);

    CodexScanReport {
        rows: deduped_rows,
        unmetered_requests_by_day,
        files: parsed_by_path,
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_codex_file_with_recursive_parents(
    file: &Path,
    range: &DayRange,
    priority_turns: &BTreeMap<String, CodexPriorityTurnMetadata>,
    session_tiers: &BTreeMap<String, CodexServiceTier>,
    catalog: Option<&ModelsDevCatalog>,
    session_index: &BTreeMap<String, PathBuf>,
    parsed_by_session: &mut BTreeMap<String, CodexParseResult>,
    visiting: &mut BTreeSet<String>,
) -> CodexParseResult {
    parse_codex_file(
        file,
        range,
        0,
        None,
        None,
        None,
        priority_turns,
        session_tiers,
        catalog,
        |parent_id, cutoff_ts| {
            if visiting.contains(parent_id) {
                return CodexForkBaseline::Unresolved;
            }
            if !parsed_by_session.contains_key(parent_id) {
                let Some(parent_path) = session_index.get(parent_id) else {
                    return CodexForkBaseline::Unresolved;
                };
                visiting.insert(parent_id.to_owned());
                let parent_parsed = parse_codex_file_with_recursive_parents(
                    parent_path,
                    range,
                    priority_turns,
                    session_tiers,
                    catalog,
                    session_index,
                    parsed_by_session,
                    visiting,
                );
                visiting.remove(parent_id);
                parsed_by_session.insert(parent_id.to_owned(), parent_parsed);
            }
            let Some(parent_res) = parsed_by_session.get(parent_id) else {
                return CodexForkBaseline::Unresolved;
            };
            if parent_res.has_unresolved_fork_baseline {
                return CodexForkBaseline::Unresolved;
            }
            let (inherited, parent_fork_origin) = resolve_parent_inherited_totals_with_origin(
                &parent_res.token_snapshots,
                None,
                cutoff_ts,
            );
            if inherited.is_none()
                && parent_res.forked_from_id.is_some()
                && parent_fork_origin.is_none()
            {
                return CodexForkBaseline::Unresolved;
            }
            CodexForkBaseline::Resolved(inherited.or(parent_fork_origin))
        },
    )
}

fn unresolved_fork_day_key(
    file_path: &Path,
    parsed: &CodexParseResult,
    range: &DayRange,
) -> Option<DayKey> {
    if let Some(ref meta) = parsed.codex_session
        && let Some(ms) = meta.latest_activity_unix_ms.or(meta.started_at_unix_ms)
        && let Ok(dt) = Timestamp::from_millisecond(ms)
    {
        return Some(timestamp::day_key_from_utc(dt, range.timezone.clone()));
    }
    let mtime = fs::symlink_metadata(file_path).ok()?.modified().ok()?;
    let dur = mtime.duration_since(UNIX_EPOCH).ok()?;
    let dt = Timestamp::from_second(dur.as_secs() as i64).ok()?;
    Some(timestamp::day_key_from_utc(dt, range.timezone.clone()))
}

/// Resolves Codex `sessions/` and `archived_sessions/` roots from explicit `options`,
/// injected `env`, and optional `user_home` without reading process environment directly.
#[must_use]
pub fn resolve_codex_roots_with(
    options: &CodexScanOptions,
    env: &BTreeMap<String, String>,
    user_home: Option<&Path>,
) -> Vec<PathBuf> {
    let sessions_root = if let Some(ref root) = options.codex_sessions_root {
        root.clone()
    } else if let Some(ref home) = options.codex_home {
        home.join("sessions")
    } else if let Some(env_home) = env.get("CODEX_HOME").map(|s| s.trim())
        && !env_home.is_empty()
    {
        PathBuf::from(env_home).join("sessions")
    } else if let Some(user_home) = user_home {
        user_home.join(".codex").join("sessions")
    } else {
        return Vec::new();
    };

    let mut roots = vec![sessions_root.clone()];
    if sessions_root.file_name().and_then(|n| n.to_str()) == Some("sessions")
        && let Some(parent) = sessions_root.parent()
    {
        roots.push(parent.join("archived_sessions"));
    }
    roots
}

#[must_use]
pub fn resolve_codex_roots(options: &CodexScanOptions) -> Vec<PathBuf> {
    resolve_codex_roots_with(options, &BTreeMap::new(), None)
}

fn collect_jsonl_files_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(root_meta) = fs::symlink_metadata(dir) else {
        return out;
    };
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return out;
    }
    let Ok(canon_root) = fs::canonicalize(dir) else {
        return out;
    };
    let mut visited = BTreeSet::new();
    visited.insert(canon_root.clone());
    let mut stack = vec![dir.to_path_buf()];

    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        let mut subdirs = Vec::new();
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                let Ok(canon_sub) = fs::canonicalize(&path) else {
                    continue;
                };
                if canon_sub.starts_with(&canon_root) && visited.insert(canon_sub) {
                    subdirs.push(path);
                }
            } else if meta.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let Ok(canon_file) = fs::canonicalize(&path) else {
                    continue;
                };
                if canon_file.starts_with(&canon_root) {
                    files.push(path);
                }
            }
        }
        subdirs.sort();
        files.sort();
        out.extend(files);
        for sub in subdirs.into_iter().rev() {
            stack.push(sub);
        }
    }
    out
}
