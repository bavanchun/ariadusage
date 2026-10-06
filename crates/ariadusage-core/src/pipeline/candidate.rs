// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderCandidateRetryRunner.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

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

/// Executes an operation across an ordered list of candidate credentials or endpoints.
pub struct CandidateRetryRunner;

impl CandidateRetryRunner {
    pub async fn run<Candidate, Output, E, F, Fut, ShouldRetry, OnRetry>(
        candidates: &[Candidate],
        mut should_retry: ShouldRetry,
        mut on_retry: OnRetry,
        mut attempt: F,
    ) -> Result<Output, CandidateRetryError<E>>
    where
        F: FnMut(&Candidate) -> Fut,
        Fut: Future<Output = Result<Output, E>>,
        ShouldRetry: FnMut(&E) -> bool,
        OnRetry: FnMut(&Candidate, &E),
    {
        if candidates.is_empty() {
            return Err(CandidateRetryError::NoCandidates);
        }

        let total = candidates.len();
        for (index, candidate) in candidates.iter().enumerate() {
            match attempt(candidate).await {
                Ok(output) => return Ok(output),
                Err(err) => {
                    let has_more = index + 1 < total;
                    if has_more && should_retry(&err) {
                        on_retry(candidate, &err);
                    } else {
                        return Err(CandidateRetryError::Attempt(err));
                    }
                }
            }
        }

        Err(CandidateRetryError::NoCandidates)
    }
}
