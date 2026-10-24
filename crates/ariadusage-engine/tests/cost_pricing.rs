// Ported from CodexBar Tests/CodexBarTests/CostUsagePricingTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;

use ariadusage_engine::cost::pricing::{
    self, ClaudeCostTokens, CodexServiceTier, CustomModelPricing, CustomModelPricingEntry,
    ModelsDevCatalog, ModelsDevCost, ModelsDevModel, ModelsDevProvider,
};

#[test]
fn normalizes_codex_model_variants_exactly() {
    assert_eq!(
        pricing::normalize_codex_model("openai/gpt-5-codex"),
        "gpt-5-codex"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-5.2-codex"),
        "gpt-5.2-codex"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-5.1-codex-max"),
        "gpt-5.1-codex-max"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-5.4-pro-2026-03-05"),
        "gpt-5.4-pro"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-5.4-mini-2026-03-17"),
        "gpt-5.4-mini"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-5.4-nano-2026-03-17"),
        "gpt-5.4-nano"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-5.5-2026-04-23"),
        "gpt-5.5"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-5.5-pro-2026-04-23"),
        "gpt-5.5-pro"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-5.3-codex-2026-03-05"),
        "gpt-5.3-codex"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-5.3-codex-spark"),
        "gpt-5.3-codex-spark"
    );
    assert_eq!(
        pricing::normalize_codex_model("openai/gpt-5.6-sol"),
        "gpt-5.6-sol"
    );
    assert_eq!(
        pricing::normalize_codex_model("openai/gpt-5.6-terra"),
        "gpt-5.6-terra"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-5.6-luna"),
        "gpt-5.6-luna"
    );
    assert_eq!(pricing::normalize_codex_model("gpt-5.6"), "gpt-5.6-sol");
    assert_eq!(
        pricing::normalize_codex_model("gpt-reserve"),
        "gpt-5.6-luna"
    );
    assert_eq!(
        pricing::normalize_codex_model("openai/gpt-reserve"),
        "gpt-5.6-luna"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-daybreak-blue-latest"),
        "gpt-5.6-sol"
    );
    assert_eq!(
        pricing::normalize_codex_model("openai/gpt-daybreak-blue-latest"),
        "gpt-5.6-sol"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-daybreak-red-latest"),
        "gpt-5.6-cyber"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-5.6-cyber"),
        "gpt-5.6-cyber"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-5.5-cyber"),
        "gpt-5.5-cyber"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-5.6-sol-2099-01-01"),
        "gpt-5.6-sol"
    );
    assert_eq!(
        pricing::normalize_codex_model("openai/gpt-5.6-terra-2099-01-01"),
        "gpt-5.6-terra"
    );
}

#[test]
fn unattributed_codex_usage_stays_unpriced_despite_a_catalog_collision() {
    let mut catalog = ModelsDevCatalog::default();
    let mut models = BTreeMap::new();
    models.insert(
        "unknown".to_owned(),
        ModelsDevModel {
            id: "unknown".to_owned(),
            name: None,
            cost: Some(ModelsDevCost {
                input: Some(99.0),
                output: Some(199.0),
                cache_read: None,
                cache_write: None,
                context_over_200k: None,
                tiers: None,
            }),
            limit: None,
        },
    );
    catalog.providers.insert(
        "openai".to_owned(),
        ModelsDevProvider {
            id: Some("openai".to_owned()),
            name: None,
            models,
            map_key: Some("openai".to_owned()),
        },
    );
    let cost = pricing::codex_cost_usd_with_tier(
        pricing::CODEX_UNATTRIBUTED_MODEL,
        100,
        10,
        5,
        None,
        Some(&catalog),
    );
    assert_eq!(cost, None);
}

#[test]
fn codex_cost_supports_gpt51_codex_max() {
    let cost = pricing::codex_cost_usd_with_tier(
        "gpt-5.1-codex-max",
        100,
        10,
        5,
        None,
        Some(&ModelsDevCatalog::default()),
    );
    assert!(cost.is_some());
}

#[test]
fn codex_cost_supports_gpt53_codex() {
    let cost = pricing::codex_cost_usd_with_tier(
        "gpt-5.3-codex",
        100,
        10,
        5,
        None,
        Some(&ModelsDevCatalog::default()),
    );
    assert!(cost.is_some());
}

