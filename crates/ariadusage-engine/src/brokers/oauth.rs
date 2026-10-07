// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthDelegatedRefreshCoordinator.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthCredentials.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeOAuth/ClaudeOAuthCredentialModels.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use ariadusage_core::gates::delegated_cooldown::{
    DelegatedRefreshCooldown, DelegatedRefreshOutcome,
};
use ariadusage_core::pipeline::FetchInteraction;
use jiff::Timestamp;
use tokio::sync::{Mutex, broadcast};

use super::call::BrokerCall;
use super::credential_file::{CredentialDecl, CredentialFileError, StatFingerprint, read};
use crate::state_store::BrokerStateStore;

/// Type alias for pinned boxed futures returned by async trait methods.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Errors that can occur during a delegated refresh touch attempt.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DelegatedRefreshError {
    #[error("delegated CLI tool is unavailable")]
    CliUnavailable,

    #[error("touch operation failed: {0}")]
    Failed(String),

    #[error("touch operation timed out")]
    Timeout,
}

/// Trait defining the external CLI refresh capability (implemented in M3 over PTY session).
pub trait DelegatedRefresher: Send + Sync {
    /// Returns true if the delegated CLI binary is present and executable.
    fn is_available(&self) -> bool;

    /// Executes the touch operation (e.g. `claude /status`) with the given timeout.
    fn touch(&self, timeout: Duration) -> BoxFuture<'_, Result<(), DelegatedRefreshError>>;
}

/// Result of a delegated refresh coordination attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegatedCoordinatorResult {
    /// Refresh observed a credential file modification (successful refresh).
    Success,

    /// Refresh was skipped because the profile is currently in cooldown.
    SkippedByCooldown { expires_at: Timestamp },

    /// Refresh was skipped because background execution was requested but not permitted.
    SkippedBackgroundDisabled,

    /// Refresh was skipped because the CLI tool is unavailable.
    CliUnavailable,

    /// Delegated touch ran but resulted in an unreadable credential file (5 min terminal cooldown).
    UnreadableResult,

    /// Delegated touch failed, timed out, or produced no credential file change (20 s cooldown).
    FailedOrUnchanged,
}

#[derive(Clone)]
struct InFlightAttempt {
    interaction: FetchInteraction,
    sender: broadcast::Sender<DelegatedCoordinatorResult>,
}

/// Coordinates delegated CLI OAuth refresh attempts with cooldown enforcement,
/// single-flight deduplication per profile, credential file observation, and state persistence.
pub struct DelegatedRefreshCoordinator {
    refresher: Arc<dyn DelegatedRefresher>,
    cooldown: DelegatedRefreshCooldown,
    state_store: Option<Arc<BrokerStateStore>>,
    in_flight: Arc<Mutex<BTreeMap<String, InFlightAttempt>>>,
    observation_ticks: Vec<Duration>,
}

impl DelegatedRefreshCoordinator {
    /// Creates a new `DelegatedRefreshCoordinator`.
    pub fn new(
        refresher: Arc<dyn DelegatedRefresher>,
        cooldown: DelegatedRefreshCooldown,
        state_store: Option<Arc<BrokerStateStore>>,
    ) -> Self {
        // Default observation ticks: check at 0.2s, 0.5s, 0.8s, and 2.0s
        let observation_ticks = vec![
            Duration::from_millis(200),
            Duration::from_millis(300),
            Duration::from_millis(300),
            Duration::from_millis(1200),
        ];
        Self {
            refresher,
            cooldown,
            state_store,
            in_flight: Arc::new(Mutex::new(BTreeMap::new())),
            observation_ticks,
        }
    }

    /// Sets custom observation intervals (used for fast testing).
    pub fn with_observation_ticks(mut self, ticks: Vec<Duration>) -> Self {
        self.observation_ticks = ticks;
        self
    }

    /// Returns a reference to the active cooldown manager.
    pub fn cooldown(&self) -> &DelegatedRefreshCooldown {
        &self.cooldown
    }

