// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderCandidateRetryRunner.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt

use std::future::Future;
use thiserror::Error;

/// Error occurring when candidate retry execution fails.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CandidateRetryError<E> {
    #[error("no candidates")]
    NoCandidates,
    #[error(transparent)]
    Attempt(E),
}

/// Executes an operation across an ordered sequence of candidate credentials or endpoints.
pub struct CandidateRetryRunner;

impl CandidateRetryRunner {
    /// Attempts candidates in order. Candidates are pulled one at a time, and the
    /// next one is pulled only after `should_retry` accepts the previous failure,
    /// so a lazy sequence never loads a candidate it does not need.
    pub async fn run<Candidates, Output, E, F, Fut, ShouldRetry, OnRetry>(
        candidates: Candidates,
        mut should_retry: ShouldRetry,
        mut on_retry: OnRetry,
        mut attempt: F,
    ) -> Result<Output, CandidateRetryError<E>>
    where
        Candidates: IntoIterator,
        F: FnMut(&Candidates::Item) -> Fut,
        Fut: Future<Output = Result<Output, E>>,
        ShouldRetry: FnMut(&E) -> bool,
        OnRetry: FnMut(&Candidates::Item, &E),
    {
        let mut candidates = candidates.into_iter();
        let Some(mut candidate) = candidates.next() else {
            return Err(CandidateRetryError::NoCandidates);
        };

        loop {
            match attempt(&candidate).await {
                Ok(output) => return Ok(output),
                Err(err) => {
                    if !should_retry(&err) {
                        return Err(CandidateRetryError::Attempt(err));
                    }
                    let Some(next) = candidates.next() else {
                        return Err(CandidateRetryError::Attempt(err));
                    };
                    on_retry(&candidate, &err);
                    candidate = next;
                }
            }
        }
    }
}
