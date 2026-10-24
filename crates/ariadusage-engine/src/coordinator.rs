// Ported from CodexBar Sources/CodexBar/ProviderRefreshCoordinator.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ProviderRefreshCoordinatorTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Outcome of awaiting an in-flight provider refresh request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitResult {
    Completed,
    RetryRequired,
}

/// Shared state for a single in-flight provider refresh request.
pub struct ProviderRefreshRequestState {
    id: u64,
    generation: u64,
    cancel: CancellationToken,
    completed: AtomicBool,
    result_tx: watch::Sender<Option<WaitResult>>,
    result_rx: watch::Receiver<Option<WaitResult>>,
}

impl std::fmt::Debug for ProviderRefreshRequestState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderRefreshRequestState")
            .field("id", &self.id)
            .field("generation", &self.generation)
            .field("is_cancelled", &self.cancel.is_cancelled())
            .field("is_completed", &self.is_completed())
            .finish()
    }
}

impl ProviderRefreshRequestState {
    fn new(id: u64, generation: u64) -> Self {
        let (result_tx, result_rx) = watch::channel(None);
        Self {
            id,
            generation,
            cancel: CancellationToken::new(),
            completed: AtomicBool::new(false),
            result_tx,
            result_rx,
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn is_completed(&self) -> bool {
        self.completed.load(Ordering::Acquire)
    }

    fn finish(&self, retry_required: bool) {
        self.completed.store(true, Ordering::Release);
        let wait_result = if retry_required {
            WaitResult::RetryRequired
        } else {
            WaitResult::Completed
        };
        let _ = self.result_tx.send(Some(wait_result));
    }

    /// Waits for this request state to complete. Cancelling the caller's future does not
    /// cancel the underlying request task.
    pub async fn wait(&self) -> WaitResult {
        if let Some(res) = *self.result_rx.borrow() {
            return res;
        }
        let mut rx = self.result_rx.clone();
        while rx.changed().await.is_ok() {
            if let Some(res) = *rx.borrow() {
                return res;
            }
        }
        let res = *rx.borrow();
        res.unwrap_or(WaitResult::Completed)
    }
}

/// Result of starting a replacing request for a provider key.
#[derive(Debug, Clone)]
pub struct Replacement {
    pub state: Arc<ProviderRefreshRequestState>,
    pub predecessor_states: Vec<Arc<ProviderRefreshRequestState>>,
    pub generation: u64,
}

#[derive(Default)]
struct CoordinatorInner<K> {
    requests_by_key: HashMap<K, Vec<Arc<ProviderRefreshRequestState>>>,
    generations_by_key: HashMap<K, u64>,
    active_counts_by_key: HashMap<K, usize>,
}

/// Coordinates per-provider refresh coalescing, replacement ordering, generation tracking,
/// invalidation, and activity reference counts.
pub struct ProviderRefreshCoordinator<K> {
    next_id: AtomicU64,
    inner: Mutex<CoordinatorInner<K>>,
}

impl<K> Default for ProviderRefreshCoordinator<K>
where
    K: Eq + Hash + Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K> ProviderRefreshCoordinator<K>
where
    K: Eq + Hash + Clone,
{
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            inner: Mutex::new(CoordinatorInner {
                requests_by_key: HashMap::new(),
                generations_by_key: HashMap::new(),
                active_counts_by_key: HashMap::new(),
            }),
        }
    }

    /// Returns the latest active (non-completed) request state for `key` if one can be coalesced.
    pub fn coalescing_state(&self, key: &K) -> Option<Arc<ProviderRefreshRequestState>> {
        let guard = self.inner.lock().expect("coordinator lock");
        let state = guard.requests_by_key.get(key)?.last()?;
        if state.is_completed() {
            None
        } else {
            Some(Arc::clone(state))
        }
    }

    /// Begins a new request for `key` that cancels and supersedes any in-flight requests for `key`
    /// while advancing the monotonic generation counter.
    pub fn begin_replacing_request(&self, key: K) -> Replacement {
        let mut guard = self.inner.lock().expect("coordinator lock");
        let predecessor_states = guard.requests_by_key.get(&key).cloned().unwrap_or_default();
        for predecessor in &predecessor_states {
            predecessor.cancel();
        }

        let next_generation = guard
            .generations_by_key
            .get(&key)
            .copied()
            .unwrap_or(0)
            .saturating_add(1);
        guard
            .generations_by_key
            .insert(key.clone(), next_generation);

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(ProviderRefreshRequestState::new(id, next_generation));
        let mut tracked = predecessor_states.clone();
        tracked.push(Arc::clone(&state));
        guard.requests_by_key.insert(key, tracked);

        Replacement {
            state,
            predecessor_states,
            generation: next_generation,
        }
    }

