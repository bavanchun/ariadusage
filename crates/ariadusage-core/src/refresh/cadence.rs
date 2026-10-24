// Ported from CodexBar Sources/AdaptiveRefreshCore/AdaptiveRefreshPolicyCore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/AdaptiveRefreshPolicy.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/UsageStore+AdaptiveRefresh.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/BackgroundWorkPowerPolicy.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Sources/CodexBar/SettingsStore.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::time::Duration;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Refresh cadence mode configured by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum RefreshFrequency {
    #[serde(rename = "manual")]
    Manual,
    #[serde(rename = "1m", alias = "oneMinute")]
    OneMinute,
    #[serde(rename = "2m", alias = "twoMinutes")]
    TwoMinutes,
    #[default]
    #[serde(rename = "5m", alias = "fiveMinutes")]
    FiveMinutes,
    #[serde(rename = "15m", alias = "fifteenMinutes")]
    FifteenMinutes,
    #[serde(rename = "30m", alias = "thirtyMinutes")]
    ThirtyMinutes,
    #[serde(rename = "adaptive")]
    Adaptive,
    #[serde(rename = "adaptiveAgentAware", alias = "adaptive_agent_aware")]
    AdaptiveAgentAware,
}

impl RefreshFrequency {
    /// Stable canonical string used in settings snapshots and config persistence.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::OneMinute => "1m",
            Self::TwoMinutes => "2m",
            Self::FiveMinutes => "5m",
            Self::FifteenMinutes => "15m",
            Self::ThirtyMinutes => "30m",
            Self::Adaptive => "adaptive",
            Self::AdaptiveAgentAware => "adaptiveAgentAware",
        }
    }

    /// Parses a refresh cadence from either shorthand settings strings (`"5m"`)
    /// or CodexBar enum raw values (`"fiveMinutes"`).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "manual" => Some(Self::Manual),
            "1m" | "oneMinute" => Some(Self::OneMinute),
            "2m" | "twoMinutes" => Some(Self::TwoMinutes),
            "5m" | "fiveMinutes" => Some(Self::FiveMinutes),
            "15m" | "fifteenMinutes" => Some(Self::FifteenMinutes),
            "30m" | "thirtyMinutes" => Some(Self::ThirtyMinutes),
            "adaptive" => Some(Self::Adaptive),
            "adaptiveAgentAware" | "adaptive_agent_aware" => Some(Self::AdaptiveAgentAware),
            _ => None,
        }
    }

    /// Fixed interval in seconds, or `None` for `.Manual` and adaptive modes.
    pub fn seconds(&self) -> Option<u64> {
        match self {
            Self::Manual | Self::Adaptive | Self::AdaptiveAgentAware => None,
            Self::OneMinute => Some(60),
            Self::TwoMinutes => Some(120),
            Self::FiveMinutes => Some(300),
            Self::FifteenMinutes => Some(900),
            Self::ThirtyMinutes => Some(1800),
        }
    }

    /// Fixed interval as a `Duration`, or `None` for `.Manual` and adaptive modes.
    pub fn duration(&self) -> Option<Duration> {
        self.seconds().map(Duration::from_secs)
    }

    /// Whether this frequency uses `AdaptiveRefreshPolicyCore`.
    pub fn uses_adaptive_policy(&self) -> bool {
        matches!(self, Self::Adaptive | Self::AdaptiveAgentAware)
    }

    /// Whether this frequency incorporates local coding activity signals.
    pub fn uses_coding_activity(&self) -> bool {
        matches!(self, Self::AdaptiveAgentAware)
    }
}

/// System thermal state mapped for adaptive refresh decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThermalPressure {
    #[default]
    Nominal,
    Constrained,
}

/// Reason why `AdaptiveRefreshPolicyCore` chose a specific delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AdaptiveRefreshReason {
    RecentInteraction,
    Warm,
    Idle,
    LongIdle,
    CodingActivity,
    Constrained,
}

/// Inputs to a single adaptive refresh delay evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveRefreshInput {
    pub now: Timestamp,
    pub last_menu_open_at: Option<Timestamp>,
    pub last_coding_activity_at: Option<Timestamp>,
    pub low_power_mode_enabled: bool,
    pub thermal_pressure: ThermalPressure,
}

