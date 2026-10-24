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

#[tokio::test]
async fn cancellation_is_not_a_failure_and_never_increments_gate_or_drops_data() {
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();

    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    plan.push_ok(make_usage(
        20.0,
        t0,
        Some("dev@example.invalid"),
        Some(claude_id.clone()),
    ));
    // Cancelled fetch
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

#[tokio::test]
async fn one_batch_at_a_time_and_required_refresh_coalescing() {
    // Ported from RequiredRefreshCoalescingTests.swift
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
        make_usage(25.0, t0, None, Some(claude_id.clone())),
    );
    claude_plan.push_ok(make_usage(30.0, t0, None, Some(claude_id.clone())));
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

    let rt_batch = Arc::clone(&runtime);
    let batch_task = tokio::spawn(async move { rt_batch.refresh_batch(opts).await });
    started.notified().await;

    // While the first batch is in flight, a second opportunistic refresh_batch is a no-op!
    assert!(!runtime.refresh_batch(opts).await);

    release.notify_waiters();
    assert!(batch_task.await.unwrap());
    assert_eq!(claude_plan.call_count(), 1);
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

#[tokio::test]
async fn adaptive_and_fixed_refresh_timer_parity_tests() {
    // Ported from AdaptiveRefreshTimerTests.swift:
    // - `launchWithNoMenuHistoryBeginsAtThirtyMinutes`
    // - `menuOpenChangesDecisionToTwoMinutes`
    // - `menuOpenAdvancesLongIdleTimerDuringRefresh`
    // - `menuOpenDoesNotPostponeSoonerTimer`
    // - `codingActivityAdvancesLongIdleTimer`
    // - `codingActivityDoesNotPostponeEarlierMenuDrivenTimer`
    // - `plainAdaptiveModeIgnoresCodingActivity`
    // - `notingMenuOpenOutsideAdaptiveModeOnlyRecordsHistory`
    // - `notingCodingActivityOutsideAgentAwareModeIsIgnored`
    // - `clearingCodingActivityObservationStopsFiveMinuteOverride`
    // - `opportunisticTimerRefreshIsANoOpWhileAnotherRefreshIsAlreadyInFlight`
    // - `manualModeNeverStartsRecurringTimer`
    // - `fixedModeTicksRecurAtOverriddenCadence`
    // - `fixedTimerUsesLowPowerClampedIntervalWhilePreservingSleepOverride`
    // - `adaptiveTimerUsesLowPowerClampedIntervalWhilePreservingSleepOverride`
    // - `fixedTimerLoopStaysIntervalAlignedAfterASlowRefresh`
    // - `adaptiveModeKeepsRecomputingAndRefreshingAcrossMenuOpenChanges`
    // - `changingFrequencyAwayFromFixedCancelsPendingTickWithoutExtraRefresh`
    // - `restartingFixedTimerReplacesExistingLoopWithoutDoubleFiring`
    let tmp = TempDir::new().unwrap();
    let clock_now = Arc::new(Mutex::new(
        "2026-03-04T12:00:00Z".parse::<Timestamp>().unwrap(),
    ));
    let claude_id = ProviderId::new("claude").unwrap();
    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    for _ in 0..20 {
        let ts = *clock_now.lock().unwrap();
        plan.push_ok(make_usage(15.0, ts, None, Some(claude_id.clone())));
    }

    let mut registry = ProviderRegistry::empty();
    registry.register_plan(claude_id.clone(), Arc::new(plan));

    let config_store = ConfigStore::new(tmp.path().join("config.json"));
    config_store
        .update(|cfg| {
            ariadusage_core::config::set_provider_enabled(cfg, &claude_id, true);
        })
        .unwrap();
    let secret_store = Arc::new(SecretStore::new(MemoryBackend::new()));
    let net = Arc::new(NetBroker::new(NetConfig::default()).unwrap());
    let cn = Arc::clone(&clock_now);
    let runtime = Arc::new(
        EngineRuntime::new(config_store, secret_store, net, registry)
            .with_clock(Arc::new(move || *cn.lock().unwrap())),
    );

    let t0 = *clock_now.lock().unwrap();
    let cadence_id = SettingId::new("app.refresh.cadence").unwrap();
    let status_checks_id = SettingId::new("app.statusChecksEnabled").unwrap();
    runtime
        .set_setting(&status_checks_id, &json!(false))
        .await
        .unwrap();

    // 1. Noting menu open & coding activity in Manual mode
    runtime
        .set_setting(&cadence_id, &json!("manual"))
        .await
        .unwrap();
    runtime.start_timer();
    assert_eq!(runtime.scheduled_fire_at(), None);
    assert!(!runtime.note_menu_opened(t0));
    assert_eq!(runtime.last_menu_opened_at(), Some(t0));
    assert!(!runtime.note_coding_activity_observed(t0));
    assert_eq!(runtime.last_coding_activity_at(), None);

    // 2. Plain Adaptive mode: launch with no menu history begins at 30m, ignores coding activity
    let tmp2 = TempDir::new().unwrap();
    let (rt_adapt, _) = test_runtime(&tmp2, ProviderRegistry::empty(), t0);
    rt_adapt
        .set_setting(&cadence_id, &json!("adaptive"))
        .await
        .unwrap();
    assert_eq!(
        rt_adapt.next_adaptive_timer_sleep_duration(),
        Some(Duration::from_secs(1800))
    );
    assert_eq!(
        rt_adapt.scheduled_fire_at(),
        Some(t0.checked_add(jiff::Span::new().seconds(1800)).unwrap())
    );
    // Plain adaptive ignores coding activity
    let t_plus_10 = t0.checked_add(jiff::Span::new().seconds(10)).unwrap();
    assert!(!rt_adapt.note_coding_activity_observed_at(t_plus_10, t_plus_10));
    assert_eq!(rt_adapt.last_coding_activity_at(), None);
    // Menu open changes decision to 2m and advances long-idle timer even during an in-flight refresh
    rt_adapt.set_refreshing_for_testing(true);
    assert!(rt_adapt.note_menu_opened(t_plus_10));
    assert_eq!(
        rt_adapt.scheduled_fire_at(),
        Some(
            t_plus_10
                .checked_add(jiff::Span::new().seconds(120))
                .unwrap()
        )
    );
    rt_adapt.set_refreshing_for_testing(false);
    // Menu open does not postpone a sooner timer
    let sooner = t_plus_10
        .checked_add(jiff::Span::new().seconds(30))
        .unwrap();
    rt_adapt.set_scheduled_fire_at_for_testing(Some(sooner));
    assert!(!rt_adapt.note_menu_opened(t_plus_10));
    assert_eq!(rt_adapt.scheduled_fire_at(), Some(sooner));

    // 3. AdaptiveAgentAware: coding activity advances long-idle timer, does not postpone menu-driven timer
    let tmp3 = TempDir::new().unwrap();
    let (rt_agent, _) = test_runtime(&tmp3, ProviderRegistry::empty(), t0);
    rt_agent
        .set_setting(&cadence_id, &json!("adaptiveAgentAware"))
        .await
        .unwrap();
    rt_agent.set_scheduled_fire_at_for_testing(Some(
        t0.checked_add(jiff::Span::new().seconds(1800)).unwrap(),
    ));
    assert!(rt_agent.note_coding_activity_observed_at(t0, t0));
    assert_eq!(
        rt_agent.scheduled_fire_at(),
        Some(t0.checked_add(jiff::Span::new().seconds(300)).unwrap())
    );
    // Does not postpone earlier menu-driven timer (t0 + 120s)
    rt_agent.set_scheduled_fire_at_for_testing(Some(
        t0.checked_add(jiff::Span::new().seconds(120)).unwrap(),
    ));
    let t_plus_30 = t0.checked_add(jiff::Span::new().seconds(30)).unwrap();
    assert!(!rt_agent.note_coding_activity_observed_at(t_plus_30, t_plus_30));
    assert_eq!(
        rt_agent.scheduled_fire_at(),
        Some(t0.checked_add(jiff::Span::new().seconds(120)).unwrap())
    );
    // Clearing coding activity observation stops 5m override
    rt_agent.clear_coding_activity_observation();
    assert_eq!(rt_agent.last_coding_activity_at(), None);

    // 4. Low-power clamping while preserving sleep override for both fixed and adaptive timers
    rt_adapt.set_low_power_mode(true, t0);
    rt_adapt
        .set_setting(&cadence_id, &json!("1m"))
        .await
        .unwrap();
    rt_adapt.restart_timer_with_sleep_override(Some(Duration::from_millis(20)));
    assert_eq!(
        rt_adapt.fixed_refresh_interval(),
        Some(Duration::from_secs(1800))
    );
    assert_eq!(
        rt_adapt.timer_sleep_override(),
        Some(Duration::from_millis(20))
    );
    rt_adapt.stop_timer();

    rt_adapt
        .set_setting(&cadence_id, &json!("adaptive"))
        .await
        .unwrap();
    rt_adapt.set_timer_sleep_override(Some(Duration::from_millis(20)));
    assert_eq!(
        rt_adapt.next_adaptive_timer_sleep_duration(),
        Some(Duration::from_millis(20))
    );
    assert_eq!(
        rt_adapt.adaptive_computed_interval(),
        Some(Duration::from_secs(1800))
    );

    // 5. Fixed mode recurring ticks with sleep override + cancelling / restarting timer
    runtime
        .set_setting(&cadence_id, &json!("1m"))
        .await
        .unwrap();
    runtime.restart_timer_with_sleep_override(Some(Duration::from_millis(15)));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while runtime.completed_refresh_count() < 2 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(runtime.completed_refresh_count() >= 2);

    // Changing frequency away from fixed to manual cancels pending tick without extra refresh
    runtime
        .set_setting(&cadence_id, &json!("manual"))
        .await
        .unwrap();
    let count_after_manual = runtime.completed_refresh_count();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(runtime.completed_refresh_count(), count_after_manual);
    runtime.stop_timer();
}

#[tokio::test]
async fn claude_credential_change_resilience_tests() {
    // Ported from ClaudeResilienceTests.swift:
    // - `supersededCredentialChangeClearsPriorClaudeStateAfterCancellation`
    // - `credentialsChangeClearsPriorClaudeSnapshotForNonTransientFailure`
    // - `credentialsChangeClearsPriorClaudeSnapshotForTransientFailure`
    // - `credentialsChangeBeforeFetchClearsStaleResetBackfill`
    // - `credentialsChangeDuringSuccessfulClaudeFetchAppliesFreshSnapshotWithoutStaleReset`
    let tmp = TempDir::new().unwrap();
    let t0: Timestamp = "2026-03-04T12:00:00Z".parse().unwrap();
    let t1: Timestamp = "2026-03-04T12:05:00Z".parse().unwrap();
    let claude_id = ProviderId::new("claude").unwrap();

    let claude_dir = tmp.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let cred_path = claude_dir.join(".credentials.json");
    let config_json_path = claude_dir.join(".config.json");

    let write_creds = |token: &str, uuid: &str| {
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
    };

    write_creds("tok-account-a", "11111111-2222-3333-4444-555555555555");

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let plan = ScriptedPlan::new("claude.oauth", SourceKind::Oauth);
    // Step 1: Cancelled in-flight fetch while credentials change to account B -> clears prior state!
    plan.push_gated(
        Arc::clone(&started),
        Arc::clone(&release),
        make_usage(20.0, t0, Some("a@example.invalid"), Some(claude_id.clone())),
    );
    // Step 2: Transient failure after credentials change to account C -> clears prior snapshot immediately!
    plan.push_err(|| {
        FetchError::Classified(
            ClassifiedError::new(ProviderErrorKind::NetworkFailure, "timed out")
                .with_transport(TransportClass::Timeout),
        )
    });
    // Step 3: Successful fetch during which credentials change to account D -> applies fresh snapshot!
    let cred_path_clone = cred_path.clone();
    let config_path_clone = config_json_path.clone();
    let claude_id_clone = claude_id.clone();
    plan.steps.lock().unwrap().push_back(Box::new(move |_cx| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&cred_path_clone)
                .unwrap();
            std::io::Write::write_all(
                &mut f,
                br#"{"claudeAiOauth":{"accessToken":"tok-account-d"}}"#,
            )
            .unwrap();
        }
        #[cfg(not(unix))]
        {
            std::fs::write(
                &cred_path_clone,
                br#"{"claudeAiOauth":{"accessToken":"tok-account-d"}}"#,
            )
            .unwrap();
        }
        std::fs::write(
            &config_path_clone,
            r#"{"oauthAccount":{"accountUuid":"dddddddd-2222-3333-4444-555555555555"}}"#,
        )
        .unwrap();
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

    let mut opts = RefreshOptions::user_initiated();
    opts.refresh_status = false;

    // Seed prior Account A snapshot
    runtime.seed_provider_snapshot(
        claude_id.clone(),
        make_usage(45.0, t0, Some("a@example.invalid"), Some(claude_id.clone())),
    );
    assert!(
        runtime
            .current_snapshot()
            .providers
            .iter()
            .find(|p| p.id == claude_id)
            .unwrap()
            .windows
            .primary
            .is_some()
    );

    // 1. Start refresh, change credentials to Account B while in flight, then cancel via replacement
    let rt1 = Arc::clone(&runtime);
    let cid1 = claude_id.clone();
    let mut bg_opts = RefreshOptions::background();
    bg_opts.refresh_status = false;
    let t_cancel = tokio::spawn(async move {
        rt1.refresh_provider(cid1, bg_opts).await;
    });
    started.notified().await;
    write_creds("tok-account-b", "bbbbbbbb-2222-3333-4444-555555555555");
    runtime.coordinator().invalidate_requests(&claude_id);
    t_cancel.await.unwrap();
    // Prior Account A snapshot was cleared when credentials changed!
    let snap_after_b = runtime.current_snapshot();
    let claude_b = snap_after_b
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .unwrap();
    assert!(claude_b.windows.primary.is_none());

    // 2. Seed Account B snapshot, change credentials to Account C, and run transient failure -> must clear prior snapshot and surface error immediately
    runtime.seed_provider_snapshot(
        claude_id.clone(),
        make_usage(50.0, t0, Some("b@example.invalid"), Some(claude_id.clone())),
    );
    write_creds("tok-account-c", "cccccccc-2222-3333-4444-555555555555");
    runtime.refresh_provider(claude_id.clone(), opts).await;
    let snap_after_c = runtime.current_snapshot();
    let claude_c = snap_after_c
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .unwrap();
    assert!(claude_c.windows.primary.is_none());
    assert!(claude_c.last_error.is_some());

    // 3. Seed Account C snapshot, run successful fetch during which credentials change to Account D -> applies fresh Account D snapshot!
    runtime.seed_provider_snapshot(
        claude_id.clone(),
        make_usage(75.0, t0, Some("c@example.invalid"), Some(claude_id.clone())),
    );
    runtime.refresh_provider(claude_id.clone(), opts).await;
    let snap_after_d = runtime.current_snapshot();
    let claude_d = snap_after_d
        .providers
        .iter()
        .find(|p| p.id == claude_id)
        .unwrap();
    assert_eq!(
        claude_d
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
        claude_d
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
