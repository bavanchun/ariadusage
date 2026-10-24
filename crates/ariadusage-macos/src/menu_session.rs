// Ported from CodexBar Sources/CodexBar/MenuSessionCoordinator.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuOpenRefreshPlan.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuCardRefreshMonitor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuReconcile.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuSmartUpdate.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuRefreshScheduling.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuSwitcherWarmup.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MergedSwitcherContentCache.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+PersistentRefreshMenuItem.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+ProviderSwitcher.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/TrackingRunLoopGate.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/StatusItemController+MenuViewportRestore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Pure-Rust menu session coordination, open-menu refresh planning, manual
//! refresh card freezing, sibling switcher tab warmup caching, position-wise
//! menu row reconciliation, and pointer-tracking state.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ariadusage_protocol::metric::{Metric, MetricState};
use ariadusage_protocol::snapshot::{EngineSnapshot, ProviderSnapshot};
use jiff::Timestamp;

use crate::card::{self, CardModel, CardPreferences, Subtitle, SubtitleStyle};
use crate::card_layout::CardSectionMode;
use crate::format::DateTexts;
use crate::menu_descriptor::{self, MenuAction, MenuEntry, MenuStructure, PrimaryContent};
use crate::switcher::{self, SwitcherSelection};

/// Canonical menu identifier for the merged status item dropdown.
pub const MERGED_MENU_ID: &str = "merged";

/// Delay after `menuWillOpen:` before running an automatic open-menu refresh
/// (`StatusItemController.menuOpenRefreshDelay`).
pub const MENU_OPEN_REFRESH_DELAY: Duration = Duration::from_millis(1200);

// ---------------------------------------------------------------------------
// 1. MenuSessionCoordinator & MenuRebuildRequestRegistry
// ---------------------------------------------------------------------------

/// Closed-menu preparation plan (`MenuSessionCoordinator.ClosedPreparationPlan`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosedPreparationPlan {
    None,
    NonDeferred,
    Required { version: u64 },
}

/// Tracks menu content versions, structural vs data-only invalidations,
/// deferred open/close refreshes, and parent rebuild deferrals during tracking
/// (`MenuSessionCoordinator.swift`).
#[derive(Debug, Clone, Default)]
pub struct MenuSessionCoordinator {
    content_version: u64,
    latest_required_rebuild_version: u64,
    latest_data_only_content_version: u64,
    latest_structural_content_version: u64,
    rendered_versions: BTreeMap<String, u64>,
    deferred_until_next_open: BTreeSet<String>,
    parent_rebuilds_deferred_during_tracking: BTreeSet<String>,
    next_menu_interaction_generation: u64,
    menu_interaction_generations: BTreeMap<String, u64>,
    next_viewport_restore_generation: u64,
    pending_viewport_restores: BTreeMap<String, u64>,
}

