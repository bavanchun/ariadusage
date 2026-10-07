// Ported from CodexBar Tests/CodexBarTests/ClaudeOAuthDelegatedRefreshCoordinatorTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/ClaudeOAuthDelegatedRefreshUnreadableResultTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use ariadusage_core::gates::delegated_cooldown::{
    DelegatedRefreshCooldown, DelegatedRefreshOutcome,
};
use ariadusage_core::pipeline::FetchInteraction;
use ariadusage_engine::brokers::call::BrokerCall;
use ariadusage_engine::brokers::credential_file::CredentialDecl;
use ariadusage_engine::brokers::oauth::{
    BoxFuture, DelegatedCoordinatorResult, DelegatedRefreshCoordinator, DelegatedRefreshError,
    DelegatedRefresher,
};
use ariadusage_engine::state_store::BrokerStateStore;
use jiff::Timestamp;
use tokio_util::sync::CancellationToken;

fn make_call(interaction: FetchInteraction) -> BrokerCall {
    BrokerCall {
        interaction,
        cancel: CancellationToken::new(),
        request_id: "test-req".to_string(),
    }
}

type TouchHook =
    Box<dyn Fn(usize) -> BoxFuture<'static, Result<(), DelegatedRefreshError>> + Send + Sync>;

struct MockRefresher {
    available: AtomicBool,
    touch_count: AtomicUsize,
    touch_hook: std::sync::Mutex<Option<TouchHook>>,
}

impl MockRefresher {
    fn new(available: bool) -> Self {
        Self {
            available: AtomicBool::new(available),
            touch_count: AtomicUsize::new(0),
            touch_hook: std::sync::Mutex::new(None),
        }
    }

    fn with_hook<F, Fut>(self, hook: F) -> Self
    where
        F: Fn(usize) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), DelegatedRefreshError>> + Send + 'static,
    {
        *self.touch_hook.lock().unwrap() = Some(Box::new(move |c| Box::pin(hook(c))));
        self
    }
}

