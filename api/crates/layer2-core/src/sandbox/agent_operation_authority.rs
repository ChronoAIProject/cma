use super::*;
use crate::resource_access::{AccessDecision, ResourceCapability};
use crate::Capability;

impl AgentOperationService {
    pub(in crate::sandbox) fn require_owner_authority(
        &self,
        caller: &Caller,
    ) -> Result<(), AgentOperationError> {
        if let Some(authority) = &self.execution_authority {
            if authority.caller() != caller {
                return Err(AgentOperationError::Forbidden);
            }
            authority
                .fence()
                .require_current()
                .map_err(|_| AgentOperationError::Forbidden)?;
            return Ok(());
        }
        caller
            .authorization
            .allows(Capability::UseOwnSandboxes)
            .then_some(())
            .ok_or(AgentOperationError::Forbidden)
    }

    pub(in crate::sandbox) async fn owned_agent(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<Agent, AgentOperationError> {
        let agent = if let Some(authority) = &self.execution_authority {
            self.require_owner_authority(caller)?;
            authority
                .current_snapshot()
                .await
                .map_err(super::super::agent_io::execution_authority::operation_error)?
                .lineage
                .agent
                .filter(|agent| agent.id == *id)
                .ok_or(AgentOperationError::NotFound)?
        } else {
            AgentStore::get_agent(self.store.as_ref(), id)
                .await
                .map_err(operation_store_error)?
                .filter(|agent| {
                    agent.id == *id
                        && AccessDecision::personal(caller, &agent.owner)
                            .allows(ResourceCapability::Interact)
                })
                .ok_or(AgentOperationError::NotFound)?
        };
        if agent.requested_stop_reason.is_some() {
            return Err(AgentOperationError::StatusConflict {
                status: agent.status,
            });
        }
        Ok(agent)
    }
}