#[test]
fn codex_cost_supports_gpt54_mini_and_nano() {
    let mini = pricing::codex_cost_usd_with_tier(
        "gpt-5.4-mini-2026-03-17",
        100,
        10,
        5,
        None,
        Some(&ModelsDevCatalog::default()),
    );
    let nano = pricing::codex_cost_usd_with_tier(
        "gpt-5.4-nano",
        100,
        10,
        5,
        None,
        Some(&ModelsDevCatalog::default()),
    );
    assert!(mini.is_some());
    assert!(nano.is_some());
}

#[test]
fn codex_cost_supports_gpt55_bundled_fallback() {
    let cost = pricing::codex_cost_usd(
        "openai/gpt-5.5-2026-04-23",
        100,
        10,
        5,
        0,
        None,
        None,
        None,
        None,
        false,
    )
    .expect("gpt-5.5 cost");
    let expected = (90.0 * 5e-6) + (10.0 * 5e-7) + (5.0 * 3e-5);
    assert!((cost - expected).abs() < 1e-12);
}

#[test]
fn codex_cost_supports_gpt56_sol_terra_luna_bundled_fallback() {
    let sol = pricing::codex_cost_usd(
        "openai/gpt-5.6-sol",
        100,
        10,
        5,
        0,
        None,
        None,
        None,
        None,
        false,
    )
    .unwrap();
    let terra = pricing::codex_cost_usd(
        "gpt-5.6-terra",
        100,
        10,
        5,
        0,
        None,
        None,
        None,
        None,
        false,
    )
    .unwrap();
    let luna =
        pricing::codex_cost_usd("gpt-5.6-luna", 100, 10, 5, 0, None, None, None, None, false)
            .unwrap();
    let reserve =
        pricing::codex_cost_usd("gpt-reserve", 100, 10, 5, 0, None, None, None, None, false)
            .unwrap();
    let alias =
        pricing::codex_cost_usd("gpt-5.6", 100, 10, 5, 0, None, None, None, None, false).unwrap();

    assert!((sol - ((90.0 * 4e-6) + (10.0 * 4e-7) + (5.0 * 2e-5))).abs() < 1e-12);
    assert!((terra - ((90.0 * 2e-6) + (10.0 * 2e-7) + (5.0 * 1.2e-5))).abs() < 1e-12);
    assert!((luna - ((90.0 * 2e-7) + (10.0 * 2e-8) + (5.0 * 1.2e-6))).abs() < 1e-12);
    assert!((reserve - luna).abs() < 1e-12);
    assert!((alias - sol).abs() < 1e-12);
}

#[test]
fn codex_cost_supports_gpt55_pro_bundled_fallback() {
    let cost = pricing::codex_cost_usd(
        "openai/gpt-5.5-pro-2026-04-23",
        100,
        10,
        5,
        0,
        None,
        None,
        None,
        None,
        false,
    );
    let expected = (100.0 * 3e-5) + (5.0 * 1.8e-4);
    assert!((cost.unwrap() - expected).abs() < 1e-12);
}

#[test]
fn codex_cost_returns_zero_for_research_preview_fallback_model() {
    let cost = pricing::codex_cost_usd(
        "gpt-5.3-codex-spark",
        100,
        10,
        5,
        0,
        None,
        None,
        None,
        None,
        false,
    );
    assert_eq!(cost, Some(0.0));
    assert_eq!(
        pricing::codex_display_label("gpt-5.3-codex-spark"),
        Some("Research Preview")
    );
    assert_eq!(pricing::codex_display_label("gpt-5.2-codex"), None);
}

#[test]
fn normalizes_claude_opus41_dated_variants() {
    assert_eq!(
        pricing::normalize_claude_model("claude-opus-4-1-20250805"),
        "claude-opus-4-1"
    );
}

#[test]
fn claude_cost_supports_opus41_dated_variant() {
    let cost = pricing::claude_cost_usd(
        "claude-opus-4-1-20250805",
        ClaudeCostTokens {
            input: 10,
            cache_read: 0,
            cache_creation: 0,
            cache_creation_1h: 0,
            output: 5,
        },
        None,
        None,
        None,
    );
    assert!(cost.is_some());
}

