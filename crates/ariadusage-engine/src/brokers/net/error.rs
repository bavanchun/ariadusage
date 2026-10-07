use ariadusage_core::pipeline::{ClassifiedError, FetchError, TransportClass};
use ariadusage_protocol::usage::ProviderErrorKind;

use super::override_validator::OverrideError;

/// Failure produced by the Net broker. It never stores request URLs or payloads.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    #[error("request cancelled")]
    Cancelled,
    #[error("request origin is not declared")]
    OriginNotDeclared,
    #[error("endpoint override rejected")]
    OverrideRejected(#[source] OverrideError),
    #[error("response body exceeds the configured cap of {cap} bytes")]
    BodyTooLarge { cap: usize },
    #[error("provider returned HTTP status {status}")]
    Http { status: u16 },
    #[error("network transport failed")]
    Transport(TransportClass),
    #[error("redirect policy rejected the request")]
    Redirect,
    #[error("request is not to an allowed literal loopback address")]
    LoopbackOnly,
    #[error("request URL is invalid for this broker")]
    InvalidUrl,
    #[error("request header is invalid")]
    InvalidHeader,
    #[error("network request failed")]
    Other,
}

impl NetError {
    /// Converts a broker failure to the core fetch error contract without retaining a source.
    pub fn into_fetch_error(self) -> FetchError {
        match self {
            Self::Cancelled => FetchError::Cancelled,
            Self::Transport(transport) => FetchError::Classified(
                ClassifiedError::new(ProviderErrorKind::NetworkFailure, "provider request failed")
                    .with_transport(transport),
            ),
            Self::Http { .. } => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::ApiFailure,
                "provider request failed",
            )),
            Self::OriginNotDeclared
            | Self::OverrideRejected(_)
            | Self::BodyTooLarge { .. }
            | Self::Redirect
            | Self::LoopbackOnly
            | Self::InvalidUrl
            | Self::InvalidHeader
            | Self::Other => FetchError::Classified(ClassifiedError::new(
                ProviderErrorKind::NetworkFailure,
                "provider request failed",
            )),
        }
    }
}
