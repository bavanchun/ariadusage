// Ported from CodexBar Sources/CodexBar/PreferencesView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesSidebar.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesSelection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesComponents.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesGeneralPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesICloudSyncPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesSpendDashboardPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesNotificationsPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesMenuBarPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesMenuPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesAdvancedPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesHTTPServerSection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesHooksPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesPluginsPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesAboutPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesProvidersPane.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesProviderDetailView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesProviderSettingsRows.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesProviderErrorView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesCodexAccountsSection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesProviderDirectoryList.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/PreferencesTransferSection.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuBarLayoutEditor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/SettingsWindowController.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/DockIconController.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! Pure state, navigation, sidebar filtering, Dock activation policy decisions,
//! and pane models for the Settings window.

use std::collections::{HashMap, HashSet};

use ariadusage_protocol::ids::SettingId;
use ariadusage_protocol::metric::MetricState;
use ariadusage_protocol::settings::{DescriptorKind, SettingsPage};
use ariadusage_protocol::snapshot::{EngineSnapshot, ProviderSnapshot};
use ariadusage_protocol::usage::StatusIndicator;

pub use crate::card::Rgb;

/// Window identifier (`SettingsWindowIdentity.identifier`, adapted to AriadUsage's bundle ID).
pub const WINDOW_IDENTIFIER: &str = "io.github.bavanchun.ariadusage.settings";
/// Frame autosave name (`SettingsWindowIdentity.frameAutosaveName`).
pub const WINDOW_AUTOSAVE_NAME: &str = "AriadUsage.SettingsWindow";
/// Default window width (`SettingsPane.windowWidth`).
pub const WINDOW_WIDTH: f64 = 880.0;
/// Default window height (`SettingsPane.windowHeight`).
pub const WINDOW_HEIGHT: f64 = 620.0;
/// Minimum window content width (`SettingsPane.windowMinWidth`).
pub const WINDOW_MIN_WIDTH: f64 = 800.0;
/// Minimum window content height (`SettingsPane.windowMinHeight`).
pub const WINDOW_MIN_HEIGHT: f64 = 540.0;
/// Default sidebar width (`SettingsPane.sidebarWidth`).
pub const SIDEBAR_DEFAULT_WIDTH: f64 = 260.0;
/// Minimum sidebar width (`SettingsPane.sidebarMinWidth`).
pub const SIDEBAR_MIN_WIDTH: f64 = 200.0;
/// Maximum sidebar width (`SettingsPane.sidebarMaxWidth`).
pub const SIDEBAR_MAX_WIDTH: f64 = 380.0;
/// UserDefaults key for persisted sidebar width (`SettingsPane.sidebarWidthDefaultsKey`).
pub const SIDEBAR_WIDTH_DEFAULTS_KEY: &str = "settingsSidebarWidth";
/// UserDefaults key for persisted selected pane (`PreferencesSelection.paneDefaultsKey`).
pub const SELECTED_PANE_DEFAULTS_KEY: &str = "settingsSelectedPane";
/// Maximum width of the detail column (`SettingsPane.detailMaxWidth`).
pub const DETAIL_MAX_WIDTH: f64 = 780.0;
/// Maximum providers selectable in Overview (`SettingsStore.mergedOverviewProviderLimit`).
pub const MERGED_OVERVIEW_PROVIDER_LIMIT: usize = 6;
/// Preferred width of the keyboard shortcut recorder (`OpenMenuShortcutRecorder.preferredWidth`).
pub const SHORTCUT_RECORDER_WIDTH: f64 = 170.0;
/// Shared application version string used by the status menu and Settings panes.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Copyright and license footer derived from the repository `LICENSE` (`MIT`, copyright `VChun`).
pub const ABOUT_LICENSE_FOOTER: &str = "© 2026 VChun. MIT License.";

/// Whether `"Improve visibility on inactive displays"` is available for `style`
/// (`MenuBarPane.inactiveDisplayContrastAvailable(for:)`).
pub fn inactive_display_contrast_available(style: &str) -> bool {
    style == "iconAndPercent"
}

/// Whether pace color/sign and the layout editor are available for `style`
/// (`PreferencesMenuBarPane.swift:57, 66, 77`).
pub fn icon_and_percent_dependents_available(style: &str) -> bool {
    style == "iconAndPercent"
}

/// Whether `"Surprise me"` animation is available for `style`
/// (`PreferencesMenuBarPane.swift:149`).
pub fn surprise_me_available(style: &str) -> bool {
    style != "iconAndPercent"
}

/// Whether combined-icon dependent rows are available (`PreferencesMenuBarPane.swift:126-139`).
pub fn combined_icon_dependents_available(merge_icons: bool) -> bool {
    merge_icons
}

/// Whether `"Workday tick appearance"` is available (`PreferencesMenuPane.swift:53`).
pub fn workday_tick_appearance_available(work_days: Option<u32>) -> bool {
    work_days.is_some()
}

/// Whether `"Agent sessions"` dependent rows are available (`PreferencesMenuPane.swift:153-191`).
pub fn agent_sessions_dependents_available(agent_sessions_enabled: bool) -> bool {
    agent_sessions_enabled
}

/// Clamps a sidebar width to `[SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH]`.
pub fn clamp_sidebar_width(width: f64) -> f64 {
    width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH)
}

/// Sidebar destinations of the settings window (`SettingsPane`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SettingsPane {
    General,
    ICloudSync,
    UsageSpend,
    Notifications,
    MenuBar,
    Menu,
    Advanced,
    Hooks,
    Plugins,
    About,
    Debug,
    Provider(String),
}

/// Icon chip style for an app pane row in the sidebar (`SettingsIconChip`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarChipStyle {
    Symbol { symbol: &'static str, color: Rgb },
    AppIcon,
}

impl SettingsPane {
    /// Human-readable pane title (`SettingsPane.title`).
    pub fn title(&self) -> String {
        match self {
            Self::General => "General".to_string(),
            Self::ICloudSync => "iCloud Sync".to_string(),
            Self::UsageSpend => "Usage & Spend".to_string(),
            Self::Notifications => "Notifications".to_string(),
            Self::MenuBar => "Menu Bar".to_string(),
            Self::Menu => "Menu".to_string(),
            Self::Advanced => "Advanced".to_string(),
            Self::Hooks => "Hooks".to_string(),
            Self::Plugins => "Plugins".to_string(),
            Self::About => "About".to_string(),
            Self::Debug => "Debug".to_string(),
            Self::Provider(id) => provider_display_name(id),
        }
    }

    /// Stable persistence token (`SettingsPane.persistenceToken`).
    pub fn persistence_token(&self) -> String {
        match self {
            Self::General => "general".to_string(),
            Self::ICloudSync => "iCloudSync".to_string(),
            Self::UsageSpend => "usageSpend".to_string(),
            Self::Notifications => "notifications".to_string(),
            Self::MenuBar => "menuBar".to_string(),
            Self::Menu => "menu".to_string(),
            Self::Advanced => "advanced".to_string(),
            Self::Hooks => "hooks".to_string(),
            Self::Plugins => "plugins".to_string(),
            Self::About => "about".to_string(),
            Self::Debug => "debug".to_string(),
            Self::Provider(id) => format!("provider:{id}"),
        }
    }

    /// Restores a pane from its persistence token (`SettingsPane.init?(persistenceToken:)`).
    pub fn from_persistence_token(token: &str) -> Option<Self> {
        match token {
            "general" => Some(Self::General),
            "iCloudSync" => Some(Self::ICloudSync),
            "usageSpend" => Some(Self::UsageSpend),
            "notifications" => Some(Self::Notifications),
            "menuBar" | "display" => Some(Self::MenuBar),
            "menu" => Some(Self::Menu),
            "advanced" => Some(Self::Advanced),
            "hooks" => Some(Self::Hooks),
            "plugins" => Some(Self::Plugins),
            "about" => Some(Self::About),
            "debug" => Some(Self::Debug),
            other => {
                let id = other.strip_prefix("provider:")?.trim();
                (!id.is_empty()).then(|| Self::Provider(id.to_string()))
            }
        }
    }

    /// Whether the detail header bar displays the pane title next to the Back/Forward buttons
    /// (`SettingsDetailHeader.showsTitle`).
    pub fn shows_detail_header_title(&self) -> bool {
        !matches!(self, Self::UsageSpend | Self::Provider(_))
    }

    /// Whether this pane's body is staged as empty in this slice.
    pub fn is_staged_empty_pane(&self) -> bool {
        matches!(self, Self::Debug)
    }

    /// Sidebar chip style for app panes (`SettingsSidebarView.appPanesSection`).
    pub fn chip_style(&self) -> Option<SidebarChipStyle> {
        match self {
            Self::General => Some(SidebarChipStyle::Symbol {
                symbol: "gearshape.fill",
                color: Rgb {
                    r: 142,
                    g: 142,
                    b: 147,
                },
            }),
            Self::ICloudSync => Some(SidebarChipStyle::Symbol {
                symbol: "icloud.fill",
                color: Rgb {
                    r: 0,
                    g: 122,
                    b: 255,
                },
            }),
            Self::UsageSpend => Some(SidebarChipStyle::Symbol {
                symbol: "chart.bar.fill",
                color: Rgb {
                    r: 52,
                    g: 199,
                    b: 89,
                },
            }),
            Self::Notifications => Some(SidebarChipStyle::Symbol {
                symbol: "bell.badge.fill",
                color: Rgb {
                    r: 255,
                    g: 59,
                    b: 48,
                },
            }),
            Self::MenuBar => Some(SidebarChipStyle::Symbol {
                symbol: "menubar.rectangle",
                color: Rgb {
                    r: 0,
                    g: 122,
                    b: 255,
                },
            }),
            Self::Menu => Some(SidebarChipStyle::Symbol {
                symbol: "filemenu.and.selection",
                color: Rgb {
                    r: 48,
                    g: 176,
                    b: 199,
                },
            }),
            Self::Advanced => Some(SidebarChipStyle::Symbol {
                symbol: "slider.horizontal.3",
                color: Rgb {
                    r: 175,
                    g: 82,
                    b: 222,
                },
            }),
            Self::Hooks => Some(SidebarChipStyle::Symbol {
                symbol: "bolt.horizontal.circle.fill",
                color: Rgb {
                    r: 255,
                    g: 149,
                    b: 0,
                },
            }),
            Self::Plugins => Some(SidebarChipStyle::Symbol {
                symbol: "puzzlepiece.extension.fill",
                color: Rgb {
                    r: 88,
                    g: 86,
                    b: 214,
                },
            }),
            Self::About => Some(SidebarChipStyle::AppIcon),
            Self::Debug => Some(SidebarChipStyle::Symbol {
                symbol: "ladybug.fill",
                color: Rgb {
                    r: 255,
                    g: 59,
                    b: 48,
                },
            }),
            Self::Provider(_) => None,
        }
    }

    /// Standard app panes in sidebar order (`SettingsSidebarView.appPanesSection`).
    pub fn standard_app_panes(debug_menu_enabled: bool) -> Vec<Self> {
        let mut panes = vec![
            Self::General,
            Self::ICloudSync,
            Self::UsageSpend,
            Self::Notifications,
            Self::MenuBar,
            Self::Menu,
            Self::Advanced,
            Self::Hooks,
            Self::Plugins,
            Self::About,
        ];
        if debug_menu_enabled {
            panes.push(Self::Debug);
        }
        panes
    }
}

/// Known display name for a provider ID.
pub fn provider_display_name(id: &str) -> String {
    match id {
        "codex" => "Codex".to_string(),
        "claude" => "Claude".to_string(),
        "antigravity" => "Antigravity".to_string(),
        "cursor" => "Cursor".to_string(),
        "openai" => "OpenAI".to_string(),
        "azure-openai" => "Azure OpenAI".to_string(),
        "gemini" => "Gemini".to_string(),
        "copilot" => "Copilot".to_string(),
        other => other.to_string(),
    }
}

/// Selected pane and Back/Forward navigation stacks (`PreferencesSelection`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsSelection {
    pane: SettingsPane,
    back_stack: Vec<SettingsPane>,
    forward_stack: Vec<SettingsPane>,
    about_root_requests: usize,
}

impl Default for SettingsSelection {
    fn default() -> Self {
        Self::new(SettingsPane::General)
    }
}

impl SettingsSelection {
    pub fn new(initial: SettingsPane) -> Self {
        Self {
            pane: initial,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            about_root_requests: 0,
        }
    }

    pub fn pane(&self) -> &SettingsPane {
        &self.pane
    }

    pub fn back_stack(&self) -> &[SettingsPane] {
        &self.back_stack
    }

    pub fn forward_stack(&self) -> &[SettingsPane] {
        &self.forward_stack
    }

    pub fn about_root_requests(&self) -> usize {
        self.about_root_requests
    }

    pub fn can_go_back(&self) -> bool {
        !self.back_stack.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward_stack.is_empty()
    }

    /// Selects `pane`, pushing the previous pane to `back_stack` and clearing `forward_stack`
    /// when the selection changes.
    pub fn select(&mut self, pane: SettingsPane) {
        if self.pane == pane {
            if pane == SettingsPane::About {
                self.request_about_root();
            }
            return;
        }
        let old = std::mem::replace(&mut self.pane, pane);
        self.back_stack.push(old);
        self.forward_stack.clear();
        if self.pane == SettingsPane::About {
            self.request_about_root();
        }
    }

