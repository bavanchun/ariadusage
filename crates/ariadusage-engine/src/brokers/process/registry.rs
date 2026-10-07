// Ported from CodexBar Sources/CodexBarCore/Host/PTY/TTYCommandRunner.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::Notify;

use super::ProcessError;
use super::teardown::{ProcessTarget, terminate};

#[derive(Clone, Default)]
pub struct ProcessRegistry {
    inner: Arc<RegistryInner>,
}

#[derive(Default)]
struct RegistryInner {
    state: Mutex<RegistryState>,
    changed: Notify,
    next_id: AtomicU64,
}

#[derive(Default)]
struct RegistryState {
    fenced: bool,
    launches: HashMap<u64, LaunchEntry>,
}

#[derive(Clone)]
struct LaunchEntry {
    target: Option<ProcessTarget>,
}

pub(crate) struct LaunchPermit {
    inner: Arc<RegistryInner>,
    id: u64,
    active: bool,
}

impl ProcessRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a launch unless shutdown has already fenced new work.
    pub(crate) fn register(&self) -> Result<LaunchPermit, ProcessError> {
        let mut state = lock(&self.inner.state);
        if state.fenced {
            return Err(ProcessError::ShuttingDown);
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        state.launches.insert(id, LaunchEntry { target: None });
        Ok(LaunchPermit {
            inner: Arc::clone(&self.inner),
            id,
            active: true,
        })
    }

    /// Prevents registrations made after this call.
    pub fn fence(&self) {
        lock(&self.inner.state).fenced = true;
        self.inner.changed.notify_waiters();
    }

    pub fn is_fenced(&self) -> bool {
        lock(&self.inner.state).fenced
    }

    /// Terminates registered targets and waits until every launch has completed.
    pub async fn shutdown(&self) {
        self.fence();
        loop {
            let notified = self.inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let (pending, targets) = {
                let state = lock(&self.inner.state);
                let pending = state.launches.values().any(|entry| entry.target.is_none());
                let targets = state
                    .launches
                    .values()
                    .filter_map(|entry| entry.target.clone())
                    .collect::<Vec<_>>();
                (pending, targets)
            };
            if pending {
                notified.await;
                continue;
            }
            for target in targets {
                terminate(&target).await;
            }
            break;
        }
        self.drain().await;
    }

    /// Waits until all registered launches finish.
    pub async fn drain(&self) {
        loop {
            let notified = self.inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if lock(&self.inner.state).launches.is_empty() {
                return;
            }
            notified.await;
        }
    }

    pub fn active_launches(&self) -> usize {
        lock(&self.inner.state).launches.len()
    }
}

impl LaunchPermit {
    pub(super) fn attach(&mut self, target: ProcessTarget) {
        if !self.active {
            return;
        }
        if let Some(entry) = lock(&self.inner.state).launches.get_mut(&self.id) {
            entry.target = Some(target);
        }
        self.inner.changed.notify_waiters();
    }

    pub fn finish(&mut self) {
        if self.active {
            self.active = false;
            lock(&self.inner.state).launches.remove(&self.id);
            self.inner.changed.notify_waiters();
        }
    }
}

impl Drop for LaunchPermit {
    fn drop(&mut self) {
        self.finish();
    }
}

impl std::fmt::Debug for ProcessRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = lock(&self.inner.state);
        formatter
            .debug_struct("ProcessRegistry")
            .field("fenced", &state.fenced)
            .field("active_launches", &state.launches.len())
            .finish()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
