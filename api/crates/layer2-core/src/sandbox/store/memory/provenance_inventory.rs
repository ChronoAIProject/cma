use super::*;
use crate::resource_access::ResourceOwner;
use crate::sandbox::store::{AgentProvenanceInventoryStore, AgentProvenanceSnapshot};
use crate::sandbox::AgentListQuery;

#[async_trait::async_trait]
impl AgentProvenanceInventoryStore for InMemorySandboxStore {
    async fn scan_agent_provenance_inventory(
        &self,
        scope: &ResourceOwner,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<AgentProvenanceSnapshot>, SandboxStoreError> {
        PageRequest::new(page.limit, page.cursor.clone())?;
        let state = self.lock();
        let ids = match scope {
            ResourceOwner::Person { subject } => {
                let mut ids: Vec<_> = state.agents.values().filter(|agent| {
                    agent.owner == *subject && query.matches(agent)
                        && page.cursor.as_deref().is_none_or(|cursor| agent.id.as_str() > cursor)
                        && state.resource_policies.get(&agent.sandbox_id).is_none_or(|policy| {
                            matches!(&policy.owner, ResourceOwner::Person { subject: owner } if owner == subject)
                        })
                }).map(|agent| agent.id.clone()).collect();
                ids.sort();
                ids.truncate(usize::from(page.limit) + 1);
                ids
            }
            ResourceOwner::Organization { subject } => {
                super::resource_access::organization_agent_snapshots(&state, subject, query, page)
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?
                    .into_iter()
                    .map(|snapshot| {
                        snapshot.lineage.agent.map(|agent| agent.id).ok_or_else(|| {
                            SandboxStoreError::Corrupt("inventory candidate has no Agent".into())
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        ids.iter()
            .map(|id| super::provenance::snapshot(&state, id))
            .collect()
    }
}
