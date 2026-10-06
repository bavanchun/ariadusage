// Ported from CodexBar TestsLinux/UsageSnapshotAccountLabelTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::{ProviderIdentitySnapshot, RateWindow, UsageSnapshot};
use ariadusage_protocol::{Confidence, ProviderId};
use jiff::Timestamp;

fn make_snapshot(email: Option<&str>) -> UsageSnapshot {
    let zai = ProviderId::new("zai").expect("zai id");
    let identity = ProviderIdentitySnapshot::new(
        Some(zai),
        email.map(str::to_string),
        Some("Fixture Organization".to_string()),
        Some("Fixture Plan".to_string()),
        Some("fixture-account-id".to_string()),
    );

    UsageSnapshot::new(
        Some(RateWindow::new(25.0, Some(300), None, None, None, false).unwrap()),
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        Timestamp::from_second(1_700_000_000).unwrap(),
        Some(identity),
        Confidence::Unknown,
    )
    .unwrap()
}

#[test]
fn fallback_label_preserves_the_provider_identity() {
    let zai = ProviderId::new("zai").expect("zai id");
    let cases = [
        (None, "Team Account"),
        (Some(""), "Team Account"),
        (Some(" \n "), "Team Account"),
        (Some(" fetched@example.com "), "fetched@example.com"),
    ];

    for (email, expected) in cases {
        let snapshot = make_snapshot(email);
        let labeled = snapshot.with_account_label(" Team Account ", &zai);

        assert_eq!(
            labeled
                .identity
                .as_ref()
                .and_then(|i| i.account_email.as_deref()),
            Some(expected)
        );
        assert_eq!(
            labeled
                .identity
                .as_ref()
                .and_then(|i| i.account_id.as_deref()),
            Some("fixture-account-id")
        );
        assert_eq!(
            labeled
                .identity
                .as_ref()
                .and_then(|i| i.account_organization.as_deref()),
            Some("Fixture Organization")
        );
        assert_eq!(
            labeled
                .identity
                .as_ref()
                .and_then(|i| i.login_method.as_deref()),
            Some("Fixture Plan")
        );
        assert_eq!(labeled.primary.as_ref().map(|p| p.used_percent), Some(25.0));
        assert_eq!(labeled.updated_at, snapshot.updated_at);
    }
}

#[test]
fn empty_labels_leave_the_original_identity_untouched() {
    let zai = ProviderId::new("zai").expect("zai id");
    let cases = ["", " \n "];

    for label in cases {
        let snapshot = make_snapshot(Some(" fetched@example.com "));
        let labeled = snapshot.with_account_label(label, &zai);

        assert_eq!(
            labeled
                .identity
                .as_ref()
                .and_then(|i| i.account_email.as_deref()),
            Some(" fetched@example.com ")
        );
        assert_eq!(
            labeled
                .identity
                .as_ref()
                .and_then(|i| i.account_id.as_deref()),
            Some("fixture-account-id")
        );
    }
}

#[test]
fn fallback_labels_never_borrow_another_provider_identity() {
    let synthetic = ProviderId::new("synthetic").expect("synthetic id");
    let snapshot = make_snapshot(Some("fetched@example.com"));
    let labeled = snapshot.with_account_label("Other Account", &synthetic);

    assert_eq!(
        labeled
            .identity
            .as_ref()
            .and_then(|i| i.provider_id.as_ref()),
        Some(&synthetic)
    );
    assert_eq!(
        labeled
            .identity
            .as_ref()
            .and_then(|i| i.account_email.as_deref()),
        Some("Other Account")
    );
    assert_eq!(
        labeled
            .identity
            .as_ref()
            .and_then(|i| i.account_id.as_deref()),
        None
    );
    assert_eq!(
        labeled
            .identity
            .as_ref()
            .and_then(|i| i.account_organization.as_deref()),
        None
    );
    assert_eq!(
        labeled
            .identity
            .as_ref()
            .and_then(|i| i.login_method.as_deref()),
        None
    );
}
