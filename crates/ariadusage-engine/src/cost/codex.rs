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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_session_id: Option<String>,
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
            .field("request_session_id", &self.request_session_id)
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

/// Buffered line for deferred subagent classification or unresolved fork baseline replay
/// (`CostUsageScanner.CodexBufferedFastLine`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexBufferedLine {
    pub line_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<i64>,
    pub end_offset: u64,
    pub raw: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_mirror: Option<CodexNearMirror>,
}

/// Counter advance classification for delayed mirror pairing (`CostUsageScanner.CodexNearMirror.CounterAdvance`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodexCounterAdvance {
    Exact,
    Bounded,
    Intervening,
    Unknown,
}

/// Near-window mirror evidence for `token_usage_record` / `token_count` pairing (`CostUsageScanner.CodexNearMirror`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexNearMirror {
    pub key: Option<String>,
    pub timestamp_ms: i64,
    pub counter_advance: CodexCounterAdvance,
}

impl CodexNearMirror {
    #[must_use]
    pub fn matches(&self, other: Option<&Self>, allow_delayed: bool) -> bool {
        let Some(other) = other else {
            return false;
        };
        let Some(ref key) = self.key else {
            return false;
        };
        if other.key.as_ref() != Some(key)
            || self.counter_advance == CodexCounterAdvance::Intervening
        {
            return false;
        }
        if (other.timestamp_ms - self.timestamp_ms).abs() <= NEAR_MIRROR_MAX_DELAY_MS {
            return true;
        }
        allow_delayed
            && self.timestamp_ms >= other.timestamp_ms
            && self.counter_advance == CodexCounterAdvance::Exact
            && other.counter_advance == CodexCounterAdvance::Exact
    }
}

/// Persisted request-ledger and mirror-window state (`CostUsageScanner.CodexRequestLedgerState`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexRequestLedgerState {
    pub session_id: Option<String>,
    pub active_turn_id: Option<String>,
    pub turn_models: BTreeMap<String, String>,
    pub thread_priority: Option<bool>,
    pub priority_turn_ids: BTreeSet<String>,
    pub counted_usage: Option<CodexTotals>,
    pub response_ids: BTreeSet<String>,
    pub mirrored_responses: BTreeMap<String, String>,
    pub legacy_row_indices: BTreeMap<String, usize>,
    pub pending_ledger_mirrors: Option<BTreeSet<String>>,
    pub pending_ledger_response_id: Option<String>,
    pub pending_ledger_near_mirror: Option<CodexNearMirror>,
    pub pending_ledger_pricing_response_id: Option<String>,
    pub pending_legacy_mirrors: Option<BTreeSet<String>>,
    pub pending_legacy_row_index: Option<usize>,
    pub pending_legacy_near_mirror: Option<CodexNearMirror>,
    pub last_ledger_total: Option<CodexTotals>,
    pub last_legacy_total: Option<CodexTotals>,
}

impl CodexRequestLedgerState {
    pub fn clear_pending_mirrors(&mut self) {
        self.pending_ledger_mirrors = None;
        self.pending_ledger_response_id = None;
        self.pending_ledger_near_mirror = None;
        self.pending_ledger_pricing_response_id = None;
        self.pending_legacy_mirrors = None;
        self.pending_legacy_row_index = None;
        self.pending_legacy_near_mirror = None;
    }

    pub fn remember_mirrors<I, S>(&mut self, keys: I, response_id: &str)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for key in keys {
            let k = key.as_ref();
            if !k.is_empty() {
                self.mirrored_responses
                    .insert(k.to_owned(), response_id.to_owned());
            }
        }
    }

    pub fn remember_legacy_counter(&mut self, total: Option<CodexTotals>, when: bool) {
        if when && let Some(total) = total {
            self.last_legacy_total = Some(total);
        }
    }

    pub fn begin_legacy_observation(
        &mut self,
        keys: &BTreeSet<String>,
        snapshot: &str,
        near: Option<&CodexNearMirror>,
    ) -> Option<String> {
        if let Some(existing) = self.mirrored_responses.get(snapshot).cloned() {
            if self.pending_ledger_response_id.as_deref() == Some(existing.as_str()) {
                let pricing_owner = self.pending_ledger_pricing_response_id.clone();
                self.clear_pending_mirrors();
                return pricing_owner;
            }
            return None;
        }
        let matches_pending = self
            .pending_ledger_mirrors
            .as_ref()
            .is_some_and(|p| !p.is_disjoint(keys))
            || near.is_some_and(|n| n.matches(self.pending_ledger_near_mirror.as_ref(), true));
        let Some(response_id) = (if matches_pending {
            self.pending_ledger_response_id.clone()
        } else {
            None
        }) else {
            self.clear_pending_mirrors();
            return None;
        };
        let pricing_owner = self.pending_ledger_pricing_response_id.clone();
        self.clear_pending_mirrors();
        self.remember_mirrors(std::iter::once(snapshot), &response_id);
        self.remember_mirrors(keys.iter(), &response_id);
        pricing_owner
    }
}

/// Source pricing identity key for retaining row-level pricing across re-scans (`CostUsageScanner.CodexSourcePricingKey`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CodexSourcePricingKey {
    pub day: String,
    pub model: String,
    pub turn_id: Option<String>,
    pub input: i64,
    pub cached: i64,
    pub output: i64,
    pub response_id: Option<String>,
}

impl CodexSourcePricingKey {
    #[must_use]
    pub fn from_row(row: &CodexUsageRow) -> Option<Self> {
        if row.input <= 0 && row.cached <= 0 && row.output <= 0 {
            return None;
        }
        Some(Self {
            day: row.day.clone(),
            model: row.model.clone(),
            turn_id: row.turn_id.clone(),
            input: row.input,
            cached: row.cached,
            output: row.output,
            response_id: row.response_id.clone(),
        })
    }

    #[must_use]
    pub fn match_keys(row: &CodexUsageRow) -> Vec<Self> {
        let Some(exact) = Self::from_row(row) else {
            return Vec::new();
        };
        let mut keys = vec![exact.clone()];
        if exact.response_id.is_some() {
            keys.push(Self {
                response_id: None,
                ..exact.clone()
            });
        }
        if exact.turn_id.is_some() {
            keys.push(Self {
                turn_id: None,
                response_id: None,
                ..exact
            });
        }
        keys
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_counted_totals: Option<CodexTotals>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_raw_totals_baseline: Option<CodexTotals>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_raw_totals_watermark: Option<CodexTotals>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seen_raw_totals: Vec<CodexTotals>,
    #[serde(default)]
    pub has_divergent_totals: bool,
    #[serde(default)]
    pub has_interleaved_totals: bool,
    pub last_codex_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_service_tier: Option<CodexServiceTier>,
    #[serde(default)]
    pub next_usage_row_index: usize,
    pub session_id: Option<String>,
    pub forked_from_id: Option<String>,
    pub fork_timestamp: Option<String>,
    pub depends_on_parent_totals: bool,
    pub has_unresolved_fork_baseline: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_inherited_totals: Option<CodexTotals>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_inherited_totals: Option<CodexTotals>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buffered_subagent_lines: Vec<CodexBufferedLine>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buffered_unresolved_fork_lines: Vec<CodexBufferedLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_ledger_state: Option<CodexRequestLedgerState>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub replaced_legacy_row_indices: BTreeSet<usize>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ledger_legacy_pricing_keys: BTreeMap<usize, CodexSourcePricingKey>,
    pub codex_session: Option<CodexSessionMetadata>,
}

/// Resolution result for a fork parent's cumulative baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexForkBaseline {
    Resolved(Option<CodexTotals>),
    Unresolved,
}

/// Optional resume and bounded-read state for incremental `parse_codex_file_with_state` calls.
#[derive(Debug, Clone, Default)]
pub struct CodexParseResumeOptions<'a> {
    pub start_offset: u64,
    pub max_bytes_to_read: Option<u64>,
    pub initial_model: Option<&'a str>,
    pub initial_session_id: Option<&'a str>,
    pub initial_totals: Option<CodexTotals>,
    pub initial_raw_totals_baseline: Option<CodexTotals>,
    pub initial_raw_totals_watermark: Option<CodexTotals>,
    pub initial_seen_raw_totals: &'a [CodexTotals],
    pub initial_has_divergent_totals: bool,
    pub initial_has_interleaved_totals: bool,
    pub initial_turn_id: Option<&'a str>,
    pub initial_service_tier: Option<CodexServiceTier>,
    pub initial_next_usage_row_index: usize,
    pub initial_forked_from_id: Option<&'a str>,
    pub initial_fork_timestamp: Option<&'a str>,
    pub initial_resolved_inherited_totals: Option<Option<CodexTotals>>,
    pub initial_remaining_inherited_totals: Option<Option<CodexTotals>>,
    pub initial_buffered_subagent_lines: Option<&'a [CodexBufferedLine]>,
    pub initial_buffered_unresolved_fork_lines: Option<&'a [CodexBufferedLine]>,
    pub initial_request_ledger_state: Option<&'a CodexRequestLedgerState>,
    pub initial_request_ledger_rows: &'a [CodexUsageRow],
}

/// Maximum line bytes for normal JSONL line scanning in `parse_codex_file` (`512 KiB`).
pub const CODEX_DEFAULT_MAX_LINE_BYTES: usize = 512 * 1024;

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
    inherited_totals_resolver: F,
) -> CodexParseResult
where
    F: FnMut(&str, Option<&str>) -> CodexForkBaseline,
{
    let resume = CodexParseResumeOptions {
        start_offset,
        initial_model,
        initial_totals,
        initial_turn_id,
        ..Default::default()
    };
    parse_codex_file_with_state(
        file_path,
        range,
        &resume,
        priority_turns,
        session_tiers,
        catalog,
        inherited_totals_resolver,
    )
}

