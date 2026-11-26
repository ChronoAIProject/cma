use async_trait::async_trait;

use crate::ids::AgentId;
use crate::sandbox::{Agent, AgentLogEntry, AgentLogKind, Sandbox, SandboxStatus};

use super::{SandboxMutationGuard, SandboxStoreError};

/// One generation-fenced, all-or-nothing conversation mutation.
#[derive(Debug, Clone)]
pub struct AgentConversationMutation {
    /// Fresh settings-specific policy proof, absent for outcome recording.
    pub settings_fence: Option<std::sync::Arc<crate::sandbox::AgentSettingsFence>>,
    /// Ledger claim bound to the new runtime operation in this same transaction.
    pub command_claim: Option<crate::sandbox::agent_io::CommandClaim>,
    /// Opaque current resource permission; never attached to outcome evidence.
    pub execution_fence: Option<std::sync::Arc<crate::sandbox::agent_io::CommandExecutionFence>>,
    /// Exact immutable Trigger first-message binding and current store proof.
    pub trigger_execution_fence:
        Option<std::sync::Arc<crate::sandbox::agent_io::TriggerExecutionFence>>,
    /// Exact parent authority for newly admitted input; absent for runtime evidence.
    pub admission_guard: Option<SandboxMutationGuard>,
    pub expected_agent_revision: u64,
    pub expected_last_seq: u64,
    pub runtime_generation: u64,
    pub agent: Agent,
    pub appended_at: String,
    pub agent_logs: Vec<AgentLogKind>,
}

impl AgentConversationMutation {
    pub fn is_managed_context_admission(&self) -> bool {
        self.admission_guard.is_some()
            && self
                .agent
                .conversation
                .operation
                .as_ref()
                .is_some_and(|operation| {
                    operation.state == crate::sandbox::AgentOperationDelivery::DispatchFenced
                        && matches!(
                            operation.intent,
                            crate::sandbox::AgentOperationIntent::RefreshContext { .. }
                        )
                })
    }
}

