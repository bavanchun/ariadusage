// Ported from CodexBar Tests/CodexBarTests/AntigravityQuotaSourceParityTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use ariadusage_core::pipeline::{FetchInteraction, TransportClass};
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::providers::antigravity::*;
use jiff::Timestamp;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
fn credentials(expiry: Option<Timestamp>) -> AntigravityOAuthCredentials {
    AntigravityOAuthCredentials::new(
        "synthetic-token",
        None,
        expiry,
        None,
        Some("quota@example.com".into()),
        Some("synthetic-project".into()),
        None,
        None,
    )
}
fn environment(credentials: &AntigravityOAuthCredentials) -> BTreeMap<String, String> {
    BTreeMap::from([(
        AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.into(),
        AntigravityOAuthCredentialsStore::token_account_value(credentials).unwrap(),
    )])
}
fn summary(starter: bool) -> Value {
    let groups =
        [("Gemini Models", "gemini"), ("Claude and GPT models", "3p")].map(|(title, id)| {
            let mut buckets = vec![json!({
                "bucketId": format!("{id}-weekly"),
                "displayName": "Weekly Limit",
                "window": "weekly",
                "remainingFraction": if starter{
                    1.0
                }else{
                    0.958
                }
            })];
            if !starter {
                buckets.push(json!({
                    "bucketId": format!("{id}-5h"),
                    "displayName": "Five Hour Limit",
                    "window": "5h",
                    "remainingFraction": 0.749,
                    "resetTime": "2026-07-23T17:05:10Z"
                }));
            }
            json!({
                "displayName": title,
                "buckets": buckets
            })
        });
    json!({
        "groups": groups
    })
}
fn response(status: u16, body: Value) -> RemoteHttpResponse {
    RemoteHttpResponse {
        status,
        body: serde_json::to_vec(&body).unwrap(),
    }
}

// upstream: AntigravityQuotaSourceParityTests.swift:25 OAuth preserves the same grouped quotas as local and print sources
#[tokio::test]
async fn oauth_preserves_the_same_grouped_quotas_as_local_and_print_sources() {
    for starter in [true, false] {
        let credentials = credentials(Some(
            Timestamp::now()
                .saturating_add(jiff::Span::new().seconds(3600))
                .unwrap(),
        ));
        let data = serde_json::to_vec(&summary(starter)).unwrap();
        let fetcher = AntigravityRemoteUsageFetcher::new(
            Duration::from_secs(8),
            "/synthetic-antigravity-home",
        )
        .with_environment(environment(&credentials))
        .with_data_loader(move |req| {
            Box::pin(async move {
                assert_eq!(req.header("Authorization"), Some("Bearer synthetic-token"));
                let body = match req.url.path() {
                    "/v1internal:loadCodeAssist" => json!({
                        "currentTier":{
                            "id": if starter{
                                "free-tier"
                            }else{
                                "standard-tier"
                            }
                        }
                    }),
                    "/v1internal:retrieveUserQuotaSummary" => {
                        let posted: Value =
                            serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
                        assert_eq!(posted["project"], "synthetic-project");
                        summary(starter)
                    }
                    _ => json!({
                        "models":{
                            "gemini-2.5-pro":{
                                "quotaInfo":{
                                    "remainingFraction": 0.749
                                }
                            }
                        }
                    }),
                };
                Ok(response(200, body))
            })
        });
        let remote = fetcher.fetch().await.unwrap();
        let local = AntigravityStatusProbe::parse_quota_summary_response(&data).unwrap();
        let report = json!({
            "status": "SUCCESS",
            "command":{
                "name": "usage",
                "data": summary(starter)
            }
        });
        let cli =
            AntigravityStatusProbe::parse_cli_usage_report(&serde_json::to_vec(&report).unwrap())
                .unwrap();
        let local_usage = local.to_usage_snapshot().unwrap();
        let cli_usage = cli.to_usage_snapshot().unwrap();
        let usage = AntigravityOAuthFetchStrategy::usage_snapshot_from(&remote).unwrap();
        let windows = usage.extra_rate_windows.as_ref().unwrap();
        assert_eq!(windows.len(), if starter { 2 } else { 4 });
        assert_eq!(usage.extra_rate_windows, local_usage.extra_rate_windows);
        assert_eq!(usage.extra_rate_windows, cli_usage.extra_rate_windows);
        assert_eq!(remote.source, AntigravitySnapshotSource::Remote);
        assert_eq!(
            usage.identity.as_ref().unwrap().account_email.as_deref(),
            Some("quota@example.com")
        );
        assert_eq!(
            usage.identity.as_ref().unwrap().login_method.as_deref(),
            Some(if starter { "Free" } else { "Paid" })
        );
        assert!(usage.secondary.is_some());
        if starter {
            assert_eq!(
                windows
                    .iter()
                    .map(|w| w.window.window_minutes)
                    .collect::<Vec<_>>(),
                [Some(10080); 2]
            );
            assert_eq!(
                windows
                    .iter()
                    .map(|w| w.window.remaining_percent())
                    .collect::<Vec<_>>(),
                [100.; 2]
            );
            assert!(AntigravityQuotaFamilyVisibility::idle_window_ids(&usage).is_empty());
        }
    }
}

