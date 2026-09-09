use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;

use crate::ids::{AgentId, SandboxId, SubjectId};
use crate::sandbox::{
    Agent, AgentLogKind, RuntimeBinding, Sandbox, SandboxCheckpointRef, SandboxEventKind,
    SandboxLifecycleOperation, SandboxStatus, SandboxWorkLease, SandboxWorkStep,
};
use crate::seconds_from_rfc3339;

use super::lifecycle_agent_validation::validate_lifecycle_agent_phase;
use super::lifecycle_execution_validation::{
    checkpoint_sandbox_advanced, lifecycle_sandbox_advanced,
};
use super::{
    Page, PageRequest, SandboxEventStore, SandboxStore, SandboxStoreError, SandboxTransactionStore,
    SandboxWorkerStore, MAX_AGENT_LOG_BATCH_SIZE,
};

pub const MAX_SANDBOX_LIFECYCLE_AGENTS: usize = 256;
pub const MAX_SANDBOX_LIFECYCLE_EVENTS: usize = MAX_SANDBOX_LIFECYCLE_AGENTS + 1;

/// Exact persisted authority a lifecycle side effect was observed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxMutationGuard {
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub expected_revision: u64,
    pub status: SandboxStatus,
    pub work_step: SandboxWorkStep,
    pub lease: Option<SandboxWorkLease>,
    pub operation: Option<SandboxLifecycleOperation>,
    pub runtime: Option<RuntimeBinding>,
    pub checkpoint_capture: Option<crate::sandbox::SandboxCheckpointCapture>,
}

impl SandboxMutationGuard {
    pub fn capture(sandbox: &Sandbox) -> Self {
        Self {
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            expected_revision: sandbox.revision,
            status: sandbox.status,
            work_step: sandbox.work.step,
            lease: sandbox.work.lease.clone(),
            operation: sandbox.lifecycle.operation.clone(),
            runtime: sandbox.runtime.clone(),
            checkpoint_capture: sandbox.lifecycle.checkpoint_capture.clone(),
        }
    }

    fn matches(&self, sandbox: &Sandbox) -> bool {
        self.sandbox_id == sandbox.id
            && self.owner == sandbox.owner
            && self.expected_revision == sandbox.revision
            && self.status == sandbox.status
            && self.work_step == sandbox.work.step
            && self.lease == sandbox.work.lease
            && self.operation == sandbox.lifecycle.operation
            && self.runtime == sandbox.runtime
            && self.checkpoint_capture == sandbox.lifecycle.checkpoint_capture
    }
}

/// Atomic publication of one complete whole-sandbox checkpoint.
#[derive(Debug, Clone)]
pub struct SandboxCheckpointMutation {
    pub guard: SandboxMutationGuard,
    pub agents: Vec<SandboxCheckpointAgentGuard>,
    /// A checkpoint read from an immutable snapshot may finish after live
    /// Agent rows advance. Such publication fences the current Sandbox only;
    /// the captured Agent set belongs to the snapshot and is intentionally not
    /// compared with today's rows.
    pub nonblocking: bool,
    pub checkpoint: SandboxCheckpointRef,
    pub sandbox: Sandbox,
    pub appended_at: String,
}

/// The exact durable conversation position observed during checkpoint capture.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SandboxCheckpointAgentGuard {
    pub agent_id: AgentId,
    pub revision: u64,
    pub last_seq: u64,
}

