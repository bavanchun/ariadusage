// Ported from CodexBar Sources/CodexBarCLI/CLIServeOperationCoordinator.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
//
// Per-key operation coalescing, partial-progress publication, deadline tightening,
// and non-overlapping successor queuing for `serve`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, oneshot};
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub type BoxOperationFuture<Value> = Pin<Box<dyn Future<Output = Value> + Send>>;
pub type AcceptTransformFn<Value> = Arc<dyn Fn(Value) -> BoxOperationFuture<Value> + Send + Sync>;
pub type OperationWithProgressFn<Value> =
    Arc<dyn Fn(CliServeOperationProgressSink<Value>) -> BoxOperationFuture<Value> + Send + Sync>;
pub type PublishProgressFn<Value> =
    Arc<dyn Fn(Value) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;
pub type NowFn = Arc<dyn Fn() -> Instant + Send + Sync>;
pub type SleepUntilFn = Arc<
    dyn Fn(Instant) -> Pin<Box<dyn Future<Output = Result<(), CoordinatorCancelled>> + Send>>
        + Send
        + Sync,
>;

/// Error returned when a sleep or waiter is cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("operation cancelled")]
pub struct CoordinatorCancelled;

/// Point-in-time snapshot of coordinator state (`CLIServeOperationCoordinator.Snapshot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoordinatorSnapshot {
    pub operation_count: usize,
    pub waiter_count: usize,
    pub timer_count: usize,
    pub is_shut_down: bool,
}

/// Progress sink passed to an in-flight operation so it can publish partial snapshots
/// that timed-out waiters may return, and observe cooperative cancellation.
#[derive(Clone)]
pub struct CliServeOperationProgressSink<Value: Clone + Send + Sync + 'static> {
    publish: PublishProgressFn<Value>,
    cancel_token: CancellationToken,
}

impl<Value: Clone + Send + Sync + 'static> CliServeOperationProgressSink<Value> {
    #[must_use]
    pub fn new<F, Fut>(publish: F, cancel_token: CancellationToken) -> Self
    where
        F: Fn(Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self {
            publish: Arc::new(move |val| Box::pin(publish(val))),
            cancel_token,
        }
    }

    pub async fn publish(&self, value: Value) {
        (self.publish)(value).await;
    }

    pub async fn publish_partial(&self, value: Value) {
        (self.publish)(value).await;
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    #[must_use]
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }
}

struct Waiter<Value> {
    id: Uuid,
    tx: Option<oneshot::Sender<Value>>,
    timeout_value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationPhase {
    Running,
    Accepting,
    TimedOut,
}

struct Operation<Value: Clone + Send + Sync + 'static> {
    generation: u64,
    fingerprint: String,
    deadline: Option<Instant>,
    make_value: OperationWithProgressFn<Value>,
    accept_value: AcceptTransformFn<Value>,
    cancel_token: CancellationToken,
    deadline_abort: Option<AbortHandle>,
    waiters: Vec<Waiter<Value>>,
    phase: OperationPhase,
    partial_value: Option<Value>,
}

struct Slot<Value: Clone + Send + Sync + 'static> {
    active: Option<Operation<Value>>,
    pending: Option<Operation<Value>>,
}

#[derive(Clone, Copy)]
enum WaitTarget {
    Active,
    Pending,
}

struct CoordinatorState<Value: Clone + Send + Sync + 'static> {
    slots: HashMap<String, Slot<Value>>,
    next_generation: u64,
    is_shut_down: bool,
}

/// Coalesces concurrent `serve` operations by key and fingerprint while respecting
/// per-caller deadlines and non-overlapping successor execution.
pub struct CliServeOperationCoordinator<Value: Clone + Send + Sync + 'static> {
    now: NowFn,
    sleep_until: SleepUntilFn,
    state: Arc<Mutex<CoordinatorState<Value>>>,
}

impl<Value: Clone + Send + Sync + 'static> Default for CliServeOperationCoordinator<Value> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Value: Clone + Send + Sync + 'static> Clone for CliServeOperationCoordinator<Value> {
    fn clone(&self) -> Self {
        Self {
            now: Arc::clone(&self.now),
            sleep_until: Arc::clone(&self.sleep_until),
            state: Arc::clone(&self.state),
        }
    }
}

