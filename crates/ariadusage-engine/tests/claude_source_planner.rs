// Ported from CodexBar Tests/CodexBarTests/ClaudeSourcePlannerTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::pipeline::FetchRuntime;
use ariadusage_core::providers::SourceMode;
use ariadusage_engine::providers::claude::{
    ClaudeSourcePlanner, ClaudeSourcePlanningInput, ClaudeSourceSelectedStrategy,
    ClaudeSourceSelectionReason, ClaudeUsageStrategy,
};

#[test]
fn app_auto_plan_preserves_ordered_steps_and_reasons() {
    let plan = ClaudeSourcePlanner::resolve(ClaudeSourcePlanningInput {
        runtime: FetchRuntime::Daemon,
        selected_data_source: SourceMode::Auto,
        web_extras_enabled: false,
        has_web_session: true,
        has_cli: true,
        has_oauth_credentials: true,
    });

    assert_eq!(
        plan.ordered_steps
            .iter()
            .map(|s| s.data_source)
            .collect::<Vec<_>>(),
        vec![
            ClaudeSourceSelectedStrategy::Oauth,
            ClaudeSourceSelectedStrategy::Cli,
            ClaudeSourceSelectedStrategy::Web,
        ]
    );
    assert_eq!(
        plan.ordered_steps
            .iter()
            .map(|s| s.inclusion_reason)
            .collect::<Vec<_>>(),
        vec![
            ClaudeSourceSelectionReason::AppAutoPreferredOAuth,
            ClaudeSourceSelectionReason::AppAutoFallbackCli,
            ClaudeSourceSelectionReason::AppAutoFallbackWeb,
        ]
    );
    assert_eq!(
        plan.available_steps()
            .into_iter()
            .map(|s| s.data_source)
            .collect::<Vec<_>>(),
        vec![
            ClaudeSourceSelectedStrategy::Oauth,
            ClaudeSourceSelectedStrategy::Cli,
            ClaudeSourceSelectedStrategy::Web,
        ]
    );
    assert_eq!(
        plan.preferred_step().map(|s| s.data_source),
        Some(ClaudeSourceSelectedStrategy::Oauth)
    );
}

#[test]
fn cli_auto_plan_preserves_ordered_steps_and_reasons() {
    let plan = ClaudeSourcePlanner::resolve(ClaudeSourcePlanningInput {
        runtime: FetchRuntime::Cli,
        selected_data_source: SourceMode::Auto,
        web_extras_enabled: false,
        has_web_session: true,
        has_cli: true,
        has_oauth_credentials: false,
    });

    assert_eq!(
        plan.ordered_steps
            .iter()
            .map(|s| s.data_source)
            .collect::<Vec<_>>(),
        vec![
            ClaudeSourceSelectedStrategy::Web,
            ClaudeSourceSelectedStrategy::Cli,
        ]
    );
    assert_eq!(
        plan.ordered_steps
            .iter()
            .map(|s| s.inclusion_reason)
            .collect::<Vec<_>>(),
        vec![
            ClaudeSourceSelectionReason::CliAutoPreferredWeb,
            ClaudeSourceSelectionReason::CliAutoFallbackCli,
        ]
    );
    assert_eq!(
        plan.preferred_step().map(|s| s.data_source),
        Some(ClaudeSourceSelectedStrategy::Web)
    );
}

#[test]
fn cli_auto_plan_prefers_oauth_over_cli_when_web_session_is_unavailable() {
    let plan = ClaudeSourcePlanner::resolve(ClaudeSourcePlanningInput {
        runtime: FetchRuntime::Cli,
        selected_data_source: SourceMode::Auto,
        web_extras_enabled: false,
        has_web_session: false,
        has_cli: true,
        has_oauth_credentials: true,
    });

    assert_eq!(
        plan.ordered_steps
            .iter()
            .map(|s| s.inclusion_reason)
            .collect::<Vec<_>>(),
        vec![
            ClaudeSourceSelectionReason::CliAutoPreferredWeb,
            ClaudeSourceSelectionReason::CliAutoFallbackOAuth,
            ClaudeSourceSelectionReason::CliAutoFallbackCli,
        ]
    );
    assert_eq!(
        plan.available_steps()
            .into_iter()
            .map(|s| s.data_source)
            .collect::<Vec<_>>(),
        vec![
            ClaudeSourceSelectedStrategy::Oauth,
            ClaudeSourceSelectedStrategy::Cli,
        ]
    );
    assert_eq!(
        plan.preferred_step().map(|s| s.data_source),
        Some(ClaudeSourceSelectedStrategy::Oauth)
    );
}

#[test]
fn explicit_mode_plan_is_single_step() {
    let plan = ClaudeSourcePlanner::resolve(ClaudeSourcePlanningInput {
        runtime: FetchRuntime::Daemon,
        selected_data_source: SourceMode::Cli,
        web_extras_enabled: true,
        has_web_session: false,
        has_cli: true,
        has_oauth_credentials: false,
    });

    assert_eq!(plan.ordered_steps.len(), 1);
    assert_eq!(
        plan.ordered_steps.first().map(|s| s.data_source),
        Some(ClaudeSourceSelectedStrategy::Cli)
    );
    assert_eq!(
        plan.ordered_steps.first().map(|s| s.inclusion_reason),
        Some(ClaudeSourceSelectionReason::ExplicitSourceSelection)
    );
    assert_eq!(
        plan.compatibility_strategy(),
        Some(ClaudeUsageStrategy {
            data_source: ClaudeSourceSelectedStrategy::Cli,
            use_web_extras: true,
        })
    );
}

