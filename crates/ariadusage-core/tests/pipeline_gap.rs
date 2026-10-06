use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ariadusage_core::model::UsageSnapshot;
use ariadusage_core::pipeline::*;
use ariadusage_core::providers::SourceMode;
use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::secret::SecretString;
use ariadusage_protocol::usage::{ProviderErrorCategory, ProviderErrorKind};
use ariadusage_protocol::{Confidence, ProviderId};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default)]
struct RecordingSleeper {
    delays: Mutex<Vec<Duration>>,
    cancel_on_sleep: Option<CancellationToken>,
}

impl Sleeper for RecordingSleeper {
    fn sleep<'a>(&'a self, duration: Duration) -> BoxFuture<'a, ()> {
        self.delays.lock().unwrap().push(duration);
        if let Some(ref c) = self.cancel_on_sleep {
            c.cancel();
        }
        Box::pin(std::future::ready(()))
    }
}

fn empty_usage_snapshot() -> UsageSnapshot {
    UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        jiff::Timestamp::from_second(1_700_000_000).unwrap(),
        None,
        Confidence::Unknown,
    )
    .unwrap()
}

fn test_context(cancel: CancellationToken) -> FetchContext {
    FetchContext {
        provider: ProviderId::new("codex").unwrap(),
        runtime: FetchRuntime::Cli,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "gap-req-1".to_string(),
        source_mode: SourceMode::Api,
        env: BTreeMap::new(),
        include_credits: false,
        selected_token_account: None,
        cancel,
    }
}

struct ClosurePlan {
    factory: Box<dyn Fn() -> Vec<Box<dyn FetchStrategy>> + Send + Sync>,
}

impl ClosurePlan {
    fn new<F>(f: F) -> Self
    where
        F: Fn() -> Vec<Box<dyn FetchStrategy>> + Send + Sync + 'static,
    {
        Self {
            factory: Box::new(f),
        }
    }
}

impl ProviderPlan for ClosurePlan {
    fn resolve_strategies<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
        Box::pin(std::future::ready((self.factory)()))
    }
}

// ----------------------------------------------------------------------------
// Gap 1: All strategies skipped yields NoAvailableStrategy with N skipped attempts
// ----------------------------------------------------------------------------

#[derive(Clone)]
struct UnavailableStrategy {
    id: &'static str,
}

impl FetchStrategy for UnavailableStrategy {
    fn id(&self) -> &str {
        self.id
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Api
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(std::future::ready(false))
    }

    fn fetch<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        unreachable!()
    }

    fn should_fallback(&self, _err: &FetchError, _cx: &FetchContext) -> bool {
        false
    }
}

#[tokio::test]
async fn test_all_strategies_skipped_returns_no_available_strategy_with_n_attempts() {
    let plan = ClosurePlan::new(|| {
        vec![
            Box::new(UnavailableStrategy { id: "strategy.one" }),
            Box::new(UnavailableStrategy { id: "strategy.two" }),
        ]
    });
    let sleeper = RecordingSleeper::default();
    let cx = test_context(CancellationToken::new());

    let provider = ProviderId::new("codex").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &sleeper).await;

    match outcome.result {
        Err(FetchError::NoAvailableStrategy(p)) => assert_eq!(p.as_str(), "codex"),
        other => panic!("Expected NoAvailableStrategy, got {other:?}"),
    }

    assert_eq!(outcome.attempts.len(), 2);
    assert_eq!(outcome.attempts[0].strategy_id, "strategy.one");
    assert!(!outcome.attempts[0].was_available);
    assert_eq!(outcome.attempts[1].strategy_id, "strategy.two");
    assert!(!outcome.attempts[1].was_available);

    let summary = outcome.failure_summary.expect("Expected failure summary");
    assert_eq!(
        summary,
        "strategy.one (api): skipped: unavailable -> strategy.two (api): skipped: unavailable"
    );
}

// ----------------------------------------------------------------------------
// Gap 2: Empty list yields NoAvailableStrategy with 0 attempts
// ----------------------------------------------------------------------------

#[tokio::test]
async fn test_empty_list_returns_no_available_strategy_with_zero_attempts() {
    let plan = ClosurePlan::new(Vec::new);
    let sleeper = RecordingSleeper::default();
    let cx = test_context(CancellationToken::new());

    let provider = ProviderId::new("codex").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &sleeper).await;

    match outcome.result {
        Err(FetchError::NoAvailableStrategy(p)) => assert_eq!(p.as_str(), "codex"),
        other => panic!("Expected NoAvailableStrategy, got {other:?}"),
    }

    assert_eq!(outcome.attempts.len(), 0);
    assert!(outcome.failure_summary.is_none());
}

