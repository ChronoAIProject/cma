//! Derived activity is used only by the enclosing still-open transaction.
use std::collections::BTreeMap;

use layer2_core::{
    sandbox::{store::SandboxStoreError, Agent, AgentLogEntry},
    AgentId, SandboxId,
};

use crate::store::{
    agent_capacity_recovery::{lineage, source::capture_segmented_scoped},
    agent_log_segments::transaction_scope::TransactionScope,
    sandbox_maintenance::transaction::ApplyError,
    sandbox_mongo::MongoSandboxStore,
};

pub(in crate::store::sandbox_maintenance) struct CapturedActivities {
    rows: BTreeMap<AgentId, (Agent, Option<AgentLogEntry>)>,
}

impl CapturedActivities {
    pub(super) fn contains(&self, agent: &Agent) -> bool {
        self.rows.contains_key(&agent.id)
    }

    pub(in crate::store::sandbox_maintenance) async fn capture(
        store: &MongoSandboxStore,
        id: &SandboxId,
        scope: &mut TransactionScope<'_>,
    ) -> Result<Self, ApplyError> {
        let mut rows = BTreeMap::new();
        for agent in lineage::agents(store, id, scope).await? {
            if !agent.status.is_live() {
                continue;
            }
            if let Some(source) = capture_segmented_scoped(store, &agent, scope).await? {
                let newest = source.latest_entry().cloned();
                rows.insert(agent.id.clone(), (agent, newest));
            }
        }
        Ok(Self { rows })
    }

    pub(super) fn latest(
        &self,
        agent: &Agent,
    ) -> Result<Option<Option<AgentLogEntry>>, ApplyError> {
        match self.rows.get(&agent.id) {
            Some((captured, entry)) if captured == agent => Ok(Some(entry.clone())),
            Some(_) => Err(ApplyError::Domain(SandboxStoreError::Conflict)),
            None => Ok(None),
        }
    }
}
