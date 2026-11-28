use std::sync::Arc;

use crate::resource_access::{ResourceOwner, ResourcePolicyFence};
use crate::sandbox::{Agent, AgentOperationError, AgentOperationStore, Sandbox, SandboxStatus};

use crate::trigger_domain::Launch;

use super::super::{AdmittedAgentCommand, CommandLedgerError};
use super::TriggerFirstMessageBinding;

/// Produced only by a trusted store after reading and matching all permanent
/// receipt copies and current resource/cleanup predicates in one snapshot.
#[derive(Debug, Clone)]
pub struct TriggerExecutionProof {
    binding: TriggerFirstMessageBinding,
    agent: Agent,
    parent: Sandbox,
    policy: ResourcePolicyFence,
}

impl TriggerExecutionProof {
    pub fn checked(
        binding: TriggerFirstMessageBinding,
        launch: Option<&Launch>,
        command: &AdmittedAgentCommand,
        agent: Agent,
        parent: Sandbox,
        owner: ResourceOwner,
        policy: ResourcePolicyFence,
    ) -> Result<Self, CommandLedgerError> {
        match launch {
            Some(launch) => binding.require_launch(launch)?,
            None if binding.is_onboarding() => {}
            None => return Err(CommandLedgerError::Corrupt),
        }
        binding.require_command(command)?;
        binding.require_agent(&agent)?;
        binding.require_parent(&parent, &owner)?;
        if agent
            .conversation
            .first_message
            .as_ref()
            .and_then(|first| first.common_binding.as_ref())
            != Some(&binding)
            || !agent.status.is_live()
            || agent.requested_stop_reason.is_some()
            || !matches!(
                parent.status,
                SandboxStatus::Active | SandboxStatus::CredentialRotating
            )
            || parent.lifecycle.checkpoint_capture.is_some()
            || policy.sandbox_id != parent.id
            || policy.sandbox_created_at != parent.created_at
        {
            return Err(CommandLedgerError::NotAdmissible);
        }
        Ok(Self {
            binding,
            agent,
            parent,
            policy,
        })
    }
    pub fn agent(&self) -> &Agent {
        &self.agent
    }
    pub fn parent(&self) -> &Sandbox {
        &self.parent
    }
    pub fn binding(&self) -> &TriggerFirstMessageBinding {
        &self.binding
    }
}

#[derive(Clone)]
pub struct TriggerExecutionAuthorization {
    store: Arc<dyn AgentOperationStore>,
    fence: Arc<TriggerExecutionFence>,
}

#[derive(Debug)]
pub struct TriggerExecutionFence {
    proof: TriggerExecutionProof,
    command: AdmittedAgentCommand,
    deadline: tokio::time::Instant,
}

impl TriggerExecutionAuthorization {
    pub(in crate::sandbox) async fn new(
        store: Arc<dyn AgentOperationStore>,
        command: &AdmittedAgentCommand,
    ) -> Result<Self, AgentOperationError> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        let proof = tokio::time::timeout_at(deadline, store.trigger_execution_proof(command))
            .await
            .map_err(|_| AgentOperationError::Conflict)?
            .map_err(super::super::super::agent_operation_service::operation_store_error)?;
        let value = Self {
            store,
            fence: Arc::new(TriggerExecutionFence {
                proof,
                command: command.clone(),
                deadline,
            }),
        };
        value.fence.require_dispatch(command)?;
        Ok(value)
    }
    pub fn fence(&self) -> Arc<TriggerExecutionFence> {
        self.fence.clone()
    }
    pub async fn current_proof(&self) -> Result<TriggerExecutionProof, AgentOperationError> {
        self.fence.require_current()?;
        let proof = tokio::time::timeout_at(
            self.fence.deadline,
            self.store.trigger_execution_proof(&self.fence.command),
        )
        .await
        .map_err(|_| AgentOperationError::Conflict)?
        .map_err(super::super::super::agent_operation_service::operation_store_error)?;
        self.fence.require_proof(&proof)?;
        Ok(proof)
    }
}

impl TriggerExecutionFence {
    pub fn binding(&self) -> &TriggerFirstMessageBinding {
        &self.proof.binding
    }
    pub fn require_current(&self) -> Result<(), AgentOperationError> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(AgentOperationError::Conflict);
        }
        Ok(())
    }
    pub fn require_dispatch(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), AgentOperationError> {
        self.require_current()?;
        self.binding()
            .require_command(command)
            .map_err(|_| AgentOperationError::Forbidden)?;
        if command.work.lease.as_ref().map(|lease| &lease.claim)
            != self.command.work.lease.as_ref().map(|lease| &lease.claim)
        {
            return Err(AgentOperationError::Conflict);
        }
        Ok(())
    }
    pub fn require_proof(&self, proof: &TriggerExecutionProof) -> Result<(), AgentOperationError> {
        self.require_current()?;
        if self.proof.binding != proof.binding || self.proof.policy != proof.policy {
            return Err(AgentOperationError::Conflict);
        }
        Ok(())
    }
}