/// Validate the parent read in the same transaction as the Agent mutation. A store
/// must also write-fence this parent when the guard is present.
#[doc(hidden)]
pub fn validate_agent_conversation_admission(
    current: &Agent,
    parent: Option<&Sandbox>,
    mutation: &AgentConversationMutation,
) -> Result<(), SandboxStoreError> {
    if current
        .conversation
        .turn_timeout
        .as_ref()
        .is_some_and(|timeout| timeout.blocks_dispatch())
        && (admits_input(current, mutation)
            || current.conversation.turn_timeout != mutation.agent.conversation.turn_timeout)
    {
        return Err(SandboxStoreError::Conflict);
    }
    crate::sandbox::agent_preferences::validate_preferences_mutation(current, mutation)?;
    crate::sandbox::agent_managed_context::validate_mutation(current, mutation)?;
    crate::sandbox::agent_settings_validation::validate_settings_mutation(current, mutation)?;
    let skill_admission =
        crate::sandbox::agent_live_skills::is_install_admission(current, &mutation.agent);
    if current.work != mutation.agent.work && !skill_admission {
        return Err(SandboxStoreError::Conflict);
    }
    if skill_admission
        && (!matches!(
            parent.map(|p| p.status),
            Some(SandboxStatus::Active | SandboxStatus::CredentialRotating)
        ) || mutation.agent_logs
            != vec![AgentLogKind::SkillInstallationChanged {
                skill_id: mutation
                    .agent
                    .work
                    .live_skills
                    .last()
                    .expect("validated")
                    .reference
                    .id
                    .as_str()
                    .into(),
                status: crate::sandbox::AgentSkillInstallStatus::Pending,
            }])
    {
        return Err(SandboxStoreError::Conflict);
    }
    let admission = skill_admission
        || current.conversation.preferences.title != mutation.agent.conversation.preferences.title
        || admits_input(current, mutation);
    if [
        mutation.settings_fence.is_some(),
        mutation.execution_fence.is_some(),
        mutation.trigger_execution_fence.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count()
        > 1
    {
        return Err(SandboxStoreError::Conflict);
    }
    if (mutation.execution_fence.is_some() || mutation.trigger_execution_fence.is_some())
        && (mutation.command_claim.is_none() || mutation.admission_guard.is_none() || !admission)
    {
        return Err(SandboxStoreError::Conflict);
    }
    if let Some(fence) = &mutation.settings_fence {
        if mutation.command_claim.is_some() || mutation.admission_guard.is_none() || !admission {
            return Err(SandboxStoreError::Conflict);
        }
        fence.validate_mutation(mutation)?;
    }
    let Some(guard) = mutation.admission_guard.as_ref() else {
        return if admission {
            Err(SandboxStoreError::Conflict)
        } else {
            Ok(())
        };
    };
    let parent = parent.ok_or(SandboxStoreError::Conflict)?;
    if !admission
        || parent.id != current.sandbox_id
        || parent.owner != current.owner
        || parent.id != mutation.agent.sandbox_id
        || parent.owner != mutation.agent.owner
    {
        return Err(SandboxStoreError::Conflict);
    }
    if parent.lifecycle.checkpoint_capture.is_some() {
        return Err(SandboxStoreError::CheckpointInProgress);
    }
    // Recheck inside the dispatch transaction, even if a caller observed an
    // active parent before lifecycle work started. Legacy lifecycle queues do
    // not carry a common command claim and retain their existing admission.
    if mutation.command_claim.is_some()
        && !matches!(
            parent.status,
            SandboxStatus::Active | SandboxStatus::CredentialRotating
        )
    {
        return Err(SandboxStoreError::Conflict);
    }
    if *guard != SandboxMutationGuard::capture(parent)
        || !matches!(
            parent.status,
            SandboxStatus::Active
                | SandboxStatus::CredentialRotating
                | SandboxStatus::Pausing
                | SandboxStatus::Paused
                | SandboxStatus::Reviving
                | SandboxStatus::Rotating
        )
    {
        return Err(SandboxStoreError::Conflict);
    }
    parent
        .validate()
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
}

fn admits_input(current: &Agent, mutation: &AgentConversationMutation) -> bool {
    let next = &mutation.agent.conversation;
    let previous = &current.conversation;
    mutation.agent_logs.iter().any(|kind| {
        matches!(
            kind,
            AgentLogKind::OwnerMessage { .. } | AgentLogKind::TurnStarted { .. }
        )
    }) || next.operation.as_ref().is_some_and(|operation| {
        previous
            .operation
            .as_ref()
            .is_none_or(|old| old.sequence != operation.sequence)
    }) || next.queued_messages.iter().any(|message| {
        !previous
            .queued_messages
            .iter()
            .any(|old| old.sequence == message.sequence)
    }) || next.first_message.as_ref().is_some_and(|message| {
        previous.first_message.as_ref().is_none_or(|old| {
            old.attempt != message.attempt || old.recovery_generation != message.recovery_generation
        })
    })
}

/// Durable agent conversation state and its gap-free semantic log.
#[async_trait]
pub trait AgentConversationStore: Send + Sync {
    /// Atomically publish a policy terminal and settle its exact command/lane.
    /// The durable action must remain replayable after checkpoint advancement.
    async fn commit_turn_timeout(
        &self,
        _settlement: &crate::sandbox::turn_timeout::TurnTimeoutSettlement,
    ) -> Result<(), SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "turn policy settlement is unsupported",
        ))
    }

    /// Settle a known-aborted output append without admitting new work. A store
    /// must verify the exact snapshot and normal-capacity refusal transactionally.
    async fn commit_agent_capacity_update(
        &self,
        _settlement: super::AgentConversationCapacitySettlement,
    ) -> Result<crate::sandbox::AgentUpdateOutcome, SandboxStoreError> {
        Err(SandboxStoreError::AgentLogCapacityExceeded)
    }

    /// Record a quiet original conversation under the exact current lifecycle lease.
    /// Implementations without a parent write fence must refuse this operation.
    async fn commit_agent_reopen_update(
        &self,
        _current: Agent,
        _previous: Option<super::AgentMessageCheckpoint>,
        _next: super::AgentMessageCheckpoint,
        _mutation: Option<AgentConversationMutation>,
        _guard: super::AgentReopenGuard,
    ) -> Result<crate::sandbox::AgentUpdateOutcome, SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "durable reopen recording is unsupported",
        ))
    }

    /// Read the aggregate and private checkpoint atomically from one row/lock.
    async fn agent_message_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<Option<super::AgentMessageSnapshot>, SandboxStoreError>;

    async fn agent_message_checkpoint(
        &self,
        id: &AgentId,
    ) -> Result<Option<super::AgentMessageCheckpoint>, SandboxStoreError>;

    /// CAS the private checkpoint and, when present, semantic history in one transaction.
    /// Repeating the exact update ID/fingerprint returns the original bounded outcome.
    async fn commit_agent_message_update(
        &self,
        current: Agent,
        previous: Option<super::AgentMessageCheckpoint>,
        next: super::AgentMessageCheckpoint,
        mutation: Option<AgentConversationMutation>,
    ) -> Result<crate::sandbox::AgentUpdateOutcome, SandboxStoreError>;

    async fn commit_agent_conversation(
        &self,
        mutation: AgentConversationMutation,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError>;

    async fn pending_agent_approvals(
        &self,
        id: &AgentId,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError>;
}
