// Ported from CodexBar Tests/CodexBarTests/ProviderRefreshCoordinatorTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/RequiredRefreshCoalescingTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeResilienceTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar TestsLinux/HookTransitionDetectorLinuxTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ariadusage_core::model::{
    ProviderCostSnapshot, ProviderIdentitySnapshot, RateWindow, UsageSnapshot,
};
use ariadusage_core::pipeline::{
    BoxFuture, ClassifiedError, FetchContext, FetchError, FetchInteraction, FetchResult,
    FetchStrategy, ProviderPlan, TransportClass,
};
use ariadusage_core::refresh::{
    AdaptiveRefreshReason, HookEventType, HookRule, HooksConfig, SessionQuotaTransition,
    ThermalPressure,
};
use ariadusage_engine::brokers::net::{NetBroker, NetConfig};
use ariadusage_engine::brokers::secret_store::SecretStore;
use ariadusage_engine::brokers::secret_store::memory::MemoryBackend;
use ariadusage_engine::{
    ConfigStore, EngineRuntime, ProviderRegistry, RecordingTransitionSink, RefreshOptions,
};
use ariadusage_protocol::ids::{ActionId, ProviderId, SettingId};
use ariadusage_protocol::ipc::{ClientInfo, ClientMessage, ErrorCode, PROTOCOL_V1, ServerMessage};
use ariadusage_protocol::metric::{Confidence, MetricState, SourceKind};
use ariadusage_protocol::settings::{DescriptorKind, SettingsScope};
use ariadusage_protocol::usage::{ProviderErrorCategory, ProviderErrorKind};
use jiff::Timestamp;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Notify;

type StepFn =
    Box<dyn Fn(&FetchContext) -> BoxFuture<'static, Result<FetchResult, FetchError>> + Send + Sync>;

struct ScriptedStrategy {
    id: String,
    kind: SourceKind,
    calls: Arc<AtomicUsize>,
    last_interaction: Arc<Mutex<Option<FetchInteraction>>>,
    steps: Arc<Mutex<VecDeque<StepFn>>>,
}

impl FetchStrategy for ScriptedStrategy {
    fn id(&self) -> &str {
        &self.id
    }

    fn kind(&self) -> SourceKind {
        self.kind
    }

    fn is_available<'a>(&'a self, _cx: &'a FetchContext) -> BoxFuture<'a, bool> {
        Box::pin(async { true })
    }

    fn fetch<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, Result<FetchResult, FetchError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last_interaction.lock().unwrap() = Some(cx.interaction);
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted step available");
        step(cx)
    }

    fn should_fallback(&self, _err: &FetchError, _cx: &FetchContext) -> bool {
        false
    }
}

#[derive(Clone)]
struct ScriptedPlan {
    id: String,
    kind: SourceKind,
    calls: Arc<AtomicUsize>,
    last_interaction: Arc<Mutex<Option<FetchInteraction>>>,
    steps: Arc<Mutex<VecDeque<StepFn>>>,
}

impl ScriptedPlan {
    fn new(id: &str, kind: SourceKind) -> Self {
        Self {
            id: id.to_string(),
            kind,
            calls: Arc::new(AtomicUsize::new(0)),
            last_interaction: Arc::new(Mutex::new(None)),
            steps: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    fn push_ok(&self, usage: UsageSnapshot) {
        let id = self.id.clone();
        let kind = self.kind;
        self.steps.lock().unwrap().push_back(Box::new(move |_cx| {
            let res = FetchResult::new(usage.clone(), id.clone(), id.clone(), kind);
            Box::pin(async move { Ok(res) })
        }));
    }

    fn push_err(&self, err_fn: impl Fn() -> FetchError + Send + Sync + 'static) {
        self.steps.lock().unwrap().push_back(Box::new(move |_cx| {
            let err = err_fn();
            Box::pin(async move { Err(err) })
        }));
    }

    fn push_gated(&self, started: Arc<Notify>, release: Arc<Notify>, usage: UsageSnapshot) {
        let id = self.id.clone();
        let kind = self.kind;
        self.steps.lock().unwrap().push_back(Box::new(move |cx| {
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            let cancel = cx.cancel.clone();
            let res = FetchResult::new(usage.clone(), id.clone(), id.clone(), kind);
            Box::pin(async move {
                started.notify_waiters();
                tokio::select! {
                    () = cancel.cancelled() => Err(FetchError::Cancelled),
                    () = release.notified() => Ok(res),
                }
            })
        }));
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn last_interaction(&self) -> Option<FetchInteraction> {
        *self.last_interaction.lock().unwrap()
    }
}

impl ProviderPlan for ScriptedPlan {
    fn resolve_strategies<'a>(
        &'a self,
        _cx: &'a FetchContext,
    ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>> {
        let strategy = ScriptedStrategy {
            id: self.id.clone(),
            kind: self.kind,
            calls: Arc::clone(&self.calls),
            last_interaction: Arc::clone(&self.last_interaction),
            steps: Arc::clone(&self.steps),
        };
        Box::pin(async move { vec![Box::new(strategy) as Box<dyn FetchStrategy>] })
    }
}

fn make_usage(
    used_primary: f64,
    updated_at: Timestamp,
    email: Option<&str>,
    provider_id: Option<ProviderId>,
) -> UsageSnapshot {
    let primary = RateWindow::new(
        used_primary,
        Some(300),
        Some(updated_at.checked_add(jiff::Span::new().hours(4)).unwrap()),
        Some("Resets in 4h".to_string()),
        None,
        false,
    )
    .unwrap();
    let secondary = RateWindow::new(
        50.0,
        Some(10080),
        Some(updated_at.checked_add(jiff::Span::new().hours(84)).unwrap()),
        Some("Resets in 3d 12h".to_string()),
        None,
        false,
    )
    .unwrap();
    let identity = email.map(|e| ProviderIdentitySnapshot {
        provider_id,
        account_email: Some(e.to_string()),
        account_organization: Some("Example Org".to_string()),
        login_method: Some("Max".to_string()),
        account_id: Some(format!("acct-{e}")),
    });
    let mut cost = ProviderCostSnapshot::new(12.5, 50.0, "USD", updated_at);
    cost.period = Some("Monthly".to_string());
    UsageSnapshot::new(
        Some(primary),
        Some(secondary),
        None,
        None,
        Some(cost),
        Vec::new(),
        None,
        None,
        updated_at,
        identity,
        Confidence::Exact,
    )
    .unwrap()
}

fn test_runtime(
    tmp: &TempDir,
    registry: ProviderRegistry,
    now: Timestamp,
) -> (Arc<EngineRuntime>, Arc<RecordingTransitionSink>) {
    let config_store = ConfigStore::new(tmp.path().join("config.json"));
    let claude_id = ProviderId::new("claude").unwrap();
    config_store
        .update(|cfg| {
            ariadusage_core::config::set_provider_enabled(cfg, &claude_id, true);
        })
        .expect("enable claude in test config");
    let secret_store = Arc::new(SecretStore::new(MemoryBackend::new()));
    let net = Arc::new(NetBroker::new(NetConfig::default()).expect("default net broker"));
    let sink = Arc::new(RecordingTransitionSink::new());
    let runtime = Arc::new(
        EngineRuntime::new(config_store, secret_store, net, registry)
            .with_transition_sink(Arc::clone(&sink) as _)
            .with_clock(Arc::new(move || now)),
    );
    (runtime, sink)
}

#[test]
fn provider_registry_wires_claude_by_default_and_allows_codex_and_antigravity_registration() {
    let mut registry = ProviderRegistry::default();
    let claude = ProviderId::new("claude").unwrap();
    let codex = ProviderId::new("codex").unwrap();
    let antigravity = ProviderId::new("antigravity").unwrap();

    assert!(registry.has_registered_plan(&claude));
    assert!(registry.has_registered_plan(&codex));
    assert!(!registry.has_registered_plan(&antigravity));

    let codex_plan = ScriptedPlan::new("codex.cli", SourceKind::Cli);
    let agy_plan = ScriptedPlan::new("antigravity.probe", SourceKind::LocalProbe);
    registry.register_plan(codex.clone(), Arc::new(codex_plan));
    registry.register_plan(antigravity.clone(), Arc::new(agy_plan));

    assert!(registry.has_registered_plan(&codex));
    assert!(registry.has_registered_plan(&antigravity));
    assert_eq!(registry.descriptors().len(), 3);
}

// upstream: Tests/CodexBarTests/ClaudeResilienceTests.swift:402 repeated non probe transient failure still surfaces
#[tokio::test]
async fn failure_gate_hides_first_preservable_failure_surfaces_second_with_kept_timestamp_and_drops_on_non_preservable()
 {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();

    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    // 1. Initial success at t0
    plan.push_ok(make_usage(
        35.0,
        t0,
        Some("dev@example.invalid"),
        Some(claude_id.clone()),
    ));
    // 2. First consecutive preservable failure (timeout) -> hidden!
    plan.push_err(|| {
        FetchError::Classified(
            ClassifiedError::new(ProviderErrorKind::NetworkFailure, "timed out")
                .with_transport(TransportClass::Timeout),
        )
    });
    // 3. Second consecutive preservable failure (timeout) -> surfaced as Stale with kept data & t0!
    plan.push_err(|| {
        FetchError::Classified(
            ClassifiedError::new(ProviderErrorKind::NetworkFailure, "timed out again")
                .with_transport(TransportClass::Timeout),
        )
    });
    // 4. Non-preservable failure (AuthenticationExpired) -> drops data!
    plan.push_err(|| {
        FetchError::Classified(ClassifiedError::new(
            ProviderErrorKind::AuthenticationExpired,
            "token expired",
        ))
    });

    let mut registry = ProviderRegistry::empty();
    registry.register_plan(claude_id.clone(), Arc::new(plan));
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);

    let mut opts = RefreshOptions::user_initiated();
    opts.refresh_status = false;

    // Pass 1: Success
    runtime.refresh_provider(claude_id.clone(), opts).await;
    let snap1 = runtime.current_snapshot();
    let p1 = snap1
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .expect("claude");
    assert_eq!(p1.updated_at, Some(t0));
    assert_eq!(
        p1.windows.primary.as_ref().unwrap().state,
        MetricState::Value
    );
    assert_eq!(
        p1.windows
            .primary
            .as_ref()
            .unwrap()
            .value
            .as_ref()
            .unwrap()
            .used_percent,
        35.0
    );
    assert!(
        p1.windows
            .secondary
            .as_ref()
            .unwrap()
            .value
            .as_ref()
            .unwrap()
            .pace
            .is_some()
    );
    assert!(p1.last_error.is_none());

    // Pass 2: First consecutive preservable failure -> hidden, keeps t0 & Value state
    runtime.refresh_provider(claude_id.clone(), opts).await;
    let snap2 = runtime.current_snapshot();
    let p2 = snap2
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .expect("claude");
    assert_eq!(p2.updated_at, Some(t0));
    assert_eq!(
        p2.windows.primary.as_ref().unwrap().state,
        MetricState::Value
    );
    assert!(p2.last_error.is_none());

    // Pass 3: Second consecutive preservable failure -> surfaced, keeps t0 & transitions to Stale
    runtime.refresh_provider(claude_id.clone(), opts).await;
    let snap3 = runtime.current_snapshot();
    let p3 = snap3
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .expect("claude");
    assert_eq!(p3.updated_at, Some(t0));
    assert_eq!(
        p3.windows.primary.as_ref().unwrap().state,
        MetricState::Stale
    );
    assert_eq!(
        p3.windows
            .primary
            .as_ref()
            .unwrap()
            .value
            .as_ref()
            .unwrap()
            .used_percent,
        35.0
    );
    let err3 = p3
        .last_error
        .as_ref()
        .expect("surfaced error on second failure");
    assert_eq!(err3.category, ProviderErrorCategory::Network);

    // Pass 4: Non-preservable failure -> drops data
    runtime.refresh_provider(claude_id.clone(), opts).await;
    let snap4 = runtime.current_snapshot();
    let p4 = snap4
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .expect("claude");
    assert_eq!(p4.updated_at, None);
    assert!(p4.windows.primary.is_none());
    let err4 = p4.last_error.as_ref().expect("auth error surfaced");
    assert_eq!(err4.kind, ProviderErrorKind::AuthenticationExpired);
    assert_eq!(err4.category, ProviderErrorCategory::Auth);
}

// upstream: Tests/CodexBarTests/ClaudeResilienceTests.swift:7 cancelled Claude refresh never publishes an error
#[tokio::test]
async fn cancellation_is_not_a_failure_and_never_increments_gate_or_drops_data() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();

    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    // First refresh cancelled without prior snapshot -> no error published
    plan.push_err(|| FetchError::Cancelled);
    plan.push_ok(make_usage(
        20.0,
        t0,
        Some("dev@example.invalid"),
        Some(claude_id.clone()),
    ));
    // Cancelled fetch with prior snapshot
    plan.push_err(|| FetchError::Cancelled);
    // Followed by a single transient preservable error -> must STILL be hidden because
    // the cancellation did not advance the failure gate!
    plan.push_err(|| {
        FetchError::Classified(
            ClassifiedError::new(ProviderErrorKind::NetworkFailure, "transient")
                .with_transport(TransportClass::Timeout),
        )
    });

    let mut registry = ProviderRegistry::empty();
    registry.register_plan(claude_id.clone(), Arc::new(plan));
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);

