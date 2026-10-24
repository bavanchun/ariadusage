// Ported from CodexBar Tests/CodexBarTests/AntigravityQuotaSummaryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use ariadusage_core::model::UsageSnapshot;
use ariadusage_engine::providers::antigravity::*;
use jiff::Timestamp;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
#[path = "support/antigravity-quota.rs"]
mod quota;
fn parse(json: &str) -> UsageSnapshot {
    AntigravityStatusProbe::parse_quota_summary_response(json.as_bytes())
        .unwrap()
        .to_usage_snapshot()
        .unwrap()
}

// upstream: AntigravityQuotaSummaryTests.swift:26 semantic cadences resolve parsed quotas independently of family representatives
#[test]
fn semantic_cadences_resolve_parsed_quotas_independently_of_family_representatives() {
    let usage = parse(&quota::quota_summary(0.86, 0.55, 1., 1.));
    let rows = usage.extra_rate_windows.as_ref().unwrap();
    assert_eq!(
        rows.iter()
            .map(|w| w.window.remaining_percent().round() as i32)
            .collect::<Vec<_>>(),
        [86, 55, 100, 100]
    );
    assert_eq!(
        rows.iter().map(|w| w.usage_known).collect::<Vec<_>>(),
        [true, true, true, true]
    );
    assert_eq!(usage.primary.as_ref(), Some(&rows[1].window));
    assert_eq!(usage.secondary.as_ref(), Some(&rows[2].window));
    let windows = AntigravityProviderPresentation::semantic_windows(&usage);
    assert_eq!(windows.session.as_ref(), Some(&rows[0].window));
    assert_eq!(windows.weekly.as_ref(), Some(&rows[1].window));
    // Native menu attributed-title and accessibility rendering belong to the native frontend.
}

// upstream: AntigravityQuotaSummaryTests.swift:85 parses quota summary response into two model groups with session before weekly windows
#[test]
fn parses_quota_summary_response_into_two_model_groups_with_session_before_weekly_windows() {
    let snapshot = AntigravityStatusProbe::parse_quota_summary_response(
        quota::quota_summary(0.91, 0.82, 0.73, 0.64).as_bytes(),
    )
    .unwrap();
    assert!(snapshot.model_quotas.is_empty());
    let usage = snapshot.to_usage_snapshot().unwrap();
    let windows = usage.extra_rate_windows.as_ref().unwrap();
    assert_eq!(
        windows.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
        [
            "antigravity-quota-summary-gemini-5h",
            "antigravity-quota-summary-gemini-weekly",
            "antigravity-quota-summary-3p-5h",
            "antigravity-quota-summary-3p-weekly"
        ]
    );
    assert_eq!(
        windows.iter().map(|w| w.title.as_str()).collect::<Vec<_>>(),
        [
            "Gemini 5-hour",
            "Gemini weekly",
            "Claude/GPT 5-hour",
            "Claude/GPT weekly"
        ]
    );
    assert_eq!(
        windows
            .iter()
            .map(|w| w.window.window_minutes)
            .collect::<Vec<_>>(),
        [Some(300), Some(10080), Some(300), Some(10080)]
    );
    assert_eq!(
        windows
            .iter()
            .map(|w| w.window.remaining_percent().round() as i32)
            .collect::<Vec<_>>(),
        [91, 82, 73, 64]
    );
    assert_eq!(
        windows.iter().map(|w| w.usage_known).collect::<Vec<_>>(),
        [true; 4]
    );
    assert_eq!(
        windows
            .iter()
            .map(|w| w.window.resets_at)
            .collect::<Vec<_>>(),
        [
            "2026-06-15T11:39:34Z",
            "2026-06-19T08:45:39Z",
            "2026-06-15T12:52:10Z",
            "2026-06-20T00:39:54Z"
        ]
        .map(|s| Some(s.parse::<Timestamp>().unwrap()))
    );
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        82.
    );
    assert_eq!(
        usage
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        64.
    );
    assert!(usage.tertiary.is_none());
}

// upstream: AntigravityQuotaSummaryTests.swift:123 parses quota summary oneof remaining value shape
#[test]
fn parses_quota_summary_oneof_remaining_value_shape() {
    let usage = parse(
        r#"{
  "groups": [
    {
      "displayName": "Gemini Models",
      "buckets": [
        {
          "bucketId": "gemini-weekly",
          "displayName": "Weekly Limit",
          "remaining": { "case": "remainingFraction", "value": 0.5 }
        }
      ]
    }
  ]
}
"#,
    );
    let window = usage.extra_rate_windows.as_ref().unwrap().first().unwrap();
    assert_eq!(window.window.remaining_percent(), 50.);
}