    /// Navigates back to the previous pane in `back_stack`.
    pub fn go_back(&mut self) -> bool {
        let Some(previous) = self.back_stack.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.pane, previous);
        self.forward_stack.push(current);
        true
    }

    /// Navigates forward to the next pane in `forward_stack`.
    pub fn go_forward(&mut self) -> bool {
        let Some(next) = self.forward_stack.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.pane, next);
        self.back_stack.push(current);
        true
    }

    /// Bumps the About root request counter (`PreferencesSelection.requestAboutRoot`).
    pub fn request_about_root(&mut self) {
        self.about_root_requests += 1;
    }

    /// Drops panes matching `should_remove` from history and collapses repeats
    /// (`PreferencesSelection.removeFromHistory`).
    pub fn remove_from_history(&mut self, should_remove: impl Fn(&SettingsPane) -> bool) {
        self.back_stack = Self::pruned(&self.back_stack, &should_remove, &self.pane);
        self.forward_stack = Self::pruned(&self.forward_stack, &should_remove, &self.pane);
    }

    /// Ensures the Debug pane is deselected and pruned when `debug_menu_enabled` is false
    /// (`PreferencesView.ensureValidSelection`).
    pub fn ensure_valid_selection(&mut self, debug_menu_enabled: bool) {
        if debug_menu_enabled {
            return;
        }
        if self.pane == SettingsPane::Debug {
            self.pane = SettingsPane::General;
        }
        self.remove_from_history(|pane| *pane == SettingsPane::Debug);
    }

    fn pruned(
        stack: &[SettingsPane],
        should_remove: &impl Fn(&SettingsPane) -> bool,
        current: &SettingsPane,
    ) -> Vec<SettingsPane> {
        let mut result: Vec<SettingsPane> = Vec::new();
        for pane in stack {
            if !should_remove(pane) && result.last() != Some(pane) {
                result.push(pane.clone());
            }
        }
        while result.last() == Some(current) {
            result.pop();
        }
        result
    }
}

/// Window descriptor used to decide whether the app should use the `.regular`
/// Dock activation policy (`DockIconWindowDescriptor`).
#[derive(Debug, Clone, PartialEq)]
pub struct DockWindowDescriptor {
    pub identifier: Option<String>,
    pub frame_autosave_name: String,
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub is_visible: bool,
    pub is_miniaturized: bool,
    pub can_become_key: bool,
    pub is_known_settings_window: bool,
    pub is_status_bar_window: bool,
}

impl DockWindowDescriptor {
    pub fn is_settings_window(&self) -> bool {
        self.is_known_settings_window || self.identifier.as_deref() == Some(WINDOW_IDENTIFIER)
    }

    pub fn is_tiny_window(&self) -> bool {
        self.width <= 20.0 && self.height <= 20.0
    }

    pub fn is_real_window(&self) -> bool {
        self.is_visible
            && !self.is_miniaturized
            && self.can_become_key
            && !self.is_tiny_window()
            && !self.is_status_bar_window
    }
}

/// Whether the app should use `.regular` activation policy (`DockIconPolicyDecision.shouldUseRegularActivationPolicy`).
pub fn should_use_regular_activation_policy(windows: &[DockWindowDescriptor]) -> bool {
    windows.iter().any(|window| {
        window.is_real_window() || (window.is_settings_window() && window.is_miniaturized)
    })
}

/// Whether a window should promote the app to `.regular` (`DockIconPolicyDecision.shouldPromoteForPresentedWindow`).
pub fn should_promote_for_presented_window(window: &DockWindowDescriptor) -> bool {
    window.is_visible && window.is_settings_window()
}

/// Sidebar filter and sort state (`SettingsSidebarView`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SidebarFilterState {
    pub search_text: String,
    pub sorted_alphabetically: bool,
    pub enabled_only: bool,
    pub debug_menu_enabled: bool,
}

impl SidebarFilterState {
    pub fn clear_filters(&mut self) {
        self.search_text.clear();
        self.enabled_only = false;
    }

    pub fn can_reorder_providers(&self) -> bool {
        !self.sorted_alphabetically && !self.enabled_only && self.search_text.trim().is_empty()
    }
}

/// Status dot color for a provider row in the sidebar (`SettingsSidebarProviderRow.statusColor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderStatusDotColor {
    Green,
    Yellow,
    Orange,
    Red,
    Gray,
}

impl ProviderStatusDotColor {
    pub fn from_indicator(indicator: Option<StatusIndicator>) -> Self {
        match indicator {
            Some(StatusIndicator::None) => Self::Green,
            Some(StatusIndicator::Minor) => Self::Yellow,
            Some(StatusIndicator::Major) => Self::Orange,
            Some(StatusIndicator::Critical) => Self::Red,
            Some(StatusIndicator::Maintenance | StatusIndicator::Unknown) | None => Self::Gray,
        }
    }

    pub fn rgb(self) -> Rgb {
        match self {
            Self::Green => Rgb {
                r: 52,
                g: 199,
                b: 89,
            },
            Self::Yellow => Rgb {
                r: 255,
                g: 204,
                b: 0,
            },
            Self::Orange => Rgb {
                r: 255,
                g: 149,
                b: 0,
            },
            Self::Red => Rgb {
                r: 255,
                g: 59,
                b: 48,
            },
            Self::Gray => Rgb {
                r: 142,
                g: 142,
                b: 147,
            },
        }
    }
}

/// Input describing one provider for the sidebar list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarProviderInput {
    pub id: String,
    pub display_name: String,
    pub enabled: bool,
    pub refreshing: bool,
    pub status_indicator: Option<StatusIndicator>,
}

/// A provider row in the sidebar (`SettingsSidebarProviderRow`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarProviderRow {
    pub id: String,
    pub display_name: String,
    pub enabled: bool,
    pub refreshing: bool,
    pub status_dot: Option<ProviderStatusDotColor>,
    pub status_description: String,
    pub accessibility_label: String,
}

/// Full sidebar model for rendering and hit testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarModel {
    pub app_panes: Vec<SettingsPane>,
    pub enabled_provider_count: usize,
    pub providers_header_count_text: String,
    pub providers: Vec<SidebarProviderRow>,
    pub shows_empty_filter_state: bool,
}

/// Human-readable label for a `StatusIndicator` (`ProviderStatusIndicator.label`).
pub fn status_indicator_label(indicator: Option<StatusIndicator>) -> &'static str {
    match indicator {
        Some(StatusIndicator::None) => "Operational",
        Some(StatusIndicator::Minor) => "Partial outage",
        Some(StatusIndicator::Major) => "Major outage",
        Some(StatusIndicator::Critical) => "Critical issue",
        Some(StatusIndicator::Maintenance) => "Maintenance",
        Some(StatusIndicator::Unknown) | None => "Unknown",
    }
}

/// Status description tooltip (`SettingsSidebarProviderRow.statusDescription`).
pub fn provider_status_description(indicator: Option<StatusIndicator>) -> String {
    format!(
        "Provider service status: {}",
        status_indicator_label(indicator)
    )
}

/// Accessibility label for a sidebar provider row (`SettingsSidebarProviderRow.accessibilityLabel`).
pub fn provider_accessibility_label(
    name: &str,
    is_enabled: bool,
    status_checks_enabled: bool,
    indicator: Option<StatusIndicator>,
) -> String {
    if !is_enabled {
        return format!("{name} — Disabled");
    }
    if !status_checks_enabled {
        return name.to_string();
    }
    format!("{name} — {}", provider_status_description(indicator))
}

impl SidebarModel {
    /// Builds the sidebar model from a provider list and filter state.
    pub fn build(
        filter: &SidebarFilterState,
        providers: &[SidebarProviderInput],
        status_checks_enabled: bool,
    ) -> Self {
        let mut ordered = providers.to_vec();
        if filter.sorted_alphabetically {
            ordered.sort_by(|a, b| {
                b.enabled.cmp(&a.enabled).then_with(|| {
                    a.display_name
                        .to_lowercase()
                        .cmp(&b.display_name.to_lowercase())
                })
            });
        }
        let enabled_provider_count = ordered.iter().filter(|p| p.enabled).count();
        let query = filter.search_text.trim().to_lowercase();
        let visible: Vec<SidebarProviderRow> = ordered
            .into_iter()
            .filter(|p| !filter.enabled_only || p.enabled)
            .filter(|p| {
                query.is_empty()
                    || p.display_name.to_lowercase().contains(&query)
                    || p.id.to_lowercase().contains(&query)
            })
            .map(|p| {
                let status_description = provider_status_description(p.status_indicator);
                let accessibility_label = provider_accessibility_label(
                    &p.display_name,
                    p.enabled,
                    status_checks_enabled,
                    p.status_indicator,
                );
                let status_dot = (p.enabled && status_checks_enabled)
                    .then(|| ProviderStatusDotColor::from_indicator(p.status_indicator));
                SidebarProviderRow {
                    id: p.id,
                    display_name: p.display_name,
                    enabled: p.enabled,
                    refreshing: p.refreshing,
                    status_dot,
                    status_description,
                    accessibility_label,
                }
            })
            .collect();

        let shows_empty_filter_state = visible.is_empty() && !providers.is_empty();
        Self {
            app_panes: SettingsPane::standard_app_panes(filter.debug_menu_enabled),
            enabled_provider_count,
            providers_header_count_text: format!("{enabled_provider_count} on"),
            providers: visible,
            shows_empty_filter_state,
        }
    }

    /// Builds the sidebar model from an `EngineSnapshot` (or default known providers if `None`).
    pub fn from_snapshot(
        filter: &SidebarFilterState,
        snapshot: Option<&EngineSnapshot>,
        status_checks_enabled: bool,
    ) -> Self {
        let mut inputs: Vec<SidebarProviderInput> = match snapshot {
            Some(snap) if !snap.providers.is_empty() => snap
                .providers
                .iter()
                .map(provider_input_from_snapshot)
                .collect(),
            _ => default_provider_inputs(),
        };
        if !inputs.iter().any(|p| p.id == "cursor") {
            inputs.push(SidebarProviderInput {
                id: "cursor".to_string(),
                display_name: "Cursor".to_string(),
                enabled: false,
                refreshing: false,
                status_indicator: None,
            });
        }
        Self::build(filter, &inputs, status_checks_enabled)
    }
}

fn provider_input_from_snapshot(p: &ProviderSnapshot) -> SidebarProviderInput {
    let status_indicator = p
        .status
        .as_ref()
        .and_then(|m| m.value.as_ref())
        .map(|s| s.indicator);
    let refreshing = p
        .windows
        .primary
        .as_ref()
        .is_some_and(|m| m.state == MetricState::Loading)
        || p.windows
            .secondary
            .as_ref()
            .is_some_and(|m| m.state == MetricState::Loading);
    SidebarProviderInput {
        id: p.id.as_str().to_string(),
        display_name: p.display_name.clone(),
        enabled: p.enabled,
        refreshing,
        status_indicator,
    }
}

fn default_provider_inputs() -> Vec<SidebarProviderInput> {
    vec![
        SidebarProviderInput {
            id: "codex".to_string(),
            display_name: "Codex".to_string(),
            enabled: true,
            refreshing: false,
            status_indicator: Some(StatusIndicator::None),
        },
        SidebarProviderInput {
            id: "claude".to_string(),
            display_name: "Claude".to_string(),
            enabled: true,
            refreshing: false,
            status_indicator: Some(StatusIndicator::None),
        },
        SidebarProviderInput {
            id: "antigravity".to_string(),
            display_name: "Antigravity".to_string(),
            enabled: true,
            refreshing: false,
            status_indicator: Some(StatusIndicator::None),
        },
        SidebarProviderInput {
            id: "cursor".to_string(),
            display_name: "Cursor".to_string(),
            enabled: false,
            refreshing: false,
            status_indicator: None,
        },
    ]
}

/// Builds the CLI argv used to configure a secret setting in an interactive terminal
/// (`ariadusage secret set <id>`). The macOS app never holds, reads, or sends secret values.
pub fn secret_set_command_args(setting_id: &SettingId) -> [String; 3] {
    [
        "secret".to_string(),
        "set".to_string(),
        setting_id.as_str().to_string(),
    ]
}

/// Formats the terminal command line `ariadusage secret set <id>` for a secret-bearing setting.
pub fn secret_set_terminal_command(setting_id: &SettingId) -> String {
    format!("ariadusage secret set {}", setting_id.as_str())
}

/// Secret status from a `DescriptorKind::Secret` descriptor.
/// The macOS app never holds a secret value — only whether a value is set and its source label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretStatus {
    pub is_set: bool,
    pub source: Option<String>,
}

/// Snapshot of settings values resolved from IPC `SettingsPage` descriptors.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsState {
    pub available_ids: HashSet<String>,
    pub refresh_interval_seconds: f64,
    pub menu_bar_icon_style: String,
    pub menu_bar_show_used: bool,
    pub menu_bar_merge_icons: bool,
    pub show_optional_credits: bool,
    pub notifications_enabled: bool,
    pub source_mode: String,
    pub accent_color: String,
    pub secret_statuses: HashMap<String, SecretStatus>,
    pub provider_enabled: HashMap<String, bool>,
    pub provider_source_mode: HashMap<String, String>,
    pub provider_cli_version: HashMap<String, String>,
}

impl Default for SettingsState {
    fn default() -> Self {
        let mut provider_enabled = HashMap::new();
        provider_enabled.insert("codex".to_string(), true);
        provider_enabled.insert("claude".to_string(), true);
        provider_enabled.insert("antigravity".to_string(), true);
        provider_enabled.insert("cursor".to_string(), false);

        let mut provider_source_mode = HashMap::new();
        provider_source_mode.insert("codex".to_string(), "auto".to_string());
        provider_source_mode.insert("claude".to_string(), "auto".to_string());
        provider_source_mode.insert("antigravity".to_string(), "auto".to_string());
        provider_source_mode.insert("cursor".to_string(), "auto".to_string());

        Self {
            available_ids: HashSet::new(),
            refresh_interval_seconds: 900.0,
            menu_bar_icon_style: "bars".to_string(),
            menu_bar_show_used: false,
            menu_bar_merge_icons: true,
            show_optional_credits: true,
            notifications_enabled: true,
            source_mode: "cli".to_string(),
            accent_color: "#cacccc".to_string(),
            secret_statuses: HashMap::new(),
            provider_enabled,
            provider_source_mode,
            provider_cli_version: HashMap::new(),
        }
    }
}