impl MenuSessionCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current_content_version(&self) -> u64 {
        self.content_version
    }

    pub fn required_rebuild_version(&self) -> u64 {
        self.latest_required_rebuild_version
    }

    pub fn data_only_content_version(&self) -> u64 {
        self.latest_data_only_content_version
    }

    pub fn structural_content_version(&self) -> u64 {
        self.latest_structural_content_version
    }

    /// Advances `content_version` and records whether the change is data-only
    /// (`allows_stale_content == true`) or structural (`MenuSessionCoordinator.invalidate`).
    pub fn invalidate(&mut self, allows_stale_content: bool, requires_rebuild: bool) -> u64 {
        self.content_version += 1;
        if allows_stale_content {
            self.latest_data_only_content_version = self.content_version;
        } else {
            self.latest_structural_content_version = self.content_version;
            if requires_rebuild {
                self.latest_required_rebuild_version = self.content_version;
            }
        }
        self.content_version
    }

    pub fn mark_rendered(&mut self, menu_id: &str, version: Option<u64>) {
        let v = version.unwrap_or(self.content_version);
        self.rendered_versions.insert(menu_id.to_string(), v);
        self.deferred_until_next_open.remove(menu_id);
        self.parent_rebuilds_deferred_during_tracking
            .remove(menu_id);
    }

    pub fn mark_fresh(&mut self, menu_id: &str) {
        self.rendered_versions
            .insert(menu_id.to_string(), self.content_version);
    }

    pub fn rendered_version(&self, menu_id: &str) -> Option<u64> {
        self.rendered_versions.get(menu_id).copied()
    }

    pub fn can_preserve_stale_content(&self, menu_id: &str) -> bool {
        let Some(rendered) = self.rendered_versions.get(menu_id).copied() else {
            return false;
        };
        self.content_version == self.latest_data_only_content_version
            && rendered >= self.latest_structural_content_version
    }

    pub fn has_required_closed_preparation(&self, menu_ids: &[&str]) -> bool {
        if self.latest_required_rebuild_version == 0 {
            return false;
        }
        menu_ids.iter().any(|id| {
            self.rendered_versions
                .get(*id)
                .is_none_or(|&v| v < self.latest_required_rebuild_version)
        })
    }

    pub fn closed_preparation_plan(&self, menu_ids: &[&str]) -> ClosedPreparationPlan {
        if self.has_required_closed_preparation(menu_ids) {
            return ClosedPreparationPlan::Required {
                version: self.latest_required_rebuild_version,
            };
        }
        if self.content_version > self.latest_required_rebuild_version {
            return ClosedPreparationPlan::None;
        }
        ClosedPreparationPlan::NonDeferred
    }

    pub fn needs_refresh(&self, menu_id: &str, allows_stale_content: bool) -> bool {
        let Some(rendered) = self.rendered_versions.get(menu_id).copied() else {
            return true;
        };
        let can_preserve_stale =
            allows_stale_content && rendered >= self.latest_structural_content_version;
        if can_preserve_stale {
            return false;
        }
        rendered != self.content_version
    }

    pub fn needs_required_rebuild(&self, menu_id: &str) -> bool {
        let Some(rendered) = self.rendered_versions.get(menu_id).copied() else {
            return true;
        };
        rendered < self.latest_required_rebuild_version
    }

    pub fn defer_until_next_open(&mut self, menu_id: &str) {
        self.deferred_until_next_open.insert(menu_id.to_string());
    }

    pub fn is_deferred_until_next_open(&self, menu_id: &str) -> bool {
        self.deferred_until_next_open.contains(menu_id)
    }

    pub fn consume_deferred_refresh_for_open(&mut self, menu_id: &str) -> bool {
        self.deferred_until_next_open.remove(menu_id)
    }

    pub fn defer_parent_rebuild(&mut self, menu_id: &str) {
        self.parent_rebuilds_deferred_during_tracking
            .insert(menu_id.to_string());
    }

    pub fn consume_deferred_parent_rebuild(&mut self, menu_id: &str) -> bool {
        self.parent_rebuilds_deferred_during_tracking
            .remove(menu_id)
    }

    pub fn has_deferred_parent_rebuild(&self, menu_id: &str) -> bool {
        self.parent_rebuilds_deferred_during_tracking
            .contains(menu_id)
    }

    pub fn begin_interaction_session(&mut self, menu_id: &str) -> u64 {
        self.next_menu_interaction_generation += 1;
        let next = self.next_menu_interaction_generation;
        self.menu_interaction_generations
            .insert(menu_id.to_string(), next);
        next
    }

    pub fn end_interaction_session(&mut self, menu_id: &str) {
        self.menu_interaction_generations.remove(menu_id);
        self.pending_viewport_restores.remove(menu_id);
    }

    pub fn advance_menu_interaction(&mut self, menu_id: &str) -> Option<u64> {
        if !self.menu_interaction_generations.contains_key(menu_id) {
            return None;
        }
        Some(self.begin_interaction_session(menu_id))
    }

    pub fn menu_interaction_generation(&self, menu_id: &str) -> Option<u64> {
        self.menu_interaction_generations.get(menu_id).copied()
    }

    pub fn is_current_menu_interaction(&self, menu_id: &str, generation: u64) -> bool {
        self.menu_interaction_generations.get(menu_id).copied() == Some(generation)
    }

    pub fn interaction_generation(&self, menu_id: &str) -> u64 {
        self.menu_interaction_generations
            .get(menu_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn arm_viewport_restore(&mut self, menu_id: &str) -> u64 {
        self.next_viewport_restore_generation += 1;
        let next = self.next_viewport_restore_generation;
        self.pending_viewport_restores
            .insert(menu_id.to_string(), next);
        next
    }

    pub fn is_current_viewport_restore(&self, menu_id: &str, generation: u64) -> bool {
        self.pending_viewport_restores.get(menu_id).copied() == Some(generation)
    }

    pub fn consume_viewport_restore_generation(&mut self, menu_id: &str, generation: u64) -> bool {
        if !self.is_current_viewport_restore(menu_id, generation) {
            return false;
        }
        self.pending_viewport_restores.remove(menu_id);
        true
    }

    pub fn has_pending_viewport_restores(&self) -> bool {
        !self.pending_viewport_restores.is_empty()
    }

    pub fn mark_viewport_restore_needed(&mut self, menu_id: &str) {
        let _ = self.arm_viewport_restore(menu_id);
    }

    pub fn consume_viewport_restore(&mut self, menu_id: &str) -> bool {
        self.pending_viewport_restores.remove(menu_id).is_some()
    }

    pub fn remove_menu(&mut self, menu_id: &str) {
        self.rendered_versions.remove(menu_id);
        self.deferred_until_next_open.remove(menu_id);
        self.parent_rebuilds_deferred_during_tracking
            .remove(menu_id);
        self.menu_interaction_generations.remove(menu_id);
        self.pending_viewport_restores.remove(menu_id);
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Monotonic token registry that coalesces asynchronous menu rebuild requests
/// (`MenuRebuildRequestRegistry`).
#[derive(Debug, Clone, Default)]
pub struct MenuRebuildRequestRegistry {
    latest_tokens: BTreeMap<String, u64>,
    next_token: u64,
}

impl MenuRebuildRequestRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin(&mut self, menu_id: &str) -> u64 {
        self.next_token += 1;
        let token = self.next_token;
        self.latest_tokens.insert(menu_id.to_string(), token);
        token
    }

    pub fn is_current(&self, menu_id: &str, token: u64) -> bool {
        self.latest_tokens.get(menu_id).copied() == Some(token)
    }

    pub fn complete(&mut self, menu_id: &str, token: u64) -> bool {
        if self.latest_tokens.get(menu_id).copied() != Some(token) {
            return false;
        }
        self.latest_tokens.remove(menu_id);
        true
    }

    pub fn cancel(&mut self, menu_id: &str) {
        self.latest_tokens.remove(menu_id);
    }

    pub fn cancel_all(&mut self) {
        self.latest_tokens.clear();
    }
}

// ---------------------------------------------------------------------------
// 2. Snapshot Classification (`SnapshotDelta`)
// ---------------------------------------------------------------------------

/// Whether an incoming snapshot is data-only or structural.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotDelta {
    pub allows_stale_content: bool,
    pub requires_rebuild: bool,
}

/// Classifies the difference between `prev` and `next` (`StatusItemController.bindStore`).
///
/// When `prev` is `None` or the enabled provider topology / presentation metadata
/// changed, returns a structural invalidation (`allows_stale_content: false,
/// requires_rebuild: true`). When only usage metrics, timestamps, errors, or costs
/// changed, returns a data-only invalidation (`allows_stale_content: true,
/// requires_rebuild: false`).
pub fn classify_snapshot_update(
    prev: Option<&EngineSnapshot>,
    next: &EngineSnapshot,
) -> SnapshotDelta {
    let Some(prev) = prev else {
        return SnapshotDelta {
            allows_stale_content: false,
            requires_rebuild: true,
        };
    };
    if prev.providers.len() != next.providers.len() {
        return SnapshotDelta {
            allows_stale_content: false,
            requires_rebuild: true,
        };
    }
    for (p, n) in prev.providers.iter().zip(next.providers.iter()) {
        if p.id != n.id
            || p.enabled != n.enabled
            || p.display_name != n.display_name
            || p.presentation != n.presentation
        {
            return SnapshotDelta {
                allows_stale_content: false,
                requires_rebuild: true,
            };
        }
    }
    SnapshotDelta {
        allows_stale_content: true,
        requires_rebuild: false,
    }
}

// ---------------------------------------------------------------------------
// 3. MenuOpenRefreshPlan
// ---------------------------------------------------------------------------

/// How providers in a [`MenuOpenRefreshPlan`] are refreshed (`MenuOpenRefreshPlan.Scheduling`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshScheduling {
    Concurrent,
    Sequential,
}

