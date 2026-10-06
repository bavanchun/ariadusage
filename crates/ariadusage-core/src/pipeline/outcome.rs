// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderFetchPlan.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_protocol::ProviderId;
use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::usage::{ProviderErrorCategory, ProviderErrorKind};

use crate::model::UsageSnapshot;
use crate::pipeline::context::FetchContext;
use crate::pipeline::diagnostics::kind_label;
use crate::pipeline::error::FetchError;
use crate::pipeline::retry::{Sleeper, run_delayed_retry};
use crate::pipeline::strategy::ProviderPlan;

/// Successful result returned by a winning strategy.
#[derive(Debug, Clone, PartialEq)]
pub struct FetchResult {
    pub usage: UsageSnapshot,
    pub source_label: String,
    pub strategy_id: String,
    pub strategy_kind: SourceKind,
    pub diagnostic: Option<String>,
}

impl FetchResult {
    pub fn new(
        usage: UsageSnapshot,
        source_label: impl Into<String>,
        strategy_id: impl Into<String>,
        strategy_kind: SourceKind,
    ) -> Self {
        Self {
            usage,
            source_label: source_label.into(),
            strategy_id: strategy_id.into(),
            strategy_kind,
            diagnostic: None,
        }
    }

    pub fn with_diagnostic(mut self, diagnostic: impl Into<String>) -> Self {
        self.diagnostic = Some(diagnostic.into());
        self
    }
}

/// Structured summary of a strategy failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptFailure {
    pub kind: ProviderErrorKind,
    pub category: ProviderErrorCategory,
}

/// Status outcome of a strategy attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    Succeeded,
    Skipped,
    Failed,
}

/// Record of one strategy evaluated during the fetch pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchAttempt {
    pub strategy_id: String,
    pub kind: SourceKind,
    pub was_available: bool,
    pub failure: Option<AttemptFailure>,
}

impl FetchAttempt {
    pub fn outcome(&self) -> AttemptOutcome {
        if !self.was_available {
            AttemptOutcome::Skipped
        } else if self.failure.is_none() {
            AttemptOutcome::Succeeded
        } else {
            AttemptOutcome::Failed
        }
    }
}

/// Overall outcome of a provider fetch execution.
#[derive(Debug)]
pub struct FetchOutcome {
    pub result: Result<FetchResult, FetchError>,
    pub attempts: Vec<FetchAttempt>,
    pub failure_summary: Option<String>,
}