#[test]
fn claude_cost_supports_opus46_dated_variant() {
    let cost = pricing::claude_cost_usd(
        "claude-opus-4-6-20260205",
        ClaudeCostTokens {
            input: 10,
            cache_read: 0,
            cache_creation: 0,
            cache_creation_1h: 0,
            output: 5,
        },
        None,
        None,
        None,
    );
    assert!(cost.is_some());
}

#[test]
fn claude_cost_supports_opus47() {
    let cost = pricing::claude_cost_usd(
        "claude-opus-4-7",
        ClaudeCostTokens {
            input: 10,
            cache_read: 0,
            cache_creation: 0,
            cache_creation_1h: 0,
            output: 5,
        },
        None,
        None,
        None,
    )
    .unwrap();
    assert!((cost - ((10.0 * 5e-6) + (5.0 * 2.5e-5))).abs() < 1e-12);
}

#[test]
fn claude_cost_supports_opus48() {
    let cost = pricing::claude_cost_usd(
        "claude-opus-4-8",
        ClaudeCostTokens {
            input: 10,
            cache_read: 0,
            cache_creation: 0,
            cache_creation_1h: 0,
            output: 5,
        },
        None,
        None,
        None,
    )
    .unwrap();
    assert!((cost - ((10.0 * 5e-6) + (5.0 * 2.5e-5))).abs() < 1e-12);
}

#[test]
fn claude_cost_supports_fable5_bundled_fallback() {
    let cost = pricing::claude_cost_usd(
        "claude-fable-5",
        ClaudeCostTokens {
            input: 100,
            cache_read: 20,
            cache_creation: 10,
            cache_creation_1h: 0,
            output: 5,
        },
        None,
        None,
        None,
    )
    .unwrap();
    let expected = (100.0 * 1e-5) + (20.0 * 1e-6) + (10.0 * 1.25e-5) + (5.0 * 5e-5);
    assert!((cost - expected).abs() < 1e-12);
}

#[test]
fn claude_cost_preserves_historical_sonnet46_long_context_pricing() {
    let historical = pricing::claude_cost_usd(
        "claude-sonnet-4-6",
        ClaudeCostTokens {
            input: 240_000,
            cache_read: 0,
            cache_creation: 0,
            cache_creation_1h: 0,
            output: 0,
        },
        Some(1_773_359_999),
        None,
        None,
    )
    .unwrap();
    let current = pricing::claude_cost_usd(
        "claude-sonnet-4-6",
        ClaudeCostTokens {
            input: 240_000,
            cache_read: 0,
            cache_creation: 0,
            cache_creation_1h: 0,
            output: 0,
        },
        Some(1_773_360_000),
        None,
        None,
    )
    .unwrap();
    assert!((historical - 1.44).abs() < 1e-9);
    assert!((current - 0.72).abs() < 1e-9);
}

#[test]
fn claude_cost_prices_one_hour_cache_writes_separately() {
    let cost = pricing::claude_cost_usd(
        "claude-fable-5",
        ClaudeCostTokens {
            input: 100,
            cache_read: 20,
            cache_creation: 30,
            cache_creation_1h: 20,
            output: 5,
        },
        None,
        None,
        None,
    )
    .unwrap();
    let expected = (100.0 * 1e-5) + (20.0 * 1e-6) + (10.0 * 1.25e-5) + (20.0 * 2e-5) + (5.0 * 5e-5);
    assert!((cost - expected).abs() < 1e-12);
}

#[test]
fn claude_sonnet46_uses_standard_pricing_across_full_context() {
    let cost = pricing::claude_cost_usd(
        "claude-sonnet-4-6",
        ClaudeCostTokens {
            input: 0,
            cache_read: 0,
            cache_creation: 240_000,
            cache_creation_1h: 0,
            output: 0,
        },
        None,
        None,
        None,
    )
    .unwrap();
    assert!((cost - 240_000.0 * 3.75e-6).abs() < 1e-12);
}

#[test]
fn claude_cost_returns_nil_for_unknown_models() {
    let cost = pricing::claude_cost_usd(
        "glm-4.6",
        ClaudeCostTokens {
            input: 100,
            cache_read: 500,
            cache_creation: 0,
            cache_creation_1h: 0,
            output: 40,
        },
        None,
        None,
        None,
    );
    assert_eq!(cost, None);
}

