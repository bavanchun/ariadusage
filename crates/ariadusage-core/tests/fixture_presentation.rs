//! The protocol fixture server cannot depend on core, so it repeats the
//! provider presentation literals. This test keeps them equal to what the
//! descriptors project.
#![cfg(unix)]

use ariadusage_core::providers::find_by_id;
use ariadusage_protocol::generate_scenario_snapshot;

#[test]
fn fixture_presentation_matches_descriptor_presentation() {
    for step in 0..5 {
        let snapshot = generate_scenario_snapshot(step);
        assert_eq!(snapshot.providers.len(), 3);
        for provider in &snapshot.providers {
            let descriptor = find_by_id(&provider.id)
                .unwrap_or_else(|| panic!("fixture provider {} has a descriptor", provider.id));
            assert_eq!(
                provider.presentation.as_ref(),
                Some(&descriptor.presentation()),
                "presentation drifted for {} at step {step}",
                provider.id
            );
        }
    }
}
