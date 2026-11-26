use std::collections::BTreeSet;

use super::*;

const OWNER_READ_BUDGET: Duration = Duration::from_millis(200);

/// Page-local, single-use observations. They never authorize an effect.
#[derive(Default)]
pub struct ForeignReconciliationOwners(BTreeMap<AgentId, Instant>);

impl OwnershipCoordinator {
    pub async fn observe_foreign_reconciliation_owners<'a>(
        &self,
        candidates: impl IntoIterator<Item = &'a AgentId>,
    ) -> ForeignReconciliationOwners {
        if !self.reconciliation_observation_ready() {
            return ForeignReconciliationOwners::default();
        }
        // Ordinary acquisition handles every tracked slot, including revoked
        // authorities and unresolved cancelled receivers.
        let eligible: BTreeSet<_> = {
            let owned = self.owned.lock().unwrap();
            candidates
                .into_iter()
                .filter(|id| !owned.contains_key(*id))
                .collect()
        };
        if eligible.is_empty() {
            return ForeignReconciliationOwners::default();
        }
        let ids: Vec<_> = eligible.iter().copied().collect();
        let started = Instant::now();
        let Ok(Ok(owners)) =
            tokio::time::timeout(OWNER_READ_BUDGET, self.store.reconciliation_owners(&ids)).await
        else {
            return ForeignReconciliationOwners::default();
        };
        let mut observations = BTreeMap::new();
        for owner in owners {
            let token = owner.token;
            if token.protocol != super::super::PROTOCOL_VERSION
                || token.epoch == 0
                || token.instance_id.is_empty()
                || token.instance_id == self.instance_id
                || !eligible.contains(&token.agent_id)
            {
                continue;
            }
            // Start at the local instant before the query, so checkout,
            // execution and cursor waits can only shorten the observation.
            if let Some(deadline) = started.checked_add(Duration::from_millis(owner.remaining_ms)) {
                observations.insert(token.agent_id, deadline);
            }
        }
        ForeignReconciliationOwners(observations)
    }

    pub fn defer_reconciliation_for_foreign_owner(
        &self,
        owners: &mut ForeignReconciliationOwners,
        id: &AgentId,
    ) -> bool {
        owners.0.remove(id).is_some_and(|deadline| {
            Instant::now() < deadline
                && self.reconciliation_observation_ready()
                && !self.owned.lock().unwrap().contains_key(id)
        })
    }

    fn reconciliation_observation_ready(&self) -> bool {
        self.is_ready() && self.skipped_heartbeats.load(Ordering::Acquire) == 0
    }
}
