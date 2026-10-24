// Ported from CodexBar Tests/CodexBarTests/CodexOAuthExpiryPipelineTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

#![cfg(unix)]

use ariadusage_core::{pipeline::*, providers::SourceMode};
use ariadusage_engine::brokers::net::NetConfig;
use ariadusage_engine::providers::codex::*;
use ariadusage_protocol::{ProviderId, metric::SourceKind};
use jiff::Timestamp;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, os::unix::fs::MetadataExt, sync::Arc, time::Duration};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
#[path = "support/https.rs"]
mod https;
fn base64url_no_pad(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0x3f) as usize] as char);
        }
    }
    out
}

fn make_jwt(claims: &Value) -> String {
    make_jwt_raw(&serde_json::to_vec(claims).unwrap())
}

fn make_jwt_raw(payload_bytes: &[u8]) -> String {
    let header = base64url_no_pad(r#"{"alg":"none","typ":"JWT"}"#.as_bytes());
    let payload = base64url_no_pad(payload_bytes);
    format!("{header}.{payload}.signature")
}

const USAGE: &str = r####"{
  "plan_type":"pro",
  "rate_limit":{
    "primary_window":{"used_percent":22,"reset_at":4102444800,"limit_window_seconds":18000},
    "secondary_window":{"used_percent":43,"reset_at":4102444800,"limit_window_seconds":604800}
  },
  "additional_rate_limits":[{
    "limit_name":"GPT-5.3-Codex-Spark",
    "metered_feature":"gpt_5_3_codex_spark",
    "rate_limit":{
      "primary_window":{"used_percent":30,"reset_at":4102444800,"limit_window_seconds":18000},
      "secondary_window":{"used_percent":80,"reset_at":4102444800,"limit_window_seconds":604800}
    }
  }]
}"####;

fn context(mode: SourceMode) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("codex").unwrap(),
        runtime: FetchRuntime::Cli,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "fixture".into(),
        source_mode: mode,
        env: BTreeMap::new(),
        include_credits: false,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    }
}
struct Fixture {
    home: TempDir,
    token: String,
    data: Vec<u8>,
    meta: fs::Metadata,
    cfg: CodexStrategyConfig,
}
impl Fixture {
    fn new(expiry: i64, managed: bool, base: &str) -> Self {
        let home = TempDir::new().unwrap();
        let token = make_jwt(&json!({"exp":expiry}));
        let data=serde_json::to_vec(&json!({"tokens":{"access_token":token,"refresh_token":"fixture-refresh","account_id":"fixture-account"},"last_refresh":"2000-01-01T00:00:00Z"})).unwrap();
        let path = home.path().join("auth.json");
        fs::write(&path, &data).unwrap();
        let meta = fs::metadata(&path).unwrap();
        fs::write(
            home.path().join("config.toml"),
            format!("chatgpt_base_url = \"{base}/backend-api\""),
        )
        .unwrap();
        let env = BTreeMap::from([
            (
                "CODEX_HOME".into(),
                home.path().to_string_lossy().into_owned(),
            ),
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("PATH".into(), String::new()),
        ]);
        let mut cfg = CodexStrategyConfig::new(
            Arc::new(https::broker(NetConfig::default())),
            env,
            home.path(),
            home.path(),
        );
        if managed {
            cfg.managed_workspace_account_id = Some("fixture-workspace".into());
        }
        Self {
            home,
            token,
            data,
            meta,
            cfg,
        }
    }
    fn unchanged(&self) {
        let path = self.home.path().join("auth.json");
        assert_eq!(fs::read(&path).unwrap(), self.data);
        let meta = fs::metadata(path).unwrap();
        assert_eq!(meta.modified().unwrap(), self.meta.modified().unwrap());
        assert_eq!(meta.ino(), self.meta.ino());
    }
    fn credentials(&self) -> CodexOAuthCredentials {
        CodexOAuthCredentialsStore::parse(&self.data, CodexOAuthCredentialSource::CodexHome)
            .unwrap()
    }
}
struct Probe {
    id: String,
    cfg: CodexStrategyConfig,
}
impl FetchStrategy for Probe {
    fn id(&self) -> &str {
        &self.id
    }
    fn kind(&self) -> SourceKind {
        SourceKind::Cli
    }
    fn is_available<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            if self.id == "codex.oauth-native-refresh-cli" {
                CodexOAuthNativeRefreshCLIStrategy::new(self.cfg.clone())
                    .with_binary_resolver(|| true)
                    .is_available_sync(cx.source_mode)
            } else {
                true
            }
        })
    }
    fn fetch<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async {
            Err(FetchError::Classified(ClassifiedError::new(
                ariadusage_protocol::usage::ProviderErrorKind::ProviderUnavailable,
                "CLISelected",
            )))
        })
    }
    fn should_fallback(&self, _: &FetchError, _: &FetchContext) -> bool {
        false
    }
}
struct Plan(CodexStrategyConfig);
impl ProviderPlan for Plan {
    fn resolve_strategies<'a>(
        &'a self,
        cx: &'a FetchContext,
    ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
        Box::pin(async move {
            CodexProviderPlan::new(self.0.clone())
                .resolve_strategies(cx)
                .await
                .into_iter()
                .map(|s| {
                    if s.kind() == SourceKind::Cli {
                        Box::new(Probe {
                            id: s.id().into(),
                            cfg: self.0.clone(),
                        }) as Box<dyn FetchStrategy>
                    } else {
                        s
                    }
                })
                .collect()
        })
    }
}
struct NoRetry;
impl Sleeper for NoRetry {
    fn sleep<'a>(&'a self, _: Duration) -> BoxFuture<'a, ()> {
        Box::pin(async { panic!("No retry expected") })
    }
}
async fn pipeline(cfg: CodexStrategyConfig, cx: &FetchContext) -> FetchOutcome {
    fetch_outcome(&cx.provider, &Plan(cfg), cx, &NoRetry).await
}
fn error_message(error: &FetchError) -> String {
    match error {
        FetchError::Classified(c) => c.message.clone(),
        _ => error.to_string(),
    }
}
fn active_ids(outcome: &FetchOutcome) -> Vec<&str> {
    outcome
        .attempts
        .iter()
        .filter(|a| a.was_available)
        .map(|a| a.strategy_id.as_str())
        .collect()
}