impl DelegatedRefresher for MockRefresher {
    fn is_available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    fn touch(&self, _timeout: Duration) -> BoxFuture<'_, Result<(), DelegatedRefreshError>> {
        let count = self.touch_count.fetch_add(1, Ordering::SeqCst);
        if let Some(hook) = self.touch_hook.lock().unwrap().as_ref() {
            hook(count)
        } else {
            Box::pin(async { Ok(()) })
        }
    }
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthDelegatedRefreshCoordinatorTests.swift:99
#[tokio::test]
async fn test_cooldown_prevents_repeated_background_attempts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cred_path = temp.path().join("creds.json");
    std::fs::write(&cred_path, b"initial creds").expect("write creds");
    let decl = CredentialDecl::for_test(&cred_path);

    let now_sec = Arc::new(std::sync::atomic::AtomicI64::new(1_000_000));
    let now_sec_clone = now_sec.clone();
    let clock = move || Timestamp::from_second(now_sec_clone.load(Ordering::SeqCst)).unwrap();

    let refresher = Arc::new(MockRefresher::new(true));
    let cooldown = DelegatedRefreshCooldown::new(clock);
    let coordinator = DelegatedRefreshCoordinator::new(refresher.clone(), cooldown, None)
        .with_observation_ticks(vec![Duration::from_millis(1)]);

    let call = make_call(FetchInteraction::Background);

    // 1st attempt: no change -> FailedOrUnchanged with 20s cooldown
    let res1 = coordinator.refresh("prof-1", &decl, &call, true).await;
    assert_eq!(res1, DelegatedCoordinatorResult::FailedOrUnchanged);
    assert_eq!(refresher.touch_count.load(Ordering::SeqCst), 1);

    // Advance clock 5s (still within 20s cooldown)
    now_sec.fetch_add(5, Ordering::SeqCst);

    // 2nd attempt: skipped by active cooldown
    let res2 = coordinator.refresh("prof-1", &decl, &call, true).await;
    assert!(matches!(
        res2,
        DelegatedCoordinatorResult::SkippedByCooldown { .. }
    ));
    assert_eq!(
        refresher.touch_count.load(Ordering::SeqCst),
        1,
        "No additional touch should occur while under cooldown"
    );
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthDelegatedRefreshCoordinatorTests.swift:139
#[tokio::test]
async fn test_cli_unavailable_returns_cli_unavailable() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cred_path = temp.path().join("creds.json");
    std::fs::write(&cred_path, b"initial creds").expect("write creds");
    let decl = CredentialDecl::for_test(&cred_path);

    let refresher = Arc::new(MockRefresher::new(false)); // Unavailable
    let coordinator = DelegatedRefreshCoordinator::new(
        refresher.clone(),
        DelegatedRefreshCooldown::default(),
        None,
    );

    let call = make_call(FetchInteraction::UserInitiated);
    let res = coordinator.refresh("prof-1", &decl, &call, true).await;

    assert_eq!(res, DelegatedCoordinatorResult::CliUnavailable);
    assert_eq!(refresher.touch_count.load(Ordering::SeqCst), 0);
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthDelegatedRefreshCoordinatorTests.swift:157
#[tokio::test]
async fn test_background_refresh_never_launches_without_opt_in() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cred_path = temp.path().join("creds.json");
    std::fs::write(&cred_path, b"initial creds").expect("write creds");
    let decl = CredentialDecl::for_test(&cred_path);

    let refresher = Arc::new(MockRefresher::new(true));
    let coordinator = DelegatedRefreshCoordinator::new(
        refresher.clone(),
        DelegatedRefreshCooldown::default(),
        None,
    );

    let call = make_call(FetchInteraction::Background);
    // allow_background is false
    let res = coordinator.refresh("prof-1", &decl, &call, false).await;

    assert_eq!(res, DelegatedCoordinatorResult::SkippedBackgroundDisabled);
    assert_eq!(refresher.touch_count.load(Ordering::SeqCst), 0);
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthDelegatedRefreshCoordinatorTests.swift:228
#[tokio::test]
async fn test_successful_auth_touch_sets_5_minute_cooldown() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cred_path = temp.path().join("creds.json");
    std::fs::write(&cred_path, b"initial creds").expect("write creds");
    let decl = CredentialDecl::for_test(&cred_path);

    let cred_path_clone = cred_path.clone();
    let refresher = Arc::new(MockRefresher::new(true).with_hook(move |_| {
        let cred_path = cred_path_clone.clone();
        async move {
            std::fs::write(&cred_path, b"new rotated credentials").unwrap();
            Ok(())
        }
    }));

    let state_file = temp.path().join("broker-state.json");
    let state_store = Arc::new(BrokerStateStore::new(state_file));
    let cooldown = DelegatedRefreshCooldown::default();

    let coordinator = DelegatedRefreshCoordinator::new(
        refresher.clone(),
        cooldown.clone(),
        Some(state_store.clone()),
    )
    .with_observation_ticks(vec![Duration::from_millis(5)]);

    let call = make_call(FetchInteraction::UserInitiated);
    let res = coordinator
        .refresh("prof-success", &decl, &call, true)
        .await;

    assert_eq!(res, DelegatedCoordinatorResult::Success);
    assert_eq!(refresher.touch_count.load(Ordering::SeqCst), 1);

    // Verify 5 minute (300 s) cooldown
    let entry = cooldown
        .get_entry("prof-success")
        .expect("cooldown entry recorded");
    assert_eq!(entry.interval(), Duration::from_secs(300));

    // Verify state store persisted success
    let persisted = state_store.load().expect("load persisted state");
    assert!(persisted.last_seen.contains_key("prof-success"));
    assert!(persisted.delegated_cooldowns.contains_key("prof-success"));
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthDelegatedRefreshCoordinatorTests.swift:265
#[tokio::test]
async fn test_failed_auth_touch_sets_20_second_cooldown() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cred_path = temp.path().join("creds.json");
    std::fs::write(&cred_path, b"initial creds").expect("write creds");
    let decl = CredentialDecl::for_test(&cred_path);

    let refresher =
        Arc::new(MockRefresher::new(true).with_hook(|_| async {
            Err(DelegatedRefreshError::Failed("process failed".to_string()))
        }));

    let cooldown = DelegatedRefreshCooldown::default();
    let coordinator = DelegatedRefreshCoordinator::new(refresher.clone(), cooldown.clone(), None)
        .with_observation_ticks(vec![Duration::from_millis(5)]);

    let call = make_call(FetchInteraction::UserInitiated);
    let res = coordinator.refresh("prof-fail", &decl, &call, true).await;

    assert_eq!(res, DelegatedCoordinatorResult::FailedOrUnchanged);
    let entry = cooldown
        .get_entry("prof-fail")
        .expect("cooldown entry recorded");
    assert_eq!(entry.interval(), Duration::from_secs(20));
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthDelegatedRefreshCoordinatorTests.swift:337
#[tokio::test]
async fn test_concurrent_attempts_join_in_flight() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cred_path = temp.path().join("creds.json");
    std::fs::write(&cred_path, b"initial creds").expect("write creds");
    let decl = CredentialDecl::for_test(&cred_path);

    let cred_path_clone = cred_path.clone();
    let refresher = Arc::new(MockRefresher::new(true).with_hook(move |_| {
        let cred_path = cred_path_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            std::fs::write(&cred_path, b"refreshed tokens").unwrap();
            Ok(())
        }
    }));

    let coordinator = Arc::new(
        DelegatedRefreshCoordinator::new(
            refresher.clone(),
            DelegatedRefreshCooldown::default(),
            None,
        )
        .with_observation_ticks(vec![Duration::from_millis(10)]),
    );

    let coord1 = coordinator.clone();
    let decl1 = decl.clone();
    let task1 = tokio::spawn(async move {
        coord1
            .refresh(
                "prof-shared",
                &decl1,
                &make_call(FetchInteraction::UserInitiated),
                true,
            )
            .await
    });

    let coord2 = coordinator.clone();
    let decl2 = decl.clone();
    let task2 = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        coord2
            .refresh(
                "prof-shared",
                &decl2,
                &make_call(FetchInteraction::UserInitiated),
                true,
            )
            .await
    });

    let (res1, res2) = tokio::join!(task1, task2);
    assert_eq!(res1.unwrap(), DelegatedCoordinatorResult::Success);
    assert_eq!(res2.unwrap(), DelegatedCoordinatorResult::Success);

    // Exactly 1 touch run for both concurrent callers
    assert_eq!(refresher.touch_count.load(Ordering::SeqCst), 1);
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthDelegatedRefreshCoordinatorTests.swift:438
#[tokio::test]
async fn test_user_action_retries_after_joining_failed_background_attempt() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cred_path = temp.path().join("creds.json");
    std::fs::write(&cred_path, b"initial creds").expect("write creds");
    let decl = CredentialDecl::for_test(&cred_path);

    let cred_path_clone = cred_path.clone();
    let refresher = Arc::new(MockRefresher::new(true).with_hook(move |attempt| {
        let cred_path = cred_path_clone.clone();
        async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            if attempt == 0 {
                // First attempt (background) fails
                Err(DelegatedRefreshError::Failed(
                    "transient failure".to_string(),
                ))
            } else {
                // Second attempt (user retry) succeeds
                std::fs::write(&cred_path, b"fresh credentials").unwrap();
                Ok(())
            }
        }
    }));

    let coordinator = Arc::new(
        DelegatedRefreshCoordinator::new(
            refresher.clone(),
            DelegatedRefreshCooldown::default(),
            None,
        )
        .with_observation_ticks(vec![Duration::from_millis(10)]),
    );

    let coord1 = coordinator.clone();
    let decl1 = decl.clone();
    // Background task starts first
    let bg_task = tokio::spawn(async move {
        coord1
            .refresh(
                "prof-retry",
                &decl1,
                &make_call(FetchInteraction::Background),
                true,
            )
            .await
    });

    let coord2 = coordinator.clone();
    let decl2 = decl.clone();
    // User task joins while background is in flight
    let user_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        coord2
            .refresh(
                "prof-retry",
                &decl2,
                &make_call(FetchInteraction::UserInitiated),
                true,
            )
            .await
    });

    let (bg_res, user_res) = tokio::join!(bg_task, user_task);
    assert_eq!(
        bg_res.unwrap(),
        DelegatedCoordinatorResult::FailedOrUnchanged
    );
    assert_eq!(user_res.unwrap(), DelegatedCoordinatorResult::Success);

    // Exactly 2 touches: background + user retry
    assert_eq!(refresher.touch_count.load(Ordering::SeqCst), 2);
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthDelegatedRefreshUnreadableResultTests.swift:85
#[tokio::test]
async fn test_unreadable_refresh_result_reports_terminal_outcome() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cred_path = temp.path().join("creds.json");
    std::fs::write(&cred_path, b"initial creds").expect("write creds");
    let decl = CredentialDecl::for_test(&cred_path);

    let cred_path_clone = cred_path.clone();
    let refresher = Arc::new(MockRefresher::new(true).with_hook(move |_| {
        let cred_path = cred_path_clone.clone();
        async move {
            // Replace file with a directory -> Unreadable!
            std::fs::remove_file(&cred_path).unwrap();
            std::fs::create_dir(&cred_path).unwrap();
            Ok(())
        }
    }));

    let cooldown = DelegatedRefreshCooldown::default();
    let coordinator = DelegatedRefreshCoordinator::new(refresher.clone(), cooldown.clone(), None)
        .with_observation_ticks(vec![Duration::from_millis(5)]);

    let call = make_call(FetchInteraction::UserInitiated);
    let res = coordinator
        .refresh("prof-unreadable", &decl, &call, true)
        .await;

    assert_eq!(res, DelegatedCoordinatorResult::UnreadableResult);

    // Terminal outcome sets 5-minute cooldown (300 s)
    let entry = cooldown
        .get_entry("prof-unreadable")
        .expect("cooldown entry");
    assert_eq!(entry.interval(), Duration::from_secs(300));
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthDelegatedRefreshUnreadableResultTests.swift:102
#[tokio::test]
async fn test_failed_touch_stays_retryable_at_20_seconds() {
    let cooldown = DelegatedRefreshCooldown::default();
    cooldown.finalize("prof-failed", DelegatedRefreshOutcome::FailedOrUnchanged);

    let entry = cooldown
        .get_entry("prof-failed")
        .expect("cooldown recorded");
    assert_eq!(entry.interval(), Duration::from_secs(20));
}

// CodexBar: Tests/CodexBarTests/ClaudeOAuthDelegatedRefreshUnreadableResultTests.swift:127
#[tokio::test]
async fn test_unchanged_fingerprint_stays_retryable_at_20_seconds() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cred_path = temp.path().join("creds.json");
    std::fs::write(&cred_path, b"static credentials").expect("write creds");
    let decl = CredentialDecl::for_test(&cred_path);

    // Touch does nothing to the file -> unchanged
    let refresher = Arc::new(MockRefresher::new(true));
    let cooldown = DelegatedRefreshCooldown::default();
    let coordinator = DelegatedRefreshCoordinator::new(refresher.clone(), cooldown.clone(), None)
        .with_observation_ticks(vec![Duration::from_millis(5)]);

    let call = make_call(FetchInteraction::UserInitiated);
    let res = coordinator
        .refresh("prof-unchanged", &decl, &call, true)
        .await;

    assert_eq!(res, DelegatedCoordinatorResult::FailedOrUnchanged);

    let entry = cooldown
        .get_entry("prof-unchanged")
        .expect("cooldown recorded");
    assert_eq!(entry.interval(), Duration::from_secs(20));
}
