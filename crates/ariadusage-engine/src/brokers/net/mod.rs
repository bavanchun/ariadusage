mod body;
mod classify;
mod client;
mod error;
mod origin;
mod override_validator;
mod redirect;

pub use body::NetBody;
#[cfg(feature = "test-hooks")]
pub use classify::test_io_transport_class;
pub use classify::{DnsFailure, transport_class};
pub use client::{NetBroker, NetConfig, NetRequest, NetResponse, RedactedHeaders};
pub use error::NetError;
pub use origin::{DeclaredOrigins, Origin, OriginError};
pub use override_validator::{
    OverrideError, OverridePolicy, is_loopback_host, is_private_network_host, validate_override,
};
pub use redirect::same_origin_https;

#[cfg(feature = "test-hooks")]
pub use client::test_credential_header_value;
