//! Observe ordinary live credential rotation without lifecycle mutation.
use super::*;
use layer2_core::sandbox::{Agent, Sandbox};

impl AgentAccessWorker {
    pub(super) async fn updated(
        &self,
        record: &AgentAccessRecord,
        agent: &Agent,
        sandbox: &Sandbox,
    ) -> Result<(), AgentAccessError> {
        if !delivery_ready(record, agent, sandbox)? {
            return Ok(());
        }
        let mut progress = record.progress.clone();
        progress.delivered_credential = sandbox.credential.current.clone();
        progress.delivered_runtime = sandbox.runtime.clone();
        self.store
            .advance_access_request(
                record.lease.as_ref().ok_or(AgentAccessError::Conflict)?,
                Phase::Updating,
                Phase::Notifying,
                progress,
            )
            .await?;
        Ok(())
    }
}

fn delivery_ready(
    record: &AgentAccessRecord,
    agent: &Agent,
    sandbox: &Sandbox,
) -> Result<bool, AgentAccessError> {
    let hot = record
        .progress
        .hot_update
        .as_ref()
        .ok_or(AgentAccessError::Invalid)?;
    let floor = record
        .progress
        .grant_generation_floor
        .ok_or(AgentAccessError::Invalid)?;
    if sandbox.runtime.as_ref() != Some(&hot.runtime)
        || sandbox.lifecycle.next_authority != hot.lifecycle_next_authority
        || sandbox.lifecycle.operation.is_some()
    {
        return Err(AgentAccessError::Invalid);
    }
    Ok(sandbox.status == SandboxStatus::Active
        && effects::settled(sandbox)
        && sandbox.credential.delivered
        && sandbox.credential.revoke_debt.is_empty()
        && sandbox
            .credential
            .current
            .as_ref()
            .is_some_and(|current| current.generation > floor)
        && agent.id == record.requester.agent_id
        && agent.conversation.conversation_id.as_deref() == Some(&record.requester.conversation)
        && agent.conversation.runtime_generation == hot.runtime.provision_generation
        && record
            .approved_services()?
            .iter()
            .all(|service| sandbox.credential.effective_reaches().contains(service)))
}
