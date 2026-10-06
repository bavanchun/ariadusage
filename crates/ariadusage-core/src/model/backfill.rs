// Ported from CodexBar Sources/CodexBarCore/UsageFetcher.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use crate::model::identity::ProviderIdentitySnapshot;
use crate::model::snapshot::UsageSnapshot;

/// Compares two provider identities for reset-time backfill matching.
///
/// Matches if both are absent. If both are present, checks non-empty trimmed account IDs first.
/// If neither ID is present/usable, checks non-empty trimmed emails. If neither field is usable,
/// treats them as a match.
pub fn identities_match(
    lhs: Option<&ProviderIdentitySnapshot>,
    rhs: Option<&ProviderIdentitySnapshot>,
) -> bool {
    if lhs.is_none() && rhs.is_none() {
        return true;
    }
    let (Some(lhs), Some(rhs)) = (lhs, rhs) else {
        return false;
    };

    let lhs_account_id = lhs.account_id.as_deref().map(str::trim);
    let rhs_account_id = rhs.account_id.as_deref().map(str::trim);
    if let (Some(lhs_id), Some(rhs_id)) = (lhs_account_id, rhs_account_id)
        && !lhs_id.is_empty()
        && !rhs_id.is_empty()
    {
        return lhs_id == rhs_id;
    }

    let lhs_email = lhs.account_email.as_deref().map(str::trim);
    let rhs_email = rhs.account_email.as_deref().map(str::trim);
    if let (Some(lhs_e), Some(rhs_e)) = (lhs_email, rhs_email)
        && !lhs_e.is_empty()
        && !rhs_e.is_empty()
    {
        return lhs_e == rhs_e;
    }

    true
}

/// Backfills missing reset metadata from cached snapshot into current snapshot.
pub fn backfilling_reset_times(
    current: &UsageSnapshot,
    cached: Option<&UsageSnapshot>,
    now: jiff::Timestamp,
) -> UsageSnapshot {
    let Some(cached) = cached else {
        return current.clone();
    };
    if !identities_match(current.identity.as_ref(), cached.identity.as_ref()) {
        return current.clone();
    }

    let primary = current
        .primary
        .as_ref()
        .map(|p| p.backfilling_reset_time(cached.primary.as_ref(), now));
    let secondary = current
        .secondary
        .as_ref()
        .map(|s| s.backfilling_reset_time(cached.secondary.as_ref(), now));
    let tertiary = current
        .tertiary
        .as_ref()
        .map(|t| t.backfilling_reset_time(cached.tertiary.as_ref(), now));

    if primary == current.primary && secondary == current.secondary && tertiary == current.tertiary
    {
        current.clone()
    } else {
        current
            .clone()
            .with_rate_windows(primary, secondary)
            .with_tertiary(tertiary)
    }
}
