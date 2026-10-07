//! Cooldown state machine for delegated CLI OAuth refresh attempts.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::pipeline::FetchInteraction;

/// Outcome of a delegated refresh attempt used to finalize the cooldown duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DelegatedRefreshOutcome {
    /// Refresh observed a credential change / successful refresh (5 min cooldown).
    ObservedSuccess,
    /// Refresh completed cleanly but produced an unreadable or unobservable credential source (5 min cooldown).
    UnreadableResult,
    /// Refresh failed, timed out, or resulted in unchanged credentials (20 s cooldown).
    FailedOrUnchanged,
}

impl fmt::Display for DelegatedRefreshOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ObservedSuccess => write!(f, "observed_success"),
            Self::UnreadableResult => write!(f, "unreadable_result"),
            Self::FailedOrUnchanged => write!(f, "failed_or_unchanged"),
        }
    }
}

/// Persisted cooldown state for a single profile digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CooldownState {
    pub last_attempt_at: Timestamp,
    pub interval_secs: u64,
}

impl CooldownState {
    /// Creates a new cooldown state.
    pub fn new(last_attempt_at: Timestamp, interval: Duration) -> Self {
        Self {
            last_attempt_at,
            interval_secs: interval.as_secs(),
        }
    }

    /// Returns the cooldown duration.
    pub fn interval(&self) -> Duration {
        Duration::from_secs(self.interval_secs)
    }

    /// Returns the timestamp when this cooldown expires, if computable.
    pub fn expires_at(&self) -> Option<Timestamp> {
        let span = jiff::Span::new().seconds(self.interval_secs as i64);
        self.last_attempt_at.checked_add(span).ok()
    }

    /// Checks if this cooldown is currently active relative to `now`.
    pub fn is_active(&self, now: Timestamp) -> bool {
        match self.expires_at() {
            Some(exp) => now < exp,
            None => false,
        }
    }
}

type Clock = dyn Fn() -> Timestamp + Send + Sync;

fn default_clock() -> Arc<Clock> {
    Arc::new(Timestamp::now)
}

/// Manages cooldowns for delegated OAuth refresh attempts across credential profiles.
#[derive(Clone)]
pub struct DelegatedRefreshCooldown {
    now: Arc<Clock>,
    entries: Arc<Mutex<BTreeMap<String, CooldownState>>>,
}

#[derive(Serialize, Deserialize)]
struct SerializableCooldown {
    entries: BTreeMap<String, CooldownState>,
}

impl Serialize for DelegatedRefreshCooldown {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let entries = lock(&self.entries).clone();
        SerializableCooldown { entries }.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DelegatedRefreshCooldown {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let data = SerializableCooldown::deserialize(deserializer)?;
        Ok(Self {
            now: default_clock(),
            entries: Arc::new(Mutex::new(data.entries)),
        })
    }
}

impl DelegatedRefreshCooldown {
    /// Default cooldown period after observed success or unreadable outcome (5 minutes).
    pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(5 * 60);

    /// Short cooldown period reserved before a touch and used after failed/unchanged attempts (20 seconds).
    pub const SHORT_COOLDOWN: Duration = Duration::from_secs(20);

    /// Creates a new `DelegatedRefreshCooldown` with an injected clock.
    pub fn new(now: impl Fn() -> Timestamp + Send + Sync + 'static) -> Self {
        Self {
            now: Arc::new(now),
            entries: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Replaces the clock on this instance (useful after deserialization in tests).
    pub fn with_clock(self, now: impl Fn() -> Timestamp + Send + Sync + 'static) -> Self {
        Self {
            now: Arc::new(now),
            entries: self.entries,
        }
    }

    /// Checks whether an attempt for `profile_digest` is currently in cooldown.
    ///
    /// User-initiated interactions always bypass cooldown and return `Ok(())`.
    /// Background interactions return `Err(cooldown_expiry)` if still active.
    pub fn check(
        &self,
        profile_digest: &str,
        interaction: FetchInteraction,
    ) -> Result<(), Timestamp> {
        if interaction == FetchInteraction::UserInitiated {
            return Ok(());
        }

        let now = (self.now)();
        let entries = lock(&self.entries);
        if let Some(state) = entries.get(profile_digest)
            && state.is_active(now)
        {
            return Err(state.expires_at().unwrap_or(now));
        }
        Ok(())
    }

    /// Reserves a delegated refresh attempt before executing the touch.
    ///
    /// If in active cooldown and interaction is background, returns `Err(cooldown_expiry)`.
    /// Otherwise, reserves the attempt with `SHORT_COOLDOWN` (20 s) and returns `Ok(())`.
    pub fn reserve(
        &self,
        profile_digest: &str,
        interaction: FetchInteraction,
    ) -> Result<(), Timestamp> {
        let now = (self.now)();
        let mut entries = lock(&self.entries);

        if interaction == FetchInteraction::Background
            && let Some(state) = entries.get(profile_digest)
            && state.is_active(now)
        {
            return Err(state.expires_at().unwrap_or(now));
        }

        entries.insert(
            profile_digest.to_owned(),
            CooldownState::new(now, Self::SHORT_COOLDOWN),
        );
        Ok(())
    }

    /// Finalizes the cooldown following the touch outcome.
    ///
    /// - `ObservedSuccess` or `UnreadableResult` -> 5 minutes.
    /// - `FailedOrUnchanged` -> 20 seconds.
    pub fn finalize(&self, profile_digest: &str, outcome: DelegatedRefreshOutcome) {
        let now = (self.now)();
        let interval = match outcome {
            DelegatedRefreshOutcome::ObservedSuccess
            | DelegatedRefreshOutcome::UnreadableResult => Self::DEFAULT_COOLDOWN,
            DelegatedRefreshOutcome::FailedOrUnchanged => Self::SHORT_COOLDOWN,
        };

        let mut entries = lock(&self.entries);
        entries.insert(profile_digest.to_owned(), CooldownState::new(now, interval));
    }

    /// Gets the cooldown state for a profile digest, if recorded.
    pub fn get_entry(&self, profile_digest: &str) -> Option<CooldownState> {
        lock(&self.entries).get(profile_digest).copied()
    }

    /// Returns a copy of all current cooldown entries.
    pub fn all_entries(&self) -> BTreeMap<String, CooldownState> {
        lock(&self.entries).clone()
    }
}

impl Default for DelegatedRefreshCooldown {
    fn default() -> Self {
        Self::new(Timestamp::now)
    }
}

impl fmt::Debug for DelegatedRefreshCooldown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DelegatedRefreshCooldown")
            .field("entries_count", &lock(&self.entries).len())
            .finish()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
