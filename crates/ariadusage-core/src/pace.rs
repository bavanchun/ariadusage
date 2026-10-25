// Ported from CodexBar Sources/CodexBarCore/UsagePace.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderDescriptor.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/RateWindow+BindingCap.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_protocol::{Confidence, PaceStage};
use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::model::UsageSnapshot;
use crate::model::window::RateWindow;

/// Default weekly quota window duration in minutes (7 days).
pub const DEFAULT_WEEKLY_WINDOW_MINUTES: i64 = 7 * 24 * 60; // 10080
/// Default session quota window duration in minutes (5 hours).
pub const DEFAULT_SESSION_WINDOW_MINUTES: i64 = 5 * 60; // 300
/// Monthly rate window sentinel duration in minutes (30 days).
pub const MONTHLY_WINDOW_SENTINEL_MINUTES: i64 = 30 * 24 * 60; // 43200
/// Minimum expected used percentage before session or weekly pace is shown (`UsagePaceText.swift:6`).
pub const MINIMUM_EXPECTED_PERCENT: f64 = 3.0;

/// Core usage pacing computation comparing quota consumption to time progression.
#[derive(Debug, Clone, PartialEq)]
pub struct UsagePace {
    pub stage: PaceStage,
    pub delta_percent: f64,
    pub expected_used_percent: f64,
    pub actual_used_percent: f64,
    pub eta_seconds: Option<f64>,
    pub will_last_to_reset: bool,
    pub run_out_probability: Option<f64>,
    pub speed_multiplier_to_reset: Option<f64>,
}

