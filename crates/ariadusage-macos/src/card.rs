// Ported from CodexBar Sources/CodexBar/MenuCardView.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuCardView+ModelHelpers.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/MenuCardView+Costs.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

//! The usage menu card's model: what the card shows for one provider.
//!
//! [`card_model`] follows `UsageMenuCardView.Model.make` for the parts the
//! snapshot carries: the header, the subtitle, one row per rate window, and
//! the credits section. Window titles and the accent color are inputs, so
//! the engine's presentation block supplies them; [`card_presentation`]
//! is the one place that maps a snapshot to them.

use ariadusage_protocol::metric::{Metric, MetricState};
use ariadusage_protocol::snapshot::ProviderSnapshot;
use ariadusage_protocol::usage::{Credits, Pace, PaceStage, RateWindow};
use jiff::Timestamp;

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

    /// The pace and detail halves joined with ` · `.
    pub fn meta_text(&self) -> Option<String> {
        let parts: Vec<&str> = [&self.detail_left_text, &self.detail_right_text]
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

/// The credits section.
#[derive(Debug, Clone, PartialEq)]
pub struct CreditsSection {
    /// `N left`, or the credits error.
    pub text: String,
    pub show_progress: bool,
    pub progress_percent: Option<f64>,
    /// `N tokens`, the scale of the bar.
    pub scale_text: Option<String>,
}

/// Everything the card shows.
#[derive(Debug, Clone, PartialEq)]
pub struct CardModel {
    pub provider_name: String,
    /// The account email, or empty. Never log it.
    pub email: String,
    pub subtitle: Subtitle,
    pub plan: Option<String>,
    pub metrics: Vec<MetricRow>,
    pub placeholder: Option<String>,
    pub credits: Option<CreditsSection>,
    pub accent: Accent,
}

impl CardModel {
    pub fn has_usage_content(&self) -> bool {
        !self.metrics.is_empty() || self.placeholder.is_some()
    }

    /// Rows, credits or cost stack below the header in sections.
    pub fn uses_stacked_detail_layout(&self) -> bool {
        !self.metrics.is_empty() || self.credits.is_some()
    }

    /// Whether anything follows the header.
    pub fn has_details(&self) -> bool {
        self.has_usage_content() || self.uses_stacked_detail_layout()
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
    let builder = RowBuilder {
        provider_id,
        prefs,
        now,
        dates,
        percent_style: if prefs.show_used {
            PercentStyle::Used
        } else {
            PercentStyle::Left
        },
    };
    let titles = &presentation.titles;
    let windows = &provider.windows;

    let mut metrics = Vec::new();
    if let Some(primary) = windows.primary.as_ref().and_then(displayable_value) {
        metrics.push(builder.primary(primary, titles.session.clone()));
    }
    if let Some(weekly) = windows.secondary.as_ref().and_then(displayable_value) {
        let pace = provider.pace.as_ref().and_then(displayable_value);
        metrics.push(builder.secondary(weekly, titles.weekly.clone(), pace));
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

    CardModel {
        provider_name: provider.display_name.clone(),
        email: account_email(provider),
        subtitle: subtitle(provider, last_error, refreshing, now, dates),
        plan: plan_text(provider),
        metrics,
        placeholder: (!has_snapshot && !refreshing && last_error.is_none())
            .then(|| "No usage yet".to_string()),
        credits: provider.credits.as_ref().and_then(credits_section),
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

    /// The session row. CodexBar derives session pace in the app from the
    /// window; AriadUsage leaves pace to the engine, which sends one weekly
    /// pace, so this row has none.
    fn primary(&self, window: &RateWindow, title: String) -> MetricRow {
        let mut row = self.row("primary", title, window);
        row.warning_marker_percents = progress_bar::warning_marker_percents(
            &self.prefs.session_warning_thresholds,
            self.prefs.show_used,
        );
        row
    }

    /// The weekly row, with the engine's pace.
    fn secondary(&self, window: &RateWindow, title: String, pace: Option<&Pace>) -> MetricRow {
        let mut row = self.row("secondary", title, window);
        if let Some(pace) = pace {
            self.apply_weekly_pace(&mut row, window, pace);
        }
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
        row.warning_marker_percents = progress_bar::warning_marker_percents(
            &self.prefs.weekly_warning_thresholds,
            self.prefs.show_used,
        );
        row
    }

    /// A named extra window. Unknown or failed usage shows `Unavailable`;
    /// a window still loading has no row yet.
    fn extra(&self, id: &str, title: &str, metric: &Metric<RateWindow>) -> Option<MetricRow> {
        match (metric.state, displayable_value(metric)) {
            (_, Some(window)) => Some(self.row(id, title.to_string(), window)),
            (MetricState::Loading, None) => None,
            (_, None) => {
                let mut row = MetricRow::new(id, title.to_string(), 0.0, self.percent_style);
                row.status_text = Some("Unavailable".to_string());
                Some(row)
            }
        }
    }

    /// `weeklyPaceDetail` and `paceDetail`.
    fn apply_weekly_pace(&self, row: &mut MetricRow, window: &RateWindow, pace: &Pace) {
        if (100.0 - window.used_percent).max(0.0) <= 0.0 {
            return;
        }
        if !pace.expected_used_percent.is_finite() || !window.used_percent.is_finite() {
            return;
        }
        let facts = PaceFacts {
            stage: pace.stage,
            delta_percent: pace.delta_percent,
            expected_used_percent: pace.expected_used_percent,
            will_last_to_reset: pace.will_last_to_reset,
            eta_seconds: pace.eta_seconds.map(|s| s as f64),
            // The snapshot does not carry these yet.
            run_out_probability: None,
            speed_multiplier_to_reset: None,
        };
        let detail = pace_text::detail(
            &facts,
            shows_headroom_hint(self.provider_id),
            PaceContext::Weekly,
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
}

/// Codex is the only v1 provider whose pace shows the headroom hint
/// (`CodexProviderDescriptor.swift`).
fn shows_headroom_hint(provider_id: &str) -> bool {
    provider_id == "codex"
}

/// Clears pace output when the user hides pace (`paceGatedMetrics`).
fn gate_pace(metric: &mut MetricRow) {
    if metric.detail_is_pace_derived {
        metric.detail_left_text = None;
        metric.detail_right_text = None;
    }
    metric.pace_percent = None;
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
        });
    }
    let message = metric.error.as_ref()?.message.trim();
    (!message.is_empty()).then(|| CreditsSection {
        text: message.to_string(),
        show_progress: true,
        progress_percent: None,
        scale_text: None,
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
