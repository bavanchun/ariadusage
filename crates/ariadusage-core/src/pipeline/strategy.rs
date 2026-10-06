// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderFetchPlan.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_protocol::metric::SourceKind;

use crate::pipeline::context::FetchContext;
use crate::pipeline::error::FetchError;
use crate::pipeline::outcome::FetchResult;
use crate::pipeline::retry::BoxFuture;

/// Strategy for fetching usage data from one source mechanism.
pub trait FetchStrategy: Send + Sync {
    /// Unique identifier for this strategy instance (e.g. "claude.cli", "antigravity.oauth").
    fn id(&self) -> &str;

    /// Acquisition kind (CLI, web, OAuth, API token, local probe).
    fn kind(&self) -> SourceKind;

    /// Checks if this strategy is eligible and available in the current environment.
    fn is_available<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, bool>;

    /// Executes the usage fetch for this strategy.
    fn fetch<'a>(&'a self, cx: &'a FetchContext) -> BoxFuture<'a, Result<FetchResult, FetchError>>;

    /// Determines whether the pipeline should attempt subsequent strategies after this failure.
    fn should_fallback(&self, err: &FetchError, cx: &FetchContext) -> bool;

    /// Lets a successful degraded strategy attach a diagnostic explaining earlier failures.
    fn diagnostic_for_prior_failure(&self, _err: &FetchError) -> Option<String> {
        None
    }
}

/// Provider-level plan managing strategy resolution and fallback error synthesis.
pub trait ProviderPlan: Send + Sync {
    /// Resolves the candidate strategy chain for the given fetch context.
    fn resolve_strategies<'a>(
        &'a self,
        cx: &'a FetchContext,
    ) -> BoxFuture<'a, Vec<Box<dyn FetchStrategy>>>;

    /// Synthesizes or routes the surfaced error when an attempt fails.
    /// Default keeps the latest raw failure.
    fn resolve_fallback_error(&self, prev: Option<FetchError>, cur: &FetchError) -> FetchError {
        let _ = prev;
        cur.clone_fallback()
    }
}