// upstream: CodexOAuthExpiryPipelineTests.swift:9 OAuth fetch retries an owner publication in progress
#[tokio::test]
async fn oauth_fetch_retries_an_owner_publication_in_progress() {
    for publication in ["missing", "partial", "incomplete", "expired", "near-expiry"] {
        let server = https::start_server();
        let fresh = Fixture::new(4_102_444_800, true, &server.base_url());
        let stale = Fixture::new(
            if publication == "near-expiry" {
                Timestamp::now().as_second() + 120
            } else {
                1
            },
            true,
            &server.base_url(),
        );
        let mut reads = 0;
        let c =
            CodexOAuthFetchStrategy::load_credentials_with_reader(true, Timestamp::now(), || {
                reads += 1;
                if reads != 1 {
                    return Ok(fresh.credentials());
                }
                match publication {
                    "missing" => Err(CodexOAuthCredentialsError::NotFound),
                    "partial" => CodexOAuthCredentialsStore::parse(
                        br#"{"tokens":"#,
                        CodexOAuthCredentialSource::CodexHome,
                    ),
                    "incomplete" => CodexOAuthCredentialsStore::parse(
                        br#"{"tokens":{}}"#,
                        CodexOAuthCredentialSource::CodexHome,
                    ),
                    _ => Ok(stale.credentials()),
                }
            })
            .await
            .unwrap();
        let mock = server
            .mock_async(|when, then| {
                when.path("/backend-api/wham/usage")
                    .header("authorization", format!("Bearer {}", fresh.token))
                    .header("chatgpt-account-id", "fixture-workspace");
                then.status(200).body(USAGE);
            })
            .await;
        let r = CodexOAuthFetchStrategy::new(fresh.cfg.clone())
            .fetch_with_credentials(&context(SourceMode::Oauth), c)
            .await
            .unwrap();
        assert_eq!(r.usage.primary.unwrap().used_percent, 22.);
        assert_eq!(reads, 2);
        mock.assert_calls_async(1).await;
        fresh.unchanged();
    }
}