    /// Invalidates all in-flight requests for `key`: advances the generation so in-flight work
    /// cannot publish, cancels all tracked tasks, and clears the coalescing slot.
    pub fn invalidate_requests(&self, key: &K) {
        let states = {
            let mut guard = self.inner.lock().expect("coordinator lock");
            let next_generation = guard
                .generations_by_key
                .get(key)
                .copied()
                .unwrap_or(0)
                .saturating_add(1);
            guard
                .generations_by_key
                .insert(key.clone(), next_generation);
            guard.requests_by_key.remove(key).unwrap_or_default()
        };
        for state in states {
            state.cancel();
        }
    }

    /// Returns whether `generation` is still the current generation for `key`.
    pub fn is_current(&self, generation: u64, key: &K) -> bool {
        let guard = self.inner.lock().expect("coordinator lock");
        guard.generations_by_key.get(key).copied().unwrap_or(0) == generation
    }

    /// Returns the current generation counter for `key` (`0` if no request has started).
    pub fn current_generation(&self, key: &K) -> u64 {
        let guard = self.inner.lock().expect("coordinator lock");
        guard.generations_by_key.get(key).copied().unwrap_or(0)
    }

    /// Marks `state` completed for `key`, notifies any waiters, and removes `state` from tracking.
    pub fn complete(
        &self,
        state: &Arc<ProviderRefreshRequestState>,
        key: &K,
        retry_required: bool,
    ) {
        state.finish(retry_required);
        let mut guard = self.inner.lock().expect("coordinator lock");
        let Some(states) = guard.requests_by_key.get_mut(key) else {
            return;
        };
        states.retain(|candidate| !Arc::ptr_eq(candidate, state));
        if states.is_empty() {
            guard.requests_by_key.remove(key);
        }
    }

    /// Waits for `state` to finish. Dropping or cancelling this future does not cancel `state`.
    pub async fn wait(&self, _key: &K, state: &Arc<ProviderRefreshRequestState>) -> WaitResult {
        state.wait().await
    }

    /// Waits for all predecessor states to finish before a replacement request begins its fetch.
    pub async fn wait_predecessors(predecessors: &[Arc<ProviderRefreshRequestState>]) {
        for predecessor in predecessors {
            let _ = predecessor.wait().await;
        }
    }

    /// Increments the active refresh count for `key`. Returns `true` when transitioning `0 -> 1`.
    pub fn begin_activity(&self, key: K) -> bool {
        let mut guard = self.inner.lock().expect("coordinator lock");
        let count = guard.active_counts_by_key.entry(key).or_insert(0);
        *count += 1;
        *count == 1
    }

    /// Decrements the active refresh count for `key`. Returns `true` when transitioning `1 -> 0`.
    pub fn end_activity(&self, key: &K) -> bool {
        let mut guard = self.inner.lock().expect("coordinator lock");
        let next = guard
            .active_counts_by_key
            .get(key)
            .copied()
            .unwrap_or(0)
            .saturating_sub(1);
        if next == 0 {
            guard.active_counts_by_key.remove(key);
            true
        } else {
            guard.active_counts_by_key.insert(key.clone(), next);
            false
        }
    }

