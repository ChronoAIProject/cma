use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use super::super::{
    store::{
        AgentControlStoreError, AgentControlTarget, AgentRestartAcceptance, AgentRestartRequest,
        AgentRestartSource, AgentServiceStore,
    },
    Agent, AgentServiceError, AgentStatus, AgentStopReason, Sandbox, Workspace, Worktree,
};
use super::support::{control_error, store_error};
use crate::{
    now_rfc3339, operation::valid_idempotency_key, AgentId, Caller, Capability, OperationClaim,
};

#[async_trait::async_trait]
pub trait AgentRestartObservation: Send + Sync {
    async fn prove_closed(
        &self,
        agent: &Agent,
        sandbox: &Sandbox,
        workspace: &Workspace,
        worktree: &Worktree,
    ) -> Result<AgentRestartSource, AgentServiceError>;
}

pub struct AgentRestartService {
    store: Arc<dyn AgentServiceStore>,
    observation: Arc<dyn AgentRestartObservation>,
}

#[async_trait::async_trait]
pub trait AgentRestartControl: Send + Sync {
    async fn restart(
        &self,
        caller: &Caller,
        agent_id: &AgentId,
        key: &str,
    ) -> Result<AgentRestartAcceptance, AgentServiceError>;
}

impl AgentRestartService {
    pub fn new(
        store: Arc<dyn AgentServiceStore>,
        observation: Arc<dyn AgentRestartObservation>,
    ) -> Arc<Self> {
        Arc::new(Self { store, observation })
    }
}

#[async_trait::async_trait]
impl AgentRestartControl for AgentRestartService {
    async fn restart(
        &self,
        caller: &Caller,
        agent_id: &AgentId,
        key: &str,
    ) -> Result<AgentRestartAcceptance, AgentServiceError> {
        if !caller.authorization.allows(Capability::UseOwnSandboxes) {
            return Err(AgentServiceError::Forbidden);
        }
        if !valid_idempotency_key(key.as_bytes()) {
            return Err(AgentServiceError::InvalidRequest(
                "invalid restart key".into(),
            ));
        }
        let agent = self
            .store
            .get_agent(agent_id)
            .await
            .map_err(store_error)?
            .filter(|agent| agent.owner == caller.subject)
            .ok_or(AgentServiceError::NotFound)?;
        let operation = format!("agent:owner-restart:{}", agent.id);
        let claim = OperationClaim::new(
            caller.subject.clone(),
            operation.clone(),
            format!("{:x}", Sha256::digest(key.as_bytes())),
            Sha256::digest(operation.as_bytes()).into(),
            SystemTime::UNIX_EPOCH,
            Duration::ZERO,
        );
        let mut request = AgentRestartRequest {
            target: AgentControlTarget::from(&agent),
            caller: caller.clone(),
            claim,
            observed_process: None,
            requested_at: now_rfc3339(),
        };
        match self.store.schedule_agent_restart(request.clone()).await {
            Ok(accepted) => return Ok(accepted),
            Err(AgentControlStoreError::RestartEvidenceRequired) => {}
            Err(error) => return Err(control_error(error)),
        }
        if agent.status != AgentStatus::Stopped {
            return Err(AgentServiceError::StatusConflict {
                status: agent.status,
            });
        }
        let (sandbox, workspace, worktree) = tokio::join!(
            self.store.get_sandbox(&agent.sandbox_id),
            self.store.get_workspace(&agent.workspace_id),
            self.store.get_worktree(&agent.worktree_id),
        );
        let sandbox = sandbox
            .map_err(store_error)?
            .ok_or(AgentServiceError::NotFound)?;
        let workspace = workspace
            .map_err(store_error)?
            .ok_or(AgentServiceError::NotFound)?;
        let worktree = worktree
            .map_err(store_error)?
            .ok_or(AgentServiceError::NotFound)?;
        let has_stopped_turn = agent.stop_reason == Some(AgentStopReason::Owner)
            && agent.conversation.active_turn.is_some();
        let classify_unsupported = |error| {
            if has_stopped_turn && error == AgentServiceError::RestartRuntimeUnsupported {
                AgentServiceError::RestartTurnUnsettled
            } else {
                error
            }
        };
        agent
            .plan_owner_restart(&worktree, request.requested_at.clone())
            .map_err(|_| AgentServiceError::ControlConflict)?;
        request.observed_process = Some(
            self.observation
                .prove_closed(&agent, &sandbox, &workspace, &worktree)
                .await
                .map_err(classify_unsupported)?,
        );
        request.requested_at = now_rfc3339();
        self.store
            .schedule_agent_restart(request)
            .await
            .map_err(control_error)
    }
}
