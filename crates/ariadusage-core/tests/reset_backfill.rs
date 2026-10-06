// Ported from CodexBar Tests/CodexBarTests/ResetTimeBackfillTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::model::{
    NamedRateWindow, ProviderIdentitySnapshot, RateWindow, UsageSnapshot,
};
use ariadusage_protocol::{DetailRow, DetailSection, ProviderId};
use jiff::Timestamp;

#[test]
fn test_backfills_missing_reset_metadata_from_cached_window() {
    let now = Timestamp::from_second(1_800_000_000).expect("timestamp");
    let reset = Timestamp::from_second(1_800_000_000 + 3600).expect("timestamp");
    let cached = RateWindow::new(
        50.0,
        Some(300),
        Some(reset),
        Some("Resets in 1h".to_string()),
        Some(9.0),
        false,
    )
    .expect("cached");
    let fresh = RateWindow::new(62.0, None, None, None, Some(4.0), false).expect("fresh");

    let result = fresh.backfilling_reset_time(Some(&cached), now);

    assert_eq!(result.used_percent, 62.0);
    assert_eq!(result.window_minutes, Some(300));
    assert_eq!(result.resets_at, Some(reset));
    assert_eq!(result.reset_description.as_deref(), Some("Resets in 1h"));
    assert_eq!(result.next_regen_percent, Some(4.0));
}

#[test]
fn test_backfills_zero_window_duration_from_cached_window() {
    let now = Timestamp::from_second(1_800_000_000).expect("timestamp");
    let reset = Timestamp::from_second(1_800_000_000 + 3600).expect("timestamp");
    let cached = RateWindow::new(50.0, Some(300), Some(reset), None, None, false).expect("cached");
    let fresh = RateWindow::new(62.0, Some(0), None, None, None, false).expect("fresh");

    let result = fresh.backfilling_reset_time(Some(&cached), now);

    assert_eq!(result.window_minutes, Some(300));
    assert_eq!(result.resets_at, Some(reset));
}

#[test]
fn test_skips_expired_cached_reset() {
    let now = Timestamp::from_second(1_800_000_000).expect("timestamp");
    let cached_reset = Timestamp::from_second(1_800_000_000 - 60).expect("timestamp");
    let cached = RateWindow::new(
        50.0,
        Some(300),
        Some(cached_reset),
        Some("Expired".to_string()),
        None,
        false,
    )
    .expect("cached");
    let fresh = RateWindow::new(62.0, None, None, None, None, false).expect("fresh");

    let result = fresh.backfilling_reset_time(Some(&cached), now);

    assert_eq!(result.resets_at, None);
    assert_eq!(result.window_minutes, None);
    assert_eq!(result.reset_description, None);
}

#[test]
fn test_snapshot_backfill_preserves_current_snapshot_fields() {
    let now = Timestamp::from_second(1_800_000_000).expect("timestamp");
    let reset = Timestamp::from_second(1_800_000_000 + 3600).expect("timestamp");
    let claude_id = ProviderId::new("claude").expect("claude id");

    let identity = ProviderIdentitySnapshot::new(
        Some(claude_id),
        Some("invented@example.com".to_string()),
        Some("Org".to_string()),
        Some("OAuth".to_string()),
        None,
    );

    let cached = UsageSnapshot::new(
        Some(
            RateWindow::new(
                40.0,
                Some(300),
                Some(reset),
                Some("Soon".to_string()),
                None,
                false,
            )
            .unwrap(),
        ),
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        Timestamp::from_second(1_800_000_000 - 300).unwrap(),
        Some(identity.clone()),
        ariadusage_protocol::Confidence::Unknown,
    )
    .unwrap();

    let extra = NamedRateWindow::new(
        "overflow",
        "Overflow",
        RateWindow::new(12.0, None, None, None, Some(2.0), false).unwrap(),
    );

    let detail_row = DetailRow::new(
        None::<&str>,
        "Request quota",
        "10 / 50",
        None::<&str>,
        None,
        None,
    )
    .unwrap();
    let detail_section = DetailSection::new(None::<&str>, vec![detail_row], None).unwrap();

    let fresh = UsageSnapshot::new(
        Some(RateWindow::new(66.0, None, None, None, Some(7.0), false).unwrap()),
        None,
        None,
        Some(vec![extra]),
        None,
        vec![detail_section],
        Some(Timestamp::from_second(1_800_000_000 + 3600 + 86400).unwrap()),
        Some(Timestamp::from_second(1_800_000_000 + 3600 + 43200).unwrap()),
        now,
        Some(identity),
        ariadusage_protocol::Confidence::Unknown,
    )
    .unwrap();

    let result = fresh.backfilling_reset_times(Some(&cached), now);

    assert_eq!(
        result.primary.as_ref().and_then(|p| p.resets_at),
        Some(reset)
    );
    assert_eq!(result.primary.as_ref().map(|p| p.used_percent), Some(66.0));
    assert_eq!(
        result.primary.as_ref().and_then(|p| p.next_regen_percent),
        Some(7.0)
    );
    assert_eq!(
        result
            .extra_rate_windows
            .as_ref()
            .and_then(|w| w.first())
            .map(|w| w.id.as_str()),
        Some("overflow")
    );
    assert_eq!(
        result
            .extra_rate_windows
            .as_ref()
            .and_then(|w| w.first())
            .and_then(|w| w.window.next_regen_percent),
        Some(2.0)
    );
    assert_eq!(
        result.detail_row("Request quota").map(|r| r.value.as_str()),
        Some("10 / 50")
    );
    assert_eq!(
        result.subscription_expires_at,
        Some(Timestamp::from_second(1_800_000_000 + 3600 + 86400).unwrap())
    );
    assert_eq!(
        result.subscription_renews_at,
        Some(Timestamp::from_second(1_800_000_000 + 3600 + 43200).unwrap())
    );
    assert_eq!(
        result
            .identity
            .as_ref()
            .and_then(|i| i.account_email.as_deref()),
        Some("invented@example.com")
    );
}