/// Executes the full provider fetch pipeline according to ordering and cancellation rules.
pub async fn fetch_outcome(
    provider: &ProviderId,
    plan: &dyn ProviderPlan,
    cx: &FetchContext,
    sleeper: &dyn Sleeper,
) -> FetchOutcome {
    // 1. strategies resolve before the first cancellation check
    let strategies = plan.resolve_strategies(cx).await;
    let mut attempts = Vec::with_capacity(strategies.len());
    let mut last_available_error: Option<FetchError> = None;

    if cx.cancel.is_cancelled() {
        return FetchOutcome {
            result: Err(FetchError::Cancelled),
            attempts,
            failure_summary: None,
        };
    }

    for strategy in &strategies {
        // 2. cancellation checked before each strategy
        if cx.cancel.is_cancelled() {
            return FetchOutcome {
                result: Err(FetchError::Cancelled),
                attempts,
                failure_summary: None,
            };
        }

        let is_available_fut = strategy.is_available(cx);
        let available = match cx.cancel.run_until_cancelled(is_available_fut).await {
            Some(avail) => avail,
            None => {
                // Cancelled during is_available: no attempt recorded
                return FetchOutcome {
                    result: Err(FetchError::Cancelled),
                    attempts,
                    failure_summary: None,
                };
            }
        };

        // Cancellation checked after is_available (no attempt recorded)
        if cx.cancel.is_cancelled() {
            return FetchOutcome {
                result: Err(FetchError::Cancelled),
                attempts,
                failure_summary: None,
            };
        }

        if !available {
            // 3. a skipped strategy records was_available = false
            attempts.push(FetchAttempt {
                strategy_id: strategy.id().to_string(),
                kind: strategy.kind(),
                was_available: false,
                failure: None,
            });
            continue;
        }

        let fetch_res = run_delayed_retry(&cx.cancel, sleeper, || strategy.fetch(cx)).await;

        match fetch_res {
            Ok(mut result) => {
                // 4. after fetch returns Ok, if cx.cancel.is_cancelled(), record a failed attempt and return Cancelled
                if cx.cancel.is_cancelled() {
                    attempts.push(FetchAttempt {
                        strategy_id: strategy.id().to_string(),
                        kind: strategy.kind(),
                        was_available: true,
                        failure: Some(AttemptFailure {
                            kind: ProviderErrorKind::Unknown,
                            category: ProviderErrorCategory::Unknown,
                        }),
                    });
                    return FetchOutcome {
                        result: Err(FetchError::Cancelled),
                        attempts,
                        failure_summary: None,
                    };
                }

                // 7. a diagnostic is attached only on fallback success and never overwrites one
                if result.diagnostic.is_none()
                    && let Some(diag) = last_available_error
                        .as_ref()
                        .and_then(|prior| strategy.diagnostic_for_prior_failure(prior))
                {
                    result = result.with_diagnostic(diag);
                }

                attempts.push(FetchAttempt {
                    strategy_id: strategy.id().to_string(),
                    kind: strategy.kind(),
                    was_available: true,
                    failure: None,
                });

                return FetchOutcome {
                    result: Ok(result),
                    attempts,
                    failure_summary: None,
                };
            }
            Err(err) => {
                // 5. after fetch returns Err, the error is resolved and recorded, then is_cancelled() or an Err(Cancelled) returns Cancelled before should_fallback is asked
                // 6. fallback resolver and should_fallback both see the raw error; attempt records the raw error's kind and category; surfaced error is resolved one
                let resolved_error = plan.resolve_fallback_error(last_available_error, &err);
                last_available_error = Some(resolved_error);

                let (kind, category) = match &err {
                    FetchError::Classified(c) => (c.kind, c.category()),
                    FetchError::Cancelled => {
                        (ProviderErrorKind::Unknown, ProviderErrorCategory::Unknown)
                    }
                    FetchError::NoAvailableStrategy(_) => {
                        (ProviderErrorKind::Unknown, ProviderErrorCategory::Unknown)
                    }
                };

                attempts.push(FetchAttempt {
                    strategy_id: strategy.id().to_string(),
                    kind: strategy.kind(),
                    was_available: true,
                    failure: Some(AttemptFailure { kind, category }),
                });

                if cx.cancel.is_cancelled() || matches!(err, FetchError::Cancelled) {
                    return FetchOutcome {
                        result: Err(FetchError::Cancelled),
                        attempts,
                        failure_summary: None,
                    };
                }

                if strategy.should_fallback(&err, cx) {
                    continue;
                }

                let surfaced = last_available_error.unwrap_or(err);
                let failure_summary = build_failure_summary(&attempts);
                return FetchOutcome {
                    result: Err(surfaced),
                    attempts,
                    failure_summary,
                };
            }
        }
    }

    // 8. an empty or all-skipped list yields NoAvailableStrategy
    let error =
        last_available_error.unwrap_or_else(|| FetchError::NoAvailableStrategy(provider.clone()));
    let failure_summary = build_failure_summary(&attempts);
    FetchOutcome {
        result: Err(error),
        attempts,
        failure_summary,
    }
}

/// Builds the single-line debug summary of all attempted sources for logging.
pub fn build_failure_summary(attempts: &[FetchAttempt]) -> Option<String> {
    if attempts.is_empty() {
        return None;
    }
    let parts: Vec<String> = attempts
        .iter()
        .map(|a| {
            let detail = match a.outcome() {
                AttemptOutcome::Failed => {
                    let cat = a
                        .failure
                        .as_ref()
                        .map(|f| category_label(f.category))
                        .unwrap_or("unknown");
                    format!("failed: {cat}")
                }
                AttemptOutcome::Skipped => "skipped: unavailable".to_string(),
                AttemptOutcome::Succeeded => "succeeded".to_string(),
            };
            format!("{} ({}): {}", a.strategy_id, kind_label(a.kind), detail)
        })
        .collect();
    Some(parts.join(" -> "))
}

fn category_label(cat: ProviderErrorCategory) -> &'static str {
    match cat {
        ProviderErrorCategory::Auth => "auth",
        ProviderErrorCategory::Api => "api",
        ProviderErrorCategory::Parse => "parse",
        ProviderErrorCategory::Network => "network",
        ProviderErrorCategory::Configuration => "configuration",
        ProviderErrorCategory::Unknown => "unknown",
    }
}
