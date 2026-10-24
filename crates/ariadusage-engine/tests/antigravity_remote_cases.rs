// Ported from CodexBar Tests/CodexBarTests/AntigravityRemoteUsageFetcherTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/GeminiAPITestHelpers.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use ariadusage_core::providers::{SourceMode, find_by_id_str};
use ariadusage_engine::providers::antigravity::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jiff::Timestamp;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tempfile::{TempDir, tempdir};
fn make_id_token(email: &str, hosted_domain: Option<&str>) -> String {
    let header = "header";
    let mut payload = serde_json::Map::new();
    payload.insert("email".to_owned(), Value::String(email.to_owned()));
    if let Some(hd) = hosted_domain {
        payload.insert("hd".to_owned(), Value::String(hd.to_owned()));
    }
    let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    format!("{header}.{payload_b64}.sig")
}
fn decode_form_body(bytes: &[u8]) -> BTreeMap<String, String> {
    let text = std::str::from_utf8(bytes).expect("utf8 form body");
    let mut map = BTreeMap::new();
    for pair in text.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').expect("key=value");
        map.insert(percent_decode(k), percent_decode(v));
    }
    map
}
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
            let byte = u8::from_str_radix(hex, 16).unwrap();
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).expect("utf8 decoded component")
}
struct TestEnvironment {
    dir: TempDir,
}
impl TestEnvironment {
    fn new() -> Self {
        let dir = tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join(".gemini")).unwrap();
        fs::create_dir_all(AntigravityOAuthCredentialsStore::default_directory_url(
            dir.path(),
        ))
        .unwrap();
        Self { dir }
    }
    fn home(&self) -> &Path {
        self.dir.path()
    }
    #[allow(clippy::too_many_arguments)]
    fn write_antigravity_credentials(
        &self,
        access_token: &str,
        refresh_token: Option<&str>,
        expiry: Timestamp,
        id_token: Option<&str>,
        email: Option<&str>,
        project_id: Option<&str>,
        client_id: Option<&str>,
        client_secret: Option<&str>,
    ) {
        let creds = AntigravityOAuthCredentials::new(
            access_token,
            refresh_token.map(str::to_owned),
            Some(expiry),
            id_token.map(str::to_owned),
            email.map(str::to_owned),
            project_id.map(str::to_owned),
            client_id.map(str::to_owned),
            client_secret.map(str::to_owned),
        );
        let url = AntigravityOAuthCredentialsStore::credentials_url(self.home());
        AntigravityOAuthCredentialsStore::save(&creds, &url).unwrap();
    }
    fn read_antigravity_credentials_json(&self) -> serde_json::Map<String, Value> {
        let url = AntigravityOAuthCredentialsStore::credentials_url(self.home());
        let data = fs::read(url).unwrap();
        serde_json::from_slice(&data).unwrap()
    }
    fn write_gemini_credentials(&self, access_token: &str, expiry: Timestamp, id_token: &str) {
        let payload = json!({
            "access_token": access_token,
            "expiry_date": expiry.as_millisecond() as f64,
            "id_token": id_token,
        });
        fs::write(
            self.home().join(".gemini").join("oauth_creds.json"),
            serde_json::to_vec(&payload).unwrap(),
        )
        .unwrap();
    }
}
fn available_models_response() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "models":{
            "claude-sonnet-4":{
                "displayName": "Claude Sonnet 4",
                "quotaInfo":{
                    "remainingFraction": 0.5,
                    "resetTime": "2025-01-01T00:00:00Z",
                }
            },
            "gemini-3-pro-low":{
                "displayName": "Gemini 3 Pro Low",
                "quotaInfo":{
                    "remainingFraction": 0.8,
                    "resetTime": "2025-01-01T00:00:00Z",
                }
            },
            "gemini-3-flash":{
                "displayName": "Gemini 3 Flash",
                "quotaInfo":{
                    "remainingFraction": 0.2,
                    "resetTime": "2025-01-01T00:00:00Z",
                }
            },
            "gemini-3-flash-lite":{
                "displayName": "Gemini 3 Flash Lite",
                "quotaInfo":{
                    "remainingFraction": 0.7,
                    "resetTime": "2025-01-01T00:00:00Z",
                }
            }
        }
    }))
    .unwrap()
}
fn response(status: u16, body: Value) -> RemoteHttpResponse {
    RemoteHttpResponse {
        status,
        body: serde_json::to_vec(&body).unwrap(),
    }
}
fn fresh() -> Timestamp {
    Timestamp::now()
        .saturating_add(jiff::Span::new().seconds(3600))
        .unwrap()
}
fn standard_env() -> TestEnvironment {
    let env = TestEnvironment::new();
    env.write_antigravity_credentials(
        "token",
        None,
        fresh(),
        Some(&make_id_token("user@example.com", None)),
        Some("user@example.com"),
        None,
        None,
        None,
    );
    env
}
fn load() -> Value {
    json!({
        "currentTier":{
            "id": "standard-tier",
            "name": "standard"
        },
        "cloudaicompanionProject": "managed-project-123"
    })
}
fn models() -> Value {
    json!({
        "models":{
            "claude-sonnet-4":{
                "displayName": "Claude Sonnet 4",
                "quotaInfo":{
                    "remainingFraction": 1
                }
            },
            "gemini-2.5-pro":{
                "displayName": "Gemini 2.5 Pro",
                "quotaInfo":{
                    "remainingFraction": 1
                }
            }
        }
    })
}
fn permission_denied() -> Value {
    json!({
        "error":{
            "code": 403,
            "message": "The caller does not have permission",
            "status": "PERMISSION_DENIED"
        }
    })
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:20 remote plan respects paid metadata and measured five hour quotas
#[tokio::test]
async fn remote_plan_respects_paid_metadata_and_measured_five_hour_quotas() {
    for scenario in [
        "paid-tier",
        "paid-tier-id",
        "five-hour",
        "weekly",
        "disabled",
        "unmeasured",
    ] {
        let env = TestEnvironment::new();
        env.write_antigravity_credentials(
            "token",
            None,
            Timestamp::now()
                .saturating_add(jiff::Span::new().seconds(3600))
                .unwrap(),
            None,
            Some("user@example.com"),
            Some("test-project"),
            None,
            None,
        );
        let scenario_owned = scenario.to_owned();
        let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
            .with_data_loader(move |req| {
                let scenario = scenario_owned.clone();
                Box::pin(async move {
                    let body = match req.url.path() {
                        "/v1internal:loadCodeAssist" => {
                            let mut map = serde_json::Map::new();
                            map.insert(
                                "currentTier".to_owned(),
                                json!({
                                    "id": "free-tier",
                                    "name": "Free"
                                }),
                            );
                            if scenario == "paid-tier" {
                                map.insert(
                                    "paidTier".to_owned(),
                                    json!({
                                        "id": "g1-pro-tier",
                                        "name": "Google AI Pro"
                                    }),
                                );
                                map.insert(
                                    "planInfo".to_owned(),
                                    json!({
                                        "planType": "Free"
                                    }),
                                );
                            } else if scenario == "paid-tier-id" {
                                map.insert(
                                    "paidTier".to_owned(),
                                    json!({
                                        "id": "standard-tier"
                                    }),
                                );
                                map.insert(
                                    "planInfo".to_owned(),
                                    json!({
                                        "planType": "Free"
                                    }),
                                );
                            }
                            Value::Object(map)
                        }
                        "/v1internal:retrieveUserQuotaSummary" => {
                            let mut buckets = vec![json!({
                                "bucketId": "gemini-weekly",
                                "window": "weekly",
                                "remainingFraction": 0.75,
                            })];
                            if scenario != "weekly" {
                                let mut session = serde_json::Map::new();
                                session.insert("bucketId".to_owned(), json!("gemini-5h"));
                                session.insert("window".to_owned(), json!("5h"));
                                if scenario != "unmeasured" {
                                    session.insert("remainingFraction".to_owned(), json!(0.5));
                                }
                                session
                                    .insert("disabled".to_owned(), json!(scenario == "disabled"));
                                buckets.push(Value::Object(session));
                            }
                            json!({
                                "groups":[{
                                        "displayName": "Gemini Models",
                                        "buckets": buckets
                                    }
                                ]
                            })
                        }
                        other => panic!("unexpected path {other}"),
                    };
                    Ok(RemoteHttpResponse {
                        status: 200,
                        body: serde_json::to_vec(&body).unwrap(),
                    })
                })
            });
        let snapshot = fetcher.fetch().await.unwrap();
        let expected_plan = match scenario {
            "paid-tier" => Some("Google AI Pro"),
            "paid-tier-id" => Some("Paid"),
            "five-hour" => None,
            _ => Some("Free"),
        };
        assert_eq!(snapshot.account_plan.as_deref(), expected_plan);
        let usage = snapshot.to_usage_snapshot().unwrap();
        assert_eq!(
            usage.identity.as_ref().unwrap().login_method.as_deref(),
            expected_plan
        );
        assert_eq!(snapshot.account_email.as_deref(), Some("user@example.com"));
        let weekly_used = usage
            .extra_rate_windows
            .as_ref()
            .unwrap()
            .iter()
            .find(|w| w.window.window_minutes == Some(10_080))
            .unwrap()
            .window
            .used_percent;
        assert_eq!(weekly_used, 25.0);
    }
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:75 antigravity supports token accounts for quick account switching
#[test]
fn antigravity_supports_token_accounts_for_quick_account_switching() {
    assert_eq!(
        AntigravityProviderPresentation::TOKEN_ACCOUNT_TITLE,
        "Google accounts"
    );
    const {
        assert!(!AntigravityProviderPresentation::TOKEN_ACCOUNT_REQUIRES_MANUAL_COOKIE_SOURCE);
    }
    assert_eq!(
        AntigravityProviderPresentation::token_account_env_override("serialized-credentials")
            .get(AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY)
            .map(String::as_str),
        Some("serialized-credentials")
    );
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:87 oauth credentials round trip through token account value
#[test]
fn oauth_credentials_round_trip_through_token_account_value() {
    let credentials = AntigravityOAuthCredentials::new(
        "access-token",
        Some("refresh-token".to_owned()),
        Some(Timestamp::from_second(1_700_000_000).unwrap()),
        Some(make_id_token("user@example.com", None)),
        Some("user@example.com".to_owned()),
        Some("project-123".to_owned()),
        Some("client-id".to_owned()),
        Some("client-secret".to_owned()),
    );
    let token = AntigravityOAuthCredentialsStore::token_account_value(&credentials).unwrap();
    let decoded =
        AntigravityOAuthCredentialsStore::credentials_from_token_account_value(&token).unwrap();
    assert_eq!(decoded, credentials);
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:165 remote fetch refreshes selected token account without mutating shared credentials
#[tokio::test]
async fn remote_fetch_refreshes_selected_token_account_without_mutating_shared_credentials() {
    for suffix in ["", "+&=%2B /東京"] {
        let env = TestEnvironment::new();
        env.write_antigravity_credentials(
            "shared-token",
            Some("shared-refresh"),
            Timestamp::now()
                .saturating_add(jiff::Span::new().seconds(3600))
                .unwrap(),
            Some(&make_id_token("shared@example.com", None)),
            Some("shared@example.com"),
            None,
            Some("shared-client-id"),
            Some("shared-client-secret"),
        );
        let refresh_token = format!("selected-refresh{suffix}");
        let client_id = format!("selected-client-id{suffix}");
        let client_secret = format!("selected-client-secret{suffix}");
        let selected = AntigravityOAuthCredentials::new(
            "selected-old-token",
            Some(refresh_token.clone()),
            Some(
                Timestamp::now()
                    .saturating_sub(jiff::Span::new().seconds(3600))
                    .unwrap(),
            ),
            Some(make_id_token("selected-old@example.com", None)),
            Some("selected-old@example.com".to_owned()),
            None,
            Some(client_id.clone()),
            Some(client_secret.clone()),
        );
        let token = AntigravityOAuthCredentialsStore::token_account_value(&selected).unwrap();
        let captured_updates = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = Arc::clone(&captured_updates);
        let mut env_map = BTreeMap::new();
        env_map.insert(
            AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.to_owned(),
            token,
        );
        let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
            .with_environment(env_map)
            .with_credentials_update_handler(move |creds| {
                let captured = Arc::clone(&captured_clone);
                Box::pin(async move {
                    captured.lock().unwrap().push(creds);
                    Ok(())
                })
            })
            .with_data_loader(move |req| {
                let refresh_token = refresh_token.clone();
                let client_id = client_id.clone();
                let client_secret = client_secret.clone();
                Box::pin(async move {
                    match req.url.host_str().unwrap() {
                        "oauth2.googleapis.com" => {
                            let decoded = decode_form_body(req.body.as_ref().unwrap());
                            let mut expected = BTreeMap::new();
                            expected.insert("client_id".to_owned(), client_id);
                            expected.insert("client_secret".to_owned(), client_secret);
                            expected.insert("refresh_token".to_owned(), refresh_token);
                            expected.insert("grant_type".to_owned(), "refresh_token".to_owned());
                            assert_eq!(decoded, expected);
                            Ok(RemoteHttpResponse {
                                status: 200,
                                body: serde_json::to_vec(&json!({
                                    "access_token": "selected-new-token",
                                    "expires_in": 3600,
                                    "id_token": make_id_token("selected-new@example.com",
                                    None),
                                }))
                                .unwrap(),
                            })
                        }
                        "cloudcode-pa.googleapis.com" => {
                            assert_eq!(
                                req.header("Authorization"),
                                Some("Bearer selected-new-token")
                            );
                            if req.url.path() == "/v1internal:loadCodeAssist" {
                                return Ok(RemoteHttpResponse {
                                    status: 200,
                                    body: serde_json::to_vec(&json!({
                                        "currentTier":{
                                            "id": "free-tier",
                                            "name": "free"
                                        },
                                        "cloudaicompanionProject": "selected-project-123",
                                    }))
                                    .unwrap(),
                                });
                            }
                            if req.url.path() == "/v1internal:fetchAvailableModels" {
                                return Ok(RemoteHttpResponse {
                                    status: 200,
                                    body: available_models_response(),
                                });
                            }
                            Ok(RemoteHttpResponse {
                                status: 404,
                                body: Vec::new(),
                            })
                        }
                        _ => Ok(RemoteHttpResponse {
                            status: 404,
                            body: Vec::new(),
                        }),
                    }
                })
            });
        let snapshot = fetcher.fetch().await.unwrap();
        let shared = env.read_antigravity_credentials_json();
        let updates = captured_updates.lock().unwrap();
        assert_eq!(
            snapshot.account_email.as_deref(),
            Some("selected-new@example.com")
        );
        assert_eq!(shared.get("access_token").unwrap(), "shared-token");
        assert_eq!(shared.get("email").unwrap(), "shared@example.com");
        assert_eq!(updates.last().unwrap().access_token, "selected-new-token");
        assert_eq!(
            updates.last().unwrap().project_id.as_deref(),
            Some("selected-project-123")
        );
    }
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:314 remote fetch rejects invalid selected token account
#[tokio::test]
async fn remote_fetch_rejects_invalid_selected_token_account() {
    let env = TestEnvironment::new();
    env.write_antigravity_credentials(
        "shared-token",
        None,
        fresh(),
        Some(&make_id_token("shared@example.com", None)),
        Some("shared@example.com"),
        None,
        None,
        None,
    );
    let error = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_environment(BTreeMap::from([(
            AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.into(),
            "not-json".into(),
        )]))
        .with_data_loader(|_| {
            Box::pin(async { panic!("invalid selected credentials must fail before a request") })
        })
        .fetch()
        .await
        .unwrap_err();
    assert!(matches!(error,
AntigravityRemoteFetchError::ParseFailed(message) if message.contains("selected account")));
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:105 remote fetch uses selected token account credentials before shared credentials
#[tokio::test]
async fn remote_fetch_uses_selected_token_account_credentials_before_shared_credentials() {
    let env = TestEnvironment::new();
    env.write_antigravity_credentials(
        "shared-token",
        None,
        fresh(),
        Some(&make_id_token("shared@example.com", None)),
        Some("shared@example.com"),
        None,
        None,
        None,
    );
    let selected = AntigravityOAuthCredentials::new(
        "selected-token",
        None,
        Some(fresh()),
        Some(make_id_token("selected@example.com", None)),
        Some("selected@example.com".into()),
        None,
        None,
        None,
    );
    let token = AntigravityOAuthCredentialsStore::token_account_value(&selected).unwrap();
    let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_environment(BTreeMap::from([(
            AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.into(),
            token,
        )]))
        .with_data_loader(|req| {
            Box::pin(async move {
                if req.url.host_str() != Some("cloudcode-pa.googleapis.com") {
                    return Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    });
                }
                assert_eq!(req.header("Authorization"), Some("Bearer selected-token"));
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(response(
                        200,
                        json!({
                            "currentTier":{
                                "id": "free-tier",
                                "name": "free"
                            },
                            "cloudaicompanionProject": "managed-project-123"
                        }),
                    )),
                    "/v1internal:fetchAvailableModels" => Ok(RemoteHttpResponse {
                        status: 200,
                        body: available_models_response(),
                    }),
                    _ => Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    }),
                }
            })
        });
    let snapshot = fetcher.fetch().await.unwrap();
    assert_eq!(
        snapshot.account_email.as_deref(),
        Some("selected@example.com")
    );
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:258 remote fetch ignores selected token account project id persistence failure
#[tokio::test]
async fn remote_fetch_ignores_selected_token_account_project_id_persistence_failure() {
    let env = TestEnvironment::new();
    let selected = AntigravityOAuthCredentials::new(
        "selected-token",
        None,
        Some(fresh()),
        Some(make_id_token("selected@example.com", None)),
        Some("selected@example.com".into()),
        None,
        None,
        None,
    );
    let token = AntigravityOAuthCredentialsStore::token_account_value(&selected).unwrap();
    let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_environment(BTreeMap::from([(
            AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY.into(),
            token,
        )]))
        .with_data_loader(|req| {
            Box::pin(async move {
                if req.url.host_str() != Some("cloudcode-pa.googleapis.com") {
                    return Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    });
                }
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(response(
                        200,
                        json!({
                            "currentTier":{
                                "id": "free-tier",
                                "name": "free"
                            },
                            "cloudaicompanionProject": "selected-project-123"
                        }),
                    )),
                    "/v1internal:fetchAvailableModels" => Ok(RemoteHttpResponse {
                        status: 200,
                        body: available_models_response(),
                    }),
                    _ => Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    }),
                }
            })
        });
    let fetcher = fetcher.with_credentials_update_handler(|_| {
        Box::pin(async {
            Err(AntigravityRemoteFetchError::ApiError(
                "fileWriteUnknown".into(),
            ))
        })
    });
    let snapshot = fetcher.fetch().await.unwrap();
    assert_eq!(
        snapshot.account_email.as_deref(),
        Some("selected@example.com")
    );
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:347 remote fetch maps cloud code models into antigravity usage
#[tokio::test]
async fn remote_fetch_maps_cloud_code_models_into_antigravity_usage() {
    let env = TestEnvironment::new();
    env.write_antigravity_credentials(
        "token",
        None,
        fresh(),
        Some(&make_id_token("user@company.com", Some("company.com"))),
        Some("user@company.com"),
        None,
        None,
        None,
    );
    let snapshot = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_data_loader(|req| {
            Box::pin(async move {
                if req.url.host_str() != Some("cloudcode-pa.googleapis.com") {
                    return Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    });
                }
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(response(200, load())),
                    "/v1internal:fetchAvailableModels" => {
                        let body: Value =
                            serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
                        assert_eq!(body["project"], "managed-project-123");
                        Ok(RemoteHttpResponse {
                            status: 200,
                            body: available_models_response(),
                        })
                    }
                    _ => Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    }),
                }
            })
        })
        .fetch()
        .await
        .unwrap();
    assert_eq!(snapshot.account_email.as_deref(), Some("user@company.com"));
    assert_eq!(snapshot.account_plan.as_deref(), Some("Paid"));
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(
        usage.primary.as_ref().unwrap().remaining_percent().round(),
        20.
    );
    assert_eq!(
        usage
            .secondary
            .as_ref()
            .unwrap()
            .remaining_percent()
            .round(),
        50.
    );
    assert!(usage.tertiary.is_none());
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:404 remote fetch verifies full model quotas with quota endpoint
#[tokio::test]
async fn remote_fetch_verifies_full_model_quotas_with_quota_endpoint() {
    let env = standard_env();
    let quota_calls = Arc::new(AtomicUsize::new(0));
    let counter = quota_calls.clone();
    let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_data_loader(move |req| {
            let counter = counter.clone();
            Box::pin(async move {
                if req.url.host_str() != Some("cloudcode-pa.googleapis.com") {
                    return Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    });
                }
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(response(200, load())),
                    "/v1internal:fetchAvailableModels" => {
                        let mut value = models();
                        value["models"]["gemini-2.5-flash"] = json!({
                            "displayName": "Gemini 2.5 Flash",
                            "quotaInfo":{
                                "remainingFraction": 1
                            }
                        });
                        Ok(response(200, value))
                    }
                    "/v1internal:retrieveUserQuota" => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Ok(response(
                            200,
                            json!({
                                "buckets":[{
                                        "modelId": "claude-sonnet-4",
                                        "resetTime": "2025-01-01T00:00:00Z"
                                    },{
                                        "modelId": "gemini-2.5-pro",
                                        "remainingFraction": 0.6,
                                        "resetTime": "2025-01-01T00:00:00Z"
                                    },{
                                        "modelId": "gemini-2.5-flash",
                                        "remainingFraction": 0.9,
                                        "resetTime": "2025-01-01T00:00:00Z"
                                    }
                                ]
                            }),
                        ))
                    }
                    _ => Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    }),
                }
            })
        });
    let snapshot = fetcher.fetch().await.unwrap();
    assert_eq!(quota_calls.load(Ordering::SeqCst), 1);
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(usage.primary.as_ref().unwrap().remaining_percent(), 60.);
    assert_eq!(usage.secondary.as_ref().unwrap().remaining_percent(), 100.);
    assert!(usage.tertiary.is_none());
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:512 remote fetch ignores full model availability when verification has no quota data
#[tokio::test]
async fn remote_fetch_ignores_full_model_availability_when_verification_has_no_quota_data() {
    let env = standard_env();
    let quota_calls = Arc::new(AtomicUsize::new(0));
    let counter = quota_calls.clone();
    let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_data_loader(move |req| {
            let counter = counter.clone();
            Box::pin(async move {
                if req.url.host_str() != Some("cloudcode-pa.googleapis.com") {
                    return Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    });
                }
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(response(200, load())),
                    "/v1internal:fetchAvailableModels" => Ok(response(200, models())),
                    "/v1internal:retrieveUserQuota" => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Ok(response(
                            200,
                            json!({
                                "buckets":[
                                ]
                            }),
                        ))
                    }
                    _ => Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    }),
                }
            })
        });
    let snapshot = fetcher.fetch().await.unwrap();
    assert!(snapshot.model_quotas.is_empty());
    assert_eq!(snapshot.account_email.as_deref(), Some("user@example.com"));
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:577 remote fetch propagates quota verification server errors
#[tokio::test]
async fn remote_fetch_propagates_quota_verification_server_errors() {
    let env = standard_env();
    let quota_calls = Arc::new(AtomicUsize::new(0));
    let counter = quota_calls.clone();
    let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_data_loader(move |req| {
            let counter = counter.clone();
            Box::pin(async move {
                if req.url.host_str() != Some("cloudcode-pa.googleapis.com") {
                    return Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    });
                }
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(response(200, load())),
                    "/v1internal:fetchAvailableModels" => Ok(response(200, models())),
                    "/v1internal:retrieveUserQuota" => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Ok(RemoteHttpResponse {
                            status: 503,
                            body: b"temporary outage".to_vec(),
                        })
                    }
                    _ => Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    }),
                }
            })
        });
    assert!(matches!(fetcher.fetch().await.unwrap_err(),
AntigravityRemoteFetchError::ApiError(message) if message.contains("HTTP 503")));
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:650 remote fetch keeps full quotas when verified quota endpoint has fractions
#[tokio::test]
async fn remote_fetch_keeps_full_quotas_when_verified_quota_endpoint_has_fractions() {
    let env = standard_env();
    let quota_calls = Arc::new(AtomicUsize::new(0));
    let counter = quota_calls.clone();
    let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_data_loader(move |req| {
            let counter = counter.clone();
            Box::pin(async move {
                if req.url.host_str() != Some("cloudcode-pa.googleapis.com") {
                    return Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    });
                }
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(response(200, load())),
                    "/v1internal:fetchAvailableModels" => Ok(response(200, models())),
                    "/v1internal:retrieveUserQuota" => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Ok(response(
                            200,
                            json!({
                                "buckets":[{
                                        "modelId": "claude-sonnet-4",
                                        "remainingFraction": 1,
                                        "resetTime": "2025-01-01T00:00:00Z"
                                    },{
                                        "modelId": "gemini-2.5-pro",
                                        "remainingFraction": 1,
                                        "resetTime": "2025-01-01T00:00:00Z"
                                    }
                                ]
                            }),
                        ))
                    }
                    _ => Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    }),
                }
            })
        });
    let snapshot = fetcher.fetch().await.unwrap();
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(usage.primary.as_ref().unwrap().remaining_percent(), 100.);
    assert_eq!(usage.secondary.as_ref().unwrap().remaining_percent(), 100.);
    assert!(usage.tertiary.is_none());
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:730 remote fetch drops full quota rows absent from partial verification
#[tokio::test]
async fn remote_fetch_drops_full_quota_rows_absent_from_partial_verification() {
    let env = standard_env();
    let quota_calls = Arc::new(AtomicUsize::new(0));
    let counter = quota_calls.clone();
    let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_data_loader(move |req| {
            let counter = counter.clone();
            Box::pin(async move {
                if req.url.host_str() != Some("cloudcode-pa.googleapis.com") {
                    return Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    });
                }
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(response(200, load())),
                    "/v1internal:fetchAvailableModels" => Ok(response(200, models())),
                    "/v1internal:retrieveUserQuota" => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Ok(response(
                            200,
                            json!({
                                "buckets":[{
                                        "modelId": "gemini-2.5-pro",
                                        "remainingFraction": 0.5
                                    }
                                ]
                            }),
                        ))
                    }
                    _ => Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    }),
                }
            })
        });
    let snapshot = fetcher.fetch().await.unwrap();
    assert_eq!(
        snapshot
            .model_quotas
            .iter()
            .map(|q| q.model_id.as_str())
            .collect::<Vec<_>>(),
        ["gemini-2.5-pro"]
    );
    assert_eq!(
        snapshot
            .model_quotas
            .iter()
            .map(|q| q.remaining_fraction)
            .collect::<Vec<_>>(),
        [Some(0.5)]
    );
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:1037 remote fetch falls back to retrieve user quota when model endpoint is forbidden
#[tokio::test]
async fn remote_fetch_falls_back_to_retrieve_user_quota_when_model_endpoint_is_forbidden() {
    let env = standard_env();
    let quota_calls = Arc::new(AtomicUsize::new(0));
    let counter = quota_calls.clone();
    let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_data_loader(move |req| {
            let counter = counter.clone();
            Box::pin(async move {
                if req.url.host_str() != Some("cloudcode-pa.googleapis.com") {
                    return Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    });
                }
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(response(200, load())),
                    "/v1internal:fetchAvailableModels" => Ok(response(403, permission_denied())),
                    "/v1internal:retrieveUserQuota" => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Ok(response(
                            200,
                            json!({
                                "buckets":[{
                                        "modelId": "gemini-2.5-pro",
                                        "remainingFraction": 0.6,
                                        "resetTime": "2025-01-01T00:00:00Z"
                                    },{
                                        "modelId": "gemini-2.5-flash",
                                        "remainingFraction": 0.9,
                                        "resetTime": "2025-01-01T00:00:00Z"
                                    },{
                                        "modelId": "gemini-2.5-flash-lite",
                                        "remainingFraction": 0.8,
                                        "resetTime": "2025-01-01T00:00:00Z"
                                    }
                                ]
                            }),
                        ))
                    }
                    _ => Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    }),
                }
            })
        });
    let snapshot = fetcher.fetch().await.unwrap();
    assert_eq!(quota_calls.load(Ordering::SeqCst), 1);
    let usage = snapshot.to_usage_snapshot().unwrap();
    assert_eq!(usage.primary.as_ref().unwrap().remaining_percent(), 60.);
    assert!(usage.secondary.is_none());
    assert!(usage.tertiary.is_none());
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:1126 remote fetch returns identity when both remote quota endpoints are forbidden
#[tokio::test]
async fn remote_fetch_returns_identity_when_both_remote_quota_endpoints_are_forbidden() {
    let env = standard_env();
    let quota_calls = Arc::new(AtomicUsize::new(0));
    let counter = quota_calls.clone();
    let fetcher = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_data_loader(move |req| {
            let counter = counter.clone();
            Box::pin(async move {
                if req.url.host_str() != Some("cloudcode-pa.googleapis.com") {
                    return Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    });
                }
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(response(200, load())),
                    "/v1internal:fetchAvailableModels" => Ok(response(403, permission_denied())),
                    "/v1internal:retrieveUserQuota" => {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Ok(response(403, permission_denied()))
                    }
                    _ => Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    }),
                }
            })
        });
    let snapshot = fetcher.fetch().await.unwrap();
    let usage = AntigravityOAuthFetchStrategy::usage_snapshot_from(&snapshot).unwrap();
    assert!(snapshot.model_quotas.is_empty());
    assert_eq!(snapshot.account_email.as_deref(), Some("user@example.com"));
    assert_eq!(snapshot.account_plan.as_deref(), Some("Paid"));
    assert!(AntigravityOAuthFetchStrategy::rate_limits_unavailable(
        &usage
    ));
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:796 remote fetch refreshes expired shared google token
#[tokio::test]
async fn remote_fetch_refreshes_expired_shared_google_token() {
    let env_refresh = TestEnvironment::new();
    env_refresh.write_antigravity_credentials(
        "old-token",
        Some("refresh-token"),
        Timestamp::now()
            .saturating_sub(jiff::Span::new().seconds(3600))
            .unwrap(),
        Some(&make_id_token("stale@example.com", None)),
        Some("stale@example.com"),
        None,
        Some("test-client-id"),
        Some("test-client-secret"),
    );
    let fetcher_refresh =
        AntigravityRemoteUsageFetcher::new(Duration::from_secs(2), env_refresh.home())
            .with_data_loader(|req| {
                Box::pin(async move {
                    match req.url.host_str().unwrap() {
                        "oauth2.googleapis.com" => Ok(RemoteHttpResponse {
                            status: 200,
                            body: serde_json::to_vec(&json!({
                                "access_token": "new-token",
                                "expires_in": 3600,
                                "id_token": make_id_token("refreshed@example.com",
                                None),
                            }))
                            .unwrap(),
                        }),
                        "cloudcode-pa.googleapis.com" => {
                            assert_eq!(req.header("Authorization"), Some("Bearer new-token"));
                            if req.url.path() == "/v1internal:loadCodeAssist" {
                                return Ok(RemoteHttpResponse {
                                    status: 200,
                                    body: serde_json::to_vec(&json!({
                                        "currentTier":{
                                            "id": "standard-tier",
                                            "name": "standard"
                                        },
                                        "cloudaicompanionProject": "managed-project-123",
                                    }))
                                    .unwrap(),
                                });
                            }
                            if req.url.path() == "/v1internal:fetchAvailableModels" {
                                return Ok(RemoteHttpResponse {
                                    status: 200,
                                    body: available_models_response(),
                                });
                            }
                            Ok(RemoteHttpResponse {
                                status: 404,
                                body: Vec::new(),
                            })
                        }
                        _ => Ok(RemoteHttpResponse {
                            status: 404,
                            body: Vec::new(),
                        }),
                    }
                })
            });
    let refreshed_snap = fetcher_refresh.fetch().await.unwrap();
    let updated_json = env_refresh.read_antigravity_credentials_json();
    assert_eq!(updated_json.get("access_token").unwrap(), "new-token");
    assert_eq!(updated_json.get("client_id").unwrap(), "test-client-id");
    assert_eq!(
        updated_json.get("client_secret").unwrap(),
        "test-client-secret"
    );
    assert_eq!(
        refreshed_snap.account_email.as_deref(),
        Some("refreshed@example.com")
    );
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:859 remote fetch refreshes nearly expired shared google token
#[tokio::test]
async fn remote_fetch_refreshes_nearly_expired_shared_google_token() {
    let env_refresh = TestEnvironment::new();
    env_refresh.write_antigravity_credentials(
        "old-token",
        Some("refresh-token"),
        Timestamp::now()
            .saturating_add(jiff::Span::new().seconds(5))
            .unwrap(),
        Some(&make_id_token("stale@example.com", None)),
        Some("stale@example.com"),
        None,
        Some("test-client-id"),
        Some("test-client-secret"),
    );
    let fetcher_refresh =
        AntigravityRemoteUsageFetcher::new(Duration::from_secs(2), env_refresh.home())
            .with_data_loader(|req| {
                Box::pin(async move {
                    match req.url.host_str().unwrap() {
                        "oauth2.googleapis.com" => Ok(RemoteHttpResponse {
                            status: 200,
                            body: serde_json::to_vec(&json!({
                                "access_token": "new-token",
                                "expires_in": 3600,
                                "id_token": make_id_token("refreshed@example.com",
                                None),
                            }))
                            .unwrap(),
                        }),
                        "cloudcode-pa.googleapis.com" => {
                            assert_eq!(req.header("Authorization"), Some("Bearer new-token"));
                            if req.url.path() == "/v1internal:loadCodeAssist" {
                                return Ok(RemoteHttpResponse {
                                    status: 200,
                                    body: serde_json::to_vec(&json!({
                                        "currentTier":{
                                            "id": "standard-tier",
                                            "name": "standard"
                                        },
                                        "cloudaicompanionProject": "managed-project-123",
                                    }))
                                    .unwrap(),
                                });
                            }
                            if req.url.path() == "/v1internal:fetchAvailableModels" {
                                return Ok(RemoteHttpResponse {
                                    status: 200,
                                    body: available_models_response(),
                                });
                            }
                            Ok(RemoteHttpResponse {
                                status: 404,
                                body: Vec::new(),
                            })
                        }
                        _ => Ok(RemoteHttpResponse {
                            status: 404,
                            body: Vec::new(),
                        }),
                    }
                })
            });
    let refreshed_snap = fetcher_refresh.fetch().await.unwrap();
    let updated_json = env_refresh.read_antigravity_credentials_json();
    assert_eq!(updated_json.get("access_token").unwrap(), "new-token");
    assert_eq!(updated_json.get("client_id").unwrap(), "test-client-id");
    assert_eq!(
        updated_json.get("client_secret").unwrap(),
        "test-client-secret"
    );
    assert_eq!(
        refreshed_snap.account_email.as_deref(),
        Some("refreshed@example.com")
    );
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:921 remote refresh requires configured oauth client
#[tokio::test]
async fn remote_refresh_requires_configured_oauth_client() {
    let env_no_client = TestEnvironment::new();
    env_no_client.write_antigravity_credentials(
        "old-token",
        Some("refresh-token"),
        Timestamp::now()
            .saturating_sub(jiff::Span::new().seconds(3600))
            .unwrap(),
        Some(&make_id_token("user@example.com", None)),
        Some("user@example.com"),
        None,
        None,
        None,
    );
    let err_no_client =
        AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env_no_client.home())
            .with_oauth_client_resolver(|| None)
            .fetch()
            .await
            .unwrap_err();
    assert!(matches!(
        err_no_client,
        AntigravityRemoteFetchError::ApiError(ref msg) if msg.contains("ANTIGRAVITY_OAUTH_CLIENT_ID")
    ));
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:1184 remote fetch ignores gemini credentials when antigravity auth is missing
#[tokio::test]
async fn remote_fetch_ignores_gemini_credentials_when_antigravity_auth_is_missing() {
    let env_gemini_only = TestEnvironment::new();
    env_gemini_only.write_gemini_credentials(
        "gemini-token",
        Timestamp::now()
            .saturating_add(jiff::Span::new().seconds(3600))
            .unwrap(),
        &make_id_token("gemini@example.com", None),
    );
    let err_missing =
        AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env_gemini_only.home())
            .fetch()
            .await
            .unwrap_err();
    assert_eq!(err_missing, AntigravityRemoteFetchError::NotLoggedIn);
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:1280 consumer client recovery preserves onboarded and unrelated eligibility responses
#[tokio::test]
async fn consumer_client_recovery_preserves_onboarded_and_unrelated_eligibility_responses() {
    for case_name in [
        "stored-project",
        "returned-project",
        "current-tier",
        "other-reason",
    ] {
        let env_case = TestEnvironment::new();
        env_case.write_antigravity_credentials(
            "token",
            None,
            Timestamp::now()
                .saturating_add(jiff::Span::new().seconds(3600))
                .unwrap(),
            None,
            Some("user@example.com"),
            if case_name == "stored-project" {
                Some("saved-project")
            } else {
                None
            },
            None,
            None,
        );
        let case_owned = case_name.to_owned();
        let snap_case = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env_case.home())
            .with_data_loader(move |req| {
                let case_name = case_owned.clone();
                Box::pin(async move {
                    let body = match req.url.path() {
                        "/v1internal:loadCodeAssist" => {
                            let mut map = serde_json::Map::new();
                            let reason = if case_name == "other-reason" {
                                "OTHER_REASON"
                            } else {
                                "GOOGLE_TOS_NOT_SUPPORTED_BY_CLIENT"
                            };
                            map.insert(
                                "ineligibleTiers".to_owned(),
                                json!([{
                                        "reasonCode": reason
                                    }
                                ]),
                            );
                            if case_name == "returned-project" {
                                map.insert(
                                    "cloudaicompanionProject".to_owned(),
                                    json!("returned-project"),
                                );
                            }
                            if case_name == "current-tier" {
                                map.insert(
                                    "currentTier".to_owned(),
                                    json!({
                                        "id": "standard-tier"
                                    }),
                                );
                            }
                            Value::Object(map)
                        }
                        "/v1internal:onboardUser" => {
                            json!({
                                "response":{
                                    "cloudaicompanionProject": "onboarded-project"
                                }
                            })
                        }
                        _ => json!({
                            "groups":[{
                                    "displayName": "All Models",
                                    "buckets":[{
                                            "bucketId": "gemini-3.1-pro-high",
                                            "remainingFraction": 0.5
                                        }
                                    ]
                                }
                            ]
                        }),
                    };
                    Ok(RemoteHttpResponse {
                        status: 200,
                        body: serde_json::to_vec(&body).unwrap(),
                    })
                })
            })
            .fetch()
            .await
            .unwrap();
        assert!(snap_case.has_known_quota_summary());
        assert_eq!(snap_case.account_email.as_deref(), Some("user@example.com"));
    }
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:1120 antigravity descriptor advertises oauth mode
#[test]
fn antigravity_descriptor_advertises_oauth_mode() {
    let descriptor = find_by_id_str("antigravity").unwrap();
    assert_eq!(
        descriptor.source_modes,
        [SourceMode::Auto, SourceMode::Cli, SourceMode::Oauth]
    );
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:954 remote fetch onboards project before fetching models
#[tokio::test]
async fn remote_fetch_onboards_project_before_fetching_models() {
    let env = standard_env();
    let projects = Arc::new(Mutex::new(Vec::new()));
    let recorder = projects.clone();
    AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_data_loader(move |req| {
            let recorder = recorder.clone();
            Box::pin(async move {
                if req.url.host_str() != Some("cloudcode-pa.googleapis.com") {
                    return Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    });
                }
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(response(
                        200,
                        json!({
                            "currentTier":{
                                "id": "standard-tier",
                                "name": "standard"
                            },
                            "allowedTiers":[{
                                    "id": "standard-tier",
                                    "isDefault": true
                                }
                            ]
                        }),
                    )),
                    "/v1internal:onboardUser" => Ok(response(
                        200,
                        json!({
                            "response":{
                                "cloudaicompanionProject":{
                                    "id": "onboarded-project-456"
                                }
                            }
                        }),
                    )),
                    "/v1internal:fetchAvailableModels" => {
                        let body: Value =
                            serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
                        if let Some(project) = body["project"].as_str() {
                            recorder.lock().unwrap().push(project.to_owned());
                        }
                        Ok(RemoteHttpResponse {
                            status: 200,
                            body: available_models_response(),
                        })
                    }
                    _ => Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    }),
                }
            })
        })
        .fetch()
        .await
        .unwrap();
    assert_eq!(
        projects.lock().unwrap().last().map(String::as_str),
        Some("onboarded-project-456")
    );
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:1319 remote fetch prefers stored project id from antigravity credentials
#[tokio::test]
async fn remote_fetch_prefers_stored_project_id_from_antigravity_credentials() {
    let env = TestEnvironment::new();
    env.write_antigravity_credentials(
        "token",
        None,
        fresh(),
        Some(&make_id_token("user@example.com", None)),
        Some("user@example.com"),
        Some("stored-project-789"),
        None,
        None,
    );
    let projects = Arc::new(Mutex::new(Vec::new()));
    let recorder = projects.clone();
    AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env.home())
        .with_data_loader(move |req| {
            let recorder = recorder.clone();
            Box::pin(async move {
                if req.url.host_str() != Some("cloudcode-pa.googleapis.com") {
                    return Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    });
                }
                match req.url.path() {
                    "/v1internal:loadCodeAssist" => Ok(response(
                        200,
                        json!({
                            "currentTier":{
                                "id": "standard-tier",
                                "name": "standard"
                            }
                        }),
                    )),
                    "/v1internal:fetchAvailableModels" => {
                        let body: Value =
                            serde_json::from_slice(req.body.as_ref().unwrap()).unwrap();
                        if let Some(project) = body["project"].as_str() {
                            recorder.lock().unwrap().push(project.to_owned());
                        }
                        Ok(RemoteHttpResponse {
                            status: 200,
                            body: available_models_response(),
                        })
                    }
                    _ => Ok(RemoteHttpResponse {
                        status: 404,
                        body: vec![],
                    }),
                }
            })
        })
        .fetch()
        .await
        .unwrap();
    assert_eq!(
        projects.lock().unwrap().last().map(String::as_str),
        Some("stored-project-789")
    );
}

