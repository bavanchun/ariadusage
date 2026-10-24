// Ported from CodexBar Tests/CodexBarTests/AntigravitySourceAccountIsolationTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityLocalSnapshotSelectionTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
// Ported from CodexBar Tests/CodexBarTests/AntigravityQuotaWindowBoundaryTests.swift at b49d42eb5; MIT, see LICENSES/CodexBar-MIT.txt
use ariadusage_engine::providers::antigravity::*;
use std::collections::BTreeMap;

// upstream: AntigravitySourceAccountIsolationTests.swift:8 local source keeps saved OAuth accounts passive without deleting them
#[test]
fn local_source_keeps_saved_oauth_accounts_passive_without_deleting_them() {
    let accounts = [("selected@example.com", "synthetic-oauth-credentials")];
    let selected = &accounts[0];
    assert_eq!(
        AntigravityUsageDataSource::Oauth.effective_selected_token_account(Some(selected)),
        Some(selected)
    );
    assert!(
        AntigravityUsageDataSource::Cli
            .effective_selected_token_account(Some(selected))
            .is_none()
    );
    assert_eq!(accounts.first(), Some(selected));
    assert_eq!(accounts.len(), 1);
    let environment =
        AntigravityUsageDataSource::Cli.make_environment(&BTreeMap::new(), Some(selected.1));
    assert!(
        !environment.contains_key(AntigravityOAuthCredentialsStore::ENVIRONMENT_CREDENTIALS_KEY)
    );
    assert_eq!(
        AntigravityUsageDataSource::Auto.effective_selected_token_account(Some(selected)),
        Some(selected)
    );
    assert_eq!(
        AntigravityUsageDataSource::Oauth.effective_selected_token_account(Some(selected)),
        Some(selected)
    );
}

// upstream: AntigravityLocalSnapshotSelectionTests.swift:5 selected account wins before quota richness
#[test]
fn selected_account_wins_before_quota_richness() {
    let selected_account = AntigravityStatusSnapshot::new(
        vec![AntigravityModelQuota::new(
            "Gemini 3 Pro Low",
            "gemini-3-pro-low",
            Some(0.9),
            None,
            None,
        )],
        Some("selected@example.com".into()),
        Some("Pro".into()),
    )
    .with_source(AntigravitySnapshotSource::Local);
    let richer_other_account = AntigravityStatusSnapshot::from_quota_summary(
        AntigravityQuotaSummary {
            raw_description: None,
            groups: vec![AntigravityQuotaSummaryGroup {
                key: "Gemini Models".into(),
                label: "Gemini Models".into(),
                buckets: vec![AntigravityQuotaSummaryBucket {
                    bucket_id: "gemini-5h".into(),
                    display_name: Some("Five Hour Limit".into()),
                    remaining_fraction: Some(0.9),
                    reset_time: None,
                    reset_description: None,
                    disabled: false,
                    window: None,
                }],
            }],
        },
        Some("other@example.com".into()),
        Some("Ultra".into()),
        AntigravitySnapshotSource::Local,
    );
    let result = AntigravityStatusProbe::preferred_local_snapshot_matching(
        &[richer_other_account, selected_account],
        Some(" SELECTED@example.com "),
    )
    .unwrap();
    assert_eq!(
        result.account_email.as_deref(),
        Some("selected@example.com")
    );
}
// Antigravity half only: native menu construction (`:11`, `:38`) and the Codex UI comparison in `:49` belong to the native frontend.
// upstream: AntigravityQuotaWindowBoundaryTests.swift:49 Antigravity ignores observed resets while Codex still honors them
#[test]
fn antigravity_engine_presentation_ignores_observed_quota_reset_boundaries() {
    const {
        assert!(AntigravityProviderPresentation::IGNORES_OBSERVED_QUOTA_RESET_BOUNDARIES);
    }
}
