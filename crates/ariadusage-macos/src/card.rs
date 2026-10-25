// Ported from CodexBar Sources/CodexBar/MenuCardView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuCardView+ModelHelpers.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuCardView+Costs.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuCardView+SessionEquivalent.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuCardView+CodexResetCredits.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/InlineUsageDashboardContent.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/RateWindow+BindingCap.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! The usage menu card's model: what the card shows for one provider.
//!
//! [`card_model`] follows `UsageMenuCardView.Model.make` for the parts the
//! snapshot carries: the header, the subtitle, one row per rate window, and
//! the credits, limit-reset credits, inline usage dashboard, cloud credits,
//! and extra usage sections. Window titles and the accent color are inputs, so
//! the engine's presentation block supplies them; [`card_presentation`]
//! is the one place that maps a snapshot to them.

use std::collections::BTreeMap;

use ariadusage_protocol::metric::{Metric, MetricState};
use ariadusage_protocol::snapshot::ProviderSnapshot;
use ariadusage_protocol::usage::{
    Cost, CostDailyEntry, Credits, Pace, PaceStage, RateWindow, ResetCredits, SubscriptionDates,
};
use jiff::Timestamp;

use crate::chart::MiniBarPoint;
use crate::format::{self, DateTexts, ResetTimeDisplayStyle};
use crate::pace_text::{self, PaceContext, PaceFacts};
use crate::progress_bar::{self, BarSpec, WorkdayTickAppearance};

/// Titles of the positional windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardTitles {
    pub session: String,
    pub weekly: String,
    /// The tertiary window's title; without one the tertiary row is hidden,
    /// as CodexBar does for providers that do not support it.
    pub tertiary: Option<String>,
}

impl Default for CardTitles {
    /// CodexBar's default `sessionLabel` and `weeklyLabel`.
    fn default() -> Self {
        Self {
            session: "Session".to_string(),
            weekly: "Weekly".to_string(),
            tertiary: None,
        }
    }
}

/// An sRGB color with 8-bit channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    /// Parses `#RRGGBB`.
    pub fn from_hex(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#')?;
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Self {
            r: channel(0)?,
            g: channel(2)?,
            b: channel(4)?,
        })
    }
}

/// The progress fill color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Accent {
    /// The provider's brand color.
    Brand(Rgb),
    /// `NSColor.controlAccentColor`, used until the engine sends a color.
    #[default]
    System,
}

/// Titles and accent for one provider's card.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CardPresentation {
    pub titles: CardTitles,
    pub accent: Accent,
}

/// Titles and accent for `provider` from the engine's presentation block.
///
/// Without a block the card falls back to "Session", "Weekly", no tertiary
/// row and the system accent; a block without an accent color also uses
/// the system accent.
pub fn card_presentation(provider: &ProviderSnapshot) -> CardPresentation {
    let Some(presentation) = &provider.presentation else {
        return CardPresentation::default();
    };
    CardPresentation {
        titles: CardTitles {
            session: presentation.session_label.clone(),
            weekly: presentation.weekly_label.clone(),
            tertiary: presentation.tertiary_label.clone(),
        },
        accent: presentation
            .accent_color
            .as_deref()
            .and_then(Rgb::from_hex)
            .map_or(Accent::System, Accent::Brand),
    }
}

/// Display preferences that shape the card, with CodexBar's defaults
/// (`SettingsStore.swift`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardPreferences {
    /// `usageBarsShowUsed`, default false.
    pub show_used: bool,
    /// `resetTimesShowAbsolute`, default false.
    pub reset_times_show_absolute: bool,
    /// `paceVisible`, default true.
    pub pace_visible: bool,
    /// `showOptionalCreditsAndExtraUsage`, default true.
    pub show_optional_credits_and_extra_usage: bool,
    /// `claudeDailyRoutinesUsageVisible`, default true.
    pub claude_daily_routines_usage_visible: bool,
    /// `weeklyProgressWorkDays`, unset by default.
    pub work_days_per_week: Option<i32>,
    /// `workdayTickAppearance`, default subtle.
    pub workday_tick_appearance: WorkdayTickAppearance,
    /// Remaining-percent warning thresholds of enabled session warnings;
    /// empty while quota warnings are off, which is the default.
    pub session_warning_thresholds: Vec<i32>,
    /// The same for weekly warnings.
    pub weekly_warning_thresholds: Vec<i32>,
}

impl Default for CardPreferences {
    fn default() -> Self {
        Self {
            show_used: false,
            reset_times_show_absolute: false,
            pace_visible: true,
            show_optional_credits_and_extra_usage: true,
            claude_daily_routines_usage_visible: true,
            work_days_per_week: None,
            workday_tick_appearance: WorkdayTickAppearance::Subtle,
            session_warning_thresholds: Vec::new(),
            weekly_warning_thresholds: Vec::new(),
        }
    }
}

impl CardPreferences {
    fn reset_style(&self) -> ResetTimeDisplayStyle {
        if self.reset_times_show_absolute {
            ResetTimeDisplayStyle::Absolute
        } else {
            ResetTimeDisplayStyle::Countdown
        }
    }
}

/// Whether percentages count what is left or what is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PercentStyle {
    Left,
    Used,
}

impl PercentStyle {
    pub fn label_suffix(self) -> &'static str {
        match self {
            PercentStyle::Left => format::SUFFIX_LEFT,
            PercentStyle::Used => format::SUFFIX_USED,
        }
    }

    pub fn accessibility_label(self) -> &'static str {
        match self {
            PercentStyle::Left => "Usage remaining",
            PercentStyle::Used => "Usage used",
        }
    }
}

/// One rate-window row.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricRow {
    pub id: String,
    pub title: String,
    pub percent: f64,
    pub percent_style: PercentStyle,
    /// Replaces the bar with a status line when set.
    pub status_text: Option<String>,
    pub reset_text: Option<String>,
    pub detail_text: Option<String>,
    pub detail_left_text: Option<String>,
    pub detail_right_text: Option<String>,
    pub session_equivalent_left_text: Option<String>,
    pub session_equivalent_right_text: Option<String>,
    pub pace_percent: Option<f64>,
    pub detail_is_pace_derived: bool,
    pub pace_on_top: bool,
    pub warning_marker_percents: Vec<f64>,
    pub workday_marker_percents: Vec<f64>,
    pub workday_tick_appearance: WorkdayTickAppearance,
}

