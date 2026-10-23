// Ported from CodexBar Sources/CodexBarCore/Providers/Codex/CodexCLILaunchGate.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use ariadusage_core::pipeline::FetchInteraction;
use jiff::Timestamp;

#[derive(Debug, Clone)]
struct Entry {
    message: String,
    expires_at: Timestamp,
}

#[derive(Debug, Default)]
pub struct CodexCLILaunchGate {
    entries: Mutex<HashMap<String, Entry>>,
}

impl CodexCLILaunchGate {
    pub const COOLDOWN: Duration = Duration::from_secs(30 * 60);

    pub fn new() -> Self {
        Self::default()
    }

    pub fn background_skip_message(
        &self,
        binary: &str,
        now: Timestamp,
        interaction: FetchInteraction,
    ) -> Option<String> {
        if interaction != FetchInteraction::Background {
            return None;
        }
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.get(binary)?;
        if entry.expires_at > now {
            Some(entry.message.clone())
        } else {
            entries.remove(binary);
            None
        }
    }

    pub fn record_launch_failure(
        &self,
        binary: &str,
        message: &str,
        now: Timestamp,
    ) -> Option<String> {
        if !Self::should_throttle_launch_failure(message) {
            return None;
        }
        let throttled = Self::throttled_message(binary, message);
        let expires_at = now.checked_add(Self::COOLDOWN).unwrap_or(now);
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.insert(
            binary.to_owned(),
            Entry {
                message: throttled.clone(),
                expires_at,
            },
        );
        Some(throttled)
    }

    pub fn reset_for_testing(&self) {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    pub fn should_throttle_launch_failure(message: &str) -> bool {
        let lower = message.to_lowercase();
        !(lower.contains("openpty")
            || lower.contains("write to pty")
            || lower.contains("app shutdown"))
    }

    fn throttled_message(binary: &str, original_message: &str) -> String {
        format!(
            "Codex CLI launch failed; background refresh is paused for 30 minutes. Reinstall or unblock `{binary}` in macOS security settings, then refresh manually. Last error: {original_message}"
        )
    }
}