// upstream: CodexOAuthExpiryPipelineTests.swift:44 OAuth availability retries a partial credential publication
#[tokio::test]
async fn oauth_availability_retries_a_partial_credential_publication() {
    let server = https::start_server();
    let fresh = Fixture::new(4_102_444_800, true, &server.base_url());
    let mut reads = 0;
    let available =
        CodexOAuthFetchStrategy::load_credentials_with_reader(false, Timestamp::now(), || {
            reads += 1;
            if reads == 1 {
                CodexOAuthCredentialsStore::parse(b"{", CodexOAuthCredentialSource::CodexHome)
            } else {
                Ok(fresh.credentials())
            }
        })
        .await
        .is_ok();
    assert!(available);
    assert_eq!(reads, 2);
}

// upstream: CodexOAuthExpiryPipelineTests.swift:60 OAuth read retries are bounded and preserve the final error
#[tokio::test]
async fn oauth_read_retries_are_bounded_and_preserve_the_final_error() {
    for failure in ["missing", "partial", "incomplete", "expired", "unreadable"] {
        let server = https::start_server();
        let unexpected = server
            .mock_async(|w, t| {
                w.any_request();
                t.status(500);
            })
            .await;
        let stale = Fixture::new(1, true, &server.base_url());
        let mut reads = 0;
        let loaded =
            CodexOAuthFetchStrategy::load_credentials_with_reader(true, Timestamp::now(), || {
                reads += 1;
                match failure {
                    "missing" => Err(CodexOAuthCredentialsError::NotFound),
                    "unreadable" => Err(CodexOAuthCredentialsError::Unreadable),
                    "partial" => CodexOAuthCredentialsStore::parse(
                        b"{",
                        CodexOAuthCredentialSource::CodexHome,
                    ),
                    "incomplete" => CodexOAuthCredentialsStore::parse(
                        br#"{"tokens":{}}"#,
                        CodexOAuthCredentialSource::CodexHome,
                    ),
                    _ => Ok(stale.credentials()),
                }
            })
            .await;
        let error = loaded
            .and_then(|c| {
                CodexOAuthFetchStrategy::prepare_credentials_for_usage(&c, Timestamp::now())
            })
            .unwrap_err();
        match failure {
            "missing" => assert_eq!(error, CodexOAuthCredentialsError::NotFound),
            "partial" => assert!(matches!(error, CodexOAuthCredentialsError::DecodeFailed(_))),
            "incomplete" => assert_eq!(error, CodexOAuthCredentialsError::MissingTokens),
            "unreadable" => assert_eq!(error, CodexOAuthCredentialsError::Unreadable),
            _ => assert_eq!(error, CodexOAuthCredentialsError::NativeRefreshRequired),
        }
        assert_eq!(reads, 3);
        unexpected.assert_calls_async(0).await;
        stale.unchanged();
    }
}

// upstream: CodexOAuthExpiryPipelineTests.swift:101 cancelled OAuth fetch does not read credentials
#[tokio::test]
async fn cancelled_oauth_fetch_does_not_read_credentials() {
    let server = https::start_server();
    let fresh = Fixture::new(4_102_444_800, true, &server.base_url());
    let call = https::call();
    call.cancel.cancel();
    let mut reads = 0;
    let result = CodexOAuthFetchStrategy::load_credentials_with_reader_and_call(
        true,
        Timestamp::now(),
        Some(&call),
        || {
            reads += 1;
            Err(CodexOAuthCredentialsError::NotFound)
        },
    )
    .await;
    assert_eq!(result.unwrap_err(), CodexOAuthCredentialsError::Cancelled);
    assert_eq!(reads, 0);
    let cx = context(SourceMode::Oauth);
    cx.cancel.cancel();
    assert!(matches!(
        CodexOAuthFetchStrategy::new(fresh.cfg).fetch(&cx).await,
        Err(FetchError::Cancelled)
    ));
}