/// Inputs to [`MenuOpenRefreshPlan::resolve`] (`MenuOpenRefreshPlan.Inputs`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MenuOpenRefreshInputs {
    pub refresh_all_on_open: bool,
    pub enabled_providers: Vec<String>,
    pub visible_providers: Vec<String>,
    pub refreshing_providers: BTreeSet<String>,
    pub stale_providers: BTreeSet<String>,
    pub missing_providers: BTreeSet<String>,
}

/// Resolved open-menu refresh plan (`MenuOpenRefreshPlan.swift`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuOpenRefreshPlan {
    pub providers: Vec<String>,
    pub scheduling: RefreshScheduling,
    pub refresh_codex_dashboard: bool,
    pub refresh_token_cost: bool,
}

impl MenuOpenRefreshPlan {
    /// Resolves which providers should be refreshed after the menu opens
    /// (`MenuOpenRefreshPlan.resolve`).
    pub fn resolve(inputs: &MenuOpenRefreshInputs) -> Self {
        if inputs.refresh_all_on_open {
            return Self {
                providers: inputs.enabled_providers.clone(),
                scheduling: RefreshScheduling::Concurrent,
                refresh_codex_dashboard: inputs.enabled_providers.iter().any(|p| p == "codex"),
                refresh_token_cost: !inputs.enabled_providers.is_empty(),
            };
        }
        let enabled_set: BTreeSet<&str> = inputs
            .enabled_providers
            .iter()
            .map(String::as_str)
            .collect();
        let providers = inputs
            .visible_providers
            .iter()
            .filter(|id| {
                enabled_set.contains(id.as_str())
                    && (inputs.refreshing_providers.contains(*id)
                        || inputs.stale_providers.contains(*id)
                        || inputs.missing_providers.contains(*id))
            })
            .cloned()
            .collect();
        Self {
            providers,
            scheduling: RefreshScheduling::Sequential,
            refresh_codex_dashboard: false,
            refresh_token_cost: false,
        }
    }

    /// Builds a [`MenuOpenRefreshPlan`] directly from `snapshot` and the currently
    /// visible selection (`StatusItemController.scheduleMenuOpenRefreshIfNeeded`).
    pub fn from_snapshot(
        snapshot: &EngineSnapshot,
        selection: Option<&SwitcherSelection>,
        refreshing_providers: &BTreeSet<String>,
        refresh_all_on_open: bool,
    ) -> Self {
        let enabled_providers: Vec<String> = snapshot
            .providers
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.id.as_str().to_string())
            .collect();
        let visible_providers: Vec<String> = match selection {
            Some(SwitcherSelection::Provider(id)) => vec![id.clone()],
            Some(SwitcherSelection::Overview) | None => enabled_providers.clone(),
        };
        let mut stale_providers = BTreeSet::new();
        let mut missing_providers = BTreeSet::new();
        for p in snapshot.providers.iter().filter(|p| p.enabled) {
            if is_provider_stale(p) {
                stale_providers.insert(p.id.as_str().to_string());
            }
            if is_provider_missing_usage(p) {
                missing_providers.insert(p.id.as_str().to_string());
            }
        }
        Self::resolve(&MenuOpenRefreshInputs {
            refresh_all_on_open,
            enabled_providers,
            visible_providers,
            refreshing_providers: refreshing_providers.clone(),
            stale_providers,
            missing_providers,
        })
    }
}