impl UsagePace {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        stage: PaceStage,
        delta_percent: f64,
        expected_used_percent: f64,
        actual_used_percent: f64,
        eta_seconds: Option<f64>,
        will_last_to_reset: bool,
        run_out_probability: Option<f64>,
        speed_multiplier_to_reset: Option<f64>,
    ) -> Self {
        Self {
            stage,
            delta_percent,
            expected_used_percent,
            actual_used_percent,
            eta_seconds,
            will_last_to_reset,
            run_out_probability,
            speed_multiplier_to_reset,
        }
    }

    /// Computes weekly or session pace against a rate window.
    pub fn weekly(
        window: &RateWindow,
        now: Timestamp,
        default_window_minutes: Option<i64>,
        work_days: Option<u32>,
        time_zone: &TimeZone,
    ) -> Option<Self> {
        let resets_at = window.resets_at?;
        let minutes = window
            .window_minutes
            .unwrap_or(default_window_minutes.unwrap_or(DEFAULT_WEEKLY_WINDOW_MINUTES));
        if minutes <= 0 {
            return None;
        }

        let duration = minutes as f64 * 60.0;
        let time_until_reset = (resets_at.as_second() - now.as_second()) as f64
            + (resets_at.subsec_nanosecond() as f64 - now.subsec_nanosecond() as f64) / 1e9;
        if time_until_reset <= 0.0 || time_until_reset > duration {
            return None;
        }
        let elapsed = (duration - time_until_reset).clamp(0.0, duration);

        let workday_progress = if let Some(wd) = work_days {
            if (2..7).contains(&wd) && minutes == DEFAULT_WEEKLY_WINDOW_MINUTES {
                Self::workday_progress(now, duration, resets_at, wd, time_zone)
            } else {
                None
            }
        } else {
            None
        };

        let expected = match &workday_progress {
            Some(wp) => wp.expected_used_percent(),
            None => ((elapsed / duration) * 100.0).clamp(0.0, 100.0),
        };
        let actual = window.used_percent.clamp(0.0, 100.0);
        if elapsed == 0.0 && actual > 0.0 {
            return None;
        }
        let delta = actual - expected;
        let stage = Self::stage(delta);

        let mut eta_seconds = None;
        let mut will_last_to_reset = false;

        let pace_elapsed = workday_progress
            .as_ref()
            .map(|wp| wp.elapsed_seconds)
            .unwrap_or(elapsed);
        let effective_time_until_reset = workday_progress
            .as_ref()
            .map(|wp| wp.remaining_seconds)
            .unwrap_or(time_until_reset);
        let projected_remaining_usage = if pace_elapsed > 0.0 {
            actual * effective_time_until_reset / pace_elapsed
        } else {
            0.0
        };
        let speed_multiplier_to_reset =
            Self::safe_speed_multiplier(100.0 - actual, projected_remaining_usage);

        if actual >= 100.0 {
            eta_seconds = Some(0.0);
        } else if pace_elapsed > 0.0 && actual > 0.0 {
            let rate = actual / pace_elapsed;
            if rate > 0.0 {
                let remaining = 100.0 - actual;
                let candidate = remaining / rate;
                if candidate >= effective_time_until_reset {
                    will_last_to_reset = true;
                } else if let Some(wp) = &workday_progress {
                    eta_seconds = Self::wall_clock_interval(
                        now,
                        resets_at,
                        candidate,
                        wp.work_days,
                        time_zone,
                    );
                } else {
                    eta_seconds = Some(candidate);
                }
            }
        } else if pace_elapsed > 0.0 && actual == 0.0 {
            will_last_to_reset = true;
        }

        Some(Self {
            stage,
            delta_percent: delta,
            expected_used_percent: expected,
            actual_used_percent: actual,
            eta_seconds,
            will_last_to_reset,
            run_out_probability: None,
            speed_multiplier_to_reset,
        })
    }

    /// Constructs pace metrics from historical or model estimates.
    pub fn historical(
        expected_used_percent: f64,
        actual_used_percent: f64,
        eta_seconds: Option<f64>,
        will_last_to_reset: bool,
        run_out_probability: Option<f64>,
        projected_remaining_usage: Option<f64>,
    ) -> Self {
        let expected = expected_used_percent.clamp(0.0, 100.0);
        let actual = actual_used_percent.clamp(0.0, 100.0);
        let delta = actual - expected;
        let speed_multiplier_to_reset =
            projected_remaining_usage.and_then(|p| Self::safe_speed_multiplier(100.0 - actual, p));
        Self {
            stage: Self::stage(delta),
            delta_percent: delta,
            expected_used_percent: expected,
            actual_used_percent: actual,
            eta_seconds,
            will_last_to_reset,
            run_out_probability,
            speed_multiplier_to_reset,
        }
    }

    fn safe_speed_multiplier(
        remaining_capacity: f64,
        projected_remaining_usage: f64,
    ) -> Option<f64> {
        if remaining_capacity > 0.0 && projected_remaining_usage > 0.0 {
            let multiplier = remaining_capacity / projected_remaining_usage;
            if multiplier.is_finite() {
                Some(multiplier)
            } else {
                None
            }
        } else {
            None
        }
    }

    fn workday_progress(
        now: Timestamp,
        duration: f64,
        resets_at: Timestamp,
        work_days: u32,
        time_zone: &TimeZone,
    ) -> Option<WorkdayProgress> {
        let window_start = resets_at
            .checked_sub(jiff::Span::new().seconds(duration.round() as i64))
            .ok()?;

        let mut total_work_seconds = 0.0;
        let mut elapsed_work_seconds = 0.0;
        let mut remaining_work_seconds = 0.0;

        let mut cursor = window_start;
        while cursor < resets_at {
            let start_of_next_day = Self::next_day_boundary(cursor, time_zone)?;
            if start_of_next_day <= cursor {
                return None;
            }
            let slice_end = start_of_next_day.min(resets_at);

            if Self::is_workday(cursor, time_zone, work_days) {
                let slice_duration = (slice_end.as_second() - cursor.as_second()) as f64
                    + (slice_end.subsec_nanosecond() as f64 - cursor.subsec_nanosecond() as f64)
                        / 1e9;
                total_work_seconds += slice_duration;

                if now > cursor {
                    let eff_end = now.min(slice_end);
                    let elapsed_slice = (eff_end.as_second() - cursor.as_second()) as f64
                        + (eff_end.subsec_nanosecond() as f64 - cursor.subsec_nanosecond() as f64)
                            / 1e9;
                    elapsed_work_seconds += elapsed_slice;
                }
                if now < slice_end {
                    let eff_start = now.max(cursor);
                    let remaining_slice = (slice_end.as_second() - eff_start.as_second()) as f64
                        + (slice_end.subsec_nanosecond() as f64
                            - eff_start.subsec_nanosecond() as f64)
                            / 1e9;
                    remaining_work_seconds += remaining_slice;
                }
            }
            cursor = slice_end;
        }

        if total_work_seconds <= 0.0 {
            return None;
        }

        Some(WorkdayProgress {
            work_days,
            total_seconds: total_work_seconds,
            elapsed_seconds: elapsed_work_seconds,
            remaining_seconds: remaining_work_seconds,
        })
    }

    fn wall_clock_interval(
        now: Timestamp,
        resets_at: Timestamp,
        required_work_seconds: f64,
        work_days: u32,
        time_zone: &TimeZone,
    ) -> Option<f64> {
        if required_work_seconds <= 0.0 {
            return Some(0.0);
        }

        let mut remaining = required_work_seconds;
        let mut cursor = now;
        while cursor < resets_at {
            let start_of_next_day = Self::next_day_boundary(cursor, time_zone)?;
            if start_of_next_day <= cursor {
                return None;
            }
            let slice_end = start_of_next_day.min(resets_at);

            if Self::is_workday(cursor, time_zone, work_days) {
                let available = (slice_end.as_second() - cursor.as_second()) as f64
                    + (slice_end.subsec_nanosecond() as f64 - cursor.subsec_nanosecond() as f64)
                        / 1e9;
                if remaining <= available {
                    let cursor_since_now = (cursor.as_second() - now.as_second()) as f64
                        + (cursor.subsec_nanosecond() as f64 - now.subsec_nanosecond() as f64)
                            / 1e9;
                    return Some(cursor_since_now + remaining);
                }
                remaining -= available;
            }
            cursor = slice_end;
        }
        None
    }

    fn next_day_boundary(after: Timestamp, time_zone: &TimeZone) -> Option<Timestamp> {
        let zdt = after.to_zoned(time_zone.clone());
        let next_date = zdt.date().checked_add(jiff::Span::new().days(1)).ok()?;
        let next_zdt = next_date.to_zoned(time_zone.clone()).ok()?;
        Some(next_zdt.timestamp())
    }

    fn is_workday(date: Timestamp, time_zone: &TimeZone, work_days: u32) -> bool {
        let zdt = date.to_zoned(time_zone.clone());
        let iso_weekday = zdt.weekday().to_monday_one_offset();
        (iso_weekday as u32) <= work_days
    }

    fn stage(delta: f64) -> PaceStage {
        let abs_delta = delta.abs();
        if abs_delta <= 2.0 {
            PaceStage::OnTrack
        } else if abs_delta <= 6.0 {
            if delta >= 0.0 {
                PaceStage::SlightlyAhead
            } else {
                PaceStage::SlightlyBehind
            }
        } else if abs_delta <= 12.0 {
            if delta >= 0.0 {
                PaceStage::Ahead
            } else {
                PaceStage::Behind
            }
        } else if delta >= 0.0 {
            PaceStage::FarAhead
        } else {
            PaceStage::FarBehind
        }
    }
}