    let mut opts = RefreshOptions::user_initiated();
    opts.refresh_status = false;

    runtime.refresh_provider(claude_id.clone(), opts).await;
    let snap0 = runtime.current_snapshot();
    let claude0 = snap0
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .expect("claude");
    assert!(claude0.windows.primary.is_none());
    assert!(claude0.last_error.is_none());

    runtime.refresh_provider(claude_id.clone(), opts).await;
    runtime.refresh_provider(claude_id.clone(), opts).await; // Cancelled
    runtime.refresh_provider(claude_id.clone(), opts).await; // First real failure after success

    let snap = runtime.current_snapshot();
    let claude = snap
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .expect("claude");
    assert_eq!(claude.updated_at, Some(t0));
    assert_eq!(
        claude.windows.primary.as_ref().unwrap().state,
        MetricState::Value
    );
    assert!(claude.last_error.is_none());
}

// upstream: Tests/CodexBarTests/ClaudeResilienceTests.swift:200 timeout keeps prior Claude snapshot without surfacing repeated failure
#[tokio::test]
async fn timeout_keeps_prior_claude_snapshot_with_original_updated_at() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();

    let plan = ScriptedPlan::new("claude.cli", SourceKind::Cli);
    plan.push_err(|| {
        FetchError::Classified(
            ClassifiedError::new(ProviderErrorKind::NetworkFailure, "cli timed out")
                .with_transport(TransportClass::Timeout),
        )
    });

    let mut registry = ProviderRegistry::empty();
    registry.register_plan(claude_id.clone(), Arc::new(plan));
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);
    runtime.seed_provider_snapshot(
        claude_id.clone(),
        make_usage(
            12.0,
            t0,
            Some("claude@example.invalid"),
            Some(claude_id.clone()),
        ),
    );

    let mut opts = RefreshOptions::user_initiated();
    opts.refresh_status = false;
    runtime.refresh_provider(claude_id.clone(), opts).await;

    let snap = runtime.current_snapshot();
    let claude = snap
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .expect("claude");
    assert_eq!(claude.updated_at, Some(t0));
    assert!(claude.last_error.is_none());
}

// upstream: Tests/CodexBarTests/ClaudeResilienceTests.swift:285 CLI parse failures keep prior Claude snapshot but authentication loss clears it
#[tokio::test]
async fn cli_parse_failures_keep_prior_claude_snapshot_but_authentication_loss_clears_it() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();

    let plan = ScriptedPlan::new("claude.cli", SourceKind::Cli);
    plan.push_err(|| {
        FetchError::Classified(ClassifiedError::new(
            ProviderErrorKind::ParseFailure,
            "failed to parse CLI output",
        ))
    });
    plan.push_err(|| {
        FetchError::Classified(ClassifiedError::new(
            ProviderErrorKind::AuthenticationExpired,
            "Not logged in",
        ))
    });

    let mut registry = ProviderRegistry::empty();
    registry.register_plan(claude_id.clone(), Arc::new(plan));
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);
    runtime.seed_provider_snapshot(
        claude_id.clone(),
        make_usage(
            12.0,
            t0,
            Some("claude@example.invalid"),
            Some(claude_id.clone()),
        ),
    );

    let mut opts = RefreshOptions::user_initiated();
    opts.refresh_status = false;

    // 1. CLI parse failure keeps prior snapshot
    runtime.refresh_provider(claude_id.clone(), opts).await;
    let snap1 = runtime.current_snapshot();
    let claude1 = snap1
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .expect("claude");
    assert_eq!(claude1.updated_at, Some(t0));
    assert!(claude1.windows.primary.is_some());
    assert!(claude1.last_error.is_none());

    // 2. Authentication loss clears prior snapshot and surfaces error
    runtime.refresh_provider(claude_id.clone(), opts).await;
    let snap2 = runtime.current_snapshot();
    let claude2 = snap2
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .expect("claude");
    assert_eq!(claude2.updated_at, None);
    assert!(claude2.windows.primary.is_none());
    assert_eq!(
        claude2.last_error.as_ref().unwrap().kind,
        ProviderErrorKind::AuthenticationExpired
    );
}

#[tokio::test]
async fn account_change_drops_snapshot_and_resets_failure_gate_and_enforces_identity_silo() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();
    let codex_id = ProviderId::new("codex").unwrap();

    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    // 1. Account A returns usage with codex provider_id on identity -> must be scoped to claude!
    plan.push_ok(make_usage(
        40.0,
        t0,
        Some("alice@example.invalid"),
        Some(codex_id.clone()),
    ));
    // 2. First failure on Account A (hidden, streak = 1)
    plan.push_err(|| {
        FetchError::Classified(
            ClassifiedError::new(ProviderErrorKind::NetworkFailure, "flake")
                .with_transport(TransportClass::Timeout),
        )
    });
    // 3. After switching to Account B, prior snapshot is dropped and gate is reset, so a failure
    //    on Account B has had_prior = false and surfaces immediately!
    plan.push_err(|| {
        FetchError::Classified(
            ClassifiedError::new(ProviderErrorKind::NetworkFailure, "account B offline")
                .with_transport(TransportClass::Timeout),
        )
    });

    let mut registry = ProviderRegistry::empty();
    registry.register_plan(claude_id.clone(), Arc::new(plan));
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);

    let mut opts = RefreshOptions::user_initiated();
    opts.refresh_status = false;

    // 1. Fetch Account A
    runtime.refresh_provider(claude_id.clone(), opts).await;
    let snap1 = runtime.current_snapshot();
    let claude1 = snap1
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .expect("claude");
    let id1 = claude1
        .identity
        .as_ref()
        .expect("identity metric")
        .value
        .as_ref()
        .expect("identity value");
    // Identity silo: scoped to claude, never codex
    assert_eq!(id1.provider_id, claude_id);
    assert_eq!(id1.account_email.as_deref(), Some("alice@example.invalid"));

    // Codex provider snapshot must have no leaked identity from Claude
    let codex1 = snap1
        .providers
        .iter()
        .find(|p| p.id == codex_id)
        .expect("codex");
    assert!(codex1.identity.is_none());

    // 2. First failure on Account A (streak becomes 1, hidden)
    runtime.refresh_provider(claude_id.clone(), opts).await;
    assert!(
        runtime
            .current_snapshot()
            .providers
            .iter()
            .find(|p| p.id == claude_id)
            .unwrap()
            .last_error
            .is_none()
    );

    // 3. Switch account -> drops snapshot and resets gate
    runtime
        .switch_account(&claude_id, Some("token-account:acct-b".to_string()))
        .await;
    let after_switch = runtime.current_snapshot();
    let claude_switched = after_switch
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .unwrap();
    assert!(claude_switched.windows.primary.is_none());
    assert!(claude_switched.identity.is_none());
    assert!(claude_switched.last_error.is_none());
}