impl MetricRow {
    fn new(id: &str, title: String, percent: f64, percent_style: PercentStyle) -> Self {
        Self {
            id: id.to_string(),
            title,
            percent,
            percent_style,
            status_text: None,
            reset_text: None,
            detail_text: None,
            detail_left_text: None,
            detail_right_text: None,
            session_equivalent_left_text: None,
            session_equivalent_right_text: None,
            pace_percent: None,
            detail_is_pace_derived: false,
            pace_on_top: true,
            warning_marker_percents: Vec::new(),
            workday_marker_percents: Vec::new(),
            workday_tick_appearance: WorkdayTickAppearance::Subtle,
        }
    }

    /// `58% left`.
    pub fn percent_label(&self) -> String {
        format::percent_text(self.percent, self.percent_style.label_suffix())
    }

    /// `Session 58% left`.
    pub fn title_text(&self) -> String {
        format!("{} {}", self.title, self.percent_label())
    }

    /// The pace, detail, and session-equivalent forecast segments joined with ` · `.
    pub fn meta_text(&self) -> Option<String> {
        let parts: Vec<&str> = [
            &self.detail_left_text,
            &self.detail_right_text,
            &self.session_equivalent_left_text,
            &self.session_equivalent_right_text,
        ]
        .into_iter()
        .filter_map(|t| t.as_deref().map(str::trim))
        .filter(|t| !t.is_empty())
        .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// The bar this row draws.
    pub fn bar(&self) -> BarSpec {
        BarSpec {
            percent: self.percent,
            pace_percent: self.pace_percent,
            pace_on_top: self.pace_on_top,
            warning_marker_percents: self.warning_marker_percents.clone(),
            workday_marker_percents: self.workday_marker_percents.clone(),
            workday_appearance: self.workday_tick_appearance,
        }
    }
}

/// How the subtitle is colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleStyle {
    Info,
    Loading,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subtitle {
    pub text: String,
    pub style: SubtitleStyle,
}

/// The credits section (`UsageMenuCardCreditsSectionView`).
#[derive(Debug, Clone, PartialEq)]
pub struct CreditsSection {
    /// `N left`, or the credits error.
    pub text: String,
    pub show_progress: bool,
    pub progress_percent: Option<f64>,
    /// `N tokens`, the scale of the bar.
    pub scale_text: Option<String>,
    /// Optional hint line below the bar (`MenuCardView+Costs.swift:71-118`).
    pub hint_text: Option<String>,
}

/// Codex Limit Reset Credits (`LimitResetCreditsContent`, `MenuCardView+CodexResetCredits.swift:105-140`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitResetCreditsSection {
    pub count_text: String,
    pub expiry_text: Option<String>,
}

/// KPI value font emphasis (`InlineUsageDashboardContent.swift:775-808`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KpiEmphasis {
    Primary,
    Secondary,
}

/// One cell in the 2-column KPI grid (`KPIBlock`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KpiItem {
    pub title: String,
    pub value: String,
    pub emphasis: KpiEmphasis,
}

/// One row under `Recent windows` (`InlineUsageDashboardContent.swift:738-766`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowComparisonItem {
    pub title: String,
    pub value: String,
    pub range: String,
    pub note: Option<String>,
}

/// In-card inline token/cost dashboard (`InlineUsageDashboardContent`).
#[derive(Debug, Clone, PartialEq)]
pub struct InlineUsageDashboardSection {
    pub kpis: Vec<KpiItem>,
    pub max_bar_label: Option<String>,
    pub points: Vec<MiniBarPoint>,
    pub recent_windows: Vec<WindowComparisonItem>,
    pub detail_lines: Vec<String>,
}

/// How a `ProviderCostSection` renders (`MenuCardView+Costs.swift:44-69`).
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderCostPresentation {
    /// Single-row `title` + `value` (`InlineBalanceCostRow`).
    InlineBalance { value: String },
    /// Full extra-usage block (`ProviderCostDetailContent`).
    Detail {
        percent_used: Option<f64>,
        percent_style: PercentStyle,
        spend_line: String,
        percent_line: Option<String>,
        balance_line: Option<String>,
        personal_spend_line: Option<String>,
    },
}

/// Provider cost or cloud credits section (`ProviderCostSection`).
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderCostSection {
    pub title: String,
    pub presentation: ProviderCostPresentation,
}

/// Everything the card shows.
#[derive(Debug, Clone, PartialEq)]
pub struct CardModel {
    pub provider_name: String,
    /// The account email, or empty. Never log it.
    pub email: String,
    pub subtitle: Subtitle,
    pub plan: Option<String>,
    pub shows_submenu_indicator: bool,
    pub metrics: Vec<MetricRow>,
    pub usage_notes: Vec<String>,
    pub placeholder: Option<String>,
    pub limit_reset_credits: Option<LimitResetCreditsSection>,
    pub inline_usage_dashboard: Option<InlineUsageDashboardSection>,
    pub cloud_credits: Option<ProviderCostSection>,
    pub credits: Option<CreditsSection>,
    pub provider_cost: Option<ProviderCostSection>,
    pub accent: Accent,
}

impl Default for CardModel {
    fn default() -> Self {
        Self {
            provider_name: String::new(),
            email: String::new(),
            subtitle: Subtitle {
                text: String::new(),
                style: SubtitleStyle::Info,
            },
            plan: None,
            shows_submenu_indicator: false,
            metrics: Vec::new(),
            usage_notes: Vec::new(),
            placeholder: None,
            limit_reset_credits: None,
            inline_usage_dashboard: None,
            cloud_credits: None,
            credits: None,
            provider_cost: None,
            accent: Accent::System,
        }
    }
}

impl CardModel {
    pub fn has_usage_content(&self) -> bool {
        !self.metrics.is_empty()
            || !self.usage_notes.is_empty()
            || self.limit_reset_credits.is_some()
            || self.inline_usage_dashboard.is_some()
            || self.cloud_credits.is_some()
    }

    /// Rows, credits or cost stack below the header in sections.
    pub fn uses_stacked_detail_layout(&self) -> bool {
        self.has_usage_content() || self.credits.is_some() || self.provider_cost.is_some()
    }

