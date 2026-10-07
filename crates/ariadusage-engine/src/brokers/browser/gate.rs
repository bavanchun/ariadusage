use std::sync::Arc;

use ariadusage_core::digest::sha256_hex;
use ariadusage_core::pipeline::FetchInteraction;

use crate::brokers::call::BrokerCall;
use crate::state_store::BrokerStateStore;

use super::Browser;
use super::error::BrowserError;

const COOLDOWN_SECONDS: i64 = 6 * 60 * 60;

/// Allows one explicit retry within one browser-import call during a saved cooldown.
#[derive(Default)]
pub struct RetryScope {
    chromium_retry_used: bool,
    chromium_dismissed_in_scope: bool,
}

impl RetryScope {
    pub fn new() -> Self {
        Self::default()
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
        call: &BrokerCall,
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
        if call.interaction == FetchInteraction::UserInitiated && !retry_scope.chromium_retry_used {
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

    pub fn record_success(&self, browser: Browser) -> Result<(), BrowserError> {
        self.state
            .update(|state| {
                state.browser_cooldowns.remove(&browser_key(browser));
                if browser.is_chromium() {
                    state.browser_cooldowns.remove(&family_key());
                }
            })
            .map_err(|_| BrowserError::Io)
    }
}

fn browser_key(browser: Browser) -> String {
    sha256_hex("browser_cooldown", &[browser.display_name().as_bytes()])
}

fn family_key() -> String {
    sha256_hex("browser_cooldown", &[b"chromium-family"])
}