/// Whether `provider` has a stale window or last error (`UsageStore.isStale`).
pub fn is_provider_stale(provider: &ProviderSnapshot) -> bool {
    provider.last_error.is_some()
        || [&provider.windows.primary, &provider.windows.secondary]
            .into_iter()
            .flatten()
            .any(|m| m.state == MetricState::Stale)
}

/// Whether `provider` lacks any displayable usage window or credits (`missingProviders`).
pub fn is_provider_missing_usage(provider: &ProviderSnapshot) -> bool {
    let has_primary = provider
        .windows
        .primary
        .as_ref()
        .is_some_and(has_displayable_metric);
    let has_secondary = provider
        .windows
        .secondary
        .as_ref()
        .is_some_and(has_displayable_metric);
    let has_credits = provider
        .credits
        .as_ref()
        .is_some_and(has_displayable_metric);
    !has_primary && !has_secondary && !has_credits
}

fn has_displayable_metric<T>(metric: &Metric<T>) -> bool {
    matches!(metric.state, MetricState::Value | MetricState::Stale) && metric.value.is_some()
}

// ---------------------------------------------------------------------------
// 4. ManualRefreshScope & MenuCardRefreshMonitor
// ---------------------------------------------------------------------------

/// Scope of a user-initiated refresh action (`StatusItemController+Actions.swift`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualRefreshScope {
    /// Refreshes all enabled providers (`ClientMessage::Refresh { provider: None }`).
    Global,
    /// Refreshes a single provider (`ClientMessage::Refresh { provider: Some(id) }`).
    Provider(String),
}

/// Tracks in-flight manual refreshes and freezes each refreshing provider's
/// last non-empty [`CardModel`] so its quota bars remain visible with a
/// `"Refreshing…"` loading subtitle (`MenuCardRefreshMonitor.swift`).
#[derive(Debug, Clone, Default)]
pub struct MenuCardRefreshMonitor {
    global_in_flight: bool,
    providers_in_flight: BTreeSet<String>,
    frozen_cards: BTreeMap<String, CardModel>,
}

