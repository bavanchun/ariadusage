// Ported from CodexBar Sources/CodexBarCore/CookieHeaderCache.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/CookieHeaderCache+Fingerprint.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::cookie as cookie_core;
use ariadusage_core::digest::sha256_hex;
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::secret::SecretString;
use cookie_core::normalize;
use jiff::Timestamp;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// Policy to enforce when authentication fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CookieAuthenticationFailurePolicy {
    StopFallback,
}

/// One cached cookie header entry.
#[derive(Clone)]
pub struct CookieCacheEntry {
    pub header: SecretString,
    pub stored_at: Timestamp,
    pub source_label: String,
    pub auth_failure_policy: Option<CookieAuthenticationFailurePolicy>,
}

impl CookieCacheEntry {
    pub fn new(
        header: SecretString,
        stored_at: Timestamp,
        source_label: impl Into<String>,
        auth_failure_policy: Option<CookieAuthenticationFailurePolicy>,
    ) -> Self {
        Self {
            header,
            stored_at,
            source_label: source_label.into(),
            auth_failure_policy,
        }
    }
}

impl PartialEq for CookieCacheEntry {
    fn eq(&self, other: &Self) -> bool {
        self.header.expose_secret() == other.header.expose_secret()
            && self.stored_at == other.stored_at
            && self.source_label == other.source_label
            && self.auth_failure_policy == other.auth_failure_policy
    }
}

impl Eq for CookieCacheEntry {}

impl std::fmt::Debug for CookieCacheEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookieCacheEntry")
            .field("header", &"[redacted]")
            .field("stored_at", &self.stored_at)
            .field("source_label", &self.source_label)
            .field("auth_failure_policy", &self.auth_failure_policy)
            .finish()
    }
}

/// Scope isolating cookie cache entries beyond provider identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CacheScope {
    ProfileHome(String),
    ProviderVariant(String),
}

impl CacheScope {
    pub fn profile_home(path: &str) -> Self {
        let standardized = path.trim_end_matches('/');
        let digest = sha256_hex("profile_home", &[standardized.as_bytes()]);
        Self::ProfileHome(digest)
    }

    pub fn provider_variant(variant: &str) -> Self {
        let digest = sha256_hex("provider_variant", &[variant.as_bytes()]);
        Self::ProviderVariant(digest)
    }

    pub fn isolation_identifier(&self) -> String {
        match self {
            Self::ProfileHome(digest) => format!("profile-home.{digest}"),
            Self::ProviderVariant(digest) => format!("provider-variant.{digest}"),
        }
    }
}

pub type CacheKey = (ProviderId, Option<CacheScope>);

/// Stable, non-reversible SHA-256 fingerprint for a normalized cookie header.
///
/// CodexBar: CookieHeaderCache+Fingerprint.swift:9
pub fn credential_fingerprint(cookie_header: &str) -> String {
    let normalized = normalize(cookie_header).unwrap_or_else(|| cookie_header.to_string());
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{b:02x}"));
    }
    out
}

#[derive(Default)]
struct GateState {
    generation: u64,
    active_tokens: HashSet<u64>,
}

#[derive(Default)]
struct CoordinatorInner {
    next_token_id: u64,
    gates: HashMap<CacheKey, GateState>,
}

/// Coordinates generation-based mutation gates across threads/tasks.
#[derive(Clone, Default)]
pub struct ConditionalMutationCoordinator {
    inner: Arc<Mutex<CoordinatorInner>>,
}

impl ConditionalMutationCoordinator {
    pub fn new() -> Self {
        Self::default()
    }
}

/// RAII gate token active during an interactive credential mutation.
pub struct MutationGate {
    coordinator: ConditionalMutationCoordinator,
    key: CacheKey,
    token_id: u64,
    active: bool,
}

impl MutationGate {
    pub fn end(&mut self) {
        if self.active {
            self.active = false;
            let mut guard = self.coordinator.inner.lock().expect("lock not poisoned");
            if let Some(state) = guard.gates.get_mut(&self.key)
                && state.active_tokens.remove(&self.token_id)
            {
                state.generation = state.generation.wrapping_add(1);
            }
        }
    }
}

impl Drop for MutationGate {
    fn drop(&mut self) {
        self.end();
    }
}

/// An observation captured before asynchronous work to verify before committing.
#[derive(Clone)]
pub struct ConditionalMutationObservation {
    pub entry: Option<CookieCacheEntry>,
    gate_generation: u64,
    coordinator: ConditionalMutationCoordinator,
}

impl ConditionalMutationObservation {
    pub fn after_owned_clear(&self) -> Self {
        Self {
            entry: None,
            gate_generation: self.gate_generation,
            coordinator: self.coordinator.clone(),
        }
    }
}

#[derive(Default)]
struct CacheState {
    entries: HashMap<CacheKey, CookieCacheEntry>,
}

/// In-memory cookie cache with scopes, conditional mutation, and generation gates.
#[derive(Clone, Default)]
pub struct CookieCache {
    state: Arc<Mutex<CacheState>>,
    default_coordinator: ConditionalMutationCoordinator,
}