// ----------------------------------------------------------------------------
// Gap 3: Diagnostic attached only on fallback success and never overwritten
// ----------------------------------------------------------------------------

#[derive(Clone)]
struct FailingStrategy;

impl FetchStrategy for FailingStrategy {
    fn id(&self) -> &str {
        "strategy.fail"
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Api
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(std::future::ready(true))
    }

    fn fetch<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        let err = ClassifiedError::new(ProviderErrorKind::NetworkFailure, "offline");
        Box::pin(std::future::ready(Err(FetchError::Classified(err))))
    }

    fn should_fallback(&self, _err: &FetchError, _cx: &FetchContext) -> bool {
        true
    }
}

#[derive(Clone)]
struct DiagnosticFallbackStrategy {
    preexisting_diagnostic: Option<String>,
}

impl FetchStrategy for DiagnosticFallbackStrategy {
    fn id(&self) -> &str {
        "strategy.fallback"
    }

    fn kind(&self) -> SourceKind {
        SourceKind::LocalProbe
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(std::future::ready(true))
    }

    fn fetch<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        let mut res = FetchResult::new(empty_usage_snapshot(), "local", self.id(), self.kind());
        if let Some(ref d) = self.preexisting_diagnostic {
            res = res.with_diagnostic(d.clone());
        }
        Box::pin(std::future::ready(Ok(res)))
    }

    fn should_fallback(&self, _err: &FetchError, _cx: &FetchContext) -> bool {
        false
    }

    fn diagnostic_for_prior_failure(&self, err: &FetchError) -> Option<String> {
        match err {
            FetchError::Classified(c) => Some(format!("recovered from kind {:?}", c.kind)),
            _ => None,
        }
    }
}

#[tokio::test]
async fn test_diagnostic_attached_on_fallback_success() {
    let plan = ClosurePlan::new(|| {
        vec![
            Box::new(FailingStrategy),
            Box::new(DiagnosticFallbackStrategy {
                preexisting_diagnostic: None,
            }),
        ]
    });
    let sleeper = RecordingSleeper::default();
    let cx = test_context(CancellationToken::new());

    let provider = ProviderId::new("antigravity").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &sleeper).await;

    let res = outcome.result.expect("Expected fallback success");
    assert_eq!(
        res.diagnostic.as_deref(),
        Some("recovered from kind NetworkFailure")
    );
}

#[tokio::test]
async fn test_diagnostic_never_overwrites_existing_diagnostic() {
    let plan = ClosurePlan::new(|| {
        vec![
            Box::new(FailingStrategy),
            Box::new(DiagnosticFallbackStrategy {
                preexisting_diagnostic: Some("preexisting-diagnostic".to_string()),
            }),
        ]
    });
    let sleeper = RecordingSleeper::default();
    let cx = test_context(CancellationToken::new());

    let provider = ProviderId::new("antigravity").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &sleeper).await;

    let res = outcome.result.expect("Expected fallback success");
    assert_eq!(res.diagnostic.as_deref(), Some("preexisting-diagnostic"));
}

// ----------------------------------------------------------------------------
// Gap 4: A non-classified error is not retried
// ----------------------------------------------------------------------------

#[derive(Clone)]
struct NonClassifiedErrorStrategy {
    fetches: Arc<AtomicUsize>,
}

impl FetchStrategy for NonClassifiedErrorStrategy {
    fn id(&self) -> &str {
        "strategy.non-classified"
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Api
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(std::future::ready(true))
    }

    fn fetch<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Err(FetchError::Cancelled)))
    }

    fn should_fallback(&self, _err: &FetchError, _cx: &FetchContext) -> bool {
        false
    }
}

#[tokio::test]
async fn test_non_classified_error_is_not_retried() {
    let fetches = Arc::new(AtomicUsize::new(0));
    let fetches_clone = fetches.clone();
    let plan = ClosurePlan::new(move || {
        vec![Box::new(NonClassifiedErrorStrategy {
            fetches: fetches_clone.clone(),
        })]
    });
    let sleeper = RecordingSleeper::default();
    let cx = test_context(CancellationToken::new());

    let provider = ProviderId::new("codex").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &sleeper).await;

    assert!(matches!(outcome.result, Err(FetchError::Cancelled)));
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
    assert!(sleeper.delays.lock().unwrap().is_empty());
}