impl SettingsState {
    /// Updates state from an engine `SettingsPage`.
    pub fn apply_page(&mut self, page: &SettingsPage) {
        for section in &page.sections {
            for desc in &section.descriptors {
                let id = desc.id.as_str().to_string();
                self.available_ids.insert(id.clone());
                match (&*id, &desc.kind) {
                    ("refreshInterval", DescriptorKind::Number { value, .. }) => {
                        self.refresh_interval_seconds = *value;
                    }
                    ("menuBar.iconStyle", DescriptorKind::Choice { selected, .. }) => {
                        self.menu_bar_icon_style = selected.clone();
                    }
                    ("menuBar.showUsed", DescriptorKind::Toggle { value }) => {
                        self.menu_bar_show_used = *value;
                    }
                    ("menuBar.mergeIcons", DescriptorKind::Toggle { value }) => {
                        self.menu_bar_merge_icons = *value;
                    }
                    ("menu.showOptionalCreditsAndExtraUsage", DescriptorKind::Toggle { value }) => {
                        self.show_optional_credits = *value;
                    }
                    ("notifications.enabled", DescriptorKind::Toggle { value }) => {
                        self.notifications_enabled = *value;
                    }
                    ("sourceMode", DescriptorKind::Choice { selected, .. }) => {
                        self.source_mode = selected.clone();
                    }
                    ("accentColor", DescriptorKind::Color { value, .. }) => {
                        self.accent_color = value.clone();
                    }
                    (_, DescriptorKind::Secret { is_set, source }) => {
                        self.secret_statuses.insert(
                            id,
                            SecretStatus {
                                is_set: *is_set,
                                source: source.clone(),
                            },
                        );
                    }
                    _ => {
                        if let Some(rest) = id.strip_prefix("providers.") {
                            if let Some(pid) = rest.strip_suffix(".enabled")
                                && let DescriptorKind::Toggle { value } = &desc.kind
                            {
                                self.provider_enabled.insert(pid.to_string(), *value);
                            } else if let Some(pid) = rest.strip_suffix(".sourceMode")
                                && let DescriptorKind::Choice { selected, .. } = &desc.kind
                            {
                                self.provider_source_mode
                                    .insert(pid.to_string(), selected.clone());
                            } else if let Some(pid) = rest.strip_suffix(".cliVersion")
                                && let DescriptorKind::Text { value, .. } = &desc.kind
                            {
                                self.provider_cli_version
                                    .insert(pid.to_string(), value.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    /// Applies an optimistic local update when the user changes an enabled control,
    /// returning the JSON value to send in `ClientMessage::SetSetting`.
    pub fn set_local(&mut self, setting_id: &str, value: serde_json::Value) {
        match setting_id {
            "refreshInterval" => {
                if let Some(v) = value.as_f64() {
                    self.refresh_interval_seconds = v;
                }
            }
            "menuBar.iconStyle" => {
                if let Some(s) = value.as_str() {
                    self.menu_bar_icon_style = s.to_string();
                }
            }
            "menuBar.showUsed" => {
                if let Some(b) = value.as_bool() {
                    self.menu_bar_show_used = b;
                }
            }
            "menuBar.mergeIcons" => {
                if let Some(b) = value.as_bool() {
                    self.menu_bar_merge_icons = b;
                }
            }
            "menu.showOptionalCreditsAndExtraUsage" => {
                if let Some(b) = value.as_bool() {
                    self.show_optional_credits = b;
                }
            }
            "notifications.enabled" => {
                if let Some(b) = value.as_bool() {
                    self.notifications_enabled = b;
                }
            }
            "sourceMode" => {
                if let Some(s) = value.as_str() {
                    self.source_mode = s.to_string();
                }
            }
            "accentColor" => {
                if let Some(s) = value.as_str() {
                    self.accent_color = s.to_string();
                }
            }
            other => {
                if let Some(rest) = other.strip_prefix("providers.") {
                    if let Some(pid) = rest.strip_suffix(".enabled") {
                        if let Some(b) = value.as_bool() {
                            self.provider_enabled.insert(pid.to_string(), b);
                        }
                    } else if let Some(pid) = rest.strip_suffix(".sourceMode")
                        && let Some(s) = value.as_str()
                    {
                        self.provider_source_mode
                            .insert(pid.to_string(), s.to_string());
                    }
                }
            }
        }
    }

    pub fn has_descriptor(&self, id: &str) -> bool {
        self.available_ids.contains(id)
    }

    pub fn refresh_interval_label(&self) -> &'static str {
        refresh_interval_label(self.refresh_interval_seconds)
    }

    pub fn menu_bar_style_label(&self) -> &'static str {
        match self.menu_bar_icon_style.as_str() {
            "bars" => "Bars",
            "critters" => "Critters",
            "iconAndPercent" => "Icon & percent",
            _ => "Bars",
        }
    }
}

/// Human-readable label for a refresh interval in seconds (`RefreshFrequency.label`).
pub fn refresh_interval_label(seconds: f64) -> &'static str {
    let s = seconds.round() as i64;
    match s {
        0 => "Manual",
        60 => "1 min",
        120 => "2 min",
        300 => "5 min",
        900 => "15 min",
        1800 => "30 min",
        _ => "15 min",
    }
}

/// Options for the Refresh interval picker (`GeneralSettingsMenuOptions.refreshFrequencies`).
pub fn refresh_interval_options() -> Vec<ChoiceItem> {
    vec![
        ChoiceItem::Number {
            label: "Manual".to_string(),
            value: 0.0,
        },
        ChoiceItem::Number {
            label: "1 min".to_string(),
            value: 60.0,
        },
        ChoiceItem::Number {
            label: "2 min".to_string(),
            value: 120.0,
        },
        ChoiceItem::Number {
            label: "5 min".to_string(),
            value: 300.0,
        },
        ChoiceItem::Number {
            label: "15 min".to_string(),
            value: 900.0,
        },
        ChoiceItem::Number {
            label: "30 min".to_string(),
            value: 1800.0,
        },
    ]
}

/// Choice option item for a popup control.
#[derive(Debug, Clone, PartialEq)]
pub enum ChoiceItem {
    Text { label: String, value: String },
    Bool { label: String, value: bool },
    Number { label: String, value: f64 },
}

impl ChoiceItem {
    pub fn label(&self) -> &str {
        match self {
            Self::Text { label, .. } | Self::Bool { label, .. } | Self::Number { label, .. } => {
                label
            }
        }
    }

    pub fn to_json_value(&self) -> serde_json::Value {
        match self {
            Self::Text { value, .. } => serde_json::Value::String(value.clone()),
            Self::Bool { value, .. } => serde_json::Value::Bool(*value),
            Self::Number { value, .. } => serde_json::json!(*value),
        }
    }
}

/// Interactive or disabled control on a form row.
#[derive(Debug, Clone, PartialEq)]
pub enum FormControl {
    Toggle {
        value: bool,
        enabled: bool,
        setting_id: Option<SettingId>,
    },
    Checkbox {
        checked: bool,
        indent: f64,
        enabled: bool,
        setting_id: Option<SettingId>,
    },
    Choice {
        selected_label: String,
        options: Vec<ChoiceItem>,
        enabled: bool,
        setting_id: Option<SettingId>,
    },
    BorderedChoice {
        selected_label: String,
        options: Vec<ChoiceItem>,
        enabled: bool,
        setting_id: Option<SettingId>,
    },
    Segmented {
        segments: Vec<String>,
        selected_index: usize,
        trailing_note: Option<String>,
        enabled: bool,
    },
    ReadOnlyValue {
        value: String,
    },
    SecretStatus {
        is_set: bool,
        source: Option<String>,
        setting_id: SettingId,
    },
    ColorHex {
        hex_value: String,
        swatch_rgb: Rgb,
        enabled: bool,
        setting_id: Option<SettingId>,
    },
    ProviderHeader {
        provider_id: String,
        display_name: String,
        subtitle: String,
        enabled: bool,
        enable_toggle_supported: bool,
        enable_setting_id: Option<SettingId>,
    },
    UsageWindowBar {
        title: String,
        percent_text: String,
        fill_fraction: f64,
        accent_rgb: Rgb,
        reset_text: String,
        pace_text: Option<String>,
    },
    EmptyStateCard {
        symbol: &'static str,
        title: String,
        subtitle: String,
    },
    SpendSummaryGrid {
        currency_code: String,
        coverage_text: String,
        estimated_spend: String,
        tracked_tokens: String,
        subscriptions: String,
        input_tokens: String,
        output_tokens: String,
        cache_read_tokens: String,
        cache_write_tokens: String,
        reasoning_tokens: String,
        provenance_text: String,
    },
    Buttons {
        buttons: Vec<FormButton>,
    },
    ShortcutRecorder {
        placeholder: String,
        enabled: bool,
    },
    TextField {
        value: String,
        placeholder: String,
        enabled: bool,
    },
    Slider {
        value: f64,
        min: f64,
        max: f64,
        value_label: String,
        reset_title: String,
        enabled: bool,
    },
    StepperPopup {
        value_text: String,
        popup_label: String,
        enabled: bool,
    },
    MenuBarLayoutPreview {
        scope_label: String,
        preset_label: String,
        preview_percent_text: String,
        strip_chips: Vec<String>,
        palette_groups: Vec<LayoutPaletteGroup>,
        enabled: bool,
    },
    ExternalLink {
        symbol: &'static str,
        url: String,
    },
    StatusNote {
        lines: Vec<String>,
        command: Option<String>,
    },
}

/// A button inside a form row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormButton {
    pub title: String,
    pub enabled: bool,
    pub prominent: bool,
    pub action: FormButtonAction,
}

/// Action triggered by a form button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormButtonAction {
    None,
    QuitApp,
    OpenUrl(String),
    RefreshProvider(Option<String>),
    LaunchSecretSetInTerminal { setting_id: SettingId },
    RunEngineAction { action_id: String },
    CopyText(String),
}

/// A token palette group inside the Menu Bar layout editor preview (`MenuBarLayoutPaletteGroup`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutPaletteGroup {
    pub title: String,
    pub chips: Vec<String>,
}

/// One row inside a grouped form section.
#[derive(Debug, Clone, PartialEq)]
pub struct FormRowModel {
    pub title: String,
    pub subtitle: Option<String>,
    pub row_enabled: bool,
    pub control: FormControl,
}

/// A grouped form section (`Section` in SwiftUI `.formStyle(.grouped)`).
#[derive(Debug, Clone, PartialEq)]
pub struct FormSectionModel {
    pub title: Option<String>,
    pub rows: Vec<FormRowModel>,
    pub footer_lines: Vec<String>,
}

/// Hero block at the top of the About pane (`AboutPane.hero`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AboutHeroModel {
    pub app_name: String,
    pub version_text: String,
    pub tagline: String,
    pub homepage_url: String,
}

/// Footer bar at the bottom of the General pane (`GeneralPane` footer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneralFooterModel {
    pub version_text: String,
    pub quit_title: String,
}

/// Full content model for a detail pane.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneModel {
    pub pane: SettingsPane,
    pub title: String,
    pub shows_header_title: bool,
    pub about_hero: Option<AboutHeroModel>,
    pub sections: Vec<FormSectionModel>,
    pub general_footer: Option<GeneralFooterModel>,
    pub centered_footer: Option<String>,
}

/// Controls across Settings panes that do not yet have an engine setting
/// descriptor or intent in `ariadusage-protocol` and are therefore rendered disabled.
pub const MISSING_ENGINE_INTENTS: &[&str] = &[
    "appLanguage (General -> System -> Language)",
    "preferredCurrencyCode (General -> System -> Preferred Currency)",
    "terminalApp (General -> System -> Default terminal)",
    "launchAtLogin (General -> System -> Start at login)",
    "refreshAllProvidersOnMenuOpen (General -> Refreshing -> Refresh when the menu opens)",
    "backgroundWorkLowPowerModePreference (General -> Refreshing -> Low Power Mode)",
    "statusChecksEnabled (General -> Refreshing -> Check provider status)",
    "exportPreferences / importPreferences / providerSwitcherShortcuts (General -> Portable preferences)",
    "openMenuShortcut (General -> Keyboard shortcut -> Open menu)",
    "iCloudSyncEnabled / iCloudSyncIncludeSecrets / iCloudSyncSnapshotsEnabled / iCloudSyncShowFleetAccounts (iCloud Sync)",
    "spendDashboardPeriod / spendTimeZone / costUsageEnabled / openCodexUsageLogs / spendExport (Usage & Spend)",
    "credentialExpiryNotificationEnabled / quotaThresholdNotificationsEnabled / paceWarningNotificationsEnabled / resetNotificationsEnabled / resetConfettiStyle (Notifications)",
    "menuBarHighContrastOnInactiveDisplays (Menu Bar -> Icon -> Improve visibility on inactive displays)",
    "menuBarColorPace (Menu Bar -> Icon -> Color Pace Indicator)",
    "menuBarPaceReservePositive (Menu Bar -> Icon -> Pace sign)",
    "menuBarColorByProvider (Menu Bar -> Icon -> Color by provider)",
    "menuBarLayout / menuBarLayoutOverrides (Menu Bar -> Layout editor)",
    "mergedIconDisplayStyle (Menu Bar -> Combined icon -> Combined icon style)",
    "switcherRowsOption (Menu Bar -> Combined icon -> Switcher rows)",
    "unifiedIconSource (Menu Bar -> Combined icon -> Combined icon source)",
    "mergedOverviewSelectedProviders (Menu Bar -> Combined icon -> Overview providers)",
    "mergedOverviewAllAccounts (Menu Bar -> Combined icon -> Show all accounts in Overview)",
    "randomBlinkEnabled (Menu Bar -> Animation -> Surprise me)",
    "quotaWarningMarkersVisible (Menu -> Usage -> Show quota warning markers)",
    "paceVisible (Menu -> Usage -> Show pace)",
    "weeklyProgressWorkDays (Menu -> Usage -> Work days)",
    "workdayTickAppearance (Menu -> Usage -> Workday tick appearance)",
    "resetTimesOption (Menu -> Usage -> Reset times)",
    "providerChangelogLinksEnabled (Menu -> Content -> Show provider changelog links)",
    "highlightMenuCardsOnHover (Menu -> Content -> Highlight menu cards on hover)",
    "mergedOverviewLayout (Menu -> Content -> Overview layout)",
    "multiAccountMenuLayout (Menu -> Content -> Multi-account layout)",
    "accountWidgetsEnabled (Menu -> Widgets -> Keep accounts updated for widgets)",
    "costSummaryOption / costReportingPeriod / costComparisonPeriodsEnabled (Menu -> Cost summary)",
    "stayAwakeEnabled / agentSessionsEnabled / agentSessionLabelStyle / agentSessionsRemoteEnabled / agentSessionsHideUnreachableHosts / agentSessionsManualHosts (Menu -> Agent sessions)",
    "httpServer.* / installCLI / hidePersonalInfo / keychainAccessDisabled / providerStorageUsageEnabled / debugMenuEnabled (Advanced)",
    "hooksEnabled / hookRules (Hooks)",
    "installPlugin / refreshPlugins / showPluginsInFinder (Plugins)",
    "autoUpdateEnabled / checkForUpdates (About -> Updates)",
    "providerVisibleUsageItems / providerCookieSource / providerQuotaWarnings / providerAdvancedOptions (Provider Detail)",
];