struct WorkdayProgress {
    work_days: u32,
    total_seconds: f64,
    elapsed_seconds: f64,
    remaining_seconds: f64,
}

impl WorkdayProgress {
    fn expected_used_percent(&self) -> f64 {
        ((self.elapsed_seconds / self.total_seconds) * 100.0).clamp(0.0, 100.0)
    }
}

/// Which window horizon a pace lane describes (`ProviderPaceKind`, `ProviderDescriptor.swift:155-169`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPaceKind {
    Session,
    Weekly,
}

impl ProviderPaceKind {
    pub const fn default_window_minutes(self) -> i64 {
        match self {
            Self::Session => DEFAULT_SESSION_WINDOW_MINUTES,
            Self::Weekly => DEFAULT_WEEKLY_WINDOW_MINUTES,
        }
    }
}

/// Positional rate window slot (`ProviderPaceSlot`, `ProviderDescriptor.swift:171-177`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPaceSlot {
    Primary,
    Secondary,
    Tertiary,
}

/// Rule for matching a rate window to a pace lane (`ProviderPaceWindowRule`,
/// `ProviderDescriptor.swift:179-225`).
#[derive(Debug, Clone, Copy)]
pub enum ProviderPaceWindowRule {
    Unsupported,
    Always,
    ResetDatePresent,
    WindowDurationPresent,
    WindowDuration(i64),
    WindowDurationAtMost {
        maximum_minutes: i64,
        requires_duration: bool,
    },
    Custom(fn(&RateWindow, Timestamp) -> bool),
}

