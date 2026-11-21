use super::*;
use crate::resource_access::ResourceOwner;
use crate::sandbox::store::{AgentChannelUsageSnapshot, AgentProvenanceInventoryStore};
use crate::sandbox::AgentListQuery;

#[async_trait::async_trait]
impl AgentProvenanceInventoryStore for InMemorySandboxStore {
    async fn scan_agent_provenance_inventory(
        &self,
        scope: &ResourceOwner,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<AgentChannelUsageSnapshot>, SandboxStoreError> {
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
        ids.iter().map(|id| task_snapshot(&state, id)).collect()
    }
    async fn scan_shared_agent_provenance_inventory(
        &self,
        recipient: &SubjectId,
        organizations: &[SubjectId],
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<AgentChannelUsageSnapshot>, SandboxStoreError> {
        use crate::resource_access::ResourceSelector;
        PageRequest::new(page.limit, page.cursor.clone())?;
        let state = self.lock();
        let mut ids: Vec<_> = state.agents.values().filter(|agent| {
            query.matches(agent)
                && page.cursor.as_deref().is_none_or(|cursor| agent.id.as_str() > cursor)
                && state.resource_shares.values().any(|receipt| {
                    receipt.sandbox_id == agent.sandbox_id
                        && receipt.share.incoming_for(recipient, organizations)
                        && receipt.share.revoked_at.is_none()
                        && !matches!(&receipt.share.owner, ResourceOwner::Person { subject } if subject == recipient)
                        && match &receipt.share.selector {
                            ResourceSelector::Sandbox { sandbox_id } => sandbox_id == &agent.sandbox_id,
                            ResourceSelector::Agent { agent_id } => agent_id == &agent.id,
                        }
                })
        }).map(|agent| agent.id.clone()).collect();
        ids.sort();
        ids.truncate(usize::from(page.limit) + 1);
        ids.iter()
            .map(|id| {
                let value = task_snapshot(&state, id)?;
                value
                    .provenance
                    .resource
                    .shared_agent(recipient, organizations)
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
                Ok(value)
            })
            .collect()
    }
}

fn task_snapshot(
    state: &State,
    id: &AgentId,
) -> Result<AgentChannelUsageSnapshot, SandboxStoreError> {
    let mut snapshot = super::channel_usage::snapshot(state, id)?;
    let agent = snapshot
        .provenance
        .resource
        .lineage
        .agent
        .as_ref()
        .ok_or(SandboxStoreError::NotFound)?;
    let entries = state.agent_logs.get(id).map_or(&[][..], Vec::as_slice);
    let pending_work = state.agent_commands.values().any(|command| {
        command.receipt.envelope.agent_id == agent.id && !command.status.is_terminal()
    });
    snapshot.task_status = crate::sandbox::AgentTaskStatus::from_log(agent, entries, pending_work);
    Ok(snapshot)
}