impl MenuCardRefreshMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a manual refresh for `scope`, freezing the current card model(s)
    /// from `snapshot` when available (`MenuCardRefreshMonitor.beginManualRefresh`).
    pub fn begin_manual_refresh(
        &mut self,
        scope: ManualRefreshScope,
        snapshot: Option<&EngineSnapshot>,
        prefs: &CardPreferences,
        now: Timestamp,
        dates: &dyn DateTexts,
    ) {
        match scope {
            ManualRefreshScope::Global => {
                self.global_in_flight = true;
                if let Some(snap) = snapshot {
                    for p in snap.providers.iter().filter(|p| p.enabled) {
                        self.providers_in_flight.insert(p.id.as_str().to_string());
                        let model = card::provider_card(p, prefs, now, dates);
                        if !model.metrics.is_empty() {
                            self.frozen_cards.insert(p.id.as_str().to_string(), model);
                        }
                    }
                }
            }
            ManualRefreshScope::Provider(provider_id) => {
                if let Some(snap) = snapshot
                    && let Some(p) = snap
                        .providers
                        .iter()
                        .find(|p| p.id.as_str() == provider_id.as_str())
                {
                    let model = card::provider_card(p, prefs, now, dates);
                    if !model.metrics.is_empty() {
                        self.frozen_cards.insert(provider_id.clone(), model);
                    }
                }
                self.providers_in_flight.insert(provider_id);
            }
        }
    }

    /// Finishes an in-flight manual refresh for `provider_id`.
    pub fn finish_provider_refresh(&mut self, provider_id: &str) {
        self.providers_in_flight.remove(provider_id);
        self.frozen_cards.remove(provider_id);
        if self.providers_in_flight.is_empty() {
            self.global_in_flight = false;
        }
    }

    /// Reconciles in-flight manual refresh state when a new `snapshot` arrives
    /// from the engine (`MenuCardRefreshMonitor.reconcile`).
    ///
    /// Any provider whose metrics are no longer in [`MetricState::Loading`] has
    /// completed its refresh cycle, so its frozen card and in-flight flag clear.
    pub fn reconcile_snapshot(
        &mut self,
        snapshot: &EngineSnapshot,
        prefs: &CardPreferences,
        now: Timestamp,
        dates: &dyn DateTexts,
    ) {
        if !snapshot.engine.refreshing {
            self.global_in_flight = false;
        }
        let mut still_loading = BTreeSet::new();
        for p in &snapshot.providers {
            let id = p.id.as_str();
            if is_provider_actively_loading(p)
                || (snapshot.engine.refreshing
                    && (self.global_in_flight || self.providers_in_flight.contains(id)))
            {
                still_loading.insert(id.to_string());
            } else {
                self.providers_in_flight.remove(id);
                let model = card::provider_card(p, prefs, now, dates);
                if !model.metrics.is_empty() {
                    self.frozen_cards.insert(id.to_string(), model);
                } else if !snapshot.engine.refreshing {
                    self.frozen_cards.remove(id);
                }
            }
        }
        self.providers_in_flight
            .retain(|id| still_loading.contains(id));
        if self.providers_in_flight.is_empty() && !snapshot.engine.refreshing {
            self.global_in_flight = false;
        }
    }

    /// Clears all in-flight refresh state (used on engine disconnect).
    pub fn clear(&mut self) {
        self.global_in_flight = false;
        self.providers_in_flight.clear();
        self.frozen_cards.clear();
    }

    pub fn is_global_in_flight(&self) -> bool {
        self.global_in_flight
    }

    pub fn is_refreshing(&self) -> bool {
        self.global_in_flight || !self.providers_in_flight.is_empty()
    }

    pub fn refreshing_providers(&self, snapshot: Option<&EngineSnapshot>) -> BTreeSet<String> {
        let mut set = self.providers_in_flight.clone();
        if let Some(snap) = snapshot {
            for p in snap.providers.iter().filter(|p| p.enabled) {
                if self.global_in_flight
                    || snap.engine.refreshing
                    || is_provider_actively_loading(p)
                {
                    set.insert(p.id.as_str().to_string());
                }
            }
        }
        set
    }

    /// Whether `provider_id` is currently refreshing (`StatusItemController.isRefreshing`).
    pub fn is_provider_refreshing(
        &self,
        provider_id: &str,
        snapshot: Option<&EngineSnapshot>,
    ) -> bool {
        if self.global_in_flight || self.providers_in_flight.contains(provider_id) {
            return true;
        }
        snapshot.is_some_and(|snap| {
            snap.engine.refreshing
                || snap
                    .providers
                    .iter()
                    .find(|p| p.id.as_str() == provider_id)
                    .is_some_and(is_provider_actively_loading)
        })
    }

    /// Whether the `"Refresh"` menu action should be disabled as in-flight for
    /// `selection` (`StatusItemController.isRefreshActionInFlight(for:)`).
    ///
    /// A manual refresh of a single provider (`ManualRefreshScope::Provider(id)`)
    /// only disables `"Refresh"` on that provider's tab, leaving other provider
    /// tabs ready.
    pub fn is_refresh_action_in_flight(
        &self,
        selection: Option<&SwitcherSelection>,
        snapshot: Option<&EngineSnapshot>,
    ) -> bool {
        if self.global_in_flight || snapshot.is_some_and(|s| s.engine.refreshing) {
            return true;
        }
        match selection {
            Some(SwitcherSelection::Provider(id)) => {
                self.is_provider_refreshing(id.as_str(), snapshot)
            }
            Some(SwitcherSelection::Overview) => snapshot.is_some_and(|snap| {
                let enabled: Vec<&ProviderSnapshot> =
                    snap.providers.iter().filter(|p| p.enabled).collect();
                !enabled.is_empty()
                    && enabled
                        .iter()
                        .all(|p| self.is_provider_refreshing(p.id.as_str(), Some(snap)))
            }),
            None => false,
        }
    }

    /// Applies the frozen card model and `"Refreshing…"` loading subtitle when
    /// `provider_id` is refreshing (`MenuCardRefreshMonitor.effectiveModel`).
    pub fn effective_card_model(
        &self,
        provider_id: &str,
        candidate: CardModel,
        is_refreshing: bool,
    ) -> CardModel {
        if !is_refreshing {
            return candidate;
        }
        let mut base = if candidate.metrics.is_empty()
            && let Some(frozen) = self.frozen_cards.get(provider_id)
            && !frozen.metrics.is_empty()
        {
            frozen.clone()
        } else {
            candidate
        };
        base.subtitle = Subtitle {
            text: "Refreshing…".to_string(),
            style: SubtitleStyle::Loading,
        };
        base.placeholder = None;
        base
    }

    /// Whether `existing_height` and `candidate_height` match closely enough to
    /// update an open menu card in place without shifting row positions
    /// (`MenuCardRefreshMonitor.hasCompatibleTrackedLayout`).
    pub fn has_compatible_tracked_layout(existing_height: f64, candidate_height: f64) -> bool {
        (existing_height - candidate_height).abs() <= 0.5
    }
}

fn is_provider_actively_loading(provider: &ProviderSnapshot) -> bool {
    [
        &provider.windows.primary,
        &provider.windows.secondary,
        &provider.windows.tertiary,
    ]
    .into_iter()
    .flatten()
    .any(|m| m.state == MetricState::Loading)
}

// ---------------------------------------------------------------------------
// 5. MergedSwitcherContentCache & Sibling Tab Warmup
// ---------------------------------------------------------------------------

