// Ported from CodexBar Sources/CodexBarCore/UsagePace.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderDescriptor.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_protocol::PaceStage;
use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::model::window::RateWindow;

/// Default weekly quota window duration in minutes (7 days).
pub const DEFAULT_WEEKLY_WINDOW_MINUTES: i64 = 7 * 24 * 60; // 10080
/// Default session quota window duration in minutes (5 hours).
pub const DEFAULT_SESSION_WINDOW_MINUTES: i64 = 5 * 60; // 300
/// Monthly rate window sentinel duration in minutes (30 days).
pub const MONTHLY_WINDOW_SENTINEL_MINUTES: i64 = 30 * 24 * 60; // 43200

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

/// Rule for matching a rate window to reset-window pace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPaceWindowRule {
    Unsupported,
    WindowDuration(i64),
    WindowDurationPresent,
}

/// Rule for inferring monthly calendar duration for a rate window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderPaceDurationRule {
    Unsupported,
    WindowDuration(i64),
}

/// Capabilities and rules governing pacing computation for a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPaceCapability {
    pub reset_window_pace: ProviderPaceWindowRule,
    pub inferred_monthly_duration: ProviderPaceDurationRule,
    pub shows_headroom_hint: bool,
}

impl ProviderPaceCapability {
    pub const UNSUPPORTED: Self = Self {
        reset_window_pace: ProviderPaceWindowRule::Unsupported,
        inferred_monthly_duration: ProviderPaceDurationRule::Unsupported,
        shows_headroom_hint: false,
    };

    pub const CALENDAR_MONTH_RESET_WINDOW: Self = Self {
        reset_window_pace: ProviderPaceWindowRule::WindowDuration(MONTHLY_WINDOW_SENTINEL_MINUTES),
        inferred_monthly_duration: ProviderPaceDurationRule::WindowDuration(
            MONTHLY_WINDOW_SENTINEL_MINUTES,
        ),
        shows_headroom_hint: false,
    };

    pub const fn unsupported() -> Self {
        Self::UNSUPPORTED
    }

    pub const fn calendar_month_reset_window() -> Self {
        Self::CALENDAR_MONTH_RESET_WINDOW
    }

    pub fn supports_reset_window_pace(&self, window: &RateWindow, _now: Timestamp) -> bool {
        match self.reset_window_pace {
            ProviderPaceWindowRule::Unsupported => false,
            ProviderPaceWindowRule::WindowDuration(m) => window.window_minutes == Some(m),
            ProviderPaceWindowRule::WindowDurationPresent => window.window_minutes.is_some(),
        }
    }

    pub fn uses_inferred_monthly_duration(&self, window: &RateWindow) -> bool {
        match self.inferred_monthly_duration {
            ProviderPaceDurationRule::Unsupported => false,
            ProviderPaceDurationRule::WindowDuration(m) => window.window_minutes == Some(m),
        }
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