impl PartialEq for ProviderPaceWindowRule {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unsupported, Self::Unsupported)
            | (Self::Always, Self::Always)
            | (Self::ResetDatePresent, Self::ResetDatePresent)
            | (Self::WindowDurationPresent, Self::WindowDurationPresent) => true,
            (Self::WindowDuration(a), Self::WindowDuration(b)) => a == b,
            (
                Self::WindowDurationAtMost {
                    maximum_minutes: a_max,
                    requires_duration: a_req,
                },
                Self::WindowDurationAtMost {
                    maximum_minutes: b_max,
                    requires_duration: b_req,
                },
            ) => a_max == b_max && a_req == b_req,
            (Self::Custom(a), Self::Custom(b)) => std::ptr::fn_addr_eq(*a, *b),
            _ => false,
        }
    }
}

impl Eq for ProviderPaceWindowRule {}

impl ProviderPaceWindowRule {
    pub const fn unsupported() -> Self {
        Self::Unsupported
    }

    pub const fn always() -> Self {
        Self::Always
    }

    pub const fn reset_date_present() -> Self {
        Self::ResetDatePresent
    }

    pub const fn window_duration_present() -> Self {
        Self::WindowDurationPresent
    }

    pub const fn window_duration(minutes: i64) -> Self {
        Self::WindowDuration(minutes)
    }

    pub const fn window_duration_at_most(maximum_minutes: i64, requires_duration: bool) -> Self {
        Self::WindowDurationAtMost {
            maximum_minutes,
            requires_duration,
        }
    }

    pub fn matches(&self, window: &RateWindow, now: Timestamp) -> bool {
        match self {
            Self::Unsupported => false,
            Self::Always => true,
            Self::ResetDatePresent => window.resets_at.is_some(),
            Self::WindowDurationPresent => window.window_minutes.is_some(),
            Self::WindowDuration(minutes) => window.window_minutes == Some(*minutes),
            Self::WindowDurationAtMost {
                maximum_minutes,
                requires_duration,
            } => match window.window_minutes {
                Some(window_minutes) => window_minutes <= *maximum_minutes,
                None => !*requires_duration,
            },
            Self::Custom(predicate) => predicate(window, now),
        }
    }
}

/// Rule for inferring monthly calendar duration for a rate window
/// (`ProviderPaceDurationRule`, `ProviderDescriptor.swift:227-263`).
#[derive(Debug, Clone, Copy)]
pub enum ProviderPaceDurationRule {
    Unsupported,
    WindowDurationMissing,
    WindowDuration(i64),
    Custom(fn(&RateWindow) -> bool),
}

impl PartialEq for ProviderPaceDurationRule {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unsupported, Self::Unsupported)
            | (Self::WindowDurationMissing, Self::WindowDurationMissing) => true,
            (Self::WindowDuration(a), Self::WindowDuration(b)) => a == b,
            (Self::Custom(a), Self::Custom(b)) => std::ptr::fn_addr_eq(*a, *b),
            _ => false,
        }
    }
}

impl Eq for ProviderPaceDurationRule {}

impl ProviderPaceDurationRule {
    pub const fn unsupported() -> Self {
        Self::Unsupported
    }

    pub const fn window_duration_missing() -> Self {
        Self::WindowDurationMissing
    }

    pub const fn window_duration(minutes: i64) -> Self {
        Self::WindowDuration(minutes)
    }

    pub fn matches(&self, window: &RateWindow) -> bool {
        match self {
            Self::Unsupported => false,
            Self::WindowDurationMissing => window.window_minutes.is_none(),
            Self::WindowDuration(minutes) => window.window_minutes == Some(*minutes),
            Self::Custom(predicate) => predicate(window),
        }
    }
}

/// Standard positional pace lane rule (`ProviderStandardPaceLane`,
/// `ProviderDescriptor.swift:265-289`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderStandardPaceLane {
    pub kind: ProviderPaceKind,
    pub window_rule: ProviderPaceWindowRule,
}