impl AdaptiveRefreshInput {
    pub fn new(
        now: Timestamp,
        last_menu_open_at: Option<Timestamp>,
        last_coding_activity_at: Option<Timestamp>,
        low_power_mode_enabled: bool,
        thermal_pressure: ThermalPressure,
    ) -> Self {
        Self {
            now,
            last_menu_open_at,
            last_coding_activity_at,
            low_power_mode_enabled,
            thermal_pressure,
        }
    }
}

/// Computed delay and reason from `AdaptiveRefreshPolicyCore`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdaptiveRefreshDecision {
    pub delay: Duration,
    pub reason: AdaptiveRefreshReason,
}

impl AdaptiveRefreshDecision {
    pub const fn new(delay: Duration, reason: AdaptiveRefreshReason) -> Self {
        Self { delay, reason }
    }
}

/// Pure, platform-neutral adaptive refresh policy.
#[derive(Debug, Clone, Copy, Default)]
pub struct AdaptiveRefreshPolicyCore;

impl AdaptiveRefreshPolicyCore {
    pub const RECENT_INTERACTION_THRESHOLD_SECS: f64 = 5.0 * 60.0;
    pub const WARM_THRESHOLD_SECS: f64 = 60.0 * 60.0;
    pub const IDLE_THRESHOLD_SECS: f64 = 4.0 * 60.0 * 60.0;
    pub const CODING_ACTIVITY_THRESHOLD_SECS: f64 = 5.0 * 60.0;

    pub const RECENT_INTERACTION_DELAY: Duration = Duration::from_secs(2 * 60);
    pub const WARM_DELAY: Duration = Duration::from_secs(5 * 60);
    pub const IDLE_DELAY: Duration = Duration::from_secs(15 * 60);
    pub const LONG_IDLE_DELAY: Duration = Duration::from_secs(30 * 60);
    pub const CONSTRAINED_DELAY: Duration = Duration::from_secs(30 * 60);
    pub const CODING_ACTIVITY_DELAY_CAP: Duration = Duration::from_secs(5 * 60);

    /// Fallback nominal interval supplied to downstream consumers that expect a single interval
    /// (for example reset-boundary scheduling heuristics) when the user is in adaptive mode.
    pub const NOMINAL_INTERVAL_FOR_HEURISTICS_SECS: u64 = 5 * 60;
    pub const NOMINAL_INTERVAL_FOR_HEURISTICS: Duration =
        Duration::from_secs(Self::NOMINAL_INTERVAL_FOR_HEURISTICS_SECS);

    pub fn next_delay(&self, input: &AdaptiveRefreshInput) -> AdaptiveRefreshDecision {
        if input.low_power_mode_enabled || input.thermal_pressure == ThermalPressure::Constrained {
            return AdaptiveRefreshDecision::new(
                Self::CONSTRAINED_DELAY,
                AdaptiveRefreshReason::Constrained,
            );
        }

        let base_decision = match input.last_menu_open_at {
            None => {
                AdaptiveRefreshDecision::new(Self::LONG_IDLE_DELAY, AdaptiveRefreshReason::LongIdle)
            }
            Some(last_menu_open_at) => {
                // Negative ages (clock adjustment) read as recent rather than falling through to
                // the long-idle bucket.
                let age_secs = input.now.duration_since(last_menu_open_at).as_secs_f64();
                if age_secs <= Self::RECENT_INTERACTION_THRESHOLD_SECS {
                    AdaptiveRefreshDecision::new(
                        Self::RECENT_INTERACTION_DELAY,
                        AdaptiveRefreshReason::RecentInteraction,
                    )
                } else if age_secs <= Self::WARM_THRESHOLD_SECS {
                    AdaptiveRefreshDecision::new(Self::WARM_DELAY, AdaptiveRefreshReason::Warm)
                } else if age_secs < Self::IDLE_THRESHOLD_SECS {
                    AdaptiveRefreshDecision::new(Self::IDLE_DELAY, AdaptiveRefreshReason::Idle)
                } else {
                    AdaptiveRefreshDecision::new(
                        Self::LONG_IDLE_DELAY,
                        AdaptiveRefreshReason::LongIdle,
                    )
                }
            }
        };

        if let Some(last_coding_activity_at) = input.last_coding_activity_at {
            let coding_age_secs = input
                .now
                .duration_since(last_coding_activity_at)
                .as_secs_f64();
            if coding_age_secs < Self::CODING_ACTIVITY_THRESHOLD_SECS
                && base_decision.delay > Self::CODING_ACTIVITY_DELAY_CAP
            {
                return AdaptiveRefreshDecision::new(
                    Self::CODING_ACTIVITY_DELAY_CAP,
                    AdaptiveRefreshReason::CodingActivity,
                );
            }
        }

        base_decision
    }
}