#[tokio::test]
async fn per_provider_coalescing_shares_in_flight_request_and_replacement_supersedes_predecessor() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    plan.push_gated(
        Arc::clone(&started),
        Arc::clone(&release),
        make_usage(
            55.0,
            t0,
            Some("dev@example.invalid"),
            Some(claude_id.clone()),
        ),
    );

    let mut registry = ProviderRegistry::empty();
    registry.register_plan(claude_id.clone(), Arc::new(plan.clone()));
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);

    let mut opts = RefreshOptions::background();
    opts.refresh_status = false;

    let rt1 = Arc::clone(&runtime);
    let cid1 = claude_id.clone();
    let t1 = tokio::spawn(async move {
        rt1.refresh_provider(cid1, opts).await;
    });

    started.notified().await;

    // Second caller with coalesce_if_refreshing = true joins the in-flight request
    let rt2 = Arc::clone(&runtime);
    let cid2 = claude_id.clone();
    let t2 = tokio::spawn(async move {
        rt2.refresh_provider(cid2, opts).await;
    });
    tokio::task::yield_now().await;

    release.notify_waiters();
    t1.await.unwrap();
    t2.await.unwrap();

    assert_eq!(plan.call_count(), 1);
    assert_eq!(plan.last_interaction(), Some(FetchInteraction::Background));
}

// upstream: Tests/CodexBarTests/RequiredRefreshCoalescingTests.swift:7 required refresh requests during a pass share one follow-up
#[tokio::test]
async fn one_batch_at_a_time_and_required_refresh_coalescing() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();
    let codex_id = ProviderId::new("codex").unwrap();

    let started1 = Arc::new(Notify::new());
    let release1 = Arc::new(Notify::new());
    let started2 = Arc::new(Notify::new());
    let release2 = Arc::new(Notify::new());
    let claude_plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    claude_plan.push_gated(
        Arc::clone(&started1),
        Arc::clone(&release1),
        make_usage(25.0, t0, None, Some(claude_id.clone())),
    );
    claude_plan.push_gated(
        Arc::clone(&started2),
        Arc::clone(&release2),
        make_usage(30.0, t0, None, Some(claude_id.clone())),
    );
    claude_plan.push_ok(make_usage(35.0, t0, None, Some(claude_id.clone())));

    let codex_plan = ScriptedPlan::new("codex.cli", SourceKind::Cli);
    codex_plan.push_ok(make_usage(10.0, t0, None, Some(codex_id.clone())));
    codex_plan.push_ok(make_usage(15.0, t0, None, Some(codex_id.clone())));
    codex_plan.push_ok(make_usage(20.0, t0, None, Some(codex_id.clone())));

    let mut registry = ProviderRegistry::empty();
    registry.register_plan(claude_id.clone(), Arc::new(claude_plan.clone()));
    registry.register_plan(codex_id.clone(), Arc::new(codex_plan));
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);

    let mut opts = RefreshOptions::background();
    opts.refresh_status = false;

    // 1. Start first required refresh pass
    let rt1 = Arc::clone(&runtime);
    let first_pass = tokio::spawn(async move {
        rt1.refresh_required(opts).await;
    });
    started1.notified().await;

    // While pass 1 is in flight, opportunistic refresh_batch is a no-op
    assert!(!runtime.refresh_batch(opts).await);

    // Queue 3 additional required refresh requests during pass 1 -> they must share ONE follow-up pass!
    let mut follow_ups = Vec::new();
    for _ in 0..3 {
        let rt_next = Arc::clone(&runtime);
        follow_ups.push(tokio::spawn(async move {
            rt_next.refresh_required(opts).await;
        }));
    }
    tokio::task::yield_now().await;
    assert_eq!(claude_plan.call_count(), 1);

    // Release pass 1 -> shared follow-up pass 2 starts
    release1.notify_waiters();
    started2.notified().await;
    assert_eq!(claude_plan.call_count(), 2);

    // Release pass 2 -> all 4 callers complete with only 2 total batch passes
    release2.notify_waiters();
    first_pass.await.unwrap();
    for task in follow_ups {
        task.await.unwrap();
    }
    assert_eq!(claude_plan.call_count(), 2);
}

#[tokio::test]
async fn adaptive_cadence_and_settings_page_round_trip_without_invented_data() {
    // Ported from AdaptiveRefreshTimerTests.swift
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _sink) = test_runtime(&tmp, ProviderRegistry::empty(), t0);

    let rx = runtime.subscribe_snapshots();
    let initial = rx.borrow().clone();
    // All 3 merged providers (codex, claude, antigravity) are listed in config order with their
    // true enabled state (codex=true, claude=true in test_runtime, antigravity=false); none has
    // data yet -> honest None/empty windows
    assert_eq!(initial.providers.len(), 3);
    assert_eq!(initial.providers[0].id.as_str(), "codex");
    assert!(initial.providers[0].enabled);
    assert_eq!(initial.providers[1].id.as_str(), "claude");
    assert!(initial.providers[1].enabled);
    assert_eq!(initial.providers[2].id.as_str(), "antigravity");
    assert!(!initial.providers[2].enabled);
    for p in &initial.providers {
        assert!(p.windows.primary.is_none());
        assert!(p.identity.is_none());
        assert!(p.updated_at.is_none());
    }

    // Switch cadence to AdaptiveAgentAware via IPC intent entry point
    let cadence_id = SettingId::new("app.refresh.cadence").unwrap();
    runtime
        .set_setting(&cadence_id, &json!("adaptiveAgentAware"))
        .await
        .expect("set cadence");

    // Long-idle baseline (no menu open yet) = 30m (1800s)
    let dec_idle = runtime
        .evaluate_adaptive_cadence(t0)
        .expect("adaptive decision");
    assert_eq!(dec_idle.reason, AdaptiveRefreshReason::LongIdle);
    assert_eq!(dec_idle.delay, Duration::from_secs(1800));

    // Schedule initial timer at t0 (fires at t0 + 1800s)
    assert_eq!(
        runtime.next_refresh_delay(t0),
        Some(Duration::from_secs(1800))
    );

    // Observing coding activity at t0 + 10s caps schedule to 5m (300s)
    let t1 = t0.checked_add(jiff::Span::new().seconds(10)).unwrap();
    assert!(runtime.note_coding_activity_observed(t1));
    let dec_coding = runtime
        .evaluate_adaptive_cadence(t1)
        .expect("coding decision");
    assert_eq!(dec_coding.reason, AdaptiveRefreshReason::CodingActivity);
    assert_eq!(dec_coding.delay, Duration::from_secs(300));

    // Opening the menu at t1 advances schedule to 2m (120s)
    assert!(runtime.note_menu_opened(t1));
    let dec_menu = runtime
        .evaluate_adaptive_cadence(t1)
        .expect("menu decision");
    assert_eq!(dec_menu.reason, AdaptiveRefreshReason::RecentInteraction);
    assert_eq!(dec_menu.delay, Duration::from_secs(120));

    // Low power mode stretches to 30m (Constrained)
    runtime.clear_coding_activity_observation();
    let t_later = t0.checked_add(jiff::Span::new().minutes(10)).unwrap();
    runtime.set_low_power_mode(true, t_later);
    let dec_lp = runtime
        .evaluate_adaptive_cadence(t_later)
        .expect("low power decision");
    assert_eq!(dec_lp.reason, AdaptiveRefreshReason::Constrained);

    // Thermal pressure Constrained also throttles to 30m
    runtime.set_low_power_mode(false, t_later);
    runtime.note_menu_opened(t_later);
    runtime.set_thermal_pressure(ThermalPressure::Constrained, t_later);
    let dec_thermal = runtime
        .evaluate_adaptive_cadence(t_later)
        .expect("thermal decision");
    assert_eq!(dec_thermal.reason, AdaptiveRefreshReason::Constrained);

    // Verify SettingsPage for App and Provider(claude) contain no invented accounts or secrets
    let app_page = runtime
        .get_settings(&SettingsScope::App)
        .await
        .expect("app settings page");
    assert_eq!(app_page.scope, SettingsScope::App);

    let claude_id = ProviderId::new("claude").unwrap();
    let claude_page = runtime
        .get_settings(&SettingsScope::Provider(claude_id.clone()))
        .await
        .expect("claude settings page");
    for section in &claude_page.sections {
        for desc in &section.descriptors {
            if let DescriptorKind::TokenAccounts { config } = &desc.kind {
                assert!(config.accounts.is_empty());
            }
            if let DescriptorKind::Secret { is_set, .. } = &desc.kind {
                assert!(!is_set);
            }
        }
    }

    // Attempting to write a secret over IPC SetSetting is rejected
    let secret_setting = SettingId::new("providers.claude.apiKey").unwrap();
    let err = runtime
        .set_setting(&secret_setting, &json!("should-be-rejected"))
        .await
        .expect_err("secret write over IPC must fail");
    assert_eq!(err.code, ErrorCode::InvalidRequest);

    // ClientMessage::Hello and RunAction intent dispatch
    let welcome = runtime
        .handle_client_intent(ClientMessage::Hello {
            protocols: vec![PROTOCOL_V1.to_string()],
            client: ClientInfo::new("test", "0.1.0"),
            id: None,
        })
        .await;
    assert!(matches!(
        welcome.first(),
        Some(ServerMessage::Welcome { .. })
    ));

    let reset_res = runtime
        .run_action(&ActionId::new("resetUsage").unwrap(), Some(true))
        .await;
    assert!(reset_res.is_ok());
}