/// Pre-built menu content cached for a switcher tab
/// (`CachedMergedSwitcherMenuContent`, `StatusItemController+MergedSwitcherContentCache.swift`).
#[derive(Debug, Clone, PartialEq)]
pub struct CachedMergedSwitcherMenuContent {
    pub selection: SwitcherSelection,
    pub required_menu_content_version: u64,
    pub menu_width: f64,
    pub prefs: CardPreferences,
    pub structure: MenuStructure,
    /// Never includes flexible blank padding rows (`height == 0`).
    pub has_blank_padding_rows: bool,
}

/// Caches pre-built [`MenuStructure`]s for sibling switcher tabs and warms
/// non-selected tabs after the menu opens or rebuilds
/// (`StatusItemController+MergedSwitcherContentCache.swift`,
/// `StatusItemController+MenuSwitcherWarmup.swift`).
#[derive(Debug, Clone, Default)]
pub struct MergedSwitcherContentCache {
    entries: BTreeMap<String, CachedMergedSwitcherMenuContent>,
    preserves_caches_during_invalidation: bool,
    warmup_build_count: usize,
}

impl MergedSwitcherContentCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn selection_key(selection: &SwitcherSelection) -> String {
        match selection {
            SwitcherSelection::Overview => "overview".to_string(),
            SwitcherSelection::Provider(id) => format!("provider:{id}"),
        }
    }

    /// Looks up a cached entry matching `selection`, `required_version`, `menu_width`, and `prefs`.
    pub fn get(
        &self,
        selection: &SwitcherSelection,
        required_version: u64,
        menu_width: f64,
        prefs: &CardPreferences,
    ) -> Option<&CachedMergedSwitcherMenuContent> {
        let key = Self::selection_key(selection);
        let entry = self.entries.get(&key)?;
        if entry.required_menu_content_version == required_version
            && (entry.menu_width - menu_width).abs() <= f64::EPSILON
            && &entry.prefs == prefs
        {
            Some(entry)
        } else {
            None
        }
    }

    /// Inserts or updates a cached tab entry.
    pub fn insert(&mut self, entry: CachedMergedSwitcherMenuContent) {
        let key = Self::selection_key(&entry.selection);
        self.entries.insert(key, entry);
    }

    /// Clears all cached switcher content unless `preserves_caches_during_invalidation`
    /// is active (`StatusItemController.invalidateMenus`).
    pub fn invalidate(&mut self) {
        if !self.preserves_caches_during_invalidation {
            self.entries.clear();
        }
    }

    /// Runs `f` while preserving cached sibling content across invalidations
    /// (used when the user switches switcher tabs).
    pub fn with_preserved_caches<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let prev = self.preserves_caches_during_invalidation;
        self.preserves_caches_during_invalidation = true;
        let out = f(self);
        self.preserves_caches_during_invalidation = prev;
        out
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn warmup_build_count(&self) -> usize {
        self.warmup_build_count
    }

    pub fn cached_entries(&self) -> Vec<CachedMergedSwitcherMenuContent> {
        self.entries.values().cloned().collect()
    }

    /// Pre-warms every non-selected switcher tab for `(required_version, menu_width, prefs)`,
    /// skipping tabs already cached (`StatusItemController.warmMergedSwitcherSiblingContent`).
    /// Returns the selections that were newly built on this call.
    #[allow(clippy::too_many_arguments)]
    pub fn warm_sibling_content(
        &mut self,
        snapshot: &EngineSnapshot,
        connected: bool,
        active_selection: &SwitcherSelection,
        required_version: u64,
        menu_width: f64,
        prefs: &CardPreferences,
        now: Timestamp,
        dates: &dyn DateTexts,
        refresh_monitor: Option<&MenuCardRefreshMonitor>,
    ) -> Vec<SwitcherSelection> {
        if !connected || !switcher::shows_switcher(snapshot) {
            return Vec::new();
        }
        let mut candidates = Vec::new();
        candidates.push(SwitcherSelection::Overview);
        for p in snapshot.providers.iter().filter(|p| p.enabled) {
            candidates.push(SwitcherSelection::Provider(p.id.as_str().to_string()));
        }

        let mut warmed = Vec::new();
        for candidate in candidates {
            if &candidate == active_selection {
                continue;
            }
            if self
                .get(&candidate, required_version, menu_width, prefs)
                .is_some()
            {
                continue;
            }
            let structure = menu_descriptor::build_with_refresh(
                Some(snapshot),
                connected,
                Some(&candidate),
                prefs,
                now,
                dates,
                refresh_monitor,
            );
            self.warmup_build_count += 1;
            self.insert(CachedMergedSwitcherMenuContent {
                selection: candidate.clone(),
                required_menu_content_version: required_version,
                menu_width,
                prefs: prefs.clone(),
                structure,
                has_blank_padding_rows: false,
            });
            warmed.push(candidate);
        }
        warmed
    }
}

// ---------------------------------------------------------------------------
// 6. Position-wise Menu Row Reconciliation (`MenuRowShape`)
// ---------------------------------------------------------------------------

