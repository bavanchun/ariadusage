// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexCLILaunchGate.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::pipeline::FetchInteraction;

/// How a process launch failed, so PTY setup failures do not throttle the binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchFailureKind {
    Process,
    PtyInfrastructure,
}

type Clock = dyn Fn() -> Instant + Send + Sync;

/// Suppresses repeated background launches after a process launch failure.
#[derive(Clone)]
pub struct LaunchGate {
    now: Arc<Clock>,
    entries: Arc<Mutex<HashMap<String, Instant>>>,
}

impl LaunchGate {
    pub const COOLDOWN: Duration = Duration::from_secs(30 * 60);

    /// Creates a launch gate backed by the supplied monotonic clock.
    pub fn new(now: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        Self {
            now: Arc::new(now),
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Returns the cooldown deadline when a background launch should be skipped.
    pub fn check(&self, binary: &str, interaction: FetchInteraction) -> Result<(), Instant> {
        if interaction == FetchInteraction::UserInitiated {
            return Ok(());
        }

        let now = (self.now)();
        let mut entries = lock(&self.entries);
        match entries.get(binary).copied() {
            Some(until) if until > now => Err(until),
            Some(_) => {
                entries.remove(binary);
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Records a launch failure unless it came from PTY infrastructure.
    pub fn record_failure(&self, binary: &str, kind: LaunchFailureKind) -> Option<Instant> {
        if kind == LaunchFailureKind::PtyInfrastructure {
            return None;
        }
        let now = (self.now)();
        let until = now.checked_add(Self::COOLDOWN).unwrap_or(now);
        lock(&self.entries).insert(binary.to_owned(), until);
        Some(until)
    }
}

impl Default for LaunchGate {
    fn default() -> Self {
        Self::new(Instant::now)
    }
}

impl std::fmt::Debug for LaunchGate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LaunchGate")
            .field("suppressed_binaries", &lock(&self.entries).len())
            .finish()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