// ----------------------------------------------------------------------------
// Gap 5: A second classified error is not retried
// ----------------------------------------------------------------------------

#[derive(Clone)]
struct DoubleRetryFailureStrategy {
    fetches: Arc<AtomicUsize>,
}

impl FetchStrategy for DoubleRetryFailureStrategy {
    fn id(&self) -> &str {
        "strategy.double-failure"
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Api
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(std::future::ready(true))
    }

    fn fetch<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        let count = self.fetches.fetch_add(1, Ordering::SeqCst);
        let err = ClassifiedError::new(
            ProviderErrorKind::RateLimited,
            format!("fail count {count}"),
        )
        .with_retry_after_secs(1.0);
        Box::pin(std::future::ready(Err(FetchError::Classified(err))))
    }

    fn should_fallback(&self, _err: &FetchError, _cx: &FetchContext) -> bool {
        false
    }
}

#[tokio::test]
async fn test_second_classified_error_is_not_retried() {
    let fetches = Arc::new(AtomicUsize::new(0));
    let fetches_clone = fetches.clone();
    let plan = ClosurePlan::new(move || {
        vec![Box::new(DoubleRetryFailureStrategy {
            fetches: fetches_clone.clone(),
        })]
    });
    let sleeper = RecordingSleeper::default();
    let cx = test_context(CancellationToken::new());

    let provider = ProviderId::new("codex").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &sleeper).await;

    assert!(matches!(outcome.result, Err(FetchError::Classified(_))));
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
    assert_eq!(sleeper.delays.lock().unwrap().len(), 1);
}

// ----------------------------------------------------------------------------
// Gap 6: Cancel during retry sleep returns Cancelled with attempt recorded
// ----------------------------------------------------------------------------

#[tokio::test]
async fn test_cancel_during_retry_sleep_returns_cancelled_with_attempt_recorded() {
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    let plan = ClosurePlan::new(|| {
        vec![Box::new(DoubleRetryFailureStrategy {
            fetches: Arc::new(AtomicUsize::new(0)),
        })]
    });

    let sleeper = RecordingSleeper {
        cancel_on_sleep: Some(cancel_clone),
        ..Default::default()
    };

    let cx = test_context(cancel);
    let provider = ProviderId::new("codex").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &sleeper).await;

    assert!(matches!(outcome.result, Err(FetchError::Cancelled)));
    assert_eq!(outcome.attempts.len(), 1);
    assert!(outcome.attempts[0].was_available);
    assert!(outcome.attempts[0].failure.is_some());
}

// ----------------------------------------------------------------------------
// Gap 7: Success returned after cancellation returns Cancelled with failed attempt
// ----------------------------------------------------------------------------

#[derive(Clone)]
struct CancelAndSucceedStrategy;

impl FetchStrategy for CancelAndSucceedStrategy {
    fn id(&self) -> &str {
        "strategy.cancel-succeed"
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Api
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(std::future::ready(true))
    }

    fn fetch<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        cx.cancel.cancel();
        let res = FetchResult::new(empty_usage_snapshot(), "test", self.id(), self.kind());
        Box::pin(std::future::ready(Ok(res)))
    }

    fn should_fallback(&self, _err: &FetchError, _cx: &FetchContext) -> bool {
        false
    }
}

#[tokio::test]
async fn test_success_returned_after_cancellation_returns_cancelled_with_failed_attempt() {
    let plan = ClosurePlan::new(|| vec![Box::new(CancelAndSucceedStrategy)]);
    let sleeper = RecordingSleeper::default();
    let cx = test_context(CancellationToken::new());

    let provider = ProviderId::new("codex").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &sleeper).await;

    assert!(matches!(outcome.result, Err(FetchError::Cancelled)));
    assert_eq!(outcome.attempts.len(), 1);
    assert!(outcome.attempts[0].was_available);
    assert_eq!(
        outcome.attempts[0].failure,
        Some(AttemptFailure {
            kind: ProviderErrorKind::Unknown,
            category: ProviderErrorCategory::Unknown,
        })
    );
}

// ----------------------------------------------------------------------------
// Gap 8: Error returned after cancellation returns Cancelled and should_fallback not called
// ----------------------------------------------------------------------------

#[derive(Clone)]
struct CancelAndFailStrategy {
    fallback_called: Arc<AtomicBool>,
}

impl FetchStrategy for CancelAndFailStrategy {
    fn id(&self) -> &str {
        "strategy.cancel-fail"
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Api
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(std::future::ready(true))
    }