    /// Whether anything follows the header.
    pub fn has_details(&self) -> bool {
        self.placeholder.is_some() || self.uses_stacked_detail_layout()
    }
}

/// The card for `provider` with its presentation from [`card_presentation`].
pub fn provider_card(
    provider: &ProviderSnapshot,
    prefs: &CardPreferences,
    now: Timestamp,
    dates: &dyn DateTexts,
) -> CardModel {
    card_model(provider, &card_presentation(provider), prefs, now, dates)
}

/// Builds the card for one provider.
pub fn card_model(
    provider: &ProviderSnapshot,
    presentation: &CardPresentation,
    prefs: &CardPreferences,
    now: Timestamp,
    dates: &dyn DateTexts,
) -> CardModel {
    let provider_id = provider.id.as_str();
    let percent_style = if prefs.show_used {
        PercentStyle::Used
    } else {
        PercentStyle::Left
    };
    let builder = RowBuilder {
        provider_id,
        prefs,
        now,
        dates,
        percent_style,
    };
    let titles = &presentation.titles;
    let windows = &provider.windows;

    let mut metrics = Vec::new();
    let secondary_value = windows.secondary.as_ref().and_then(displayable_value);
    if let Some(primary) = windows.primary.as_ref().and_then(displayable_value) {
        metrics.push(builder.primary(primary, secondary_value, titles.session.clone()));
    }
    if let Some(weekly) = windows.secondary.as_ref().and_then(displayable_value) {
        metrics.push(builder.secondary(weekly, titles.weekly.clone()));
    }
    if let (Some(title), Some(tertiary)) = (
        titles.tertiary.as_ref(),
        windows.tertiary.as_ref().and_then(displayable_value),
    ) {
        metrics.push(builder.tertiary(tertiary, title.clone()));
    }
    metrics.extend(
        windows
            .extra
            .iter()
            .filter_map(|named| builder.extra(&named.id, &named.title, &named.window)),
    );
    if !prefs.pace_visible {
        for metric in &mut metrics {
            gate_pace(metric);
        }
    }

    let last_error = provider
        .last_error
        .as_ref()
        .map(|e| e.message.trim())
        .filter(|m| !m.is_empty());
    let refreshing = is_refreshing(provider);
    let has_snapshot = provider.updated_at.is_some() || has_any_value(provider);
    let usage_notes = subscription_notes(provider, dates);
    let limit_reset_credits = limit_reset_credits_section(provider, prefs, now, dates);
    let inline_usage_dashboard = inline_usage_dashboard_section(provider, dates);
    let cloud_credits = cloud_credits_section(provider, prefs);
    let credits = (prefs.show_optional_credits_and_extra_usage
        && supports_credits_section(provider_id))
    .then(|| provider.credits.as_ref().and_then(credits_section))
    .flatten();
    let provider_cost = prefs
        .show_optional_credits_and_extra_usage
        .then(|| {
            provider
                .cost
                .as_ref()
                .and_then(displayable_value)
                .and_then(|cost| provider_cost_section(provider_id, cost, percent_style))
        })
        .flatten();
    let shows_submenu_indicator = inline_usage_dashboard.is_some()
        && provider
            .cost_history
            .as_ref()
            .and_then(displayable_value)
            .is_some_and(|history| !history.daily.is_empty());

    CardModel {
        provider_name: provider.display_name.clone(),
        email: account_email(provider),
        subtitle: subtitle(provider, last_error, refreshing, now, dates),
        plan: plan_text(provider),
        shows_submenu_indicator,
        metrics,
        usage_notes,
        placeholder: (!has_snapshot && !refreshing && last_error.is_none())
            .then(|| "No usage yet".to_string()),
        limit_reset_credits,
        inline_usage_dashboard,
        cloud_credits,
        credits,
        provider_cost,
        accent: presentation.accent,
    }
}

struct RowBuilder<'a> {
    provider_id: &'a str,
    prefs: &'a CardPreferences,
    now: Timestamp,
    dates: &'a dyn DateTexts,
    percent_style: PercentStyle,
}

