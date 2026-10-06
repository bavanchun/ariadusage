// Ported from CodexBar Tests/CodexBarTests/ProviderInstanceIDTests.swift at 6a26b2e9b; MIT, see LICENSES/CodexBar-MIT.txt

use ariadusage_core::providers::{find_by_id, first_party_order};
use ariadusage_protocol::ProviderId;

#[test]
fn first_party_mapping_preserves_raw_values_and_rejects_dynamic_ids() {
    for descriptor in first_party_order() {
        assert_eq!(descriptor.id.as_str(), descriptor.id.as_str());
        let found = find_by_id(&descriptor.id).expect("first party descriptor must be found");
        assert_eq!(found.id, descriptor.id);
    }

    let dynamic_id = ProviderId::new("acme-gateway").expect("valid dynamic ID");
    assert!(find_by_id(&dynamic_id).is_none());
}
