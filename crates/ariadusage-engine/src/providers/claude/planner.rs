// Ported from CodexBar Sources/CodexBarCore/Providers/Claude/ClaudeSourcePlanner.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::pipeline::FetchRuntime;
use ariadusage_core::providers::SourceMode;

/// Executable strategy data source selected by `ClaudeSourcePlanner`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaudeSourceSelectedStrategy {
    Api,
    Oauth,
    Web,
    Cli,
}

impl ClaudeSourceSelectedStrategy {
    /// Strategy identifier used by `FetchStrategy::id`.
    pub const fn strategy_id(self) -> &'static str {
        match self {
            Self::Api => "claude.admin-api",
            Self::Oauth => "claude.oauth",
            Self::Web => "claude.web",
            Self::Cli => "claude.cli",
        }
    }

    /// Canonical wire/debug label for this data source.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Oauth => "oauth",
            Self::Web => "web",
            Self::Cli => "cli",
        }
    }
}

/// Reason a step was included in a `ClaudeSourcePlan`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaudeSourceSelectionReason {
    ExplicitSourceSelection,
    ExplicitOAuthOwnerCliFallback,
    AppAutoPreferredOAuth,
    AppAutoFallbackCli,
    AppAutoFallbackWeb,
    CliAutoPreferredWeb,
    CliAutoFallbackOAuth,
    CliAutoFallbackCli,
}

impl ClaudeSourceSelectionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitSourceSelection => "explicit-source-selection",
            Self::ExplicitOAuthOwnerCliFallback => "explicit-oauth-owner-cli-fallback",
            Self::AppAutoPreferredOAuth => "app-auto-preferred-oauth",
            Self::AppAutoFallbackCli => "app-auto-fallback-cli",
            Self::AppAutoFallbackWeb => "app-auto-fallback-web",
            Self::CliAutoPreferredWeb => "cli-auto-preferred-web",
            Self::CliAutoFallbackOAuth => "cli-auto-fallback-oauth",
            Self::CliAutoFallbackCli => "cli-auto-fallback-cli",
        }
    }
}

/// Inputs governing Claude strategy ordering across daemon (`App`) and one-shot `Cli` runtimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeSourcePlanningInput {
    pub runtime: FetchRuntime,
    pub selected_data_source: SourceMode,
    pub web_extras_enabled: bool,
    pub has_web_session: bool,
    pub has_cli: bool,
    pub has_oauth_credentials: bool,
}

/// One planned fetch step produced by `ClaudeSourcePlanner`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeFetchPlanStep {
    pub data_source: ClaudeSourceSelectedStrategy,
    pub inclusion_reason: ClaudeSourceSelectionReason,
    pub is_plausibly_available: bool,
}

/// Strategy summary selected by the compatibility resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaudeUsageStrategy {
    pub data_source: ClaudeSourceSelectedStrategy,
    pub use_web_extras: bool,
}

/// Resolved ordered strategy plan for Claude usage fetching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSourcePlan {
    pub input: ClaudeSourcePlanningInput,
    pub ordered_steps: Vec<ClaudeFetchPlanStep>,
}

impl ClaudeSourcePlan {
    /// Steps in `ordered_steps` that are plausibly available.
    pub fn available_steps(&self) -> Vec<ClaudeFetchPlanStep> {
        self.ordered_steps
            .iter()
            .filter(|s| s.is_plausibly_available)
            .copied()
            .collect()
    }

    /// True when none of `ordered_steps` is plausibly available.
    pub fn is_no_source_available(&self) -> bool {
        !self.ordered_steps.iter().any(|s| s.is_plausibly_available)
    }

    /// Returns the preferred step according to mode and runtime rules.
    pub fn preferred_step(&self) -> Option<&ClaudeFetchPlanStep> {
        match self.input.selected_data_source {
            SourceMode::Auto => self.ordered_steps.iter().find(|s| s.is_plausibly_available),
            SourceMode::Oauth if self.input.runtime == FetchRuntime::Daemon => {
                self.ordered_steps.iter().find(|s| s.is_plausibly_available)
            }
            SourceMode::Api | SourceMode::Oauth | SourceMode::Web | SourceMode::Cli => {
                self.ordered_steps.first()
            }
        }
    }

    /// Steps to execute for the selected data source mode.
    pub fn execution_steps(&self) -> Vec<ClaudeFetchPlanStep> {
        match self.input.selected_data_source {
            SourceMode::Auto => self.available_steps(),
            SourceMode::Api | SourceMode::Oauth | SourceMode::Web | SourceMode::Cli => {
                self.ordered_steps.clone()
            }
        }
    }

    /// Derives the compatibility strategy from `preferred_step`.
    pub fn compatibility_strategy(&self) -> Option<ClaudeUsageStrategy> {
        let preferred_step = self.preferred_step()?;
        let use_web_extras = self.input.runtime == FetchRuntime::Daemon
            && preferred_step.data_source == ClaudeSourceSelectedStrategy::Cli
            && self.input.web_extras_enabled;
        Some(ClaudeUsageStrategy {
            data_source: preferred_step.data_source,
            use_web_extras,
        })
    }