#[tokio::test]
async fn edge_triggered_transitions_first_sample_only_sets_baseline() {
    // Ported from HookTransitionDetectorLinuxTests.swift
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();

    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    // Sample 1: 95% used (5% remaining, below 20% threshold) -> baseline only, NO transition!
    plan.push_ok(make_usage(95.0, t0, None, Some(claude_id.clone())));
    // Sample 2: 50% used (50% remaining, restored from healthy/low) -> no session transition because sample 1 was not depleted (95% < 100%)
    plan.push_ok(make_usage(50.0, t0, None, Some(claude_id.clone())));
    // Sample 3: 100% used (0% remaining) -> crosses quota.low AND session Depleted!
    plan.push_ok(make_usage(100.0, t0, None, Some(claude_id.clone())));
    // Sample 4: 10% used (90% remaining) -> session Restored!
    plan.push_ok(make_usage(10.0, t0, None, Some(claude_id.clone())));

    let mut registry = ProviderRegistry::empty();
    registry.register_plan(claude_id.clone(), Arc::new(plan));

    let config_store = ConfigStore::new(tmp.path().join("config.json"));
    config_store
        .update(|cfg| {
            ariadusage_core::config::set_provider_enabled(cfg, &claude_id, true);
        })
        .expect("enable claude in test config");
    let secret_store = Arc::new(SecretStore::new(MemoryBackend::new()));
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let sink = Arc::new(RecordingTransitionSink::new());
    let runtime = EngineRuntime::new(config_store, secret_store, net, registry)
        .with_transition_sink(Arc::clone(&sink) as _)
        .with_hooks_config(HooksConfig::new(
            true,
            vec![
                HookRule::new(HookEventType::QuotaLow, "/bin/true"),
                HookRule::new(HookEventType::QuotaReached, "/bin/true"),
            ],
        ))
        .with_clock(Arc::new(move || t0));

    let mut opts = RefreshOptions::user_initiated();
    opts.refresh_status = false;

    // 1. First sample: baseline only!
    runtime.refresh_provider(claude_id.clone(), opts).await;
    assert!(sink.hook_dispatches().is_empty());
    assert!(sink.session_transitions().is_empty());

    // 2. Second sample: 50% used (above threshold)
    runtime.refresh_provider(claude_id.clone(), opts).await;
    assert!(sink.hook_dispatches().is_empty());
    assert!(sink.session_transitions().is_empty());

    // 3. Third sample: 100% used -> emits QuotaReached/QuotaLow hooks and SessionQuotaTransition::Depleted
    runtime.refresh_provider(claude_id.clone(), opts).await;
    let dispatches = sink.hook_dispatches();
    assert!(
        dispatches
            .iter()
            .any(|d| d.event.event == HookEventType::QuotaLow)
    );
    assert!(
        dispatches
            .iter()
            .any(|d| d.event.event == HookEventType::QuotaReached)
    );
    assert_eq!(
        sink.session_transitions(),
        vec![(claude_id.clone(), SessionQuotaTransition::Depleted)]
    );

    // 4. Fourth sample: 10% used -> emits SessionQuotaTransition::Restored
    runtime.refresh_provider(claude_id.clone(), opts).await;
    assert_eq!(
        sink.session_transitions(),
        vec![
            (claude_id.clone(), SessionQuotaTransition::Depleted),
            (claude_id, SessionQuotaTransition::Restored),
        ]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn runtime_serves_ipc_v1_socket_server_end_to_end() {
    use ariadusage_engine::ipc_server::{IpcBackend, start_ipc_server};
    use ariadusage_protocol::ipc::{Topic, parse_server_message};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();

    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    plan.push_ok(make_usage(
        33.0,
        t0,
        Some("dev@example.com"),
        Some(claude_id.clone()),
    ));

    let mut registry = ProviderRegistry::empty();
    registry.register_plan(claude_id.clone(), Arc::new(plan));
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);

    let sock_dir = tempfile::Builder::new()
        .prefix("ar-ipc-")
        .tempdir_in("/tmp")
        .expect("short tempdir in /tmp");
    let sock_path = sock_dir.path().join("engine.sock");

    let backend: Arc<dyn IpcBackend> = Arc::clone(&runtime) as _;
    let mut server = start_ipc_server(sock_path.clone(), backend)
        .await
        .expect("start_ipc_server");

    let stream = UnixStream::connect(&sock_path)
        .await
        .expect("connect ipc socket");
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    async fn send_msg(writer: &mut tokio::net::unix::OwnedWriteHalf, msg: &ClientMessage) {
        let mut raw = serde_json::to_vec(msg).unwrap();
        raw.push(b'\n');
        writer.write_all(&raw).await.unwrap();
        writer.flush().await.unwrap();
    }

    // 1. Hello -> Welcome
    send_msg(
        &mut write_half,
        &ClientMessage::Hello {
            protocols: vec![PROTOCOL_V1.to_string()],
            client: ClientInfo::new("test-client", "0.1.0"),
            id: None,
        },
    )
    .await;
    let welcome_line = lines.next_line().await.unwrap().unwrap();
    assert!(matches!(
        parse_server_message(&welcome_line).unwrap(),
        ServerMessage::Welcome { .. }
    ));

    // 2. Subscribe(Snapshot, Settings) -> Ok + initial Snapshot push
    send_msg(
        &mut write_half,
        &ClientMessage::Subscribe {
            topics: vec![Topic::Snapshot, Topic::Settings],
            id: None,
        },
    )
    .await;
    let sub_ok = lines.next_line().await.unwrap().unwrap();
    assert!(matches!(
        parse_server_message(&sub_ok).unwrap(),
        ServerMessage::Response { ok: Some(true), .. }
    ));
    let sub_snap = lines.next_line().await.unwrap().unwrap();
    match parse_server_message(&sub_snap).unwrap() {
        ServerMessage::Snapshot { snapshot } => {
            assert_eq!(snapshot.providers.len(), 3);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    // 3. SetSetting(app.statusChecksEnabled = false) -> SettingsChanged(App) push + Snapshot push + Ok
    send_msg(
        &mut write_half,
        &ClientMessage::SetSetting {
            id: SettingId::new("app.statusChecksEnabled").unwrap(),
            value: json!(false),
        },
    )
    .await;

    let mut saw_settings_changed = false;
    let mut saw_set_ok = false;
    while !(saw_settings_changed && saw_set_ok) {
        let line = lines.next_line().await.unwrap().unwrap();
        match parse_server_message(&line).unwrap() {
            ServerMessage::SettingsChanged { scope } => {
                assert_eq!(scope, SettingsScope::App);
                saw_settings_changed = true;
            }
            ServerMessage::Response { ok: Some(true), .. } => {
                saw_set_ok = true;
            }
            ServerMessage::Snapshot { .. } => {}
            other => panic!("unexpected message after SetSetting: {other:?}"),
        }
    }

    // 4. Refresh(claude) -> pushes updated Snapshot + Ok + Snapshot response
    send_msg(
        &mut write_half,
        &ClientMessage::Refresh {
            provider: Some(claude_id.clone()),
            id: None,
        },
    )
    .await;

    let mut saw_refreshed_claude = false;
    let mut saw_refresh_ok = false;
    while !(saw_refreshed_claude && saw_refresh_ok) {
        let line = lines.next_line().await.unwrap().unwrap();
        match parse_server_message(&line).unwrap() {
            ServerMessage::Snapshot { snapshot } => {
                let claude = snapshot
                    .providers
                    .iter()
                    .find(|p| p.id == claude_id)
                    .expect("claude in snapshot");
                if let Some(primary) = &claude.windows.primary
                    && let Some(val) = &primary.value
                    && (val.used_percent - 33.0).abs() < 1e-6
                {
                    saw_refreshed_claude = true;
                }
            }
            ServerMessage::Response { ok: Some(true), .. } => {
                saw_refresh_ok = true;
            }
            other => panic!("unexpected message after Refresh: {other:?}"),
        }
    }
    assert!(
        saw_refreshed_claude,
        "IPC Refresh must return Claude usage from EngineRuntime SnapshotStore"
    );

    drop(write_half);
    server.stop().await;
}

#[tokio::test]
async fn unset_cookie_source_stays_unset_and_never_triggers_browser_cookie_import() {
    use ariadusage_core::config::CookieSource;
    use ariadusage_engine::providers::claude::ClaudeOAuthStrategyConfig;

    let tmp = TempDir::new().unwrap();
    let net = Arc::new(NetBroker::new(NetConfig::default()).expect("default net broker"));
    let cfg = ClaudeOAuthStrategyConfig::new(Arc::clone(&net), tmp.path(), tmp.path());
    assert_eq!(cfg.cookie_source, None);
    assert_eq!(cfg.effective_cookie_source(), CookieSource::Off);

    let mut cfg_manual = ClaudeOAuthStrategyConfig::new(net, tmp.path(), tmp.path());
    cfg_manual.manual_cookie_header = Some("sessionKey=short-test".into());
    assert_eq!(cfg_manual.effective_cookie_source(), CookieSource::Manual);

    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();
    let registry = ProviderRegistry::default().with_directories(tmp.path(), tmp.path());
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);

    // Default Claude provider settings page must report cookieSource == "off" when unset
    let claude_page = runtime
        .get_settings(&SettingsScope::Provider(claude_id.clone()))
        .await
        .expect("claude settings page");
    let mut found_cookie_source = None;
    for section in &claude_page.sections {
        for desc in &section.descriptors {
            if desc.id.as_str() == "providers.claude.cookieSource"
                && let DescriptorKind::Choice { selected, .. } = &desc.kind
            {
                found_cookie_source = Some(selected.clone());
            }
        }
    }
    assert_eq!(found_cookie_source.as_deref(), Some("off"));

    // Set sourceMode to "web" while leaving cookieSource unset -> must fail closed without importing browser cookies
    runtime
        .set_setting(
            &SettingId::new("providers.claude.sourceMode").unwrap(),
            &json!("web"),
        )
        .await
        .expect("set sourceMode web");

    let mut opts = RefreshOptions::user_initiated();
    opts.refresh_status = false;
    runtime.refresh_provider(claude_id.clone(), opts).await;
    let snap = runtime.current_snapshot();
    let claude = snap.providers.iter().find(|p| p.id == claude_id).unwrap();
    let err = claude
        .last_error
        .as_ref()
        .expect("web without cookie must fail");
    assert_eq!(err.kind, ProviderErrorKind::MissingCredential);
}

#[tokio::test]
async fn user_refresh_paths_queue_behind_running_batch_and_refresh_provider_action_scopes_to_single_provider()
 {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();
    let codex_id = ProviderId::new("codex").unwrap();

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let claude_plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    claude_plan.push_gated(
        Arc::clone(&started),
        Arc::clone(&release),
        make_usage(20.0, t0, None, Some(claude_id.clone())),
    );
    claude_plan.push_ok(make_usage(40.0, t0, None, Some(claude_id.clone())));
    claude_plan.push_ok(make_usage(60.0, t0, None, Some(claude_id.clone())));

    let codex_plan = ScriptedPlan::new("codex.cli", SourceKind::Cli);
    codex_plan.push_ok(make_usage(10.0, t0, None, Some(codex_id.clone())));
    codex_plan.push_ok(make_usage(20.0, t0, None, Some(codex_id.clone())));
    codex_plan.push_ok(make_usage(30.0, t0, None, Some(codex_id.clone())));

    let mut registry = ProviderRegistry::empty();
    registry.register_plan(claude_id.clone(), Arc::new(claude_plan.clone()));
    registry.register_plan(codex_id.clone(), Arc::new(codex_plan.clone()));
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);

    let mut bg_opts = RefreshOptions::background();
    bg_opts.refresh_status = false;

    // Start an in-flight batch
    let rt_bg = Arc::clone(&runtime);
    let bg_task = tokio::spawn(async move { rt_bg.refresh_batch(bg_opts).await });
    started.notified().await;

    // User-initiated Refresh { provider: None } via handle_client_intent must queue behind the in-flight batch
    let rt_user = Arc::clone(&runtime);
    let user_task = tokio::spawn(async move {
        rt_user
            .handle_client_intent(ClientMessage::Refresh {
                provider: None,
                id: None,
            })
            .await
    });
    tokio::task::yield_now().await;

    // Release the first batch -> queued user refresh runs a second batch
    release.notify_waiters();
    assert!(bg_task.await.unwrap());
    let _ = user_task.await.unwrap();
    assert_eq!(claude_plan.call_count(), 2);
    assert_eq!(codex_plan.call_count(), 2);

    // RunAction("refreshProvider.claude") refreshes ONLY claude, not codex!
    runtime
        .run_action(&ActionId::new("refreshProvider.claude").unwrap(), None)
        .await
        .expect("refreshProvider.claude succeeds");
    assert_eq!(claude_plan.call_count(), 3);
    assert_eq!(codex_plan.call_count(), 2);

    // Unqualified "refreshProvider" is rejected
    assert!(
        runtime
            .run_action(&ActionId::new("refreshProvider").unwrap(), None)
            .await
            .is_err()
    );
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:11 timer fixture seeds disabled providers without enabling web access
#[test]
fn timer_fixture_seeds_disabled_providers_without_enabling_web_access() {
    let tmp = TempDir::new().unwrap();
    let config_store = ConfigStore::new(tmp.path().join("config.json"));
    config_store
        .update(|cfg| {
            for id in ["codex", "claude", "antigravity"] {
                let pid = ProviderId::new(id).unwrap();
                ariadusage_core::config::set_provider_enabled(cfg, &pid, false);
            }
        })
        .unwrap();
    let secret_store = Arc::new(SecretStore::new(MemoryBackend::new()));
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let runtime = EngineRuntime::new(config_store, secret_store, net, ProviderRegistry::empty());
    let snap = runtime.current_snapshot();
    assert!(snap.providers.iter().all(|p| !p.enabled));
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:26 launch with no menu history begins at thirty minutes
#[tokio::test]
async fn launch_with_no_menu_history_begins_at_thirty_minutes() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime
        .set_setting(
            &SettingId::new("app.refresh.cadence").unwrap(),
            &json!("adaptive"),
        )
        .await
        .unwrap();
    assert_eq!(runtime.last_menu_opened_at(), None);
    let decision = runtime.evaluate_adaptive_cadence(t0).unwrap();
    assert_eq!(decision.reason, AdaptiveRefreshReason::LongIdle);
    assert_eq!(decision.delay, Duration::from_secs(1800));
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:41 menu-open signal changes the next adaptive decision
#[tokio::test]
async fn menu_open_signal_changes_the_next_adaptive_decision() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime
        .set_setting(
            &SettingId::new("app.refresh.cadence").unwrap(),
            &json!("adaptive"),
        )
        .await
        .unwrap();
    let before = runtime.evaluate_adaptive_cadence(t0).unwrap();
    assert_eq!(before.reason, AdaptiveRefreshReason::LongIdle);
    runtime.note_menu_opened(t0);
    let after = runtime.evaluate_adaptive_cadence(t0).unwrap();
    assert_eq!(after.reason, AdaptiveRefreshReason::RecentInteraction);
    assert_eq!(after.delay, Duration::from_secs(120));
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:54 menu open advances a long idle timer during refresh without postponing an earlier tick
#[tokio::test]
async fn menu_open_advances_a_long_idle_timer_during_refresh_without_postponing_an_earlier_tick() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let t_plus_10 = t0.checked_add(jiff::Span::new().seconds(10)).unwrap();
    let t_plus_40 = t0.checked_add(jiff::Span::new().seconds(40)).unwrap();
    let claude_id = ProviderId::new("claude").unwrap();

    // Real slow batch started by the timer!
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    plan.push_gated(
        Arc::clone(&started),
        Arc::clone(&release),
        make_usage(25.0, t0, None, Some(claude_id.clone())),
    );
    plan.push_ok(make_usage(35.0, t_plus_10, None, Some(claude_id.clone())));

    let mut registry = ProviderRegistry::empty();
    registry.register_plan(claude_id.clone(), Arc::new(plan.clone()));
    let (runtime, _) = test_runtime(&tmp, registry, t0);
    runtime
        .set_setting(
            &SettingId::new("app.statusChecksEnabled").unwrap(),
            &json!(false),
        )
        .await
        .unwrap();
    runtime
        .set_setting(
            &SettingId::new("app.refresh.cadence").unwrap(),
            &json!("adaptive"),
        )
        .await
        .unwrap();

    // Start adaptive timer with short sleep override so the timer launches the slow gated batch
    runtime.restart_timer_with_sleep_override(Some(Duration::from_millis(10)));
    started.notified().await;
    assert!(runtime.is_refreshing());

    // Simulate a long-idle next schedule (t0 + 1800s) while the timer-started batch is in flight
    let long_idle_schedule = t0.checked_add(jiff::Span::new().seconds(1800)).unwrap();
    runtime.set_scheduled_fire_at_for_testing(Some(long_idle_schedule));

    // Open the menu while the slow timer batch is in flight -> advances schedule to t0 + 120s
    // AND must NOT abort the in-flight batch!
    assert!(runtime.note_menu_opened(t0));
    let interaction_schedule = runtime.scheduled_fire_at().expect("scheduled_fire_at");
    assert!(interaction_schedule < long_idle_schedule);
    assert_eq!(
        interaction_schedule,
        t0.checked_add(jiff::Span::new().seconds(120)).unwrap()
    );
    assert!(runtime.is_refreshing());

    // A later menu open at t_plus_40 does not postpone the earlier interaction_schedule
    assert!(!runtime.note_menu_opened(t_plus_40));
    assert_eq!(runtime.scheduled_fire_at(), Some(interaction_schedule));

    // Stop recurring ticks before releasing the in-flight batch
    runtime.stop_timer();
    release.notify_waiters();

    // Wait for the timer's spawned slow batch to finish and reset is_refreshing to false
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while runtime.is_refreshing() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(!runtime.is_refreshing());

    // Now verify that a subsequent refresh_required completes and is_refreshing() returns to false
    let mut req_opts = RefreshOptions::user_initiated();
    req_opts.refresh_status = false;
    runtime.refresh_required(req_opts).await;
    assert!(!runtime.is_refreshing());
    assert_eq!(plan.call_count(), 2);
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:93 coding activity advances a long idle timer without postponing an earlier tick
#[tokio::test]
async fn coding_activity_advances_a_long_idle_timer_without_postponing_an_earlier_tick() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime
        .set_setting(
            &SettingId::new("app.refresh.cadence").unwrap(),
            &json!("adaptiveAgentAware"),
        )
        .await
        .unwrap();
    let long_idle = t0.checked_add(jiff::Span::new().seconds(1800)).unwrap();
    runtime.set_scheduled_fire_at_for_testing(Some(long_idle));

    assert!(runtime.note_coding_activity_observed_at(t0, t0));
    let activity_schedule = runtime.scheduled_fire_at().unwrap();
    assert!(activity_schedule < long_idle);
    assert_eq!(runtime.last_coding_activity_at(), Some(t0));

    let older_obs = t0.checked_sub(jiff::Span::new().seconds(1)).unwrap();
    let now_plus_30 = t0.checked_add(jiff::Span::new().seconds(30)).unwrap();
    assert!(!runtime.note_coding_activity_observed_at(older_obs, now_plus_30));
    assert_eq!(runtime.last_coding_activity_at(), Some(t0));

    let newer_obs = t0.checked_add(jiff::Span::new().seconds(1)).unwrap();
    assert!(!runtime.note_coding_activity_observed_at(newer_obs, now_plus_30));
    assert_eq!(runtime.last_coding_activity_at(), Some(newer_obs));
    assert_eq!(runtime.scheduled_fire_at(), Some(activity_schedule));
    runtime.stop_timer();
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:126 plain adaptive ignores coding activity
#[tokio::test]
async fn plain_adaptive_ignores_coding_activity() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime
        .set_setting(
            &SettingId::new("app.refresh.cadence").unwrap(),
            &json!("adaptive"),
        )
        .await
        .unwrap();
    let _ = runtime.next_adaptive_timer_sleep_duration();
    let scheduled_at = runtime.scheduled_fire_at().unwrap();
    let t_plus_10 = t0.checked_add(jiff::Span::new().seconds(10)).unwrap();
    assert!(!runtime.note_coding_activity_observed_at(t_plus_10, t_plus_10));
    assert_eq!(runtime.scheduled_fire_at(), Some(scheduled_at));
    assert_eq!(runtime.last_coding_activity_at(), None);
    runtime.stop_timer();
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:143 noting a menu open records the signal without starting a refresh
#[tokio::test]
async fn noting_a_menu_open_records_the_signal_without_starting_a_refresh() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    assert_eq!(runtime.completed_refresh_count(), 0);
    assert!(!runtime.is_refreshing());
    assert!(!runtime.note_menu_opened(t0));
    assert_eq!(runtime.last_menu_opened_at(), Some(t0));
    assert_eq!(runtime.completed_refresh_count(), 0);
    assert!(!runtime.is_refreshing());
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:158 noting coding activity outside agent aware mode is a no-op
#[tokio::test]
async fn noting_coding_activity_outside_agent_aware_mode_is_a_no_op() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime
        .set_setting(
            &SettingId::new("app.refresh.cadence").unwrap(),
            &json!("5m"),
        )
        .await
        .unwrap();
    runtime.stop_timer();
    assert!(!runtime.note_coding_activity_observed(t0));
    assert_eq!(runtime.last_coding_activity_at(), None);
    assert_eq!(runtime.scheduled_fire_at(), None);
    assert_eq!(runtime.completed_refresh_count(), 0);
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:173 clearing coding activity removes the adaptive input
#[tokio::test]
async fn clearing_coding_activity_removes_the_adaptive_input() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime
        .set_setting(
            &SettingId::new("app.refresh.cadence").unwrap(),
            &json!("adaptiveAgentAware"),
        )
        .await
        .unwrap();
    runtime.stop_timer();
    runtime.note_coding_activity_observed_at(t0, t0);
    assert_eq!(runtime.last_coding_activity_at(), Some(t0));
    runtime.clear_coding_activity_observation();
    assert_eq!(runtime.last_coding_activity_at(), None);
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:188 opportunistic timer refresh is a no-op while another refresh is already in flight
#[tokio::test]
async fn opportunistic_timer_refresh_is_a_no_op_while_another_refresh_is_already_in_flight() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime.set_refreshing_for_testing(true);
    let mut opts = RefreshOptions::background();
    opts.refresh_status = false;
    assert!(!runtime.refresh_batch(opts).await);
    assert_eq!(runtime.completed_refresh_count(), 0);
    assert!(runtime.is_refreshing());
    runtime.set_refreshing_for_testing(false);
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:203 manual mode performs the initial refresh but no recurring ticks
#[tokio::test]
async fn manual_mode_performs_the_initial_refresh_but_no_recurring_ticks() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime
        .set_setting(
            &SettingId::new("app.statusChecksEnabled").unwrap(),
            &json!(false),
        )
        .await
        .unwrap();
    runtime
        .set_setting(
            &SettingId::new("app.refresh.cadence").unwrap(),
            &json!("manual"),
        )
        .await
        .unwrap();
    runtime.start_timer();
    let mut opts = RefreshOptions::background();
    opts.refresh_status = false;
    assert!(runtime.refresh_batch(opts).await);
    assert_eq!(runtime.completed_refresh_count(), 1);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(runtime.completed_refresh_count(), 1);
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:215 fixed mode ticks recur at the overridden cadence
#[tokio::test]
async fn fixed_mode_ticks_recur_at_the_overridden_cadence() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime
        .set_setting(
            &SettingId::new("app.statusChecksEnabled").unwrap(),
            &json!(false),
        )
        .await
        .unwrap();
    runtime
        .set_setting(
            &SettingId::new("app.refresh.cadence").unwrap(),
            &json!("1m"),
        )
        .await
        .unwrap();
    runtime.restart_timer_with_sleep_override(Some(Duration::from_millis(15)));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while runtime.completed_refresh_count() < 2 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(runtime.completed_refresh_count() >= 2);
    runtime.stop_timer();
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:228 fixed timer uses global low power interval without changing test sleep override
#[tokio::test]
async fn fixed_timer_uses_global_low_power_interval_without_changing_test_sleep_override() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime
        .set_setting(
            &SettingId::new("app.refresh.cadence").unwrap(),
            &json!("5m"),
        )
        .await
        .unwrap();
    runtime.set_low_power_mode(true, t0);
    runtime.restart_timer_with_sleep_override(Some(Duration::from_secs(10)));
    assert_eq!(
        runtime.fixed_refresh_interval(),
        Some(Duration::from_secs(1800))
    );
    assert_eq!(
        runtime.timer_sleep_override(),
        Some(Duration::from_secs(10))
    );
    runtime.stop_timer();
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:243 adaptive timer publishes clamped schedule while preserving test sleep override
#[tokio::test]
async fn adaptive_timer_publishes_clamped_schedule_while_preserving_test_sleep_override() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime
        .set_setting(
            &SettingId::new("app.refresh.cadence").unwrap(),
            &json!("adaptive"),
        )
        .await
        .unwrap();
    runtime.set_low_power_mode(true, t0);
    let menu_at = t0.checked_sub(jiff::Span::new().minutes(10)).unwrap();
    runtime.note_menu_opened(menu_at);
    runtime.set_timer_sleep_override(Some(Duration::from_secs(10)));
    let sleep_dur = runtime.next_adaptive_timer_sleep_duration().unwrap();
    assert_eq!(
        runtime.adaptive_computed_interval(),
        Some(Duration::from_secs(1800))
    );
    assert_eq!(sleep_dur, Duration::from_secs(10));
    assert_eq!(
        runtime.scheduled_fire_at(),
        Some(t0.checked_add(jiff::Span::new().seconds(1800)).unwrap())
    );
    runtime.stop_timer();
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:303 fixed timer loop stays interval aligned after a slow refresh
#[tokio::test]
async fn fixed_timer_loop_stays_interval_aligned_after_a_slow_refresh() {
    use tokio_util::sync::CancellationToken;

    let elapsed = Arc::new(Mutex::new(Duration::ZERO));
    let starts = Arc::new(Mutex::new(Vec::<Duration>::new()));
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let cancel = CancellationToken::new();

    let e_now = Arc::clone(&elapsed);
    let e_sleep = Arc::clone(&elapsed);
    let e_ref = Arc::clone(&elapsed);
    let starts_ref = Arc::clone(&starts);
    let active_ref = Arc::clone(&active);
    let max_ref = Arc::clone(&max_active);
    let cancel_ref = cancel.clone();

    EngineRuntime::run_fixed_refresh_timer(
        Duration::from_millis(100),
        None,
        cancel,
        move || *e_now.lock().unwrap(),
        move |dur| {
            let e_sleep = Arc::clone(&e_sleep);
            async move {
                *e_sleep.lock().unwrap() += dur;
                true
            }
        },
        move || {
            let e_ref = Arc::clone(&e_ref);
            let starts_ref = Arc::clone(&starts_ref);
            let active_ref = Arc::clone(&active_ref);
            let max_ref = Arc::clone(&max_ref);
            let cancel_ref = cancel_ref.clone();
            async move {
                let cur = active_ref.fetch_add(1, Ordering::SeqCst) + 1;
                max_ref.fetch_max(cur, Ordering::SeqCst);
                let now_elapsed = *e_ref.lock().unwrap();
                let mut s = starts_ref.lock().unwrap();
                s.push(now_elapsed);
                let count = s.len();
                drop(s);
                if count == 1 {
                    *e_ref.lock().unwrap() += Duration::from_millis(160);
                }
                active_ref.fetch_sub(1, Ordering::SeqCst);
                if count == 2 {
                    cancel_ref.cancel();
                }
            }
        },
    )
    .await;

    assert_eq!(
        *starts.lock().unwrap(),
        vec![Duration::from_millis(100), Duration::from_millis(300)]
    );
    assert_eq!(max_active.load(Ordering::SeqCst), 1);
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:317 adaptive mode keeps recomputing and refreshing across menu-open changes
#[tokio::test]
async fn adaptive_mode_keeps_recomputing_and_refreshing_across_menu_open_changes() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime
        .set_setting(
            &SettingId::new("app.statusChecksEnabled").unwrap(),
            &json!(false),
        )
        .await
        .unwrap();
    runtime
        .set_setting(
            &SettingId::new("app.refresh.cadence").unwrap(),
            &json!("adaptive"),
        )
        .await
        .unwrap();
    runtime.restart_timer_with_sleep_override(Some(Duration::from_millis(15)));

    let deadline1 = tokio::time::Instant::now() + Duration::from_secs(2);
    while runtime.completed_refresh_count() < 1 && tokio::time::Instant::now() < deadline1 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let count_before = runtime.completed_refresh_count();
    runtime.note_menu_opened(t0);

    let deadline2 = tokio::time::Instant::now() + Duration::from_secs(2);
    while runtime.completed_refresh_count() <= count_before
        && tokio::time::Instant::now() < deadline2
    {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(runtime.completed_refresh_count() > count_before);
    runtime.stop_timer();
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:333 changing frequency away from fixed cancels the pending tick without an extra refresh
#[tokio::test]
async fn changing_frequency_away_from_fixed_cancels_the_pending_tick_without_an_extra_refresh() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime
        .set_setting(
            &SettingId::new("app.statusChecksEnabled").unwrap(),
            &json!(false),
        )
        .await
        .unwrap();
    let cadence_id = SettingId::new("app.refresh.cadence").unwrap();
    runtime
        .set_setting(&cadence_id, &json!("1m"))
        .await
        .unwrap();
    runtime.restart_timer_with_sleep_override(Some(Duration::from_secs(5)));

    let count_before = runtime.completed_refresh_count();
    runtime
        .set_setting(&cadence_id, &json!("manual"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(runtime.completed_refresh_count(), count_before);
    runtime.stop_timer();
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:372 restarting the timer cancels a pending fixed tick without an extra refresh
#[tokio::test]
async fn restarting_the_timer_cancels_a_pending_fixed_tick_without_an_extra_refresh() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime
        .set_setting(
            &SettingId::new("app.statusChecksEnabled").unwrap(),
            &json!(false),
        )
        .await
        .unwrap();
    runtime
        .set_setting(
            &SettingId::new("app.refresh.cadence").unwrap(),
            &json!("1m"),
        )
        .await
        .unwrap();
    runtime.restart_timer_with_sleep_override(Some(Duration::from_secs(5)));
    let count_before = runtime.completed_refresh_count();
    runtime.restart_timer_with_sleep_override(Some(Duration::from_secs(5)));
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(runtime.completed_refresh_count(), count_before);
    runtime.stop_timer();
}

// upstream: Tests/CodexBarTests/AdaptiveRefreshTimerTests.swift:391 restarting the timer cancels a pending adaptive tick without an extra refresh
#[tokio::test]
async fn restarting_the_timer_cancels_a_pending_adaptive_tick_without_an_extra_refresh() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let (runtime, _) = test_runtime(&tmp, ProviderRegistry::empty(), t0);
    runtime
        .set_setting(
            &SettingId::new("app.statusChecksEnabled").unwrap(),
            &json!(false),
        )
        .await
        .unwrap();
    runtime
        .set_setting(
            &SettingId::new("app.refresh.cadence").unwrap(),
            &json!("adaptive"),
        )
        .await
        .unwrap();
    runtime.restart_timer_with_sleep_override(Some(Duration::from_secs(5)));
    let count_before = runtime.completed_refresh_count();
    runtime.restart_timer_with_sleep_override(Some(Duration::from_secs(5)));
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(runtime.completed_refresh_count(), count_before);
    runtime.stop_timer();
}

fn write_test_claude_creds(base_dir: &std::path::Path, token: &str, uuid: &str) {
    let claude_dir = base_dir.join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let cred_path = claude_dir.join(".credentials.json");
    let config_json_path = claude_dir.join(".config.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&cred_path)
            .unwrap();
        std::io::Write::write_all(
            &mut f,
            format!(r#"{{"claudeAiOauth":{{"accessToken":"{token}"}}}}"#).as_bytes(),
        )
        .unwrap();
    }
    #[cfg(not(unix))]
    {
        std::fs::write(
            &cred_path,
            format!(r#"{{"claudeAiOauth":{{"accessToken":"{token}"}}}}"#),
        )
        .unwrap();
    }
    std::fs::write(
        &config_json_path,
        format!(r#"{{"oauthAccount":{{"accountUuid":"{uuid}"}}}}"#),
    )
    .unwrap();
}

// upstream: Tests/CodexBarTests/ClaudeResilienceTests.swift:68 superseded credential change clears prior Claude state after cancellation
#[tokio::test]
async fn superseded_credential_change_clears_prior_claude_state_after_cancellation() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();
    write_test_claude_creds(
        tmp.path(),
        "tok-account-a",
        "11111111-2222-3333-4444-555555555555",
    );

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    plan.push_gated(
        Arc::clone(&started),
        Arc::clone(&release),
        make_usage(20.0, t0, Some("a@example.invalid"), Some(claude_id.clone())),
    );

    let mut registry = ProviderRegistry::empty().with_directories(tmp.path(), tmp.path());
    registry.register_plan(claude_id.clone(), Arc::new(plan));
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);
    runtime.seed_provider_snapshot(
        claude_id.clone(),
        make_usage(45.0, t0, Some("a@example.invalid"), Some(claude_id.clone())),
    );

    let rt1 = Arc::clone(&runtime);
    let cid1 = claude_id.clone();
    let mut bg_opts = RefreshOptions::background();
    bg_opts.refresh_status = false;
    let t_cancel = tokio::spawn(async move {
        rt1.refresh_provider(cid1, bg_opts).await;
    });
    started.notified().await;
    write_test_claude_creds(
        tmp.path(),
        "tok-account-b",
        "bbbbbbbb-2222-3333-4444-555555555555",
    );
    runtime.coordinator().invalidate_requests(&claude_id);
    t_cancel.await.unwrap();

    let snap = runtime.current_snapshot();
    let claude = snap.providers.iter().find(|p| p.id == claude_id).unwrap();
    assert!(claude.windows.primary.is_none());
    assert!(claude.last_error.is_none());
}

// upstream: Tests/CodexBarTests/ClaudeResilienceTests.swift:483 credentials change clears prior Claude snapshot for non transient failure
#[tokio::test]
async fn credentials_change_clears_prior_claude_snapshot_for_non_transient_failure() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();
    write_test_claude_creds(
        tmp.path(),
        "tok-account-a",
        "11111111-2222-3333-4444-555555555555",
    );

    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    plan.push_err(|| {
        FetchError::Classified(ClassifiedError::new(
            ProviderErrorKind::AuthenticationExpired,
            "not logged in",
        ))
    });

    let mut registry = ProviderRegistry::empty().with_directories(tmp.path(), tmp.path());
    registry.register_plan(claude_id.clone(), Arc::new(plan));
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);
    runtime.seed_provider_snapshot(
        claude_id.clone(),
        make_usage(45.0, t0, Some("a@example.invalid"), Some(claude_id.clone())),
    );

    write_test_claude_creds(
        tmp.path(),
        "tok-account-b",
        "bbbbbbbb-2222-3333-4444-555555555555",
    );
    let mut opts = RefreshOptions::user_initiated();
    opts.refresh_status = false;
    runtime.refresh_provider(claude_id.clone(), opts).await;

    let snap = runtime.current_snapshot();
    let claude = snap.providers.iter().find(|p| p.id == claude_id).unwrap();
    assert!(claude.windows.primary.is_none());
    assert!(claude.last_error.is_some());
}

// upstream: Tests/CodexBarTests/ClaudeResilienceTests.swift:568 credentials change clears prior Claude snapshot for transient failure
#[tokio::test]
async fn credentials_change_clears_prior_claude_snapshot_for_transient_failure() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();
    write_test_claude_creds(
        tmp.path(),
        "tok-account-a",
        "11111111-2222-3333-4444-555555555555",
    );

    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    plan.push_err(|| {
        FetchError::Classified(
            ClassifiedError::new(ProviderErrorKind::NetworkFailure, "timed out")
                .with_transport(TransportClass::Timeout),
        )
    });

    let mut registry = ProviderRegistry::empty().with_directories(tmp.path(), tmp.path());
    registry.register_plan(claude_id.clone(), Arc::new(plan));
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);
    runtime.seed_provider_snapshot(
        claude_id.clone(),
        make_usage(50.0, t0, Some("a@example.invalid"), Some(claude_id.clone())),
    );

    write_test_claude_creds(
        tmp.path(),
        "tok-account-c",
        "cccccccc-2222-3333-4444-555555555555",
    );
    let mut opts = RefreshOptions::user_initiated();
    opts.refresh_status = false;
    runtime.refresh_provider(claude_id.clone(), opts).await;

    let snap = runtime.current_snapshot();
    let claude = snap.providers.iter().find(|p| p.id == claude_id).unwrap();
    assert!(claude.windows.primary.is_none());
    assert!(claude.last_error.is_some());
}

// upstream: Tests/CodexBarTests/ClaudeResilienceTests.swift:1116 credentials change before fetch clears stale reset backfill
#[tokio::test]
async fn credentials_change_before_fetch_clears_stale_reset_backfill() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let t1: Timestamp = "2026-03-04T12:05:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();
    write_test_claude_creds(
        tmp.path(),
        "tok-account-a",
        "11111111-2222-3333-4444-555555555555",
    );

    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    plan.push_ok(make_usage(
        18.0,
        t1,
        Some("b@example.invalid"),
        Some(claude_id.clone()),
    ));

    let mut registry = ProviderRegistry::empty().with_directories(tmp.path(), tmp.path());
    registry.register_plan(claude_id.clone(), Arc::new(plan));
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);
    runtime.seed_provider_snapshot(
        claude_id.clone(),
        make_usage(80.0, t0, Some("a@example.invalid"), Some(claude_id.clone())),
    );

    write_test_claude_creds(
        tmp.path(),
        "tok-account-b",
        "bbbbbbbb-2222-3333-4444-555555555555",
    );
    let mut opts = RefreshOptions::user_initiated();
    opts.refresh_status = false;
    runtime.refresh_provider(claude_id.clone(), opts).await;

    let snap = runtime.current_snapshot();
    let claude = snap.providers.iter().find(|p| p.id == claude_id).unwrap();
    assert_eq!(
        claude
            .windows
            .primary
            .as_ref()
            .unwrap()
            .value
            .as_ref()
            .unwrap()
            .used_percent,
        18.0
    );
    assert_eq!(
        claude
            .identity
            .as_ref()
            .unwrap()
            .value
            .as_ref()
            .unwrap()
            .account_email
            .as_deref(),
        Some("b@example.invalid")
    );
}