impl ProviderStandardPaceLane {
    pub const fn new(kind: ProviderPaceKind, window_rule: ProviderPaceWindowRule) -> Self {
        Self { kind, window_rule }
    }

    pub const fn session(maximum_minutes: i64, requires_duration: bool) -> Self {
        Self {
            kind: ProviderPaceKind::Session,
            window_rule: ProviderPaceWindowRule::window_duration_at_most(
                maximum_minutes,
                requires_duration,
            ),
        }
    }

    pub const fn weekly() -> Self {
        Self {
            kind: ProviderPaceKind::Weekly,
            window_rule: ProviderPaceWindowRule::always(),
        }
    }

    pub const fn weekly_with_duration() -> Self {
        Self {
            kind: ProviderPaceKind::Weekly,
            window_rule: ProviderPaceWindowRule::window_duration_present(),
        }
    }

    pub const fn exact(kind: ProviderPaceKind, minutes: i64) -> Self {
        Self {
            kind,
            window_rule: ProviderPaceWindowRule::window_duration(minutes),
        }
    }
}

/// Capabilities and rules governing pacing computation for a provider
/// (`ProviderPaceCapability`, `ProviderDescriptor.swift:291-332`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderPaceCapability {
    pub reset_window_pace: ProviderPaceWindowRule,
    pub inferred_monthly_duration: ProviderPaceDurationRule,
    pub primary: Option<ProviderStandardPaceLane>,
    pub secondary: Option<ProviderStandardPaceLane>,
    pub tertiary: Option<ProviderStandardPaceLane>,
    pub shows_headroom_hint: bool,
    pub session_pace_window_rule: ProviderPaceWindowRule,
    pub allows_estimated_usage: bool,
}

impl Default for ProviderPaceCapability {
    fn default() -> Self {
        Self::UNSUPPORTED
    }
}

impl ProviderPaceCapability {
    pub const UNSUPPORTED: Self = Self {
        reset_window_pace: ProviderPaceWindowRule::Unsupported,
        inferred_monthly_duration: ProviderPaceDurationRule::Unsupported,
        primary: None,
        secondary: None,
        tertiary: None,
        shows_headroom_hint: false,
        session_pace_window_rule: ProviderPaceWindowRule::Unsupported,
        allows_estimated_usage: true,
    };

    pub const CALENDAR_MONTH_RESET_WINDOW: Self = Self {
        reset_window_pace: ProviderPaceWindowRule::WindowDuration(MONTHLY_WINDOW_SENTINEL_MINUTES),
        inferred_monthly_duration: ProviderPaceDurationRule::WindowDuration(
            MONTHLY_WINDOW_SENTINEL_MINUTES,
        ),
        primary: Some(ProviderStandardPaceLane::session(
            DEFAULT_SESSION_WINDOW_MINUTES,
            false,
        )),
        secondary: Some(ProviderStandardPaceLane::weekly()),
        tertiary: Some(ProviderStandardPaceLane::weekly()),
        shows_headroom_hint: false,
        session_pace_window_rule: ProviderPaceWindowRule::Always,
        allows_estimated_usage: true,
    };

    pub const fn unsupported() -> Self {
        Self::UNSUPPORTED
    }

    pub const fn calendar_month_reset_window() -> Self {
        Self::CALENDAR_MONTH_RESET_WINDOW
    }

    pub const fn allows_pace(&self, data_confidence: Confidence) -> bool {
        !matches!(data_confidence, Confidence::Estimated) || self.allows_estimated_usage
    }

    pub fn supports_reset_window_pace(&self, window: &RateWindow, now: Timestamp) -> bool {
        self.reset_window_pace.matches(window, now)
    }

    pub fn uses_inferred_monthly_duration(&self, window: &RateWindow) -> bool {
        self.inferred_monthly_duration.matches(window)
    }

    pub fn resolved_reset_window_for_pace(&self, window: &RateWindow) -> RateWindow {
        if self.uses_inferred_monthly_duration(window)
            && let Some(resets_at) = window.resets_at
            && let Some(minutes) = Self::inferred_monthly_window_minutes(resets_at)
        {
            RateWindow {
                used_percent: window.used_percent,
                window_minutes: Some(minutes),
                resets_at: window.resets_at,
                reset_description: window.reset_description.clone(),
                next_regen_percent: window.next_regen_percent,
                is_synthetic_placeholder: window.is_synthetic_placeholder,
            }
        } else {
            window.clone()
        }
    }

