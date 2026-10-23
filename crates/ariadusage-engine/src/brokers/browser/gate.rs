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
/// A saved denial cooldown blocks every read, user-initiated ones included. Only an
/// acknowledged, user-initiated retry may make one keyring read through the cooldown.
pub struct RetryScope {
    bypasses_cooldown: bool,
    chromium_retry_used: bool,
    chromium_dismissed_in_scope: bool,
}

impl RetryScope {
    pub fn new(call: &BrokerCall, acknowledged_retry: bool) -> Self {
        Self {
            bypasses_cooldown: acknowledged_retry
                && call.interaction == FetchInteraction::UserInitiated,
            chromium_retry_used: false,
            chromium_dismissed_in_scope: false,
        }
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