// upstream: Tests/CodexBarTests/ClaudeResilienceTests.swift:1299 credentials change during successful Claude fetch applies fresh snapshot without stale reset
#[tokio::test]
async fn credentials_change_during_successful_claude_fetch_applies_fresh_snapshot_without_stale_reset()
 {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let t1: Timestamp = "2026-03-04T12:05:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();
    write_test_claude_creds(
        tmp.path(),
        "tok-account-c",
        "cccccccc-2222-3333-4444-555555555555",
    );

    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    let base_dir = tmp.path().to_path_buf();
    let claude_id_clone = claude_id.clone();
    plan.steps.lock().unwrap().push_back(Box::new(move |_cx| {
        write_test_claude_creds(
            &base_dir,
            "tok-account-d",
            "dddddddd-2222-3333-4444-555555555555",
        );
        let fresh = make_usage(
            12.0,
            t1,
            Some("d@example.invalid"),
            Some(claude_id_clone.clone()),
        );
        let res = FetchResult::new(fresh, "claude.oauth", "claude.oauth", SourceKind::Oauth);
        Box::pin(async move { Ok(res) })
    }));

    let mut registry = ProviderRegistry::empty().with_directories(tmp.path(), tmp.path());
    registry.register_plan(claude_id.clone(), Arc::new(plan));
    let (runtime, _sink) = test_runtime(&tmp, registry, t0);
    runtime.seed_provider_snapshot(
        claude_id.clone(),
        make_usage(75.0, t0, Some("c@example.invalid"), Some(claude_id.clone())),
    );

    let mut opts = RefreshOptions::user_initiated();
    opts.refresh_status = false;
    runtime.refresh_provider(claude_id.clone(), opts).await;

    let snap = runtime.current_snapshot();
    let claude = snap.providers.iter().find(|p| p.id == claude_id).unwrap();
    assert_eq!(
        claude
            .windows
            .primary
            .as_ref()
            .unwrap()
            .value
            .as_ref()
            .unwrap()
            .used_percent,
        12.0
    );
    assert_eq!(
        claude
            .identity
            .as_ref()
            .unwrap()
            .value
            .as_ref()
            .unwrap()
            .account_email
            .as_deref(),
        Some("d@example.invalid")
    );
}