    /// Arrow-joined order label (e.g. `oauth→cli→web`).
    pub fn order_label(&self) -> String {
        self.ordered_steps
            .iter()
            .map(|s| s.data_source.as_str())
            .collect::<Vec<_>>()
            .join("→")
    }

    /// Formats deterministic debug lines matching `ClaudeFetchPlan.debugLines()`.
    pub fn debug_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("planner_order={}", self.order_label())];
        let selected = self
            .preferred_step()
            .map(|s| s.data_source.as_str())
            .unwrap_or("none");
        lines.push(format!("planner_selected={selected}"));
        lines.push(format!(
            "planner_no_source={}",
            self.is_no_source_available()
        ));
        for step in &self.ordered_steps {
            let availability = if step.is_plausibly_available {
                "available"
            } else {
                "unavailable"
            };
            lines.push(format!(
                "planner_step.{}={availability} reason={}",
                step.data_source.as_str(),
                step.inclusion_reason.as_str()
            ));
        }
        lines
    }
}

/// Resolves the ordered Claude fetch plan for a given runtime and `SourceMode`.
pub struct ClaudeSourcePlanner;

impl ClaudeSourcePlanner {
    pub fn resolve(input: ClaudeSourcePlanningInput) -> ClaudeSourcePlan {
        let ordered_steps = Self::ordered_steps(&input);
        ClaudeSourcePlan {
            input,
            ordered_steps,
        }
    }

    fn ordered_steps(input: &ClaudeSourcePlanningInput) -> Vec<ClaudeFetchPlanStep> {
        match input.selected_data_source {
            SourceMode::Auto => match input.runtime {
                FetchRuntime::Daemon => vec![
                    Self::step(
                        ClaudeSourceSelectedStrategy::Oauth,
                        ClaudeSourceSelectionReason::AppAutoPreferredOAuth,
                        input,
                    ),
                    Self::step(
                        ClaudeSourceSelectedStrategy::Cli,
                        ClaudeSourceSelectionReason::AppAutoFallbackCli,
                        input,
                    ),
                    Self::step(
                        ClaudeSourceSelectedStrategy::Web,
                        ClaudeSourceSelectionReason::AppAutoFallbackWeb,
                        input,
                    ),
                ],
                FetchRuntime::Cli => {
                    let mut steps = vec![Self::step(
                        ClaudeSourceSelectedStrategy::Web,
                        ClaudeSourceSelectionReason::CliAutoPreferredWeb,
                        input,
                    )];
                    if input.has_oauth_credentials {
                        steps.push(Self::step(
                            ClaudeSourceSelectedStrategy::Oauth,
                            ClaudeSourceSelectionReason::CliAutoFallbackOAuth,
                            input,
                        ));
                    }
                    steps.push(Self::step(
                        ClaudeSourceSelectedStrategy::Cli,
                        ClaudeSourceSelectionReason::CliAutoFallbackCli,
                        input,
                    ));
                    steps
                }
            },
            SourceMode::Api => vec![Self::step(
                ClaudeSourceSelectedStrategy::Api,
                ClaudeSourceSelectionReason::ExplicitSourceSelection,
                input,
            )],
            SourceMode::Oauth => match input.runtime {
                FetchRuntime::Daemon => vec![
                    Self::step(
                        ClaudeSourceSelectedStrategy::Oauth,
                        ClaudeSourceSelectionReason::ExplicitSourceSelection,
                        input,
                    ),
                    Self::step(
                        ClaudeSourceSelectedStrategy::Cli,
                        ClaudeSourceSelectionReason::ExplicitOAuthOwnerCliFallback,
                        input,
                    ),
                ],
                FetchRuntime::Cli => vec![Self::step(
                    ClaudeSourceSelectedStrategy::Oauth,
                    ClaudeSourceSelectionReason::ExplicitSourceSelection,
                    input,
                )],
            },
            SourceMode::Web => vec![Self::step(
                ClaudeSourceSelectedStrategy::Web,
                ClaudeSourceSelectionReason::ExplicitSourceSelection,
                input,
            )],
            SourceMode::Cli => vec![Self::step(
                ClaudeSourceSelectedStrategy::Cli,
                ClaudeSourceSelectionReason::ExplicitSourceSelection,
                input,
            )],
        }
    }

    fn step(
        data_source: ClaudeSourceSelectedStrategy,
        inclusion_reason: ClaudeSourceSelectionReason,
        input: &ClaudeSourcePlanningInput,
    ) -> ClaudeFetchPlanStep {
        let is_plausibly_available = match data_source {
            ClaudeSourceSelectedStrategy::Api => false,
            ClaudeSourceSelectedStrategy::Oauth => input.has_oauth_credentials,
            ClaudeSourceSelectedStrategy::Web => input.has_web_session,
            ClaudeSourceSelectedStrategy::Cli => input.has_cli,
        };
        ClaudeFetchPlanStep {
            data_source,
            inclusion_reason,
            is_plausibly_available,
        }
    }
}
