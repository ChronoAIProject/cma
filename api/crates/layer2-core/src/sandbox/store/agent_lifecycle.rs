use async_trait::async_trait;

use crate::ids::{AgentId, SandboxId};
use crate::sandbox::{Agent, AgentLogKind, AgentStatus, AgentWorkStep, SandboxEventKind, Worktree};
use crate::seconds_from_rfc3339;

use super::{Page, PageRequest, SandboxStoreError};

#[derive(Debug, Clone)]
pub struct VersionedWorktree {
    pub expected_revision: u64,
    pub worktree: Worktree,
}

/// One fenced, all-or-nothing agent lifecycle mutation.
#[derive(Debug, Clone)]
pub struct AgentWorkMutation {
    pub expected_agent_revision: u64,
    pub holder: String,
    pub fence: u64,
    pub agent: Agent,
    pub worktree: Option<VersionedWorktree>,
    pub appended_at: String,
    pub sandbox_event: Option<SandboxEventKind>,
    pub agent_logs: Vec<AgentLogKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentConnectionRecoveryCause {
    ControlPlaneRestart,
    UpdateStreamLost,
}

/// Content-free durable identity used to reconcile runtime attachments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConnectionRecoveryCandidate {
    pub agent_id: AgentId,
    pub sandbox_id: SandboxId,
    pub conversation_id: String,
    pub runtime_generation: u64,
    pub revision: u64,
    pub work_step: AgentWorkStep,
    /// Monotonic applied revision from the same validated Agent observation.
    /// This can skip completed maintenance, never authorize a new write.
    pub managed_guidance_revision: u64,
}

impl AgentConnectionRecoveryCandidate {
    pub fn from_agent(agent: &Agent) -> Option<Self> {
        let conversation_id = agent
            .conversation
            .conversation_id
            .as_ref()
            .filter(|id| !id.trim().is_empty() && id.len() <= 512)?;
        let runtime_generation = agent
            .conversation
            .runtime_generation
            .filter(|value| *value > 0)?;
        agent.status.is_live().then(|| Self {
            agent_id: agent.id.clone(),
            sandbox_id: agent.sandbox_id.clone(),
            conversation_id: conversation_id.clone(),
            runtime_generation,
            revision: agent.revision,
            work_step: agent.work.step,
            managed_guidance_revision: agent.conversation.managed_guidance_revision,
        })
    }