impl<Value: Clone + Send + Sync + 'static> CliServeOperationCoordinator<Value> {
    #[must_use]
    pub fn new() -> Self {
        Self::with_clock(
            Arc::new(Instant::now),
            Arc::new(|deadline| {
                Box::pin(async move {
                    let now = Instant::now();
                    if deadline > now {
                        tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
                    }
                    Ok(())
                })
            }),
        )
    }

    #[must_use]
    pub fn with_clock(now: NowFn, sleep_until: SleepUntilFn) -> Self {
        Self {
            now,
            sleep_until,
            state: Arc::new(Mutex::new(CoordinatorState {
                slots: HashMap::new(),
                next_generation: 0,
                is_shut_down: false,
            })),
        }
    }

    pub async fn snapshot(&self) -> CoordinatorSnapshot {
        let guard = self.state.lock().await;
        let mut operation_count = 0;
        let mut waiter_count = 0;
        let mut timer_count = 0;
        for slot in guard.slots.values() {
            for op in [slot.active.as_ref(), slot.pending.as_ref()]
                .into_iter()
                .flatten()
            {
                operation_count += 1;
                waiter_count += op.waiters.len();
                if op.deadline_abort.is_some() {
                    timer_count += 1;
                }
            }
        }
        CoordinatorSnapshot {
            operation_count,
            waiter_count,
            timer_count,
            is_shut_down: guard.is_shut_down,
        }
    }

    pub async fn value<A, AOut, Op, Fut>(
        &self,
        key: &str,
        fingerprint: Option<&str>,
        deadline: Option<Instant>,
        timeout_value: Value,
        accept: A,
        operation: Op,
    ) -> Value
    where
        A: Fn(Value) -> AOut + Send + Sync + 'static,
        AOut: Future<Output = Value> + Send + 'static,
        Op: Fn(CliServeOperationProgressSink<Value>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Value> + Send + 'static,
    {
        self.value_with_progress(
            key,
            fingerprint.unwrap_or(key),
            deadline,
            timeout_value,
            Arc::new(move |v| Box::pin(accept(v))),
            Arc::new(move |sink| Box::pin(operation(sink))),
        )
        .await
    }

    pub async fn value_with_progress(
        &self,
        key: &str,
        fingerprint: &str,
        deadline: Option<Instant>,
        timeout_value: Value,
        accept: AcceptTransformFn<Value>,
        make_value: OperationWithProgressFn<Value>,
    ) -> Value {
        let waiter_id = Uuid::new_v4();
        let (tx, rx) = oneshot::channel();

        {
            let mut guard = self.state.lock().await;
            if guard.is_shut_down || self.is_expired(deadline) {
                return timeout_value;
            }
            self.expire_overdue_operations(&mut guard, key);

            if let Some(slot) = guard.slots.get(key) {
                if let Some(active) = slot.active.as_ref() {
                    if active.fingerprint == fingerprint && active.phase != OperationPhase::TimedOut
                    {
                        if !Self::deadlines_are_compatible(active.deadline, deadline) {
                            return timeout_value;
                        }
                        if !Self::can_join_accepting(active, deadline) {
                            return timeout_value;
                        }
                        self.join_target(
                            &mut guard,
                            key,
                            fingerprint,
                            deadline,
                            Waiter {
                                id: waiter_id,
                                tx: Some(tx),
                                timeout_value: timeout_value.clone(),
                            },
                            WaitTarget::Active,
                        );
                    } else if let Some(pending) = slot.pending.as_ref()
                        && pending.fingerprint == fingerprint
                    {
                        if !Self::deadlines_are_compatible(pending.deadline, deadline) {
                            return timeout_value;
                        }
                        self.join_target(
                            &mut guard,
                            key,
                            fingerprint,
                            deadline,
                            Waiter {
                                id: waiter_id,
                                tx: Some(tx),
                                timeout_value: timeout_value.clone(),
                            },
                            WaitTarget::Pending,
                        );
                    } else {
                        if let Some(pending_gen) = guard
                            .slots
                            .get(key)
                            .and_then(|s| s.pending.as_ref())
                            .map(|p| p.generation)
                        {
                            Self::timeout_pending(&mut guard, key, pending_gen);
                        }
                        self.install_pending(
                            &mut guard,
                            key,
                            fingerprint,
                            deadline,
                            Waiter {
                                id: waiter_id,
                                tx: Some(tx),
                                timeout_value: timeout_value.clone(),
                            },
                            accept,
                            make_value,
                        );
                    }
                } else {
                    self.install_active(
                        &mut guard,
                        key,
                        fingerprint,
                        deadline,
                        Waiter {
                            id: waiter_id,
                            tx: Some(tx),
                            timeout_value: timeout_value.clone(),
                        },
                        accept,
                        make_value,
                    );
                }
            } else {
                self.install_active(
                    &mut guard,
                    key,
                    fingerprint,
                    deadline,
                    Waiter {
                        id: waiter_id,
                        tx: Some(tx),
                        timeout_value: timeout_value.clone(),
                    },
                    accept,
                    make_value,
                );
            }
        }

        let mut drop_guard = WaiterDropGuard {
            coordinator: self.clone(),
            key: key.to_string(),
            waiter_id,
            completed: false,
        };

        let res = rx.await;
        drop_guard.completed = true;
        res.unwrap_or(timeout_value)
    }

    pub async fn shutdown(&self) {
        let mut guard = self.state.lock().await;
        if guard.is_shut_down {
            return;
        }
        guard.is_shut_down = true;
        let active: Vec<(String, u64)> = guard
            .slots
            .iter()
            .filter_map(|(k, s)| s.active.as_ref().map(|a| (k.clone(), a.generation)))
            .collect();
        let pending: Vec<(String, u64)> = guard
            .slots
            .iter()
            .filter_map(|(k, s)| s.pending.as_ref().map(|p| (k.clone(), p.generation)))
            .collect();
        for (k, generation) in pending {
            Self::timeout_pending(&mut guard, &k, generation);
        }
        for (k, generation) in active {
            Self::shutdown_active(&mut guard, &k, generation);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn install_active(
        &self,
        guard: &mut CoordinatorState<Value>,
        key: &str,
        fingerprint: &str,
        deadline: Option<Instant>,
        waiter: Waiter<Value>,
        accept_value: AcceptTransformFn<Value>,
        make_value: OperationWithProgressFn<Value>,
    ) {
        guard.next_generation = guard.next_generation.wrapping_add(1);
        let generation = guard.next_generation;
        let cancel_token = CancellationToken::new();
        let deadline_abort = self.make_deadline_task(key.to_string(), generation, deadline);
        self.spawn_source_task(
            key.to_string(),
            generation,
            Arc::clone(&make_value),
            cancel_token.clone(),
        );
        let op = Operation {
            generation,
            fingerprint: fingerprint.to_string(),
            deadline,
            make_value,
            accept_value,
            cancel_token,
            deadline_abort,
            waiters: vec![waiter],
            phase: OperationPhase::Running,
            partial_value: None,
        };
        guard.slots.insert(
            key.to_string(),
            Slot {
                active: Some(op),
                pending: None,
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn install_pending(
        &self,
        guard: &mut CoordinatorState<Value>,
        key: &str,
        fingerprint: &str,
        deadline: Option<Instant>,
        waiter: Waiter<Value>,
        accept_value: AcceptTransformFn<Value>,
        make_value: OperationWithProgressFn<Value>,
    ) {
        if guard.slots.get(key).is_none_or(|s| s.active.is_none()) {
            self.install_active(
                guard,
                key,
                fingerprint,
                deadline,
                waiter,
                accept_value,
                make_value,
            );
            return;
        }
        guard.next_generation = guard.next_generation.wrapping_add(1);
        let generation = guard.next_generation;
        let cancel_token = CancellationToken::new();
        let deadline_abort = self.make_deadline_task(key.to_string(), generation, deadline);
        let op = Operation {
            generation,
            fingerprint: fingerprint.to_string(),
            deadline,
            make_value,
            accept_value,
            cancel_token,
            deadline_abort,
            waiters: vec![waiter],
            phase: OperationPhase::Running,
            partial_value: None,
        };
        if let Some(slot) = guard.slots.get_mut(key) {
            slot.pending = Some(op);
        }
    }

    fn join_target(
        &self,
        guard: &mut CoordinatorState<Value>,
        key: &str,
        fingerprint: &str,
        deadline: Option<Instant>,
        mut waiter: Waiter<Value>,
        target: WaitTarget,
    ) {
        let Some(slot) = guard.slots.get_mut(key) else {
            if let Some(tx) = waiter.tx.take() {
                let _ = tx.send(waiter.timeout_value);
            }
            return;
        };
        match target {
            WaitTarget::Active => {
                let Some(op) = slot.active.as_mut() else {
                    if let Some(tx) = waiter.tx.take() {
                        let _ = tx.send(waiter.timeout_value);
                    }
                    return;
                };
                if op.fingerprint != fingerprint
                    || op.phase == OperationPhase::TimedOut
                    || !Self::can_join_accepting(op, deadline)
                {
                    if let Some(tx) = waiter.tx.take() {
                        let _ = tx.send(waiter.timeout_value);
                    }
                    return;
                }
                self.tighten_deadline(op, deadline, key);
                op.waiters.push(waiter);
            }
            WaitTarget::Pending => {
                let Some(op) = slot.pending.as_mut() else {
                    if let Some(tx) = waiter.tx.take() {
                        let _ = tx.send(waiter.timeout_value);
                    }
                    return;
                };
                if op.fingerprint != fingerprint {
                    if let Some(tx) = waiter.tx.take() {
                        let _ = tx.send(waiter.timeout_value);
                    }
                    return;
                }
                self.tighten_deadline(op, deadline, key);
                op.waiters.push(waiter);
            }
        }
    }

    fn tighten_deadline(&self, op: &mut Operation<Value>, requested: Option<Instant>, key: &str) {
        if op.phase != OperationPhase::Running {
            return;
        }
        let (Some(current), Some(requested)) = (op.deadline, requested) else {
            return;
        };
        if requested >= current {
            return;
        }
        if let Some(abort) = op.deadline_abort.take() {
            abort.abort();
        }
        op.deadline = Some(requested);
        op.deadline_abort =
            self.make_deadline_task(key.to_string(), op.generation, Some(requested));
    }

    fn spawn_source_task(
        &self,
        key: String,
        generation: u64,
        make_value: OperationWithProgressFn<Value>,
        cancel_token: CancellationToken,
    ) {
        let coord = self.clone();
        let key_for_partial = key.clone();
        let sink = CliServeOperationProgressSink::new(
            move |partial| {
                let coord = coord.clone();
                let k = key_for_partial.clone();
                async move {
                    coord.publish_partial(&k, generation, partial).await;
                }
            },
            cancel_token,
        );
        let coord_for_task = self.clone();
        let _: JoinHandle<()> = tokio::spawn(async move {
            let value = make_value(sink).await;
            coord_for_task
                .source_produced(&key, generation, value)
                .await;
        });
    }

    fn make_deadline_task(
        &self,
        key: String,
        generation: u64,
        deadline: Option<Instant>,
    ) -> Option<AbortHandle> {
        let deadline = deadline?;
        let sleep_until = Arc::clone(&self.sleep_until);
        let coord = self.clone();
        let handle = tokio::spawn(async move {
            if sleep_until(deadline).await.is_err() {
                return;
            }
            coord.deadline_reached(&key, generation).await;
        });
        Some(handle.abort_handle())
    }

    async fn publish_partial(&self, key: &str, generation: u64, value: Value) {
        let mut guard = self.state.lock().await;
        let expired = guard
            .slots
            .get(key)
            .and_then(|s| s.active.as_ref())
            .is_none_or(|op| self.is_expired(op.deadline));
        if expired {
            return;
        }
        if let Some(slot) = guard.slots.get_mut(key)
            && let Some(op) = slot.active.as_mut()
            && op.generation == generation
            && op.phase == OperationPhase::Running
        {
            op.partial_value = Some(value);
        }
    }

    async fn cancel_waiter(&self, key: &str, waiter_id: Uuid) {
        let mut guard = self.state.lock().await;
        let Some(slot) = guard.slots.get_mut(key) else {
            return;
        };
        if let Some(active) = slot.active.as_mut()
            && let Some(idx) = active.waiters.iter().position(|w| w.id == waiter_id)
        {
            let mut waiter = active.waiters.remove(idx);
            let partial = active.partial_value.clone();
            let is_empty = active.waiters.is_empty();
            let is_running = active.phase == OperationPhase::Running;
            let generation = active.generation;
            if let Some(tx) = waiter.tx.take() {
                let _ = tx.send(partial.unwrap_or(waiter.timeout_value));
            }
            if is_empty && is_running {
                Self::timeout_active(&mut guard, key, generation);
            }
            return;
        }

        if let Some(pending) = slot.pending.as_mut()
            && let Some(idx) = pending.waiters.iter().position(|w| w.id == waiter_id)
        {
            let mut waiter = pending.waiters.remove(idx);
            if let Some(tx) = waiter.tx.take() {
                let _ = tx.send(waiter.timeout_value);
            }
            if pending.waiters.is_empty() {
                pending.cancel_token.cancel();
                if let Some(abort) = pending.deadline_abort.take() {
                    abort.abort();
                }
                slot.pending = None;
            }
            Self::cleanup_slot(&mut guard, key);
        }
    }

    async fn deadline_reached(&self, key: &str, generation: u64) {
        let mut guard = self.state.lock().await;
        if guard
            .slots
            .get(key)
            .and_then(|s| s.active.as_ref())
            .is_some_and(|a| a.generation == generation)
        {
            Self::timeout_active(&mut guard, key, generation);
        } else if guard
            .slots
            .get(key)
            .and_then(|s| s.pending.as_ref())
            .is_some_and(|p| p.generation == generation)
        {
            Self::timeout_pending(&mut guard, key, generation);
        }
    }

    fn expire_overdue_operations(&self, guard: &mut CoordinatorState<Value>, key: &str) {
        if let Some(active_gen) = guard
            .slots
            .get(key)
            .and_then(|s| s.active.as_ref())
            .and_then(|a| {
                (a.phase == OperationPhase::Running && self.is_expired(a.deadline))
                    .then_some(a.generation)
            })
        {
            Self::timeout_active(guard, key, active_gen);
        }
        if let Some(pending_gen) = guard
            .slots
            .get(key)
            .and_then(|s| s.pending.as_ref())
            .and_then(|p| self.is_expired(p.deadline).then_some(p.generation))
        {
            Self::timeout_pending(guard, key, pending_gen);
        }
    }

    fn timeout_active(guard: &mut CoordinatorState<Value>, key: &str, generation: u64) {
        let Some(slot) = guard.slots.get_mut(key) else {
            return;
        };
        let Some(op) = slot.active.as_mut() else {
            return;
        };
        if op.generation != generation || op.phase != OperationPhase::Running {
            return;
        }
        op.phase = OperationPhase::TimedOut;
        op.cancel_token.cancel();
        if let Some(abort) = op.deadline_abort.take() {
            abort.abort();
        }
        let waiters = std::mem::take(&mut op.waiters);
        let partial = op.partial_value.clone();
        for mut waiter in waiters {
            if let Some(tx) = waiter.tx.take() {
                let _ = tx.send(partial.clone().unwrap_or(waiter.timeout_value));
            }
        }
    }

    fn shutdown_active(guard: &mut CoordinatorState<Value>, key: &str, generation: u64) {
        let Some(phase) = guard
            .slots
            .get(key)
            .and_then(|s| s.active.as_ref())
            .filter(|op| op.generation == generation)
            .map(|op| op.phase)
        else {
            return;
        };
        if phase == OperationPhase::Running {
            Self::timeout_active(guard, key, generation);
            return;
        }
        if phase != OperationPhase::Accepting {
            return;
        }
        if let Some(slot) = guard.slots.get_mut(key)
            && let Some(op) = slot.active.as_mut()
        {
            op.phase = OperationPhase::TimedOut;
            op.cancel_token.cancel();
            let waiters = std::mem::take(&mut op.waiters);
            let partial = op.partial_value.clone();
            for mut waiter in waiters {
                if let Some(tx) = waiter.tx.take() {
                    let _ = tx.send(partial.clone().unwrap_or(waiter.timeout_value));
                }
            }
        }
    }

    fn timeout_pending(guard: &mut CoordinatorState<Value>, key: &str, generation: u64) {
        let Some(slot) = guard.slots.get_mut(key) else {
            return;
        };
        let Some(pending) = slot.pending.as_ref() else {
            return;
        };
        if pending.generation != generation {
            return;
        }
        let mut op = slot.pending.take().expect("pending exists");
        op.cancel_token.cancel();
        if let Some(abort) = op.deadline_abort.take() {
            abort.abort();
        }
        Self::cleanup_slot(guard, key);
        for mut waiter in op.waiters {
            if let Some(tx) = waiter.tx.take() {
                let _ = tx.send(waiter.timeout_value);
            }
        }
    }

    async fn source_produced(&self, key: &str, generation: u64, value: Value) {
        let accept_fn = {
            let mut guard = self.state.lock().await;
            let Some(observed_deadline) = guard
                .slots
                .get(key)
                .and_then(|s| s.active.as_ref())
                .filter(|a| a.generation == generation)
                .map(|a| (a.phase, a.deadline))
            else {
                return;
            };
            if observed_deadline.0 == OperationPhase::Running
                && self.is_expired(observed_deadline.1)
            {
                Self::timeout_active(&mut guard, key, generation);
            }
            let Some(slot) = guard.slots.get_mut(key) else {
                return;
            };
            let Some(op) = slot.active.as_mut() else {
                return;
            };
            if op.generation != generation {
                return;
            }
            if op.phase != OperationPhase::TimedOut {
                op.phase = OperationPhase::Accepting;
                if let Some(abort) = op.deadline_abort.take() {
                    abort.abort();
                }
            }
            Arc::clone(&op.accept_value)
        };

        let accepted_value = accept_fn(value).await;

        let mut guard = self.state.lock().await;
        let Some(slot) = guard.slots.get_mut(key) else {
            return;
        };
        let Some(completed) = slot
            .active
            .as_ref()
            .filter(|a| a.generation == generation)
            .map(|a| (a.phase, a.fingerprint.clone()))
        else {
            return;
        };
        let mut active_op = slot.active.take().expect("active exists");

        if completed.0 == OperationPhase::TimedOut {
            if let Some(pending) = slot.pending.as_ref()
                && pending.fingerprint == completed.1
            {
                let mut pending_op = slot.pending.take().expect("pending exists");
                if let Some(abort) = pending_op.deadline_abort.take() {
                    abort.abort();
                }
                Self::cleanup_slot(&mut guard, key);
                for mut waiter in pending_op.waiters {
                    if let Some(tx) = waiter.tx.take() {
                        let _ = tx.send(accepted_value.clone());
                    }
                }
                return;
            }
            Self::cleanup_slot(&mut guard, key);
            self.promote_pending(&mut guard, key);
            return;
        }

        Self::cleanup_slot(&mut guard, key);
        if completed.0 == OperationPhase::Accepting {
            for mut waiter in active_op.waiters.drain(..) {
                if let Some(tx) = waiter.tx.take() {
                    let _ = tx.send(accepted_value.clone());
                }
            }
        }
        self.promote_pending(&mut guard, key);
    }

    fn promote_pending(&self, guard: &mut CoordinatorState<Value>, key: &str) {
        if guard.is_shut_down {
            return;
        }
        let Some((pending_gen, pending_deadline)) = guard
            .slots
            .get(key)
            .filter(|s| s.active.is_none())
            .and_then(|s| s.pending.as_ref())
            .map(|p| (p.generation, p.deadline))
        else {
            return;
        };
        if self.is_expired(pending_deadline) {
            Self::timeout_pending(guard, key, pending_gen);
            return;
        }
        if let Some(slot) = guard.slots.get_mut(key)
            && let Some(pending) = slot.pending.take()
        {
            self.spawn_source_task(
                key.to_string(),
                pending.generation,
                Arc::clone(&pending.make_value),
                pending.cancel_token.clone(),
            );
            slot.active = Some(pending);
        }
    }

    const fn deadlines_are_compatible(active: Option<Instant>, requested: Option<Instant>) -> bool {
        active.is_none() == requested.is_none()
    }

    fn can_join_accepting(op: &Operation<Value>, deadline: Option<Instant>) -> bool {
        if op.phase != OperationPhase::Accepting {
            return true;
        }
        let (Some(active_deadline), Some(deadline)) = (op.deadline, deadline) else {
            return true;
        };
        deadline >= active_deadline
    }

    fn is_expired(&self, deadline: Option<Instant>) -> bool {
        let Some(deadline) = deadline else {
            return false;
        };
        deadline <= (self.now)()
    }

    fn cleanup_slot(guard: &mut CoordinatorState<Value>, key: &str) {
        if guard
            .slots
            .get(key)
            .is_some_and(|s| s.active.is_none() && s.pending.is_none())
        {
            guard.slots.remove(key);
        }
    }
}

struct WaiterDropGuard<Value: Clone + Send + Sync + 'static> {
    coordinator: CliServeOperationCoordinator<Value>,
    key: String,
    waiter_id: Uuid,
    completed: bool,
}

impl<Value: Clone + Send + Sync + 'static> Drop for WaiterDropGuard<Value> {
    fn drop(&mut self) {
        if !self.completed {
            let coord = self.coordinator.clone();
            let key = self.key.clone();
            let waiter_id = self.waiter_id;
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    coord.cancel_waiter(&key, waiter_id).await;
                });
            }
        }
    }
}
