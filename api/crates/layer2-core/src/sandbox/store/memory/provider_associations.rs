use super::*;
use crate::sandbox::provider_associations::*;
use std::ops::Bound::{Excluded, Included, Unbounded};

#[async_trait]
impl AgentProviderAssociationStore for InMemorySandboxStore {
    async fn agent_provider_associations(
        &self,
        owner: &SubjectId,
        id: &AgentId,
        page: &PageRequest,
    ) -> Result<AgentProviderAssociationSnapshot, SandboxStoreError> {
        let state = self.lock();
        let provenance = super::provenance::snapshot(&state, id)?;
        let creation = provenance.creation_provenance(id)?;
        if &provenance.resource.lineage.sandbox.owner != owner {
            return Err(SandboxStoreError::Conflict);
        }
        let after = association_cursor(page, &creation)?;
        let start = (owner.clone(), id.clone(), after.clone().unwrap_or_default());
        let range = state.binding_association_index.range((
            if after.is_some() {
                Excluded(start)
            } else {
                Included(start)
            },
            Unbounded,
        ));
        let mut records = Vec::with_capacity(usize::from(page.limit) + 1);
        for ((indexed_owner, indexed_agent, key), ()) in range
            .take_while(|((o, a, _), _)| o == owner && a == id)
            .take(usize::from(page.limit) + 1)
        {
            let conversation = state.binding_conversations.get(key).ok_or_else(corrupt)?;
            if &conversation.owner != indexed_owner
                || &conversation.agent_id != indexed_agent
                || crate::sandbox::binding::store::conversation_storage_key(conversation) != *key
            {
                return Err(corrupt());
            }
            let binding = state
                .bindings
                .get(&conversation.binding_id)
                .ok_or_else(corrupt)?;
            records.push(ProviderAssociationRecord {
                conversation: conversation.clone(),
                binding: binding.clone(),
            });
        }
        let snapshot = AgentProviderAssociationSnapshot {
            provenance,
            records,
        };
        association_page(&snapshot, owner, id, page)?;
        Ok(snapshot)
    }
}
fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("provider association index or parent missing".into())
}