// upstream: CodexOAuthExpiryPipelineTests.swift:128 managed refresh observes owner credential replacement on the next fetch
#[tokio::test]
async fn managed_refresh_observes_owner_credential_replacement_on_the_next_fetch() {
    for mode in [SourceMode::Auto, SourceMode::Oauth] {
        let server = https::start_server();
        let unexpected = server
            .mock_async(|w, t| {
                w.any_request();
                t.status(500);
            })
            .await;
        let stale = Fixture::new(1, true, &server.base_url());
        let fresh = Fixture::new(4_102_444_800, true, &server.base_url());
        let cx = context(mode);
        let outcome = pipeline(stale.cfg.clone(), &cx).await;
        assert_eq!(
            error_message(outcome.result.as_ref().unwrap_err()),
            CodexOAuthCredentialsError::NativeRefreshRequired.to_string()
        );
        unexpected.assert_calls_async(0).await;
        unexpected.delete_async().await;
        fs::write(stale.home.path().join("auth.json"), &fresh.data).unwrap();
        let mock = server
            .mock_async(|when, then| {
                when.path("/backend-api/wham/usage")
                    .header("authorization", format!("Bearer {}", fresh.token))
                    .header("chatgpt-account-id", "fixture-workspace");
                then.status(200).body(USAGE);
            })
            .await;
        for _ in 0..2 {
            let outcome = pipeline(stale.cfg.clone(), &cx).await;
            assert_eq!(active_ids(&outcome), ["codex.oauth"]);
            let r = outcome.result.unwrap();
            assert_eq!(r.source_label, "oauth");
            assert_eq!(r.usage.primary.unwrap().used_percent, 22.);
        }
        mock.assert_calls_async(2).await;
        assert_eq!(
            fs::read(stale.home.path().join("auth.json")).unwrap(),
            fresh.data
        );
    }
}

// upstream: CodexOAuthExpiryPipelineTests.swift:216 future expiry keeps OAuth model windows and account scope despite old refresh age
#[tokio::test]
async fn future_expiry_keeps_oauth_model_windows_and_account_scope_despite_old_refresh_age() {
    for mode in [SourceMode::Auto, SourceMode::Oauth] {
        for managed in [false, true] {
            let server = https::start_server();
            let f = Fixture::new(4_102_444_800, managed, &server.base_url());
            let mock = server
                .mock_async(|when, then| {
                    when.method(httpmock::Method::GET)
                        .path("/backend-api/wham/usage")
                        .header("authorization", format!("Bearer {}", f.token))
                        .header(
                            "chatgpt-account-id",
                            if managed {
                                "fixture-workspace"
                            } else {
                                "fixture-account"
                            },
                        );
                    then.status(200).body(USAGE);
                })
                .await;
            let outcome = pipeline(f.cfg.clone(), &context(mode)).await;
            assert_eq!(active_ids(&outcome), ["codex.oauth"]);
            let r = outcome.result.unwrap();
            assert_eq!(r.strategy_id, "codex.oauth");
            assert_eq!(r.usage.primary.unwrap().used_percent, 22.);
            assert_eq!(r.usage.secondary.unwrap().used_percent, 43.);
            let extras = r.usage.extra_rate_windows.unwrap();
            assert_eq!(
                extras.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
                ["codex-spark", "codex-spark-weekly"]
            );
            assert_eq!(
                extras
                    .iter()
                    .map(|w| w.window.used_percent)
                    .collect::<Vec<_>>(),
                [30., 80.]
            );
            mock.assert_calls_async(1).await;
            f.unchanged();
        }
    }
}