#[test]
fn gpt_5_6_terra_and_luna_use_explicit_july_2026_boundary_dates() {
    let before = 1_785_369_599_i64;
    let after = 1_785_369_601_i64;

    let terra_old = pricing::codex_cost_usd(
        "gpt-5.6-terra",
        100,
        10,
        5,
        0,
        Some(before),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    let terra_new = pricing::codex_cost_usd(
        "gpt-5.6-terra",
        100,
        10,
        5,
        0,
        Some(after),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    let luna_old = pricing::codex_cost_usd(
        "gpt-5.6-luna",
        100,
        10,
        5,
        0,
        Some(before),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    let luna_new = pricing::codex_cost_usd(
        "gpt-5.6-luna",
        100,
        10,
        5,
        0,
        Some(after),
        None,
        None,
        None,
        false,
    )
    .unwrap();

    assert!((terra_old - ((90.0 * 2.5e-6) + (10.0 * 2.5e-7) + (5.0 * 1.5e-5))).abs() < 1e-12);
    assert!((terra_new - ((90.0 * 2e-6) + (10.0 * 2e-7) + (5.0 * 1.2e-5))).abs() < 1e-12);
    assert!(terra_old > terra_new);
    assert!((luna_old - ((90.0 * 1e-6) + (10.0 * 1e-7) + (5.0 * 6e-6))).abs() < 1e-12);
    assert!((luna_new - ((90.0 * 2e-7) + (10.0 * 2e-8) + (5.0 * 1.2e-6))).abs() < 1e-12);
    assert!(luna_old > luna_new);
}

#[test]
fn gpt_5_6_sol_pricing_is_unchanged_across_explicit_july_2026_boundary_dates() {
    let before = 1_785_369_599_i64;
    let after = 1_785_369_601_i64;
    let sol_old = pricing::codex_cost_usd(
        "gpt-5.6-sol",
        100,
        10,
        5,
        0,
        Some(before),
        None,
        None,
        None,
        false,
    );
    let sol_new = pricing::codex_cost_usd(
        "gpt-5.6-sol",
        100,
        10,
        5,
        0,
        Some(after),
        None,
        None,
        None,
        false,
    );
    assert_eq!(sol_old, sol_new);
}

#[test]
fn historical_pricing_does_not_activate_without_a_pricingdate() {
    let after = 1_785_369_601_i64;
    let terra_nil = pricing::codex_cost_usd(
        "gpt-5.6-terra",
        100,
        10,
        5,
        0,
        None,
        None,
        None,
        None,
        false,
    );
    let terra_new = pricing::codex_cost_usd(
        "gpt-5.6-terra",
        100,
        10,
        5,
        0,
        Some(after),
        None,
        None,
        None,
        false,
    );
    assert_eq!(terra_nil, terra_new);
}

#[test]
fn claude_model_normalization_strips_provider_prefixes_and_dates() {
    assert_eq!(
        pricing::normalize_claude_model("anthropic.claude-sonnet-4-20250514-v1:0"),
        "claude-sonnet-4-20250514"
    );
    assert_eq!(
        pricing::normalize_claude_model("us.anthropic.claude-opus-4-5-20251101-v1:0"),
        "claude-opus-4-5"
    );
    assert_eq!(
        pricing::normalize_claude_model("claude-opus-4-1-20250805"),
        "claude-opus-4-1"
    );
    assert_eq!(
        pricing::normalize_claude_model("eu.anthropic.claude-haiku-4-5-20251001-v1:0"),
        "claude-haiku-4-5"
    );
    assert_eq!(
        pricing::normalize_claude_model("kimi-for-coding"),
        "kimi-for-coding"
    );
}

#[test]
fn claude_pricing_calculates_standard_and_tiered_costs() {
    // Under 200k threshold: claude-sonnet-4-5 is $3/M input, $15/M output
    let under = pricing::claude_cost_usd(
        "claude-sonnet-4-5",
        ClaudeCostTokens {
            input: 100_000,
            cache_read: 0,
            cache_creation: 0,
            cache_creation_1h: 0,
            output: 10_000,
        },
        None,
        None,
        None,
    )
    .expect("priced model");
    let expected_under = 100_000.0 * 3e-6 + 10_000.0 * 15e-6;
    assert!((under - expected_under).abs() < 1e-9);

    // Over 200k threshold: Claude's above-threshold rate applies to all tokens ($6/M)
    let over = pricing::claude_cost_usd(
        "claude-sonnet-4-5",
        ClaudeCostTokens {
            input: 300_000,
            cache_read: 0,
            cache_creation: 0,
            cache_creation_1h: 0,
            output: 0,
        },
        None,
        None,
        None,
    )
    .expect("priced model");
    let expected_over = 300_000.0 * 6e-6;
    assert!((over - expected_over).abs() < 1e-9);
    assert!(over > under * 2.0);
}

#[test]
fn claude_pricing_supports_1h_cache_creation_split() {
    let cost = pricing::claude_cost_usd(
        "claude-sonnet-4-20250514",
        ClaudeCostTokens {
            input: 10_000,
            cache_read: 0,
            cache_creation: 20_000,
            cache_creation_1h: 5_000,
            output: 1_000,
        },
        None,
        None,
        None,
    )
    .expect("priced model");
    // 15,000 5m tokens at $3.75/M + 5,000 1h tokens at $6.00/M (1.6x of 3.75)
    let expected = 10_000.0 * 3e-6 + 15_000.0 * 3.75e-6 + 5_000.0 * 6.0e-6 + 1_000.0 * 15e-6;
    assert!((cost - expected).abs() < 1e-9);
}

#[test]
fn codex_model_normalization_and_priority_multiplier() {
    assert_eq!(
        pricing::normalize_codex_model("openai/gpt-5.2-codex"),
        "gpt-5.2-codex"
    );
    assert_eq!(
        pricing::normalize_codex_model("gpt-5.3-codex-spark"),
        "gpt-5.3-codex-spark"
    );
    assert!(pricing::is_codex_unattributed_model("unknown"));
    assert_eq!(
        pricing::codex_display_label("gpt-5.3-codex-spark"),
        Some("Research Preview")
    );

    let standard = pricing::codex_cost_usd_with_tier(
        "gpt-5.2-codex",
        100_000,
        20_000,
        10_000,
        Some(CodexServiceTier::Standard),
        None,
    )
    .expect("standard cost");
    let priority = pricing::codex_cost_usd_with_tier(
        "gpt-5.2-codex",
        100_000,
        20_000,
        10_000,
        Some(CodexServiceTier::Priority),
        None,
    )
    .expect("priority cost");

    assert!((priority - standard * 2.0).abs() < 1e-9);
}

#[test]
fn models_dev_catalog_and_custom_pricing_overrides_resolve() {
    let mut catalog = ModelsDevCatalog::default();
    let mut provider_models = BTreeMap::new();
    provider_models.insert(
        "gpt-6-future".to_owned(),
        ModelsDevModel {
            id: "gpt-6-future".to_owned(),
            name: Some("GPT-6 Future".to_owned()),
            cost: Some(ModelsDevCost {
                input: Some(4.0),
                output: Some(16.0),
                cache_read: Some(0.4),
                cache_write: None,
                context_over_200k: None,
                tiers: None,
            }),
            limit: None,
        },
    );
    catalog.providers.insert(
        "openai".to_owned(),
        ModelsDevProvider {
            id: Some("openai".to_owned()),
            name: Some("OpenAI".to_owned()),
            models: provider_models,
            map_key: Some("openai".to_owned()),
        },
    );

    let cost = pricing::codex_cost_usd_with_tier(
        "openai/gpt-6-future",
        1_000_000,
        500_000,
        250_000,
        Some(CodexServiceTier::Standard),
        Some(&catalog),
    )
    .expect("resolved from models.dev catalog");
    // 1M * $4 + 0.5M * $0.4 + 0.25M * $16 = 4.0 + 0.2 + 4.0 = 8.2
    assert!((cost - 8.2).abs() < 1e-9);

    let mut custom = CustomModelPricing::default();
    custom.models.insert(
        "custom-claude-model".to_owned(),
        CustomModelPricingEntry {
            input_cost_per_token: 2e-6,
            output_cost_per_token: 8e-6,
            cache_read_input_cost_per_token: Some(0.2e-6),
        },
    );
    let custom_pricing = custom.get("custom-claude-model").expect("custom pricing");
    assert!((custom_pricing.input_cost_per_token - 2e-6).abs() < 1e-12);
}
