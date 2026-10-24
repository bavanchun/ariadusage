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
    DiagnosticAuthSummary, DiagnosticBatchExport, DiagnosticError, DiagnosticExport,
    DiagnosticExportBuilder, DiagnosticExportBuilderInput, DiagnosticFetchAttempt,
    DiagnosticPlatform, DiagnosticRateWindow, DiagnosticSettingsSummary, DiagnosticUsageSummary,
    category_label_safe_description, category_safe_description, error_category_label, kind_label,
    redact_diagnostic_text, source_mode_label,
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