// upstream: AntigravityQuotaSummaryTests.swift:177 recognizes underscore cadence without rewriting bucket ID
#[test]
fn recognizes_underscore_cadence_without_rewriting_bucket_id() {
    let usage = parse(
        r#"{
  "groups": [
    {
      "displayName": "Gemini Models",
      "buckets": [
        {
          "bucketId": "gemini_session",
          "displayName": "Gemini",
          "remaining": { "remainingFraction": 0.75 }
        }
      ]
    }
  ]
}
"#,
    );
    let window = usage.extra_rate_windows.as_ref().unwrap().first().unwrap();
    assert_eq!(window.id, "antigravity-quota-summary-gemini_session");
    assert_eq!(window.title, "Gemini 5-hour");
    assert_eq!(window.window.window_minutes, Some(300));
}

// upstream: AntigravityQuotaSummaryTests.swift:204 recognizes prefixed cadence before limit suffix
#[test]
fn recognizes_prefixed_cadence_before_limit_suffix() {
    let usage = parse(
        r#"{
  "groups": [
    {
      "displayName": "Gemini Models",
      "buckets": [
        {
          "bucketId": "gemini-5h limit",
          "displayName": "Gemini quota",
          "remaining": { "remainingFraction": 0.75 }
        }
      ]
    }
  ]
}
"#,
    );
    let window = usage.extra_rate_windows.as_ref().unwrap().first().unwrap();
    assert_eq!(window.id, "antigravity-quota-summary-gemini-5h limit");
    assert_eq!(window.title, "Gemini 5-hour");
    assert_eq!(window.window.window_minutes, Some(300));
}

// upstream: AntigravityQuotaSummaryTests.swift:231 does not classify cadence aliases embedded inside unrelated words
#[test]
fn does_not_classify_cadence_aliases_embedded_inside_unrelated_words() {
    let usage = parse(
        r#"{
  "groups": [
    {
      "displayName": "Gemini Models",
      "buckets": [
        {
          "bucketId": "gemini-session-history",
          "displayName": "Session History",
          "remaining": { "remainingFraction": 0.75 }
        }
      ]
    }
  ]
}
"#,
    );
    let window = usage.extra_rate_windows.as_ref().unwrap().first().unwrap();
    assert_eq!(window.title, "Gemini Session History");
    assert_eq!(window.window.window_minutes, None);
}

// upstream: AntigravityQuotaSummaryTests.swift:148 normalizes supported session cadence aliases without rewriting bucket IDs
#[test]
fn normalizes_supported_session_cadence_aliases_without_rewriting_bucket_ids() {
    for alias in ["session", "5h", "5-hour", "five hour", "five-hour"] {
        let bucket_id = format!("gemini-{alias}");
        let json = serde_json::json!({
            "groups":[{
                    "displayName": "Gemini Models",
                    "buckets":[{
                            "bucketId": bucket_id,
                            "displayName": alias,
                            "remaining":{
                                "remainingFraction": 0.75
                            }
                        }
                    ]
                }
            ]
        });
        let usage = parse(&json.to_string());
        let window = usage.extra_rate_windows.as_ref().unwrap().first().unwrap();
        assert_eq!(window.id, format!("antigravity-quota-summary-{bucket_id}"));
        assert_eq!(window.title, "Gemini 5-hour");
        assert_eq!(window.window.window_minutes, Some(300));
        assert_eq!(window.window.remaining_percent(), 75.);
    }
}

// upstream: AntigravityQuotaSummaryTests.swift:257 fetch snapshot prefers quota summary endpoint and merges identity
#[tokio::test]
async fn fetch_snapshot_prefers_quota_summary_endpoint_and_merges_identity() {
    let paths = Arc::new(Mutex::new(Vec::new()));
    let recorder = paths.clone();
    let clock = Arc::new(Mutex::new(Instant::now()));
    let now = clock.clone();
    let deadline =
        OperationDeadline::with_now_provider(Duration::from_secs(2), move || *now.lock().unwrap());
    let sender: EndpointRequestSender = Arc::new(move |payload, _, timeout| {
        let recorder = recorder.clone();
        let clock = clock.clone();
        Box::pin(async move {
            recorder.lock().unwrap().push(payload.path.clone());
            let _ = (&clock, timeout);
            if payload.path.contains("GetUserStatus") {
                Ok(quota::user_status().into_bytes())
            } else {
                Ok(quota::quota_summary(0.91, 0.82, 0.73, 0.64).into_bytes())
            }
        })
    });
    let context = RequestContext {
        endpoints: vec![AntigravityConnectionEndpoint::new(
            "https",
            64440,
            "token",
            EndpointSource::LanguageServer,
        )],
        timeout: Duration::from_secs(1),
    };
    let snapshot = AntigravityStatusProbe::fetch_snapshot(&context, &deadline, sender)
        .await
        .unwrap();
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        *paths.lock().unwrap(),
        [QUOTA_SUMMARY_PATH, GET_USER_STATUS_PATH]
    );
    assert_eq!(usage.extra_rate_windows.as_ref().unwrap().len(), 4);
    assert_eq!(
        usage.identity.as_ref().unwrap().account_email.as_deref(),
        Some("test@example.com")
    );
    assert_eq!(
        usage.identity.as_ref().unwrap().login_method.as_deref(),
        Some("Pro")
    );
}