impl RowBuilder<'_> {
    fn percent(&self, window: &RateWindow) -> f64 {
        let used = window.used_percent;
        clamped(if self.prefs.show_used {
            used
        } else {
            100.0 - used
        })
    }

    fn reset_text(&self, window: &RateWindow) -> Option<String> {
        format::reset_line(window, self.prefs.reset_style(), self.now, self.dates)
    }

    fn row(&self, id: &str, title: String, window: &RateWindow) -> MetricRow {
        let mut row = MetricRow::new(id, title, self.percent(window), self.percent_style);
        row.reset_text = self.reset_text(window);
        row
    }

    /// The session row, with Claude binding-quota projection (`MenuCardView.swift:1327-1346`)
    /// and the engine's session or reset-window pace.
    fn primary(
        &self,
        window: &RateWindow,
        secondary_window: Option<&RateWindow>,
        title: String,
    ) -> MetricRow {
        let (effective_window, capped_by_binding_quota) = if self.provider_id == "claude" {
            claude_binding_quota_projection(window, secondary_window, self.now)
        } else {
            (window.clone(), false)
        };
        let mut row = self.row("primary", title, &effective_window);
        if !capped_by_binding_quota
            && let Some(pace) = effective_window.pace.as_ref()
            && let Some(context) =
                primary_pace_context(self.provider_id, &effective_window, self.now)
        {
            self.apply_pace(&mut row, &effective_window, pace, context);
        }
        self.apply_session_equivalent(&mut row, &effective_window);
        row.warning_marker_percents = progress_bar::warning_marker_percents(
            &self.prefs.session_warning_thresholds,
            self.prefs.show_used,
        );
        row
    }

    /// The weekly row, with the engine's weekly pace (`PaceContext::Weekly`) and
    /// session-equivalent forecast when present.
    fn secondary(&self, window: &RateWindow, title: String) -> MetricRow {
        let mut row = self.row("secondary", title, window);
        if let Some(pace) = window.pace.as_ref()
            && let Some(context) = secondary_or_tertiary_pace_context(self.provider_id, window)
        {
            self.apply_pace(&mut row, window, pace, context);
        }
        self.apply_session_equivalent(&mut row, window);
        row.warning_marker_percents = progress_bar::warning_marker_percents(
            &self.prefs.weekly_warning_thresholds,
            self.prefs.show_used,
        );
        row.workday_tick_appearance = self.prefs.workday_tick_appearance;
        if self.prefs.workday_tick_appearance != WorkdayTickAppearance::Hidden {
            row.workday_marker_percents = progress_bar::work_day_marker_percents(
                self.prefs.work_days_per_week,
                window.window_minutes,
            );
        }
        row
    }

    fn tertiary(&self, window: &RateWindow, title: String) -> MetricRow {
        let mut row = self.row("tertiary", title, window);
        if let Some(pace) = window.pace.as_ref()
            && let Some(context) = secondary_or_tertiary_pace_context(self.provider_id, window)
        {
            self.apply_pace(&mut row, window, pace, context);
        }
        self.apply_session_equivalent(&mut row, window);
        row.warning_marker_percents = progress_bar::warning_marker_percents(
            &self.prefs.weekly_warning_thresholds,
            self.prefs.show_used,
        );
        row
    }

    /// A named extra window. Unknown or failed usage shows `Unavailable`
    /// (with `- <reset>` when the metric carries a window with a reset);
    /// a window still loading has no row yet.
    fn extra(&self, id: &str, title: &str, metric: &Metric<RateWindow>) -> Option<MetricRow> {
        if self.provider_id == "codex" && !self.prefs.show_optional_credits_and_extra_usage {
            return None;
        }
        if self.provider_id == "claude"
            && id == "claude-routines"
            && (!self.prefs.show_optional_credits_and_extra_usage
                || !self.prefs.claude_daily_routines_usage_visible)
        {
            return None;
        }
        let title = extra_title(self.provider_id, id, title);
        match (
            metric.state,
            displayable_value(metric).filter(|w| w.used_percent.is_finite()),
        ) {
            (_, Some(window)) => {
                let mut row = self.row(id, title, window);
                if let Some(pace) = window.pace.as_ref()
                    && let Some(context) = extra_pace_context(self.provider_id, window)
                {
                    self.apply_pace(&mut row, window, pace, context);
                }
                self.apply_session_equivalent(&mut row, window);
                Some(row)
            }
            (MetricState::Loading, None) => None,
            (_, None) => {
                let mut row = MetricRow::new(id, title, 0.0, self.percent_style);
                let status = match metric.value.as_ref().and_then(|w| self.reset_text(w)) {
                    Some(reset) => format!("Unavailable - {reset}"),
                    None => "Unavailable".to_string(),
                };
                row.status_text = Some(status);
                Some(row)
            }
        }
    }

    /// `weeklyPaceDetail` / `sessionPaceDetail` and `paceDetail`.
    fn apply_pace(
        &self,
        row: &mut MetricRow,
        window: &RateWindow,
        pace: &Pace,
        context: PaceContext,
    ) {
        if (100.0 - window.used_percent).max(0.0) <= 0.0 {
            return;
        }
        if !pace.expected_used_percent.is_finite() || !window.used_percent.is_finite() {
            return;
        }
        match context {
            PaceContext::Session => {
                if !pace_text::supports_session_pace(self.provider_id, window)
                    || pace.expected_used_percent < pace_text::MINIMUM_EXPECTED_PERCENT
                {
                    return;
                }
            }
            PaceContext::Weekly => {
                if pace.expected_used_percent < pace_text::MINIMUM_EXPECTED_PERCENT
                    && pace.eta_seconds != Some(0)
                {
                    return;
                }
            }
        }
        let facts = PaceFacts {
            stage: pace.stage,
            delta_percent: pace.delta_percent,
            expected_used_percent: pace.expected_used_percent,
            will_last_to_reset: pace.will_last_to_reset,
            eta_seconds: pace.eta_seconds.map(|s| s as f64),
            run_out_probability: pace.run_out_probability,
            speed_multiplier_to_reset: pace.speed_multiplier_to_reset,
        };
        let detail = pace_text::detail(
            &facts,
            pace_text::shows_headroom_hint(self.provider_id),
            context,
        );
        let expected = if self.prefs.show_used {
            pace.expected_used_percent
        } else {
            100.0 - pace.expected_used_percent
        };
        row.detail_left_text = Some(detail.left);
        row.detail_right_text = detail.right;
        row.pace_percent = match pace.stage {
            PaceStage::OnTrack | PaceStage::Unknown => None,
            _ => Some(expected),
        };
        row.pace_on_top = window.used_percent <= pace.expected_used_percent;
        row.detail_is_pace_derived = true;
    }

    fn apply_session_equivalent(&self, row: &mut MetricRow, window: &RateWindow) {
        let Some(forecast) = window.session_equivalent.as_ref() else {
            return;
        };
        let detail = pace_text::session_equivalent_detail(forecast, self.dates);
        row.session_equivalent_left_text = Some(detail.left);
        row.session_equivalent_right_text = Some(detail.right);
    }
}

/// Resolves the pace context for the primary lane (`MenuCardView.swift:599-630`).
fn primary_pace_context(
    provider_id: &str,
    window: &RateWindow,
    now: Timestamp,
) -> Option<PaceContext> {
    match provider_id {
        "claude" => {
            if window.window_minutes == Some(43_200) {
                if window.resets_at.is_none_or(|resets_at| resets_at > now) {
                    Some(PaceContext::Weekly)
                } else {
                    None
                }
            } else {
                Some(PaceContext::Session)
            }
        }
        "codex" => {
            pace_text::supports_session_pace(provider_id, window).then_some(PaceContext::Session)
        }
        "antigravity" => match window.window_minutes {
            Some(300) => Some(PaceContext::Session),
            Some(10_080) => Some(PaceContext::Weekly),
            _ => None,
        },
        _ => None,
    }
}

/// Resolves the pace context for the secondary or tertiary lane (`MenuCardView.swift:618-630`, `MenuCardView+ModelHelpers.swift:1145`).
fn secondary_or_tertiary_pace_context(
    provider_id: &str,
    window: &RateWindow,
) -> Option<PaceContext> {
    match provider_id {
        "antigravity" => match window.window_minutes {
            Some(300) => Some(PaceContext::Session),
            Some(10_080) => Some(PaceContext::Weekly),
            _ => None,
        },
        "codex" => (window.window_minutes.is_none() || window.window_minutes == Some(10_080))
            .then_some(PaceContext::Weekly),
        "claude" => (window.window_minutes.is_none()
            || matches!(window.window_minutes, Some(10_080 | 43_200)))
        .then_some(PaceContext::Weekly),
        _ => None,
    }
}