// upstream: CodexOAuthExpiryPipelineTests.swift:245 expired and near expiry native tokens retain CLI ownership and managed scope
#[tokio::test]
async fn expired_and_near_expiry_native_tokens_retain_cli_ownership_and_managed_scope() {
    for mode in [SourceMode::Auto, SourceMode::Oauth] {
        for managed in [false, true] {
            for offset in [-60, 120] {
                let server = https::start_server();
                let unexpected = server
                    .mock_async(|w, t| {
                        w.any_request();
                        t.status(500);
                    })
                    .await;
                let mut f = Fixture::new(
                    Timestamp::now().as_second() + offset,
                    managed,
                    &server.base_url(),
                );
                let mut value: Value = serde_json::from_slice(&f.data).unwrap();
                value["last_refresh"] = json!(Timestamp::now().to_string());
                f.data = serde_json::to_vec(&value).unwrap();
                fs::write(f.home.path().join("auth.json"), &f.data).unwrap();
                f.meta = fs::metadata(f.home.path().join("auth.json")).unwrap();
                assert_eq!(
                    CodexOAuthNativeRefreshCLIStrategy::new(f.cfg.clone())
                        .with_binary_resolver(|| true)
                        .is_available_sync(mode),
                    mode == SourceMode::Oauth && !managed
                );
                let outcome = pipeline(f.cfg.clone(), &context(mode)).await;
                let error = error_message(outcome.result.as_ref().unwrap_err());
                if managed {
                    assert_eq!(
                        error,
                        CodexOAuthCredentialsError::NativeRefreshRequired.to_string()
                    );
                    assert!(!error.contains("will retry"));
                    assert!(error.contains("Codex home"));
                    assert!(
                        outcome
                            .attempts
                            .iter()
                            .all(|a| a.kind != SourceKind::Cli || !a.was_available)
                    );
                } else {
                    assert_eq!(error, "CLISelected");
                    assert_eq!(
                        outcome.attempts.last().unwrap().strategy_id,
                        if mode == SourceMode::Auto {
                            "codex.cli"
                        } else {
                            "codex.oauth-native-refresh-cli"
                        }
                    );
                }
                unexpected.assert_calls_async(0).await;
                f.unchanged();
            }
        }
    }
}

// upstream: CodexOAuthExpiryPipelineTests.swift:287 unverified future expiry retains unauthorized and transient failure contracts
#[tokio::test]
async fn unverified_future_expiry_retains_unauthorized_and_transient_failure_contracts() {
    use tokio::io::AsyncReadExt;
    for failure in ["401", "403", "500", "decode", "network"] {
        for scenario in ["auto", "oauth", "managed-auto", "managed-oauth"] {
            let mode = if scenario.ends_with("auto") {
                SourceMode::Auto
            } else {
                SourceMode::Oauth
            };
            let managed = scenario.starts_with("managed");
            let server = https::start_server();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = if failure == "network" {
                format!("https://{}", listener.local_addr().unwrap())
            } else {
                server.base_url()
            };
            let f = Fixture::new(4_102_444_800, managed, &base);
            let mock = server
                .mock_async(|when, then| {
                    when.method(httpmock::Method::GET)
                        .header("authorization", format!("Bearer {}", f.token));
                    then.status(failure.parse::<u16>().unwrap_or(200))
                        .body("not-json");
                })
                .await;
            let closed = if failure == "network" {
                Some(tokio::spawn(async move {
                    let (mut stream, _) =
                        tokio::time::timeout(Duration::from_secs(3), listener.accept())
                            .await
                            .unwrap()
                            .unwrap();
                    let mut byte = [0];
                    assert!(
                        tokio::time::timeout(Duration::from_secs(3), stream.read(&mut byte))
                            .await
                            .unwrap()
                            .unwrap()
                            > 0
                    );
                }))
            } else {
                None
            };
            let outcome = pipeline(f.cfg.clone(), &context(mode)).await;
            let error = outcome.result.as_ref().unwrap_err();
            let expected_cli = failure == "401" && mode == SourceMode::Auto && !managed;
            if expected_cli {
                assert_eq!(error_message(error), "CLISelected");
                assert_eq!(outcome.attempts.last().unwrap().strategy_id, "codex.cli");
            } else {
                let FetchError::Classified(c) = error else {
                    panic!("expected classified OAuth error")
                };
                match failure {
                    "401" => assert_eq!(
                        c.kind,
                        ariadusage_protocol::usage::ProviderErrorKind::AuthenticationExpired
                    ),
                    "403" | "500" => assert!(
                        c.message.contains(&format!("Codex API error {failure}")),
                        "scenario={scenario}, failure={failure}, kind={:?}, message={}",
                        c.kind,
                        c.message
                    ),
                    "decode" => {
                        assert_eq!(c.message, CodexOAuthFetchError::InvalidResponse.to_string())
                    }
                    _ => assert!(c.message.contains("Network error")),
                }
                assert!(
                    outcome
                        .attempts
                        .iter()
                        .all(|a| a.kind != SourceKind::Cli || !a.was_available)
                );
            }
            if let Some(task) = closed {
                task.await.unwrap();
            } else {
                mock.assert_calls_async(1).await;
            }
            f.unchanged();
        }
    }
}