/// Resumes parsing a Codex JSONL session file from a previous `CodexParseResult`.
#[allow(clippy::too_many_arguments)]
pub fn resume_codex_file<F>(
    file_path: &Path,
    range: &DayRange,
    previous: &CodexParseResult,
    max_bytes_to_read: Option<u64>,
    retained_rows: Option<&[CodexUsageRow]>,
    priority_turns: &BTreeMap<String, CodexPriorityTurnMetadata>,
    session_tiers: &BTreeMap<String, CodexServiceTier>,
    catalog: Option<&ModelsDevCatalog>,
    inherited_totals_resolver: F,
) -> CodexParseResult
where
    F: FnMut(&str, Option<&str>) -> CodexForkBaseline,
{
    let resume = CodexParseResumeOptions {
        start_offset: previous.parsed_bytes,
        max_bytes_to_read,
        initial_model: previous.last_model.as_deref(),
        initial_session_id: previous.session_id.as_deref(),
        initial_totals: previous.last_counted_totals.or(previous.last_totals),
        initial_raw_totals_baseline: previous.last_raw_totals_baseline.or(previous.last_totals),
        initial_raw_totals_watermark: previous.last_raw_totals_watermark.or(previous.last_totals),
        initial_seen_raw_totals: &previous.seen_raw_totals,
        initial_has_divergent_totals: previous.has_divergent_totals,
        initial_has_interleaved_totals: previous.has_interleaved_totals,
        initial_turn_id: previous.last_codex_turn_id.as_deref(),
        initial_service_tier: previous.last_service_tier,
        initial_next_usage_row_index: previous.next_usage_row_index,
        initial_forked_from_id: previous.forked_from_id.as_deref(),
        initial_fork_timestamp: previous.fork_timestamp.as_deref(),
        initial_resolved_inherited_totals: if previous.forked_from_id.is_some()
            && !previous.has_unresolved_fork_baseline
            && previous
                .codex_session
                .as_ref()
                .is_none_or(|m| !m.is_subagent_thread)
        {
            Some(previous.resolved_inherited_totals)
        } else {
            None
        },
        initial_remaining_inherited_totals: if previous.forked_from_id.is_some()
            && !previous.has_unresolved_fork_baseline
            && previous
                .codex_session
                .as_ref()
                .is_none_or(|m| !m.is_subagent_thread)
        {
            Some(previous.remaining_inherited_totals)
        } else {
            None
        },
        initial_buffered_subagent_lines: if previous.buffered_subagent_lines.is_empty() {
            None
        } else {
            Some(&previous.buffered_subagent_lines)
        },
        initial_buffered_unresolved_fork_lines: if previous
            .buffered_unresolved_fork_lines
            .is_empty()
        {
            None
        } else {
            Some(&previous.buffered_unresolved_fork_lines)
        },
        initial_request_ledger_state: previous.request_ledger_state.as_ref(),
        initial_request_ledger_rows: retained_rows.unwrap_or(&previous.rows),
    };
    parse_codex_file_with_state(
        file_path,
        range,
        &resume,
        priority_turns,
        session_tiers,
        catalog,
        inherited_totals_resolver,
    )
}

const MIRROR_BOUNDARY_TYPE: &str = "__mirror_boundary__";
const MIRROR_BOUNDARY_JSON: &str = r#"{"type":"__mirror_boundary__"}"#;

