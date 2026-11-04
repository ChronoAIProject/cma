//! Sandbox colours projected onto owner-facing Sandbox views.

use std::collections::BTreeMap;

use layer2_core::sandbox::Sandbox;
use layer2_core::SubjectId;

use super::instances::HierarchyState;

/// Each listed Sandbox's colour keyed by its id, as the view spells it. Empty
/// when this deployment composes no colour service.
pub(super) async fn colors(
    state: &HierarchyState,
    caller: &SubjectId,
    sandboxes: &[&Sandbox],
) -> BTreeMap<String, String> {
    let Some(service) = &state.colors else {
        return BTreeMap::new();
    };
    service
        .colors(caller, sandboxes)
        .await
        .into_iter()
        .map(|(sandbox, color)| (sandbox.to_string(), color.to_string()))
        .collect()
}