#[test]
fn app_explicit_oauth_selects_owner_mediated_cli_when_direct_credentials_are_unavailable() {
    let plan = ClaudeSourcePlanner::resolve(ClaudeSourcePlanningInput {
        runtime: FetchRuntime::Daemon,
        selected_data_source: SourceMode::Oauth,
        web_extras_enabled: false,
        has_web_session: true,
        has_cli: true,
        has_oauth_credentials: false,
    });

    assert_eq!(
        plan.ordered_steps
            .iter()
            .map(|s| s.data_source)
            .collect::<Vec<_>>(),
        vec![
            ClaudeSourceSelectedStrategy::Oauth,
            ClaudeSourceSelectedStrategy::Cli,
        ]
    );
    assert_eq!(
        plan.ordered_steps
            .iter()
            .map(|s| s.inclusion_reason)
            .collect::<Vec<_>>(),
        vec![
            ClaudeSourceSelectionReason::ExplicitSourceSelection,
            ClaudeSourceSelectionReason::ExplicitOAuthOwnerCliFallback,
        ]
    );
    assert_eq!(
        plan.compatibility_strategy(),
        Some(ClaudeUsageStrategy {
            data_source: ClaudeSourceSelectedStrategy::Cli,
            use_web_extras: false,
        })
    );
}

#[test]
fn app_explicit_oauth_selects_direct_oauth_when_credentials_are_available() {
    let plan = ClaudeSourcePlanner::resolve(ClaudeSourcePlanningInput {
        runtime: FetchRuntime::Daemon,
        selected_data_source: SourceMode::Oauth,
        web_extras_enabled: false,
        has_web_session: true,
        has_cli: true,
        has_oauth_credentials: true,
    });

    assert_eq!(
        plan.ordered_steps
            .iter()
            .map(|s| s.data_source)
            .collect::<Vec<_>>(),
        vec![
            ClaudeSourceSelectedStrategy::Oauth,
            ClaudeSourceSelectedStrategy::Cli,
        ]
    );
    assert_eq!(
        plan.compatibility_strategy(),
        Some(ClaudeUsageStrategy {
            data_source: ClaudeSourceSelectedStrategy::Oauth,
            use_web_extras: false,
        })
    );
}

#[test]
fn cli_explicit_oauth_remains_terminal() {
    let plan = ClaudeSourcePlanner::resolve(ClaudeSourcePlanningInput {
        runtime: FetchRuntime::Cli,
        selected_data_source: SourceMode::Oauth,
        web_extras_enabled: false,
        has_web_session: true,
        has_cli: true,
        has_oauth_credentials: false,
    });

    assert_eq!(
        plan.ordered_steps
            .iter()
            .map(|s| s.data_source)
            .collect::<Vec<_>>(),
        vec![ClaudeSourceSelectedStrategy::Oauth]
    );
    assert_eq!(
        plan.ordered_steps
            .iter()
            .map(|s| s.inclusion_reason)
            .collect::<Vec<_>>(),
        vec![ClaudeSourceSelectionReason::ExplicitSourceSelection]
    );
}

#[test]
fn app_auto_cli_fallback_reports_web_extras_like_runtime() {
    let plan = ClaudeSourcePlanner::resolve(ClaudeSourcePlanningInput {
        runtime: FetchRuntime::Daemon,
        selected_data_source: SourceMode::Auto,
        web_extras_enabled: true,
        has_web_session: false,
        has_cli: true,
        has_oauth_credentials: false,
    });

    assert_eq!(
        plan.preferred_step().map(|s| s.data_source),
        Some(ClaudeSourceSelectedStrategy::Cli)
    );
    assert_eq!(
        plan.compatibility_strategy(),
        Some(ClaudeUsageStrategy {
            data_source: ClaudeSourceSelectedStrategy::Cli,
            use_web_extras: true,
        })
    );
}

#[test]
fn no_source_planner_output_is_deterministic() {
    let input = ClaudeSourcePlanningInput {
        runtime: FetchRuntime::Daemon,
        selected_data_source: SourceMode::Auto,
        web_extras_enabled: false,
        has_web_session: false,
        has_cli: false,
        has_oauth_credentials: false,
    };
    let plan = ClaudeSourcePlanner::resolve(input);

    assert_eq!(
        plan.ordered_steps
            .iter()
            .map(|s| s.data_source)
            .collect::<Vec<_>>(),
        vec![
            ClaudeSourceSelectedStrategy::Oauth,
            ClaudeSourceSelectedStrategy::Cli,
            ClaudeSourceSelectedStrategy::Web,
        ]
    );
    assert!(plan.available_steps().is_empty());
    assert!(plan.is_no_source_available());
    assert_eq!(plan.preferred_step(), None);
    assert!(plan.execution_steps().is_empty());
    assert_eq!(
        plan.debug_lines(),
        vec![
            "planner_order=oauth→cli→web",
            "planner_selected=none",
            "planner_no_source=true",
            "planner_step.oauth=unavailable reason=app-auto-preferred-oauth",
            "planner_step.cli=unavailable reason=app-auto-fallback-cli",
            "planner_step.web=unavailable reason=app-auto-fallback-web",
        ]
    );
}
