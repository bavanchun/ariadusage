// Ported from CodexBar Sources/CodexBarCore/Providers/Providers.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderUsagePresentation.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCLI/CLIOptions.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::{HashMap, HashSet};

use ariadusage_core::{ProviderPaceCapability, RateWindow as CoreRateWindow, UsagePace};
use ariadusage_protocol::{
    CliNamedRateWindow, CliProviderCostSnapshot, CliRateWindow, CliUsageSnapshot, ProviderId,
    ProviderWindowLabelsPayload,
};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

use crate::format::format_cost_amount;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CliProvider {
    Codex,
    Claude,
    Antigravity,
}

impl CliProvider {
    pub const ALL: &'static [Self] = &[Self::Codex, Self::Claude, Self::Antigravity];

    pub const V1_PROVIDERS: &'static [Self] = &[Self::Codex, Self::Claude, Self::Antigravity];

    #[must_use]
    pub const fn raw_value(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Antigravity => "antigravity",
        }
    }

    #[must_use]
    pub fn from_cli_name(raw: &str) -> Option<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "codex" => Some(Self::Codex),
            "claude" => Some(Self::Claude),
            "antigravity" => Some(Self::Antigravity),
            _ => None,
        }
    }

    #[must_use]
    pub fn to_core_provider_id(self) -> ProviderId {
        ProviderId::new(self.raw_value()).expect("valid provider id")
    }

    #[must_use]
    pub fn from_core_provider_id(id: &ProviderId) -> Option<Self> {
        Self::from_cli_name(id.as_str())
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::Antigravity => "Antigravity",
        }
    }

    #[must_use]
    pub const fn cli_short_label(self) -> Option<&'static str> {
        let _ = self;
        None
    }

    #[must_use]
    pub const fn supports_credits(self) -> bool {
        matches!(self, Self::Codex)
    }

    #[must_use]
    pub const fn is_primary_provider(self) -> bool {
        matches!(self, Self::Codex | Self::Claude | Self::Antigravity)
    }

    #[must_use]
    pub const fn default_enabled(self) -> bool {
        matches!(self, Self::Codex | Self::Claude)
    }

    #[must_use]
    pub const fn uses_account_fallback(self) -> bool {
        matches!(self, Self::Codex)
    }

    #[must_use]
    pub const fn uses_detail_backed_window(self) -> bool {
        let _ = self;
        false
    }

    #[must_use]
    pub const fn shows_headroom_hint(self) -> bool {
        matches!(self, Self::Codex)
    }

    #[must_use]
    pub const fn status_page_url(self) -> Option<&'static str> {
        match self {
            Self::Codex => Some("https://status.openai.com/"),
            Self::Claude => Some("https://status.claude.com/"),
            Self::Antigravity => None,
        }
    }

    #[must_use]
    pub const fn status_link_url(self) -> Option<&'static str> {
        match self {
            Self::Antigravity => Some(
                "https://www.google.com/appsstatus/dashboard/products/npdyhgECDJ6tB66MxXyo/history",
            ),
            Self::Codex | Self::Claude => None,
        }
    }

    #[must_use]
    pub const fn status_workspace_product_id(self) -> Option<&'static str> {
        match self {
            Self::Antigravity => Some("npdyhgECDJ6tB66MxXyo"),
            Self::Codex | Self::Claude => None,
        }
    }

    #[must_use]
    pub fn rate_window_labels(
        self,
        _snapshot: &CliUsageSnapshot,
        _now: Timestamp,
    ) -> ProviderRateWindowLabels {
        match self {
            Self::Codex => ProviderRateWindowLabels {
                primary: "Session".to_string(),
                secondary: "Weekly".to_string(),
                tertiary: "Sonnet".to_string(),
                shows_tertiary: false,
            },
            Self::Claude => ProviderRateWindowLabels {
                primary: "Session".to_string(),
                secondary: "Weekly".to_string(),
                tertiary: "Sonnet".to_string(),
                shows_tertiary: true,
            },
            Self::Antigravity => ProviderRateWindowLabels {
                primary: "Gemini Models".to_string(),
                secondary: "Claude and GPT".to_string(),
                tertiary: "Sonnet".to_string(),
                shows_tertiary: false,
            },
        }
    }

    #[must_use]
    pub fn extra_rate_windows(self, _snapshot: &CliUsageSnapshot) -> Vec<CliNamedRateWindow> {
        let _ = self;
        Vec::new()
    }

    #[must_use]
    pub fn resolve_pace(
        self,
        snapshot: &CliUsageSnapshot,
        slot: ProviderPaceSlot,
        window: &CliRateWindow,
        now: Timestamp,
        weekly_work_days: Option<u32>,
    ) -> Option<ProviderPaceResolution> {
        if window.remaining_percent() <= 0.0 {
            return None;
        }
        let spec = self.pace_spec(snapshot, slot, window, now)?;
        let mut core_window = cli_rate_window_to_core(window)?;
        if self.uses_inferred_monthly_duration(window)
            && let Some(resets_at) = core_window.resets_at
            && let Some(inferred_minutes) =
                ProviderPaceCapability::inferred_monthly_window_minutes(resets_at)
        {
            core_window.window_minutes = Some(inferred_minutes);
        }
        let tz = TimeZone::system();
        let work_days_arg = match spec.kind {
            ProviderPaceKind::Session => None,
            ProviderPaceKind::Weekly => weekly_work_days,
        };
        let pace = UsagePace::weekly(
            &core_window,
            now,
            Some(spec.default_window_minutes),
            work_days_arg,
            &tz,
        )?;
        Some(ProviderPaceResolution {
            kind: spec.kind,
            pace,
        })
    }

    fn uses_inferred_monthly_duration(self, window: &CliRateWindow) -> bool {
        matches!(self, Self::Claude) && window.window_minutes == Some(43_200)
    }

    fn pace_spec(
        self,
        snapshot: &CliUsageSnapshot,
        slot: ProviderPaceSlot,
        window: &CliRateWindow,
        now: Timestamp,
    ) -> Option<ProviderPaceSpec> {
        let _ = (snapshot, now);
        if let Some(reset_spec) = self.reset_window_pace_spec(window) {
            return Some(reset_spec);
        }
        match slot {
            ProviderPaceSlot::ResetWindow | ProviderPaceSlot::Extra => None,
            ProviderPaceSlot::Primary => self
                .slot_pace_spec(SlotRule::Primary, window)
                .or_else(|| self.inferred_monthly_pace_spec(window)),
            ProviderPaceSlot::Secondary => self
                .slot_pace_spec(SlotRule::Secondary, window)
                .or_else(|| self.inferred_monthly_pace_spec(window)),
            ProviderPaceSlot::Tertiary => self
                .slot_pace_spec(SlotRule::Tertiary, window)
                .or_else(|| self.inferred_monthly_pace_spec(window)),
        }
    }

    fn reset_window_pace_spec(self, window: &CliRateWindow) -> Option<ProviderPaceSpec> {
        if matches!(self, Self::Claude) && window.window_minutes == Some(43_200) {
            Some(ProviderPaceSpec {
                kind: ProviderPaceKind::Weekly,
                default_window_minutes: 43_200,
            })
        } else {
            None
        }
    }

    fn inferred_monthly_pace_spec(self, window: &CliRateWindow) -> Option<ProviderPaceSpec> {
        if self.uses_inferred_monthly_duration(window) {
            Some(ProviderPaceSpec {
                kind: ProviderPaceKind::Weekly,
                default_window_minutes: 43_200,
            })
        } else {
            None
        }
    }

    fn slot_pace_spec(self, slot: SlotRule, window: &CliRateWindow) -> Option<ProviderPaceSpec> {
        match (self, slot) {
            (Self::Codex | Self::Claude, SlotRule::Primary) => {
                session_pace_spec(window, 300, false)
            }
            (Self::Codex | Self::Claude, SlotRule::Secondary) => Some(weekly_pace_spec(window)),
            (Self::Claude, SlotRule::Tertiary) => Some(weekly_pace_spec(window)),
            _ => None,
        }
    }

    #[must_use]
    pub fn cost_presentation(self, snapshot: &CliUsageSnapshot) -> ProviderCostPresentation {
        match self {
            Self::Codex => {
                let Some(cost) = snapshot.provider_cost.as_ref().filter(|c| {
                    c.currency_code
                        .eq_ignore_ascii_case(CliProviderCostSnapshot::CODEX_CREDITS_CURRENCY_CODE)
                }) else {
                    return ProviderCostPresentation::default();
                };
                let mut cli_lines = Vec::new();
                if let Some(balance) = cost.balance {
                    cli_lines.push(ProviderCliCostLine {
                        label: "Extra usage balance".to_string(),
                        value: format_cost_amount(balance, &cost.currency_code),
                    });
                }
                let shows_generic_fallback = !(cost.used == 0.0 && cost.limit == 0.0);
                ProviderCostPresentation {
                    cli_lines,
                    shows_generic_fallback,
                    replaced_detail_rows: HashMap::new(),
                }
            }
            Self::Claude => {
                let mut cli_lines = Vec::new();
                if let Some(cost) = snapshot.provider_cost.as_ref()
                    && let Some(balance) = cost.balance
                {
                    cli_lines.push(ProviderCliCostLine {
                        label: "Extra usage balance".to_string(),
                        value: format_cost_amount(balance, &cost.currency_code),
                    });
                }
                let shows_generic_fallback = snapshot.provider_cost.as_ref().is_none_or(|cost| {
                    !(cost.used == 0.0 && cost.limit == 0.0 && cost.balance.is_some())
                });
                ProviderCostPresentation {
                    cli_lines,
                    shows_generic_fallback,
                    replaced_detail_rows: HashMap::new(),
                }
            }
            Self::Antigravity => ProviderCostPresentation::default(),
        }
    }

    #[must_use]
    pub fn identity_presentation(
        self,
        snapshot: &CliUsageSnapshot,
    ) -> ProviderIdentityPresentation {
        let raw_plan = snapshot
            .login_method_for(self.raw_value())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match self {
            Self::Codex => {
                let Some(raw_plan) = raw_plan else {
                    return ProviderIdentityPresentation::default();
                };
                let display =
                    codex_plan_display_name(raw_plan).unwrap_or_else(|| raw_plan.to_string());
                ProviderIdentityPresentation {
                    plan_text: Some(display.clone()),
                    activity_text: None,
                    badge_text: Some(display),
                }
            }
            Self::Claude => {
                let Some(raw_plan) = raw_plan else {
                    return ProviderIdentityPresentation::default();
                };
                let display = if raw_plan.starts_with("Claude ")
                    && is_claude_compatibility_login_method(raw_plan)
                {
                    raw_plan.to_string()
                } else {
                    swift_capitalized(raw_plan)
                };
                ProviderIdentityPresentation {
                    plan_text: Some(display.clone()),
                    activity_text: None,
                    badge_text: Some(display),
                }
            }
            Self::Antigravity => {
                let Some(raw_plan) = raw_plan else {
                    return ProviderIdentityPresentation::default();
                };
                let display = swift_capitalized(raw_plan);
                ProviderIdentityPresentation {
                    plan_text: Some(display.clone()),
                    activity_text: None,
                    badge_text: Some(display),
                }
            }
        }
    }

    #[must_use]
    pub const fn plan_row_presentation(self) -> ProviderPlanRowPresentation {
        let _ = self;
        ProviderPlanRowPresentation {
            label: "Plan",
            strips_balance_prefix: false,
        }
    }
}