    pub fn resolved_kind(
        &self,
        slot: ProviderPaceSlot,
        window: &RateWindow,
        now: Timestamp,
    ) -> Option<ProviderPaceKind> {
        if self.supports_reset_window_pace(window, now) {
            return Some(ProviderPaceKind::Weekly);
        }
        let lane = match slot {
            ProviderPaceSlot::Primary => self.primary?,
            ProviderPaceSlot::Secondary => self.secondary?,
            ProviderPaceSlot::Tertiary => self.tertiary?,
        };
        lane.window_rule.matches(window, now).then_some(lane.kind)
    }

    pub fn supports_session_pace(&self, window: &RateWindow, now: Timestamp) -> bool {
        self.session_pace_window_rule.matches(window, now)
    }

    /// Computes session pace for a 5-hour session window (`UsagePaceText.sessionPace`,
    /// `UsagePaceText.swift:92-104`).
    pub fn session_pace(
        &self,
        window: &RateWindow,
        now: Timestamp,
        time_zone: &TimeZone,
    ) -> Option<UsagePace> {
        if !self.supports_session_pace(window, now) {
            return None;
        }
        if window.remaining_percent() <= 0.0 {
            return None;
        }
        let pace = UsagePace::weekly(
            window,
            now,
            Some(DEFAULT_SESSION_WINDOW_MINUTES),
            None,
            time_zone,
        )?;
        (pace.expected_used_percent >= MINIMUM_EXPECTED_PERCENT).then_some(pace)
    }

    pub fn inferred_monthly_window_minutes(ending_at: Timestamp) -> Option<i64> {
        let resets_zdt = ending_at.to_zoned(TimeZone::UTC);
        let starts_zdt = resets_zdt.checked_sub(jiff::Span::new().months(1)).ok()?;
        let seconds = (resets_zdt.timestamp().as_second() - starts_zdt.timestamp().as_second())
            as f64
            + (resets_zdt.timestamp().subsec_nanosecond() as f64
                - starts_zdt.timestamp().subsec_nanosecond() as f64)
                / 1e9;
        let minutes = seconds / 60.0;
        if minutes.is_finite() && minutes > 0.0 {
            Some(minutes.round() as i64)
        } else {
            None
        }
    }
}

/// Displayed primary percentage and reset when an exhausted longer-horizon binding quota caps
/// the primary session lane (`RateWindowBindingQuotaProjection`, `RateWindow+BindingCap.swift:3-65`).
#[derive(Debug, Clone, PartialEq)]
pub struct RateWindowBindingQuotaProjection {
    pub used_percent: f64,
    pub resets_at: Option<Timestamp>,
    pub reset_description: Option<String>,
}

/// Projects the displayed primary percentage and reset through exhausted longer binding lanes
/// (`RateWindow.bindingQuotaProjection`, `RateWindow+BindingCap.swift:21-65`).
pub fn binding_quota_projection(
    primary: &RateWindow,
    binding_lanes: &[RateWindow],
    now: Timestamp,
) -> Option<RateWindowBindingQuotaProjection> {
    let primary_minutes = primary
        .window_minutes
        .unwrap_or(DEFAULT_SESSION_WINDOW_MINUTES);
    let mut blockers: Vec<&RateWindow> = binding_lanes
        .iter()
        .filter(|lane| {
            lane.window_minutes
                .is_some_and(|minutes| minutes > primary_minutes)
                && is_actively_exhausted(lane, now)
        })
        .collect();
    if blockers.is_empty() {
        return None;
    }
    if is_actively_exhausted(primary, now) {
        blockers.push(primary);
    }
    let (resets_at, reset_description) = effective_binding_reset(&blockers);
    Some(RateWindowBindingQuotaProjection {
        used_percent: 100.0,
        resets_at,
        reset_description,
    })
}

fn is_actively_exhausted(window: &RateWindow, now: Timestamp) -> bool {
    window.remaining_percent() <= 0.0 && window.resets_at.is_none_or(|reset| reset > now)
}