impl SandboxCheckpointAgentGuard {
    pub fn capture(agent: &Agent) -> Self {
        Self {
            agent_id: agent.id.clone(),
            revision: agent.revision,
            last_seq: agent.conversation.last_seq,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxLifecycleAgentScope {
    Touched,
    AllLive,
    RestoreParticipants,
}

/// One agent row and its gap-free log additions in a sandbox lifecycle commit.
#[derive(Debug, Clone)]
pub struct SandboxLifecycleAgentMutation {
    pub expected_revision: u64,
    pub agent: Agent,
    pub logs: Vec<AgentLogKind>,
}

/// One all-or-nothing sandbox phase publication, optionally with agent rows.
#[derive(Debug, Clone)]
pub struct SandboxLifecycleMutation {
    pub guard: SandboxMutationGuard,
    pub sandbox: Sandbox,
    pub agents: Vec<SandboxLifecycleAgentMutation>,
    pub agent_scope: SandboxLifecycleAgentScope,
    pub appended_at: String,
    pub events: Vec<SandboxEventKind>,
}

#[async_trait]
pub trait SandboxLifecycleExecutionStore: Send + Sync {
    async fn sandbox_agents(
        &self,
        sandbox_id: &SandboxId,
        page: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError>;

    async fn publish_sandbox_checkpoint(
        &self,
        mutation: SandboxCheckpointMutation,
    ) -> Result<(), SandboxStoreError>;

    async fn commit_sandbox_lifecycle(
        &self,
        mutation: SandboxLifecycleMutation,
    ) -> Result<(), SandboxStoreError>;
}

pub trait SandboxLifecycleStore:
    SandboxStore
    + super::WorkspaceStore
    + super::WorktreeStore
    + SandboxEventStore
    + SandboxTransactionStore
    + SandboxWorkerStore
    + SandboxLifecycleExecutionStore
{
}

impl<T> SandboxLifecycleStore for T where
    T: SandboxStore
        + super::WorkspaceStore
        + super::WorktreeStore
        + SandboxEventStore
        + SandboxTransactionStore
        + SandboxWorkerStore
        + SandboxLifecycleExecutionStore
{
}

#[doc(hidden)]
pub fn validate_checkpoint_mutation(
    current: &Sandbox,
    agents: &[(Agent, u64)],
    mutation: &SandboxCheckpointMutation,
) -> Result<(), SandboxStoreError> {
    validate_guard(current, &mutation.guard)?;
    let eligible = match current.lifecycle.operation.as_ref() {
        None => current.status == SandboxStatus::Active && current.work.lease.is_none(),
        Some(SandboxLifecycleOperation::Pause {
            phase: crate::sandbox::SandboxPausePhase::CaptureAndPublishCheckpoint,
            ..
        })
        | Some(SandboxLifecycleOperation::ImageRotation {
            phase: crate::sandbox::SandboxImageRotationPhase::CaptureAndPublishCheckpoint,
            ..
        }) => {
            current.work.step == SandboxWorkStep::RunLifecycleOperation
                && current.work.lease.is_some()
        }
        _ => false,
    };
    if !eligible || current.runtime.is_none() {
        return Err(SandboxStoreError::Conflict);
    }
    validate_next_sandbox(current, &mutation.sandbox, &mutation.appended_at)?;
    mutation
        .checkpoint
        .validate()
        .map_err(|_| invalid("checkpoint reference is invalid"))?;
    if !mutation.nonblocking {
        let live = agents.iter().filter(|(agent, _)| agent.status.is_live());
        let expected = live
            .clone()
            .map(|(agent, _)| SandboxCheckpointAgentGuard::capture(agent))
            .collect::<BTreeSet<_>>();
        let captured = mutation.agents.iter().cloned().collect::<BTreeSet<_>>();
        if live.count() != expected.len()
            || captured.len() != mutation.agents.len()
            || captured != expected
        {
            return Err(SandboxStoreError::Conflict);
        }
        if agents.iter().any(|(agent, last_seq)| {
            agent.status.is_live() && agent.conversation.last_seq != *last_seq
        }) {
            return Err(SandboxStoreError::SequenceConflict);
        }
    } else if !mutation.agents.is_empty() {
        return Err(SandboxStoreError::Conflict);
    }
    if !mutation.nonblocking {
        let rows = agents
            .iter()
            .map(|(agent, _)| agent.clone())
            .collect::<Vec<_>>();
        validate_checkpoint_agents(current, &rows, &mutation.checkpoint)?;
    }

    if !checkpoint_sandbox_advanced(
        current,
        &mutation.sandbox,
        &mutation.checkpoint,
        &mutation.appended_at,
    ) {
        return Err(invalid(
            "checkpoint publication changed unrelated sandbox state",
        ));
    }
    Ok(())
}

#[doc(hidden)]
pub fn validate_lifecycle_mutation(
    current: &Sandbox,
    agents: &[(Agent, u64)],
    mutation: &SandboxLifecycleMutation,
) -> Result<(), SandboxStoreError> {
    validate_guard(current, &mutation.guard)?;
    if current.lifecycle.checkpoint_capture.is_some()
        || mutation.sandbox.lifecycle.checkpoint_capture.is_some()
    {
        return super::checkpoint_capture::validate_capture_mutation(current, agents, mutation);
    }
    validate_next_sandbox(current, &mutation.sandbox, &mutation.appended_at)?;
    if mutation.agents.len() > MAX_SANDBOX_LIFECYCLE_AGENTS
        || mutation.events.is_empty()
        || mutation.events.len() > MAX_SANDBOX_LIFECYCLE_EVENTS
        || !lifecycle_sandbox_advanced(current, &mutation.sandbox, &mutation.appended_at)
    {
        return Err(invalid(
            "sandbox lifecycle mutation is not bounded or ordered",
        ));
    }
    if mutation.sandbox.lifecycle.runtime_provision != current.lifecycle.runtime_provision {
        if let Some(reservation) = mutation.sandbox.lifecycle.runtime_provision.as_ref() {
            if agents.iter().any(|(agent, _)| {
                agent.status.is_live()
                    && agent
                        .conversation
                        .runtime_generation
                        .is_some_and(|generation| generation >= reservation.generation)
            }) {
                return Err(SandboxStoreError::Conflict);
            }
        }
    }

    let current_by_id = agents
        .iter()
        .map(|(agent, last_seq)| (agent.id.clone(), (agent, *last_seq)))
        .collect::<BTreeMap<_, _>>();
    if current_by_id.len() != agents.len()
        || agents
            .iter()
            .any(|(agent, _)| agent.sandbox_id != current.id || agent.owner != current.owner)
    {
        return Err(SandboxStoreError::Corrupt(
            "stored lifecycle agent lineage is inconsistent".into(),
        ));
    }

    let mut touched = BTreeSet::new();
    for change in &mutation.agents {
        if !touched.insert(change.agent.id.clone()) {
            return Err(invalid("lifecycle agent mutation contains a duplicate"));
        }
        let (stored, actual_last_seq) = current_by_id
            .get(&change.agent.id)
            .copied()
            .ok_or(SandboxStoreError::NotFound)?;
        validate_agent_change(
            current,
            stored,
            actual_last_seq,
            change,
            &mutation.appended_at,
            &mutation.events,
        )?;
    }

    if mutation.agent_scope != SandboxLifecycleAgentScope::Touched {
        let live = agents
            .iter()
            .filter(|(agent, _)| match mutation.agent_scope {
                SandboxLifecycleAgentScope::AllLive => agent.status.is_live(),
                SandboxLifecycleAgentScope::RestoreParticipants => current.restores_agent(agent),
                SandboxLifecycleAgentScope::Touched => unreachable!(),
            })
            .map(|(agent, _)| agent.id.clone())
            .collect::<BTreeSet<_>>();
        if live != touched {
            return Err(SandboxStoreError::Conflict);
        }
    }
    validate_lifecycle_agent_phase(current, agents, mutation, &touched)?;
    Ok(())
}

fn validate_guard(
    current: &Sandbox,
    guard: &SandboxMutationGuard,
) -> Result<(), SandboxStoreError> {
    if guard.matches(current) {
        Ok(())
    } else {
        Err(SandboxStoreError::Conflict)
    }
}

fn validate_next_sandbox(
    current: &Sandbox,
    next: &Sandbox,
    appended_at: &str,
) -> Result<(), SandboxStoreError> {
    let next_revision = current
        .revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    if next.id != current.id
        || next.owner != current.owner
        || next.revision != next_revision
        || next.updated_at != appended_at
        || seconds_from_rfc3339(appended_at).is_none()
        || next.name != current.name
        || next.origin != current.origin
        || next.profile != current.profile
        || next.limits != current.limits
        || next.created_at != current.created_at
        || (next.lifecycle.next_authority != current.lifecycle.next_authority
            && !(current.lifecycle.next_authority.checked_add(1)
                == Some(next.lifecycle.next_authority)
                && matches!(
                    current.lifecycle.operation,
                    Some(SandboxLifecycleOperation::Revive { .. })
                )
                && matches!(
                    next.lifecycle.operation,
                    Some(SandboxLifecycleOperation::Pause { .. })
                )))
        || next.work.execution != current.work.execution
    {
        return Err(invalid(
            "sandbox lifecycle mutation changed immutable state",
        ));
    }
    next.validate()
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
}

fn validate_checkpoint_agents(
    sandbox: &Sandbox,
    agents: &[Agent],
    checkpoint: &SandboxCheckpointRef,
) -> Result<(), SandboxStoreError> {
    if agents.len() > MAX_SANDBOX_LIFECYCLE_AGENTS
        || agents
            .iter()
            .any(|agent| agent.sandbox_id != sandbox.id || agent.owner != sandbox.owner)
    {
        return Err(SandboxStoreError::Corrupt(
            "stored checkpoint agent lineage is inconsistent".into(),
        ));
    }
    let live = agents
        .iter()
        .filter(|agent| agent.status.is_live())
        .map(|agent| {
            if agent.conversation.active_turn.is_some()
                || agent
                    .conversation
                    .operation
                    .as_ref()
                    .is_some_and(|operation| operation.blocks_dispatch())
                || agent.work.step.is_pending()
                || agent.work.lease.is_some()
                || agent.requested_stop_reason.is_some()
            {
                return Err(SandboxStoreError::Conflict);
            }
            let conversation_id = agent
                .conversation
                .conversation_id
                .as_ref()
                .ok_or(SandboxStoreError::Conflict)?;
            Ok((agent.id.clone(), conversation_id.clone()))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let captured = checkpoint
        .conversations
        .iter()
        .map(|conversation| {
            (
                conversation.agent_id.clone(),
                conversation.conversation_id.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    if live != captured {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

fn validate_agent_change(
    sandbox: &Sandbox,
    current: &Agent,
    actual_last_seq: u64,
    change: &SandboxLifecycleAgentMutation,
    appended_at: &str,
    events: &[SandboxEventKind],
) -> Result<(), SandboxStoreError> {
    let next = &change.agent;
    let expected_revision = current
        .revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    let expected_last_seq = current
        .conversation
        .last_seq
        .checked_add(u64::try_from(change.logs.len()).map_err(|_| SandboxStoreError::Conflict)?)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    let status_transition = current.status == next.status
        || current.status.transition(next.status).ok() == Some(next.status);
    let mut expected_work = current.work.clone();
    if current.status == crate::sandbox::AgentStatus::Paused
        && next.status == crate::sandbox::AgentStatus::Ready
        && current
            .conversation
            .first_message
            .as_ref()
            .is_some_and(|message| message.state == crate::sandbox::FirstMessageDelivery::Queued)
    {
        expected_work.schedule(
            crate::sandbox::AgentWorkStep::DispatchFirstMessage,
            appended_at.into(),
        );
    }
    if change.logs.len() > MAX_AGENT_LOG_BATCH_SIZE
        || actual_last_seq != current.conversation.last_seq
        || change.expected_revision != current.revision
        || next.revision != expected_revision
        || next.conversation.last_seq != expected_last_seq
        || next.sandbox_id != sandbox.id
        || next.owner != sandbox.owner
        || !current.status.is_live()
        || !next.status.is_live()
        || !status_transition
        || expected_work != next.work
        || current.work.step.is_pending()
        || current.work.lease.is_some()
        || current.requested_stop_reason.is_some()
        || next.updated_at != appended_at
        || !agent_identity_fields_match(current, next)
    {
        return Err(SandboxStoreError::Conflict);
    }
    next.validate_conversation()
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;

    let _ = events;
    Ok(())
}

fn agent_identity_fields_match(current: &Agent, next: &Agent) -> bool {
    current.id == next.id
        && current.worktree_id == next.worktree_id
        && current.workspace_id == next.workspace_id
        && current.creation_key == next.creation_key
        && current.creation_fingerprint == next.creation_fingerprint
        && current.profile == next.profile
        && current.failure == next.failure
        && current.title == next.title
        && current.created_at == next.created_at
        && current.ready_at == next.ready_at
        && current.stopped_at == next.stopped_at
        && current.requested_stop_reason == next.requested_stop_reason
        && current.stop_reason == next.stop_reason
        && current.deleted_at == next.deleted_at
        && current.origin == next.origin
}

const fn invalid(detail: &'static str) -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction(detail)
}
