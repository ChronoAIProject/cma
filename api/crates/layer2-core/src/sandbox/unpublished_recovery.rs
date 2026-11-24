//! Admission for replacing a failed runtime that never published its conversations.
//!
//! CMA generation fences prove that it admitted no turn into this incarnation.
//! They do not prove native idleness: a provider may resume persisted work on
//! reopening. Admission preserves its inspected source and durable quiescence
//! plan; the worker must finish that plan before replacing the old runtime.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::store::SandboxStoreError;
use super::{
    Agent, AgentStatus, FirstMessageDelivery, QueuedMessageState, RuntimeBinding, Sandbox,
    SandboxEventKind, SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxReviveCause,
    SandboxRevivePhase, SandboxStatus, SandboxWorkStep, Workspace, WorkspaceWorkStep,
};
use crate::{seconds_from_rfc3339, AgentId, SandboxId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxUnpublishedRecoverySnapshot {
    pub sandbox: Sandbox,
    /// Complete set, including stopped/deleted Agents; never only the live page.
    pub agents: Vec<Agent>,
    pub workspaces: Vec<Workspace>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxRecoveryConversationProof {
    pub agent_id: AgentId,
    pub conversation_id: String,
    /// Digest of the exact original conversation history observed by the provider.
    pub history_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxUnpublishedRecoveryProof {
    pub sandbox_id: SandboxId,
    pub runtime_id: String,
    pub runtime_generation: u64,
    /// Exact uncompleted allocation whose provider authority was independently
    /// proved. The retained source generation and binding remain unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserved_provision: Option<super::SandboxRuntimeProvisionReservation>,
    /// Current allocation independently verified against the exact retained home
    /// and rendered request when older code lost its original operation record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_provision_generation: Option<u64>,
    pub home_identity: String,
    pub boot_id: Option<String>,
    /// Fresh provider evidence proves the old runtime cannot execute. This is
    /// required for the empty-Agent case with unfinished Workspace preparation.
    pub runtime_process_absent: bool,
    /// Always true: a separate durable receipt is required before suspension.
    pub requires_native_quiescence: bool,
    /// Opaque provider process/source inspection, never interpreted by core.
    pub provider_plan: serde_json::Value,
    pub provider_plan_sha256: String,
    pub conversations: Vec<SandboxRecoveryConversationProof>,
}

/// Only a trusted composition adapter issues this proof. HTTP input must never
/// supply it. Verify the original native graph/home and same boot around all
/// history reads and exact conversation identity. The provider plan must permit
/// replayable graceful completion without discarding history or user files.
/// A missing/corrupt history or unprovable process identity is a refusal.
#[async_trait::async_trait]
pub trait SandboxUnpublishedRecoveryVerifier: Send + Sync {
    async fn verify(
        &self,
        snapshot: &SandboxUnpublishedRecoverySnapshot,
    ) -> Result<SandboxUnpublishedRecoveryProof, SandboxStoreError>;
}

#[derive(Debug, Clone)]
pub struct SandboxUnpublishedRecoveryRequest {
    pub snapshot: SandboxUnpublishedRecoverySnapshot,
    pub proof: SandboxUnpublishedRecoveryProof,
    pub target_runtime_version: String,
    pub released_runtime: Option<layer3_adapter::ReleasedRuntime>,
    pub automatic: bool,
    pub requested_at: String,
}

#[derive(Debug, Clone)]
pub struct SandboxUnpublishedRecoveryMutation {
    pub sandbox: Sandbox,
    pub event: SandboxEventKind,
}

impl Sandbox {
    /// Exact persisted source; a stalled revival keeps its source suspended even
    /// when the provider has not completed the previous generation reservation.
    pub fn unpublished_recovery_binding(&self) -> Option<&RuntimeBinding> {
        self.runtime.as_ref().or_else(|| {
            if self.unpublished_allocation_source() {
                self.lifecycle.suspended_runtime.as_deref()
            } else {
                None
            }
        })
    }

    pub(super) fn stalled_unpublished_revival(&self) -> bool {
        self.status == SandboxStatus::Reviving
            && self.runtime.is_none()
            && self.work.step == SandboxWorkStep::RunLifecycleOperation
            && self.work.attempts > 0
            && matches!(
                self.lifecycle.operation,
                Some(SandboxLifecycleOperation::Revive {
                    phase: SandboxRevivePhase::RuntimeProvisioning,
                    cause: SandboxReviveCause::RuntimeLost,
                    ..
                })
            )
            && self
                .lifecycle
                .runtime_provision
                .as_ref()
                .is_some_and(|reservation| {
                    self.lifecycle
                        .suspended_runtime
                        .as_ref()
                        .and_then(|binding| binding.provision_generation)
                        .is_some_and(|source| {
                            source > 0
                                && source < reservation.generation
                                && self
                                    .retained_home_recovery()
                                    .is_some_and(|home| home.source_generation == source)
                        })
                        && reservation.generation == self.lifecycle.runtime_generation_high_water
                })
            && self.lifecycle.unpublished_recovery.is_none()
            && !self.lifecycle.unpublished_recovery_attempted
    }

    fn stalled_unpublished_reopen(&self) -> bool {
        self.runtime.is_some()
            && self.work.step == SandboxWorkStep::RunLifecycleOperation
            && self.work.attempts > 0
            && matches!(
                (&self.status, &self.lifecycle.operation),
                (
                    SandboxStatus::Reviving,
                    Some(SandboxLifecycleOperation::Revive {
                        phase: SandboxRevivePhase::ConversationReopen,
                        ..
                    })
                ) | (
                    SandboxStatus::Rotating,
                    Some(SandboxLifecycleOperation::ImageRotation {
                        phase: SandboxImageRotationPhase::ConversationReopen,
                        ..
                    })
                )
            )
    }

    /// An automatic repair cannot supersede an owner's queued or started update.
    pub(super) fn automatic_unpublished_recovery_target(&self) -> Option<&str> {
        self.runtime_update_request()
            .filter(|request| request.completed_at.is_none())
            .map(|request| request.target_version.as_str())
            .or_else(|| self.admitted_runtime_update_target())
            .or_else(|| {
                self.unpublished_recovery_binding()
                    .map(|binding| binding.runtime_version.as_str())
            })
    }

    /// Cheap preselection only; complete child and native proof is still required.
    pub fn unpublished_recovery_candidate(&self) -> bool {
        if !(self.supports_retained_runtime_update()
            || self.supports_retained_custom_runtime_recovery())
            || self.unpublished_recovery_binding().is_none()
            || self.work.lease.is_some()
            || (self.work.step != SandboxWorkStep::None
                && !self.stalled_unpublished_revival()
                && !self.stalled_unpublished_reopen())
            || self.lifecycle.checkpoint_capture.is_some()
            || (self.lifecycle.runtime_provision.is_some() && !self.stalled_unpublished_revival())
            || self
                .lifecycle
                .unpublished_recovery
                .as_ref()
                .is_some_and(|recovery| {
                    recovery.quiescence.is_none()
                        || self
                            .runtime
                            .as_ref()
                            .and_then(|binding| binding.provision_generation)
                            .is_none_or(|generation| {
                                generation <= recovery.proof.runtime_generation
                            })
                })
            || self.credential.current.is_none()
        {
            return false;
        }
        // A persisted retry at reopen can stall indefinitely without a parked
        // update failure. The lease guard above admits only between workers;
        // complete child/log and native proofs still decide whether it is safe.
        if self.unpublished_allocation_source() || self.stalled_unpublished_reopen() {
            return true;
        }
        match &self.lifecycle.operation {
            Some(SandboxLifecycleOperation::ImageRotation {
                phase: SandboxImageRotationPhase::ConversationReopen,
                ..
            })
            | Some(SandboxLifecycleOperation::Revive {
                phase: SandboxRevivePhase::ConversationReopen,
                ..
            }) => self
                .runtime_update_request()
                .is_some_and(|request| request.completed_at.is_none() && request.failure.is_some()),
            None => {
                self.status == SandboxStatus::Failed
                    && self.failure.as_ref().is_some_and(|failure| {
                        matches!(
                            failure.class.as_str(),
                            "restore_failed" | "provisioning_failed"
                        )
                    })
            }
            _ => false,
        }
    }
}

impl SandboxUnpublishedRecoverySnapshot {
    pub fn home_identity(&self) -> Result<String, SandboxStoreError> {
        self.sandbox
            .lifecycle
            .home
            .as_ref()
            .filter(|home| !home.released)
            .and_then(|home| home.attachment())
            .ok_or_else(refused)?
            .identity()
            .map_err(|_| refused())
    }

    pub fn runtime_generation(&self) -> Result<u64, SandboxStoreError> {
        self.sandbox
            .unpublished_recovery_binding()
            .and_then(|binding| binding.provision_generation)
            .filter(|generation| *generation > 0)
            .ok_or_else(refused)
    }

    /// Validate before any provider read, and again in the admission transaction.
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        self.sandbox.validate().map_err(|_| refused())?;
        if !self.sandbox.unpublished_recovery_candidate()
            || (self.runtime_generation()? != self.sandbox.lifecycle.runtime_generation_high_water
                && !self.sandbox.unpublished_allocation_source())
            || (self.sandbox.unpublished_allocation_source() && !self.agents.is_empty())
        {
            return Err(refused());
        }
        self.home_identity()?;
        let generation = self.runtime_generation()?;
        let mut ids = BTreeSet::new();
        for agent in &self.agents {
            agent.validate_conversation().map_err(|_| refused())?;
            // Deletion may retain an interrupted turn or delivery record as
            // history. Only completed deletion with no remaining obligations
            // makes that history incapable of dispatch; the work, operation and
            // runtime-publication fences below still apply to every Agent.
            let deleted = agent.status == AgentStatus::Deleted;
            if !ids.insert(&agent.id)
                || agent.sandbox_id != self.sandbox.id
                || agent.owner != self.sandbox.owner
                || agent.work.step.is_pending()
                || agent.work.lease.is_some()
                || (deleted
                    && (agent
                        .deleted_at
                        .as_deref()
                        .and_then(seconds_from_rfc3339)
                        .is_none()
                        || !agent.work.cleanup_debt.is_empty()
                        || agent.requested_stop_reason.is_some()
                        || !agent.conversation.queued_messages.is_empty()))
                || agent
                    .conversation
                    .runtime_generation
                    .is_some_and(|old| old >= generation)
                || agent
                    .conversation
                    .operation
                    .as_ref()
                    .is_some_and(|operation| {
                        operation.runtime_generation >= generation || operation.blocks_dispatch()
                    })
                || agent
                    .conversation
                    .first_message
                    .as_ref()
                    .is_some_and(|first| {
                        // dispatch_generation is an Agent work-authority fence,
                        // not a Sandbox runtime generation.
                        !deleted
                            && !first.is_terminal()
                            && first.state != FirstMessageDelivery::Queued
                    })
                || agent
                    .conversation
                    .queued_messages
                    .iter()
                    .any(|message| message.state != QueuedMessageState::Queued)
            {
                return Err(refused());
            }
            // Runtime loss clears process receipts, including OpenConversation.
            // The original conversation identity, complete publication-log scan
            // in the store, and native proof authorize reopening it instead.
            if agent.status.is_live()
                && (agent.status != AgentStatus::Paused
                    || agent.conversation.active_turn.is_some()
                    || agent.requested_stop_reason.is_some()
                    || agent.conversation.runtime_generation.is_none()
                    || agent
                        .conversation
                        .conversation_id
                        .as_ref()
                        .is_none_or(|id| id.trim().is_empty()))
            {
                return Err(refused());
            }
        }
        let mut workspace_ids = BTreeSet::new();
        for workspace in &self.workspaces {
            workspace
                .validate_for_admission(&self.sandbox, self.sandbox.status)
                .map_err(|_| refused())?;
            if !workspace_ids.insert(&workspace.id)
                || workspace.sandbox_id != self.sandbox.id
                || workspace.owner != self.sandbox.owner
                || workspace.work.lease.is_some()
                || (workspace.work.step.is_pending()
                    && !(self.agents.is_empty()
                        && workspace.work.step == WorkspaceWorkStep::Prepare))
            {
                return Err(refused());
            }
        }
        // An early provisioning failure is only recoverable without any Agent.
        if !self.agents.is_empty()
            && self
                .sandbox
                .failure
                .as_ref()
                .is_some_and(|failure| failure.class == "provisioning_failed")
        {
            return Err(refused());
        }
        Ok(())
    }
}

impl SandboxUnpublishedRecoveryProof {
    pub fn validate_for(
        &self,
        snapshot: &SandboxUnpublishedRecoverySnapshot,
    ) -> Result<(), SandboxStoreError> {
        snapshot.validate()?;
        self.validate_provider_plan().map_err(|_| refused())?;
        self.validate_reserved_provision(snapshot)?;
        let runtime = snapshot
            .sandbox
            .unpublished_recovery_binding()
            .ok_or_else(refused)?;
        if self.sandbox_id != snapshot.sandbox.id
            || self.runtime_id != runtime.runtime_id
            || self.runtime_generation != snapshot.runtime_generation()?
            || self.home_identity != snapshot.home_identity()?
            || self.boot_id.as_ref().is_some_and(|boot| !hex(boot, 32))
            || (!snapshot.agents.is_empty() && self.boot_id.is_none())
            || (self.boot_id.is_none() && !self.runtime_process_absent)
            || (snapshot.sandbox.unpublished_allocation_source() && self.boot_id.is_some())
            || self
                .boot_id
                .as_ref()
                .is_some_and(|boot| runtime.boot_id.as_ref().is_some_and(|held| held != boot))
            || (snapshot
                .workspaces
                .iter()
                .any(|workspace| workspace.work.step.is_pending())
                && !self.runtime_process_absent)
        {
            return Err(refused());
        }
        let expected = snapshot
            .agents
            .iter()
            .filter(|agent| agent.status.is_live())
            .map(|agent| {
                (
                    &agent.id,
                    agent
                        .conversation
                        .conversation_id
                        .as_deref()
                        .unwrap_or_default(),
                )
            })
            .collect::<BTreeSet<_>>();
        let actual = self
            .conversations
            .iter()
            .map(|proof| (&proof.agent_id, proof.conversation_id.as_str()))
            .collect::<BTreeSet<_>>();
        if actual != expected
            || actual.len() != self.conversations.len()
            || self
                .conversations
                .iter()
                .any(|proof| !hex(&proof.history_sha256, 64))
        {
            return Err(refused());
        }
        Ok(())
    }
}

/// Store implementations must read/fence the complete child sets and compare the
/// current snapshot with the verified one in the same transaction as this write.
pub fn apply_unpublished_runtime_recovery(
    current: &SandboxUnpublishedRecoverySnapshot,
    request: &SandboxUnpublishedRecoveryRequest,
) -> Result<SandboxUnpublishedRecoveryMutation, SandboxStoreError> {
    if current != &request.snapshot {
        return Err(SandboxStoreError::Conflict);
    }
    request.proof.validate_for(current)?;
    if request.automatic
        && (current.sandbox.legacy_unpublished_allocation()
            || current.sandbox.lifecycle.unpublished_recovery_attempted
            || current.sandbox.lifecycle.unpublished_recovery.is_some()
            || current.sandbox.automatic_unpublished_recovery_target()
                != Some(request.target_runtime_version.as_str()))
    {
        return Err(refused());
    }
    let now = crate::seconds_from_rfc3339(&request.requested_at).ok_or_else(refused)?;
    if crate::seconds_from_rfc3339(&current.sandbox.updated_at).is_none_or(|at| at > now)
        || request.target_runtime_version.trim().is_empty()
        || request.target_runtime_version.len() > 512
        || request.target_runtime_version.chars().any(char::is_control)
    {
        return Err(refused());
    }
    let sandbox = current
        .sandbox
        .begin_unpublished_runtime_recovery(
            &current.agents,
            &request.proof,
            &request.target_runtime_version,
            request.automatic,
            &request.requested_at,
        )
        .map_err(|_| refused())?;
    Ok(SandboxUnpublishedRecoveryMutation {
        sandbox,
        event: SandboxEventKind::RuntimeRecoveryAdmitted {
            previous_status: current.sandbox.status,
            target_runtime_version: request.target_runtime_version.clone(),
            proof: Box::new(request.proof.clone()),
        },
    })
}

fn hex(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn refused() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction("unpublished runtime recovery proof is not complete")
}

#[cfg(test)]
#[path = "unpublished_recovery_tests.rs"]
mod tests;