/// Resolves the pace context for a named extra rate window (`extraRateWindowPaceDetail`, `MenuCardView+ModelHelpers.swift:1125-1160`).
fn extra_pace_context(provider_id: &str, window: &RateWindow) -> Option<PaceContext> {
    match (provider_id, window.window_minutes) {
        ("claude", Some(10_080 | 43_200)) => Some(PaceContext::Weekly),
        ("codex" | "antigravity", Some(300)) => Some(PaceContext::Session),
        ("codex" | "antigravity", Some(10_080)) => Some(PaceContext::Weekly),
        _ => None,
    }
}

/// Returns `(effective_primary, is_capped_by_binding_quota)` for Claude's primary lane
/// (`RateWindow+BindingCap.swift:21-65`, `MenuCardView.swift:1327-1346`).
fn claude_binding_quota_projection(
    primary: &RateWindow,
    secondary: Option<&RateWindow>,
    now: Timestamp,
) -> (RateWindow, bool) {
    let Some(binding) = secondary else {
        return (primary.clone(), false);
    };
    if !is_binding_quota_over(binding, primary, now) {
        return (primary.clone(), false);
    }
    let (resets_at, reset_description) = if is_actively_exhausted(primary, now) {
        match (primary.resets_at, binding.resets_at) {
            (Some(pr), Some(br)) => {
                if br > pr {
                    (Some(br), binding.reset_description.clone())
                } else {
                    (Some(pr), primary.reset_description.clone())
                }
            }
            (None, Some(br)) => (Some(br), binding.reset_description.clone()),
            (Some(pr), None) => (Some(pr), primary.reset_description.clone()),
            (None, None) => (
                None,
                binding
                    .reset_description
                    .clone()
                    .or_else(|| primary.reset_description.clone()),
            ),
        }
    } else {
        (binding.resets_at, binding.reset_description.clone())
    };
    let mut capped = primary.clone();
    capped.used_percent = 100.0;
    capped.resets_at = resets_at;
    capped.reset_description = reset_description;
    capped.pace = None;
    (capped, true)
}

fn is_actively_exhausted(window: &RateWindow, now: Timestamp) -> bool {
    if (100.0 - window.used_percent).max(0.0) > 0.0 {
        return false;
    }
    window.resets_at.is_none_or(|resets_at| resets_at > now)
}

fn is_binding_quota_over(binding: &RateWindow, candidate: &RateWindow, now: Timestamp) -> bool {
    if !is_actively_exhausted(binding, now) {
        return false;
    }
    match (binding.window_minutes, candidate.window_minutes) {
        (Some(bw), Some(cw)) => bw > cw,
        _ => match (binding.resets_at, candidate.resets_at) {
            (Some(br), Some(cr)) => br > cr,
            (Some(br), None) => br > now,
            _ => true,
        },
    }
}

/// Claude's model-scoped weekly carve-out carries a canonical `<Model> only`
/// title outside the menu and renders as `<Model> weekly` inside the card
/// (`MenuCardView+ModelHelpers.swift:1009-1012`).
fn extra_title(provider_id: &str, id: &str, title: &str) -> String {
    if provider_id == "claude" && id.starts_with("claude-weekly-scoped-") {
        format!("{} weekly", strip_trailing_only(title))
    } else {
        title.to_string()
    }
}

/// Strips a trailing case-insensitive `\s+only\s*$`.
fn strip_trailing_only(title: &str) -> &str {
    let trimmed = title.trim_end();
    if trimmed.len() >= 4 {
        let split_at = trimmed.len() - 4;
        if trimmed.is_char_boundary(split_at) && trimmed[split_at..].eq_ignore_ascii_case("only") {
            let before = &trimmed[..split_at];
            let stripped = before.trim_end();
            if stripped.len() < before.len() {
                return stripped;
            }
        }
    }
    title
}

/// Clears pace output when the user hides pace (`paceGatedMetrics`).
fn gate_pace(metric: &mut MetricRow) {
    if metric.detail_is_pace_derived {
        metric.detail_left_text = None;
        metric.detail_right_text = None;
    }
    metric.session_equivalent_left_text = None;
    metric.session_equivalent_right_text = None;
    metric.pace_percent = None;
}

/// `subscriptionNotes` (`MenuCardView+ModelHelpers.swift:720-752`).
fn subscription_notes(provider: &ProviderSnapshot, dates: &dyn DateTexts) -> Vec<String> {
    let Some(SubscriptionDates { renews, expires }) =
        provider.subscription.as_ref().and_then(displayable_value)
    else {
        return Vec::new();
    };
    let mut notes = Vec::new();
    if let Some(billing) = renews {
        let date_text = dates.month_day_year(billing.at, billing.date_only);
        notes.push(format!("Plan renews: {date_text}"));
    } else if let Some(billing) = expires {
        let date_text = dates.month_day_year(billing.at, billing.date_only);
        notes.push(format!("Plan expires: {date_text}"));
    }
    notes
}

/// `codexLimitResetCreditsSection` (`MenuCardView+CodexResetCredits.swift:10-42`).
fn limit_reset_credits_section(
    provider: &ProviderSnapshot,
    prefs: &CardPreferences,
    now: Timestamp,
    dates: &dyn DateTexts,
) -> Option<LimitResetCreditsSection> {
    if provider.id.as_str() != "codex" || !prefs.show_optional_credits_and_extra_usage {
        return None;
    }
    let credits = provider.credits.as_ref().and_then(displayable_value)?;
    let ResetCredits {
        available_count,
        next_expires_at,
    } = credits.reset_credits.as_ref()?;
    let total = *available_count;
    if total == 0 {
        return None;
    }
    let count_text = format!("{total} available");
    let expiry_text = next_expires_at.and_then(|expires| {
        let secs = expires.duration_since(now).as_secs_f64();
        if secs <= 0.0 {
            None
        } else {
            match prefs.reset_style() {
                ResetTimeDisplayStyle::Countdown => {
                    let cd = format::reset_countdown_description(secs);
                    Some(cd.strip_prefix("in ").unwrap_or(&cd).to_string())
                }
                ResetTimeDisplayStyle::Absolute => {
                    Some(format::reset_description(expires, now, dates))
                }
            }
        }
    });
    Some(LimitResetCreditsSection {
        count_text,
        expiry_text,
    })
}