/// Parses a single Codex JSONL session file with optional bounded byte budget and resume state.
///
/// Mirrors `CostUsageScanner.parseCodexFileCancellable`.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn parse_codex_file_with_state<F>(
    file_path: &Path,
    range: &DayRange,
    resume: &CodexParseResumeOptions<'_>,
    priority_turns: &BTreeMap<String, CodexPriorityTurnMetadata>,
    session_tiers: &BTreeMap<String, CodexServiceTier>,
    catalog: Option<&ModelsDevCatalog>,
    mut inherited_totals_resolver: F,
) -> CodexParseResult
where
    F: FnMut(&str, Option<&str>) -> CodexForkBaseline,
{
    let start_offset = resume.start_offset;
    let file_meta = codex_file_metadata(file_path);
    let current_file_size = file_meta.size;
    let file_identity = file_meta
        .file_id
        .unwrap_or_else(|| file_path.to_string_lossy().into_owned());
    let Ok(mut file) = crate::cost::claude::open_regular_file_nofollow(file_path) else {
        return CodexParseResult::default();
    };
    if start_offset > 0 && file.seek(SeekFrom::Start(start_offset)).is_err() {
        return CodexParseResult::default();
    }

    let bytes_to_target = current_file_size.saturating_sub(start_offset);
    let bounded_bytes_to_read = resume
        .max_bytes_to_read
        .map_or(bytes_to_target, |b| b.min(bytes_to_target));

    let mut reader = BufReader::new(file);
    let mut offset = start_offset;
    let mut raw_lines: Vec<(u64, String, bool, Option<CodexNearMirror>, bool)> = Vec::new();
    let mut byte_buf: Vec<u8> = Vec::with_capacity(4096);
    let mut bytes_consumed = 0u64;

    if start_offset > 0
        && let Some(sub_lines) = resume.initial_buffered_subagent_lines
    {
        for bl in sub_lines {
            raw_lines.push((
                bl.end_offset,
                bl.raw.clone(),
                false,
                bl.ledger_mirror.clone(),
                true,
            ));
        }
    }

    while bytes_consumed < bounded_bytes_to_read {
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
        bytes_consumed += bytes_read as u64;
        let trimmed_bytes = byte_buf.strip_suffix(b"\r").unwrap_or(byte_buf.as_slice());
        if trimmed_bytes.is_empty() && !was_truncated {
            continue;
        }
        let text = String::from_utf8_lossy(trimmed_bytes).into_owned();
        raw_lines.push((offset, text, was_truncated, None, false));
    }

    let has_unconsumed_physical_tail = offset < current_file_size;

    // Seed session_meta from resume state if starting past offset 0
    let mut session_meta: Option<CodexSessionMetadata> = resume
        .initial_session_id
        .filter(|s| !s.is_empty())
        .map(|sid| CodexSessionMetadata {
            session_id: sid.to_owned(),
            forked_from_id: resume.initial_forked_from_id.map(String::from),
            fork_timestamp: resume.initial_fork_timestamp.map(String::from),
            is_subagent_thread: resume.initial_buffered_subagent_lines.is_some(),
            subagent_history_start_ordinal: None,
            history_base_thread_id: None,
            request_session_id: resume
                .initial_request_ledger_state
                .and_then(|l| l.session_id.clone()),
            cwd: None,
            title: None,
            started_at_unix_ms: None,
            latest_activity_unix_ms: None,
        });
    let mut did_capture_leaf_metadata = session_meta.is_some();

    // Also inspect any buffered unresolved fork lines for leaf session_meta when resuming at start_offset > 0
    if start_offset > 0
        && let Some(fork_lines) = resume.initial_buffered_unresolved_fork_lines
    {
        for bl in fork_lines {
            if let Some(v) = parse_codex_line_json(&bl.raw)
                && v.get("type").and_then(Value::as_str) == Some("session_meta")
                && let Some(meta) = extract_session_metadata(&v).filter(|m| !m.session_id.is_empty())
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
        }
    }

    let mut parsed_lines: Vec<PredecodedCodexLine> = Vec::with_capacity(raw_lines.len());
    for (idx, (end_offset, raw, was_truncated, deferred_mirror, is_replayed_buffer)) in
        raw_lines.into_iter().enumerate()
    {
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
            && let Some(meta) = extract_session_metadata(v).filter(|m| !m.session_id.is_empty())
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
        parsed_lines.push((
            idx,
            end_offset,
            ordinal,
            was_truncated,
            raw,
            val,
            deferred_mirror,
            is_replayed_buffer,
        ));
    }

    let is_subagent_session = session_meta.as_ref().is_some_and(|m| m.is_subagent_thread);
    let defer_subagent_until_eof = is_subagent_session && has_unconsumed_physical_tail;

    let mut subagent_counter_semantics: Option<CodexSubagentCounterSemantics> = None;
    let mut owned_suffix: Option<CodexSubagentOwnedSuffix> = None;
    let mut uses_local_subagent_boundary = false;
    let mut candidate_boundary_depends_on_parent_totals = false;
    let mut suppress_unowned_copied_prefix = false;
    let mut cached_parent_resolution: Option<CodexForkBaseline> = None;

    if let Some(ref mut meta) = session_meta
        && meta.is_subagent_thread
        && !defer_subagent_until_eof
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

    if let Some(pre_resolved) = resume.initial_resolved_inherited_totals {
        inherited_totals = pre_resolved;
        has_unresolved_fork_baseline = false;
    } else if !defer_subagent_until_eof
        && let Some(ref meta) = session_meta
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

    let buffer_subagent_lines = defer_subagent_until_eof
        || (is_subagent_session
            && subagent_counter_semantics == Some(CodexSubagentCounterSemantics::CopiedPrefix)
            && owned_suffix.is_none()
            && has_unresolved_fork_baseline);

    let has_fork_parent = resume.initial_resolved_inherited_totals.is_some()
        || session_meta
            .as_ref()
            .and_then(|m| m.forked_from_id.as_ref())
            .is_some();
    let depends_on_parent_totals = has_fork_parent
        && (candidate_boundary_depends_on_parent_totals
            || (subagent_counter_semantics != Some(CodexSubagentCounterSemantics::Independent)
                && !uses_local_subagent_boundary));

    let is_forked_session = has_fork_parent;

    // If we had buffered unresolved fork lines from an earlier pass and the parent is now resolved,
    // prepend them before the newly read lines so they replay in order.
    let mut buffered_unresolved_fork_lines: Vec<CodexBufferedLine> = Vec::new();
    if start_offset > 0
        && let Some(prev_fork_lines) = resume.initial_buffered_unresolved_fork_lines
    {
        if has_unresolved_fork_baseline {
            buffered_unresolved_fork_lines.extend(prev_fork_lines.iter().cloned());
        } else {
            let mut replayed_prefix = Vec::with_capacity(prev_fork_lines.len() + parsed_lines.len());
            for (idx, bl) in prev_fork_lines.iter().enumerate() {
                let val = parse_codex_line_json(&bl.raw);
                let ordinal = val
                    .as_ref()
                    .and_then(Value::as_object)
                    .and_then(|o| o.get("ordinal"))
                    .and_then(Value::as_i64);
                replayed_prefix.push((
                    idx,
                    bl.end_offset,
                    ordinal,
                    false,
                    bl.raw.clone(),
                    val,
                    bl.ledger_mirror.clone(),
                    true,
                ));
            }
            let base_idx = replayed_prefix.len();
            for (_, end_offset, ordinal, was_truncated, raw, val, deferred_mirror, is_replayed) in
                parsed_lines
            {
                let next_idx = base_idx + replayed_prefix.len() - base_idx;
                replayed_prefix.push((
                    next_idx,
                    end_offset,
                    ordinal,
                    was_truncated,
                    raw,
                    val,
                    deferred_mirror,
                    is_replayed,
                ));
            }
            parsed_lines = replayed_prefix;
        }
    }

    let mut current_model: Option<String> = if owned_suffix.is_some() {
        None
    } else {
        resume.initial_model.map(String::from)
    };
    let mut current_turn_id: Option<String> = if owned_suffix.is_some() {
        None
    } else {
        resume.initial_turn_id.map(String::from)
    };
    let mut current_service_tier: Option<CodexServiceTier> = if owned_suffix.is_some() {
        None
    } else {
        resume.initial_service_tier
    }
    .or_else(|| {
        session_meta
            .as_ref()
            .and_then(|m| session_tiers.get(&m.session_id).copied())
    });
    let mut last_accepted_day_key: Option<DayKey> = None;
    let mut last_accepted_ts_ms: Option<i64> = None;
    let mut last_accepted_ts_str: Option<String> = None;

    let initial_counted = if owned_suffix.is_some() {
        None
    } else {
        resume.initial_totals
    };
    let initial_raw_baseline = subagent_raw_baseline.or(
        if owned_suffix.is_some() {
            None
        } else {
            resume.initial_raw_totals_baseline.or(resume.initial_totals)
        },
    );
    let initial_watermark = subagent_raw_baseline.or(
        if owned_suffix.is_some() {
            None
        } else {
            resume
                .initial_raw_totals_watermark
                .or(resume.initial_raw_totals_baseline)
                .or(resume.initial_totals)
        },
    );
    let mut accumulator = CodexSnapshotAccumulator {
        counted_totals: initial_counted,
        raw_totals_baseline: initial_raw_baseline,
        saw_divergent_totals: if owned_suffix.is_some() {
            false
        } else {
            resume.initial_has_divergent_totals
        },
        tracker: CodexTotalsTracker {
            watermark: initial_watermark,
            seen_raw_totals: if owned_suffix.is_some() {
                Vec::new()
            } else {
                resume.initial_seen_raw_totals.to_vec()
            },
            saw_interleaved_totals: if owned_suffix.is_some() {
                false
            } else {
                resume.initial_has_interleaved_totals
            },
        },
    };
    let mut raised_continued_counter = initial_counted.is_some();
    let mut remaining_inherited_totals: Option<CodexTotals> =
        if let Some(rem) = resume.initial_remaining_inherited_totals {
            rem
        } else if initial_counted.is_some() {
            None
        } else {
            inherited_totals
        };
    let mut fork_raw_baseline: Option<CodexTotals> = initial_raw_baseline;
    let mut fork_counted_totals: Option<CodexTotals> = initial_counted;
    let mut fork_saw_divergent = if owned_suffix.is_some() {
        false
    } else {
        resume.initial_has_divergent_totals
    };

    let mut days: BTreeMap<String, BTreeMap<String, Vec<i64>>> = BTreeMap::new();
    let mut rows: Vec<CodexUsageRow> = Vec::new();
    let mut token_snapshots: Vec<CodexTokenSnapshot> = Vec::new();
    let mut ledger = resume.initial_request_ledger_state.cloned().unwrap_or_default();
    if ledger.session_id.is_none()
        && let Some(ref meta) = session_meta
    {
        ledger.session_id = meta
            .request_session_id
            .clone()
            .or_else(|| Some(meta.session_id.clone()).filter(|s| !s.is_empty()));
    }
    let retained_rows_by_index: BTreeMap<usize, CodexUsageRow> = resume
        .initial_request_ledger_rows
        .iter()
        .filter_map(|r| r.event_index.map(|idx| (idx, r.clone())))
        .collect();
    let mut replaced_legacy_row_indices: BTreeSet<usize> = BTreeSet::new();
    let mut ledger_legacy_pricing_keys: BTreeMap<usize, CodexSourcePricingKey> = BTreeMap::new();
    let mut buffered_subagent_lines: Vec<CodexBufferedLine> = Vec::new();
    let mut event_index = resume.initial_next_usage_row_index;
    let mut seen_leaf_session_meta_in_pass = start_offset > 0 && resume.initial_session_id.is_some();

    for (
        line_idx,
        end_offset,
        ordinal,
        was_truncated,
        raw,
        val_opt,
        deferred_mirror,
        is_replayed_buffer,
    ) in parsed_lines
    {
        if !is_replayed_buffer
            && !was_truncated
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

        if buffer_subagent_lines {
            // Bare usage is counted immediately even while buffering subagent lines;
            // replace its buffered entry with a mirror boundary so replay does not double-count it.
            if !was_truncated
                && let Some(ref val) = val_opt
                && let Some(obj) = val.as_object()
                && obj.get("type").is_none()
                && let Some((bare_delta, bare_model)) = extract_bare_usage_row(obj)
                && !bare_delta.is_zero()
            {
                let line_ts_str = obj.get("timestamp").and_then(Value::as_str);
                let line_day_and_ms = line_ts_str
                    .and_then(|ts| timestamp::day_key_from_timestamp(ts, range.timezone.clone()));
                if let Some((ref dk, ms)) = line_day_and_ms {
                    last_accepted_day_key = Some(dk.clone());
                    last_accepted_ts_ms = Some(ms);
                    last_accepted_ts_str = line_ts_str.map(String::from);
                }
                if let Some(day_key) = line_day_and_ms
                    .as_ref()
                    .map(|(dk, _)| dk.clone())
                    .or_else(|| last_accepted_day_key.clone())
                {
                    let ts_ms = line_day_and_ms
                        .as_ref()
                        .map(|(_, ms)| *ms)
                        .or(last_accepted_ts_ms);
                    ledger.clear_pending_mirrors();
                    let effective_raw = bare_model
                        .as_deref()
                        .or(current_model.as_deref())
                        .unwrap_or("gpt-5");
                    let normalized_model = pricing::normalize_codex_model(effective_raw);
                    let pricing_model = resolve_codex_priority_pricing_model(
                        &normalized_model,
                        current_turn_id.as_deref(),
                        priority_turns,
                        catalog,
                    );
                    let session_id_opt = session_meta
                        .as_ref()
                        .map(|m| m.session_id.as_str())
                        .filter(|s| !s.is_empty());
                    let bare_tier = pricing::normalize_codex_service_tier(
                        obj.get("service_tier").and_then(Value::as_str),
                    )
                    .or(current_service_tier);
                    let is_fast = is_codex_turn_priority(
                        bare_tier,
                        current_turn_id.as_deref(),
                        session_id_opt,
                        &ledger.priority_turn_ids,
                        priority_turns,
                    );
                    let this_event_index = event_index;
                    event_index += 1;
                    record_codex_row(
                        day_key.as_str(),
                        &normalized_model,
                        &pricing_model,
                        Some(effective_raw),
                        current_turn_id.as_deref(),
                        Some(this_event_index),
                        ts_ms,
                        bare_delta,
                        is_fast,
                        None,
                        None,
                        session_id_opt,
                        Some(&file_identity),
                        None,
                        range,
                        catalog,
                        &mut days,
                        &mut rows,
                    );
                    buffered_subagent_lines.push(CodexBufferedLine {
                        line_index: line_idx,
                        ordinal,
                        end_offset,
                        raw: MIRROR_BOUNDARY_JSON.to_owned(),
                        ledger_mirror: None,
                    });
                    continue;
                }
            }
            if !was_truncated
                && let Some(ref val) = val_opt
                && let Some(obj) = val.as_object()
            {
                let line_ts_str = obj.get("timestamp").and_then(Value::as_str);
                let line_day_and_ms = line_ts_str
                    .and_then(|ts| timestamp::day_key_from_timestamp(ts, range.timezone.clone()));
                if let Some((ref dk, ms)) = line_day_and_ms {
                    last_accepted_day_key = Some(dk.clone());
                    last_accepted_ts_ms = Some(ms);
                    last_accepted_ts_str = line_ts_str.map(String::from);
                }
                if obj.get("type").and_then(Value::as_str) == Some("turn_context")
                    && line_day_and_ms.is_some()
                    && let Some(payload) = obj.get("payload").and_then(Value::as_object)
                {
                    if let Some(m) = extract_turn_context_model(payload) {
                        current_model = if m.is_empty() { None } else { Some(m) };
                    }
                    if let Some(tid) = extract_codex_turn_id(payload) {
                        current_turn_id = Some(tid);
                    }
                }
            }
            if !was_truncated {
                buffered_subagent_lines.push(CodexBufferedLine {
                    line_index: line_idx,
                    ordinal,
                    end_offset,
                    raw,
                    ledger_mirror: None,
                });
            }
            continue;
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
        if record_type == Some(MIRROR_BOUNDARY_TYPE) {
            ledger.clear_pending_mirrors();
            if has_unresolved_fork_baseline {
                buffered_unresolved_fork_lines.push(CodexBufferedLine {
                    line_index: line_idx,
                    ordinal,
                    end_offset,
                    raw,
                    ledger_mirror: None,
                });
            }
            continue;
        }

        if matches!(
            record_type,
            Some("turn_context" | "inter_agent_communication_metadata" | "token_usage_record")
        ) && line_day_and_ms.is_none()
        {
            continue;
        }

        if let Some((ref dk, ms)) = line_day_and_ms {
            last_accepted_day_key = Some(dk.clone());
            last_accepted_ts_ms = Some(ms);
            last_accepted_ts_str = line_ts_str.map(String::from);
            if let Some(ref mut meta) = session_meta {
                meta.latest_activity_unix_ms =
                    Some(meta.latest_activity_unix_ms.map_or(ms, |prev| prev.max(ms)));
                if meta.started_at_unix_ms.is_none() {
                    meta.started_at_unix_ms = Some(ms);
                }
            }
        }

        let mut line_ledger_mirror: Option<CodexNearMirror> = None;
        let mut line_replaced_by_boundary = false;

        match record_type {
            Some("session_meta") => {
                if let Some(meta) = extract_session_metadata(&val).filter(|m| !m.session_id.is_empty()) {
                    if seen_leaf_session_meta_in_pass {
                        if let Some(ref leaf) = session_meta
                            && CodexSubagentRolloutShape::same_concrete_session_id(
                                Some(&leaf.session_id),
                                Some(&meta.session_id),
                            )
                        {
                            ledger.clear_pending_mirrors();
                            ledger.last_ledger_total = None;
                            ledger.last_legacy_total = None;
                            if ledger.session_id.is_none() {
                                ledger.session_id = meta
                                    .request_session_id
                                    .or(Some(meta.session_id));
                            }
                        } else if !is_forked_session && !is_subagent_session {
                            session_meta = Some(meta.clone());
                            ledger.session_id = meta
                                .request_session_id
                                .or(Some(meta.session_id));
                            ledger.clear_pending_mirrors();
                            ledger.legacy_row_indices.clear();
                        }
                    } else {
                        seen_leaf_session_meta_in_pass = true;
                        if ledger.session_id.is_none() {
                            ledger.session_id = meta
                                .request_session_id
                                .or(Some(meta.session_id));
                        }
                    }
                }
                if let Some(payload) = obj.get("payload").and_then(Value::as_object)
                    && let Some(tier) = pricing::normalize_codex_service_tier(
                        payload.get("service_tier").and_then(Value::as_str),
                    )
                {
                    current_service_tier = Some(tier);
                }
            }
            Some("turn_context") => {
                ledger.clear_pending_mirrors();
                if let Some(payload) = obj.get("payload").and_then(Value::as_object) {
                    let turn_id_in_ctx = extract_codex_turn_id(payload);
                    if let Some(ref tid) = turn_id_in_ctx {
                        current_turn_id = Some(tid.clone());
                        ledger.active_turn_id = Some(tid.clone());
                    }
                    if let Some(m) = extract_turn_context_model(payload) {
                        current_model = if m.is_empty() { None } else { Some(m) };
                    }
                    if let (Some(tid), Some(model)) = (&turn_id_in_ctx, &current_model) {
                        ledger.turn_models.insert(tid.clone(), model.clone());
                    }
                    if let Some(tier) = pricing::normalize_codex_service_tier(
                        payload.get("service_tier").and_then(Value::as_str),
                    ) {
                        current_service_tier = Some(tier);
                    }
                    if let Some(ref mut meta) = session_meta {
                        if let Some(cwd) = payload
                            .get("cwd")
                            .or_else(|| payload.get("current_working_directory"))
                            .or_else(|| payload.get("currentWorkingDirectory"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                        {
                            meta.cwd = Some(cwd.to_owned());
                        }
                        if let Some(title) = payload
                            .get("title")
                            .or_else(|| payload.get("name"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                        {
                            meta.title = Some(title.to_owned());
                        }
                    }
                }
            }
            Some("token_usage_record") => {
                if !suppress_unowned_copied_prefix {
                    line_ledger_mirror = process_token_usage_record(
                        obj,
                        line_ts_str.unwrap_or_default(),
                        line_day_and_ms.as_ref(),
                        session_meta.as_ref(),
                        &file_identity,
                        current_model.as_deref(),
                        current_turn_id.as_deref(),
                        current_service_tier,
                        priority_turns,
                        range,
                        catalog,
                        has_unresolved_fork_baseline,
                        deferred_mirror.as_ref(),
                        &retained_rows_by_index,
                        &mut event_index,
                        &mut ledger,
                        &mut replaced_legacy_row_indices,
                        &mut ledger_legacy_pricing_keys,
                        &mut days,
                        &mut rows,
                    );
                }
            }
            Some("event_msg") => {
                if line_day_and_ms.is_none() {
                    continue;
                }
                let Some(payload) = obj.get("payload").and_then(Value::as_object) else {
                    continue;
                };
                let payload_type = payload.get("type").and_then(Value::as_str);
                if payload_type == Some("thread_settings_applied") {
                    // LOW 1: match upstream CostUsageScanner.swift:3766-3770 and 4901-4902:
                    // only inspect `payload["thread_settings"]["service_tier"]`, trim whitespace,
                    // and check exact equality `== "priority"` (no `"fast"`, no case-folding).
                    let tier_raw = payload
                        .get("thread_settings")
                        .and_then(Value::as_object)
                        .and_then(|ts| ts.get("service_tier"))
                        .and_then(Value::as_str)
                        .map(str::trim);
                    let is_prio = tier_raw == Some("priority");
                    ledger.thread_priority = Some(is_prio);
                } else if payload_type == Some("task_started") || payload_type == Some("turn_started") {
                    ledger.clear_pending_mirrors();
                    let tid = extract_codex_turn_id(payload);
                    current_turn_id = tid.clone();
                    let turn_key = tid.unwrap_or_default();
                    if ledger.thread_priority == Some(true) {
                        ledger.priority_turn_ids.insert(turn_key);
                    } else {
                        ledger.priority_turn_ids.remove(&turn_key);
                    }
                } else if payload_type == Some("token_count") && line_idx >= skip_token_lines_before {
                    if let Some(info) = payload.get("info").and_then(Value::as_object) {
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
                        let record_turn_id = extract_codex_turn_id(payload);
                        let last_usage = info
                            .get("last_token_usage")
                            .and_then(Value::as_object)
                            .and_then(parse_codex_totals_map);
                        let total_usage = info
                            .get("total_token_usage")
                            .and_then(Value::as_object)
                            .and_then(parse_codex_totals_map);

                        if (last_usage.is_some() || total_usage.is_some())
                            && !suppress_unowned_copied_prefix
                            && let Some((ref day_key, ts_ms)) = line_day_and_ms
                        {
                            let ts_str = line_ts_str.unwrap_or_default();
                            let mirror_turn_id = record_turn_id
                                .as_deref()
                                .or(current_turn_id.as_deref())
                                .or(ledger.active_turn_id.as_deref())
                                .map(String::from);
                            let observation_key = make_codex_mirror_key(
                                mirror_turn_id.as_deref(),
                                last_usage,
                                total_usage,
                                Some(ts_str),
                            );
                            let already_observed = (total_usage.is_some()
                                && ledger.legacy_row_indices.contains_key(&observation_key))
                                || (last_usage.is_none()
                                    && ledger.mirrored_responses.contains_key(&observation_key));
                            if !already_observed {
                                if has_unresolved_fork_baseline {
                                    ledger.clear_pending_mirrors();
                                } else {
                                    let original_response_id = if ledger.pending_ledger_near_mirror.is_some() {
                                        ledger.pending_ledger_response_id.clone()
                                    } else {
                                        None
                                    };
                                    let mut mirror = observe_legacy_mirror(
                                        &mut ledger,
                                        last_usage,
                                        mirror_turn_id.as_deref(),
                                        total_usage,
                                        ts_str,
                                        ts_ms,
                                    );
                                    let owner = mirror
                                        .as_ref()
                                        .and_then(|m| ledger.mirrored_responses.get(&m.snapshot))
                                        .cloned();
                                    ledger.remember_legacy_counter(
                                        total_usage,
                                        original_response_id.is_some() && owner == original_response_id,
                                    );

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

                                    let (delta, skipped_seen_or_stale) = if is_forked_session {
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
                                        apply_accumulator_event_with_skip(
                                            &mut accumulator,
                                            last_usage,
                                            total_usage,
                                        )
                                    };

                                    if skipped_seen_or_stale {
                                        if delta.is_zero()
                                            && total_usage.is_some()
                                            && total_usage == ledger.last_ledger_total
                                            && let Some(ref resp_id) =
                                                ledger.pending_ledger_response_id.clone()
                                        {
                                            let row_turn_id = record_turn_id
                                                .as_deref()
                                                .or(current_turn_id.as_deref());
                                            let session_id_opt = session_meta
                                                .as_ref()
                                                .map(|m| m.session_id.as_str())
                                                .filter(|s| !s.is_empty());
                                            let is_fast = is_codex_turn_priority(
                                                current_service_tier,
                                                row_turn_id,
                                                session_id_opt,
                                                &ledger.priority_turn_ids,
                                                priority_turns,
                                            );
                                            if is_fast {
                                                heal_ledger_row_from_legacy_mirror(
                                                    resp_id,
                                                    None,
                                                    None,
                                                    true,
                                                    catalog,
                                                    &mut days,
                                                    &mut rows,
                                                );
                                            }
                                        }
                                        if mirror.is_none() {
                                            ledger.clear_pending_mirrors();
                                        }
                                    } else {
                                        ledger.remember_legacy_counter(total_usage, owner.is_none());
                                        if mirror.is_none() {
                                            mirror = observe_legacy_mirror(
                                                &mut ledger,
                                                Some(delta),
                                                mirror_turn_id.as_deref(),
                                                total_usage,
                                                ts_str,
                                                ts_ms,
                                            );
                                        }
                                        if mirror.is_none() {
                                            ledger.clear_pending_mirrors();
                                        }

                                        let raw_model = current_model
                                            .as_deref()
                                            .or(record_model.as_deref())
                                            .map(String::from)
                                            .unwrap_or_else(|| {
                                                resolve_codex_turn_raw_model(
                                                    None,
                                                    mirror_turn_id.as_deref(),
                                                    priority_turns,
                                                )
                                            });
                                        let normalized_model =
                                            pricing::normalize_codex_model(&raw_model);
                                        let row_turn_id = record_turn_id
                                            .as_deref()
                                            .or(current_turn_id.as_deref());
                                        let session_id_opt = session_meta
                                            .as_ref()
                                            .map(|m| m.session_id.as_str())
                                            .filter(|s| !s.is_empty());
                                        let is_fast = is_codex_turn_priority(
                                            current_service_tier,
                                            row_turn_id,
                                            session_id_opt,
                                            &ledger.priority_turn_ids,
                                            priority_turns,
                                        );

                                        if let Some(ref m) = mirror
                                            && let Some(response_id) =
                                                ledger.mirrored_responses.get(&m.snapshot).cloned()
                                        {
                                            ledger.remember_mirrors(
                                                std::iter::once(observation_key.as_str()),
                                                &response_id,
                                            );
                                            heal_ledger_row_from_legacy_mirror(
                                                &response_id,
                                                Some(&normalized_model),
                                                Some(&raw_model),
                                                is_fast,
                                                catalog,
                                                &mut days,
                                                &mut rows,
                                            );
                                            let legacy_row = CodexUsageRow {
                                                day: day_key.as_str().to_owned(),
                                                model: normalized_model,
                                                raw_model: Some(raw_model),
                                                turn_id: row_turn_id.map(String::from),
                                                event_index: None,
                                                timestamp_unix_ms: Some(ts_ms),
                                                input: delta.input,
                                                cached: delta.cached,
                                                output: delta.output,
                                                reasoning: delta.reasoning,
                                                response_id: None,
                                                request_mirror_keys: None,
                                                known_cost_nanos: None,
                                                unpriced_tokens: None,
                                                pricing_model: None,
                                                pricing_mode: None,
                                                session_id: None,
                                                file_identity: None,
                                            };
                                            remember_mirrored_legacy_pricing_key(
                                                &response_id,
                                                &legacy_row,
                                                (&m.snapshot, m.owning_response_id.as_deref()),
                                                &rows,
                                                &retained_rows_by_index,
                                                &mut ledger_legacy_pricing_keys,
                                            );
                                        } else if !delta.is_zero() {
                                            let pricing_model = resolve_codex_priority_pricing_model(
                                                &normalized_model,
                                                row_turn_id,
                                                priority_turns,
                                                catalog,
                                            );
                                            let mirror_keys = mirror.as_ref().map(|m| {
                                                let mut ks = vec![m.snapshot.clone()];
                                                if last_usage.is_none() {
                                                    ks.push(observation_key.clone());
                                                }
                                                ks
                                            });
                                            let this_event_index = event_index;
                                            event_index += 1;
                                            if let Some(ref ks) = mirror_keys {
                                                for k in ks {
                                                    ledger
                                                        .legacy_row_indices
                                                        .insert(k.clone(), this_event_index);
                                                }
                                            }
                                            ledger.pending_legacy_mirrors =
                                                mirror.as_ref().map(|m| m.adjacent.clone());
                                            ledger.pending_legacy_row_index =
                                                Some(this_event_index);
                                            ledger.pending_legacy_near_mirror =
                                                mirror.and_then(|m| m.near);

                                            record_codex_row(
                                                day_key.as_str(),
                                                &normalized_model,
                                                &pricing_model,
                                                Some(&raw_model),
                                                row_turn_id,
                                                Some(this_event_index),
                                                Some(ts_ms),
                                                delta,
                                                is_fast,
                                                None,
                                                mirror_keys,
                                                session_id_opt,
                                                Some(&file_identity),
                                                None,
                                                range,
                                                catalog,
                                                &mut days,
                                                &mut rows,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            None => {
                // Check for bare usage row (`usage`, `data.usage`, `response.usage`, `result.usage`)
                // when `type` is absent at root (`CostUsageScanner.swift:5088`).
                if !suppress_unowned_copied_prefix
                    && let Some((bare_delta, bare_model)) = extract_bare_usage_row(obj)
                    && !bare_delta.is_zero()
                    && let Some(day_key) = line_day_and_ms
                        .as_ref()
                        .map(|(dk, _)| dk.clone())
                        .or_else(|| last_accepted_day_key.clone())
                {
                    let ts_ms = line_day_and_ms
                        .as_ref()
                        .map(|(_, ms)| *ms)
                        .or(last_accepted_ts_ms);
                    if line_day_and_ms.is_none()
                        && let Some(ref prev_ts) = last_accepted_ts_str
                    {
                        let _ = prev_ts;
                    }
                    ledger.clear_pending_mirrors();
                    let effective_raw = bare_model
                        .as_deref()
                        .or(current_model.as_deref())
                        .unwrap_or("gpt-5");
                    let normalized_model = pricing::normalize_codex_model(effective_raw);
                    let pricing_model = resolve_codex_priority_pricing_model(
                        &normalized_model,
                        current_turn_id.as_deref(),
                        priority_turns,
                        catalog,
                    );
                    let session_id_opt = session_meta
                        .as_ref()
                        .map(|m| m.session_id.as_str())
                        .filter(|s| !s.is_empty());
                    let bare_tier = pricing::normalize_codex_service_tier(
                        obj.get("service_tier").and_then(Value::as_str),
                    )
                    .or(current_service_tier);
                    let is_fast = is_codex_turn_priority(
                        bare_tier,
                        current_turn_id.as_deref(),
                        session_id_opt,
                        &ledger.priority_turn_ids,
                        priority_turns,
                    );
                    let this_event_index = event_index;
                    event_index += 1;

                    record_codex_row(
                        day_key.as_str(),
                        &normalized_model,
                        &pricing_model,
                        Some(effective_raw),
                        current_turn_id.as_deref(),
                        Some(this_event_index),
                        ts_ms,
                        bare_delta,
                        is_fast,
                        None,
                        None,
                        session_id_opt,
                        Some(&file_identity),
                        None,
                        range,
                        catalog,
                        &mut days,
                        &mut rows,
                    );
                    line_replaced_by_boundary = true;
                }
            }
            _ => {}
        }

        if has_unresolved_fork_baseline {
            buffered_unresolved_fork_lines.push(CodexBufferedLine {
                line_index: line_idx,
                ordinal,
                end_offset,
                raw: if line_replaced_by_boundary {
                    MIRROR_BOUNDARY_JSON.to_owned()
                } else {
                    raw
                },
                ledger_mirror: line_ledger_mirror,
            });
        }
    }

    let final_counted_totals = if is_forked_session {
        fork_counted_totals
    } else {
        accumulator.counted_totals
    };
    let final_raw_baseline = if is_forked_session {
        fork_raw_baseline
    } else {
        accumulator.raw_totals_baseline
    };
    let final_saw_divergent = if is_forked_session {
        fork_saw_divergent
    } else {
        accumulator.saw_divergent_totals
    };
    let effective_session_id = session_meta
        .as_ref()
        .map(|m| m.session_id.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| resume.initial_session_id.map(String::from));
    let effective_forked_from_id = session_meta
        .as_ref()
        .and_then(|m| m.forked_from_id.clone())
        .or_else(|| resume.initial_forked_from_id.map(String::from));
    let effective_fork_timestamp = session_meta
        .as_ref()
        .and_then(|m| m.fork_timestamp.clone())
        .or_else(|| resume.initial_fork_timestamp.map(String::from));

    let ledger_is_empty = ledger.response_ids.is_empty()
        && ledger.legacy_row_indices.is_empty()
        && ledger.turn_models.is_empty()
        && ledger.active_turn_id.is_none()
        && ledger.session_id == effective_session_id
        && ledger.thread_priority.is_none()
        && ledger.priority_turn_ids.is_empty()
        && ledger.pending_ledger_mirrors.is_none()
        && ledger.pending_legacy_mirrors.is_none()
        && ledger.last_ledger_total.is_none()
        && ledger.last_legacy_total.is_none();

    CodexParseResult {
        days,
        rows,
        token_snapshots,
        parsed_bytes: offset,
        last_model: current_model,
        last_totals: final_raw_baseline.or(final_counted_totals),
        last_counted_totals: final_counted_totals,
        last_raw_totals_baseline: final_raw_baseline,
        last_raw_totals_watermark: accumulator.tracker.watermark,
        seen_raw_totals: accumulator.tracker.seen_raw_totals,
        has_divergent_totals: final_saw_divergent
            && !codex_totals_equal(final_raw_baseline, final_counted_totals),
        has_interleaved_totals: accumulator.tracker.saw_interleaved_totals,
        last_codex_turn_id: current_turn_id,
        last_service_tier: current_service_tier,
        next_usage_row_index: event_index,
        session_id: effective_session_id,
        forked_from_id: effective_forked_from_id,
        fork_timestamp: effective_fork_timestamp,
        depends_on_parent_totals,
        has_unresolved_fork_baseline,
        resolved_inherited_totals: if is_forked_session && !has_unresolved_fork_baseline {
            inherited_totals
        } else {
            None
        },
        remaining_inherited_totals: if is_forked_session && !has_unresolved_fork_baseline {
            remaining_inherited_totals
        } else {
            None
        },
        buffered_subagent_lines: if buffer_subagent_lines {
            buffered_subagent_lines
        } else {
            Vec::new()
        },
        buffered_unresolved_fork_lines: if has_unresolved_fork_baseline {
            buffered_unresolved_fork_lines
        } else {
            Vec::new()
        },
        request_ledger_state: if ledger_is_empty { None } else { Some(ledger) },
        replaced_legacy_row_indices,
        ledger_legacy_pricing_keys,
        codex_session: session_meta,
    }
}

fn apply_accumulator_event_with_skip(
    accumulator: &mut CodexSnapshotAccumulator,
    last: Option<CodexTotals>,
    total: Option<CodexTotals>,
) -> (CodexTotals, bool) {
    if let Some(total) = total {
        if accumulator.tracker.is_seen(total) {
            return (CodexTotals::default(), true);
        }
        let stale_baseline = accumulator
            .tracker
            .watermark
            .or(accumulator.raw_totals_baseline);
        if let Some(previous_total) = stale_baseline
            && codex_looks_like_stale_regression(
                total,
                previous_total,
                last.unwrap_or_default(),
            )
        {
            return (CodexTotals::default(), true);
        }
    }
    let (delta, _) = accumulator.apply_event(last, total, false);
    (delta, false)
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
) -> (CodexTotals, bool) {
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
            return (CodexTotals::default(), true);
        }
        let stale_baseline = tracker.watermark.or(*raw_baseline);
        if let Some(prev_water) = stale_baseline
            && codex_looks_like_stale_regression(
                adjusted_total,
                prev_water,
                last.unwrap_or_default(),
            )
        {
            return (CodexTotals::default(), true);
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
        return (delta, false);
    }

    if let Some(last) = last {
        let adjusted_delta = adjust_last_delta(last);
        let base = counted_totals.unwrap_or_default();
        let next = codex_add_totals(base, adjusted_delta);
        *counted_totals = Some(next);
        *raw_baseline = Some(next);
        tracker.raise_watermark(next);
        return (adjusted_delta, false);
    }

    (CodexTotals::default(), true)
}

#[derive(Debug, Clone)]
struct LegacyMirrorObservation {
    snapshot: String,
    adjacent: BTreeSet<String>,
    near: Option<CodexNearMirror>,
    owning_response_id: Option<String>,
}

fn observe_legacy_mirror(
    ledger: &mut CodexRequestLedgerState,
    usage: Option<CodexTotals>,
    turn_id: Option<&str>,
    total: Option<CodexTotals>,
    timestamp: &str,
    timestamp_ms: i64,
) -> Option<LegacyMirrorObservation> {
    let usage = usage?;
    let snapshot = make_codex_mirror_key(turn_id, Some(usage), total, Some(timestamp));
    let adjacent = make_codex_adjacent_mirror_keys(turn_id, usage, total, timestamp);
    let near = Some(make_codex_near_mirror(
        turn_id,
        usage,
        total,
        ledger.last_legacy_total,
        timestamp_ms,
    ));
    let owning_response_id = ledger.begin_legacy_observation(&adjacent, &snapshot, near.as_ref());
    Some(LegacyMirrorObservation {
        snapshot,
        adjacent,
        near,
        owning_response_id,
    })
}

/// Builds a canonical mirror snapshot key (`CostUsageScanner.mirrorKey`).
#[must_use]
pub fn make_codex_mirror_key(
    turn_id: Option<&str>,
    usage: Option<CodexTotals>,
    total: Option<CodexTotals>,
    timestamp: Option<&str>,
) -> String {
    [
        turn_id.unwrap_or(""),
        &usage.map_or_else(String::new, |u| u.input.to_string()),
        &usage.map_or_else(String::new, |u| u.cached.to_string()),
        &usage.map_or_else(String::new, |u| u.output.to_string()),
        &usage.map_or_else(String::new, |u| u.reasoning.unwrap_or(0).to_string()),
        &total.map_or_else(String::new, |t| t.input.to_string()),
        &total.map_or_else(String::new, |t| t.cached.to_string()),
        &total.map_or_else(String::new, |t| t.output.to_string()),
        timestamp.unwrap_or(""),
    ]
    .join("\x1F")
}

/// Builds the adjacent mirror key set (`CostUsageScanner.adjacentMirrorKeys`).
#[must_use]
pub fn make_codex_adjacent_mirror_keys(
    turn_id: Option<&str>,
    usage: CodexTotals,
    total: Option<CodexTotals>,
    timestamp: &str,
) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    keys.insert(make_codex_mirror_key(
        turn_id,
        Some(usage),
        None,
        Some(timestamp),
    ));
    if total.is_some() {
        keys.insert(make_codex_mirror_key(turn_id, Some(usage), total, None));
    }
    keys
}

/// Builds the near-window mirror descriptor (`CostUsageScanner.nearMirror`).
#[must_use]
pub fn make_codex_near_mirror(
    turn_id: Option<&str>,
    usage: CodexTotals,
    total: Option<CodexTotals>,
    previous: Option<CodexTotals>,
    timestamp_ms: i64,
) -> CodexNearMirror {
    let key = turn_id.filter(|t| !t.is_empty()).map(|turn| {
        [
            turn,
            &usage.input.to_string(),
            &usage.cached.to_string(),
            &usage.output.to_string(),
        ]
        .join("\x1F")
    });
    let counter_advance = match (previous, total) {
        (Some(prev), Some(tot)) => {
            let delta = [
                tot.input - prev.input,
                tot.cached - prev.cached,
                tot.output - prev.output,
            ];
            let expected = [usage.input, usage.cached, usage.output];
            if delta == expected {
                CodexCounterAdvance::Exact
            } else if delta.iter().zip(expected.iter()).any(|(d, e)| d > e) {
                CodexCounterAdvance::Intervening
            } else {
                CodexCounterAdvance::Bounded
            }
        }
        _ => CodexCounterAdvance::Unknown,
    };
    CodexNearMirror {
        key,
        timestamp_ms,
        counter_advance,
    }
}

fn parse_strict_request_usage(map: &serde_json::Map<String, Value>) -> Option<CodexTotals> {
    let integer = |key: &str, default_val: Option<i64>| -> Option<i64> {
        let Some(val) = map.get(key) else {
            return default_val;
        };
        let num = val.as_number()?;
        if let Some(i) = num.as_i64() {
            if i >= 0 && i < i64::MAX {
                return Some(i);
            }
            return None;
        }
        if let Some(f) = num.as_f64()
            && f.is_finite()
            && f >= 0.0
            && f < (i64::MAX as f64)
            && f.trunc() == f
        {
            return Some(f as i64);
        }
        None
    };
    let input = integer("input_tokens", None)?;
    let output = integer("output_tokens", None)?;
    let cached = integer("cached_input_tokens", Some(0))?;
    if cached > input {
        return None;
    }
    let reasoning = integer("reasoning_output_tokens", Some(0))?;
    if reasoning > output {
        return None;
    }
    input.checked_add(output)?;
    Some(CodexTotals {
        input,
        cached,
        output,
        reasoning: Some(reasoning),
    })
}

#[allow(clippy::too_many_arguments)]
fn process_token_usage_record(
    obj: &serde_json::Map<String, Value>,
    timestamp_str: &str,
    line_day_and_ms: Option<&(DayKey, i64)>,
    session_meta: Option<&CodexSessionMetadata>,
    file_identity: &str,
    current_model: Option<&str>,
    current_turn_id: Option<&str>,
    current_service_tier: Option<CodexServiceTier>,
    priority_turns: &BTreeMap<String, CodexPriorityTurnMetadata>,
    range: &DayRange,
    catalog: Option<&ModelsDevCatalog>,
    has_unresolved_fork_baseline: bool,
    deferred_mirror: Option<&CodexNearMirror>,
    retained_rows_by_index: &BTreeMap<usize, CodexUsageRow>,
    event_index: &mut usize,
    ledger: &mut CodexRequestLedgerState,
    replaced_legacy_row_indices: &mut BTreeSet<usize>,
    ledger_legacy_pricing_keys: &mut BTreeMap<usize, CodexSourcePricingKey>,
    days: &mut BTreeMap<String, BTreeMap<String, Vec<i64>>>,
    rows: &mut Vec<CodexUsageRow>,
) -> Option<CodexNearMirror> {
    let payload = obj.get("payload")?.as_object()?;
    let leaf_session_id = session_meta
        .map(|m| m.session_id.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("");
    let record_thread_id = match payload.get("thread_id") {
        Some(v) => v.as_str().map(str::trim).filter(|s| !s.is_empty())?,
        None if leaf_session_id.is_empty() => "",
        None => return None,
    };
    let response_id = match payload.get("response_id") {
        Some(v) => v
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())?
            .to_owned(),
        None if payload
            .get("turn_id")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty() && s.trim().is_empty()) =>
        {
            format!("anon-resp-{}", *event_index)
        }
        None => return None,
    };
    let usage = payload
        .get("usage")
        .and_then(Value::as_object)
        .and_then(parse_strict_request_usage)?;
    let thread_total = match payload.get("thread_token_usage") {
        Some(v) => v.as_object().and_then(parse_strict_request_usage)?,
        None => payload
            .get("total_token_usage")
            .and_then(Value::as_object)
            .and_then(parse_strict_request_usage)
            .unwrap_or(usage),
    };
    if !codex_totals_at_least(thread_total, usage) {
        return None;
    }
    let record_session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if payload.get("session_id").is_some_and(|v| !v.is_null()) && record_session_id.is_none() {
        return None;
    }
    let turn_total = payload
        .get("turn_token_usage")
        .and_then(Value::as_object)
        .and_then(parse_strict_request_usage)
        .filter(|tt| codex_totals_at_least(*tt, usage));

    if !leaf_session_id.is_empty() && record_thread_id != leaf_session_id {
        return None;
    }
    let expected_request_session_id = ledger.session_id.as_deref().unwrap_or(leaf_session_id);
    if let Some(sid) = record_session_id
        && !expected_request_session_id.is_empty()
        && sid != expected_request_session_id
    {
        return None;
    }

    let (day_key, ts_ms) = line_day_and_ms?;
    let record_turn_id = extract_codex_turn_id(payload);
    let turn_id = record_turn_id
        .as_deref()
        .or(current_turn_id)
        .or(ledger.active_turn_id.as_deref())
        .map(String::from);

    let mut keys = vec![
        make_codex_mirror_key(
            turn_id.as_deref(),
            Some(usage),
            Some(thread_total),
            Some(timestamp_str),
        ),
        make_codex_mirror_key(turn_id.as_deref(), Some(usage), None, Some(timestamp_str)),
    ];
    if let Some(tt) = turn_total {
        keys.push(make_codex_mirror_key(
            turn_id.as_deref(),
            Some(usage),
            Some(tt),
            Some(timestamp_str),
        ));
    }

    let record_model = payload
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let raw_model = record_model
        .or_else(|| {
            turn_id
                .as_deref()
                .and_then(|tid| ledger.turn_models.get(tid).cloned())
        })
        .or_else(|| {
            if turn_id.as_deref() == current_turn_id
                || turn_id.as_deref() == ledger.active_turn_id.as_deref()
            {
                current_model.map(String::from)
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            resolve_codex_turn_raw_model(None, turn_id.as_deref(), priority_turns)
        });

    let is_replay = ledger.response_ids.contains(&response_id);
    let mirror = keys
        .iter()
        .find(|k| ledger.legacy_row_indices.contains_key(*k))
        .cloned();
    let adjacent_keys =
        make_codex_adjacent_mirror_keys(turn_id.as_deref(), usage, Some(thread_total), timestamp_str);
    let pending_original = if ledger.pending_ledger_response_id.as_deref()
        == Some(response_id.as_str())
        && ledger.pending_ledger_mirrors.as_ref() == Some(&adjacent_keys)
    {
        ledger.pending_ledger_near_mirror.clone()
    } else {
        None
    };
    let near = if is_replay {
        deferred_mirror.cloned().or(pending_original)
    } else {
        Some(make_codex_near_mirror(
            turn_id.as_deref(),
            usage,
            Some(thread_total),
            ledger.last_ledger_total,
            *ts_ms,
        ))
    };
    if !is_replay || deferred_mirror.is_some() {
        ledger.last_ledger_total = Some(thread_total);
    }

    let matches_adjacent = ledger
        .pending_legacy_mirrors
        .as_ref()
        .is_some_and(|p| !p.is_disjoint(&adjacent_keys))
        || near
            .as_ref()
            .is_some_and(|n| n.matches(ledger.pending_legacy_near_mirror.as_ref(), false));
    let adjacent_index = if matches_adjacent {
        ledger.pending_legacy_row_index
    } else {
        None
    };
    let mirror_index = mirror
        .as_ref()
        .and_then(|k| ledger.legacy_row_indices.get(k).copied())
        .or(adjacent_index);
    let has_mirror = mirror_index.is_some();

    ledger.clear_pending_mirrors();
    ledger.pending_ledger_mirrors = if has_mirror {
        None
    } else {
        Some(adjacent_keys)
    };
    ledger.pending_ledger_response_id = if has_mirror {
        None
    } else {
        Some(response_id.clone())
    };
    ledger.pending_ledger_near_mirror = if has_mirror { None } else { near.clone() };

    let owns_pricing = !is_replay
        || rows
            .iter()
            .rev()
            .find(|r| r.response_id.as_deref() == Some(response_id.as_str()))
            .or_else(|| {
                retained_rows_by_index
                    .values()
                    .find(|r| r.response_id.as_deref() == Some(response_id.as_str()))
            })
            .and_then(|r| r.request_mirror_keys.as_ref())
            .and_then(|ks| ks.first())
            == keys.first();
    ledger.pending_ledger_pricing_response_id = if !has_mirror && owns_pricing {
        Some(response_id.clone())
    } else {
        None
    };

    let legacy_row = mirror_index.and_then(|idx| {
        rows.iter()
            .find(|r| r.event_index == Some(idx))
            .cloned()
            .or_else(|| retained_rows_by_index.get(&idx).cloned())
    });
    if let Some(ref lrow) = legacy_row
        && let Some(ref mks) = lrow.request_mirror_keys
    {
        keys.extend(mks.iter().cloned());
    }

    let base = ledger.counted_usage.unwrap_or_default();
    if !is_replay {
        let new_in = base.input.checked_add(usage.input)?;
        let _new_cached = base.cached.checked_add(usage.cached)?;
        let new_out = base.output.checked_add(usage.output)?;
        new_in.checked_add(new_out)?;
    }

    if !has_unresolved_fork_baseline {
        ledger.remember_mirrors(keys.iter(), &response_id);
    }
    ledger.response_ids.insert(response_id.clone());

    if let (Some(idx), Some(lrow)) = (mirror_index, legacy_row.as_ref()) {
        ledger.legacy_row_indices.retain(|_, v| *v != idx);
        replaced_legacy_row_indices.insert(idx);
        if let Some(pos) = rows.iter().position(|r| r.event_index == Some(idx)) {
            let removed = rows.remove(pos);
            subtract_codex_row_from_days(&removed, days);
        }
        let _ = lrow;
    }

    if is_replay {
        if let Some(ref lrow) = legacy_row {
            remember_mirrored_legacy_pricing_key(
                &response_id,
                lrow,
                (&keys[0], if owns_pricing { Some(response_id.as_str()) } else { None }),
                rows,
                retained_rows_by_index,
                ledger_legacy_pricing_keys,
            );
        }
        return deferred_mirror.cloned();
    }

    ledger.counted_usage = Some(codex_add_totals(base, usage));

    let raw_model = if pricing::normalize_codex_model(&raw_model) == "unknown"
        && let Some(ref lrow) = legacy_row
        && lrow.model != "unknown"
    {
        lrow.raw_model
            .clone()
            .unwrap_or_else(|| lrow.model.clone())
    } else {
        raw_model
    };
    let normalized_model = pricing::normalize_codex_model(&raw_model);
    let pricing_model = resolve_codex_priority_pricing_model(
        &normalized_model,
        turn_id.as_deref(),
        priority_turns,
        catalog,
    );
    let tier =
        pricing::normalize_codex_service_tier(payload.get("service_tier").and_then(Value::as_str))
            .or(current_service_tier);
    let is_fast = is_codex_turn_priority(
        tier,
        turn_id.as_deref(),
        Some(leaf_session_id),
        &ledger.priority_turn_ids,
        priority_turns,
    );
    let this_event_index = *event_index;
    *event_index += 1;

    record_codex_row(
        day_key.as_str(),
        &normalized_model,
        &pricing_model,
        Some(&raw_model),
        turn_id.as_deref(),
        Some(this_event_index),
        Some(*ts_ms),
        usage,
        is_fast,
        Some(response_id.clone()),
        Some(keys.clone()),
        Some(leaf_session_id),
        Some(file_identity),
        legacy_row.as_ref(),
        range,
        catalog,
        days,
        rows,
    );

    if let Some(ref lrow) = legacy_row {
        remember_mirrored_legacy_pricing_key(
            &response_id,
            lrow,
            (&keys[0], if owns_pricing { Some(response_id.as_str()) } else { None }),
            rows,
            retained_rows_by_index,
            ledger_legacy_pricing_keys,
        );
    }

    near
}

fn remember_mirrored_legacy_pricing_key(
    response_id: &str,
    legacy_row: &CodexUsageRow,
    mirror: (&str, Option<&str>),
    rows: &[CodexUsageRow],
    retained_rows_by_index: &BTreeMap<usize, CodexUsageRow>,
    ledger_legacy_pricing_keys: &mut BTreeMap<usize, CodexSourcePricingKey>,
) {
    let Some(ledger_row) = rows
        .iter()
        .rev()
        .find(|r| r.response_id.as_deref() == Some(response_id))
        .or_else(|| {
            retained_rows_by_index
                .values()
                .find(|r| r.response_id.as_deref() == Some(response_id))
        })
    else {
        return;
    };
    let Some(ledger_index) = ledger_row.event_index else {
        return;
    };
    if ledger_row.model != legacy_row.model {
        return;
    }
    let Some(legacy_key) = CodexSourcePricingKey::from_row(legacy_row) else {
        return;
    };
    let owns_snapshot = mirror.1 == Some(response_id)
        || ledger_row
            .request_mirror_keys
            .as_ref()
            .is_some_and(|ks| ks.iter().any(|k| k == mirror.0));
    if !owns_snapshot {
        return;
    }
    ledger_legacy_pricing_keys.insert(ledger_index, legacy_key);
}

fn extract_codex_turn_id(payload: &serde_json::Map<String, Value>) -> Option<String> {
    let info = payload.get("info").and_then(Value::as_object);
    for container in [Some(payload), info].into_iter().flatten() {
        for key in ["turn_id", "turnId", "id"] {
            if let Some(v) = container.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()) {
                return Some(v.to_owned());
            }
        }
    }
    None
}

fn extract_turn_context_model(payload: &serde_json::Map<String, Value>) -> Option<String> {
    let info = payload.get("info").and_then(Value::as_object);
    let candidates = [
        payload.get("model"),
        payload.get("model_name"),
        info.and_then(|i| i.get("model")),
        info.and_then(|i| i.get("model_name")),
    ];
    let mut saw_candidate = false;
    for cand in candidates.into_iter().flatten() {
        if let Some(s) = cand.as_str() {
            saw_candidate = true;
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    if saw_candidate {
        Some(String::new())
    } else {
        None
    }
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
    catalog: Option<&ModelsDevCatalog>,
) -> String {
    if let Some(tid) = turn_id
        && let Some(meta) = priority_turns.get(tid)
        && let Some(ref priority_model) = meta.model
        && !priority_model.is_empty()
    {
        let normalized_prio = pricing::normalize_codex_model(priority_model);
        if pricing::codex_cost_usd_with_tier(
            &normalized_prio,
            1,
            0,
            0,
            Some(CodexServiceTier::Priority),
            catalog,
        )
        .is_some()
        {
            return normalized_prio;
        }
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
    retained_pricing: Option<&CodexUsageRow>,
    range: &DayRange,
    catalog: Option<&ModelsDevCatalog>,
    days: &mut BTreeMap<String, BTreeMap<String, Vec<i64>>>,
    rows: &mut Vec<CodexUsageRow>,
) {
    if !range.contains_scan_day(day_key) {
        return;
    }
    let retained = retained_pricing.filter(|r| r.model == normalized_model);
    let effective_is_fast = is_fast
        || retained_pricing
            .and_then(|r| r.pricing_mode.as_deref())
            .is_some_and(|m| m == "priority");
    let effective_pricing_model = retained
        .and_then(|r| r.pricing_model.as_deref())
        .unwrap_or(pricing_model);
    let tier = if effective_is_fast {
        Some(CodexServiceTier::Priority)
    } else {
        Some(CodexServiceTier::Standard)
    };
    let pricing_epoch_secs = timestamp_unix_ms.map(|ms| ms.div_euclid(1000));
    let cost_nanos = retained
        .and_then(|r| r.known_cost_nanos)
        .or_else(|| {
            pricing::codex_cost_usd_with_tier_and_epoch(
                effective_pricing_model,
                delta.input,
                delta.cached,
                delta.output,
                tier,
                pricing_epoch_secs,
                catalog,
            )
            .or_else(|| {
                if effective_is_fast {
                    pricing::codex_cost_usd_with_tier_and_epoch(
                        effective_pricing_model,
                        delta.input,
                        delta.cached,
                        delta.output,
                        Some(CodexServiceTier::Standard),
                        pricing_epoch_secs,
                        catalog,
                    )
                } else {
                    None
                }
            })
            .map(|c| (c * 1_000_000_000.0).round() as i64)
        });
    let total_tokens = delta.scanner_units();
    let unpriced_tokens = retained
        .and_then(|r| r.unpriced_tokens)
        .or(if cost_nanos.is_none() {
            Some(total_tokens)
        } else {
            None
        });

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
        pricing_model: Some(effective_pricing_model.to_owned()),
        pricing_mode: Some(
            if effective_is_fast {
                "priority"
            } else {
                "standard"
            }
            .to_owned(),
        ),
        session_id: session_id.map(String::from),
        file_identity: file_identity.map(String::from),
    }
    .with_clamped_reasoning();

    add_codex_row_to_days(&row, days);
    rows.push(row);
}

fn heal_ledger_row_from_legacy_mirror(
    response_id: &str,
    legacy_normalized_model: Option<&str>,
    legacy_raw_model: Option<&str>,
    legacy_is_fast: bool,
    catalog: Option<&ModelsDevCatalog>,
    days: &mut BTreeMap<String, BTreeMap<String, Vec<i64>>>,
    rows: &mut [CodexUsageRow],
) {
    let Some(pos) = rows
        .iter()
        .rposition(|r| r.response_id.as_deref() == Some(response_id))
    else {
        return;
    };
    let mut updated = rows[pos].clone();
    let mut changed = false;

    if updated.model == "unknown"
        && let Some(m) = legacy_normalized_model
        && m != "unknown"
    {
        updated.model = m.to_owned();
        updated.raw_model = legacy_raw_model.map(String::from);
        updated.pricing_model = Some(m.to_owned());
        changed = true;
    }
    if legacy_is_fast && updated.pricing_mode.as_deref() != Some("priority") {
        updated.pricing_mode = Some("priority".to_owned());
        changed = true;
    }
    if !changed {
        return;
    }

    subtract_codex_row_from_days(&rows[pos], days);
    let is_fast = updated.pricing_mode.as_deref() == Some("priority");
    let pricing_model = updated
        .pricing_model
        .as_deref()
        .unwrap_or(&updated.model);
    let tier = if is_fast {
        Some(CodexServiceTier::Priority)
    } else {
        Some(CodexServiceTier::Standard)
    };
    let cost_nanos = pricing::codex_cost_usd_with_tier(
        pricing_model,
        updated.input,
        updated.cached,
        updated.output,
        tier,
        catalog,
    )
    .or_else(|| {
        if is_fast {
            pricing::codex_cost_usd_with_tier(
                pricing_model,
                updated.input,
                updated.cached,
                updated.output,
                Some(CodexServiceTier::Standard),
                catalog,
            )
        } else {
            None
        }
    })
    .map(|c| (c * 1_000_000_000.0).round() as i64);
    let total_tokens = CodexTotals {
        input: updated.input,
        cached: updated.cached,
        output: updated.output,
        reasoning: updated.reasoning,
    }
    .scanner_units();
    updated.known_cost_nanos = cost_nanos;
    updated.unpriced_tokens = if cost_nanos.is_none() {
        Some(total_tokens)
    } else {
        None
    };
    add_codex_row_to_days(&updated, days);
    rows[pos] = updated;
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
    let request_session_id = payload
        .and_then(|p| p.get("session_id").or_else(|| p.get("sessionId")))
        .or_else(|| obj.get("session_id"))
        .or_else(|| obj.get("sessionId"))
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
        request_session_id,
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
    if leaf.request_session_id.is_none() {
        leaf.request_session_id = extra.request_session_id.clone();
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

type PredecodedCodexLine = (
    usize,
    u64,
    Option<i64>,
    bool,
    String,
    Option<Value>,
    Option<CodexNearMirror>,
    bool,
);

fn build_subagent_observations(
    parsed_lines: &[PredecodedCodexLine],
) -> Vec<CodexSubagentObservation> {
    let mut observations = Vec::new();
    for (idx, _end_offset, _ordinal, was_truncated, raw, val_opt, _, _) in parsed_lines {
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
        .find(|(_, _, ord, _, _, _, _, _)| ord.unwrap_or(i64::MIN) >= start_ordinal)
        .map(|(idx, _, _, _, _, _, _, _)| *idx)?;

    let mut inherited_total: Option<CodexTotals> = None;
    for (_, _, ord, _, _, val_opt, _, _) in parsed_lines
        .iter()
        .take_while(|(_, _, ord, _, _, _, _, _)| ord.unwrap_or(i64::MIN) < start_ordinal)
    {
        let _ = ord;
        if let Some(val) = val_opt
            && let Some((_, _, Some(total))) = extract_boundary_token_count_from_value(val)
        {
            inherited_total = Some(total);
        }
    }

    let mut first_owned_token: Option<(usize, Option<CodexTotals>, Option<CodexTotals>)> = None;
    for (idx, _, ord, _, _, val_opt, _, _) in parsed_lines {
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
    let reasoning = json_non_negative_i64(map.get("reasoning_output_tokens"))
        .map(|r| r.max(0).min(output.max(0)));
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

    let input_raw = json_non_negative_i64(usage_map.get("input_tokens"))
        .or_else(|| json_non_negative_i64(usage_map.get("prompt_tokens")))
        .or_else(|| json_non_negative_i64(usage_map.get("input")))?;
    let output = json_non_negative_i64(usage_map.get("output_tokens"))
        .or_else(|| json_non_negative_i64(usage_map.get("completion_tokens")))
        .or_else(|| json_non_negative_i64(usage_map.get("output")))?;

    let cached = [
        "cached_input_tokens",
        "cache_read_input_tokens",
        "cached_tokens",
    ]
    .into_iter()
    .filter_map(|k| json_non_negative_i64(usage_map.get(k)))
    .max()
    .unwrap_or(0)
    .max(0);

    let input = (input_raw - cached).max(0);
    let reasoning = json_non_negative_i64(usage_map.get("reasoning_output_tokens"))
        .map(|r| r.max(0).min(output.max(0)));
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

/// Groups where recovered source rows outnumber retained store rows (`CostUsageScanner.codexExcessPricingRowGroups`).
#[must_use]
pub fn codex_excess_pricing_row_groups(
    retained: &[CodexUsageRow],
    source: &[CodexUsageRow],
) -> BTreeSet<CodexSourcePricingKey> {
    let mut retained_counts: BTreeMap<CodexSourcePricingKey, usize> = BTreeMap::new();
    let mut source_counts: BTreeMap<CodexSourcePricingKey, usize> = BTreeMap::new();
    for row in retained {
        for key in CodexSourcePricingKey::match_keys(row) {
            *retained_counts.entry(key).or_insert(0) += 1;
        }
    }
    for row in source {
        for key in CodexSourcePricingKey::match_keys(row) {
            *source_counts.entry(key).or_insert(0) += 1;
        }
    }
    source_counts
        .into_iter()
        .filter_map(|(key, count)| {
            let r_count = retained_counts.get(&key).copied().unwrap_or(0);
            if r_count > 0 && count > r_count {
                Some(key)
            } else {
                None
            }
        })
        .collect()
}

/// Returns true when `source` has more rows than `retained` for any `CodexSourcePricingKey` group.
#[must_use]
pub fn has_excess_codex_pricing_rows(
    retained: &[CodexUsageRow],
    source: &[CodexUsageRow],
) -> bool {
    !codex_excess_pricing_row_groups(retained, source).is_empty()
}

/// Reattaches retained priority/unpriced pricing metadata onto recovered source rows
/// when the trace DB is missing or older turns were trimmed (`CostUsageScanner.swift:2835-2960`).
#[must_use]
pub fn recover_codex_rows_with_retained_pricing(
    source_rows: &[CodexUsageRow],
    retained_rows: &[CodexUsageRow],
    ledger_legacy_pricing_keys: &BTreeMap<usize, CodexSourcePricingKey>,
) -> Vec<CodexUsageRow> {
    let mut retained_by_index: BTreeMap<usize, &CodexUsageRow> = BTreeMap::new();
    let mut retained_by_response_id: BTreeMap<&str, &CodexUsageRow> = BTreeMap::new();
    let mut retained_by_key: BTreeMap<CodexSourcePricingKey, Vec<(usize, &CodexUsageRow)>> =
        BTreeMap::new();

    for (pos, row) in retained_rows.iter().enumerate() {
        if let Some(idx) = row.event_index {
            retained_by_index.entry(idx).or_insert(row);
        }
        if let Some(ref rid) = row.response_id {
            retained_by_response_id.entry(rid.as_str()).or_insert(row);
        }
        for key in CodexSourcePricingKey::match_keys(row) {
            retained_by_key.entry(key).or_default().push((pos, row));
        }
    }

    let excess_groups = codex_excess_pricing_row_groups(retained_rows, source_rows);
    let mut consumed_retained_positions: BTreeSet<usize> = BTreeSet::new();
    let mut out = Vec::with_capacity(source_rows.len());

    for row in source_rows {
        let mut matched_retained: Option<&CodexUsageRow> = None;

        // 1. Exact response_id match
        if let Some(ref rid) = row.response_id
            && let Some(&r) = retained_by_response_id.get(rid.as_str())
            && r.model == row.model
            && r.input == row.input
            && r.cached == row.cached
            && r.output == row.output
        {
            matched_retained = Some(r);
        }

        // 2. Mirrored legacy pricing key recorded during request-ledger replacement
        if matched_retained.is_none()
            && let Some(idx) = row.event_index
            && let Some(legacy_key) = ledger_legacy_pricing_keys.get(&idx)
            && !excess_groups.contains(legacy_key)
            && let Some(candidates) = retained_by_key.get(legacy_key)
            && let Some(&(pos, r)) = candidates
                .iter()
                .find(|(pos, _)| !consumed_retained_positions.contains(pos))
        {
            consumed_retained_positions.insert(pos);
            matched_retained = Some(r);
        }

        // 3. Exact event_index + token/model match
        if matched_retained.is_none()
            && let Some(idx) = row.event_index
            && let Some(&r) = retained_by_index.get(&idx)
            && r.model == row.model
            && r.day == row.day
            && r.input == row.input
            && r.cached == row.cached
            && r.output == row.output
            && r.turn_id == row.turn_id
        {
            matched_retained = Some(r);
        }

        // 4. Unambiguous CodexSourcePricingKey group match (skipped if source has excess rows for that group)
        if matched_retained.is_none() {
            let match_keys = CodexSourcePricingKey::match_keys(row);
            let is_excess = match_keys.iter().any(|k| excess_groups.contains(k));
            if !is_excess {
                for key in match_keys {
                    if let Some(candidates) = retained_by_key.get(&key)
                        && let Some(&(pos, r)) = candidates
                            .iter()
                            .find(|(pos, _)| !consumed_retained_positions.contains(pos))
                    {
                        consumed_retained_positions.insert(pos);
                        matched_retained = Some(r);
                        break;
                    }
                }
            }
        }

        let mut updated = row.clone();
        if let Some(r) = matched_retained
            && (r.pricing_mode.as_deref() == Some("priority") || r.known_cost_nanos.is_some())
        {
            if r.pricing_mode.as_deref() == Some("priority")
                || updated.pricing_mode.as_deref() != Some("priority")
            {
                updated.pricing_mode = r.pricing_mode.clone().or(updated.pricing_mode);
                updated.pricing_model = r.pricing_model.clone().or(updated.pricing_model);
                updated.known_cost_nanos = r.known_cost_nanos;
                updated.unpriced_tokens = r.unpriced_tokens;
            }
        }
        out.push(updated);
    }

    out
}

/// Token snapshot index anchor for verifying cached snapshots before appending tail snapshots
/// (`CostUsageScanner.codexTokenIndexAnchor`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexTokenIndexAnchor {
    pub count: usize,
    pub last_timestamp: String,
    pub last_input: Option<i64>,
    pub last_cached: Option<i64>,
    pub last_output: Option<i64>,
    pub total_input: Option<i64>,
    pub total_cached: Option<i64>,
    pub total_output: Option<i64>,
}

/// Builds a [`CodexTokenIndexAnchor`] from the current `token_snapshots` slice.
#[must_use]
pub fn codex_token_index_anchor(
    snapshots: &[CodexTokenSnapshot],
) -> Option<CodexTokenIndexAnchor> {
    let last = snapshots.last()?;
    Some(CodexTokenIndexAnchor {
        count: snapshots.len(),
        last_timestamp: last.timestamp.clone(),
        last_input: last.last.map(|t| t.input),
        last_cached: last.last.map(|t| t.cached),
        last_output: last.last.map(|t| t.output),
        total_input: last.total.map(|t| t.input),
        total_cached: last.total.map(|t| t.cached),
        total_output: last.total.map(|t| t.output),
    })
}

/// Verifies whether `snapshots` matches the expected `anchor`.
#[must_use]
pub fn codex_token_index_anchor_matches(
    snapshots: &[CodexTokenSnapshot],
    anchor: Option<&CodexTokenIndexAnchor>,
) -> bool {
    match (codex_token_index_anchor(snapshots), anchor) {
        (None, None) => true,
        (Some(ref actual), Some(expected)) => actual == expected,
        _ => false,
    }
}

/// Computes remaining unread bytes for a file given its size and cached `parsed_bytes`
/// (`CostUsageScanner.pendingCodexScanWorkBytes`).
#[must_use]
pub fn pending_codex_scan_work_bytes(file_size: u64, cached_parsed_bytes: Option<u64>) -> u64 {
    let size = file_size;
    let Some(parsed) = cached_parsed_bytes else {
        return size;
    };
    if parsed > size {
        return size;
    }
    size - parsed
}

/// Scan budget controller for bounded incremental Codex refreshes (`CostUsageScanner.CodexScanBudget`).
#[derive(Debug, Clone)]
pub struct CodexScanBudget {
    pub max_bytes: Option<u64>,
    pub consumed_bytes: u64,
    pub processed_files: usize,
    pub deferred_changed_file: bool,
}

impl CodexScanBudget {
    #[must_use]
    pub fn new(max_bytes: Option<u64>) -> Self {
        Self {
            max_bytes,
            consumed_bytes: 0,
            processed_files: 0,
            deferred_changed_file: false,
        }
    }

    pub fn should_defer(&mut self, pending_bytes: u64) -> bool {
        if pending_bytes == 0 {
            return false;
        }
        let Some(limit) = self.max_bytes else {
            return false;
        };
        if self.processed_files > 0 && self.consumed_bytes.saturating_add(pending_bytes) > limit {
            self.deferred_changed_file = true;
            return true;
        }
        false
    }

    pub fn record_processed(&mut self, consumed_bytes: u64) {
        if consumed_bytes == 0 {
            return;
        }
        self.processed_files += 1;
        self.consumed_bytes = self.consumed_bytes.saturating_add(consumed_bytes);
    }
}

/// Options for scanning a Codex home or sessions directory.
#[derive(Debug, Clone, Default)]
pub struct CodexScanOptions {
    pub codex_home: Option<PathBuf>,
    pub codex_sessions_root: Option<PathBuf>,
    pub priority_db_path: Option<PathBuf>,
    pub timezone: Option<TimeZone>,
    pub catalog: Option<ModelsDevCatalog>,
    pub temp_dir: Option<PathBuf>,
    pub max_sqlite_bytes: Option<u64>,
    pub priority_deadline: Option<std::time::Instant>,
    pub persisted_priority_cursor: Option<codex_priority::CodexPriorityTurnsPersistedCursor>,
    pub force_rescan: bool,
}

/// Aggregate result of scanning a Codex home directory across `sessions/` and `archived_sessions/`.
#[derive(Debug, Clone, Default)]
pub struct CodexScanReport {
    pub rows: Vec<CodexUsageRow>,
    pub unmetered_requests_by_day: BTreeMap<String, u64>,
    pub unpriced_requests_by_day: BTreeMap<String, u64>,
    pub priority_data_is_partial: bool,
    pub validation_pending: bool,
    pub persisted_priority_cursor: Option<codex_priority::CodexPriorityTurnsPersistedCursor>,
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

    let mut memo_state = if options.force_rescan {
        codex_priority::CodexPriorityTurnsMemoState::default()
    } else if let Some(ref cursor) = options.persisted_priority_cursor {
        codex_priority::CodexPriorityTurnsMemoState::from_persisted_cursor(cursor)
    } else {
        codex_priority::CodexPriorityTurnsMemoState::default()
    };
    let max_sqlite_bytes = options
        .max_sqlite_bytes
        .unwrap_or(codex_priority::MAX_CODEX_SQLITE_BYTES);

    let priority_resolution = if let Some(ref db_path) = options.priority_db_path {
        codex_priority::resolve_codex_priority_turns(
            db_path,
            Some(range.scan_since_key.as_str()),
            Some(range.scan_until_key.as_str()),
            range.timezone.clone(),
            &mut memo_state,
            options.priority_deadline,
            options.temp_dir.as_deref(),
            max_sqlite_bytes,
        )
    } else if let Some(ref home) = codex_home {
        codex_priority::resolve_codex_priority_turns_for_home(
            home,
            Some(range.scan_since_key.as_str()),
            Some(range.scan_until_key.as_str()),
            range.timezone.clone(),
            &mut memo_state,
            options.priority_deadline,
            options.temp_dir.as_deref(),
            max_sqlite_bytes,
        )
    } else {
        codex_priority::CodexPriorityTurnsResolution::default()
    };
    let persisted_cursor = memo_state
        .any_persisted_cursor()
        .or_else(|| options.persisted_priority_cursor.clone());

    let priority_turns = priority_resolution.turns;
    let priority_data_is_partial =
        priority_resolution.database_skipped || priority_resolution.validation_pending;

    let session_tiers = codex_home
        .as_deref()
        .map(|home| {
            codex_priority::codex_session_service_tiers_with_temp(home, options.temp_dir.as_deref())
        })
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
    let mut unpriced_requests_by_day: BTreeMap<String, u64> = BTreeMap::new();
    for row in &deduped_rows {
        if row.known_cost_nanos.is_none() || row.unpriced_tokens.unwrap_or(0) > 0 {
            *unpriced_requests_by_day.entry(row.day.clone()).or_insert(0) += 1;
        }
    }

    CodexScanReport {
        rows: deduped_rows,
        unmetered_requests_by_day,
        unpriced_requests_by_day,
        priority_data_is_partial,
        validation_pending: priority_resolution.validation_pending,
        persisted_priority_cursor: persisted_cursor,
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
                parent_res.resolved_inherited_totals,
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