fn cli_rate_window_to_core(window: &CliRateWindow) -> Option<CoreRateWindow> {
    let resets_at = match window.resets_at.as_deref() {
        Some(raw) => Some(raw.parse::<Timestamp>().ok()?),
        None => None,
    };
    CoreRateWindow::new(
        window.used_percent,
        window.window_minutes,
        resets_at,
        window.reset_description.clone(),
        window.next_regen_percent,
        window.is_synthetic_placeholder,
    )
    .ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotRule {
    Primary,
    Secondary,
    Tertiary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPaceSlot {
    ResetWindow,
    Primary,
    Secondary,
    Tertiary,
    Extra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPaceKind {
    Session,
    Weekly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderPaceSpec {
    pub kind: ProviderPaceKind,
    pub default_window_minutes: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderPaceResolution {
    pub kind: ProviderPaceKind,
    pub pace: UsagePace,
}

fn session_pace_spec(
    window: &CliRateWindow,
    max_window_minutes: i64,
    requires_window_duration: bool,
) -> Option<ProviderPaceSpec> {
    match window.window_minutes {
        Some(minutes) if minutes <= 0 || minutes > max_window_minutes => None,
        None if requires_window_duration => None,
        _ => Some(ProviderPaceSpec {
            kind: ProviderPaceKind::Session,
            default_window_minutes: max_window_minutes,
        }),
    }
}

fn weekly_pace_spec(window: &CliRateWindow) -> ProviderPaceSpec {
    let default_window_minutes = match window.window_minutes {
        Some(minutes) if minutes > 0 => minutes,
        _ => 10_080,
    };
    ProviderPaceSpec {
        kind: ProviderPaceKind::Weekly,
        default_window_minutes,
    }
}

#[must_use]
pub fn clean_plan_name(text: &str) -> String {
    let stripped = crate::format::strip_ansi_codes(text);
    let mut trimmed = stripped.trim_start();
    while let Some(rest) = trimmed.strip_prefix('[') {
        let digits_len = rest.bytes().take_while(u8::is_ascii_digit).count();
        if (1..=3).contains(&digits_len) && rest.as_bytes().get(digits_len) == Some(&b'm') {
            trimmed = rest[digits_len + 1..].trim_start();
        } else {
            break;
        }
    }
    let words: Vec<&str> = trimmed
        .split_whitespace()
        .filter(|word| {
            let lower = word
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_ascii_lowercase();
            !matches!(lower.as_str(), "claude" | "codex" | "account" | "plan")
        })
        .collect();
    let mut cleaned = words.join(" ");
    if cleaned.is_empty() {
        cleaned = stripped.trim().to_string();
    }
    if cleaned.eq_ignore_ascii_case("oauth") {
        return "Ollama".to_string();
    }
    let mut chars = cleaned.chars();
    if let Some(first) = chars.next()
        && first.is_lowercase()
    {
        let mut out = first.to_uppercase().to_string();
        out.push_str(chars.as_str());
        return out;
    }
    cleaned
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRateWindowLabels {
    pub primary: String,
    pub secondary: String,
    pub tertiary: String,
    pub shows_tertiary: bool,
}

#[must_use]
pub fn make_provider_window_labels_payload(
    provider: CliProvider,
    usage: Option<&CliUsageSnapshot>,
    now: Timestamp,
) -> Option<ProviderWindowLabelsPayload> {
    let usage = usage?;
    let labels = provider.rate_window_labels(usage, now);
    let primary = usage.primary.as_ref().map(|_| labels.primary);
    let secondary = usage.secondary.as_ref().map(|_| labels.secondary);
    let tertiary = if labels.shows_tertiary && usage.tertiary.is_some() {
        Some(labels.tertiary)
    } else {
        None
    };
    if primary.is_none() && secondary.is_none() && tertiary.is_none() {
        None
    } else {
        Some(ProviderWindowLabelsPayload {
            primary,
            secondary,
            tertiary,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCliCostLine {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCostPresentation {
    pub cli_lines: Vec<ProviderCliCostLine>,
    pub shows_generic_fallback: bool,
    pub replaced_detail_rows: HashMap<String, HashSet<String>>,
}

impl Default for ProviderCostPresentation {
    fn default() -> Self {
        Self {
            cli_lines: Vec::new(),
            shows_generic_fallback: true,
            replaced_detail_rows: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderIdentityPresentation {
    pub plan_text: Option<String>,
    pub activity_text: Option<String>,
    pub badge_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderPlanRowPresentation {
    pub label: &'static str,
    pub strips_balance_prefix: bool,
}

#[must_use]
pub fn codex_plan_display_name(raw_plan: &str) -> Option<String> {
    let raw = raw_plan.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(exact) = codex_exact_display_name(&raw.to_ascii_lowercase()) {
        return Some(exact.to_string());
    }
    let cleaned = clean_plan_name(raw);
    let candidate = cleaned.trim();
    if candidate.is_empty() {
        return Some(raw.to_string());
    }
    if let Some(exact) = codex_exact_display_name(&candidate.to_ascii_lowercase()) {
        return Some(exact.to_string());
    }
    let parts: Vec<&str> = candidate
        .split(|c: char| matches!(c, '_' | '-') || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return Some(candidate.to_string());
    }
    let formatted = parts
        .into_iter()
        .map(|part| match part.to_ascii_lowercase().as_str() {
            "cbp" => "CBP".to_string(),
            "k12" => "K12".to_string(),
            _ if part.chars().any(char::is_alphabetic) && part == part.to_uppercase() => {
                part.to_string()
            }
            _ => {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) if first.is_lowercase() => {
                        format!("{}{}", first.to_uppercase(), chars.as_str())
                    }
                    _ => part.to_string(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    Some(if formatted.is_empty() {
        candidate.to_string()
    } else {
        formatted
    })
}

fn codex_exact_display_name(lower: &str) -> Option<&'static str> {
    match lower {
        "pro" => Some("Pro 20x"),
        "prolite" | "pro_lite" | "pro-lite" | "pro lite" => Some("Pro 5x"),
        _ => None,
    }
}

fn is_claude_compatibility_login_method(raw_plan: &str) -> bool {
    raw_plan
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .any(|w| matches!(w.as_str(), "max" | "pro" | "team" | "enterprise" | "ultra"))
}

fn swift_capitalized(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at_word_start = true;
    for ch in text.chars() {
        if ch.is_alphabetic() {
            if at_word_start {
                out.extend(ch.to_uppercase());
                at_word_start = false;
            } else {
                out.extend(ch.to_lowercase());
            }
        } else {
            out.push(ch);
            at_word_start = true;
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderSelection {
    Single(CliProvider),
    Both,
    All,
    Custom(Vec<CliProvider>),
}

impl ProviderSelection {
    #[must_use]
    pub fn from_raw(raw: Option<&str>, enabled_providers: &[CliProvider]) -> Option<Self> {
        let Some(raw) = raw else {
            return Some(if enabled_providers.len() == 1 {
                Self::Single(enabled_providers[0])
            } else if enabled_providers == [CliProvider::Codex, CliProvider::Claude] {
                Self::Both
            } else {
                Self::Custom(enabled_providers.to_vec())
            });
        };

        let cleaned = raw.trim().to_ascii_lowercase();
        match cleaned.as_str() {
            "both" => Some(Self::Both),
            "all" => Some(Self::All),
            _ => CliProvider::from_cli_name(&cleaned).map(Self::Single),
        }
    }

    #[must_use]
    pub fn as_list(&self, all_providers: &[CliProvider]) -> Vec<CliProvider> {
        match self {
            Self::Single(p) => vec![*p],
            Self::Both => vec![CliProvider::Codex, CliProvider::Claude],
            Self::All => all_providers.to_vec(),
            Self::Custom(providers) => providers.clone(),
        }
    }
}

#[must_use]
pub fn resolved_account_text(
    provider: CliProvider,
    snapshot: &CliUsageSnapshot,
    fallback_account_email: Option<&str>,
) -> Option<String> {
    if let Some(email) = snapshot
        .account_email_for(provider.raw_value())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(email.to_string());
    }
    if provider.uses_account_fallback()
        && let Some(fallback) = fallback_account_email
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        return Some(fallback.to_string());
    }
    None
}

#[must_use]
pub fn make_generic_cost_line_value(cost: &CliProviderCostSnapshot) -> String {
    format!("{:.1} / {:.1}", cost.used, cost.limit)
}