    /// Returns whether `key` currently has any active refresh activity.
    pub fn is_active(&self, key: &K) -> bool {
        let guard = self.inner.lock().expect("coordinator lock");
        guard.active_counts_by_key.get(key).copied().unwrap_or(0) > 0
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use tokio::sync::Notify;

    use super::*;

    // upstream: Tests/CodexBarTests/ProviderRefreshCoordinatorTests.swift:6 replacement cancels and orders predecessor while advancing current generation
    #[tokio::test]
    async fn replacement_cancels_and_orders_predecessor_while_advancing_current_generation() {
        let coordinator = ProviderRefreshCoordinator::<String>::new();
        let first = coordinator.begin_replacing_request("codex".to_string());
        let first_cancel = first.state.cancel_token();
        let first_task = tokio::spawn(async move {
            first_cancel.cancelled().await;
        });

        let second = coordinator.begin_replacing_request("codex".to_string());

        assert!(first.state.is_cancelled());
        assert_eq!(second.predecessor_states.len(), 1);
        assert!(Arc::ptr_eq(&second.predecessor_states[0], &first.state));
        assert!(!coordinator.is_current(first.generation, &"codex".to_string()));
        assert!(coordinator.is_current(second.generation, &"codex".to_string()));
        first_task.await.expect("first task finishes");
    }

    // upstream: Tests/CodexBarTests/ProviderRefreshCoordinatorTests.swift:27 invalidation cancels work without dropping waiter completion
    #[tokio::test]
    async fn invalidation_cancels_work_without_dropping_waiter_completion() {
        let coordinator = Arc::new(ProviderRefreshCoordinator::<String>::new());
        let request = coordinator.begin_replacing_request("codex".to_string());
        let gate = Arc::new(Notify::new());
        let gate_wait = Arc::clone(&gate);
        let task = tokio::spawn(async move {
            gate_wait.notified().await;
        });

        let coord_wait = Arc::clone(&coordinator);
        let req_state = Arc::clone(&request.state);
        let waiter =
            tokio::spawn(async move { coord_wait.wait(&"codex".to_string(), &req_state).await });
        tokio::task::yield_now().await;

        coordinator.invalidate_requests(&"codex".to_string());

        assert!(request.state.is_cancelled());
        assert!(!coordinator.is_current(request.generation, &"codex".to_string()));
        assert!(coordinator.coalescing_state(&"codex".to_string()).is_none());

        gate.notify_waiters();
        task.await.expect("task completes");
        coordinator.complete(&request.state, &"codex".to_string(), false);
        assert_eq!(
            waiter.await.expect("waiter completes"),
            WaitResult::Completed
        );
    }

    // upstream: Tests/CodexBarTests/ProviderRefreshCoordinatorTests.swift:53 coalescing returns latest request independently per key
    #[test]
    fn coalescing_returns_latest_request_independently_per_key() {
        let coordinator = ProviderRefreshCoordinator::<String>::new();
        let first_codex = coordinator.begin_replacing_request("codex".to_string());
        let claude = coordinator.begin_replacing_request("claude".to_string());
        let latest_codex = coordinator.begin_replacing_request("codex".to_string());

        let coalesced_codex = coordinator
            .coalescing_state(&"codex".to_string())
            .expect("codex coalescing state");
        let coalesced_claude = coordinator
            .coalescing_state(&"claude".to_string())
            .expect("claude coalescing state");

        assert!(Arc::ptr_eq(&coalesced_codex, &latest_codex.state));
        assert!(Arc::ptr_eq(&coalesced_claude, &claude.state));
        assert!(!Arc::ptr_eq(&coalesced_codex, &first_codex.state));
    }

    // upstream: Tests/CodexBarTests/ProviderRefreshCoordinatorTests.swift:65 canceling one of two waiters keeps shared task alive
    #[tokio::test]
    async fn canceling_one_of_two_waiters_keeps_shared_task_alive() {
        let coordinator = Arc::new(ProviderRefreshCoordinator::<String>::new());
        let request = coordinator.begin_replacing_request("codex".to_string());
        let gate = Arc::new(Notify::new());
        let gate_wait = Arc::clone(&gate);
        let task = tokio::spawn(async move {
            gate_wait.notified().await;
        });

        let c1 = Arc::clone(&coordinator);
        let s1 = Arc::clone(&request.state);
        let owner = tokio::spawn(async move { c1.wait(&"codex".to_string(), &s1).await });

        let c2 = Arc::clone(&coordinator);
        let s2 = Arc::clone(&request.state);
        let shared = tokio::spawn(async move { c2.wait(&"codex".to_string(), &s2).await });

        tokio::task::yield_now().await;
        owner.abort();
        let _ = owner.await;

        assert!(!request.state.is_cancelled());

        gate.notify_waiters();
        task.await.expect("task completes");
        coordinator.complete(&request.state, &"codex".to_string(), false);
        assert_eq!(shared.await.expect("shared waiter"), WaitResult::Completed);
    }

    // upstream: Tests/CodexBarTests/ProviderRefreshCoordinatorTests.swift:93 wait result exposes retry without leaking task state
    #[tokio::test]
    async fn wait_result_exposes_retry_without_leaking_task_state() {
        let coordinator = ProviderRefreshCoordinator::<String>::new();
        let request = coordinator.begin_replacing_request("codex".to_string());
        coordinator.complete(&request.state, &"codex".to_string(), true);

        let result = coordinator.wait(&"codex".to_string(), &request.state).await;
        assert_eq!(result, WaitResult::RetryRequired);
    }

    // upstream: Tests/CodexBarTests/ProviderRefreshCoordinatorTests.swift:106 completed request is not offered for coalescing before deferred removal
    #[test]
    fn completed_request_is_not_offered_for_coalescing() {
        let coordinator = ProviderRefreshCoordinator::<String>::new();
        let request = coordinator.begin_replacing_request("codex".to_string());
        coordinator.complete(&request.state, &"codex".to_string(), false);

        assert!(coordinator.coalescing_state(&"codex".to_string()).is_none());
    }

    // upstream: Tests/CodexBarTests/ProviderRefreshCoordinatorTests.swift:118 completion removal and activity counts are key scoped
    #[test]
    fn completion_removal_and_activity_counts_are_key_scoped() {
        let coordinator = ProviderRefreshCoordinator::<String>::new();
        let codex = coordinator.begin_replacing_request("codex".to_string());
        let claude = coordinator.begin_replacing_request("claude".to_string());

        assert!(coordinator.begin_activity("codex".to_string()));
        assert!(!coordinator.begin_activity("codex".to_string()));
        assert!(coordinator.begin_activity("claude".to_string()));
        assert!(!coordinator.end_activity(&"codex".to_string()));
        assert!(coordinator.end_activity(&"codex".to_string()));
        assert!(coordinator.end_activity(&"claude".to_string()));

        coordinator.complete(&codex.state, &"codex".to_string(), true);
        coordinator.complete(&claude.state, &"claude".to_string(), false);

        assert!(coordinator.coalescing_state(&"codex".to_string()).is_none());
        assert!(
            coordinator
                .coalescing_state(&"claude".to_string())
                .is_none()
        );
    }
}
