// Ported from CodexBar Tests/CodexBarTests/ProviderCandidateRetryRunnerTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::sync::Mutex;

use ariadusage_core::pipeline::{CandidateRetryError, CandidateRetryRunner};

#[derive(Debug, PartialEq, Eq)]
enum TestError {
    Retryable(i32),
    NonRetryable(i32),
}

#[tokio::test]
async fn test_retries_then_succeeds() {
    let candidates = vec![1, 2, 3];
    let attempted = Mutex::new(Vec::new());
    let retried = Mutex::new(Vec::new());

    let output = CandidateRetryRunner::run(
        &candidates,
        |error| matches!(error, TestError::Retryable(_)),
        |candidate, _err| {
            retried.lock().unwrap().push(*candidate);
        },
        |candidate| {
            attempted.lock().unwrap().push(*candidate);
            let c = *candidate;
            async move {
                if c == 3 {
                    Ok(c * 10)
                } else {
                    Err(TestError::Retryable(c))
                }
            }
        },
    )
    .await
    .expect("Expected successful output");

    assert_eq!(output, 30);
    assert_eq!(*attempted.lock().unwrap(), vec![1, 2, 3]);
    assert_eq!(*retried.lock().unwrap(), vec![1, 2]);
}

#[tokio::test]
async fn test_non_retryable_fails_immediately() {
    let candidates = vec![1, 2, 3];
    let attempted = Mutex::new(Vec::new());
    let retried = Mutex::new(Vec::new());

    let res: Result<i32, _> = CandidateRetryRunner::run(
        &candidates,
        |error| matches!(error, TestError::Retryable(_)),
        |candidate, _err| {
            retried.lock().unwrap().push(*candidate);
        },
        |candidate| {
            attempted.lock().unwrap().push(*candidate);
            let c = *candidate;
            async move { Err(TestError::NonRetryable(c)) }
        },
    )
    .await;

    match res {
        Err(CandidateRetryError::Attempt(TestError::NonRetryable(1))) => {}
        other => panic!("Expected TestError::NonRetryable(1), got {other:?}"),
    }

    assert_eq!(*attempted.lock().unwrap(), vec![1]);
    assert!(retried.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_exhausted_retryable_throws_last_error() {
    let candidates = vec![1, 2];
    let attempted = Mutex::new(Vec::new());
    let retried = Mutex::new(Vec::new());

    let res: Result<i32, _> = CandidateRetryRunner::run(
        &candidates,
        |error| matches!(error, TestError::Retryable(_)),
        |candidate, _err| {
            retried.lock().unwrap().push(*candidate);
        },
        |candidate| {
            attempted.lock().unwrap().push(*candidate);
            let c = *candidate;
            async move { Err(TestError::Retryable(c)) }
        },
    )
    .await;

    match res {
        Err(CandidateRetryError::Attempt(TestError::Retryable(2))) => {}
        other => panic!("Expected TestError::Retryable(2), got {other:?}"),
    }

    assert_eq!(*attempted.lock().unwrap(), vec![1, 2]);
    assert_eq!(*retried.lock().unwrap(), vec![1]);
}

#[tokio::test]
async fn test_empty_candidates_throws_no_candidates() {
    let candidates: Vec<i32> = vec![];

    let res: Result<i32, CandidateRetryError<TestError>> = CandidateRetryRunner::run(
        &candidates,
        |_error| true,
        |_candidate, _err| {},
        |_candidate| async move { Ok(1) },
    )
    .await;

    match res {
        Err(CandidateRetryError::NoCandidates) => {}
        other => panic!("Expected CandidateRetryError::NoCandidates, got {other:?}"),
    }
}