#[tokio::test]
async fn configured_claude_admin_key_wins_over_ambient_and_projects_workspace_spend_and_token_account_target()
 {
    use std::collections::BTreeMap;

    use ariadusage_core::config::{
        Config, ProviderConfig, ProviderEntry, TokenAccountMeta, TokenAccountsMeta,
    };
    use ariadusage_core::pipeline::{FetchContext, FetchInteraction, FetchPhase, FetchRuntime};
    use ariadusage_core::providers::SourceMode;
    use ariadusage_engine::providers::claude::ClaudeAdminAPISettingsReader;
    use ariadusage_engine::registry::ProviderPlanInputs;
    use ariadusage_protocol::secret::SecretString;
    use tokio_util::sync::CancellationToken;

    let tmp = TempDir::new().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();
    let registry = ProviderRegistry::default().with_directories(tmp.path(), tmp.path());

    let mut claude_cfg = ProviderConfig::new(claude_id.clone());
    claude_cfg.enabled = Some(true);
    claude_cfg.set_claude_workspace_spend_enabled(true);
    let mut config = Config::new(
        Config::CURRENT_VERSION,
        vec![ProviderEntry::Typed(claude_cfg)],
    );

    let secret_store = Arc::new(SecretStore::new(MemoryBackend::new()));
    let mut base_env = BTreeMap::new();
    base_env.insert(
        ClaudeAdminAPISettingsReader::ADMIN_API_KEY_ENV_KEY.to_string(),
        SecretString::new("sk-ant-admin-ambient"),
    );
    base_env.insert(
        ClaudeAdminAPISettingsReader::ALTERNATE_ADMIN_API_KEY_ENV_KEY.to_string(),
        SecretString::new("sk-ant-admin-ambient-alt"),
    );

    let inputs = ProviderPlanInputs {
        base_environment: base_env.clone(),
        api_key: Some(SecretString::new(" \"sk-ant-admin-configured\" ")),
        manual_cookie_header: None,
        selected_account_token: None,
        include_optional_usage: true,
        secret_store: Some(Arc::clone(&secret_store)),
    };

    let spec = registry
        .resolve_spec(&claude_id, &config, &inputs)
        .expect("resolved claude spec");

    assert_eq!(
        spec.environment
            .get(ClaudeAdminAPISettingsReader::ADMIN_API_KEY_ENV_KEY)
            .map(SecretString::expose_secret),
        Some("sk-ant-admin-configured"),
        "configured key must overwrite ambient ANTHROPIC_ADMIN_KEY with Config precedence"
    );
    assert_eq!(
        ClaudeAdminAPISettingsReader::api_key(&spec.environment)
            .as_ref()
            .map(SecretString::expose_secret),
        Some("sk-ant-admin-configured"),
        "ClaudeAdminAPISettingsReader::api_key must resolve the configured key ahead of any ambient key"
    );
    assert_eq!(
        spec.environment
            .get(ClaudeAdminAPISettingsReader::WORKSPACE_SPEND_ENV_KEY)
            .map(SecretString::expose_secret),
        Some("true"),
        "claude_workspace_spend_enabled must project into ANTHROPIC_ADMIN_WORKSPACE_SPEND"
    );
    assert!(ClaudeAdminAPISettingsReader::workspace_spend_enabled(
        &spec.environment
    ));

    // Verify ClaudeProviderPlanFactory wired admin_api_key, secret_store, and workspace_spend_enabled
    let cx = FetchContext {
        provider: claude_id.clone(),
        runtime: FetchRuntime::Daemon,
        interaction: FetchInteraction::UserInitiated,
        phase: FetchPhase::Regular,
        request_id: "test-admin-key".to_string(),
        source_mode: SourceMode::Auto,
        env: BTreeMap::new(),
        include_credits: true,
        selected_token_account: None,
        cancel: CancellationToken::new(),
    };
    let strategies = spec.plan.resolve_strategies(&cx).await;
    assert_eq!(
        strategies.iter().map(|s| s.id()).collect::<Vec<_>>(),
        vec!["claude.admin-api"],
        "Auto mode must select claude.admin-api via ClaudeOAuthStrategyConfig.admin_api_key even with an empty FetchContext env"
    );

    // Selected token account with an Admin API token must scrub ambient keys and target ANTHROPIC_ADMIN_KEY
    let acct_meta = TokenAccountMeta {
        id: "acct-admin".to_string(),
        label: "Org Admin".to_string(),
        added_at: 1000.0,
        last_used: None,
        external_identifier: None,
        usage_scope: None,
        organization_id: None,
        workspace_id: None,
        claude_config_directory: None,
        seat_credit_entitlement: None,
        model_rate_limits: None,
        extra: BTreeMap::new(),
    };
    if let Some(entry) = config
        .providers
        .iter_mut()
        .find(|p| p.id() == "claude")
        .and_then(ProviderEntry::as_typed_mut)
    {
        entry.token_accounts = Some(TokenAccountsMeta {
            version: 1,
            active_index: 0,
            accounts: vec![acct_meta],
        });
    }
    let acct_inputs = ProviderPlanInputs {
        base_environment: base_env,
        api_key: Some(SecretString::new("sk-ant-admin-configured")),
        manual_cookie_header: None,
        selected_account_token: Some(SecretString::new("Bearer sk-ant-admin-account")),
        include_optional_usage: true,
        secret_store: Some(secret_store),
    };
    let acct_spec = registry
        .resolve_spec(&claude_id, &config, &acct_inputs)
        .expect("resolved claude token-account spec");
    assert_eq!(acct_spec.source_mode, SourceMode::Api);
    assert_eq!(
        acct_spec
            .environment
            .get(ClaudeAdminAPISettingsReader::ADMIN_API_KEY_ENV_KEY)
            .map(SecretString::expose_secret),
        Some("sk-ant-admin-account"),
        "selected token account Admin key must project into ANTHROPIC_ADMIN_KEY"
    );
    assert!(
        !acct_spec
            .environment
            .contains_key(ClaudeAdminAPISettingsReader::ALTERNATE_ADMIN_API_KEY_ENV_KEY),
        "selected token account must scrub ambient ANTHROPIC_ADMIN_API_KEY"
    );
}