    fn fetch<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        cx.cancel.cancel();
        let err = ClassifiedError::new(ProviderErrorKind::RateLimited, "error detail");
        Box::pin(std::future::ready(Err(FetchError::Classified(err))))
    }

    fn should_fallback(&self, _err: &FetchError, _cx: &FetchContext) -> bool {
        self.fallback_called.store(true, Ordering::SeqCst);
        true
    }
}

#[tokio::test]
async fn test_error_returned_after_cancellation_does_not_call_should_fallback() {
    let fallback_called = Arc::new(AtomicBool::new(false));
    let fallback_clone = fallback_called.clone();
    let plan = ClosurePlan::new(move || {
        vec![Box::new(CancelAndFailStrategy {
            fallback_called: fallback_clone.clone(),
        })]
    });
    let sleeper = RecordingSleeper::default();
    let cx = test_context(CancellationToken::new());

    let provider = ProviderId::new("codex").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &sleeper).await;

    assert!(matches!(outcome.result, Err(FetchError::Cancelled)));
    assert!(!fallback_called.load(Ordering::SeqCst));
    assert_eq!(outcome.attempts.len(), 1);
    assert!(outcome.attempts[0].was_available);
}

// ----------------------------------------------------------------------------
// Gap 9: retry_after NaN, -1, 0, 25 normalization
// ----------------------------------------------------------------------------

#[test]
fn test_retry_after_normalization() {
    assert_eq!(normalize_retry_after(f64::NAN), None);
    assert_eq!(normalize_retry_after(-1.0), None);
    assert_eq!(normalize_retry_after(0.0), Some(Duration::ZERO));
    assert_eq!(normalize_retry_after(25.0), Some(Duration::from_secs(10)));
}

// ----------------------------------------------------------------------------
// Gap 10: Debug and Display never print secrets
// ----------------------------------------------------------------------------

#[derive(Debug)]
struct CustomSourceError(&'static str);

impl std::fmt::Display for CustomSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CustomSourceError {}

#[test]
fn test_debug_and_display_never_print_secrets() {
    let secret_token = "SUPER_SECRET_TOKEN_XYZ_12345";
    let secret_message = "SECRET_INTERNAL_PAYLOAD_ABCDE";
    let secret_source = "SECRET_SOURCE_ERROR_99999";

    let mut env = BTreeMap::new();
    env.insert("API_KEY".to_string(), SecretString::new(secret_token));

    let cx = FetchContext {
        provider: ProviderId::new("codex").unwrap(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::Background,
        phase: FetchPhase::Startup,
        request_id: "req-sec-1".to_string(),
        source_mode: SourceMode::Api,
        env,
        include_credits: true,
        selected_token_account: Some("acct-1".to_string()),
        cancel: CancellationToken::new(),
    };

    let cx_debug = format!("{cx:?}");
    assert!(
        !cx_debug.contains(secret_token),
        "Secret token leaked in FetchContext Debug: {cx_debug}"
    );
    assert!(
        cx_debug.contains("[redacted]"),
        "FetchContext Debug should show [redacted]"
    );

    let classified = ClassifiedError::new(ProviderErrorKind::RateLimited, secret_message)
        .with_source(Box::new(CustomSourceError(secret_source)));

    let classified_display = format!("{classified}");
    assert!(
        !classified_display.contains(secret_message),
        "Message leaked in ClassifiedError Display: {classified_display}"
    );
    assert!(
        !classified_display.contains(secret_source),
        "Source leaked in ClassifiedError Display: {classified_display}"
    );

    let classified_debug = format!("{classified:?}");
    assert!(
        !classified_debug.contains(secret_message),
        "Message leaked in ClassifiedError Debug: {classified_debug}"
    );
    assert!(
        !classified_debug.contains(secret_source),
        "Source leaked in ClassifiedError Debug: {classified_debug}"
    );

    let fetch_error = FetchError::Classified(classified);
    let fetch_err_display = format!("{fetch_error}");
    assert!(
        !fetch_err_display.contains(secret_message),
        "Message leaked in FetchError Display: {fetch_err_display}"
    );
    assert!(
        !fetch_err_display.contains(secret_source),
        "Source leaked in FetchError Display: {fetch_err_display}"
    );

    let fetch_err_debug = format!("{fetch_error:?}");
    assert!(
        !fetch_err_debug.contains(secret_message),
        "Message leaked in FetchError Debug: {fetch_err_debug}"
    );
    assert!(
        !fetch_err_debug.contains(secret_source),
        "Source leaked in FetchError Debug: {fetch_err_debug}"
    );
}