/// Shape of one row in the dropdown menu body (`MenuRowShape`,
/// `StatusItemController+MenuReconcile.swift:4-28`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuRowShape {
    Separator,
    DisconnectedStatus,
    CardSection {
        provider_id: String,
        has_submenu: bool,
    },
    OverviewSpendSummary,
    OverviewCard {
        provider_id: String,
        has_submenu: bool,
    },
    SecondaryText,
    Submenu {
        title: String,
        has_symbol_image: bool,
    },
    /// The persistent `"Refresh"` menu item (`StatusItemController+PersistentRefreshMenuItem.swift`),
    /// which stays attached across updates and never requires native image replacement.
    PersistentRefreshAction,
    LeafAction {
        title: String,
        has_symbol_image: bool,
    },
}

impl MenuRowShape {
    /// Native menu items with an image and no submenu cannot be repurposed in place
    /// across different actions (`MenuRowShape.requiresNativeImageReplacement`,
    /// `StatusItemController+MenuReconcile.swift:12-19`), except for the persistent
    /// `"Refresh"` item which stays attached in place.
    pub fn requires_native_image_replacement(&self) -> bool {
        matches!(
            self,
            Self::LeafAction {
                has_symbol_image: true,
                ..
            }
        )
    }

    /// Whether an existing row with shape `self` can be updated in place to `desired`
    /// (`MenuRowShape.canUpdateInPlace(to:)`, `StatusItemController+MenuReconcile.swift:56-66`).
    pub fn can_update_in_place_to(&self, desired: &Self) -> bool {
        if self.requires_native_image_replacement() || desired.requires_native_image_replacement() {
            return false;
        }
        matches!(
            (self, desired),
            (Self::Separator, Self::Separator)
                | (Self::DisconnectedStatus, Self::DisconnectedStatus)
                | (Self::CardSection { .. }, Self::CardSection { .. })
                | (Self::OverviewSpendSummary, Self::OverviewSpendSummary)
                | (Self::SecondaryText, Self::SecondaryText)
                | (Self::Submenu { .. }, Self::Submenu { .. })
                | (Self::PersistentRefreshAction, Self::PersistentRefreshAction)
                | (
                    Self::LeafAction {
                        has_symbol_image: false,
                        ..
                    },
                    Self::LeafAction {
                        has_symbol_image: false,
                        ..
                    }
                )
        ) || matches!(
            (self, desired),
            (
                Self::OverviewCard {
                    provider_id: a,
                    has_submenu: sa,
                },
                Self::OverviewCard {
                    provider_id: b,
                    has_submenu: sb,
                }
            ) if a == b && sa == sb
        )
    }
}

/// Result of reconciling an existing slice of [`MenuRowShape`]s with a desired slice
/// (`StatusItemController.reconcileMenuContent`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcilePlan {
    pub prefix_count: usize,
    pub suffix_count: usize,
    pub remove_range: Range<usize>,
    pub insert_range: Range<usize>,
}

impl ReconcilePlan {
    /// Returns `true` when every row matches in place and no items need to be
    /// removed or inserted.
    pub fn is_in_place_only(&self) -> bool {
        self.remove_range.is_empty() && self.insert_range.is_empty()
    }
}

/// Computes the common matched prefix and suffix between `existing` and `desired`,
/// isolating any structural insertions/removals to the middle slice
/// (`StatusItemController.reconcileMenuContent`, `StatusItemController+MenuReconcile.swift:30-77`).
pub fn reconcile_menu_shapes(existing: &[MenuRowShape], desired: &[MenuRowShape]) -> ReconcilePlan {
    let mut prefix_count = 0;
    while prefix_count < existing.len()
        && prefix_count < desired.len()
        && existing[prefix_count].can_update_in_place_to(&desired[prefix_count])
    {
        prefix_count += 1;
    }

    let mut suffix_count = 0;
    while prefix_count + suffix_count < existing.len()
        && prefix_count + suffix_count < desired.len()
        && existing[existing.len() - 1 - suffix_count]
            .can_update_in_place_to(&desired[desired.len() - 1 - suffix_count])
    {
        suffix_count += 1;
    }

    let remove_range = prefix_count..(existing.len() - suffix_count);
    let insert_range = prefix_count..(desired.len() - suffix_count);
    ReconcilePlan {
        prefix_count,
        suffix_count,
        remove_range,
        insert_range,
    }
}

