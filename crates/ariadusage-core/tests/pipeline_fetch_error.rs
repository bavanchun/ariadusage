// Ported from CodexBar Tests/CodexBarTests/ProviderFetchErrorTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ariadusage_core::model::UsageSnapshot;
use ariadusage_core::pipeline::*;
use ariadusage_core::providers::SourceMode;
use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::usage::{ProviderErrorCategory, ProviderErrorKind};
use ariadusage_protocol::{Confidence, ProviderId};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default)]
struct RetryDelayRecorder {
    delays: Mutex<Vec<Duration>>,
}

impl Sleeper for RetryDelayRecorder {
    fn sleep<'a>(&'a self, duration: Duration) -> BoxFuture<'a, ()> {
        self.delays.lock().unwrap().push(duration);
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
        provider: ProviderId::new("neuralwatt").unwrap(),
        runtime: FetchRuntime::Cli,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "req-123".to_string(),
        source_mode: SourceMode::Api,
        env: BTreeMap::new(),
        include_credits: false,
        selected_token_account: None,
        cancel,
    }
}

type StrategyFactory = Box<dyn Fn() -> Vec<Box<dyn FetchStrategy>> + Send + Sync>;
type FallbackErrorResolverFn =
    Box<dyn Fn(Option<FetchError>, &FetchError) -> FetchError + Send + Sync>;

struct SimplePlan {
    factory: StrategyFactory,
    custom_resolver: Option<FallbackErrorResolverFn>,
}

impl SimplePlan {
    fn new<F>(factory: F) -> Self
    where
        F: Fn() -> Vec<Box<dyn FetchStrategy>> + Send + Sync + 'static,
    {
        Self {
            factory: Box::new(factory),
            custom_resolver: None,
        }
    }

    fn with_resolver<F>(mut self, resolver: F) -> Self
    where
        F: Fn(Option<FetchError>, &FetchError) -> FetchError + Send + Sync + 'static,
    {
        self.custom_resolver = Some(Box::new(resolver));
        self
    }
}

impl ProviderPlan for SimplePlan {
    fn resolve_strategies<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
        Box::pin(std::future::ready((self.factory)()))
    }

    fn resolve_fallback_error(&self, prev: Option<FetchError>, cur: &FetchError) -> FetchError {
        if let Some(ref resolver) = self.custom_resolver {
            resolver(prev, cur)
        } else {
            cur.clone_fallback()
        }
    }
}

// ----------------------------------------------------------------------------
// Test 1: pipeline honors one exact classified retry delay
// ----------------------------------------------------------------------------

#[derive(Clone)]
struct DelayedRetryStrategy {
    fetch_count: Arc<AtomicUsize>,
}

impl FetchStrategy for DelayedRetryStrategy {
    fn id(&self) -> &str {
        "delayed-retry-test"
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
        let count = self.fetch_count.fetch_add(1, Ordering::SeqCst);
        if count == 0 {
            let err = ClassifiedError::new(ProviderErrorKind::RateLimited, "retry fixture")
                .with_retry_after_secs(3.0);
            Box::pin(std::future::ready(Err(FetchError::Classified(err))))
        } else {
            let res = FetchResult::new(empty_usage_snapshot(), "test", self.id(), self.kind());
            Box::pin(std::future::ready(Ok(res)))
        }
    }

    fn should_fallback(&self, _err: &FetchError, _cx: &FetchContext) -> bool {
        false
    }
}

#[tokio::test]
async fn test_pipeline_honors_one_exact_classified_retry_delay() {
    let fetch_count = Arc::new(AtomicUsize::new(0));
    let fetch_count_clone = fetch_count.clone();
    let delays = Arc::new(RetryDelayRecorder::default());
    let plan = SimplePlan::new(move || {
        vec![Box::new(DelayedRetryStrategy {
            fetch_count: fetch_count_clone.clone(),
        })]
    });
    let cx = test_context(CancellationToken::new());

    let provider = ProviderId::new("neuralwatt").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &*delays).await;

    assert!(outcome.result.is_ok());
    assert_eq!(fetch_count.load(Ordering::SeqCst), 2);
    assert_eq!(*delays.delays.lock().unwrap(), vec![Duration::from_secs(3)]);
    assert_eq!(outcome.attempts.len(), 1);
    assert!(outcome.attempts[0].failure.is_none());
}

// ----------------------------------------------------------------------------
// Test 2 & 3: terminal errors and resolvers
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalFixtureKind {
    First,
    Terminal,
    Resolved,
}

impl TerminalFixtureKind {
    fn to_error(self) -> ClassifiedError {
        match self {
            Self::First => ClassifiedError::new(
                ProviderErrorKind::RateLimited,
                "HTTP first failure: sensitive-payload",
            ),
            Self::Terminal => ClassifiedError::new(
                ProviderErrorKind::ApiFailure,
                "HTTP terminal failure: sensitive-payload",
            ),
            Self::Resolved => ClassifiedError::new(
                ProviderErrorKind::ProviderUnavailable,
                "HTTP resolved failure: sensitive-payload",
            ),
        }
    }
}