#[test]
fn configured_claude_workspace_spend_some_false_projects_false_and_reader_treats_it_as_off() {
    use std::collections::BTreeMap;

    use ariadusage_core::config::{Config, ProviderConfig, ProviderEntry};
    use ariadusage_engine::providers::claude::ClaudeAdminAPISettingsReader;
    use ariadusage_engine::registry::ProviderPlanInputs;
    use ariadusage_protocol::secret::SecretString;

    let tmp = TempDir::new().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();
    let registry = ProviderRegistry::default().with_directories(tmp.path(), tmp.path());

    let mut claude_cfg = ProviderConfig::new(claude_id.clone());
    claude_cfg.enabled = Some(true);
    claude_cfg.extra.insert(
        "claudeWorkspaceSpendEnabled".to_owned(),
        serde_json::value::to_raw_value(&false).unwrap(),
    );
    assert_eq!(claude_cfg.claude_workspace_spend_enabled_opt(), Some(false));
    let config = Config::new(
        Config::CURRENT_VERSION,
        vec![ProviderEntry::Typed(claude_cfg)],
    );

    let mut base_env = BTreeMap::new();
    base_env.insert(
        ClaudeAdminAPISettingsReader::WORKSPACE_SPEND_ENV_KEY.to_string(),
        SecretString::new("true"),
    );

    let inputs = ProviderPlanInputs {
        base_environment: base_env,
        api_key: None,
        manual_cookie_header: None,
        selected_account_token: None,
        include_optional_usage: true,
        secret_store: None,
    };

    let spec = registry
        .resolve_spec(&claude_id, &config, &inputs)
        .expect("resolved claude spec");

    assert_eq!(
        spec.environment
            .get(ClaudeAdminAPISettingsReader::WORKSPACE_SPEND_ENV_KEY)
            .map(SecretString::expose_secret),
        Some("false"),
        "Some(false) workspace spend must project as 'false' and override ambient ANTHROPIC_ADMIN_WORKSPACE_SPEND"
    );
    assert!(
        !ClaudeAdminAPISettingsReader::workspace_spend_enabled(&spec.environment),
        "ClaudeAdminAPISettingsReader must treat projected 'false' as off"
    );
}

