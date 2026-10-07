pub mod candidate;
pub mod context;
pub mod diagnostics;
pub mod env;
pub mod error;
pub mod outcome;
pub mod retry;
pub mod strategy;

pub use candidate::{CandidateRetryError, CandidateRetryRunner};
pub use context::{FetchContext, FetchInteraction, FetchPhase, FetchRuntime};
pub use diagnostics::{
    DiagnosticError, DiagnosticFetchAttempt, category_safe_description, kind_label,
};
pub use env::{
    AccountTokenProjection, EnvPrecedence, EnvProjection, ProviderEnvDescriptor,
    ProviderEnvironmentResolver,
};
pub use error::{
    ClassifiedError, FetchError, TransportClass, error_kind_to_category, normalize_retry_after,
};
pub use outcome::{
    AttemptFailure, AttemptOutcome, FetchAttempt, FetchOutcome, FetchResult, build_failure_summary,
    fetch_outcome,
};
pub use retry::{BoxFuture, Sleeper, run_delayed_retry};
pub use strategy::{FetchStrategy, ProviderPlan};