// upstream: AntigravityRemoteUsageFetcherTests.swift:1206 remote fetch asks for sign in again when the oauth client cannot read consumer quota
#[tokio::test]
async fn remote_fetch_asks_for_sign_in_again_when_the_oauth_client_cannot_read_consumer_quota() {
    let env_tos = TestEnvironment::new();
    env_tos.write_antigravity_credentials(
        "token",
        None,
        Timestamp::now()
            .saturating_add(jiff::Span::new().seconds(3600))
            .unwrap(),
        Some(&make_id_token("user@example.com", None)),
        Some("user@example.com"),
        None,
        None,
        None,
    );
    let tos_paths = Arc::new(Mutex::new(Vec::new()));
    let tos_paths_clone = Arc::clone(&tos_paths);
    let fetcher_tos = AntigravityRemoteUsageFetcher::new(Duration::from_secs(1), env_tos.home())
        .with_data_loader(move |req| {
            let paths = Arc::clone(&tos_paths_clone);
            Box::pin(async move {
                paths.lock().unwrap().push(req.url.path().to_owned());
                if req.url.path() != "/v1internal:loadCodeAssist" {
                    return Ok(response(
                        200,
                        json!({
                            "groups":[{
                                    "displayName": "All Models",
                                    "buckets":[{
                                            "bucketId": "gemini-3.1-pro-high",
                                            "remainingFraction": 1
                                        }
                                    ]
                                }
                            ]
                        }),
                    ));
                }
                Ok(RemoteHttpResponse {
                    status: 200,
                    body: serde_json::to_vec(&json!({
                        "allowedTiers":[{
                                "id": "standard-tier",
                                "isDefault": true,
                                "userDefinedCloudaicompanionProject": true
                            }
                        ],
                        "ineligibleTiers":[{
                                "reasonCode": "GOOGLE_TOS_NOT_SUPPORTED_BY_CLIENT",
                                "reasonMessage": "Client does not support Google TOS.",
                                "tierId": "free-tier"
                            }
                        ]
                    }))
                    .unwrap(),
                })
            })
        });
    let err_tos = fetcher_tos.fetch().await.unwrap_err();
    assert_eq!(
        err_tos,
        AntigravityRemoteFetchError::ReauthenticationRequired
    );
    assert_eq!(
        *tos_paths.lock().unwrap(),
        vec!["/v1internal:loadCodeAssist".to_owned()]
    );
    let diag = AntigravityOfflineFetchStrategy::diagnostic_for_prior_failure(
        &AntigravityRemoteFetchError::ReauthenticationRequired,
    );
    assert!(diag.unwrap().contains("Sign in to it again."));
}