fn effective_binding_reset(blockers: &[&RateWindow]) -> (Option<Timestamp>, Option<String>) {
    let unknown_reset_blockers: Vec<&&RateWindow> =
        blockers.iter().filter(|b| b.resets_at.is_none()).collect();
    if !unknown_reset_blockers.is_empty() {
        if blockers.len() == 1
            && let Some(desc) = unknown_reset_blockers[0]
                .reset_description
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        {
            return (None, Some(desc.to_string()));
        }
        return (None, None);
    }
    let latest = blockers.iter().filter_map(|b| b.resets_at).max();
    (latest, None)
}

/// Resolved pace and its window horizon kind for one rate window.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedWindowPace {
    pub pace: UsagePace,
    pub kind: ProviderPaceKind,
}

/// Computed paces across all windows of a [`UsageSnapshot`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SnapshotWindowPaces {
    pub primary: Option<ResolvedWindowPace>,
    pub secondary: Option<ResolvedWindowPace>,
    pub tertiary: Option<ResolvedWindowPace>,
    pub extra: Vec<Option<ResolvedWindowPace>>,
}

impl SnapshotWindowPaces {
    /// Computes displayable pace for every window (`primary`, `secondary`, `tertiary`, and
    /// `extra_rate_windows`) of `snapshot` according to `provider_id`'s [`ProviderPaceCapability`]
    /// and upstream menu-card/CLI rules (`MenuCardView.swift:1254-1389`,
    /// `MenuCardView+ModelHelpers.swift:127-165, 852-899, 1125-1175`).
    pub fn compute(
        provider_id: &str,
        snapshot: &UsageSnapshot,
        now: Timestamp,
        work_days: Option<u32>,
        time_zone: &TimeZone,
    ) -> Self {
        let extra_len = snapshot
            .extra_rate_windows
            .as_ref()
            .map_or(0, std::vec::Vec::len);
        let capability = crate::providers::find_by_id_str(provider_id)
            .map(|d| d.pace_capability)
            .unwrap_or_default();
        if !capability.allows_pace(snapshot.data_confidence) {
            return Self {
                primary: None,
                secondary: None,
                tertiary: None,
                extra: vec![None; extra_len],
            };
        }

        let primary = snapshot.primary.as_ref().and_then(|prim| {
            if prim.is_synthetic_placeholder {
                return None;
            }
            if provider_id == "claude"
                && let Some(sec) = snapshot.secondary.as_ref()
                && binding_quota_projection(prim, std::slice::from_ref(sec), now).is_some()
            {
                return None;
            }
            if provider_id == "antigravity" {
                return antigravity_window_pace(&capability, prim, now, work_days, time_zone);
            }
            reset_window_pace(&capability, prim, now, work_days, time_zone)
                .or_else(|| session_window_pace(&capability, prim, now, time_zone))
        });

        let secondary = snapshot.secondary.as_ref().and_then(|sec| {
            if sec.is_synthetic_placeholder {
                return None;
            }
            if provider_id == "antigravity" {
                return antigravity_window_pace(&capability, sec, now, work_days, time_zone);
            }
            match capability.resolved_kind(ProviderPaceSlot::Secondary, sec, now)? {
                ProviderPaceKind::Weekly => {
                    reset_window_pace(&capability, sec, now, work_days, time_zone).or_else(|| {
                        displayable_weekly_pace(sec, now, work_days, time_zone).map(|pace| {
                            ResolvedWindowPace {
                                pace,
                                kind: ProviderPaceKind::Weekly,
                            }
                        })
                    })
                }
                ProviderPaceKind::Session => session_window_pace(&capability, sec, now, time_zone),
            }
        });

        let tertiary = snapshot.tertiary.as_ref().and_then(|tert| {
            if tert.is_synthetic_placeholder {
                return None;
            }
            if provider_id == "antigravity" {
                return antigravity_window_pace(&capability, tert, now, work_days, time_zone);
            }
            match capability.resolved_kind(ProviderPaceSlot::Tertiary, tert, now)? {
                ProviderPaceKind::Weekly => {
                    reset_window_pace(&capability, tert, now, work_days, time_zone).or_else(|| {
                        displayable_weekly_pace(tert, now, work_days, time_zone).map(|pace| {
                            ResolvedWindowPace {
                                pace,
                                kind: ProviderPaceKind::Weekly,
                            }
                        })
                    })
                }
                ProviderPaceKind::Session => session_window_pace(&capability, tert, now, time_zone),
            }
        });

        let extra = snapshot
            .extra_rate_windows
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|named| {
                if !named.usage_known || named.window.is_synthetic_placeholder {
                    return None;
                }
                extra_rate_window_pace(
                    provider_id,
                    &capability,
                    &named.window,
                    now,
                    work_days,
                    time_zone,
                )
            })
            .collect();