    /// Coordinates a delegated refresh for `profile_digest` and `decl`.
    pub async fn refresh(
        &self,
        profile_digest: &str,
        decl: &CredentialDecl,
        call: &BrokerCall,
        allow_background: bool,
    ) -> DelegatedCoordinatorResult {
        // 1. Background check: background calls require explicit opt-in.
        if call.interaction == FetchInteraction::Background && !allow_background {
            return DelegatedCoordinatorResult::SkippedBackgroundDisabled;
        }

        // 2. Availability check: if CLI is unavailable, fail early without touching.
        if !self.refresher.is_available() {
            return DelegatedCoordinatorResult::CliUnavailable;
        }

        // 3. Cooldown check: user-initiated bypasses; background respects cooldown.
        if let Err(expires_at) = self.cooldown.check(profile_digest, call.interaction) {
            return DelegatedCoordinatorResult::SkippedByCooldown { expires_at };
        }

        // 4. Single-flight management: check if an attempt is currently in-flight for this profile.
        enum JoinedState {
            Leader(broadcast::Sender<DelegatedCoordinatorResult>),
            Follower(
                FetchInteraction,
                broadcast::Receiver<DelegatedCoordinatorResult>,
            ),
        }

        let joined = {
            let mut map = self.in_flight.lock().await;
            if let Some(existing) = map.get(profile_digest) {
                JoinedState::Follower(existing.interaction, existing.sender.subscribe())
            } else {
                let (sender, _) = broadcast::channel(1);
                map.insert(
                    profile_digest.to_owned(),
                    InFlightAttempt {
                        interaction: call.interaction,
                        sender: sender.clone(),
                    },
                );
                JoinedState::Leader(sender)
            }
        };

        match joined {
            JoinedState::Follower(in_flight_interaction, mut receiver) => {
                // Await result of the in-flight attempt.
                let result = match receiver.recv().await {
                    Ok(r) => r,
                    Err(_) => DelegatedCoordinatorResult::FailedOrUnchanged,
                };

                // CodexBar parity: A user-initiated caller joining a failed background attempt retries once!
                if call.interaction == FetchInteraction::UserInitiated
                    && in_flight_interaction == FetchInteraction::Background
                    && result == DelegatedCoordinatorResult::FailedOrUnchanged
                {
                    // Retry once as its own fresh attempt.
                    return self
                        .execute_fresh_attempt(profile_digest, decl, call, allow_background)
                        .await;
                }

                result
            }
            JoinedState::Leader(sender) => {
                let result = self
                    .execute_touch_and_observe(profile_digest, decl, call)
                    .await;

                // Broadcast to followers and remove from in_flight.
                let _ = sender.send(result.clone());
                let mut map = self.in_flight.lock().await;
                map.remove(profile_digest);

                result
            }
        }
    }

    async fn execute_fresh_attempt(
        &self,
        profile_digest: &str,
        decl: &CredentialDecl,
        call: &BrokerCall,
        _allow_background: bool,
    ) -> DelegatedCoordinatorResult {
        // Re-check cooldown before retrying.
        if let Err(expires_at) = self.cooldown.check(profile_digest, call.interaction) {
            return DelegatedCoordinatorResult::SkippedByCooldown { expires_at };
        }

        let sender = {
            let mut map = self.in_flight.lock().await;
            let (sender, _) = broadcast::channel(1);
            map.insert(
                profile_digest.to_owned(),
                InFlightAttempt {
                    interaction: call.interaction,
                    sender: sender.clone(),
                },
            );
            sender
        };

        let result = self
            .execute_touch_and_observe(profile_digest, decl, call)
            .await;

        let _ = sender.send(result.clone());
        let mut map = self.in_flight.lock().await;
        map.remove(profile_digest);

        result
    }

    async fn execute_touch_and_observe(
        &self,
        profile_digest: &str,
        decl: &CredentialDecl,
        call: &BrokerCall,
    ) -> DelegatedCoordinatorResult {
        // Reserve cooldown (20 s) prior to touch.
        let _ = self.cooldown.reserve(profile_digest, call.interaction);
        self.persist_cooldown();

        // Capture initial fingerprint before touch.
        let initial_fp = read(decl, call).ok().map(|r| r.stat);

        // Execute CLI touch.
        let touch_result = self.refresher.touch(Duration::from_secs(15)).await;

        if let Err(DelegatedRefreshError::CliUnavailable) = touch_result {
            self.cooldown
                .finalize(profile_digest, DelegatedRefreshOutcome::FailedOrUnchanged);
            self.persist_cooldown();
            return DelegatedCoordinatorResult::CliUnavailable;
        }

        // Observe credential file changes across configured intervals.
        for tick in &self.observation_ticks {
            tokio::time::sleep(*tick).await;

            match read(decl, call) {
                Ok(current_read) => {
                    let changed = match &initial_fp {
                        Some(init) => &current_read.stat != init,
                        None => true,
                    };
                    if changed {
                        self.cooldown
                            .finalize(profile_digest, DelegatedRefreshOutcome::ObservedSuccess);
                        self.persist_success(profile_digest, current_read.stat);
                        return DelegatedCoordinatorResult::Success;
                    }
                }
                Err(CredentialFileError::Unreadable) => {
                    self.cooldown
                        .finalize(profile_digest, DelegatedRefreshOutcome::UnreadableResult);
                    self.persist_cooldown();
                    return DelegatedCoordinatorResult::UnreadableResult;
                }
                Err(_) => {
                    // NotFound or temporary read state, keep observing until ticks exhaust.
                }
            }
        }

        // If observation exhausted without detected change or unreadable outcome:
        self.cooldown
            .finalize(profile_digest, DelegatedRefreshOutcome::FailedOrUnchanged);
        self.persist_cooldown();
        DelegatedCoordinatorResult::FailedOrUnchanged
    }

    fn persist_cooldown(&self) {
        if let Some(store) = &self.state_store {
            let entries = self.cooldown.all_entries();
            let _ = store.update(|state| {
                state.delegated_cooldowns = entries;
            });
        }
    }

    fn persist_success(&self, profile_digest: &str, new_fp: StatFingerprint) {
        if let Some(store) = &self.state_store {
            let entries = self.cooldown.all_entries();
            let _ = store.update(|state| {
                state.delegated_cooldowns = entries;
                state.last_seen.insert(profile_digest.to_owned(), new_fp);
            });
        }
    }
}

impl fmt::Debug for DelegatedRefreshCoordinator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DelegatedRefreshCoordinator")
            .field("cooldown", &self.cooldown)
            .finish()
    }
}