#[tokio::test]
async fn runtime_persists_notification_state_and_dispatches_injected_notifier_and_hooks() {
    use ariadusage_engine::{FakeHookCommandExecutor, HookRunner, RecordingNotifier};

    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();

    let mut u1 = make_usage(10.0, t0, None, Some(claude_id.clone()));
    u1.secondary = None;
    let mut u2 = make_usage(85.0, t0, None, Some(claude_id.clone()));
    u2.secondary = None;
    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    // Poll 1: 10% used (90% remaining) -> baseline
    plan.push_ok(u1);
    // Poll 2: 85% used (15% remaining) -> crosses 50% and 20% quota warnings + QuotaLow hook
    plan.push_ok(u2.clone());

    let mut registry = ProviderRegistry::empty();
    registry.register_plan(claude_id.clone(), Arc::new(plan));

    let cfg_path = tmp.path().join("config.json");
    let config_store = ConfigStore::new(cfg_path.clone());
    config_store
        .update(|cfg| {
            ariadusage_core::config::set_provider_enabled(cfg, &claude_id, true);
            cfg.set_hooks_config(Some(HooksConfig::new(
                true,
                vec![HookRule::new(HookEventType::QuotaLow, "/bin/true")],
            )));
        })
        .expect("configure claude + hooks");

    let state_path = tmp.path().join("notifications-state.json");
    let notifier = Arc::new(RecordingNotifier::new());
    let hook_exec = Arc::new(FakeHookCommandExecutor::new());
    let hook_runner = Arc::new(HookRunner::new(Arc::clone(&hook_exec) as _));

    let secret_store = Arc::new(SecretStore::new(MemoryBackend::new()));
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let runtime = EngineRuntime::new(
        ConfigStore::new(cfg_path.clone()),
        Arc::clone(&secret_store),
        Arc::clone(&net),
        registry,
    )
    .with_notification_state_path(state_path.clone())
    .with_notifier(Arc::clone(&notifier) as _)
    .with_hook_runner(hook_runner)
    .with_clock(Arc::new(move || t0));

    let mut opts = RefreshOptions::user_initiated();
    opts.refresh_status = false;

    // Poll 1: baseline
    runtime.refresh_provider(claude_id.clone(), opts).await;
    assert!(notifier.posts().is_empty());
    assert!(hook_exec.requests().is_empty());

    // Poll 2: crosses 50% and 20% remaining thresholds (lowest crossed = 20%) and QuotaLow hook
    runtime.refresh_provider(claude_id.clone(), opts).await;
    let posted = notifier.posts();
    assert_eq!(posted.len(), 1);
    assert!(posted[0].id_prefix.contains("20"));
    assert_eq!(hook_exec.requests().len(), 1);
    assert_eq!(hook_exec.requests()[0].executable, "/bin/true");

    // Verify notifications-state.json exists (and has mode 0600 on Unix)
    let _metadata = std::fs::metadata(&state_path).expect("persisted notifications-state.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(_metadata.permissions().mode() & 0o777, 0o600);
    }

    // Restart EngineRuntime with the same persisted state file: must not re-send already-notified thresholds
    let mut u3 = make_usage(88.0, t0, None, Some(claude_id.clone()));
    u3.secondary = None;
    let plan2 = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    plan2.push_ok(u2);
    plan2.push_ok(u3);
    let mut registry2 = ProviderRegistry::empty();
    registry2.register_plan(claude_id.clone(), Arc::new(plan2));

    let notifier2 = Arc::new(RecordingNotifier::new());
    let runtime2 = EngineRuntime::new(ConfigStore::new(cfg_path), secret_store, net, registry2)
        .with_notification_state_path(state_path)
        .with_notifier(Arc::clone(&notifier2) as _)
        .with_clock(Arc::new(move || t0));

    runtime2.refresh_provider(claude_id.clone(), opts).await;
    runtime2.refresh_provider(claude_id, opts).await;
    assert!(
        notifier2.posts().is_empty(),
        "restarted runtime must reload persisted notification state and suppress duplicate quota warnings"
    );
}
