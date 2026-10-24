// Ported from CodexBar Sources/CodexBarCore/BrowserCookieAccessGate.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use std::collections::BTreeMap;
use std::sync::Arc;

use ariadusage_core::digest::sha256_hex;
use ariadusage_core::pipeline::FetchInteraction;

use crate::brokers::call::BrokerCall;
use crate::state_store::BrokerStateStore;

use super::Browser;
use super::error::BrowserError;

const COOLDOWN_SECONDS: i64 = 6 * 60 * 60;

/// Retry state shared by every read of one browser-import call.
///
/// A saved denial cooldown blocks user-initiated reads unless the caller acknowledges
/// the retry, which permits one keyring read through the cooldown. Background reads
/// skip the cooldown because they already run noninteractively (`allow_unlock=false`).
/// On macOS, at most one interactive Keychain read is permitted per user action (`RetryScope`),
/// matching upstream `BrowserCookieAccessGate.swift`; subsequent browsers in the same scope read
/// with `FetchInteraction::Background` (`allow_interaction=false`).
pub struct RetryScope {
    interaction: FetchInteraction,
    bypasses_cooldown: bool,
    chromium_retry_used: bool,
    chromium_dismissed_in_scope: bool,
    #[cfg(target_os = "macos")]
    interactive_key_read_used: bool,
}

impl RetryScope {
    pub fn new(call: &BrokerCall, acknowledged_retry: bool) -> Self {
        Self {
            interaction: call.interaction,
            bypasses_cooldown: acknowledged_retry
                && call.interaction == FetchInteraction::UserInitiated,
            chromium_retry_used: false,
            chromium_dismissed_in_scope: false,
            #[cfg(target_os = "macos")]
            interactive_key_read_used: false,
        }
    }

    /// Returns a non-interactive (`FetchInteraction::Background`) [`BrokerCall`] preserving
    /// `call`'s cancellation token and request id.
    #[cfg(target_os = "macos")]
    pub fn background_key_read_call(call: &BrokerCall) -> BrokerCall {
        BrokerCall {
            interaction: FetchInteraction::Background,
            cancel: call.cancel.clone(),
            request_id: call.request_id.clone(),
        }
    }

    /// Claims the single per-action interactive Keychain allowance on macOS after a no-UI
    /// safe-storage read returned [`SafeStorageError::Locked`].
    ///
    /// Matches upstream `BrowserCookieAccessGate.swift:229-250`: an uncached browser key read on
    /// macOS tries a no-UI read first so an already-authorized Keychain item does not spend the
    /// interactive allowance; only when the no-UI read fails with `Locked` during a
    /// user-initiated call does `claim_interactive_key_read` permit one interactive retry.
    #[cfg(target_os = "macos")]
    pub fn claim_interactive_key_read(&mut self, call: &BrokerCall) -> Option<BrokerCall> {
        if call.interaction == FetchInteraction::UserInitiated && !self.interactive_key_read_used {
            self.interactive_key_read_used = true;
            return Some(call.clone());
        }
        None
    }
}

/// Persistent access policy for Chromium-family keyring reads.
pub struct BrowserGate {
    state: Arc<BrokerStateStore>,
}

impl BrowserGate {
    pub fn new(state: Arc<BrokerStateStore>) -> Self {
        Self { state }
    }

    pub fn should_attempt_key(
        &self,
        browser: Browser,
        retry_scope: &mut RetryScope,
        now_seconds: i64,
    ) -> Result<bool, BrowserError> {
        if !browser.is_chromium() {
            return Ok(false);
        }
        if retry_scope.chromium_dismissed_in_scope {
            return Ok(false);
        }
        if retry_scope.interaction == FetchInteraction::Background {
            return Ok(true);
        }
        let state = self.state.load().map_err(|_| BrowserError::Io)?;
        let browser_until = state
            .browser_cooldowns
            .get(&browser_key(browser))
            .copied()
            .unwrap_or_default();
        let family_until = state
            .browser_cooldowns
            .get(&family_key())
            .copied()
            .unwrap_or_default();
        if browser_until <= now_seconds && family_until <= now_seconds {
            return Ok(true);
        }
        if retry_scope.bypasses_cooldown && !retry_scope.chromium_retry_used {
            retry_scope.chromium_retry_used = true;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn record_dismissal(
        &self,
        browser: Browser,
        retry_scope: &mut RetryScope,
        now_seconds: i64,
    ) -> Result<(), BrowserError> {
        let until = now_seconds.saturating_add(COOLDOWN_SECONDS);
        retry_scope.chromium_dismissed_in_scope = true;
        self.state
            .update(|state| {
                state.browser_cooldowns.insert(browser_key(browser), until);
                state.browser_cooldowns.insert(family_key(), until);
            })
            .map_err(|_| BrowserError::Io)
    }

    /// Clears a browser's cooldown after a successful key read, but only inside an
    /// acknowledged retry. The family cooldown then falls back to the latest remaining
    /// browser cooldown, so another browser's denial keeps suppressing the family.
    pub fn record_success(
        &self,
        browser: Browser,
        retry_scope: &RetryScope,
    ) -> Result<(), BrowserError> {
        if !browser.is_chromium() || !retry_scope.bypasses_cooldown {
            return Ok(());
        }
        self.state
            .update(|state| {
                let cooldowns = &mut state.browser_cooldowns;
                let browser = browser_key(browser);
                let family = family_key();
                if !cooldowns.contains_key(&browser) || !cooldowns.contains_key(&family) {
                    return;
                }
                cooldowns.remove(&browser);
                match latest_browser_cooldown(cooldowns, &family) {
                    Some(until) => cooldowns.insert(family, until),
                    None => cooldowns.remove(&family),
                };
            })
            .map_err(|_| BrowserError::Io)
    }
}

fn latest_browser_cooldown(cooldowns: &BTreeMap<String, i64>, family: &str) -> Option<i64> {
    cooldowns
        .iter()
        .filter(|(key, _)| key.as_str() != family)
        .map(|(_, until)| *until)
        .max()
}

fn browser_key(browser: Browser) -> String {
    sha256_hex("browser_cooldown", &[browser.display_name().as_bytes()])
}

fn family_key() -> String {
    sha256_hex("browser_cooldown", &[b"chromium-family"])
}
