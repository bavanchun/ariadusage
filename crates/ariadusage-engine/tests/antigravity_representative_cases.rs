// Ported from CodexBar TestsLinux/AntigravityQuotaRepresentativeTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use ariadusage_core::model::UsageSnapshot;
use ariadusage_engine::providers::antigravity::*;
use jiff::Timestamp;
use serde_json::{Value, json};
fn bucket(id: &str, remaining: Option<f64>, cadence: &str, disabled: bool) -> Value {
    let mut bucket = json!({
        "id": id,
        "name": "Synthetic quota",
        "window": cadence,
        "disabled": disabled,
        "reset_time": "2030-03-17T17:46:40Z"
    });
    if let Some(remaining) = remaining {
        bucket["remaining_fraction"] = json!(remaining);
    }
    bucket
}
fn weekly(id: &str, remaining: Option<f64>) -> Value {
    bucket(id, remaining, "weekly", false)
}
fn usage(buckets: Vec<Value>, group_name: &str) -> UsageSnapshot {
    let unknown = buckets
        .iter()
        .all(|bucket| bucket.get("remaining_fraction").is_none());
    let quota = json!({
        "groups":[{
                "name": group_name,
                "buckets": buckets
            }
        ]
    });
    let snapshot = if unknown {
        AntigravityStatusProbe::parse_quota_summary_response(&serde_json::to_vec(&quota).unwrap())
    } else {
        let report = json!({
            "status": "SUCCESS",
            "command":{
                "name": "usage",
                "data": quota
            }
        });
        AntigravityStatusProbe::parse_cli_usage_report(&serde_json::to_vec(&report).unwrap())
    };
    snapshot.unwrap().to_usage_snapshot().unwrap()
}
fn ordinary(buckets: Vec<Value>) -> UsageSnapshot {
    usage(buckets, "Gemini Models")
}
fn icon_windows(usage: &UsageSnapshot) -> IconWindowsPair {
    AntigravityProviderPresentation::icon_windows(IconWindowsContext {
        snapshot: usage,
        secondary_override_window_id: None,
        now: usage.updated_at,
    })
}

// upstream: AntigravityQuotaRepresentativeTests.swift:8 ordinary Gemini quota represents availability beside exhausted model quota
#[test]
fn ordinary_gemini_quota_represents_availability_beside_exhausted_model_quota() {
    for ordinary_id in ["gemini-weekly", "gemini_weekly", "gemini-weekly limit"] {
        let usage = ordinary(vec![
            weekly(ordinary_id, Some(0.9996)),
            weekly("gemini-synthetic-limited-weekly", Some(0.)),
            weekly("gemini-synthetic-special-weekly", Some(0.1)),
        ]);
        let primary = usage.primary.as_ref().unwrap();
        assert!((primary.used_percent - 0.04).abs() < 0.000001);
        assert_eq!(primary.window_minutes, Some(10080));
        assert_eq!(
            primary.resets_at,
            Some(Timestamp::from_second(1_900_000_000).unwrap())
        );
        let rows = usage.extra_rate_windows.as_ref().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows.iter()
                .find(|w| w.id.ends_with("synthetic-limited-weekly"))
                .unwrap()
                .window
                .used_percent,
            100.
        );
        assert_eq!(
            AntigravityProviderPresentation::semantic_windows(&usage)
                .weekly
                .unwrap()
                .used_percent,
            100.
        );
        assert_eq!(icon_windows(&usage).secondary.unwrap().used_percent, 100.);
        let selection = AntigravityProviderPresentation::menu_bar_window(MenuBarWindowContext {
            metric: MenuBarMetric::Primary,
            snapshot: &usage,
            supports_average: false,
            prioritizes_exhausted_quotas: false,
            now: usage.updated_at,
        });
        assert_eq!(selection, MenuBarWindowSelection::Resolved(primary.clone()));
    }
}

// upstream: AntigravityQuotaRepresentativeTests.swift:34 stable ordinary bucket ID wins even when group title changes
#[test]
fn stable_ordinary_bucket_id_wins_even_when_group_title_changes() {
    let usage = usage(
        vec![
            weekly("gemini-weekly", Some(0.8)),
            weekly("gemini-synthetic-limited-weekly", Some(0.)),
        ],
        "General models",
    );
    assert_eq!(usage.primary.as_ref().unwrap().remaining_percent(), 80.);
    assert_eq!(
        AntigravityProviderPresentation::semantic_windows(&usage)
            .weekly
            .unwrap()
            .remaining_percent(),
        0.
    );
}

