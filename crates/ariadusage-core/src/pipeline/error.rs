// Ported from CodexBar Sources/CodexBarCore/Providers/ProviderFetchPlan.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::error::Error;
use std::fmt;
use std::time::Duration;

use ariadusage_protocol::ProviderId;
use ariadusage_protocol::usage::{ProviderErrorCategory, ProviderErrorKind};

use crate::projection::ProjectedError;

/// Provisional transport-level failure classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransportClass {
    Timeout,
    ConnectionLost,
    NotConnected,
    CannotFindHost,
    CannotConnect,
    Dns,
    Cancelled,
}

impl TransportClass {
    pub const fn is_preservable(&self) -> bool {
        true
    }

    pub const fn is_startup_retryable(&self) -> bool {
        !matches!(self, Self::Cancelled)
    }
}

/// A classified provider fetch failure.
pub struct ClassifiedError {
    pub kind: ProviderErrorKind,
    pub message: String,
    retry_after: Option<Duration>,
    pub transport: Option<TransportClass>,
    pub source: Option<Box<dyn Error + Send + Sync>>,
}

impl ClassifiedError {
    pub const MAXIMUM_RETRY_AFTER_SECONDS: f64 = 10.0;

    pub fn new(kind: ProviderErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after: None,
            transport: None,
            source: None,
        }
    }

    pub fn with_retry_after_secs(mut self, seconds: f64) -> Self {
        self.retry_after = normalize_retry_after(seconds);
        self
    }

    pub fn with_transport(mut self, transport: TransportClass) -> Self {
        self.transport = Some(transport);
        self
    }

    pub fn with_source(mut self, source: Box<dyn Error + Send + Sync>) -> Self {
        self.source = Some(source);
        self
    }

    pub fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }

    pub fn category(&self) -> ProviderErrorCategory {
        error_kind_to_category(self.kind)
    }
}

/// Normalizes retry delay: NaN and negative map to None, finite >= 0 is capped at 10 seconds.
pub fn normalize_retry_after(seconds: f64) -> Option<Duration> {
    if !seconds.is_finite() || seconds < 0.0 {
        None
    } else {
        let clamped = seconds.min(ClassifiedError::MAXIMUM_RETRY_AFTER_SECONDS);
        Some(Duration::from_secs_f64(clamped))
    }
}

/// Maps a classified error kind to high-level diagnostic category per CodexBar rules.
pub fn error_kind_to_category(kind: ProviderErrorKind) -> ProviderErrorCategory {
    match kind {
        ProviderErrorKind::AuthenticationExpired
        | ProviderErrorKind::MissingCredential
        | ProviderErrorKind::PermissionDenied => ProviderErrorCategory::Auth,

        ProviderErrorKind::RateLimited
        | ProviderErrorKind::ProviderUnavailable
        | ProviderErrorKind::ApiFailure => ProviderErrorCategory::Api,

        ProviderErrorKind::ParseFailure => ProviderErrorCategory::Parse,

        ProviderErrorKind::NetworkFailure => ProviderErrorCategory::Network,

        ProviderErrorKind::Unknown => ProviderErrorCategory::Unknown,
    }
}

impl fmt::Display for ClassifiedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "classified error: kind={:?}, category={:?}, transport={:?}",
            self.kind,
            self.category(),
            self.transport
        )
    }
}

impl fmt::Debug for ClassifiedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClassifiedError")
            .field("kind", &self.kind)
            .field("category", &self.category())
            .field("transport", &self.transport)
            .field("retry_after", &self.retry_after)
            .finish()
    }
}

impl Error for ClassifiedError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_ref().map(|s| &**s as &(dyn Error + 'static))
    }
}

/// Overall provider fetch failure type.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("fetch cancelled")]
    Cancelled,

    #[error("no available fetch strategy for {0}")]
    NoAvailableStrategy(ProviderId),

    #[error(transparent)]
    Classified(#[from] ClassifiedError),
}

impl FetchError {
    pub fn clone_fallback(&self) -> Self {
        match self {
            Self::Cancelled => Self::Cancelled,
            Self::NoAvailableStrategy(p) => Self::NoAvailableStrategy(p.clone()),
            Self::Classified(c) => Self::Classified(ClassifiedError {
                kind: c.kind,
                message: c.message.clone(),
                retry_after: c.retry_after,
                transport: c.transport,
                source: None,
            }),
        }
    }
}

impl From<&ClassifiedError> for ProjectedError {
    fn from(err: &ClassifiedError) -> Self {
        let mut proj = ProjectedError::new(err.kind, err.category());
        if let Some(d) = err.retry_after() {
            proj = proj.with_retry(d.as_secs());
        }
        proj
    }
}

impl From<ClassifiedError> for ProjectedError {
    fn from(err: ClassifiedError) -> Self {
        ProjectedError::from(&err)
    }
}
