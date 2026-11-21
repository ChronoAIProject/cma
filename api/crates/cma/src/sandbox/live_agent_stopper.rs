use std::{collections::HashSet, sync::Arc};

use layer2_core::sandbox::store::{
    AgentControlStore, AgentControlStoreError, AgentControlTarget, AgentStopRequest, PageRequest,
    SandboxLifecycleExecutionStore, SandboxStoreError, MAX_PAGE_SIZE, MAX_SANDBOX_LIFECYCLE_AGENTS,
};
use layer2_core::sandbox::{Agent, AgentStopReason, AgentWorkStep, Sandbox};
use layer3_adapter::RuntimeHandle;

use super::live_agents::LiveAgentRegistry;
use super::worker::{LiveAgentStopper, SandboxDependencyError};

const AGENTS_STOPPING: &str = "sandbox agents are still stopping";
const AGENT_STORE_UNAVAILABLE: &str = "sandbox agent control is temporarily unavailable";
const AGENT_STATE_CHANGED: &str = "sandbox agent state changed during retirement";
const AGENT_STATE_INVALID: &str = "sandbox agent state is not usable";
const AGENT_BOUNDARY_INVALID: &str = "sandbox agent lineage is outside retirement authority";

/// Durable retirement gate between SandboxWorker and AgentWorker.
pub(crate) struct DurableLiveAgentStopper {
    lifecycle: Arc<dyn SandboxLifecycleExecutionStore>,
    controls: Arc<dyn AgentControlStore>,
    registry: Arc<LiveAgentRegistry>,
}

impl DurableLiveAgentStopper {
    pub(crate) fn new<S>(store: Arc<S>, registry: Arc<LiveAgentRegistry>) -> Self
    where
        S: SandboxLifecycleExecutionStore + AgentControlStore + 'static,
    {
        Self {
            lifecycle: store.clone(),
            controls: store,
            registry,
        }
    }

