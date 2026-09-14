use std::collections::HashMap;

use super::{Trigger, TriggerAvailability, TriggerId, TriggerRevision, TriggerStoreError};

pub(super) fn guard_references(
    triggers: &HashMap<TriggerId, Trigger>,
    references: impl Fn(&TriggerRevision) -> bool,
) -> Result<(), TriggerStoreError> {
    let mut trigger_ids = triggers
        .values()
        .filter(|trigger| trigger.availability == TriggerAvailability::Published)
        .filter_map(|trigger| {
            trigger
                .latest_revision()
                .filter(|revision| references(revision))
                .map(|_| trigger.id.clone())
        })
        .collect::<Vec<_>>();
    trigger_ids.sort();
    if trigger_ids.is_empty() {
        Ok(())
    } else {
        Err(TriggerStoreError::ProfileReferenced { trigger_ids })
    }
}