#[test]
fn test_snapshot_backfill_skips_different_accounts() {
    let now = Timestamp::from_second(1_800_000_000).expect("timestamp");
    let claude_id = ProviderId::new("claude").expect("claude id");

    let cached = UsageSnapshot::new(
        Some(
            RateWindow::new(
                40.0,
                Some(300),
                Some(Timestamp::from_second(1_800_000_000 + 3600).unwrap()),
                Some("Soon".to_string()),
                None,
                false,
            )
            .unwrap(),
        ),
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        Timestamp::from_second(1_800_000_000 - 300).unwrap(),
        Some(ProviderIdentitySnapshot::new(
            Some(claude_id.clone()),
            Some("old@example.com".to_string()),
            None,
            None,
            None,
        )),
        ariadusage_protocol::Confidence::Unknown,
    )
    .unwrap();

    let fresh = UsageSnapshot::new(
        Some(RateWindow::new(66.0, None, None, None, None, false).unwrap()),
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        now,
        Some(ProviderIdentitySnapshot::new(
            Some(claude_id),
            Some("new@example.com".to_string()),
            None,
            None,
            None,
        )),
        ariadusage_protocol::Confidence::Unknown,
    )
    .unwrap();

    let result = fresh.backfilling_reset_times(Some(&cached), now);
    assert_eq!(result.primary.as_ref().and_then(|p| p.resets_at), None);
}

#[test]
fn test_snapshot_backfill_skips_same_email_with_different_stable_account_ids() {
    let now = Timestamp::from_second(1_800_000_000).expect("timestamp");
    let provider_id = ProviderId::new("cursor").expect("cursor id");

    let cached = UsageSnapshot::new(
        Some(
            RateWindow::new(
                40.0,
                Some(300),
                Some(Timestamp::from_second(1_800_000_000 + 3600).unwrap()),
                Some("Soon".to_string()),
                None,
                false,
            )
            .unwrap(),
        ),
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        Timestamp::from_second(1_800_000_000 - 300).unwrap(),
        Some(ProviderIdentitySnapshot::new(
            Some(provider_id.clone()),
            Some("shared@example.com".to_string()),
            None,
            None,
            Some("account-a".to_string()),
        )),
        ariadusage_protocol::Confidence::Unknown,
    )
    .unwrap();

    let fresh = UsageSnapshot::new(
        Some(RateWindow::new(66.0, None, None, None, None, false).unwrap()),
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        now,
        Some(ProviderIdentitySnapshot::new(
            Some(provider_id),
            Some("shared@example.com".to_string()),
            None,
            None,
            Some("account-b".to_string()),
        )),
        ariadusage_protocol::Confidence::Unknown,
    )
    .unwrap();

    let result = fresh.backfilling_reset_times(Some(&cached), now);
    assert_eq!(result.primary.as_ref().and_then(|p| p.resets_at), None);
}

#[test]
fn test_snapshot_backfill_keeps_other_provider_reset_when_description_changes() {
    let now = Timestamp::from_second(1_800_000_000).expect("timestamp");
    let reset = Timestamp::from_second(1_800_000_000 + 3600).expect("timestamp");
    let claude_id = ProviderId::new("claude").expect("claude id");

    let identity = ProviderIdentitySnapshot::new(
        Some(claude_id),
        Some("user@example.com".to_string()),
        None,
        None,
        None,
    );

    let cached = UsageSnapshot::new(
        Some(
            RateWindow::new(
                40.0,
                Some(300),
                Some(reset),
                Some("40 / 100 used".to_string()),
                None,
                false,
            )
            .unwrap(),
        ),
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        Timestamp::from_second(1_800_000_000 - 300).unwrap(),
        Some(identity.clone()),
        ariadusage_protocol::Confidence::Unknown,
    )
    .unwrap();

    let fresh = UsageSnapshot::new(
        Some(
            RateWindow::new(
                50.0,
                Some(300),
                None,
                Some("50 / 100 used".to_string()),
                None,
                false,
            )
            .unwrap(),
        ),
        None,
        None,
        None,
        None,
        vec![],
        None,
        None,
        now,
        Some(identity),
        ariadusage_protocol::Confidence::Unknown,
    )
    .unwrap();

    let result = fresh.backfilling_reset_times(Some(&cached), now);

    assert_eq!(
        result.primary.as_ref().and_then(|p| p.resets_at),
        Some(reset)
    );
    assert_eq!(
        result
            .primary
            .as_ref()
            .and_then(|p| p.reset_description.as_deref()),
        Some("50 / 100 used")
    );
}