impl CookieCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn default_coordinator(&self) -> &ConditionalMutationCoordinator {
        &self.default_coordinator
    }

    /// Stores a normalized cookie header. If the header normalizes to empty, the entry is cleared.
    /// Pinned entries (`StopFallback`) cannot be overwritten by non-pinned entries.
    pub fn store(
        &self,
        provider: &ProviderId,
        scope: Option<&CacheScope>,
        cookie_header: &str,
        source_label: &str,
        now: Timestamp,
    ) -> bool {
        self.store_with_policy(provider, scope, cookie_header, source_label, None, now)
    }

    pub fn store_with_policy(
        &self,
        provider: &ProviderId,
        scope: Option<&CacheScope>,
        cookie_header: &str,
        source_label: &str,
        auth_failure_policy: Option<CookieAuthenticationFailurePolicy>,
        now: Timestamp,
    ) -> bool {
        let Some(normalized) = normalize(cookie_header) else {
            self.clear(provider, scope);
            return false;
        };
        if normalized.is_empty() {
            self.clear(provider, scope);
            return false;
        }

        let key = (provider.clone(), scope.cloned());
        let mut state = self.state.lock().expect("lock not poisoned");
        if state.entries.get(&key).is_some_and(|existing| {
            existing.auth_failure_policy == Some(CookieAuthenticationFailurePolicy::StopFallback)
                && auth_failure_policy != Some(CookieAuthenticationFailurePolicy::StopFallback)
        }) {
            return false;
        }

        state.entries.insert(
            key,
            CookieCacheEntry {
                header: SecretString::from(normalized),
                stored_at: now,
                source_label: source_label.to_string(),
                auth_failure_policy,
            },
        );
        true
    }

    /// Loads the cached entry for `(provider, scope)`.
    pub fn load(
        &self,
        provider: &ProviderId,
        scope: Option<&CacheScope>,
    ) -> Option<CookieCacheEntry> {
        let key = (provider.clone(), scope.cloned());
        let state = self.state.lock().expect("lock not poisoned");
        state.entries.get(&key).cloned()
    }

    /// Checks if a pinned entry exists for `(provider, scope)`.
    pub fn has_pinned_entry(&self, provider: &ProviderId, scope: Option<&CacheScope>) -> bool {
        let key = (provider.clone(), scope.cloned());
        let state = self.state.lock().expect("lock not poisoned");
        state.entries.get(&key).is_some_and(|e| {
            e.auth_failure_policy == Some(CookieAuthenticationFailurePolicy::StopFallback)
        })
    }

    /// Clears the entry for `(provider, scope)`. Returns true if an entry was present.
    pub fn clear(&self, provider: &ProviderId, scope: Option<&CacheScope>) -> bool {
        let key = (provider.clone(), scope.cloned());
        let mut state = self.state.lock().expect("lock not poisoned");
        state.entries.remove(&key).is_some()
    }

    /// Clears all entries across all scopes for a specific provider.
    pub fn clear_all_scopes(&self, provider: &ProviderId) -> usize {
        let mut state = self.state.lock().expect("lock not poisoned");
        let initial = state.entries.len();
        state.entries.retain(|(p, _), _| p != provider);
        initial - state.entries.len()
    }

    /// Clears all entries for all providers.
    pub fn clear_all(&self) -> usize {
        let mut state = self.state.lock().expect("lock not poisoned");
        let count = state.entries.len();
        state.entries.clear();
        count
    }

    /// Stores a replacement only if current entry matches expected entry.
    pub fn store_if_current(
        &self,
        provider: &ProviderId,
        scope: Option<&CacheScope>,
        expected: Option<&CookieCacheEntry>,
        cookie_header: &str,
        source_label: &str,
        now: Timestamp,
    ) -> bool {
        let Some(normalized) = normalize(cookie_header) else {
            return false;
        };
        if normalized.is_empty() {
            return false;
        }

        let key = (provider.clone(), scope.cloned());
        let mut state = self.state.lock().expect("lock not poisoned");
        if state.entries.get(&key) != expected {
            return false;
        }

        if state.entries.get(&key).is_some_and(|existing| {
            existing.auth_failure_policy == Some(CookieAuthenticationFailurePolicy::StopFallback)
        }) {
            return false;
        }

        state.entries.insert(
            key,
            CookieCacheEntry {
                header: SecretString::from(normalized),
                stored_at: now,
                source_label: source_label.to_string(),
                auth_failure_policy: None,
            },
        );
        true
    }

    /// Clears entry only if current entry matches expected entry.
    pub fn clear_if_current(
        &self,
        provider: &ProviderId,
        scope: Option<&CacheScope>,
        expected: Option<&CookieCacheEntry>,
    ) -> bool {
        let key = (provider.clone(), scope.cloned());
        let mut state = self.state.lock().expect("lock not poisoned");
        if state.entries.get(&key) != expected {
            return false;
        }
        state.entries.remove(&key);
        true
    }

    /// Captures cache state for conditional mutation using the default coordinator.
    pub fn observe_for_conditional_mutation(
        &self,
        provider: &ProviderId,
        scope: Option<&CacheScope>,
    ) -> ConditionalMutationObservation {
        self.observe_for_conditional_mutation_with_coordinator(
            provider,
            scope,
            &self.default_coordinator,
        )
    }

    /// Captures cache state for conditional mutation using a specific coordinator.
    pub fn observe_for_conditional_mutation_with_coordinator(
        &self,
        provider: &ProviderId,
        scope: Option<&CacheScope>,
        coordinator: &ConditionalMutationCoordinator,
    ) -> ConditionalMutationObservation {
        let key = (provider.clone(), scope.cloned());
        let guard = coordinator.inner.lock().expect("lock not poisoned");
        let gate_generation = guard.gates.get(&key).map_or(0, |g| g.generation);
        drop(guard);

        let entry = self.load(provider, scope);
        ConditionalMutationObservation {
            entry,
            gate_generation,
            coordinator: coordinator.clone(),
        }
    }

    /// Begins an interactive mutation gate using the default coordinator.
    pub fn begin_conditional_mutation_gate(
        &self,
        provider: &ProviderId,
        scope: Option<&CacheScope>,
    ) -> MutationGate {
        self.begin_conditional_mutation_gate_with_coordinator(
            provider,
            scope,
            &self.default_coordinator,
        )
    }

    /// Begins an interactive mutation gate using a specific coordinator.
    pub fn begin_conditional_mutation_gate_with_coordinator(
        &self,
        provider: &ProviderId,
        scope: Option<&CacheScope>,
        coordinator: &ConditionalMutationCoordinator,
    ) -> MutationGate {
        let key = (provider.clone(), scope.cloned());
        let mut guard = coordinator.inner.lock().expect("lock not poisoned");
        guard.next_token_id += 1;
        let token_id = guard.next_token_id;
        let state = guard.gates.entry(key.clone()).or_default();
        state.generation = state.generation.wrapping_add(1);
        state.active_tokens.insert(token_id);
        MutationGate {
            coordinator: coordinator.clone(),
            key,
            token_id,
            active: true,
        }
    }

    /// Ends an interactive mutation gate explicitly.
    pub fn end_conditional_mutation_gate(&self, mut gate: MutationGate) {
        gate.end();
    }

    /// Direct store with success return value; invalid input leaves current entry intact.
    pub fn store_result(
        &self,
        provider: &ProviderId,
        scope: Option<&CacheScope>,
        cookie_header: &str,
        source_label: &str,
        auth_failure_policy: Option<CookieAuthenticationFailurePolicy>,
        now: Timestamp,
    ) -> bool {
        let Some(normalized) = normalize(cookie_header) else {
            return false;
        };
        if normalized.is_empty() {
            return false;
        }

        let key = (provider.clone(), scope.cloned());
        let mut state = self.state.lock().expect("lock not poisoned");
        if state.entries.get(&key).is_some_and(|existing| {
            existing.auth_failure_policy == Some(CookieAuthenticationFailurePolicy::StopFallback)
                && auth_failure_policy != Some(CookieAuthenticationFailurePolicy::StopFallback)
        }) {
            return false;
        }

        state.entries.insert(
            key,
            CookieCacheEntry {
                header: SecretString::from(normalized),
                stored_at: now,
                source_label: source_label.to_string(),
                auth_failure_policy,
            },
        );
        true
    }

    /// Stores only if the observation is still current (generation matches, no active gates, entry unchanged).
    pub fn store_if_observation_current(
        &self,
        provider: &ProviderId,
        scope: Option<&CacheScope>,
        expected: &ConditionalMutationObservation,
        cookie_header: &str,
        source_label: &str,
        now: Timestamp,
    ) -> bool {
        let Some(normalized) = normalize(cookie_header) else {
            return false;
        };
        if normalized.is_empty() {
            return false;
        }

        let key = (provider.clone(), scope.cloned());
        let coord_guard = expected
            .coordinator
            .inner
            .lock()
            .expect("lock not poisoned");
        if let Some(gate_state) = coord_guard.gates.get(&key) {
            if !gate_state.active_tokens.is_empty()
                || gate_state.generation != expected.gate_generation
            {
                return false;
            }
        } else if expected.gate_generation != 0 {
            return false;
        }

        let mut state = self.state.lock().expect("lock not poisoned");
        let current = state.entries.get(&key);
        if current != expected.entry.as_ref() {
            return false;
        }

        if current.is_some_and(|existing| {
            existing.auth_failure_policy == Some(CookieAuthenticationFailurePolicy::StopFallback)
        }) {
            return false;
        }

        state.entries.insert(
            key,
            CookieCacheEntry {
                header: SecretString::from(normalized),
                stored_at: now,
                source_label: source_label.to_string(),
                auth_failure_policy: None,
            },
        );
        true
    }
}