// upstream: AntigravityQuotaSummaryTests.swift:286 fetch snapshot keeps quota summary when identity endpoint fails
#[tokio::test]
async fn fetch_snapshot_keeps_quota_summary_when_identity_endpoint_fails() {
    let paths = Arc::new(Mutex::new(Vec::new()));
    let recorder = paths.clone();
    let clock = Arc::new(Mutex::new(Instant::now()));
    let now = clock.clone();
    let deadline =
        OperationDeadline::with_now_provider(Duration::from_secs(2), move || *now.lock().unwrap());
    let sender: EndpointRequestSender = Arc::new(move |payload, _, timeout| {
        let recorder = recorder.clone();
        let clock = clock.clone();
        Box::pin(async move {
            recorder.lock().unwrap().push(payload.path.clone());
            let _ = (&clock, timeout);
            if payload.path.contains("GetUserStatus") {
                Ok(br#"{"code":16}"#.to_vec())
            } else {
                Ok(quota::quota_summary(0.91, 0.82, 0.73, 0.64).into_bytes())
            }
        })
    });
    let context = RequestContext {
        endpoints: vec![AntigravityConnectionEndpoint::new(
            "https",
            64440,
            "token",
            EndpointSource::LanguageServer,
        )],
        timeout: Duration::from_secs(1),
    };
    let snapshot = AntigravityStatusProbe::fetch_snapshot(&context, &deadline, sender)
        .await
        .unwrap();
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        *paths.lock().unwrap(),
        [QUOTA_SUMMARY_PATH, GET_USER_STATUS_PATH]
    );
    assert_eq!(usage.extra_rate_windows.as_ref().unwrap().len(), 4);
    assert!(
        usage
            .identity
            .as_ref()
            .and_then(|i| i.account_email.as_deref())
            .is_none()
    );
}

// upstream: AntigravityQuotaSummaryTests.swift:314 fetch snapshot falls back to user status when quota summary is unavailable
#[tokio::test]
async fn fetch_snapshot_falls_back_to_user_status_when_quota_summary_is_unavailable() {
    let paths = Arc::new(Mutex::new(Vec::new()));
    let recorder = paths.clone();
    let clock = Arc::new(Mutex::new(Instant::now()));
    let now = clock.clone();
    let deadline =
        OperationDeadline::with_now_provider(Duration::from_secs(2), move || *now.lock().unwrap());
    let sender: EndpointRequestSender = Arc::new(move |payload, _, timeout| {
        let recorder = recorder.clone();
        let clock = clock.clone();
        Box::pin(async move {
            recorder.lock().unwrap().push(payload.path.clone());
            let _ = (&clock, timeout);
            if payload.path.contains("RetrieveUserQuotaSummary") {
                Ok(br#"{"code":16}"#.to_vec())
            } else {
                Ok(quota::user_status().into_bytes())
            }
        })
    });
    let context = RequestContext {
        endpoints: vec![AntigravityConnectionEndpoint::new(
            "https",
            64440,
            "token",
            EndpointSource::LanguageServer,
        )],
        timeout: Duration::from_secs(1),
    };
    let snapshot = AntigravityStatusProbe::fetch_snapshot(&context, &deadline, sender)
        .await
        .unwrap();
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        *paths.lock().unwrap(),
        [QUOTA_SUMMARY_PATH, GET_USER_STATUS_PATH]
    );
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        90.
    );
}