struct ExternalSnapshot {
    credentials: CodexOAuthCredentials,
    cfg: CodexStrategyConfig,
}
impl FetchStrategy for ExternalSnapshot {
    fn id(&self) -> &str {
        "codex.oauth"
    }
    fn kind(&self) -> SourceKind {
        SourceKind::Oauth
    }
    fn is_available<'a>(&'a self, _: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async { true })
    }
    fn fetch<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        Box::pin(async move {
            CodexOAuthFetchStrategy::new(self.cfg.clone())
                .fetch_with_credentials(cx, self.credentials.clone())
                .await
                .map(CodexFetchOutcome::into_fetch_result)
                .map_err(CodexOAuthError::into_fetch_error)
        })
    }
    fn should_fallback(&self, err: &FetchError, cx: &FetchContext) -> bool {
        CodexOAuthFetchStrategy::new(self.cfg.clone()).should_fallback(err, cx)
    }
}
struct ExternalPlan(CodexOAuthCredentials, CodexStrategyConfig);
impl ProviderPlan for ExternalPlan {
    fn resolve_strategies<'a>(
        &'a self,
        _: &'a FetchContext,
    ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
        Box::pin(async move {
            vec![
                Box::new(ExternalSnapshot {
                    credentials: self.0.clone(),
                    cfg: self.1.clone(),
                }) as Box<dyn FetchStrategy>,
                Box::new(Probe {
                    id: "codex.cli".into(),
                    cfg: self.1.clone(),
                }) as Box<dyn FetchStrategy>,
            ]
        })
    }
}
// upstream: CodexOAuthExpiryPipelineTests.swift:330 stale external credentials never reach transport or CLI recovery
#[tokio::test]
async fn stale_external_credentials_never_reach_transport_or_cli_recovery() {
    for mode in [SourceMode::Auto, SourceMode::Oauth] {
        for source in [
            CodexOAuthCredentialSource::LegacyCodexHome,
            CodexOAuthCredentialSource::OpenCode,
        ] {
            let home = TempDir::new().unwrap();
            let server = https::start_server();
            let unexpected = server
                .mock_async(|w, t| {
                    w.any_request();
                    t.status(500);
                })
                .await;
            let directory =
                home.path()
                    .join(if source == CodexOAuthCredentialSource::LegacyCodexHome {
                        ".config/codex"
                    } else {
                        ".local/share/opencode"
                    });
            fs::create_dir_all(&directory).unwrap();
            let token = make_jwt(&json!({"exp":4_102_444_800_i64}));
            let raw = if source == CodexOAuthCredentialSource::LegacyCodexHome {
                json!({"tokens":{"access_token":token,"refresh_token":"fixture-refresh"},"last_refresh":"2000-01-01T00:00:00Z"})
            } else {
                json!({"openai":{"type":"oauth","access":token,"refresh":"fixture-refresh","expires":(Timestamp::now().as_second()+30)*1000}})
            };
            let bytes = serde_json::to_vec(&raw).unwrap();
            let auth = directory.join("auth.json");
            fs::write(&auth, &bytes).unwrap();
            let credentials = CodexOAuthCredentialsStore::load_for_usage(
                &BTreeMap::new(),
                true,
                Some(home.path()),
            )
            .unwrap();
            assert_eq!(credentials.source, source);
            let cfg = CodexStrategyConfig::new(
                Arc::new(https::broker(NetConfig::default())),
                BTreeMap::new(),
                home.path(),
                home.path(),
            );
            let cx = context(mode);
            let outcome =
                fetch_outcome(&cx.provider, &ExternalPlan(credentials, cfg), &cx, &NoRetry).await;
            assert_eq!(
                error_message(outcome.result.as_ref().unwrap_err()),
                CodexOAuthCredentialsError::ReadOnlySource.to_string()
            );
            assert_eq!(
                outcome
                    .attempts
                    .iter()
                    .map(|a| a.strategy_id.as_str())
                    .collect::<Vec<_>>(),
                ["codex.oauth"]
            );
            unexpected.assert_calls_async(0).await;
            assert_eq!(fs::read(auth).unwrap(), bytes);
        }
    }
}
