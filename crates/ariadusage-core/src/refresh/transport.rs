// Ported from CodexBar Sources/CodexBar/UsageStore+ClaudeHistoryFallback.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use crate::pipeline::{FetchError, TransportClass};

/// Checks whether a transport class represents a preservable transient network failure.
pub const fn is_preservable_transport(transport: TransportClass) -> bool {
    transport.is_preservable()
}

/// Checks whether a fetch failure is preservable.
pub fn is_preservable_error(error: &FetchError) -> bool {
    error.is_preservable()
}

/// Checks whether a fetch error represents a cancellation that suppresses publication.
pub fn is_cancellation(error: &FetchError) -> bool {
    error.is_cancellation()
}
