// Ported from CodexBar Tests/CodexBarTests/CodexbarTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::{ProviderIdentitySnapshot, RateWindow, UsageSnapshot};
use ariadusage_protocol::{Confidence, DetailRow, DetailSection, ProviderId};
use jiff::Timestamp;

#[test]
fn copying_extra_rate_windows_preserves_subscription_dates() {
    let expires_at = Timestamp::from_second(1_810_656_000).expect("timestamp");
    let renews_at = Timestamp::from_second(1_810_569_600).expect("timestamp");
    let updated_at = Timestamp::from_second(1_800_000_000).expect("timestamp");

    let detail_row = DetailRow::new(
        None::<&str>,
        "Individual credits",
        "$12.50",
        None::<&str>,
        None,
        None,
    )
    .unwrap();
    let detail_section = DetailSection::new(None::<&str>, vec![detail_row], None).unwrap();

    let snapshot = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        vec![detail_section],
        Some(expires_at),
        Some(renews_at),
        updated_at,
        None,
        Confidence::Unknown,
    )
    .unwrap();

    let copied = snapshot.clone().with_extra_rate_windows(Some(vec![]));

    assert_eq!(copied.subscription_expires_at, Some(expires_at));
    assert_eq!(copied.subscription_renews_at, Some(renews_at));
    assert_eq!(copied.details, snapshot.details);
}

#[test]
fn subscription_metadata_replacement_switches_renewal_to_expiration() {
    let renewal = Timestamp::from_second(1_787_236_207).expect("timestamp");
    let expiration = Timestamp::from_second(1_787_236_207 + 86400).expect("timestamp");
    let updated_at = Timestamp::from_second(1_787_000_000).expect("timestamp");

    let original = UsageSnapshot::new(
        Some(RateWindow::new(20.0, Some(300), None, None, None, false).unwrap()),
        None,
        None,
        None,
        None,
        vec![],
        None,
        Some(renewal),
        updated_at,
        None,
        Confidence::Unknown,
    )
    .unwrap();

    let replaced = original
        .clone()
        .with_subscription_metadata(Some(expiration), None);

    assert_eq!(replaced.primary, original.primary);
    assert_eq!(replaced.updated_at, original.updated_at);
    assert_eq!(replaced.subscription_renews_at, None);
    assert_eq!(replaced.subscription_expires_at, Some(expiration));
}

#[test]
fn copying_rate_windows_preserves_provider_details() {
    let updated_at = Timestamp::from_second(1_800_000_000).expect("timestamp");
    let codex_id = ProviderId::new("codex").expect("codex id");
    let identity = ProviderIdentitySnapshot::new(
        Some(codex_id),
        Some("test@example.com".to_string()),
        Some("Example".to_string()),
        Some("OAuth".to_string()),
        None,
    );

    let rows = vec![
        DetailRow::new(None::<&str>, "Balance", "$12.50", None::<&str>, None, None).unwrap(),
        DetailRow::new(
            None::<&str>,
            "Request quota",
            "10 / 50",
            None::<&str>,
            None,
            None,
        )
        .unwrap(),
    ];
    let detail_section = DetailSection::new(None::<&str>, rows, None).unwrap();

    let snapshot = UsageSnapshot::new(
        None,
        None,
        Some(RateWindow::new(30.0, Some(60), None, None, None, false).unwrap()),
        None,
        None,
        vec![detail_section],
        Some(Timestamp::from_second(1_800_000_000 + 100).unwrap()),
        Some(Timestamp::from_second(1_800_000_000 + 200).unwrap()),
        updated_at,
        Some(identity),
        Confidence::Unknown,
    )
    .unwrap();

    let copied = snapshot.with_rate_windows(
        Some(RateWindow::new(40.0, Some(300), None, None, None, false).unwrap()),
        Some(RateWindow::new(50.0, Some(10080), None, None, None, false).unwrap()),
    );

    assert_eq!(copied.primary.as_ref().map(|p| p.used_percent), Some(40.0));
    assert_eq!(
        copied.secondary.as_ref().map(|p| p.used_percent),
        Some(50.0)
    );
    assert_eq!(copied.tertiary.as_ref().map(|p| p.used_percent), Some(30.0));
    assert_eq!(
        copied.detail_row("Balance").map(|r| r.value.as_str()),
        Some("$12.50")
    );
    assert_eq!(
        copied.detail_row("Request quota").map(|r| r.value.as_str()),
        Some("10 / 50")
    );
    assert_eq!(
        copied.subscription_expires_at,
        Some(Timestamp::from_second(1_800_000_000 + 100).unwrap())
    );
    assert_eq!(
        copied.subscription_renews_at,
        Some(Timestamp::from_second(1_800_000_000 + 200).unwrap())
    );
    assert_eq!(
        copied
            .identity
            .as_ref()
            .and_then(|i| i.account_organization.as_deref()),
        Some("Example")
    );
}

#[test]
fn copying_identity_preserves_provider_details() {
    let updated_at = Timestamp::from_second(1_800_000_000).expect("timestamp");

    let rows = vec![
        DetailRow::new(
            None::<&str>,
            "Individual credits",
            "$12.50",
            None::<&str>,
            None,
            None,
        )
        .unwrap(),
        DetailRow::new(
            None::<&str>,
            "Request quota",
            "10 / 50",
            None::<&str>,
            None,
            None,
        )
        .unwrap(),
    ];
    let detail_section = DetailSection::new(None::<&str>, rows, None).unwrap();

    let snapshot = UsageSnapshot::new(
        None,
        None,
        None,
        None,
        None,
        vec![detail_section.clone()],
        Some(Timestamp::from_second(1_800_000_000 + 100).unwrap()),
        Some(Timestamp::from_second(1_800_000_000 + 200).unwrap()),
        updated_at,
        None,
        Confidence::Unknown,
    )
    .unwrap();

    let provider_id = ProviderId::new("kilo").expect("kilo id");
    let identity = ProviderIdentitySnapshot::new(
        Some(provider_id),
        Some("test@example.com".to_string()),
        Some("Example".to_string()),
        Some("API".to_string()),
        None,
    );

    let copied = snapshot.with_identity(Some(identity));

    assert_eq!(copied.details, vec![detail_section]);
    assert_eq!(
        copied.detail_row("Request quota").map(|r| r.value.as_str()),
        Some("10 / 50")
    );
    assert_eq!(
        copied.subscription_expires_at,
        Some(Timestamp::from_second(1_800_000_000 + 100).unwrap())
    );
    assert_eq!(
        copied.subscription_renews_at,
        Some(Timestamp::from_second(1_800_000_000 + 200).unwrap())
    );
    assert_eq!(
        copied
            .identity
            .as_ref()
            .and_then(|i| i.account_organization.as_deref()),
        Some("Example")
    );
}