/// Power policy that clamps automatic background refresh intervals when low-power mode is enabled.
#[derive(Debug, Clone, Copy, Default)]
pub struct BackgroundWorkPowerPolicy;

impl BackgroundWorkPowerPolicy {
    pub const LOW_POWER_MINIMUM_INTERVAL: Duration = Duration::from_secs(30 * 60);

    pub fn automatic_interval(
        preferred_interval: Option<Duration>,
        low_power_mode_enabled: bool,
    ) -> Option<Duration> {
        let interval = preferred_interval?;
        if low_power_mode_enabled {
            Some(interval.max(Self::LOW_POWER_MINIMUM_INTERVAL))
        } else {
            Some(interval)
        }
    }
}

/// Advances a fixed timer from its previous scheduled tick rather than refresh completion time,
/// skipping any missed ticks when a refresh overruns its interval.
pub fn next_fixed_timer_scheduled_at(
    previous_scheduled_at: Duration,
    completed_at: Duration,
    interval: Duration,
) -> Duration {
    if interval.is_zero() {
        return completed_at;
    }
    let mut next_scheduled_at = previous_scheduled_at.saturating_add(interval);
    while next_scheduled_at <= completed_at {
        next_scheduled_at = next_scheduled_at.saturating_add(interval);
    }
    next_scheduled_at
}

