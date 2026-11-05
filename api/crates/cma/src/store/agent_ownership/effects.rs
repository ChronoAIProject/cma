//! Ownership proof joins the same transaction as each transport-derived effect.
use super::super::sandbox_mongo::MongoSandboxStore;
use super::*;
use layer2_core::agent_ownership::execution_owner;
use layer2_core::sandbox::store::SandboxStoreError;

pub(super) const WORK_EPOCH: &str = "agent_manager_work_epoch";

impl MongoSandboxStore {
    pub(in crate::store) async fn fence_checkpoint_owners(
        &self,
        mutation: &layer2_core::sandbox::store::SandboxCheckpointMutation,
        session: &mut ClientSession,
    ) -> Result<(), SandboxStoreError> {
        let Some(ownership) = &self.agent_ownership else {
            return Ok(());
        };
        // Immutable exported snapshots do not read a live Agent transport.
        if mutation.nonblocking {
            return Ok(());
        }
        for conversation in &mutation.checkpoint.conversations {
            let token = layer2_core::agent_ownership::observed_transport(&conversation.agent_id)
                .ok_or(SandboxStoreError::Conflict)?;
            ownership
                .fence_transaction(&token, session)
                .await
                .map_err(effect_error)?;
        }
        Ok(())
    }

    pub(crate) fn agent_manager_instance(&self) -> Option<&str> {
        self.agent_manager_instance.as_deref()
    }

    pub(crate) fn ownership_routing_client(&self) -> Option<Client> {
        self.agent_ownership.as_ref().map(|_| self.client.clone())
    }
    pub(crate) async fn enable_agent_ownership(
        mut self,
        instance: String,
    ) -> Result<Self, OwnershipError> {
        self.agent_ownership = Some(std::sync::Arc::new(
            MongoAgentOwnership::open(&self.client, &self.agents.namespace().db).await?,
        ));
        self.agent_manager_instance = Some(instance);
        Ok(self)
    }

    pub(in crate::store) async fn fence_agent_effect(
        &self,
        id: &AgentId,
        session: &mut ClientSession,
    ) -> Result<(), SandboxStoreError> {
        let Some(store) = &self.agent_ownership else {
            return Ok(());
        };
        let token = execution_owner()
            .filter(|owner| owner.agent_id == *id)
            .ok_or(SandboxStoreError::Conflict)?;
        store
            .fence_transaction(&token, session)
            .await
            .map_err(effect_error)
    }

    pub(in crate::store) fn agent_work_owner_fields(
        &self,
        id: &AgentId,
        fields: &mut Document,
    ) -> Result<(), SandboxStoreError> {
        if self.agent_ownership.is_some() {
            let token = execution_owner()
                .filter(|owner| owner.agent_id == *id)
                .ok_or(SandboxStoreError::Conflict)?;
            fields.insert(
                WORK_EPOCH,
                i64::try_from(token.epoch).map_err(|_| SandboxStoreError::Conflict)?,
            );
        }
        Ok(())
    }

    /// A successor must not wait for an old owner's 18-minute lifecycle claim.
    /// Persisting its new claim fences both owner epoch and Agent revision.
    pub(in crate::store) async fn recover_agent_work_owner(
        &self,
        agent: &mut layer2_core::sandbox::Agent,
    ) -> Result<(), SandboxStoreError> {
        let Some(store) = &self.agent_ownership else {
            return Ok(());
        };
        let token = execution_owner()
            .filter(|owner| owner.agent_id == agent.id)
            .ok_or(SandboxStoreError::Conflict)?;
        store.check(&token).await.map_err(effect_error)?;
        let row = self
            .agents
            .find_one(doc! {"_id": agent.id.as_str(), "revision": agent.revision as i64})
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .ok_or(SandboxStoreError::Conflict)?;
        if row.get_i64(WORK_EPOCH).ok() != i64::try_from(token.epoch).ok() {
            agent.work.lease = None;
        }
        Ok(())
    }
}

fn effect_error(error: OwnershipError) -> SandboxStoreError {
    match error {
        OwnershipError::Unavailable(detail) => SandboxStoreError::Unavailable(detail),
        _ => SandboxStoreError::Conflict,
    }
}
