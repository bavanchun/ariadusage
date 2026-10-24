// Ported from CodexBar Tests/CodexBarTests/AntigravityPoolDeduplicationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use ariadusage_core::model::UsageSnapshot;
use ariadusage_engine::providers::antigravity::{
    AntigravityModelQuota, AntigravitySnapshotSource, AntigravityStatusSnapshot,
};
use jiff::Timestamp;
use std::collections::BTreeSet;
fn reset() -> Timestamp {
    Timestamp::from_second(1_800_000_000).unwrap()
}
fn variant(
    id: &str,
    fraction: Option<f64>,
    reset: Option<Timestamp>,
    label: Option<&str>,
) -> AntigravityModelQuota {
    AntigravityModelQuota::new(
        label.unwrap_or(id),
        id,
        fraction,
        reset,
        Some("Resets later".into()),
    )
}
fn quota(id: &str) -> AntigravityModelQuota {
    variant(id, Some(0.7), Some(reset()), None)
}
fn usage(models: Vec<AntigravityModelQuota>, source: AntigravitySnapshotSource) -> UsageSnapshot {
    AntigravityStatusSnapshot::new(models, None, None)
        .with_source(source)
        .to_usage_snapshot()
        .unwrap()
}
fn remote(models: Vec<AntigravityModelQuota>) -> UsageSnapshot {
    usage(models, AntigravitySnapshotSource::Remote)
}
fn extra_ids(snapshot: &UsageSnapshot) -> BTreeSet<&str> {
    snapshot
        .extra_rate_windows
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|window| window.id.as_str())
        .collect()
}

// upstream: AntigravityPoolDeduplicationTests.swift:33 only remote pool mirrors are suppressed
#[test]
fn only_remote_pool_mirrors_are_suppressed() {
    for source in [
        AntigravitySnapshotSource::Remote,
        AntigravitySnapshotSource::Local,
    ] {
        let variants = [
            "gemini-test-flash-lite",
            "gemini-test-pro-image",
            "tab_test_autocomplete",
        ];
        let mut models = vec![quota("gemini-test-flash")];
        models.extend(variants.map(quota));
        let result = usage(models, source);
        assert_eq!(result.primary.as_ref().unwrap().used_percent, 30.);
        assert_eq!(
            extra_ids(&result),
            if source == AntigravitySnapshotSource::Remote {
                BTreeSet::new()
            } else {
                BTreeSet::from(variants)
            }
        );
    }
}

// upstream: AntigravityPoolDeduplicationTests.swift:42 independently consumed variants remain distinct
#[test]
fn independently_consumed_variants_remain_distinct() {
    for fraction in [0.2, 0.70005, 0.8] {
        let result = remote(vec![
            quota("gemini-test-flash"),
            variant(
                "gemini-test-flash-lite",
                Some(fraction),
                Some(reset()),
                None,
            ),
        ]);
        let row = result.extra_rate_windows.as_ref().unwrap().first().unwrap();
        assert_eq!(row.id, "gemini-test-flash-lite");
        assert_eq!(row.window.used_percent, 100. - fraction * 100.);
    }
}

// upstream: AntigravityPoolDeduplicationTests.swift:53 different or unobserved resets preserve variants
#[test]
fn different_or_unobserved_resets_preserve_variants() {
    for combination in [0, 1, 2, 3] {
        let pool_reset = if combination == 1 || combination == 3 {
            None
        } else {
            Some(reset())
        };
        let variant_reset = if combination == 2 || combination == 3 {
            None
        } else {
            Some(Timestamp::from_second(1_800_000_060).unwrap())
        };
        let result = remote(vec![
            variant("gemini-test-flash", Some(0.7), pool_reset, None),
            variant("gemini-test-flash-lite", Some(0.7), variant_reset, None),
        ]);
        assert_eq!(
            result
                .extra_rate_windows
                .unwrap()
                .iter()
                .map(|w| w.id.as_str())
                .collect::<Vec<_>>(),
            ["gemini-test-flash-lite"]
        );
    }
}

// upstream: AntigravityPoolDeduplicationTests.swift:64 reset only variants retain unavailable usage
#[test]
fn reset_only_variants_retain_unavailable_usage() {
    let result = remote(vec![
        quota("gemini-test-flash"),
        variant("gemini-test-flash-lite", None, Some(reset()), None),
    ]);
    assert!(
        !result
            .extra_rate_windows
            .unwrap()
            .first()
            .unwrap()
            .usage_known
    );
}

// upstream: AntigravityPoolDeduplicationTests.swift:73 known quota wins over reset only duplicate canonical model
#[test]
fn known_quota_wins_over_reset_only_duplicate_canonical_model() {
    for reverse in [false, true] {
        for fraction in [0.0, 0.4, 1.0] {
            let mut rows = vec![
                variant("gemini-test-flash-lite", None, Some(reset()), None),
                variant(
                    "gemini-test-flash-lite",
                    Some(fraction),
                    Some(reset()),
                    None,
                ),
            ];
            if reverse {
                rows.reverse();
            }
            let result = remote(rows);
            if fraction == 1. {
                assert!(result.extra_rate_windows.is_none());
            } else {
                let rows = result.extra_rate_windows.unwrap();
                let row = rows.first().unwrap();
                assert_eq!(rows.len(), 1);
                assert!(row.usage_known);
                assert_eq!(row.window.used_percent, 100. - fraction * 100.);
            }
        }
    }
}

// upstream: AntigravityPoolDeduplicationTests.swift:90 equal titles retain canonical identity as usage changes
#[test]
fn equal_titles_retain_canonical_identity_as_usage_changes() {
    for reverse in [false, true] {
        let first_id = "gemini-test-flash-lite-one";
        let second_id = "gemini-test-flash-lite-two";
        for fraction in [0.3, 0.5] {
            let mut rows = vec![
                variant(
                    first_id,
                    Some(fraction),
                    Some(reset()),
                    Some("Test Flash Lite"),
                ),
                variant(second_id, Some(0.4), Some(reset()), Some("Test Flash Lite")),
            ];
            if reverse {
                rows.reverse();
            }
            assert_eq!(
                extra_ids(&remote(rows)),
                BTreeSet::from([first_id, second_id])
            );
        }
    }
}

// upstream: AntigravityPoolDeduplicationTests.swift:104 autocomplete keeps its known family and unknown models stay independent
#[test]
fn autocomplete_keeps_its_known_family_and_unknown_models_stay_independent() {
    let result = remote(vec![
        quota("gemini-test-flash"),
        variant("claude-test-model", Some(0.5), Some(reset()), None),
        quota("claude-test-autocomplete"),
        quota("orbit-test-image"),
    ]);
    assert_eq!(
        extra_ids(&result),
        BTreeSet::from(["claude-test-autocomplete", "orbit-test-image"])
    );
}

// upstream: AntigravityPoolDeduplicationTests.swift:115 reset only pools keep Gemini then Claude order
#[test]
fn reset_only_pools_keep_gemini_then_claude_order() {
    let result = remote(vec![
        variant("claude-test-model", None, Some(reset()), None),
        variant("gemini-test-flash", None, Some(reset()), None),
    ]);
    assert_eq!(
        result
            .extra_rate_windows
            .unwrap()
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        ["antigravity-gemini", "antigravity-claude-gpt"]
    );
}
