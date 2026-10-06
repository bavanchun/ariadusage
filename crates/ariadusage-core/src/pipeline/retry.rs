// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderFetchPlan.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::pipeline::error::FetchError;
use crate::pipeline::outcome::FetchResult;

/// Pinned, heap-allocated future suitable for dynamic dispatch without extra crates.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Injected time sleeper for asynchronous retry delays.
pub trait Sleeper: Send + Sync {
    fn sleep<'a>(&'a self, duration: Duration) -> BoxFuture<'a, ()>;
}

/// Runs a single operation with delayed retry support.
///
/// Exactly one retry is attempted, only when the initial attempt fails with a
/// classified error specifying a valid `retry_after`. Non-classified errors, errors
/// without retry delay, or failures in the second attempt propagate immediately.
pub async fn run_delayed_retry<F, Fut>(
    cancel: &CancellationToken,
    sleeper: &dyn Sleeper,
    mut op: F,
) -> Result<FetchResult, FetchError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<FetchResult, FetchError>>,
{
    let first = cancel.run_until_cancelled(op()).await;
    let res = match first {
        Some(r) => r,
        None => return Err(FetchError::Cancelled),
    };

    match res {
        Ok(val) => Ok(val),
        Err(err) => {
            let retry_duration = match &err {
                FetchError::Classified(classified) => classified.retry_after(),
                _ => None,
            };

            let duration = match retry_duration {
                Some(d) => d,
                None => return Err(err),
            };

            if cancel.is_cancelled() {
                return Err(FetchError::Cancelled);
            }

            if cancel
                .run_until_cancelled(sleeper.sleep(duration))
                .await
                .is_none()
            {
                return Err(FetchError::Cancelled);
            }

            if cancel.is_cancelled() {
                return Err(FetchError::Cancelled);
            }

            match cancel.run_until_cancelled(op()).await {
                Some(r) => r,
                None => Err(FetchError::Cancelled),
            }
        }
    }
}