// upstream: AntigravityQuotaSummaryTests.swift:341 fetch snapshot falls back when quota summary has no known usage buckets
#[tokio::test]
async fn fetch_snapshot_falls_back_when_quota_summary_has_no_known_usage_buckets() {
    let paths = Arc::new(Mutex::new(Vec::new()));
    let recorder = paths.clone();
    let clock = Arc::new(Mutex::new(Instant::now()));
    let now = clock.clone();
    let deadline =
        OperationDeadline::with_now_provider(Duration::from_secs(2), move || *now.lock().unwrap());
    let sender: EndpointRequestSender = Arc::new(move |payload, _, timeout| {
        let recorder = recorder.clone();
        let clock = clock.clone();
        Box::pin(async move {
            recorder.lock().unwrap().push(payload.path.clone());
            let _ = (&clock, timeout);
            if payload.path.contains("RetrieveUserQuotaSummary") {
                Ok(quota::unknown_summary().into_bytes())
            } else {
                Ok(quota::user_status().into_bytes())
            }
        })
    });
    let context = RequestContext {
        endpoints: vec![AntigravityConnectionEndpoint::new(
            "https",
            64440,
            "token",
            EndpointSource::LanguageServer,
        )],
        timeout: Duration::from_secs(1),
    };
    let snapshot = AntigravityStatusProbe::fetch_snapshot(&context, &deadline, sender)
        .await
        .unwrap();
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        *paths.lock().unwrap(),
        [QUOTA_SUMMARY_PATH, GET_USER_STATUS_PATH]
    );
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        90.
    );
}

// upstream: AntigravityQuotaSummaryTests.swift:368 quota summary timeout reserves deadline for legacy fallback
#[tokio::test]
async fn quota_summary_timeout_reserves_deadline_for_legacy_fallback() {
    let paths = Arc::new(Mutex::new(Vec::new()));
    let recorder = paths.clone();
    let clock = Arc::new(Mutex::new(Instant::now()));
    let now = clock.clone();
    let deadline =
        OperationDeadline::with_now_provider(Duration::from_secs(2), move || *now.lock().unwrap());
    let sender: EndpointRequestSender = Arc::new(move |payload, _, timeout| {
        let recorder = recorder.clone();
        let clock = clock.clone();
        Box::pin(async move {
            recorder.lock().unwrap().push(payload.path.clone());
            if payload.path.contains("RetrieveUserQuotaSummary") {
                assert_eq!(timeout, Duration::from_secs(1));
                *clock.lock().unwrap() += timeout;
                Err(AntigravityStatusProbeError::TimedOut)
            } else {
                assert_eq!(timeout, Duration::from_millis(500));
                Ok(quota::user_status().into_bytes())
            }
        })
    });
    let context = RequestContext {
        endpoints: vec![AntigravityConnectionEndpoint::new(
            "https",
            64440,
            "token",
            EndpointSource::LanguageServer,
        )],
        timeout: Duration::from_secs(1),
    };
    let snapshot = AntigravityStatusProbe::fetch_snapshot(&context, &deadline, sender)
        .await
        .unwrap();
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        *paths.lock().unwrap(),
        [QUOTA_SUMMARY_PATH, GET_USER_STATUS_PATH]
    );
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        90.
    );
}

// upstream: AntigravityQuotaSummaryTests.swift:404 user status timeout reserves deadline for command model fallback
#[tokio::test]
async fn user_status_timeout_reserves_deadline_for_command_model_fallback() {
    let paths = Arc::new(Mutex::new(Vec::new()));
    let recorder = paths.clone();
    let clock = Arc::new(Mutex::new(Instant::now()));
    let now = clock.clone();
    let deadline =
        OperationDeadline::with_now_provider(Duration::from_secs(2), move || *now.lock().unwrap());
    let sender: EndpointRequestSender = Arc::new(move |payload, _, timeout| {
        let recorder = recorder.clone();
        let clock = clock.clone();
        Box::pin(async move {
            recorder.lock().unwrap().push(payload.path.clone());
            if payload.path.contains("RetrieveUserQuotaSummary") {
                Err(AntigravityStatusProbeError::ApiError("unsupported".into()))
            } else if payload.path.contains("GetUserStatus") {
                assert_eq!(timeout, Duration::from_secs(1));
                *clock.lock().unwrap() += timeout;
                Err(AntigravityStatusProbeError::TimedOut)
            } else {
                assert_eq!(timeout, Duration::from_secs(1));
                Ok(quota::model_config().into_bytes())
            }
        })
    });
    let context = RequestContext {
        endpoints: vec![AntigravityConnectionEndpoint::new(
            "https",
            64440,
            "token",
            EndpointSource::LanguageServer,
        )],
        timeout: Duration::from_secs(1),
    };
    let snapshot = AntigravityStatusProbe::fetch_snapshot(&context, &deadline, sender)
        .await
        .unwrap();
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        *paths.lock().unwrap(),
        [
            QUOTA_SUMMARY_PATH,
            GET_USER_STATUS_PATH,
            COMMAND_MODEL_CONFIG_PATH
        ]
    );
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        90.
    );
}