/// Returns `true` when an adaptive timer should advance to an earlier `candidate` wake time,
/// without ever postponing an already-earlier scheduled wake.
pub fn should_advance_adaptive_timer(
    scheduled_at: Option<Timestamp>,
    candidate: Timestamp,
) -> bool {
    match scheduled_at {
        Some(scheduled) => candidate < scheduled,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE_EPOCH_SECS: i64 = 1_778_371_200;

    fn reference_now() -> Timestamp {
        Timestamp::from_second(REFERENCE_EPOCH_SECS).expect("valid timestamp")
    }

    fn make_input(
        age_secs: Option<i64>,
        coding_activity_age_secs: Option<i64>,
        low_power_mode_enabled: bool,
        thermal_pressure: ThermalPressure,
    ) -> AdaptiveRefreshInput {
        let now = reference_now();
        AdaptiveRefreshInput {
            now,
            last_menu_open_at: age_secs.map(|age| {
                Timestamp::from_second(REFERENCE_EPOCH_SECS - age).expect("valid timestamp")
            }),
            last_coding_activity_at: coding_activity_age_secs.map(|age| {
                Timestamp::from_second(REFERENCE_EPOCH_SECS - age).expect("valid timestamp")
            }),
            low_power_mode_enabled,
            thermal_pressure,
        }
    }

    // upstream: Tests/AdaptiveReplayKitTests/AdaptiveRefreshPolicyCoreTests.swift:23 age determines the canonical table boundary
    #[test]
    fn age_determines_canonical_table_boundary() {
        let cases = [
            (-600, AdaptiveRefreshReason::RecentInteraction, 120u64),
            (0, AdaptiveRefreshReason::RecentInteraction, 120),
            (299, AdaptiveRefreshReason::RecentInteraction, 120),
            (300, AdaptiveRefreshReason::RecentInteraction, 120),
            (301, AdaptiveRefreshReason::Warm, 300),
            (3599, AdaptiveRefreshReason::Warm, 300),
            (3600, AdaptiveRefreshReason::Warm, 300),
            (3601, AdaptiveRefreshReason::Idle, 900),
            (14399, AdaptiveRefreshReason::Idle, 900),
            (14400, AdaptiveRefreshReason::LongIdle, 1800),
            (100_000, AdaptiveRefreshReason::LongIdle, 1800),
        ];

        let policy = AdaptiveRefreshPolicyCore;
        for (age_secs, expected_reason, expected_delay_secs) in cases {
            let decision = policy.next_delay(&make_input(
                Some(age_secs),
                None,
                false,
                ThermalPressure::Nominal,
            ));
            assert_eq!(decision.reason, expected_reason, "age={age_secs}");
            assert_eq!(
                decision.delay,
                Duration::from_secs(expected_delay_secs),
                "age={age_secs}"
            );
        }
    }

    // upstream: Tests/AdaptiveReplayKitTests/AdaptiveRefreshPolicyCoreTests.swift:46 nil last menu open is long idle
    #[test]
    fn nil_last_menu_open_is_long_idle() {
        let decision = AdaptiveRefreshPolicyCore.next_delay(&make_input(
            None,
            None,
            false,
            ThermalPressure::Nominal,
        ));
        assert_eq!(decision.reason, AdaptiveRefreshReason::LongIdle);
        assert_eq!(decision.delay, Duration::from_secs(30 * 60));
    }

    // upstream: Tests/AdaptiveReplayKitTests/AdaptiveRefreshPolicyCoreTests.swift:53 low power mode wins over recent interaction
    #[test]
    fn low_power_mode_wins_over_recent_interaction() {
        let decision = AdaptiveRefreshPolicyCore.next_delay(&make_input(
            Some(0),
            None,
            true,
            ThermalPressure::Nominal,
        ));
        assert_eq!(decision.reason, AdaptiveRefreshReason::Constrained);
        assert_eq!(decision.delay, Duration::from_secs(30 * 60));
    }

    // upstream: Tests/AdaptiveReplayKitTests/AdaptiveRefreshPolicyCoreTests.swift:62 thermal pressure wins when no menu open is recorded
    #[test]
    fn thermal_pressure_wins_when_no_menu_open_is_recorded() {
        let decision = AdaptiveRefreshPolicyCore.next_delay(&make_input(
            None,
            None,
            false,
            ThermalPressure::Constrained,
        ));
        assert_eq!(decision.reason, AdaptiveRefreshReason::Constrained);
        assert_eq!(decision.delay, Duration::from_secs(30 * 60));
    }

    // upstream: Tests/AdaptiveReplayKitTests/AdaptiveRefreshPolicyCoreTests.swift:71 recent coding activity caps slower menu decisions at five minutes
    #[test]
    fn recent_coding_activity_caps_slower_menu_decisions_at_five_minutes() {
        for age_secs in [3601, 14400, 100_000] {
            let decision = AdaptiveRefreshPolicyCore.next_delay(&make_input(
                Some(age_secs),
                Some(0),
                false,
                ThermalPressure::Nominal,
            ));
            assert_eq!(decision.reason, AdaptiveRefreshReason::CodingActivity);
            assert_eq!(decision.delay, Duration::from_secs(5 * 60));
        }
    }

    // upstream: Tests/AdaptiveReplayKitTests/AdaptiveRefreshPolicyCoreTests.swift:80 coding activity does not lengthen recent or warm decisions
    #[test]
    fn coding_activity_does_not_lengthen_recent_or_warm_decisions() {
        let recent = AdaptiveRefreshPolicyCore.next_delay(&make_input(
            Some(0),
            Some(0),
            false,
            ThermalPressure::Nominal,
        ));
        let warm = AdaptiveRefreshPolicyCore.next_delay(&make_input(
            Some(301),
            Some(0),
            false,
            ThermalPressure::Nominal,
        ));
        assert_eq!(recent.reason, AdaptiveRefreshReason::RecentInteraction);
        assert_eq!(recent.delay, Duration::from_secs(2 * 60));
        assert_eq!(warm.reason, AdaptiveRefreshReason::Warm);
        assert_eq!(warm.delay, Duration::from_secs(5 * 60));
    }

    // upstream: Tests/AdaptiveReplayKitTests/AdaptiveRefreshPolicyCoreTests.swift:94 constraints win and the coding activity boundary is exclusive
    #[test]
    fn constraints_win_and_coding_activity_boundary_is_exclusive() {
        let constrained = AdaptiveRefreshPolicyCore.next_delay(&make_input(
            None,
            Some(0),
            true,
            ThermalPressure::Nominal,
        ));
        let inside_boundary = AdaptiveRefreshPolicyCore.next_delay(&make_input(
            None,
            Some(299),
            false,
            ThermalPressure::Nominal,
        ));
        let at_boundary = AdaptiveRefreshPolicyCore.next_delay(&make_input(
            None,
            Some(300),
            false,
            ThermalPressure::Nominal,
        ));
        assert_eq!(constrained.reason, AdaptiveRefreshReason::Constrained);
        assert_eq!(constrained.delay, Duration::from_secs(30 * 60));
        assert_eq!(
            inside_boundary.reason,
            AdaptiveRefreshReason::CodingActivity
        );
        assert_eq!(inside_boundary.delay, Duration::from_secs(5 * 60));
        assert_eq!(at_boundary.reason, AdaptiveRefreshReason::LongIdle);
        assert_eq!(at_boundary.delay, Duration::from_secs(30 * 60));
    }

    // upstream: Tests/AdaptiveReplayKitTests/AdaptiveRefreshPolicyCoreTests.swift:114 future timestamps read as recent
    #[test]
    fn future_timestamps_read_as_recent() {
        let decision = AdaptiveRefreshPolicyCore.next_delay(&make_input(
            Some(-1_000_000),
            None,
            false,
            ThermalPressure::Nominal,
        ));
        assert_eq!(decision.reason, AdaptiveRefreshReason::RecentInteraction);
        assert_eq!(decision.delay, Duration::from_secs(2 * 60));
    }

    // upstream: Tests/AdaptiveReplayKitTests/AdaptiveRefreshPolicyCoreTests.swift:121 every decision stays within the two to thirty minute bounds
    #[test]
    fn every_decision_stays_within_two_to_thirty_minute_bounds() {
        let ages = [
            None,
            Some(-1_000_000),
            Some(0),
            Some(300),
            Some(301),
            Some(3600),
            Some(3601),
            Some(14399),
            Some(14400),
            Some(1_000_000),
        ];
        for age in ages {
            for low_power in [false, true] {
                for thermal in [ThermalPressure::Nominal, ThermalPressure::Constrained] {
                    let decision = AdaptiveRefreshPolicyCore
                        .next_delay(&make_input(age, None, low_power, thermal));
                    assert!(decision.delay >= Duration::from_secs(2 * 60));
                    assert!(decision.delay <= Duration::from_secs(30 * 60));
                }
            }
        }
    }

    // upstream: Tests/AdaptiveReplayKitTests/AdaptiveRefreshPolicyCoreTests.swift:141 nominal heuristic interval remains five minutes
    #[test]
    fn nominal_heuristic_interval_remains_five_minutes() {
        assert_eq!(
            AdaptiveRefreshPolicyCore::NOMINAL_INTERVAL_FOR_HEURISTICS_SECS,
            5 * 60
        );
    }

    // upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:266 fixed cadence advances from scheduled tick instead of refresh completion
    #[test]
    fn fixed_cadence_advances_from_scheduled_tick_instead_of_refresh_completion() {
        let interval = Duration::from_millis(100);
        let first_scheduled_at = Duration::from_millis(100);

        assert_eq!(
            next_fixed_timer_scheduled_at(first_scheduled_at, first_scheduled_at, interval),
            Duration::from_millis(200)
        );
        assert_eq!(
            next_fixed_timer_scheduled_at(
                first_scheduled_at,
                first_scheduled_at + Duration::from_millis(100) - Duration::from_nanos(1),
                interval
            ),
            Duration::from_millis(200)
        );
        assert_eq!(
            next_fixed_timer_scheduled_at(
                first_scheduled_at,
                first_scheduled_at + Duration::from_millis(100),
                interval
            ),
            Duration::from_millis(300)
        );
        assert_eq!(
            next_fixed_timer_scheduled_at(
                first_scheduled_at,
                first_scheduled_at + Duration::from_millis(60),
                interval
            ),
            Duration::from_millis(200)
        );
        assert_eq!(
            next_fixed_timer_scheduled_at(
                first_scheduled_at,
                first_scheduled_at + Duration::from_millis(260),
                interval
            ),
            Duration::from_millis(400)
        );
    }

    #[test]
    fn adaptive_timer_advances_only_for_earlier_candidate() {
        let base = reference_now();
        let earlier = Timestamp::from_second(REFERENCE_EPOCH_SECS - 60).expect("valid timestamp");
        let later = Timestamp::from_second(REFERENCE_EPOCH_SECS + 60).expect("valid timestamp");

        assert!(should_advance_adaptive_timer(None, base));
        assert!(should_advance_adaptive_timer(Some(base), earlier));
        assert!(!should_advance_adaptive_timer(Some(base), base));
        assert!(!should_advance_adaptive_timer(Some(base), later));
    }

    #[test]
    fn background_work_power_policy_clamps_in_low_power_mode() {
        assert_eq!(
            BackgroundWorkPowerPolicy::automatic_interval(Some(Duration::from_secs(300)), false),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            BackgroundWorkPowerPolicy::automatic_interval(Some(Duration::from_secs(300)), true),
            Some(Duration::from_secs(1800))
        );
        assert_eq!(
            BackgroundWorkPowerPolicy::automatic_interval(None, true),
            None
        );
    }
}