/// Builds the exact sequence of body [`MenuRowShape`]s (after the fixed switcher
/// and separator at indices `0..2`) for `structure`.
pub fn shapes_for_structure(structure: &MenuStructure) -> Vec<MenuRowShape> {
    let mut shapes = Vec::new();
    match &structure.primary {
        PrimaryContent::Disconnected { .. } => {
            shapes.push(MenuRowShape::DisconnectedStatus);
            shapes.push(MenuRowShape::Separator);
        }
        PrimaryContent::Provider {
            provider_id,
            sections,
            cost_submenu,
            buy_credits,
            ..
        } => {
            let mut buy_inserted = false;
            for (sec_idx, section) in sections.iter().enumerate() {
                if sec_idx > 0 {
                    shapes.push(MenuRowShape::Separator);
                }
                let has_sub = section.has_cost_submenu && !cost_submenu.is_empty();
                shapes.push(MenuRowShape::CardSection {
                    provider_id: provider_id.clone(),
                    has_submenu: has_sub,
                });
                if matches!(section.mode, CardSectionMode::CreditsOnly { .. })
                    && let Some(buy) = buy_credits
                {
                    shapes.push(shape_for_entry(buy));
                    buy_inserted = true;
                }
            }
            if !buy_inserted && let Some(buy) = buy_credits {
                shapes.push(shape_for_entry(buy));
            }
            shapes.push(MenuRowShape::Separator);
        }
        PrimaryContent::Overview {
            spend_summary,
            share_action,
            cards,
        } => {
            if spend_summary.is_some() {
                shapes.push(MenuRowShape::OverviewSpendSummary);
                if let Some(share) = share_action {
                    shapes.push(shape_for_entry(share));
                }
                shapes.push(MenuRowShape::Separator);
            }
            for (idx, ov) in cards.iter().enumerate() {
                if idx > 0 {
                    shapes.push(MenuRowShape::Separator);
                }
                shapes.push(MenuRowShape::OverviewCard {
                    provider_id: ov.provider_id.clone(),
                    has_submenu: !ov.submenu.is_empty(),
                });
            }
            if !cards.is_empty() {
                shapes.push(MenuRowShape::Separator);
            }
        }
    }

    if !structure.history.is_empty() {
        for entry in &structure.history {
            shapes.push(shape_for_entry(entry));
        }
        shapes.push(MenuRowShape::Separator);
    }

    if !structure.actions.is_empty() {
        for entry in &structure.actions {
            shapes.push(shape_for_entry(entry));
        }
        shapes.push(MenuRowShape::Separator);
    }

    for entry in &structure.meta {
        shapes.push(shape_for_entry(entry));
    }

    shapes
}

pub fn shape_for_entry(entry: &MenuEntry) -> MenuRowShape {
    match entry {
        MenuEntry::SecondaryText(_) => MenuRowShape::SecondaryText,
        MenuEntry::Action {
            title,
            system_image,
            action,
            ..
        } => {
            if *action == MenuAction::Refresh {
                MenuRowShape::PersistentRefreshAction
            } else {
                MenuRowShape::LeafAction {
                    title: title.clone(),
                    has_symbol_image: system_image.is_some(),
                }
            }
        }
        MenuEntry::Submenu {
            title,
            system_image,
            ..
        } => MenuRowShape::Submenu {
            title: title.clone(),
            has_symbol_image: system_image.is_some(),
        },
    }
}

// ---------------------------------------------------------------------------
// 7. Pointer Tracking & One-Shot RunLoop Gate (`ProviderSwitcherTrackingRunLoopScheduler`)
// ---------------------------------------------------------------------------

/// One-shot execution gate used when scheduling work across multiple run-loop
/// modes (`NSRunLoopCommonModes` + `NSDefaultRunLoopMode`) so the work runs
/// during modal `NSMenu` tracking and executes at most once
/// (`ProviderSwitcherTrackingRunLoopScheduler`, `StatusItemController+ProviderSwitcher.swift:3-31`).
#[derive(Debug, Clone, Default)]
pub struct TrackingRunLoopGate {
    executed: Arc<AtomicBool>,
}

impl TrackingRunLoopGate {
    pub fn new() -> Self {
        Self {
            executed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Runs `action` if and only if this gate has not yet fired. Returns `true`
    /// when `action` ran.
    pub fn run_once(&self, action: impl FnOnce()) -> bool {
        if self
            .executed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            action();
            true
        } else {
            false
        }
    }

    pub fn has_executed(&self) -> bool {
        self.executed.load(Ordering::SeqCst)
    }
}

/// Tracks active pointer press/release state on the provider switcher row so
/// background snapshot updates arriving mid-press defer their rebuild until
/// `mouseUp:` (`StatusItemController+ProviderSwitcher.swift`, `StatusMenuSwitcherTrackingTests`).
#[derive(Debug, Clone, Default)]
pub struct SwitcherPointerTracker {
    pressed_index: Option<usize>,
    deferred_update_during_press: bool,
}

impl SwitcherPointerTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_pressing(&self) -> bool {
        self.pressed_index.is_some()
    }

    pub fn pressed_index(&self) -> Option<usize> {
        self.pressed_index
    }

    pub fn begin_press(&mut self, index: Option<usize>) {
        self.pressed_index = index;
    }

    /// Records that a background menu update arrived while a pointer press was active.
    pub fn note_deferred_update(&mut self) {
        if self.pressed_index.is_some() {
            self.deferred_update_during_press = true;
        }
    }

    pub fn has_deferred_update(&self) -> bool {
        self.deferred_update_during_press
    }

    /// Ends the press at `released_index`. Returns `(matched_index, had_deferred_update)`:
    /// - `matched_index` is `Some(idx)` only when `pressed_index == released_index`.
    /// - `had_deferred_update` is `true` when a background update was deferred during the press.
    pub fn end_press(&mut self, released_index: Option<usize>) -> (Option<usize>, bool) {
        let pressed = self.pressed_index.take();
        let deferred = std::mem::replace(&mut self.deferred_update_during_press, false);
        let matched = pressed.filter(|&p| Some(p) == released_index);
        (matched, deferred)
    }

    pub fn cancel(&mut self) -> bool {
        self.pressed_index = None;
        std::mem::replace(&mut self.deferred_update_during_press, false)
    }
}
