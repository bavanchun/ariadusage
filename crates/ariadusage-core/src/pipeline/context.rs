// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderFetchPlan.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::collections::BTreeMap;
use std::fmt;

use ariadusage_protocol::ProviderId;
use ariadusage_protocol::secret::SecretString;
use tokio_util::sync::CancellationToken;

use crate::providers::SourceMode;

/// Host runtime driving the fetch pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FetchRuntime {
    Daemon,
    Cli,
}

/// Mode of interaction triggering the fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FetchInteraction {
    Background,
    UserInitiated,
}

/// Lifecycle phase of the provider fetch loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FetchPhase {
    Startup,
    Regular,
}

/// Execution context passed to strategies during a provider fetch.
#[derive(Clone)]
pub struct FetchContext {
    pub provider: ProviderId,
    pub runtime: FetchRuntime,
    pub interaction: FetchInteraction,
    pub phase: FetchPhase,
    pub request_id: String,
    pub source_mode: SourceMode,
    pub env: BTreeMap<String, SecretString>,
    pub include_credits: bool,
    pub selected_token_account: Option<String>,
    pub cancel: CancellationToken,
}

impl fmt::Debug for FetchContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FetchContext")
            .field("provider", &self.provider)
            .field("runtime", &self.runtime)
            .field("interaction", &self.interaction)
            .field("phase", &self.phase)
            .field("request_id", &self.request_id)
            .field("source_mode", &self.source_mode)
            .field("env", &self.env)
            .field("include_credits", &self.include_credits)
            .field("selected_token_account", &self.selected_token_account)
            .field("cancelled", &self.cancel.is_cancelled())
            .finish()
    }
}