#[derive(Clone)]
struct TerminalFixtureStrategy {
    id: &'static str,
    failure: Option<TerminalFixtureKind>,
    allows_fallback: bool,
    cancel_token: bool,
    throw_cancellation: bool,
    routed: Option<Arc<Mutex<Vec<TerminalFixtureKind>>>>,
}

impl FetchStrategy for TerminalFixtureStrategy {
    fn id(&self) -> &str {
        self.id
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Api
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(std::future::ready(true))
    }

    fn fetch<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        if self.cancel_token {
            cx.cancel.cancel();
        }
        if self.throw_cancellation {
            return Box::pin(std::future::ready(Err(FetchError::Cancelled)));
        }
        if let Some(kind) = self.failure {
            return Box::pin(std::future::ready(Err(FetchError::Classified(
                kind.to_error(),
            ))));
        }
        let res = FetchResult::new(empty_usage_snapshot(), self.id, self.id, self.kind());
        Box::pin(std::future::ready(Ok(res)))
    }

    fn should_fallback(&self, err: &FetchError, _cx: &FetchContext) -> bool {
        if let (Some(routed), FetchError::Classified(c)) = (&self.routed, err) {
            let kind = match c.kind {
                ProviderErrorKind::RateLimited => Some(TerminalFixtureKind::First),
                ProviderErrorKind::ApiFailure => Some(TerminalFixtureKind::Terminal),
                ProviderErrorKind::ProviderUnavailable => Some(TerminalFixtureKind::Resolved),
                _ => None,
            };
            if let Some(k) = kind {
                routed.lock().unwrap().push(k);
            }
        }
        self.allows_fallback
    }
}

#[tokio::test]
async fn test_terminal_errors_use_the_resolver_while_routing_retains_each_original_error() {
    let routed = Arc::new(Mutex::new(Vec::new()));
    let resolutions = Arc::new(AtomicUsize::new(0));

    let routed_clone1 = routed.clone();
    let routed_clone2 = routed.clone();
    let resolutions_clone = resolutions.clone();

    let plan = SimplePlan::new(move || {
        vec![
            Box::new(TerminalFixtureStrategy {
                id: "first",
                failure: Some(TerminalFixtureKind::First),
                allows_fallback: true,
                cancel_token: false,
                throw_cancellation: false,
                routed: Some(routed_clone1.clone()),
            }),
            Box::new(TerminalFixtureStrategy {
                id: "terminal",
                failure: Some(TerminalFixtureKind::Terminal),
                allows_fallback: false,
                cancel_token: false,
                throw_cancellation: false,
                routed: Some(routed_clone2.clone()),
            }),
        ]
    })
    .with_resolver(move |prev, _cur| {
        resolutions_clone.fetch_add(1, Ordering::SeqCst);
        prev.unwrap_or_else(|| FetchError::Classified(TerminalFixtureKind::Resolved.to_error()))
    });

    let sleeper = RetryDelayRecorder::default();
    let cx = test_context(CancellationToken::new());

    let provider = ProviderId::new("neuralwatt").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &sleeper).await;

    match outcome.result {
        Err(FetchError::Classified(c)) => {
            assert_eq!(c.kind, ProviderErrorKind::ProviderUnavailable);
            assert_eq!(c.category(), ProviderErrorCategory::Api);
        }
        other => panic!("Expected resolved terminal failure, got {other:?}"),
    }

    assert_eq!(
        *routed.lock().unwrap(),
        vec![TerminalFixtureKind::First, TerminalFixtureKind::Terminal]
    );
    assert_eq!(resolutions.load(Ordering::SeqCst), 2);

    let last_attempt = outcome.attempts.last().unwrap();
    let failure = last_attempt.failure.as_ref().unwrap();
    assert_eq!(failure.kind, ProviderErrorKind::ApiFailure);
    assert_eq!(failure.category, ProviderErrorCategory::Api);
}

#[tokio::test]
async fn test_default_resolver_keeps_the_terminal_source_error() {
    let plan = SimplePlan::new(|| {
        vec![
            Box::new(TerminalFixtureStrategy {
                id: "first",
                failure: Some(TerminalFixtureKind::First),
                allows_fallback: true,
                cancel_token: false,
                throw_cancellation: false,
                routed: None,
            }),
            Box::new(TerminalFixtureStrategy {
                id: "terminal",
                failure: Some(TerminalFixtureKind::Terminal),
                allows_fallback: false,
                cancel_token: false,
                throw_cancellation: false,
                routed: None,
            }),
        ]
    });

    let sleeper = RetryDelayRecorder::default();
    let cx = test_context(CancellationToken::new());

    let provider = ProviderId::new("neuralwatt").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &sleeper).await;

    match outcome.result {
        Err(FetchError::Classified(c)) => {
            assert_eq!(c.kind, ProviderErrorKind::ApiFailure);
            assert_eq!(c.category(), ProviderErrorCategory::Api);
        }
        other => panic!("Expected terminal failure, got {other:?}"),
    }
}