#[derive(Debug, Clone)]
struct ComputedQuotaWindow {
    label: String,
    cost_usd: Option<f64>,
    total_tokens: Option<u64>,
    range: String,
    estimated: bool,
}

fn compute_quota_windows(
    provider: &ProviderSnapshot,
    daily: &[CostDailyEntry],
    dates: &dyn DateTexts,
) -> Vec<ComputedQuotaWindow> {
    if daily.is_empty() {
        return Vec::new();
    }
    let pid = provider.id.as_str();
    if pid != "claude" && pid != "codex" {
        return Vec::new();
    }
    let Some(secondary) = provider
        .windows
        .secondary
        .as_ref()
        .and_then(displayable_value)
    else {
        return Vec::new();
    };
    let Some(reset_ts) = secondary.resets_at else {
        return Vec::new();
    };

    let estimated = pid == "codex";
    let labels = [
        "Current window",
        "Previous window",
        "2 windows ago",
        "3 windows ago",
    ];
    let week_secs = 7 * 86_400_i64;
    let mut windows = Vec::new();

    for (idx, label) in labels.into_iter().enumerate() {
        let end_sec = reset_ts.as_second() - (idx as i64) * week_secs;
        let start_sec = end_sec - week_secs;
        let Some(end_ts) = Timestamp::from_second(end_sec).ok() else {
            break;
        };
        let Some(start_ts) = Timestamp::from_second(start_sec).ok() else {
            break;
        };
        let start_date = start_ts.to_zoned(jiff::tz::TimeZone::UTC).date();
        let end_date = end_ts.to_zoned(jiff::tz::TimeZone::UTC).date();

        let mut matched = 0usize;
        let mut sum_cost = 0.0_f64;
        let mut has_cost = false;
        let mut sum_tokens = 0_u64;
        let mut has_tokens = false;

        for entry in daily {
            let Ok(entry_date) = entry.date.parse::<jiff::civil::Date>() else {
                continue;
            };
            let in_window = if idx == 0 {
                entry_date >= start_date && entry_date <= end_date
            } else {
                entry_date >= start_date && entry_date < end_date
            };
            if in_window {
                matched += 1;
                if let Some(c) = entry.cost_usd.filter(|c| c.is_finite() && *c >= 0.0) {
                    sum_cost += c;
                    has_cost = true;
                }
                if let Some(t) = entry.total_tokens {
                    sum_tokens = sum_tokens.saturating_add(t);
                    has_tokens = true;
                }
            }
        }

        if matched == 0 && idx > 0 {
            break;
        }

        let raw_range = dates.quota_window_range(start_ts, end_ts);
        let range = if estimated {
            format!("Est. {raw_range}")
        } else {
            raw_range
        };
        windows.push(ComputedQuotaWindow {
            label: label.to_string(),
            cost_usd: has_cost.then_some((sum_cost * 100.0).round() / 100.0),
            total_tokens: has_tokens.then_some(sum_tokens),
            range,
            estimated,
        });
    }

    windows
}

/// `inlineUsageDashboard` (`InlineUsageDashboardContent.swift:113-405`).
fn inline_usage_dashboard_section(
    provider: &ProviderSnapshot,
    dates: &dyn DateTexts,
) -> Option<InlineUsageDashboardSection> {
    let history = provider.cost_history.as_ref().and_then(displayable_value)?;
    if history.daily.is_empty()
        && history.session_cost_usd.is_none()
        && history.last_30_days_cost_usd.is_none()
    {
        return None;
    }
    let provider_id = provider.id.as_str();
    let is_codex = provider_id == "codex";

    let today_cost = history
        .session_cost_usd
        .map(|v| format::currency_string(v, "USD"))
        .unwrap_or_else(|| "—".to_string());
    let latest_tokens = history
        .session_tokens
        .or_else(|| history.daily.last().and_then(|d| d.total_tokens))
        .and_then(|t| i64::try_from(t).ok())
        .map(format::token_count_string)
        .unwrap_or_else(|| "—".to_string());
    let month_cost = history
        .last_30_days_cost_usd
        .map(|v| format::currency_string(v, "USD"))
        .unwrap_or_else(|| "—".to_string());
    let month_tokens = history
        .last_30_days_tokens
        .and_then(|t| i64::try_from(t).ok())
        .map(format::token_count_string)
        .unwrap_or_else(|| "—".to_string());

    let month_cost_title = if is_codex { "30d" } else { "30d cost" };
    let quota_windows = compute_quota_windows(provider, &history.daily, dates);

    let mut kpis = Vec::new();
    if let Some(current_window) = quota_windows.first() {
        let week_cost_title = if current_window.estimated {
            "Est. Current window"
        } else {
            "Current window"
        };
        let week_token_title = if current_window.estimated {
            "Est. Current window tokens"
        } else {
            "Current window tokens"
        };
        let week_cost_value = current_window
            .cost_usd
            .map(|v| format::currency_string(v, "USD"))
            .unwrap_or_else(|| "—".to_string());
        let week_token_value = current_window
            .total_tokens
            .and_then(|t| i64::try_from(t).ok())
            .map(format::token_count_string)
            .unwrap_or_else(|| "—".to_string());
        kpis.push(KpiItem {
            title: "Today".to_string(),
            value: today_cost,
            emphasis: KpiEmphasis::Primary,
        });
        kpis.push(KpiItem {
            title: week_cost_title.to_string(),
            value: week_cost_value,
            emphasis: KpiEmphasis::Primary,
        });
        kpis.push(KpiItem {
            title: "Latest tokens".to_string(),
            value: latest_tokens,
            emphasis: KpiEmphasis::Secondary,
        });
        kpis.push(KpiItem {
            title: week_token_title.to_string(),
            value: week_token_value,
            emphasis: KpiEmphasis::Secondary,
        });
        kpis.push(KpiItem {
            title: month_cost_title.to_string(),
            value: month_cost,
            emphasis: KpiEmphasis::Secondary,
        });
        kpis.push(KpiItem {
            title: "30d tokens".to_string(),
            value: month_tokens,
            emphasis: KpiEmphasis::Secondary,
        });
    } else {
        kpis.push(KpiItem {
            title: "Today".to_string(),
            value: today_cost,
            emphasis: KpiEmphasis::Primary,
        });
        kpis.push(KpiItem {
            title: month_cost_title.to_string(),
            value: month_cost,
            emphasis: KpiEmphasis::Secondary,
        });
        kpis.push(KpiItem {
            title: "Latest tokens".to_string(),
            value: latest_tokens,
            emphasis: KpiEmphasis::Secondary,
        });
        kpis.push(KpiItem {
            title: "30d tokens".to_string(),
            value: month_tokens,
            emphasis: KpiEmphasis::Secondary,
        });
    }

    let points: Vec<MiniBarPoint> = history
        .daily
        .iter()
        .rev()
        .take(30)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(mini_bar_point)
        .collect();
    let max_bar_label = points
        .iter()
        .filter_map(|p| p.value)
        .fold(0.0_f64, f64::max)
        .pipe_some()
        .filter(|&max| max > 0.0)
        .map(|max| format::compact_currency_string(max, "USD"));

    let recent_windows: Vec<WindowComparisonItem> =
        quota_windows.iter().map(quota_window_comparison).collect();

    let mut detail_lines = Vec::new();
    let top_model = history
        .top_model
        .as_ref()
        .map(|m| m.model_name.clone())
        .or_else(|| top_cost_model(&history.daily));
    if let Some(top_model) = top_model {
        detail_lines.push(format!("Top model: {}", short_model_name(&top_model)));
    }
    if provider_id == "codex" {
        detail_lines.push("Estimated from token usage · not a subscription bill".to_string());
    } else if provider_id == "claude" {
        detail_lines.push(
            "Estimated from local Claude logs at API rates; token totals include cache".to_string(),
        );
    } else if let Some(first_hint) = history.hint_lines.first() {
        detail_lines.push(first_hint.clone());
    }

    Some(InlineUsageDashboardSection {
        kpis,
        max_bar_label,
        points,
        recent_windows,
        detail_lines,
    })
}

