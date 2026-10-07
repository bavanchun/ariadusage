// Ported from CodexBar Sources/CodexBarCore/ProviderTransportError.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use std::error::Error;
use std::io;

use ariadusage_core::pipeline::TransportClass;

/// A marker used by injected resolvers so DNS failures retain their typed class.
#[derive(Debug, thiserror::Error)]
#[error("DNS resolution failed")]
pub struct DnsFailure;

/// Maps a reqwest error to the transport identity used by the core pipeline.
pub fn transport_class(error: &reqwest::Error) -> Option<TransportClass> {
    if error.is_timeout() {
        return Some(TransportClass::Timeout);
    }

    let mut source = Some(error as &(dyn Error + 'static));
    while let Some(current) = source {
        if current.is::<DnsFailure>() {
            return Some(TransportClass::Dns);
        }
        if let Some(io_error) = current.downcast_ref::<io::Error>()
            && let Some(class) = io_transport_class(io_error.kind())
        {
            return Some(class);
        }
        source = current.source();
    }

    error.is_connect().then_some(TransportClass::CannotConnect)
}

fn io_transport_class(kind: io::ErrorKind) -> Option<TransportClass> {
    match kind {
        io::ErrorKind::ConnectionRefused => Some(TransportClass::CannotConnect),
        io::ErrorKind::NetworkUnreachable | io::ErrorKind::HostUnreachable => {
            Some(TransportClass::NotConnected)
        }
        io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::UnexpectedEof
        | io::ErrorKind::BrokenPipe => Some(TransportClass::ConnectionLost),
        io::ErrorKind::NotFound | io::ErrorKind::AddrNotAvailable => {
            Some(TransportClass::CannotFindHost)
        }
        _ => None,
    }
}

#[cfg(feature = "test-hooks")]
pub fn test_io_transport_class(kind: io::ErrorKind) -> Option<TransportClass> {
    io_transport_class(kind)
}
