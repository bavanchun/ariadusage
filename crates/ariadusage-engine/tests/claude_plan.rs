// Ported from CodexBar Tests/CodexBarTests/ClaudePlanResolverTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_engine::providers::claude::ClaudePlan;

#[test]
fn oauth_rate_limit_tier_maps_to_branded_plan() {
    assert_eq!(
        ClaudePlan::oauth_login_method(None, Some("claude_pro")).as_deref(),
        Some("Claude Pro")
    );
    assert_eq!(
        ClaudePlan::oauth_login_method(None, Some("claude_team")).as_deref(),
        Some("Claude Team")
    );
    assert_eq!(
        ClaudePlan::oauth_login_method(None, Some("claude_enterprise")).as_deref(),
        Some("Claude Enterprise")
    );
}

#[test]
fn oauth_rate_limit_tier_preserves_the_max_usage_multiplier() {
    assert_eq!(
        ClaudePlan::oauth_login_method(None, Some("default_claude_max_5x")).as_deref(),
        Some("Claude Max 5x")
    );
    assert_eq!(
        ClaudePlan::oauth_login_method(None, Some("default_claude_max_20x")).as_deref(),
        Some("Claude Max 20x")
    );
    assert_eq!(
        ClaudePlan::oauth_login_method(None, Some("v2_default_claude_max_20x")).as_deref(),
        Some("Claude Max 20x")
    );
    // A bare Max tier without a multiplier keeps the plain label.
    assert_eq!(
        ClaudePlan::oauth_login_method(None, Some("claude_max")).as_deref(),
        Some("Claude Max")
    );
    assert_eq!(
        ClaudePlan::oauth_login_method(None, Some("default_claude_team_5x")).as_deref(),
        Some("Claude Team")
    );
    // A resolved non-Max plan never inherits a Max multiplier from a disagreeing tier.
    assert_eq!(
        ClaudePlan::oauth_login_method(Some("team"), Some("default_claude_max_5x")).as_deref(),
        Some("Claude Team")
    );
    assert_eq!(
        ClaudePlan::web_login_method(Some("default_claude_max_20x"), None, None).as_deref(),
        Some("Claude Max 20x")
    );
}

#[test]
fn oauth_subscription_type_overrides_generic_rate_limit_tier() {
    assert_eq!(
        ClaudePlan::oauth_login_method(Some("pro"), Some("default_claude_ai")).as_deref(),
        Some("Claude Pro")
    );
    assert_eq!(
        ClaudePlan::oauth_login_method(Some("team"), Some("default_claude_max_5x")).as_deref(),
        Some("Claude Team")
    );
    assert_eq!(
        ClaudePlan::oauth_login_method(None, Some("default_claude_ai")),
        None
    );
}

#[test]
fn web_fallback_preserves_stripe_claude_compatibility() {
    assert_eq!(
        ClaudePlan::web_login_method(Some("default_claude"), Some("stripe_subscription"), None)
            .as_deref(),
        Some("Claude Pro")
    );
}

#[test]
fn web_team_seat_tiers_map_to_specific_labels() {
    assert_eq!(
        ClaudePlan::web_login_method(
            Some("claude_team"),
            Some("stripe_subscription"),
            Some("team_standard")
        )
        .as_deref(),
        Some("Claude Team Standard")
    );
    assert_eq!(
        ClaudePlan::web_login_method(
            Some("claude_team"),
            Some("stripe_subscription"),
            Some("team_tier_1")
        )
        .as_deref(),
        Some("Claude Team Premium")
    );
    assert_eq!(
        ClaudePlan::web_login_method(None, None, Some("team_standard")).as_deref(),
        Some("Claude Team Standard")
    );
}

#[test]
fn web_team_seat_tier_near_misses_use_existing_plan_inference() {
    assert_eq!(
        ClaudePlan::web_login_method(
            Some("claude_team"),
            Some("stripe_subscription"),
            Some("team_premium")
        )
        .as_deref(),
        Some("Claude Team")
    );
    assert_eq!(
        ClaudePlan::web_login_method(
            Some("claude_team"),
            Some("stripe_subscription"),
            Some("team_standard_plus")
        )
        .as_deref(),
        Some("Claude Team")
    );
}