// upstream: AntigravityQuotaRepresentativeTests.swift:44 model specific quota remains a fallback without measured ordinary quota
#[test]
fn model_specific_quota_remains_a_fallback_without_measured_ordinary_quota() {
    for state in ["absent", "unknown", "disabled"] {
        let mut buckets = vec![weekly("gemini-synthetic-limited-weekly", Some(0.25))];
        if state != "absent" {
            buckets.push(bucket(
                "gemini-weekly",
                if state == "unknown" { None } else { Some(1.) },
                "weekly",
                state == "disabled",
            ));
        }
        let expected_count = buckets.len();
        let usage = ordinary(buckets);
        assert_eq!(usage.primary.as_ref().unwrap().remaining_percent(), 25.);
        assert_eq!(
            AntigravityProviderPresentation::semantic_windows(&usage)
                .weekly
                .unwrap()
                .remaining_percent(),
            25.
        );
        assert_eq!(
            icon_windows(&usage).secondary.unwrap().remaining_percent(),
            25.
        );
        assert_eq!(usage.extra_rate_windows.unwrap().len(), expected_count);
    }
}

// upstream: AntigravityQuotaRepresentativeTests.swift:60 cadence and icon selection retain the most constrained model quota
#[test]
fn cadence_and_icon_selection_retain_the_most_constrained_model_quota() {
    let usage = ordinary(vec![
        bucket("gemini-5h", Some(0.6), "5h", false),
        weekly("gemini-weekly", Some(0.9)),
        weekly("gemini-synthetic-limited-weekly", Some(0.)),
        weekly("3p-weekly", Some(0.7)),
    ]);
    assert_eq!(usage.primary.as_ref().unwrap().remaining_percent(), 60.);
    assert_eq!(usage.primary.as_ref().unwrap().window_minutes, Some(300));
    let semantic = AntigravityProviderPresentation::semantic_windows(&usage);
    assert_eq!(semantic.session.as_ref().unwrap().remaining_percent(), 60.);
    assert_eq!(semantic.weekly.as_ref().unwrap().remaining_percent(), 0.);
    assert_eq!(icon_windows(&usage).primary, semantic.session);
    assert_eq!(icon_windows(&usage).secondary, semantic.weekly);
}

// upstream: AntigravityQuotaRepresentativeTests.swift:77 ordinary session does not erase model specific weekly fallback
#[test]
fn ordinary_session_does_not_erase_model_specific_weekly_fallback() {
    let usage = ordinary(vec![
        bucket("gemini-5h", Some(0.8), "5h", false),
        weekly("gemini-synthetic-limited-weekly", Some(0.1)),
    ]);
    assert_eq!(usage.primary.as_ref().unwrap().remaining_percent(), 80.);
    assert_eq!(
        AntigravityProviderPresentation::semantic_windows(&usage)
            .weekly
            .unwrap()
            .remaining_percent(),
        10.
    );
    assert_eq!(
        icon_windows(&usage).secondary.unwrap().remaining_percent(),
        10.
    );
}

// upstream: AntigravityQuotaRepresentativeTests.swift:88 equal cadence buckets and groups retain source order
#[test]
fn equal_cadence_buckets_and_groups_retain_source_order() {
    let quota = json!({
        "groups":[{
                "name": "Gemini first",
                "buckets":[
                    weekly("gemini-z-weekly",
                    Some(0.7)),
                    weekly("gemini-a-weekly",
                    Some(0.8))
                ]
            },{
                "name": "Gemini second",
                "buckets":[
                    weekly("gemini-b-weekly",
                    Some(0.9))
                ]
            },
        ]
    });
    let usage =
        AntigravityStatusProbe::parse_quota_summary_response(&serde_json::to_vec(&quota).unwrap())
            .unwrap()
            .to_usage_snapshot()
            .unwrap();
    assert_eq!(
        usage
            .extra_rate_windows
            .unwrap()
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        [
            "antigravity-quota-summary-gemini-z-weekly",
            "antigravity-quota-summary-gemini-a-weekly",
            "antigravity-quota-summary-gemini-b-weekly",
        ]
    );
}

// upstream: AntigravityQuotaRepresentativeTests.swift:106 unknown quota never becomes a measured zero
#[test]
fn unknown_quota_never_becomes_a_measured_zero() {
    let usage = ordinary(vec![weekly("gemini-weekly", None)]);
    assert!(usage.primary.is_none());
    assert!(
        AntigravityProviderPresentation::semantic_windows(&usage)
            .weekly
            .is_none()
    );
    assert!(icon_windows(&usage).secondary.is_none());
    assert!(
        !usage
            .extra_rate_windows
            .unwrap()
            .first()
            .unwrap()
            .usage_known
    );
}

// upstream: AntigravityQuotaRepresentativeTests.swift:115 legacy Gemini model quota keeps its representative and reset
#[test]
fn legacy_gemini_model_quota_keeps_its_representative_and_reset() {
    let reset = Timestamp::from_second(1_900_000_000).unwrap();
    let usage = AntigravityStatusSnapshot::new(
        vec![AntigravityModelQuota::new(
            "Gemini 2.5 Pro",
            "gemini-2.5-pro",
            Some(0.4),
            Some(reset),
            Some("Synthetic reset".into()),
        )],
        None,
        None,
    )
    .with_source(AntigravitySnapshotSource::Local)
    .to_usage_snapshot()
    .unwrap();
    assert_eq!(usage.primary.as_ref().unwrap().remaining_percent(), 40.);
    assert_eq!(usage.primary.as_ref().unwrap().resets_at, Some(reset));
}