    pub fn restart_request(&self, requested_at: String) -> AgentConnectionRecoveryRequest {
        AgentConnectionRecoveryRequest {
            agent_id: self.agent_id.clone(),
            conversation_id: self.conversation_id.clone(),
            runtime_generation: self.runtime_generation,
            expected_revision: self.revision,
            expected_work_step: self.work_step,
            expected_authority_fence: None,
            requested_at,
            cause: AgentConnectionRecoveryCause::ControlPlaneRestart,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConnectionRecoveryRequest {
    pub agent_id: AgentId,
    pub conversation_id: String,
    pub runtime_generation: u64,
    pub expected_revision: u64,
    pub expected_work_step: AgentWorkStep,
    pub expected_authority_fence: Option<u64>,
    pub requested_at: String,
    pub cause: AgentConnectionRecoveryCause,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentConnectionRecoveryOutcome {
    Scheduled,
    AlreadyScheduled,
    Superseded,
    NotEligible,
}

#[doc(hidden)]
pub enum AgentConnectionRecoveryApplication {
    Persist(Box<Agent>),
    Complete(AgentConnectionRecoveryOutcome),
}

#[doc(hidden)]
pub fn apply_agent_connection_recovery(
    current: &Agent,
    request: &AgentConnectionRecoveryRequest,
) -> Result<AgentConnectionRecoveryApplication, SandboxStoreError> {
    let requested_at = seconds_from_rfc3339(&request.requested_at).ok_or(
        SandboxStoreError::InvalidTransaction("agent connection recovery request is invalid"),
    )?;
    let authority_shape = matches!(
        (request.cause, request.expected_authority_fence),
        (AgentConnectionRecoveryCause::ControlPlaneRestart, None)
            | (AgentConnectionRecoveryCause::UpdateStreamLost, Some(_))
    );
    if request.agent_id != current.id
        || request.conversation_id.trim().is_empty()
        || request.conversation_id.len() > 512
        || request.runtime_generation == 0
        || !authority_shape
        || request
            .expected_authority_fence
            .is_some_and(|fence| fence == 0)
    {
        return Err(SandboxStoreError::InvalidTransaction(
            "agent connection recovery request is invalid",
        ));
    }
    if current.conversation.conversation_id.as_deref() != Some(&request.conversation_id)
        || current.conversation.runtime_generation != Some(request.runtime_generation)
    {
        return Ok(AgentConnectionRecoveryApplication::Complete(
            AgentConnectionRecoveryOutcome::Superseded,
        ));
    }
    let eligible_status = matches!(
        current.status,
        AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
    );
    if !eligible_status || current.requested_stop_reason.is_some() {
        return Ok(AgentConnectionRecoveryApplication::Complete(
            AgentConnectionRecoveryOutcome::NotEligible,
        ));
    }
    if request
        .expected_authority_fence
        .is_some_and(|fence| current.work.next_fence <= fence)
    {
        return Ok(AgentConnectionRecoveryApplication::Complete(
            AgentConnectionRecoveryOutcome::Superseded,
        ));
    }
    if current.work.step == AgentWorkStep::PublishConnection {
        return Ok(AgentConnectionRecoveryApplication::Complete(
            AgentConnectionRecoveryOutcome::AlreadyScheduled,
        ));
    }
    if current.revision != request.expected_revision
        || current.work.step != request.expected_work_step
    {
        return Ok(AgentConnectionRecoveryApplication::Complete(
            AgentConnectionRecoveryOutcome::Superseded,
        ));
    }
    if current.work.step != AgentWorkStep::None
        || current.work.lease.is_some()
        || seconds_from_rfc3339(&current.updated_at).is_none_or(|updated| updated > requested_at)
    {
        return Ok(AgentConnectionRecoveryApplication::Complete(
            AgentConnectionRecoveryOutcome::NotEligible,
        ));
    }
    let mut scheduled = current.clone();
    scheduled.work.schedule(
        AgentWorkStep::PublishConnection,
        request.requested_at.clone(),
    );
    scheduled.updated_at = request.requested_at.clone();
    scheduled.revision = current
        .revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    scheduled
        .validate_conversation()
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    Ok(AgentConnectionRecoveryApplication::Persist(Box::new(
        scheduled,
    )))
}

/// Durable agent work discovery, lease fencing, and atomic publication.
#[async_trait]
pub trait AgentWorkerStore: Send + Sync {
    /// Exact durable preparation authority for replaying a worktree/home pair.
    /// Stores without such evidence retain the ordinary individual work steps.
    async fn prepared_agent_replay_matches(
        &self,
        _agent: &Agent,
        _sandbox: &crate::sandbox::Sandbox,
        _workspace: &crate::sandbox::Workspace,
        _worktree: &Worktree,
    ) -> Result<bool, SandboxStoreError> {
        Ok(false)
    }

    /// Publish a proved prepared open and Ready under the same fresh parent observation.
    /// False means this store has no matching preparation; use ordinary publication.
    async fn commit_prepared_agent_open_ready(
        &self,
        _mutation: AgentWorkMutation,
        _observed: super::PreparedAgentOpenContext,
    ) -> Result<bool, SandboxStoreError> {
        Ok(false)
    }

    async fn live_agent_connections(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentConnectionRecoveryCandidate>, SandboxStoreError>;

    async fn schedule_agent_connection_recovery(
        &self,
        request: AgentConnectionRecoveryRequest,
    ) -> Result<AgentConnectionRecoveryOutcome, SandboxStoreError>;

    async fn due_agent_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<AgentId>, SandboxStoreError>;

    async fn claim_agent_work(
        &self,
        id: &AgentId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Agent>, SandboxStoreError>;

    async fn commit_agent_work(&self, mutation: AgentWorkMutation)
        -> Result<(), SandboxStoreError>;
}