trait PipeSome: Sized {
    fn pipe_some(self) -> Option<Self> {
        Some(self)
    }
}
impl PipeSome for f64 {}

fn mini_bar_point(entry: &CostDailyEntry) -> MiniBarPoint {
    let date_label = format::short_day_key_label(&entry.date);
    let cost = entry.cost_usd.filter(|&v| v.is_finite() && v >= 0.0);
    let tokens = entry.total_tokens;
    let hover_summary = match (
        cost.map(|v| format::currency_string(v, "USD")),
        tokens
            .and_then(|t| i64::try_from(t).ok())
            .map(format::token_count_string),
    ) {
        (Some(c), Some(t)) => Some(format!("{date_label}: {c} · {t} tokens")),
        (Some(c), None) => Some(format!("{date_label}: {c}")),
        (None, Some(t)) => Some(format!("{date_label}: {t} tokens")),
        (None, None) => None,
    };
    MiniBarPoint {
        day_key: entry.date.clone(),
        value: cost,
        hover_summary,
    }
}

fn quota_window_comparison(window: &ComputedQuotaWindow) -> WindowComparisonItem {
    let cost = window
        .cost_usd
        .map(|v| format::currency_string(v, "USD"))
        .unwrap_or_else(|| "—".to_string());
    let value = match window
        .total_tokens
        .and_then(|t| i64::try_from(t).ok())
        .map(format::token_count_string)
    {
        Some(tokens) => format!("{cost} · {tokens}"),
        None => cost,
    };
    WindowComparisonItem {
        title: window.label.clone(),
        value,
        range: window.range.clone(),
        note: None,
    }
}

/// `topCostModel` (`InlineUsageDashboardContent.swift:670-693`).
fn top_cost_model(entries: &[CostDailyEntry]) -> Option<String> {
    let mut scores: BTreeMap<&str, (f64, u64)> = BTreeMap::new();
    for entry in entries {
        for model in &entry.model_breakdowns {
            let score = scores.entry(model.model_name.as_str()).or_insert((0.0, 0));
            score.0 += model.cost_usd.unwrap_or(0.0);
            score.1 = score.1.saturating_add(model.total_tokens.unwrap_or(0));
        }
    }
    scores
        .into_iter()
        .max_by(|(_, a), (_, b)| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(&b.1))
        })
        .map(|(name, _)| name.to_string())
}

/// `shortModelName` (`InlineUsageDashboardContent.swift:664-668`).
fn short_model_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.chars().count() <= 26 {
        trimmed.to_string()
    } else {
        let prefix: String = trimmed.chars().take(25).collect();
        format!("{prefix}…")
    }
}

/// `cloudCreditsSection` (`MenuCardView+Costs.swift:618-636`).
fn cloud_credits_section(
    provider: &ProviderSnapshot,
    prefs: &CardPreferences,
) -> Option<ProviderCostSection> {
    if provider.id.as_str() != "claude" || !prefs.show_optional_credits_and_extra_usage {
        return None;
    }
    let section = provider.details.iter().find(|s| {
        s.title
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("Cloud credits"))
    })?;
    let available = section
        .rows
        .iter()
        .find(|r| r.label.eq_ignore_ascii_case("Available"))?
        .value
        .trim();
    if available.is_empty() {
        return None;
    }
    Some(ProviderCostSection {
        title: "Cloud credits".to_string(),
        presentation: ProviderCostPresentation::InlineBalance {
            value: available.to_string(),
        },
    })
}