// ----------------------------------------------------------------------------
// Test 4: cancellation wins over a resolved earlier error
// ----------------------------------------------------------------------------

async fn check_cancellation_wins_over_a_resolved_earlier_error(cancel_task: bool) {
    let plan = SimplePlan::new(move || {
        vec![
            Box::new(TerminalFixtureStrategy {
                id: "first",
                failure: Some(TerminalFixtureKind::First),
                allows_fallback: true,
                cancel_token: false,
                throw_cancellation: false,
                routed: None,
            }),
            Box::new(TerminalFixtureStrategy {
                id: "cancel",
                failure: Some(TerminalFixtureKind::Terminal),
                allows_fallback: true,
                cancel_token: cancel_task,
                throw_cancellation: !cancel_task,
                routed: None,
            }),
            Box::new(TerminalFixtureStrategy {
                id: "unused",
                failure: None,
                allows_fallback: false,
                cancel_token: false,
                throw_cancellation: false,
                routed: None,
            }),
        ]
    })
    .with_resolver(|prev, cur| prev.unwrap_or_else(|| cur.clone_fallback()));

    let sleeper = RetryDelayRecorder::default();
    let cx = test_context(CancellationToken::new());

    let provider = ProviderId::new("neuralwatt").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &sleeper).await;

    match outcome.result {
        Err(FetchError::Cancelled) => {}
        other => panic!("Expected cancellation error, got {other:?}"),
    }

    let ids: Vec<&str> = outcome
        .attempts
        .iter()
        .map(|a| a.strategy_id.as_str())
        .collect();
    assert_eq!(ids, vec!["first", "cancel"]);
}

#[tokio::test]
async fn test_cancellation_wins_with_cancel_token() {
    check_cancellation_wins_over_a_resolved_earlier_error(true).await;
}

#[tokio::test]
async fn test_cancellation_wins_with_thrown_cancellation() {
    check_cancellation_wins_over_a_resolved_earlier_error(false).await;
}

// ----------------------------------------------------------------------------
// Test 5: terminal and exhausted failures emit one safe diagnostic
// ----------------------------------------------------------------------------

async fn check_terminal_and_exhausted_failures_emit_one_safe_diagnostic(allows_fallback: bool) {
    let plan = SimplePlan::new(move || {
        vec![Box::new(TerminalFixtureStrategy {
            id: "fixture.api",
            failure: Some(TerminalFixtureKind::Terminal),
            allows_fallback,
            cancel_token: false,
            throw_cancellation: false,
            routed: None,
        })]
    });

    let sleeper = RetryDelayRecorder::default();
    let cx = test_context(CancellationToken::new());

    let provider = ProviderId::new("neuralwatt").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &sleeper).await;

    assert_eq!(outcome.attempts.len(), 1);
    let summary = outcome.failure_summary.expect("Expected failure summary");
    assert_eq!(summary, "fixture.api (api): failed: api");
    assert!(!summary.contains("sensitive-payload"));
}

#[tokio::test]
async fn test_terminal_failure_emits_one_safe_diagnostic() {
    check_terminal_and_exhausted_failures_emit_one_safe_diagnostic(false).await;
}

#[tokio::test]
async fn test_exhausted_failure_emits_one_safe_diagnostic() {
    check_terminal_and_exhausted_failures_emit_one_safe_diagnostic(true).await;
}

// ----------------------------------------------------------------------------
// Test 6: success and cancellation do not emit failure diagnostics
// ----------------------------------------------------------------------------

async fn check_success_and_cancellation_do_not_emit_failure_diagnostics(cancel: bool) {
    let plan = SimplePlan::new(move || {
        vec![Box::new(TerminalFixtureStrategy {
            id: "fixture.api",
            failure: None,
            allows_fallback: false,
            cancel_token: false,
            throw_cancellation: cancel,
            routed: None,
        })]
    });

    let sleeper = RetryDelayRecorder::default();
    let cx = test_context(CancellationToken::new());

    let provider = ProviderId::new("neuralwatt").unwrap();
    let outcome = fetch_outcome(&provider, &plan, &cx, &sleeper).await;

    if cancel {
        assert!(matches!(outcome.result, Err(FetchError::Cancelled)));
    } else {
        assert!(outcome.result.is_ok());
    }

    assert!(outcome.failure_summary.is_none());
}

#[tokio::test]
async fn test_success_does_not_emit_failure_diagnostic() {
    check_success_and_cancellation_do_not_emit_failure_diagnostics(false).await;
}

#[tokio::test]
async fn test_cancellation_does_not_emit_failure_diagnostic() {
    check_success_and_cancellation_do_not_emit_failure_diagnostics(true).await;
}
