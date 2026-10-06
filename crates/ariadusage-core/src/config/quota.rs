// Ported from CodexBar Sources/CodexBarCore/Config/CodexBarConfig.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Target window for quota warning thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuotaWarningWindow {
    Session,
    Weekly,
}

impl QuotaWarningWindow {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Weekly => "weekly",
        }
    }
}

/// Threshold configuration for a specific quota warning window.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct QuotaWarningWindowConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<Vec<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

impl QuotaWarningWindowConfig {
    pub fn new(thresholds: Option<Vec<u32>>, enabled: Option<bool>) -> Self {
        Self {
            thresholds: thresholds.map(|t| QuotaWarnings::sanitize_thresholds(&t)),
            enabled,
        }
    }

    pub fn has_override(&self) -> bool {
        self.thresholds.is_some() || self.enabled.is_some()
    }

    pub fn is_enabled(&self, global: bool) -> bool {
        self.enabled.unwrap_or(if self.thresholds.is_some() {
            true
        } else {
            global
        })
    }
}

/// Provider-level quota warning configuration across session and weekly windows.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct QuotaWarnings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<QuotaWarningWindowConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly: Option<QuotaWarningWindowConfig>,
}

impl QuotaWarnings {
    pub const DEFAULTS: [u32; 2] = [50, 20];
    pub const MAX_THRESHOLD: u32 = 99;

    pub fn new(
        session: Option<QuotaWarningWindowConfig>,
        weekly: Option<QuotaWarningWindowConfig>,
    ) -> Self {
        Self { session, weekly }
    }

    pub fn thresholds(&self, window: QuotaWarningWindow, global: &[u32]) -> Vec<u32> {
        let raw = match window {
            QuotaWarningWindow::Session => self
                .session
                .as_ref()
                .and_then(|s| s.thresholds.as_deref())
                .unwrap_or(global),
            QuotaWarningWindow::Weekly => self
                .weekly
                .as_ref()
                .and_then(|w| w.thresholds.as_deref())
                .unwrap_or(global),
        };
        Self::sanitize_thresholds(raw)
    }

    pub fn is_enabled(&self, window: QuotaWarningWindow, global: bool) -> bool {
        match window {
            QuotaWarningWindow::Session => self
                .session
                .as_ref()
                .map_or(global, |s| s.is_enabled(global)),
            QuotaWarningWindow::Weekly => self
                .weekly
                .as_ref()
                .map_or(global, |w| w.is_enabled(global)),
        }
    }

    pub fn has_override(&self, window: QuotaWarningWindow) -> bool {
        match window {
            QuotaWarningWindow::Session => self.session.as_ref().is_some_and(|s| s.has_override()),
            QuotaWarningWindow::Weekly => self.weekly.as_ref().is_some_and(|w| w.has_override()),
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.has_override(QuotaWarningWindow::Session)
            && !self.has_override(QuotaWarningWindow::Weekly)
    }

    pub fn sanitize_thresholds(raw: &[u32]) -> Vec<u32> {
        if raw.is_empty() {
            return Self::DEFAULTS.to_vec();
        }

        let unique: BTreeSet<u32> = raw.iter().map(|&t| t.min(Self::MAX_THRESHOLD)).collect();
        let mut sorted: Vec<u32> = unique.into_iter().collect();
        sorted.reverse();
        if sorted.is_empty() {
            Self::DEFAULTS.to_vec()
        } else {
            sorted
        }
    }
}