// upstream: AntigravityQuotaSourceParityTests.swift:85 summary honors explicit cadence for opaque bucket IDs
#[test]
fn summary_honors_explicit_cadence_for_opaque_bucket_ids() {
    for cadence in ["weekly", "5h"] {
        let data = json!({
            "groups":[{
                    "displayName": "Gemini Models",
                    "buckets":[{
                            "bucketId": "gemini-allowance",
                            "displayName": "Limit Remaining",
                            "window": cadence,
                            "remainingFraction": 1
                        }
                    ]
                }
            ]
        });
        let usage = AntigravityStatusProbe::parse_quota_summary_response(
            &serde_json::to_vec(&data).unwrap(),
        )
        .unwrap()
        .to_usage_snapshot()
        .unwrap();
        let window = usage.extra_rate_windows.as_ref().unwrap().first().unwrap();
        assert_eq!(window.id, "antigravity-quota-summary-gemini-allowance");
        assert_eq!(
            window.title,
            if cadence == "weekly" {
                "Gemini weekly"
            } else {
                "Gemini 5-hour"
            }
        );
        assert_eq!(
            window.window.window_minutes,
            Some(if cadence == "weekly" { 10080 } else { 300 })
        );
    }
}

// upstream: AntigravityQuotaSourceParityTests.swift:100 explicit cadence takes precedence over legacy bucket names
#[test]
fn explicit_cadence_takes_precedence_over_legacy_bucket_names() {
    for cadence in ["weekly", "unknown"] {
        let data = json!({
            "groups":[{
                    "displayName": "Gemini Models",
                    "buckets":[{
                            "bucketId": "gemini-5h",
                            "displayName": "Five Hour Limit",
                            "window": cadence,
                            "remainingFraction": 0.8
                        }
                    ]
                }
            ]
        });
        let usage = AntigravityStatusProbe::parse_quota_summary_response(
            &serde_json::to_vec(&data).unwrap(),
        )
        .unwrap()
        .to_usage_snapshot()
        .unwrap();
        assert_eq!(
            usage
                .extra_rate_windows
                .as_ref()
                .unwrap()
                .first()
                .unwrap()
                .window
                .window_minutes,
            if cadence == "weekly" {
                Some(10080)
            } else {
                None
            }
        );
    }
}

// upstream: AntigravityQuotaSourceParityTests.swift:112 summary fallback preserves authentication and cancellation errors
#[tokio::test]
async fn summary_fallback_preserves_authentication_and_cancellation_errors() {
    for status_code in [200, 401, 403, 404, 500, -1] {
        let credentials = credentials(None);
        let call = BrokerCall {
            interaction: FetchInteraction::UserInitiated,
            cancel: CancellationToken::new(),
            request_id: "quota-source-parity".into(),
        };
        let cancel = call.cancel.clone();
        let fetcher = AntigravityRemoteUsageFetcher::new(
            Duration::from_secs(8),
            "/synthetic-antigravity-home",
        )
        .with_environment(environment(&credentials))
        .with_data_loader(move |req| {
            let cancel = cancel.clone();
            Box::pin(async move {
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(response(200, json!({}))),
                    "/v1internal:retrieveUserQuotaSummary" => {
                        assert!(req.timeout <= Duration::from_secs(2));
                        if status_code == -1 {
                            cancel.cancel();
                            return Err(AntigravityRemoteFetchError::NetworkError(
                                "Request cancelled".into(),
                            ));
                        }
                        Ok(response(
                            status_code as u16,
                            json!({
                                "buckets":[
                                ]
                            }),
                        ))
                    }
                    _ => Ok(response(
                        200,
                        json!({
                            "models":{
                                "gemini-2.5-pro":{
                                    "quotaInfo":{
                                        "remainingFraction": 0.5
                                    }
                                }
                            }
                        }),
                    )),
                }
            })
        });
        let result = fetcher.fetch_with_call(&call).await;
        if status_code == 401 {
            assert_eq!(
                result.unwrap_err(),
                AntigravityRemoteFetchError::NotLoggedIn
            );
        } else if status_code == -1 {
            assert_eq!(
                result.unwrap_err().into_classified_error().transport,
                Some(TransportClass::Cancelled)
            );
        } else {
            let snapshot = result.unwrap();
            let usage = snapshot.to_usage_snapshot().unwrap();
            assert_eq!(usage.primary.as_ref().unwrap().remaining_percent(), 50.);
            assert_eq!(
                usage.identity.as_ref().unwrap().account_email.as_deref(),
                Some("quota@example.com")
            );
            assert!(snapshot.quota_summary.is_none());
        }
    }
}
