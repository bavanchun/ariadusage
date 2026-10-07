// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderDiagnosticExport.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_protocol::metric::SourceKind;
use ariadusage_protocol::usage::ProviderErrorCategory;

use crate::pipeline::error::{ClassifiedError, FetchError};
use crate::pipeline::outcome::{AttemptOutcome, FetchAttempt};

/// Returns the short diagnostic label for a metric source kind.
pub fn kind_label(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Cli => "cli",
        SourceKind::Web => "web",
        SourceKind::Oauth => "oauth",
        SourceKind::Api => "api",
        SourceKind::LocalProbe => "local",
        SourceKind::Dashboard => "web",
        SourceKind::Unknown => "unknown",
    }
}

/// Returns the fixed safe description for a diagnostic error category.
pub fn category_safe_description(category: ProviderErrorCategory) -> &'static str {
    match category {
        ProviderErrorCategory::Network => "Network error - check your connection",
        ProviderErrorCategory::Auth => "Authentication or setup issue - check provider credentials",
        ProviderErrorCategory::Api => "API error - service returned an unexpected response",
        ProviderErrorCategory::Parse => "Parse error - unexpected response format",
        ProviderErrorCategory::Configuration => {
            "Configuration issue - check provider source and settings"
        }
        ProviderErrorCategory::Unknown => "An unexpected error occurred",
    }
}

/// Diagnostic export representation of a provider failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticError {
    pub category: String,
    pub safe_description: String,
}

impl DiagnosticError {
    pub fn from_error(error: &FetchError, auth_configured: bool) -> Self {
        let cat = match error {
            FetchError::NoAvailableStrategy(_) => {
                if auth_configured {
                    ProviderErrorCategory::Configuration
                } else {
                    ProviderErrorCategory::Auth
                }
            }
            FetchError::Cancelled => ProviderErrorCategory::Unknown,
            FetchError::Classified(classified) => classified.category(),
        };
        let cat_str = match cat {
            ProviderErrorCategory::Auth => "auth",
            ProviderErrorCategory::Api => "api",
            ProviderErrorCategory::Parse => "parse",
            ProviderErrorCategory::Network => "network",
            ProviderErrorCategory::Configuration => "configuration",
            ProviderErrorCategory::Unknown => "unknown",
        };
        Self {
            category: cat_str.to_string(),
            safe_description: category_safe_description(cat).to_string(),
        }
    }

    pub fn from_classified(classified: &ClassifiedError, _auth_configured: bool) -> Self {
        let cat = classified.category();
        let cat_str = match cat {
            ProviderErrorCategory::Auth => "auth",
            ProviderErrorCategory::Api => "api",
            ProviderErrorCategory::Parse => "parse",
            ProviderErrorCategory::Network => "network",
            ProviderErrorCategory::Configuration => "configuration",
            ProviderErrorCategory::Unknown => "unknown",
        };
        Self {
            category: cat_str.to_string(),
            safe_description: category_safe_description(cat).to_string(),
        }
    }
}

/// Diagnostic export representation of an individual strategy attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticFetchAttempt {
    pub strategy_id: String,
    pub kind: String,
    pub was_available: bool,
    pub outcome: String,
    pub error_category: Option<String>,
}

impl DiagnosticFetchAttempt {
    pub fn from_attempt(attempt: &FetchAttempt) -> Self {
        let (outcome, error_category) = match attempt.outcome() {
            AttemptOutcome::Succeeded => ("succeeded".to_string(), None),
            AttemptOutcome::Skipped => ("skipped".to_string(), None),
            AttemptOutcome::Failed => {
                let cat = attempt
                    .failure
                    .as_ref()
                    .map(|f| match f.category {
                        ProviderErrorCategory::Auth => "auth",
                        ProviderErrorCategory::Api => "api",
                        ProviderErrorCategory::Parse => "parse",
                        ProviderErrorCategory::Network => "network",
                        ProviderErrorCategory::Configuration => "configuration",
                        ProviderErrorCategory::Unknown => "unknown",
                    })
                    .unwrap_or("unknown");
                ("failed".to_string(), Some(cat.to_string()))
            }
        };

        Self {
            strategy_id: attempt.strategy_id.clone(),
            kind: kind_label(attempt.kind).to_string(),
            was_available: attempt.was_available,
            outcome,
            error_category,
        }
    }
}