#[test]
fn web_enterprise_seat_tiers_preserve_the_enterprise_label() {
    assert_eq!(
        ClaudePlan::web_login_method(
            Some("claude_enterprise"),
            Some("stripe_subscription"),
            Some("team_standard")
        )
        .as_deref(),
        Some("Claude Enterprise")
    );
    assert_eq!(
        ClaudePlan::web_login_method(
            Some("claude_enterprise"),
            Some("stripe_subscription"),
            Some("team_tier_1")
        )
        .as_deref(),
        Some("Claude Enterprise")
    );
}

#[test]
fn missing_web_seat_tier_preserves_existing_plan_labels() {
    assert_eq!(
        ClaudePlan::web_login_method(Some("default_claude_max_20x"), None, None).as_deref(),
        Some("Claude Max 20x")
    );
    assert_eq!(
        ClaudePlan::web_login_method(Some("claude_pro"), Some("stripe_subscription"), None)
            .as_deref(),
        Some("Claude Pro")
    );
    assert_eq!(
        ClaudePlan::web_login_method(Some("claude_team"), Some("stripe_subscription"), None)
            .as_deref(),
        Some("Claude Team")
    );
}

#[test]
fn compatibility_parser_understands_current_labels() {
    assert_eq!(
        ClaudePlan::from_compatibility_login_method(Some("Claude Max")),
        Some(ClaudePlan::Max)
    );
    assert_eq!(
        ClaudePlan::from_compatibility_login_method(Some("Max")),
        Some(ClaudePlan::Max)
    );
    assert_eq!(
        ClaudePlan::from_compatibility_login_method(Some("Claude Pro")),
        Some(ClaudePlan::Pro)
    );
    assert_eq!(
        ClaudePlan::from_compatibility_login_method(Some("Ultra")),
        Some(ClaudePlan::Ultra)
    );
    assert_eq!(
        ClaudePlan::from_compatibility_login_method(Some("Claude Team")),
        Some(ClaudePlan::Team)
    );
    assert_eq!(
        ClaudePlan::from_compatibility_login_method(Some("Claude Enterprise")),
        Some(ClaudePlan::Enterprise)
    );
}

#[test]
fn cli_projection_keeps_compact_compatibility_and_unknown_fallback() {
    assert_eq!(
        ClaudePlan::cli_compatibility_login_method(Some("Claude Max Account")).as_deref(),
        Some("Max")
    );
    assert_eq!(
        ClaudePlan::cli_compatibility_login_method(Some("Team")).as_deref(),
        Some("Team")
    );
    assert_eq!(
        ClaudePlan::cli_compatibility_login_method(Some("Claude Enterprise Account")).as_deref(),
        Some("Enterprise")
    );
    assert_eq!(
        ClaudePlan::cli_compatibility_login_method(Some("Claude Ultra Account")).as_deref(),
        Some("Ultra")
    );
    assert_eq!(
        ClaudePlan::cli_compatibility_login_method(Some("Experimental")).as_deref(),
        Some("Experimental")
    );
    assert_eq!(
        ClaudePlan::cli_compatibility_login_method(Some("Profile")).as_deref(),
        Some("Profile")
    );
    assert_eq!(
        ClaudePlan::cli_compatibility_login_method(Some("Browser profile")).as_deref(),
        Some("Browser profile")
    );
}

#[test]
fn subscription_compatibility_preserves_ultra_and_excludes_enterprise() {
    assert!(ClaudePlan::is_subscription_login_method(Some("Claude Max")));
    assert!(ClaudePlan::is_subscription_login_method(Some("Pro")));
    assert!(ClaudePlan::is_subscription_login_method(Some("Ultra")));
    assert!(ClaudePlan::is_subscription_login_method(Some("Team")));
    assert!(!ClaudePlan::is_subscription_login_method(Some(
        "Claude Enterprise"
    )));
    assert!(!ClaudePlan::is_subscription_login_method(Some("Profile")));
    assert!(!ClaudePlan::is_subscription_login_method(Some(
        "Browser profile"
    )));
    assert!(!ClaudePlan::is_subscription_login_method(Some("API")));
}