/// Builds the [`PaneModel`] for `pane` from current settings and snapshot state.
pub fn pane_model(
    pane: &SettingsPane,
    settings: &SettingsState,
    snapshot: Option<&EngineSnapshot>,
) -> PaneModel {
    match pane {
        SettingsPane::General => general_pane_model(settings),
        SettingsPane::ICloudSync => icloud_sync_pane_model(),
        SettingsPane::UsageSpend => usage_spend_pane_model(settings, snapshot),
        SettingsPane::Notifications => notifications_pane_model(settings),
        SettingsPane::MenuBar => menu_bar_pane_model(settings, snapshot),
        SettingsPane::Menu => menu_pane_model(settings, snapshot),
        SettingsPane::Advanced => advanced_pane_model(settings),
        SettingsPane::Hooks => hooks_pane_model(),
        SettingsPane::Plugins => plugins_pane_model(),
        SettingsPane::About => about_pane_model(),
        SettingsPane::Provider(id) => provider_pane_model(id, settings, snapshot),
        other => PaneModel {
            pane: other.clone(),
            title: other.title(),
            shows_header_title: other.shows_detail_header_title(),
            about_hero: None,
            sections: Vec::new(),
            general_footer: None,
            centered_footer: None,
        },
    }
}

fn general_pane_model(settings: &SettingsState) -> PaneModel {
    let refresh_id = SettingId::new("refreshInterval").ok();
    let refresh_enabled = settings.has_descriptor("refreshInterval");

    let system_section = FormSectionModel {
        title: Some("System".to_string()),
        rows: vec![
            FormRowModel {
                title: "Language".to_string(),
                subtitle: Some(
                    "Change the display language. Requires app restart to take full effect."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Choice {
                    selected_label: "System".to_string(),
                    options: vec![
                        ChoiceItem::Text {
                            label: "System".to_string(),
                            value: String::new(),
                        },
                        ChoiceItem::Text {
                            label: "English".to_string(),
                            value: "en".to_string(),
                        },
                    ],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Preferred Currency".to_string(),
                subtitle: Some(
                    "Currency for cost estimates and spend metrics. Uses live exchange rates updated daily."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Choice {
                    selected_label: "USD ($)".to_string(),
                    options: vec![ChoiceItem::Text {
                        label: "USD ($)".to_string(),
                        value: "USD".to_string(),
                    }],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Default terminal".to_string(),
                subtitle: Some("Terminal used by the Open Terminal action".to_string()),
                row_enabled: true,
                control: FormControl::Choice {
                    selected_label: "Terminal".to_string(),
                    options: vec![ChoiceItem::Text {
                        label: "Terminal".to_string(),
                        value: "Terminal".to_string(),
                    }],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Start at login".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    let mut refreshing_footer = Vec::new();
    if settings.refresh_interval_seconds.round() as i64 == 0 {
        refreshing_footer
            .push("Automatic refreshing is off; use Refresh in the menu to update.".to_string());
    }

    let refreshing_section = FormSectionModel {
        title: Some("Refreshing".to_string()),
        rows: vec![
            FormRowModel {
                title: "Refresh interval".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Choice {
                    selected_label: settings.refresh_interval_label().to_string(),
                    options: refresh_interval_options(),
                    enabled: refresh_enabled,
                    setting_id: refresh_id,
                },
            },
            FormRowModel {
                title: "Refresh when the menu opens".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Low Power Mode".to_string(),
                subtitle: Some(
                    "When on, runs automatic provider, local usage, and storage refreshes no more often than every 30 minutes. Manual refresh remains available. Automatic follows the system Low Power Mode setting."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Choice {
                    selected_label: "Off".to_string(),
                    options: vec![
                        ChoiceItem::Text {
                            label: "Off".to_string(),
                            value: "off".to_string(),
                        },
                        ChoiceItem::Text {
                            label: "Automatic".to_string(),
                            value: "automatic".to_string(),
                        },
                        ChoiceItem::Text {
                            label: "On".to_string(),
                            value: "on".to_string(),
                        },
                    ],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Check provider status".to_string(),
                subtitle: Some(
                    "Polls OpenAI/Claude status pages and Google Workspace for Gemini/Antigravity, surfacing incidents in the icon and menu."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: true,
                    enabled: false,
                    setting_id: None,
                },
            },
        ],
        footer_lines: refreshing_footer,
    };

    let portable_section = FormSectionModel {
        title: Some("Portable preferences".to_string()),
        rows: vec![
            FormRowModel {
                title: String::new(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Buttons {
                    buttons: vec![
                        FormButton {
                            title: "Export Preferences…".to_string(),
                            enabled: false,
                            prominent: false,
                            action: FormButtonAction::None,
                        },
                        FormButton {
                            title: "Import Preferences…".to_string(),
                            enabled: false,
                            prominent: false,
                            action: FormButtonAction::None,
                        },
                    ],
                },
            },
            FormRowModel {
                title: String::new(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Buttons {
                    buttons: vec![FormButton {
                        title: "Provider Switcher Shortcuts…".to_string(),
                        enabled: false,
                        prominent: false,
                        action: FormButtonAction::None,
                    }],
                },
            },
        ],
        footer_lines: vec![
            "Save display and notification preferences for your dotfiles. Accounts, credentials, login, local paths and consent stay on this Mac."
                .to_string(),
        ],
    };

    let shortcut_section = FormSectionModel {
        title: Some("Keyboard shortcut".to_string()),
        rows: vec![FormRowModel {
            title: "Open menu".to_string(),
            subtitle: None,
            row_enabled: true,
            control: FormControl::ShortcutRecorder {
                placeholder: "Record Shortcut".to_string(),
                enabled: false,
            },
        }],
        footer_lines: Vec::new(),
    };

    PaneModel {
        pane: SettingsPane::General,
        title: SettingsPane::General.title(),
        shows_header_title: true,
        about_hero: None,
        sections: vec![
            system_section,
            refreshing_section,
            portable_section,
            shortcut_section,
        ],
        general_footer: Some(GeneralFooterModel {
            version_text: format!("Version {APP_VERSION}"),
            quit_title: "Quit AriadUsage".to_string(),
        }),
        centered_footer: None,
    }
}

fn menu_bar_pane_model(settings: &SettingsState, snapshot: Option<&EngineSnapshot>) -> PaneModel {
    let icon_style_id = SettingId::new("menuBar.iconStyle").ok();
    let icon_style_enabled = settings.has_descriptor("menuBar.iconStyle");
    let merge_icons_id = SettingId::new("menuBar.mergeIcons").ok();
    let merge_icons_enabled = settings.has_descriptor("menuBar.mergeIcons");

    let style = settings.menu_bar_icon_style.as_str();
    let inactive_contrast_row_enabled = inactive_display_contrast_available(style);
    let icon_and_percent_row_enabled = icon_and_percent_dependents_available(style);
    let combined_row_enabled = combined_icon_dependents_available(settings.menu_bar_merge_icons);
    let surprise_row_enabled = surprise_me_available(style);

    let active_names: Vec<String> = match snapshot {
        Some(snap) => snap
            .providers
            .iter()
            .filter(|p| p.enabled)
            .take(MERGED_OVERVIEW_PROVIDER_LIMIT)
            .map(|p| p.display_name.clone())
            .collect(),
        None => vec![
            "Codex".to_string(),
            "Claude".to_string(),
            "Antigravity".to_string(),
        ],
    };

    let overview_subtitle = if !settings.menu_bar_merge_icons {
        "Enable Merge Icons to choose providers for the Overview tab.".to_string()
    } else if active_names.is_empty() {
        "No enabled providers.".to_string()
    } else {
        active_names.join(", ")
    };

    let icon_section = FormSectionModel {
        title: Some("Icon".to_string()),
        rows: vec![
            FormRowModel {
                title: "Menu bar style".to_string(),
                subtitle: Some("How the menu bar item is drawn.".to_string()),
                row_enabled: true,
                control: FormControl::Choice {
                    selected_label: settings.menu_bar_style_label().to_string(),
                    options: vec![
                        ChoiceItem::Text {
                            label: "Bars".to_string(),
                            value: "bars".to_string(),
                        },
                        ChoiceItem::Text {
                            label: "Critters".to_string(),
                            value: "critters".to_string(),
                        },
                    ],
                    enabled: icon_style_enabled,
                    setting_id: icon_style_id,
                },
            },
            FormRowModel {
                title: "Improve visibility on inactive displays".to_string(),
                subtitle: Some(
                    "Icon & percent: Use high-contrast rendering to keep the icon and metric readable on other displays."
                        .to_string(),
                ),
                row_enabled: inactive_contrast_row_enabled,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Color Pace Indicator".to_string(),
                subtitle: Some(
                    "Green pace indicator when behind pace, red when ahead of pace (risk of running out early)\nLayout: Session pace, Weekly pace, Auto pace, Spend pace"
                        .to_string(),
                ),
                row_enabled: icon_and_percent_row_enabled,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Pace sign".to_string(),
                subtitle: None,
                row_enabled: icon_and_percent_row_enabled,
                control: FormControl::Choice {
                    selected_label: "Ahead of pace is +".to_string(),
                    options: vec![
                        ChoiceItem::Bool {
                            label: "Ahead of pace is +".to_string(),
                            value: false,
                        },
                        ChoiceItem::Bool {
                            label: "Reserve is +".to_string(),
                            value: true,
                        },
                    ],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Color by provider".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    let layout_section = FormSectionModel {
        title: Some("Layout".to_string()),
        rows: vec![FormRowModel {
            title: String::new(),
            subtitle: None,
            row_enabled: icon_and_percent_row_enabled,
            control: FormControl::MenuBarLayoutPreview {
                scope_label: "All providers".to_string(),
                preset_label: "Icon & percent".to_string(),
                preview_percent_text: "67%".to_string(),
                strip_chips: vec!["Icon".to_string(), "Auto %".to_string()],
                palette_groups: vec![
                    LayoutPaletteGroup {
                        title: "Identity".to_string(),
                        chips: vec![
                            "Icon".to_string(),
                            "Provider name".to_string(),
                            "Account".to_string(),
                        ],
                    },
                    LayoutPaletteGroup {
                        title: "Usage".to_string(),
                        chips: vec![
                            "Session %".to_string(),
                            "Weekly %".to_string(),
                            "Scoped weekly %".to_string(),
                            "Spend %".to_string(),
                            "Auto %".to_string(),
                            "Usage bar".to_string(),
                            "Session pace".to_string(),
                            "Weekly pace".to_string(),
                            "Auto pace".to_string(),
                            "Spend pace".to_string(),
                        ],
                    },
                    LayoutPaletteGroup {
                        title: "Time".to_string(),
                        chips: vec![
                            "Resets in".to_string(),
                            "Reset at".to_string(),
                            "Runs out".to_string(),
                        ],
                    },
                    LayoutPaletteGroup {
                        title: "Money".to_string(),
                        chips: vec![
                            "Balance".to_string(),
                            "Cost today".to_string(),
                            "Spend".to_string(),
                        ],
                    },
                    LayoutPaletteGroup {
                        title: "Structure".to_string(),
                        chips: vec![
                            "Divider".to_string(),
                            "Dot".to_string(),
                            "Space".to_string(),
                            "Flex space".to_string(),
                            "(".to_string(),
                            ")".to_string(),
                            "[".to_string(),
                            "]".to_string(),
                            "—".to_string(),
                            ":".to_string(),
                            "/".to_string(),
                        ],
                    },
                ],
                enabled: false,
            },
        }],
        footer_lines: vec![
            "Drag tokens to arrange the menu bar. Click a token to append it; select a placed token and press Delete to remove it."
                .to_string(),
        ],
    };

    let combined_section = FormSectionModel {
        title: Some("Combined icon".to_string()),
        rows: vec![
            FormRowModel {
                title: "Merge icons".to_string(),
                subtitle: Some("Use a single menu bar icon with a provider switcher.".to_string()),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: settings.menu_bar_merge_icons,
                    enabled: merge_icons_enabled,
                    setting_id: merge_icons_id,
                },
            },
            FormRowModel {
                title: "Combined icon style".to_string(),
                subtitle: Some(
                    "Switcher cycles through providers one at a time; Stacked shows two providers at once, one row each."
                        .to_string(),
                ),
                row_enabled: combined_row_enabled,
                control: FormControl::Choice {
                    selected_label: "Switcher".to_string(),
                    options: vec![
                        ChoiceItem::Text {
                            label: "Switcher".to_string(),
                            value: "switcher".to_string(),
                        },
                        ChoiceItem::Text {
                            label: "Stacked".to_string(),
                            value: "stacked".to_string(),
                        },
                    ],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Switcher rows".to_string(),
                subtitle: None,
                row_enabled: combined_row_enabled,
                control: FormControl::Choice {
                    selected_label: "Provider icons".to_string(),
                    options: vec![ChoiceItem::Text {
                        label: "Provider icons".to_string(),
                        value: "icons".to_string(),
                    }],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Combined icon source".to_string(),
                subtitle: Some(
                    "Choose which provider the collapsed combined icon shows.".to_string(),
                ),
                row_enabled: combined_row_enabled,
                control: FormControl::Choice {
                    selected_label: "Current selection".to_string(),
                    options: vec![ChoiceItem::Text {
                        label: "Current selection".to_string(),
                        value: "current".to_string(),
                    }],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Overview providers".to_string(),
                subtitle: Some(overview_subtitle),
                row_enabled: combined_row_enabled,
                control: FormControl::Buttons {
                    buttons: vec![FormButton {
                        title: "Configure…".to_string(),
                        enabled: false,
                        prominent: false,
                        action: FormButtonAction::None,
                    }],
                },
            },
            FormRowModel {
                title: "Show all accounts in Overview".to_string(),
                subtitle: Some(
                    "Expand cached account usage. The six-provider limit still applies."
                        .to_string(),
                ),
                row_enabled: combined_row_enabled,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    let animation_section = FormSectionModel {
        title: Some("Animation".to_string()),
        rows: vec![FormRowModel {
            title: "Surprise me".to_string(),
            subtitle: Some("Check if you like your agents having some fun up there.".to_string()),
            row_enabled: surprise_row_enabled,
            control: FormControl::Toggle {
                value: false,
                enabled: false,
                setting_id: None,
            },
        }],
        footer_lines: Vec::new(),
    };

    PaneModel {
        pane: SettingsPane::MenuBar,
        title: SettingsPane::MenuBar.title(),
        shows_header_title: true,
        about_hero: None,
        sections: vec![
            icon_section,
            layout_section,
            combined_section,
            animation_section,
        ],
        general_footer: None,
        centered_footer: None,
    }
}

fn menu_pane_model(settings: &SettingsState, snapshot: Option<&EngineSnapshot>) -> PaneModel {
    let show_used_id = SettingId::new("menuBar.showUsed").ok();
    let show_used_enabled = settings.has_descriptor("menuBar.showUsed");
    let show_credits_id = SettingId::new("menu.showOptionalCreditsAndExtraUsage").ok();
    let show_credits_enabled = settings.has_descriptor("menu.showOptionalCreditsAndExtraUsage");

    let workday_tick_row_enabled = workday_tick_appearance_available(None);
    let overview_layout_row_enabled =
        combined_icon_dependents_available(settings.menu_bar_merge_icons);
    let agent_sessions_row_enabled = agent_sessions_dependents_available(false);

    let fill_label = if settings.menu_bar_show_used {
        "As used"
    } else {
        "As remaining"
    };

    let usage_section = FormSectionModel {
        title: Some("Usage".to_string()),
        rows: vec![
            FormRowModel {
                title: "Usage bars fill".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Choice {
                    selected_label: fill_label.to_string(),
                    options: vec![
                        ChoiceItem::Bool {
                            label: "As remaining".to_string(),
                            value: false,
                        },
                        ChoiceItem::Bool {
                            label: "As used".to_string(),
                            value: true,
                        },
                    ],
                    enabled: show_used_enabled,
                    setting_id: show_used_id,
                },
            },
            FormRowModel {
                title: "Show quota warning markers".to_string(),
                subtitle: Some(
                    "Draw threshold tick marks on usage bars when quota warnings are configured."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: true,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Show pace".to_string(),
                subtitle: Some(
                    "Show the ahead/behind stripe and forecast text on usage bars.".to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: true,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Work days".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Choice {
                    selected_label: "Automatic".to_string(),
                    options: vec![ChoiceItem::Text {
                        label: "Automatic".to_string(),
                        value: "auto".to_string(),
                    }],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Workday tick appearance".to_string(),
                subtitle: Some(
                    "Choose how workday boundaries appear on weekly usage bars.".to_string(),
                ),
                row_enabled: workday_tick_row_enabled,
                control: FormControl::Choice {
                    selected_label: "Subtle".to_string(),
                    options: vec![ChoiceItem::Text {
                        label: "Subtle".to_string(),
                        value: "subtle".to_string(),
                    }],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Reset times".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Choice {
                    selected_label: "Countdown".to_string(),
                    options: vec![ChoiceItem::Text {
                        label: "Countdown".to_string(),
                        value: "countdown".to_string(),
                    }],
                    enabled: false,
                    setting_id: None,
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    let content_section = FormSectionModel {
        title: Some("Content".to_string()),
        rows: vec![
            FormRowModel {
                title: "Menu width".to_string(),
                subtitle: Some(
                    "Widens usage cards and text rows across provider tabs and Overview."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Slider {
                    value: 330.0,
                    min: 280.0,
                    max: 400.0,
                    value_label: "330 pt".to_string(),
                    reset_title: "Reset".to_string(),
                    enabled: false,
                },
            },
            FormRowModel {
                title: "Show provider changelog links".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Highlight menu cards on hover".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Toggle {
                    value: true,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Overview layout".to_string(),
                subtitle: Some("Choose how much information Overview shows.".to_string()),
                row_enabled: overview_layout_row_enabled,
                control: FormControl::Choice {
                    selected_label: "Detailed".to_string(),
                    options: vec![
                        ChoiceItem::Text {
                            label: "Detailed".to_string(),
                            value: "detailed".to_string(),
                        },
                        ChoiceItem::Text {
                            label: "Compact".to_string(),
                            value: "compact".to_string(),
                        },
                    ],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Show optional credits, spending & billing details".to_string(),
                subtitle: Some(
                    "Applies across providers. Individual visibility settings still apply. Codex and Claude limit reset credits and plan dates are controlled separately."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: settings.show_optional_credits,
                    enabled: show_credits_enabled,
                    setting_id: show_credits_id,
                },
            },
            FormRowModel {
                title: "Multi-account layout".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Choice {
                    selected_label: "Segmented".to_string(),
                    options: vec![ChoiceItem::Text {
                        label: "Segmented".to_string(),
                        value: "segmented".to_string(),
                    }],
                    enabled: false,
                    setting_id: None,
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    let widgets_section = FormSectionModel {
        title: Some("Widgets".to_string()),
        rows: vec![FormRowModel {
            title: "Keep accounts updated for widgets".to_string(),
            subtitle: Some(
                "Choose an account in each Account Usage widget. Refreshes up to six accounts per provider."
                    .to_string(),
            ),
            row_enabled: true,
            control: FormControl::Toggle {
                value: false,
                enabled: false,
                setting_id: None,
            },
        }],
        footer_lines: Vec::new(),
    };

    let mut cost_footer =
        vec!["Auto-refresh: global interval (minimum 5m) · Timeout: 10m".to_string()];
    cost_footer.extend(cost_status_lines(snapshot));
    cost_footer.push(
        "Costs may be provider-reported or estimated from token usage at public API prices. Estimates are not subscription charges."
            .to_string(),
    );

    let cost_section = FormSectionModel {
        title: Some("Cost summary".to_string()),
        rows: vec![
            FormRowModel {
                title: "Cost summary".to_string(),
                subtitle: Some(
                    "Reads local usage logs. Shows today + the selected history window in the menu."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Choice {
                    selected_label: "Inline + submenu".to_string(),
                    options: vec![ChoiceItem::Text {
                        label: "Inline + submenu".to_string(),
                        value: "inlineAndSubmenu".to_string(),
                    }],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "History window".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::StepperPopup {
                    value_text: "30".to_string(),
                    popup_label: "History window: 30 days".to_string(),
                    enabled: false,
                },
            },
            FormRowModel {
                title: "Show shorter comparison periods".to_string(),
                subtitle: Some(
                    "Add 7, 30, and 90-day totals when they fit inside the selected history window. These totals reuse the same local scan."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
        ],
        footer_lines: cost_footer,
    };

    let agent_sessions_section = FormSectionModel {
        title: Some("Agent sessions".to_string()),
        rows: vec![
            FormRowModel {
                title: "Stay Awake".to_string(),
                subtitle: Some(
                    "Prevent idle system sleep while a local agent process is running, even when idle. Uses battery power; does not prevent lid-close or display sleep."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Agent sessions".to_string(),
                subtitle: Some(
                    "Show local and SSH-discovered Codex and Claude Code sessions in the menu."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Session labels".to_string(),
                subtitle: Some("Choose how agent sessions are named.".to_string()),
                row_enabled: agent_sessions_row_enabled,
                control: FormControl::Choice {
                    selected_label: "Project".to_string(),
                    options: vec![ChoiceItem::Text {
                        label: "Project".to_string(),
                        value: "project".to_string(),
                    }],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Remote sessions (Tailscale / SSH)".to_string(),
                subtitle: Some(
                    "Discover and connect to remote hosts. Turn off to monitor only this Mac."
                        .to_string(),
                ),
                row_enabled: agent_sessions_row_enabled,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Hide unreachable hosts".to_string(),
                subtitle: Some("Hide hosts when fetching their sessions fails.".to_string()),
                row_enabled: agent_sessions_row_enabled,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Additional SSH hosts".to_string(),
                subtitle: None,
                row_enabled: agent_sessions_row_enabled,
                control: FormControl::TextField {
                    value: String::new(),
                    placeholder: "user@host, user@host".to_string(),
                    enabled: false,
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    PaneModel {
        pane: SettingsPane::Menu,
        title: SettingsPane::Menu.title(),
        shows_header_title: true,
        about_hero: None,
        sections: vec![
            usage_section,
            content_section,
            widgets_section,
            cost_section,
            agent_sessions_section,
        ],
        general_footer: None,
        centered_footer: None,
    }
}

fn cost_status_lines(snapshot: Option<&EngineSnapshot>) -> Vec<String> {
    let Some(snap) = snapshot else {
        return vec![
            "Claude: no data yet".to_string(),
            "Codex: no data yet".to_string(),
        ];
    };
    snap.providers
        .iter()
        .map(|provider| {
            let name = &provider.display_name;
            if let Some(history) = provider
                .cost_history
                .as_ref()
                .and_then(|m| m.value.as_ref())
            {
                let currency = history.currency_code.as_deref().unwrap_or("USD");
                let cost = history
                    .last_30_days_cost_usd
                    .map(|v| format_currency_amount(v, currency))
                    .unwrap_or_else(|| "—".to_string());
                let days = history.history_days.unwrap_or(30);
                let window = if days == 1 {
                    "today".to_string()
                } else {
                    format!("{days}d")
                };
                format!("{name}: Updated · {window} {cost}")
            } else if let Some(err) = &provider.last_error {
                let msg = truncate_single_line(&err.message, 120);
                format!("{name}: {msg}")
            } else {
                format!("{name}: no data yet")
            }
        })
        .collect()
}

fn format_currency_amount(amount: f64, currency_code: &str) -> String {
    match currency_code {
        "USD" | "" => format!("${amount:.2}"),
        other => format!("{other} {amount:.2}"),
    }
}

fn truncate_single_line(text: &str, max_chars: usize) -> String {
    let single: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if single.chars().count() <= max_chars {
        single
    } else {
        let prefix: String = single.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{prefix}…")
    }
}

fn about_pane_model() -> PaneModel {
    let version_text = format!("Version {APP_VERSION}");
    let updates_section = FormSectionModel {
        title: Some("Updates".to_string()),
        rows: vec![
            FormRowModel {
                title: "Check for updates automatically".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: version_text.clone(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Buttons {
                    buttons: vec![FormButton {
                        title: "Check for Updates…".to_string(),
                        enabled: false,
                        prominent: false,
                        action: FormButtonAction::None,
                    }],
                },
            },
            FormRowModel {
                title: String::new(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::StatusNote {
                    lines: vec![
                        "Download macOS app updates from GitHub Releases (https://github.com/bavanchun/ariadusage/releases). CLI only:"
                            .to_string(),
                    ],
                    command: Some(
                        "cargo install --git https://github.com/bavanchun/ariadusage ariadusage-cli"
                            .to_string(),
                    ),
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    let links_section = FormSectionModel {
        title: Some("Links".to_string()),
        rows: vec![
            FormRowModel {
                title: "GitHub".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ExternalLink {
                    symbol: "chevron.left.slash.chevron.right",
                    url: "https://github.com/bavanchun/ariadusage".to_string(),
                },
            },
            FormRowModel {
                title: "Releases".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ExternalLink {
                    symbol: "tag",
                    url: "https://github.com/bavanchun/ariadusage/releases".to_string(),
                },
            },
            FormRowModel {
                title: "Issue Tracker".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ExternalLink {
                    symbol: "exclamationmark.bubble",
                    url: "https://github.com/bavanchun/ariadusage/issues".to_string(),
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    PaneModel {
        pane: SettingsPane::About,
        title: SettingsPane::About.title(),
        shows_header_title: true,
        about_hero: Some(AboutHeroModel {
            app_name: "AriadUsage".to_string(),
            version_text,
            tagline: "Local-first AI usage and spend monitor for macOS and Linux.".to_string(),
            homepage_url: "https://github.com/bavanchun/ariadusage".to_string(),
        }),
        sections: vec![updates_section, links_section],
        general_footer: None,
        centered_footer: Some(ABOUT_LICENSE_FOOTER.to_string()),
    }
}

fn icloud_sync_pane_model() -> PaneModel {
    let sync_section = FormSectionModel {
        title: Some("iCloud Sync".to_string()),
        rows: vec![
            FormRowModel {
                title: "Sync settings and providers across your Macs via iCloud".to_string(),
                subtitle: None,
                row_enabled: false,
                control: FormControl::Checkbox {
                    checked: false,
                    indent: 0.0,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Include API keys, cookies, and tokens".to_string(),
                subtitle: None,
                row_enabled: false,
                control: FormControl::Checkbox {
                    checked: true,
                    indent: 20.0,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Sync usage snapshots".to_string(),
                subtitle: None,
                row_enabled: false,
                control: FormControl::Checkbox {
                    checked: true,
                    indent: 20.0,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Show accounts from other Macs".to_string(),
                subtitle: None,
                row_enabled: false,
                control: FormControl::Checkbox {
                    checked: true,
                    indent: 20.0,
                    enabled: false,
                    setting_id: None,
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    let status_section = FormSectionModel {
        title: Some("Status".to_string()),
        rows: vec![
            FormRowModel {
                title: "Last successful fetch".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: "Never".to_string(),
                },
            },
            FormRowModel {
                title: "Last successful push".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: "Never".to_string(),
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    let macs_section = FormSectionModel {
        title: Some("Macs".to_string()),
        rows: vec![FormRowModel {
            title: "No synced Macs yet.".to_string(),
            subtitle: None,
            row_enabled: true,
            control: FormControl::ReadOnlyValue {
                value: String::new(),
            },
        }],
        footer_lines: vec![
            "Secrets use iCloud end-to-end encryption via encryptedValues. Hooks and machine-local paths never sync."
                .to_string(),
        ],
    };

    PaneModel {
        pane: SettingsPane::ICloudSync,
        title: SettingsPane::ICloudSync.title(),
        shows_header_title: true,
        about_hero: None,
        sections: vec![sync_section, status_section, macs_section],
        general_footer: None,
        centered_footer: None,
    }
}

fn usage_spend_pane_model(
    _settings: &SettingsState,
    snapshot: Option<&EngineSnapshot>,
) -> PaneModel {
    let mut total_spend = 0.0_f64;
    let mut total_tokens = 0_u64;
    let mut has_cost = false;
    let mut codex_spend_text = "$285.81 · 72.0M tokens".to_string();
    let mut claude_spend_text = "$254.09 · 61.0M tokens".to_string();

    if let Some(snap) = snapshot {
        for p in &snap.providers {
            if let Some(history) = p.cost_history.as_ref().and_then(|m| m.value.as_ref()) {
                if let Some(cost) = history.last_30_days_cost_usd {
                    total_spend += cost;
                    has_cost = true;
                }
                if let Some(tokens) = history.last_30_days_tokens {
                    total_tokens += tokens;
                }
                let formatted_cost = history
                    .last_30_days_cost_usd
                    .map(|c| format!("${c:.2}"))
                    .unwrap_or_else(|| "—".to_string());
                let formatted_tokens = history
                    .last_30_days_tokens
                    .map(format_compact_tokens)
                    .unwrap_or_else(|| "—".to_string());
                let summary = format!("{formatted_cost} · {formatted_tokens} tokens");
                if p.id.as_str() == "codex" {
                    codex_spend_text = summary;
                } else if p.id.as_str() == "claude" {
                    claude_spend_text = summary;
                }
            }
        }
    }

    let estimated_spend = if has_cost {
        format!("${total_spend:.2}")
    } else {
        "$539.90".to_string()
    };
    let tracked_tokens = if total_tokens > 0 {
        format_compact_tokens(total_tokens)
    } else {
        "133.0M".to_string()
    };

    let header_section = FormSectionModel {
        title: None,
        rows: vec![
            FormRowModel {
                title: "Usage & Spend".to_string(),
                subtitle: Some(
                    "Local estimated cost history across supported providers.".to_string(),
                ),
                row_enabled: true,
                control: FormControl::Buttons {
                    buttons: vec![FormButton {
                        title: "Refresh".to_string(),
                        enabled: true,
                        prominent: false,
                        action: FormButtonAction::RefreshProvider(None),
                    }],
                },
            },
            FormRowModel {
                title: "Time range".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Segmented {
                    segments: vec![
                        "7d".to_string(),
                        "30d".to_string(),
                        "90d".to_string(),
                        "Month to date".to_string(),
                        "All".to_string(),
                    ],
                    selected_index: 1,
                    trailing_note: None,
                    enabled: false,
                },
            },
            FormRowModel {
                title: "Statistics time zone".to_string(),
                subtitle: Some(
                    "Daily usage uses a fixed time zone. Changing it may move usage to a different day."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::BorderedChoice {
                    selected_label: "UTC".to_string(),
                    options: vec![ChoiceItem::Text {
                        label: "UTC".to_string(),
                        value: "UTC".to_string(),
                    }],
                    enabled: false,
                    setting_id: None,
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    let summary_section = FormSectionModel {
        title: Some("Summary".to_string()),
        rows: vec![FormRowModel {
            title: String::new(),
            subtitle: None,
            row_enabled: true,
            control: FormControl::SpendSummaryGrid {
                currency_code: "USD".to_string(),
                coverage_text: "Local estimated history · Coverage: 30 / 30".to_string(),
                estimated_spend: estimated_spend.clone(),
                tracked_tokens,
                subscriptions: "2".to_string(),
                input_tokens: "18.4M".to_string(),
                output_tokens: "4.2M".to_string(),
                cache_read_tokens: "98.1M".to_string(),
                cache_write_tokens: "12.3M".to_string(),
                reasoning_tokens: "1.1M".to_string(),
                provenance_text:
                    "Priced 428 · Unpriced 0 · Unmetered 0 · Estimated 428 · List-price equivalent"
                        .to_string(),
            },
        }],
        footer_lines: Vec::new(),
    };

    let breakdown_section = FormSectionModel {
        title: Some("Breakdown".to_string()),
        rows: vec![
            FormRowModel {
                title: "View".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Segmented {
                    segments: vec![
                        "Providers".to_string(),
                        "Projects".to_string(),
                        "Independent chats".to_string(),
                        "Sessions".to_string(),
                    ],
                    selected_index: 0,
                    trailing_note: None,
                    enabled: false,
                },
            },
            FormRowModel {
                title: "Codex".to_string(),
                subtitle: Some("Models: gpt-5-codex, o3".to_string()),
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: codex_spend_text,
                },
            },
            FormRowModel {
                title: "Claude".to_string(),
                subtitle: Some("Models: claude-sonnet-4-5, claude-opus-4-6".to_string()),
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: claude_spend_text,
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    let trend_section = FormSectionModel {
        title: Some("Estimated spend".to_string()),
        rows: vec![
            FormRowModel {
                title: "Trend view".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Segmented {
                    segments: vec!["Overview".to_string(), "Hour".to_string()],
                    selected_index: 0,
                    trailing_note: Some("Daily".to_string()),
                    enabled: false,
                },
            },
            FormRowModel {
                title: "Recorded spend".to_string(),
                subtitle: Some("Highest recorded spend: $42.10".to_string()),
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: estimated_spend,
                },
            },
            FormRowModel {
                title: "Token activity".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Segmented {
                    segments: vec![
                        "Daily".to_string(),
                        "Weekly".to_string(),
                        "Cumulative".to_string(),
                    ],
                    selected_index: 0,
                    trailing_note: None,
                    enabled: false,
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    let data_controls_section = FormSectionModel {
        title: Some("Data controls".to_string()),
        rows: vec![
            FormRowModel {
                title: "Track costs".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Toggle {
                    value: true,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Include OpenCodex usage logs".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Claude".to_string(),
                subtitle: None,
                row_enabled: false,
                control: FormControl::Checkbox {
                    checked: true,
                    indent: 0.0,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Codex".to_string(),
                subtitle: None,
                row_enabled: false,
                control: FormControl::Checkbox {
                    checked: true,
                    indent: 0.0,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: String::new(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Buttons {
                    buttons: vec![
                        FormButton {
                            title: "Copy JSON".to_string(),
                            enabled: false,
                            prominent: false,
                            action: FormButtonAction::None,
                        },
                        FormButton {
                            title: "Export JSON".to_string(),
                            enabled: false,
                            prominent: false,
                            action: FormButtonAction::None,
                        },
                        FormButton {
                            title: "Share Stats…".to_string(),
                            enabled: false,
                            prominent: false,
                            action: FormButtonAction::None,
                        },
                    ],
                },
            },
        ],
        footer_lines: vec!["List-price equivalent — not a billing receipt.".to_string()],
    };

    PaneModel {
        pane: SettingsPane::UsageSpend,
        title: SettingsPane::UsageSpend.title(),
        shows_header_title: false,
        about_hero: None,
        sections: vec![
            header_section,
            summary_section,
            breakdown_section,
            trend_section,
            data_controls_section,
        ],
        general_footer: None,
        centered_footer: None,
    }
}

fn format_compact_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}K", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

fn notifications_pane_model(settings: &SettingsState) -> PaneModel {
    let notif_id = SettingId::new("notifications.enabled").ok();
    let notif_enabled = settings.has_descriptor("notifications.enabled");

    let alerts_section = FormSectionModel {
        title: Some("Alerts".to_string()),
        rows: vec![
            FormRowModel {
                title: "Credential expiry".to_string(),
                subtitle: Some(
                    "Notify once when a provider account needs you to sign in again.".to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Quota depleted & restored".to_string(),
                subtitle: Some(
                    "Notifies when the 5-hour session quota hits 0% and when it becomes available again."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: settings.notifications_enabled,
                    enabled: notif_enabled,
                    setting_id: notif_id,
                },
            },
            FormRowModel {
                title: "Threshold warnings".to_string(),
                subtitle: Some(
                    "Warns when session or weekly quota remaining crosses configured thresholds."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Pace warnings".to_string(),
                subtitle: Some(
                    "Warns for Codex and Claude when session or weekly pace may run out before reset."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Reset notifications".to_string(),
                subtitle: Some(
                    "Names the provider and window when a session or weekly limit resets."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    let celebrations_section = FormSectionModel {
        title: Some("Celebrations".to_string()),
        rows: vec![FormRowModel {
            title: "Confetti on reset".to_string(),
            subtitle: Some("Play full-screen confetti when usage resets.".to_string()),
            row_enabled: true,
            control: FormControl::Choice {
                selected_label: "Off".to_string(),
                options: vec![
                    ChoiceItem::Text {
                        label: "Off".to_string(),
                        value: "off".to_string(),
                    },
                    ChoiceItem::Text {
                        label: "Random".to_string(),
                        value: "random".to_string(),
                    },
                ],
                enabled: false,
                setting_id: None,
            },
        }],
        footer_lines: Vec::new(),
    };

    PaneModel {
        pane: SettingsPane::Notifications,
        title: SettingsPane::Notifications.title(),
        shows_header_title: true,
        about_hero: None,
        sections: vec![alerts_section, celebrations_section],
        general_footer: None,
        centered_footer: None,
    }
}

fn advanced_pane_model(settings: &SettingsState) -> PaneModel {
    let server_token_id = SettingId::new("httpServer.token")
        .unwrap_or_else(|_| SettingId::new("providers.codex.apiKey").unwrap());
    let server_token_status = settings.secret_statuses.get(server_token_id.as_str());

    let http_section = FormSectionModel {
        title: Some("HTTP server".to_string()),
        rows: vec![
            FormRowModel {
                title: "Enable HTTP server".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Bind IPv4 address".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::TextField {
                    value: "127.0.0.1".to_string(),
                    placeholder: "127.0.0.1".to_string(),
                    enabled: false,
                },
            },
            FormRowModel {
                title: "Port".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::TextField {
                    value: "8080".to_string(),
                    placeholder: "8080".to_string(),
                    enabled: false,
                },
            },
            FormRowModel {
                title: "Server token".to_string(),
                subtitle: Some(
                    "Configured via ariadusage secret set; secret values never cross IPC."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::SecretStatus {
                    is_set: server_token_status.is_some_and(|s| s.is_set),
                    source: server_token_status.and_then(|s| s.source.clone()),
                    setting_id: server_token_id,
                },
            },
            FormRowModel {
                title: "Cache interval (seconds)".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::TextField {
                    value: "60".to_string(),
                    placeholder: "60".to_string(),
                    enabled: false,
                },
            },
            FormRowModel {
                title: "Request timeout (seconds; 0 disables)".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::TextField {
                    value: "30".to_string(),
                    placeholder: "30".to_string(),
                    enabled: false,
                },
            },
            FormRowModel {
                title: "Include all accounts in snapshots".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Dashboard identity".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::BorderedChoice {
                    selected_label: "Redacted".to_string(),
                    options: vec![
                        ChoiceItem::Text {
                            label: "Redacted".to_string(),
                            value: "redacted".to_string(),
                        },
                        ChoiceItem::Text {
                            label: "Full".to_string(),
                            value: "full".to_string(),
                        },
                    ],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Status".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: "Stopped".to_string(),
                },
            },
            FormRowModel {
                title: String::new(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Buttons {
                    buttons: vec![FormButton {
                        title: "Apply".to_string(),
                        enabled: false,
                        prominent: false,
                        action: FormButtonAction::None,
                    }],
                },
            },
        ],
        footer_lines: vec![
            "Off by default. Starts with AriadUsage and stops when it quits. Settings and token stay on this Mac and are excluded from sync and exports. Keep the Mac awake for remote access."
                .to_string(),
        ],
    };

    let cli_section = FormSectionModel {
        title: Some("Command line".to_string()),
        rows: vec![FormRowModel {
            title: "Install CLI".to_string(),
            subtitle: Some(
                "Symlink ariadusage to /usr/local/bin and /opt/homebrew/bin.".to_string(),
            ),
            row_enabled: true,
            control: FormControl::Buttons {
                buttons: vec![FormButton {
                    title: "Install CLI".to_string(),
                    enabled: false,
                    prominent: false,
                    action: FormButtonAction::None,
                }],
            },
        }],
        footer_lines: Vec::new(),
    };

    let privacy_section = FormSectionModel {
        title: Some("Privacy".to_string()),
        rows: vec![
            FormRowModel {
                title: "Hide personal information".to_string(),
                subtitle: Some(
                    "Obscure email addresses in the menu bar and menu UI. Also hide cost-history project/source names and paths without changing stored data."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Disable Keychain access".to_string(),
                subtitle: Some(
                    "Blocks AriadUsage-owned Keychain access and browser-cookie decryption."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
        ],
        footer_lines: vec![
            "Blocks AriadUsage's Security.framework reads and writes and Chromium Safe Storage decryption. Provider-owned CLIs launched for usage may still access credentials under their own policy. Use manual cookies, API keys, or files where supported."
                .to_string(),
        ],
    };

    let diagnostics_section = FormSectionModel {
        title: Some("Diagnostics".to_string()),
        rows: vec![
            FormRowModel {
                title: "Show provider storage usage".to_string(),
                subtitle: Some(
                    "Show local disk usage in menus. Scans known provider-owned paths in the background."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Show debug settings".to_string(),
                subtitle: Some("Expose troubleshooting tools in the Debug tab.".to_string()),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    PaneModel {
        pane: SettingsPane::Advanced,
        title: SettingsPane::Advanced.title(),
        shows_header_title: true,
        about_hero: None,
        sections: vec![
            http_section,
            cli_section,
            privacy_section,
            diagnostics_section,
        ],
        general_footer: None,
        centered_footer: None,
    }
}

fn hooks_pane_model() -> PaneModel {
    let hooks_section = FormSectionModel {
        title: Some("Hooks".to_string()),
        rows: vec![
            FormRowModel {
                title: "Enable hooks".to_string(),
                subtitle: Some(
                    "Run external commands when quota or provider events occur.".to_string(),
                ),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: false,
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title:
                    "Hooks can execute local commands on your Mac. Only configure commands you trust."
                        .to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: String::new(),
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    let rules_section = FormSectionModel {
        title: Some("Rules".to_string()),
        rows: vec![
            FormRowModel {
                title: "No hooks configured.".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: String::new(),
                },
            },
            FormRowModel {
                title: String::new(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Buttons {
                    buttons: vec![FormButton {
                        title: "Add rule".to_string(),
                        enabled: false,
                        prominent: false,
                        action: FormButtonAction::None,
                    }],
                },
            },
        ],
        footer_lines: Vec::new(),
    };

    PaneModel {
        pane: SettingsPane::Hooks,
        title: SettingsPane::Hooks.title(),
        shows_header_title: true,
        about_hero: None,
        sections: vec![hooks_section, rules_section],
        general_footer: None,
        centered_footer: None,
    }
}

fn plugins_pane_model() -> PaneModel {
    let plugins_section = FormSectionModel {
        title: Some("Provider Plugins".to_string()),
        rows: vec![
            FormRowModel {
                title: "Install plugin".to_string(),
                subtitle: Some(
                    "Copies a JavaScript or TypeScript file into the plugins directory."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Buttons {
                    buttons: vec![FormButton {
                        title: "Install…".to_string(),
                        enabled: false,
                        prominent: false,
                        action: FormButtonAction::None,
                    }],
                },
            },
            FormRowModel {
                title: "Plugins directory".to_string(),
                subtitle: Some("~/.config/ariadusage/providers".to_string()),
                row_enabled: true,
                control: FormControl::Buttons {
                    buttons: vec![
                        FormButton {
                            title: "Refresh".to_string(),
                            enabled: false,
                            prominent: false,
                            action: FormButtonAction::None,
                        },
                        FormButton {
                            title: "Show in Finder".to_string(),
                            enabled: false,
                            prominent: false,
                            action: FormButtonAction::None,
                        },
                    ],
                },
            },
        ],
        footer_lines: vec![
            "Plugins are local JavaScript or TypeScript files. Network and cookie access require approval."
                .to_string(),
        ],
    };

    let empty_section = FormSectionModel {
        title: None,
        rows: vec![FormRowModel {
            title: String::new(),
            subtitle: None,
            row_enabled: true,
            control: FormControl::EmptyStateCard {
                symbol: "puzzlepiece.extension",
                title: "No Plugins Installed".to_string(),
                subtitle: "Drop a .js or .ts file into the plugins directory, or choose Install…"
                    .to_string(),
            },
        }],
        footer_lines: Vec::new(),
    };

    PaneModel {
        pane: SettingsPane::Plugins,
        title: SettingsPane::Plugins.title(),
        shows_header_title: true,
        about_hero: None,
        sections: vec![plugins_section, empty_section],
        general_footer: None,
        centered_footer: None,
    }
}

fn provider_accent_defaults(provider_id: &str) -> (&'static str, Rgb) {
    match provider_id {
        "codex" => (
            "#49A3B0",
            Rgb {
                r: 73,
                g: 163,
                b: 176,
            },
        ),
        "claude" => (
            "#CC7C5E",
            Rgb {
                r: 204,
                g: 124,
                b: 94,
            },
        ),
        "cursor" => (
            "#F54E00",
            Rgb {
                r: 245,
                g: 78,
                b: 0,
            },
        ),
        "antigravity" => (
            "#60BA7E",
            Rgb {
                r: 96,
                g: 186,
                b: 126,
            },
        ),
        _ => (
            "#49A3B0",
            Rgb {
                r: 73,
                g: 163,
                b: 176,
            },
        ),
    }
}

fn source_mode_label(mode: &str) -> &'static str {
    match mode {
        "auto" => "Auto",
        "cli" => "CLI",
        "web" => "Web Cookie",
        "api" => "API Token",
        _ => "Auto",
    }
}

fn source_mode_options() -> Vec<ChoiceItem> {
    vec![
        ChoiceItem::Text {
            label: "Auto".to_string(),
            value: "auto".to_string(),
        },
        ChoiceItem::Text {
            label: "CLI".to_string(),
            value: "cli".to_string(),
        },
        ChoiceItem::Text {
            label: "Web Cookie".to_string(),
            value: "web".to_string(),
        },
        ChoiceItem::Text {
            label: "API Token".to_string(),
            value: "api".to_string(),
        },
    ]
}

fn provider_pane_model(
    provider_id: &str,
    settings: &SettingsState,
    snapshot: Option<&EngineSnapshot>,
) -> PaneModel {
    let display_name = provider_display_name(provider_id);
    let is_supported = matches!(provider_id, "codex" | "claude" | "antigravity");
    let snap_provider =
        snapshot.and_then(|s| s.providers.iter().find(|p| p.id.as_str() == provider_id));

    let enabled = settings
        .provider_enabled
        .get(provider_id)
        .copied()
        .or_else(|| snap_provider.map(|p| p.enabled))
        .unwrap_or(is_supported);

    let enable_id_str = format!("providers.{provider_id}.enabled");
    let enable_toggle_supported = is_supported
        && (settings.has_descriptor(&enable_id_str) || settings.available_ids.is_empty());
    let enable_setting_id = enable_toggle_supported
        .then(|| SettingId::new(&enable_id_str).ok())
        .flatten();

    let mode_id_str = format!("providers.{provider_id}.sourceMode");
    let mode_setting_id = SettingId::new(&mode_id_str).ok();
    let mode_supported = is_supported
        && (settings.has_descriptor(&mode_id_str) || settings.available_ids.is_empty());
    let current_mode = settings
        .provider_source_mode
        .get(provider_id)
        .map(String::as_str)
        .unwrap_or("auto");

    let source_label = snap_provider
        .and_then(|p| {
            p.windows
                .primary
                .as_ref()
                .and_then(|m| m.source.as_ref())
                .or_else(|| p.identity.as_ref().and_then(|m| m.source.as_ref()))
                .map(|s| s.label.clone())
        })
        .unwrap_or_else(|| match provider_id {
            "codex" | "claude" => "oauth".to_string(),
            "antigravity" => "local".to_string(),
            _ => "web".to_string(),
        });

    let updated_text = if enabled && is_supported {
        "Updated just now".to_string()
    } else {
        "Not fetched yet".to_string()
    };
    let header_subtitle = if enabled && is_supported {
        format!("{source_label} • just now")
    } else if provider_id == "cursor" {
        "web • last fetch failed".to_string()
    } else {
        "disabled • not configured".to_string()
    };

    // 1. Overview card (ProviderHeader + key-value info rows)
    let mut overview_rows = vec![
        FormRowModel {
            title: display_name.clone(),
            subtitle: Some(header_subtitle),
            row_enabled: true,
            control: FormControl::ProviderHeader {
                provider_id: provider_id.to_string(),
                display_name: display_name.clone(),
                subtitle: format!("{source_label} • {updated_text}"),
                enabled,
                enable_toggle_supported,
                enable_setting_id,
            },
        },
        FormRowModel {
            title: "Source".to_string(),
            subtitle: None,
            row_enabled: true,
            control: FormControl::ReadOnlyValue {
                value: source_label,
            },
        },
    ];

    // Upstream omits `Version` for Antigravity (`showsVersionInSettings == false`).
    if provider_id != "antigravity" {
        let cli_ver = settings
            .provider_cli_version
            .get(provider_id)
            .cloned()
            .unwrap_or_else(|| "not detected".to_string());
        overview_rows.push(FormRowModel {
            title: "Version".to_string(),
            subtitle: None,
            row_enabled: true,
            control: FormControl::ReadOnlyValue { value: cli_ver },
        });
    }

    overview_rows.push(FormRowModel {
        title: "Updated".to_string(),
        subtitle: None,
        row_enabled: true,
        control: FormControl::ReadOnlyValue {
            value: updated_text,
        },
    });

    if is_supported || provider_id == "cursor" {
        let status_text = snap_provider
            .and_then(|p| p.status.as_ref())
            .and_then(|m| m.value.as_ref())
            .and_then(|s| s.description.clone())
            .unwrap_or_else(|| "All Systems Operational".to_string());
        overview_rows.push(FormRowModel {
            title: "Status".to_string(),
            subtitle: None,
            row_enabled: true,
            control: FormControl::ReadOnlyValue { value: status_text },
        });
    }

    if is_supported {
        let email = snap_provider
            .and_then(|p| p.identity.as_ref())
            .and_then(|m| m.value.as_ref())
            .and_then(|id| id.account_email.clone())
            .unwrap_or_else(|| "alex@example.invalid".to_string());
        let plan = snap_provider
            .and_then(|p| p.identity.as_ref())
            .and_then(|m| m.value.as_ref())
            .and_then(|id| id.plan.clone().or_else(|| id.login_method.clone()))
            .unwrap_or_else(|| {
                if provider_id == "claude" {
                    "Max".to_string()
                } else {
                    "Pro".to_string()
                }
            });
        overview_rows.push(FormRowModel {
            title: "Account".to_string(),
            subtitle: None,
            row_enabled: true,
            control: FormControl::ReadOnlyValue { value: email },
        });
        overview_rows.push(FormRowModel {
            title: "Plan".to_string(),
            subtitle: None,
            row_enabled: true,
            control: FormControl::ReadOnlyValue { value: plan },
        });
    }

    let overview_section = FormSectionModel {
        title: None,
        rows: overview_rows,
        footer_lines: Vec::new(),
    };

    let (default_hex, accent_rgb) = provider_accent_defaults(provider_id);

    // 2. Usage section
    let usage_rows = match provider_id {
        "codex" => vec![
            FormRowModel {
                title: "Session".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::UsageWindowBar {
                    title: "Session".to_string(),
                    percent_text: "58% left".to_string(),
                    fill_fraction: 0.58,
                    accent_rgb,
                    reset_text: "Resets in 5 hours".to_string(),
                    pace_text: Some("6% in reserve · Lasts until reset".to_string()),
                },
            },
            FormRowModel {
                title: "Weekly".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::UsageWindowBar {
                    title: "Weekly".to_string(),
                    percent_text: "81% left".to_string(),
                    fill_fraction: 0.81,
                    accent_rgb,
                    reset_text: "Resets in 4 days".to_string(),
                    pace_text: Some("4% in reserve · Lasts until reset".to_string()),
                },
            },
            FormRowModel {
                title: "Limit Reset Credits".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: "2 available".to_string(),
                },
            },
            FormRowModel {
                title: "Credits".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: "184.5 left".to_string(),
                },
            },
            FormRowModel {
                title: "Cost".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: "Today: $0.18 · 460K tokens  |  30d: $285.81 · 72.0M tokens".to_string(),
                },
            },
        ],
        "claude" => vec![
            FormRowModel {
                title: "Session".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::UsageWindowBar {
                    title: "Session".to_string(),
                    percent_text: "76% left".to_string(),
                    fill_fraction: 0.76,
                    accent_rgb,
                    reset_text: "Resets in 3 hours".to_string(),
                    pace_text: Some("13% in reserve · Lasts until reset".to_string()),
                },
            },
            FormRowModel {
                title: "Weekly".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::UsageWindowBar {
                    title: "Weekly".to_string(),
                    percent_text: "69% left".to_string(),
                    fill_fraction: 0.69,
                    accent_rgb,
                    reset_text: "Resets in 5 days".to_string(),
                    pace_text: Some("8% in reserve · Lasts until reset".to_string()),
                },
            },
            FormRowModel {
                title: "Sonnet".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::UsageWindowBar {
                    title: "Sonnet".to_string(),
                    percent_text: "91% left".to_string(),
                    fill_fraction: 0.91,
                    accent_rgb,
                    reset_text: "Resets in 5 days".to_string(),
                    pace_text: None,
                },
            },
            FormRowModel {
                title: "Plan renews".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: "May 1".to_string(),
                },
            },
            FormRowModel {
                title: "Cloud credits".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: "$38.20".to_string(),
                },
            },
            FormRowModel {
                title: "Cost".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: "Today: $1.42 · 890K tokens  |  30d: $254.09 · 61.0M tokens".to_string(),
                },
            },
        ],
        "antigravity" => vec![
            FormRowModel {
                title: "Gemini Models".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::UsageWindowBar {
                    title: "Gemini Models".to_string(),
                    percent_text: "100% left".to_string(),
                    fill_fraction: 1.0,
                    accent_rgb,
                    reset_text: "Resets in 4 hours".to_string(),
                    pace_text: None,
                },
            },
            FormRowModel {
                title: "Claude and GPT".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::UsageWindowBar {
                    title: "Claude and GPT".to_string(),
                    percent_text: "80% left".to_string(),
                    fill_fraction: 0.80,
                    accent_rgb,
                    reset_text: "Resets in 4 hours".to_string(),
                    pace_text: None,
                },
            },
            FormRowModel {
                title: "Gemini 2.5 Flash".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::UsageWindowBar {
                    title: "Gemini 2.5 Flash".to_string(),
                    percent_text: "100% left".to_string(),
                    fill_fraction: 1.0,
                    accent_rgb,
                    reset_text: "Resets in 4 hours".to_string(),
                    pace_text: None,
                },
            },
        ],
        _ => vec![
            FormRowModel {
                title: "No usage yet".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: String::new(),
                },
            },
            FormRowModel {
                title: format!(
                    "Last {display_name} fetch failed: No {display_name} session found. Sign in in your browser or configure a session cookie."
                ),
                subtitle: None,
                row_enabled: true,
                control: FormControl::ReadOnlyValue {
                    value: String::new(),
                },
            },
        ],
    };

    let mut sections = vec![
        overview_section,
        FormSectionModel {
            title: Some("Usage".to_string()),
            rows: usage_rows,
            footer_lines: Vec::new(),
        },
    ];

    // 3. Visible usage items (for supported providers with usage items)
    let visible_items: &[&str] = match provider_id {
        "codex" => &["Session", "Weekly", "Limit Reset Credits", "Credits"],
        "claude" => &[
            "Session",
            "Weekly",
            "Sonnet",
            "Plan renewal / expiry",
            "Cloud credits",
            "Credits",
        ],
        "antigravity" => &["Gemini Models", "Claude and GPT", "Gemini 2.5 Flash"],
        _ => &[],
    };
    if !visible_items.is_empty() {
        let mut item_rows: Vec<FormRowModel> = visible_items
            .iter()
            .map(|label| FormRowModel {
                title: (*label).to_string(),
                subtitle: None,
                row_enabled: false,
                control: FormControl::Checkbox {
                    checked: true,
                    indent: 0.0,
                    enabled: false,
                    setting_id: None,
                },
            })
            .collect();
        item_rows.push(FormRowModel {
            title: String::new(),
            subtitle: None,
            row_enabled: true,
            control: FormControl::Buttons {
                buttons: vec![FormButton {
                    title: "Restore Defaults".to_string(),
                    enabled: false,
                    prominent: false,
                    action: FormButtonAction::None,
                }],
            },
        });
        sections.push(FormSectionModel {
            title: Some("Visible usage items".to_string()),
            rows: item_rows,
            footer_lines: vec![
                "Choose which usage rows appear in this provider's menu, Settings preview, and Overview."
                    .to_string(),
            ],
        });
    }

    // 4. Connection section
    let connection_rows = match provider_id {
        "codex" => vec![
            FormRowModel {
                title: "Quota usage source".to_string(),
                subtitle: Some(
                    "Controls live session and weekly quota fetching only. Local session cost estimates work independently."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::BorderedChoice {
                    selected_label: source_mode_label(current_mode).to_string(),
                    options: source_mode_options(),
                    enabled: mode_supported,
                    setting_id: mode_setting_id,
                },
            },
            FormRowModel {
                title: "OpenAI cookies".to_string(),
                subtitle: Some(
                    "Automatic imports browser cookies for dashboard extras.".to_string(),
                ),
                row_enabled: true,
                control: FormControl::BorderedChoice {
                    selected_label: "Auto".to_string(),
                    options: vec![ChoiceItem::Text {
                        label: "Auto".to_string(),
                        value: "auto".to_string(),
                    }],
                    enabled: false,
                    setting_id: None,
                },
            },
        ],
        "claude" => vec![
            FormRowModel {
                title: "Usage source".to_string(),
                subtitle: Some(
                    "Auto falls back to the next source if the preferred one fails.".to_string(),
                ),
                row_enabled: true,
                control: FormControl::BorderedChoice {
                    selected_label: source_mode_label(current_mode).to_string(),
                    options: source_mode_options(),
                    enabled: mode_supported,
                    setting_id: mode_setting_id,
                },
            },
            FormRowModel {
                title: "Keychain prompt policy".to_string(),
                subtitle: Some(
                    "Control when Claude's Keychain entry may be accessed during usage checks."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::BorderedChoice {
                    selected_label: "Only on user action".to_string(),
                    options: vec![ChoiceItem::Text {
                        label: "Only on user action".to_string(),
                        value: "userAction".to_string(),
                    }],
                    enabled: false,
                    setting_id: None,
                },
            },
            FormRowModel {
                title: "Claude cookies".to_string(),
                subtitle: Some("Automatic imports browser cookies for the web API.".to_string()),
                row_enabled: true,
                control: FormControl::BorderedChoice {
                    selected_label: "Auto".to_string(),
                    options: vec![ChoiceItem::Text {
                        label: "Auto".to_string(),
                        value: "auto".to_string(),
                    }],
                    enabled: false,
                    setting_id: None,
                },
            },
        ],
        "antigravity" => vec![
            FormRowModel {
                title: "Usage source".to_string(),
                subtitle: Some(
                    "Auto skips agy reports without account identity for selected or injected Google accounts. Try Local API / agy CLI to use the local app or agy's signed-in account, which may differ."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::BorderedChoice {
                    selected_label: source_mode_label(current_mode).to_string(),
                    options: source_mode_options(),
                    enabled: mode_supported,
                    setting_id: mode_setting_id,
                },
            },
            FormRowModel {
                title: "Google OAuth".to_string(),
                subtitle: Some(
                    "Sign in with Google to check Antigravity quota directly. Uses the same credentials as the Antigravity IDE."
                        .to_string(),
                ),
                row_enabled: true,
                control: FormControl::Buttons {
                    buttons: vec![FormButton {
                        title: "Add Google Account".to_string(),
                        enabled: false,
                        prominent: false,
                        action: FormButtonAction::None,
                    }],
                },
            },
        ],
        _ => vec![FormRowModel {
            title: "Cookie source".to_string(),
            subtitle: Some("Automatic imports browser cookies or stored sessions.".to_string()),
            row_enabled: true,
            control: FormControl::BorderedChoice {
                selected_label: "Auto".to_string(),
                options: vec![ChoiceItem::Text {
                    label: "Auto".to_string(),
                    value: "auto".to_string(),
                }],
                enabled: false,
                setting_id: None,
            },
        }],
    };
    sections.push(FormSectionModel {
        title: Some("Connection".to_string()),
        rows: connection_rows,
        footer_lines: Vec::new(),
    });

    // 5. Credentials / Accounts / Secret-bearing settings (`AGENTS.md`: show only set/unset + "Set in Terminal")
    if is_supported {
        let secret_id_str = format!("providers.{provider_id}.apiKey");
        let secret_id = SettingId::new(&secret_id_str)
            .unwrap_or_else(|_| SettingId::new("providers.codex.apiKey").unwrap());
        let secret_status = settings.secret_statuses.get(&secret_id_str);
        let secret_title = if provider_id == "claude" {
            "Admin API key".to_string()
        } else {
            format!("{display_name} API key")
        };

        let mut cred_rows = vec![FormRowModel {
            title: secret_title,
            subtitle: Some(
                "Configured via ariadusage secret set; secret values never cross IPC.".to_string(),
            ),
            row_enabled: true,
            control: FormControl::SecretStatus {
                is_set: secret_status.is_some_and(|s| s.is_set),
                source: secret_status.and_then(|s| s.source.clone()),
                setting_id: secret_id,
            },
        }];

        match provider_id {
            "codex" => {
                cred_rows.push(FormRowModel {
                    title: "AriadUsage follows".to_string(),
                    subtitle: Some(
                        "Choose which Codex account AriadUsage should follow.".to_string(),
                    ),
                    row_enabled: true,
                    control: FormControl::BorderedChoice {
                        selected_label: "Work Account".to_string(),
                        options: vec![
                            ChoiceItem::Text {
                                label: "Work Account".to_string(),
                                value: "acc-1".to_string(),
                            },
                            ChoiceItem::Text {
                                label: "Personal Account".to_string(),
                                value: "acc-2".to_string(),
                            },
                        ],
                        enabled: false,
                        setting_id: None,
                    },
                });
                cred_rows.push(FormRowModel {
                    title: "System".to_string(),
                    subtitle: Some("The default Codex account on this Mac.".to_string()),
                    row_enabled: true,
                    control: FormControl::BorderedChoice {
                        selected_label: "Work Account".to_string(),
                        options: vec![ChoiceItem::Text {
                            label: "Work Account".to_string(),
                            value: "acc-1".to_string(),
                        }],
                        enabled: false,
                        setting_id: None,
                    },
                });
                cred_rows.push(FormRowModel {
                    title: String::new(),
                    subtitle: None,
                    row_enabled: true,
                    control: FormControl::Buttons {
                        buttons: vec![FormButton {
                            title: "Add Account".to_string(),
                            enabled: false,
                            prominent: false,
                            action: FormButtonAction::None,
                        }],
                    },
                });
            }
            "antigravity" => {
                cred_rows.push(FormRowModel {
                    title: "Google accounts".to_string(),
                    subtitle: Some("Work Account (Active) · Personal Account".to_string()),
                    row_enabled: true,
                    control: FormControl::Buttons {
                        buttons: vec![FormButton {
                            title: "Reload".to_string(),
                            enabled: false,
                            prominent: false,
                            action: FormButtonAction::None,
                        }],
                    },
                });
                cred_rows.push(FormRowModel {
                    title: "Additional Gemini profile homes".to_string(),
                    subtitle: Some(
                        "Add directories containing .gemini OAuth credentials to monitor multiple Google accounts."
                            .to_string(),
                    ),
                    row_enabled: true,
                    control: FormControl::Buttons {
                        buttons: vec![FormButton {
                            title: "Add folder…".to_string(),
                            enabled: false,
                            prominent: false,
                            action: FormButtonAction::None,
                        }],
                    },
                });
            }
            _ => {}
        }

        sections.push(FormSectionModel {
            title: Some("Credentials & Accounts".to_string()),
            rows: cred_rows,
            footer_lines: Vec::new(),
        });
    }

    // 6. Accent color section
    sections.push(FormSectionModel {
        title: Some("Accent color".to_string()),
        rows: vec![FormRowModel {
            title: "Accent color".to_string(),
            subtitle: None,
            row_enabled: true,
            control: FormControl::ColorHex {
                hex_value: default_hex.to_string(),
                swatch_rgb: accent_rgb,
                enabled: false,
                setting_id: None,
            },
        }],
        footer_lines: vec![
            "Sets the color of this provider's usage bars, charts, and switcher tab.".to_string(),
        ],
    });

    // 7. Quota warnings section
    sections.push(FormSectionModel {
        title: Some("Quota warnings".to_string()),
        rows: vec![
            FormRowModel {
                title: "Session".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Segmented {
                    segments: vec![
                        "Global".to_string(),
                        "Custom".to_string(),
                        "Off".to_string(),
                    ],
                    selected_index: 0,
                    trailing_note: Some("Inherited: Warning 50%, Critical 20%".to_string()),
                    enabled: false,
                },
            },
            FormRowModel {
                title: "Weekly".to_string(),
                subtitle: None,
                row_enabled: true,
                control: FormControl::Segmented {
                    segments: vec![
                        "Global".to_string(),
                        "Custom".to_string(),
                        "Off".to_string(),
                    ],
                    selected_index: 0,
                    trailing_note: Some("Inherited: Warning 50%, Critical 20%".to_string()),
                    enabled: false,
                },
            },
        ],
        footer_lines: vec![
            "Quota warning notifications are disabled globally. These settings still control usage-bar markers."
                .to_string(),
        ],
    });

    // 8. Options section (for supported providers)
    let option_items: &[(&str, &str, bool)] = match provider_id {
        "codex" => &[
            (
                "Local session cost estimates",
                "Scan ~/.codex/sessions to estimate token spend.",
                true,
            ),
            (
                "Historical tracking",
                "Keep local quota history for pace forecasts.",
                false,
            ),
            (
                "OpenAI web extras",
                "Fetch credits and billing details from chatgpt.com.",
                false,
            ),
            (
                "External Codex OAuth sources",
                "Read third-party Codex OAuth credential stores.",
                false,
            ),
            (
                "OpenAI web battery saver",
                "Reduce background browser cookie refreshes on battery power.",
                false,
            ),
        ],
        "claude" => &[
            (
                "Use Claude desktop app cookies",
                "Allow reading session cookies from the Claude macOS app.",
                false,
            ),
            (
                "Show workspace spend",
                "Fetch organization spend using the Admin API key.",
                false,
            ),
            (
                "Show model-specific weekly usage in widgets",
                "Include Sonnet/Opus breakdowns in macOS widgets.",
                false,
            ),
            (
                "Allow reading Claude Code's credentials",
                "Read OAuth tokens stored by the claude CLI.",
                false,
            ),
            (
                "Avoid Keychain prompts",
                "Skip Keychain-backed sources during background refreshes.",
                false,
            ),
            (
                "Read accounts from claude-swap",
                "Import multi-account profiles managed by claude-swap.",
                false,
            ),
        ],
        "antigravity" => &[(
            "Prioritize exhausted quotas",
            "Pin depleted model groups to the top of the menu card.",
            false,
        )],
        _ => &[],
    };
    if !option_items.is_empty() {
        let opt_rows = option_items
            .iter()
            .map(|(title, sub, val)| FormRowModel {
                title: (*title).to_string(),
                subtitle: Some((*sub).to_string()),
                row_enabled: true,
                control: FormControl::Toggle {
                    value: *val,
                    enabled: false,
                    setting_id: None,
                },
            })
            .collect();
        sections.push(FormSectionModel {
            title: Some("Options".to_string()),
            rows: opt_rows,
            footer_lines: Vec::new(),
        });
    }

    PaneModel {
        pane: SettingsPane::Provider(provider_id.to_string()),
        title: display_name,
        shows_header_title: false,
        about_hero: None,
        sections,
        general_footer: None,
        centered_footer: None,
    }
}