    async fn sandbox_agents(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Vec<Agent>, SandboxDependencyError> {
        let mut agents = Vec::new();
        let mut agent_ids = HashSet::new();
        let mut seen_cursors = HashSet::new();
        let mut cursor = None;

        loop {
            let page = self
                .lifecycle
                .sandbox_agents(
                    &sandbox.id,
                    &PageRequest::new(MAX_PAGE_SIZE, cursor.clone()).expect("valid page size"),
                )
                .await
                .map_err(|error| sandbox_store_failure(sandbox, error))?;
            if page.items.len() > usize::from(MAX_PAGE_SIZE) {
                return Err(invalid_agent_state(
                    sandbox,
                    "sandbox-agent page exceeded its requested bound",
                ));
            }
            if page.next.is_some() && page.items.is_empty() {
                return Err(invalid_agent_state(
                    sandbox,
                    "sandbox-agent cursor advanced without an item",
                ));
            }
            for agent in page.items {
                validate_agent(sandbox, &agent)?;
                if !agent_ids.insert(agent.id.clone()) {
                    return Err(invalid_agent_state(
                        sandbox,
                        "sandbox-agent pagination repeated an agent",
                    ));
                }
                agents.push(agent);
                if agents.len() > MAX_SANDBOX_LIFECYCLE_AGENTS {
                    return Err(invalid_agent_state(
                        sandbox,
                        "sandbox-agent set exceeded its lifecycle bound",
                    ));
                }
            }

            let Some(next) = page.next else {
                return Ok(agents);
            };
            if cursor.as_ref() == Some(&next) || !seen_cursors.insert(next.clone()) {
                return Err(invalid_agent_state(
                    sandbox,
                    "sandbox-agent cursor did not make progress",
                ));
            }
            cursor = Some(next);
        }
    }

    async fn ensure_stopping(
        &self,
        sandbox: &Sandbox,
        agent: Agent,
    ) -> Result<Agent, SandboxDependencyError> {
        if agent.requested_stop_reason.is_some() {
            validate_stop_work(sandbox, &agent)?;
        }
        if agent
            .requested_stop_reason
            .as_ref()
            .is_some_and(|reason| reason != &AgentStopReason::SandboxRetired)
        {
            return Ok(agent);
        }

        let observed = agent.clone();
        let result = self
            .controls
            .schedule_agent_stop(AgentStopRequest {
                target: AgentControlTarget::from(&agent),
                reason: AgentStopReason::SandboxRetired,
                requested_at: sandbox.updated_at.clone(),
            })
            .await
            .map_err(|error| control_store_failure(sandbox, &agent, error))?
            .into_agent();
        validate_control_result(sandbox, &observed, &result)?;
        Ok(result)
    }
}

#[async_trait::async_trait]
impl LiveAgentStopper for DurableLiveAgentStopper {
    async fn stop_live_agents(
        &self,
        sandbox: &Sandbox,
        _runtime: Option<&RuntimeHandle>,
        _operation_id: &str,
    ) -> Result<(), SandboxDependencyError> {
        let agents = self.sandbox_agents(sandbox).await?;
        let mut has_live_agents = false;
        for agent in agents {
            if !agent.status.is_live() {
                continue;
            }
            has_live_agents = true;
            let stopping = self.ensure_stopping(sandbox, agent).await?;
            self.registry.fence(
                &stopping.id,
                stopping.conversation.runtime_generation,
                stopping.work.next_fence,
            );
        }

        if has_live_agents {
            Err(SandboxDependencyError::Transient(AGENTS_STOPPING.into()))
        } else {
            Ok(())
        }
    }
}

fn validate_agent(sandbox: &Sandbox, agent: &Agent) -> Result<(), SandboxDependencyError> {
    if agent.sandbox_id != sandbox.id || agent.owner != sandbox.owner {
        tracing::error!(
            sandbox_id = %sandbox.id,
            agent_id = %agent.id,
            sandbox_matches = agent.sandbox_id == sandbox.id,
            owner_matches = agent.owner == sandbox.owner,
            "sandbox retirement read an agent outside its authority"
        );
        return Err(SandboxDependencyError::Security(
            AGENT_BOUNDARY_INVALID.into(),
        ));
    }
    if agent.revision == 0
        || agent.work.next_fence == 0
        || agent.conversation.runtime_generation == Some(0)
    {
        return Err(invalid_agent_state(
            sandbox,
            "sandbox agent has invalid durable authority",
        ));
    }
    Ok(())
}

fn validate_stop_work(sandbox: &Sandbox, agent: &Agent) -> Result<(), SandboxDependencyError> {
    if matches!(
        agent.work.step,
        AgentWorkStep::FenceInteraction
            | AgentWorkStep::CloseProcess
            | AgentWorkStep::PublishStopped
    ) {
        Ok(())
    } else {
        Err(invalid_agent_state(
            sandbox,
            "requested agent stop has no active stop work",
        ))
    }
}

fn validate_control_result(
    sandbox: &Sandbox,
    observed: &Agent,
    result: &Agent,
) -> Result<(), SandboxDependencyError> {
    validate_agent(sandbox, result)?;
    if result.id != observed.id
        || result.worktree_id != observed.worktree_id
        || result.workspace_id != observed.workspace_id
    {
        tracing::error!(
            sandbox_id = %sandbox.id,
            agent_id = %observed.id,
            "agent stop scheduling crossed its durable control target"
        );
        return Err(SandboxDependencyError::Security(
            AGENT_BOUNDARY_INVALID.into(),
        ));
    }
    if !result.status.is_live()
        || result.requested_stop_reason != Some(AgentStopReason::SandboxRetired)
    {
        return Err(invalid_agent_state(
            sandbox,
            "agent stop scheduling returned an invalid state",
        ));
    }
    validate_stop_work(sandbox, result)
}

fn sandbox_store_failure(sandbox: &Sandbox, error: SandboxStoreError) -> SandboxDependencyError {
    tracing::error!(sandbox_id = %sandbox.id, error = %error, "could not read retiring sandbox agents");
    match error {
        SandboxStoreError::Unavailable(_) => {
            SandboxDependencyError::Transient(AGENT_STORE_UNAVAILABLE.into())
        }
        SandboxStoreError::NotFound
        | SandboxStoreError::Conflict
        | SandboxStoreError::SequenceConflict => {
            SandboxDependencyError::Transient(AGENT_STATE_CHANGED.into())
        }
        _ => SandboxDependencyError::Validation(AGENT_STATE_INVALID.into()),
    }
}

fn control_store_failure(
    sandbox: &Sandbox,
    agent: &Agent,
    error: AgentControlStoreError,
) -> SandboxDependencyError {
    tracing::error!(
        sandbox_id = %sandbox.id,
        agent_id = %agent.id,
        error = %error,
        "could not schedule retiring sandbox agent stop"
    );
    match error {
        AgentControlStoreError::Unavailable(_) => {
            SandboxDependencyError::Transient(AGENT_STORE_UNAVAILABLE.into())
        }
        AgentControlStoreError::NotFound
        | AgentControlStoreError::StatusConflict { .. }
        | AgentControlStoreError::StopReasonConflict
        | AgentControlStoreError::WorktreeRecoveryConflict
        | AgentControlStoreError::LimitReached { .. }
        | AgentControlStoreError::Conflict => {
            SandboxDependencyError::Transient(AGENT_STATE_CHANGED.into())
        }
        AgentControlStoreError::WorkConflict { .. } | AgentControlStoreError::Corrupt(_) => {
            SandboxDependencyError::Validation(AGENT_STATE_INVALID.into())
        }
    }
}

fn invalid_agent_state(sandbox: &Sandbox, internal: &'static str) -> SandboxDependencyError {
    tracing::error!(sandbox_id = %sandbox.id, detail = internal, "retiring sandbox agent state is invalid");
    SandboxDependencyError::Validation(AGENT_STATE_INVALID.into())
}

#[cfg(test)]
#[path = "live_agent_stopper_tests.rs"]
mod tests;