/// `providerCostSection` (`MenuCardView+Costs.swift:466-615`).
fn provider_cost_section(
    provider_id: &str,
    cost: &Cost,
    percent_style: PercentStyle,
) -> Option<ProviderCostSection> {
    let is_claude = provider_id == "claude";
    let limit_val = cost.limit.unwrap_or(0.0);
    if limit_val <= 0.0 {
        if is_claude {
            let bal = cost.balance?;
            return Some(ProviderCostSection {
                title: "Credits".to_string(),
                presentation: ProviderCostPresentation::InlineBalance {
                    value: format::currency_string(bal, &cost.currency_code),
                },
            });
        } else if provider_id == "codex" {
            let bal = cost.balance?;
            return Some(ProviderCostSection {
                title: "Extra usage".to_string(),
                presentation: ProviderCostPresentation::InlineBalance {
                    value: format!("Balance: {}", format::credits_number_string(bal)),
                },
            });
        }
        return None;
    }
    let used = format::currency_string(cost.used, &cost.currency_code);
    let limit = format::currency_string(limit_val, &cost.currency_code);
    let raw_period = cost.period.as_deref().unwrap_or("This month").trim();
    let period_label = if raw_period.eq_ignore_ascii_case("monthly")
        || raw_period.eq_ignore_ascii_case("this month")
    {
        "This month".to_string()
    } else {
        raw_period.to_string()
    };
    let percent_used = clamped((cost.used / limit_val) * 100.0);
    let effective_style = if is_claude {
        percent_style
    } else {
        PercentStyle::Used
    };
    let display_percent = match effective_style {
        PercentStyle::Used => percent_used,
        PercentStyle::Left => clamped(100.0 - percent_used),
    };
    let percent_line = format::percent_text(display_percent, effective_style.label_suffix());
    let personal_spend_line = (is_claude && cost.balance_is_workspace == Some(true))
        .then_some(cost.personal_used)
        .flatten()
        .map(|p| {
            format!(
                "Your spend: {}",
                format::currency_string(p, &cost.currency_code)
            )
        });
    let balance_line = (!is_claude && cost.balance_is_unavailable != Some(true))
        .then_some(cost.balance)
        .flatten()
        .map(|b| {
            format!(
                "Balance: {}",
                format::currency_string(b, &cost.currency_code)
            )
        });
    Some(ProviderCostSection {
        title: "Extra usage".to_string(),
        presentation: ProviderCostPresentation::Detail {
            percent_used: Some(percent_used),
            percent_style: effective_style,
            spend_line: format!("{period_label}: {used} / {limit}"),
            percent_line: Some(percent_line),
            balance_line,
            personal_spend_line,
        },
    })
}

fn supports_credits_section(provider_id: &str) -> bool {
    provider_id != "claude" && provider_id != "antigravity"
}

fn subtitle(
    provider: &ProviderSnapshot,
    last_error: Option<&str>,
    refreshing: bool,
    now: Timestamp,
    dates: &dyn DateTexts,
) -> Subtitle {
    if let Some(message) = last_error {
        return Subtitle {
            text: message.to_string(),
            style: SubtitleStyle::Error,
        };
    }
    if refreshing {
        return Subtitle {
            text: "Refreshing…".to_string(),
            style: SubtitleStyle::Loading,
        };
    }
    let text = match provider.updated_at {
        Some(updated) => format::updated_string(updated, now, dates),
        None => "Not fetched yet".to_string(),
    };
    Subtitle {
        text,
        style: SubtitleStyle::Info,
    }
}

/// The provider is refreshing while any positional window is loading.
fn is_refreshing(provider: &ProviderSnapshot) -> bool {
    let w = &provider.windows;
    [&w.primary, &w.secondary, &w.tertiary]
        .into_iter()
        .flatten()
        .any(|m| m.state == MetricState::Loading)
}

fn has_any_value(provider: &ProviderSnapshot) -> bool {
    let w = &provider.windows;
    [&w.primary, &w.secondary, &w.tertiary]
        .into_iter()
        .flatten()
        .any(|m| displayable_value(m).is_some())
        || w.extra
            .iter()
            .any(|n| displayable_value(&n.window).is_some())
        || provider
            .credits
            .as_ref()
            .is_some_and(|c| displayable_value(c).is_some())
        || provider
            .cost_history
            .as_ref()
            .is_some_and(|c| displayable_value(c).is_some())
        || provider
            .cost
            .as_ref()
            .is_some_and(|c| displayable_value(c).is_some())
}

fn account_email(provider: &ProviderSnapshot) -> String {
    provider
        .identity
        .as_ref()
        .and_then(displayable_value)
        .and_then(|identity| identity.account_email.as_deref())
        .map(str::trim)
        .unwrap_or_default()
        .to_string()
}

/// The plan label from the login method, or the plan when no login method
/// is known.
fn plan_text(provider: &ProviderSnapshot) -> Option<String> {
    let identity = provider.identity.as_ref().and_then(displayable_value)?;
    let raw = [&identity.login_method, &identity.plan]
        .into_iter()
        .flatten()
        .find(|t| !t.is_empty())?;
    Some(format::plan_display(provider.id.as_str(), raw))
}

/// `creditsLine`, `creditsProgressPercent` and `creditsScaleText`.
fn credits_section(metric: &Metric<Credits>) -> Option<CreditsSection> {
    if let Some(credits) = displayable_value(metric) {
        let workspace = credits.balance_is_workspace == Some(true);
        let scale = (!workspace)
            .then(|| fallback_credits_scale(credits.remaining))
            .flatten();
        return Some(CreditsSection {
            text: format::credits_string(credits.remaining),
            show_progress: !workspace,
            progress_percent: scale.map(|s| (credits.remaining / s * 100.0).clamp(0.0, 100.0)),
            scale_text: scale.map(credits_scale_text),
            hint_text: None,
        });
    }
    let message = metric.error.as_ref()?.message.trim();
    (!message.is_empty()).then(|| CreditsSection {
        text: message.to_string(),
        show_progress: true,
        progress_percent: None,
        scale_text: None,
        hint_text: None,
    })
}

/// The next power of ten above the balance, used as the bar's full scale.
fn fallback_credits_scale(balance: f64) -> Option<f64> {
    if !balance.is_finite() {
        return None;
    }
    let maximum = balance.max(0.0);
    let exponent = if maximum > 0.0 {
        maximum.log10().floor() + 1.0
    } else {
        0.0
    };
    Some(10f64.powf(exponent).min(f64::MAX))
}

fn credits_scale_text(scale: f64) -> String {
    let number = if scale.fract() == 0.0 && scale.abs() < 9.2e18 {
        format::token_count_string(scale as i64)
    } else {
        format::credits_number_string(scale)
    };
    format!("{number} tokens")
}

/// A metric's value when its state allows showing it; loading, unknown and
/// failed metrics never render as a number.
fn displayable_value<T>(metric: &Metric<T>) -> Option<&T> {
    match metric.state {
        MetricState::Value | MetricState::Stale => metric.value.as_ref(),
        _ => None,
    }
}

fn clamped(value: f64) -> f64 {
    value.clamp(0.0, 100.0)
}
