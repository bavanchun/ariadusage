// Ported from CodexBar Tests/CodexBarTests/UsageStoreCoverageTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::pipeline::{ClassifiedError, FetchError, TransportClass};
use ariadusage_core::refresh::{is_cancellation, is_preservable_error, is_preservable_transport};
use ariadusage_protocol::ProviderId;
use ariadusage_protocol::usage::ProviderErrorKind;

#[test]
fn is_preservable_network_transport_error_classifies_transport_failures_correctly() {
    // Ported from UsageStoreCoverageTests.swift:925-942
    assert!(is_preservable_transport(TransportClass::CannotFindHost));
    assert!(is_preservable_transport(TransportClass::CannotConnect));
    assert!(is_preservable_transport(TransportClass::Dns));
    assert!(is_preservable_transport(TransportClass::Timeout));
    assert!(is_preservable_transport(TransportClass::NotConnected));
    assert!(is_preservable_transport(TransportClass::ConnectionLost));
    assert!(is_preservable_transport(TransportClass::Cancelled));

    // Non-transport / unclassified errors are not preservable
    let non_transport =
        ClassifiedError::new(ProviderErrorKind::AuthenticationExpired, "auth failed");
    let err = FetchError::Classified(non_transport);
    assert!(!is_preservable_error(&err));
}

#[test]
fn fetch_error_preservable_and_cancellation_semantics() {
    // Cancelled variant is preservable and counts as cancellation
    let cancelled = FetchError::Cancelled;
    assert!(cancelled.is_preservable());
    assert!(cancelled.is_cancellation());
    assert!(is_preservable_error(&cancelled));
    assert!(is_cancellation(&cancelled));

    // Transport classified as Cancelled
    let transport_cancelled = FetchError::Classified(
        ClassifiedError::new(ProviderErrorKind::NetworkFailure, "request cancelled")
            .with_transport(TransportClass::Cancelled),
    );
    assert!(transport_cancelled.is_preservable());
    assert!(transport_cancelled.is_cancellation());

    // Timeout is preservable but NOT cancellation
    let timeout = FetchError::Classified(
        ClassifiedError::new(ProviderErrorKind::NetworkFailure, "gateway timed out")
            .with_transport(TransportClass::Timeout),
    );
    assert!(timeout.is_preservable());
    assert!(!timeout.is_cancellation());
    assert!(is_preservable_error(&timeout));
    assert!(!is_cancellation(&timeout));

    // NoAvailableStrategy is neither preservable nor cancellation
    let no_strategy = FetchError::NoAvailableStrategy(ProviderId::new("claude").unwrap());
    assert!(!no_strategy.is_preservable());
    assert!(!no_strategy.is_cancellation());
}