        Self {
            primary,
            secondary,
            tertiary,
            extra,
        }
    }
}

fn displayable_weekly_pace(
    window: &RateWindow,
    now: Timestamp,
    work_days: Option<u32>,
    time_zone: &TimeZone,
) -> Option<UsagePace> {
    if window.is_synthetic_placeholder || window.remaining_percent() <= 0.0 {
        return None;
    }
    let pace = UsagePace::weekly(
        window,
        now,
        Some(DEFAULT_WEEKLY_WINDOW_MINUTES),
        work_days,
        time_zone,
    )?;
    (pace.expected_used_percent >= MINIMUM_EXPECTED_PERCENT || pace.eta_seconds == Some(0.0))
        .then_some(pace)
}

fn reset_window_pace(
    capability: &ProviderPaceCapability,
    window: &RateWindow,
    now: Timestamp,
    work_days: Option<u32>,
    time_zone: &TimeZone,
) -> Option<ResolvedWindowPace> {
    if window.is_synthetic_placeholder
        || window.remaining_percent() <= 0.0
        || !capability.supports_reset_window_pace(window, now)
    {
        return None;
    }
    let pace_window = capability.resolved_reset_window_for_pace(window);
    displayable_weekly_pace(&pace_window, now, work_days, time_zone).map(|pace| {
        ResolvedWindowPace {
            pace,
            kind: ProviderPaceKind::Weekly,
        }
    })
}

fn session_window_pace(
    capability: &ProviderPaceCapability,
    window: &RateWindow,
    now: Timestamp,
    time_zone: &TimeZone,
) -> Option<ResolvedWindowPace> {
    if window.is_synthetic_placeholder {
        return None;
    }
    capability
        .session_pace(window, now, time_zone)
        .map(|pace| ResolvedWindowPace {
            pace,
            kind: ProviderPaceKind::Session,
        })
}

fn antigravity_window_pace(
    capability: &ProviderPaceCapability,
    window: &RateWindow,
    now: Timestamp,
    work_days: Option<u32>,
    time_zone: &TimeZone,
) -> Option<ResolvedWindowPace> {
    if window.window_minutes.is_none() {
        return session_window_pace(capability, window, now, time_zone);
    }
    extra_rate_window_pace("antigravity", capability, window, now, work_days, time_zone)
}

fn extra_rate_window_pace(
    provider_id: &str,
    capability: &ProviderPaceCapability,
    window: &RateWindow,
    now: Timestamp,
    work_days: Option<u32>,
    time_zone: &TimeZone,
) -> Option<ResolvedWindowPace> {
    if window.is_synthetic_placeholder || window.remaining_percent() <= 0.0 {
        return None;
    }
    if let Some(rw) = reset_window_pace(capability, window, now, work_days, time_zone) {
        return Some(rw);
    }
    if provider_id == "claude" && window.window_minutes != Some(DEFAULT_WEEKLY_WINDOW_MINUTES) {
        return None;
    }
    if !matches!(provider_id, "codex" | "claude" | "antigravity") {
        return None;
    }
    match window.window_minutes {
        Some(DEFAULT_SESSION_WINDOW_MINUTES) => {
            session_window_pace(capability, window, now, time_zone)
        }
        Some(DEFAULT_WEEKLY_WINDOW_MINUTES) => {
            displayable_weekly_pace(window, now, work_days, time_zone).map(|pace| {
                ResolvedWindowPace {
                    pace,
                    kind: ProviderPaceKind::Weekly,
                }
            })
        }
        _ => None,
    }
}
