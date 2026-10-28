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
    Agent, AgentStatus, AgentWorkStep, FirstMessageDelivery, QueuedMessageState, Sandbox,
    SandboxEventKind, SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxRevivePhase,
    SandboxStatus, SandboxWorkStep, Workspace, WorkspaceWorkStep,
};
use crate::{AgentId, SandboxId};

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
    pub automatic: bool,
    pub requested_at: String,
}

#[derive(Debug, Clone)]
pub struct SandboxUnpublishedRecoveryMutation {
    pub sandbox: Sandbox,
    pub event: SandboxEventKind,
}

impl Sandbox {
    /// Cheap preselection only; complete child and native proof is still required.
    pub fn unpublished_recovery_candidate(&self) -> bool {
        if !self.supports_retained_runtime_update()
            || self.runtime.is_none()
            || self.work.lease.is_some()
            || self.work.step != SandboxWorkStep::None
            || self.lifecycle.checkpoint_capture.is_some()
            || self.lifecycle.runtime_provision.is_some()
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
            .runtime
            .as_ref()
            .and_then(|binding| binding.provision_generation)
            .filter(|generation| *generation > 0)
            .ok_or_else(refused)
    }

    /// Validate before any provider read, and again in the admission transaction.
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        self.sandbox.validate().map_err(|_| refused())?;
        if !self.sandbox.unpublished_recovery_candidate()
            || self.runtime_generation()? != self.sandbox.lifecycle.runtime_generation_high_water
        {
            return Err(refused());
        }
        self.home_identity()?;
        let generation = self.runtime_generation()?;
        let mut ids = BTreeSet::new();
        for agent in &self.agents {
            agent.validate_conversation().map_err(|_| refused())?;
            if !ids.insert(&agent.id)
                || agent.sandbox_id != self.sandbox.id
                || agent.owner != self.sandbox.owner
                || agent.work.step.is_pending()
                || agent.work.lease.is_some()
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
                        first.dispatch_generation >= generation
                            || (!first.is_terminal() && first.state != FirstMessageDelivery::Queued)
                    })
                || agent
                    .conversation
                    .queued_messages
                    .iter()
                    .any(|message| message.state != QueuedMessageState::Queued)
            {
                return Err(refused());
            }
            if agent.status.is_live()
                && (agent.status != AgentStatus::Paused
                    || agent.conversation.active_turn.is_some()
                    || agent.requested_stop_reason.is_some()
                    || agent.conversation.runtime_generation.is_none()
                    || agent
                        .conversation
                        .conversation_id
                        .as_ref()
                        .is_none_or(|id| id.trim().is_empty())
                    || !agent
                        .work
                        .receipts
                        .iter()
                        .any(|receipt| receipt.step == AgentWorkStep::OpenConversation))
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
        let runtime = snapshot.sandbox.runtime.as_ref().ok_or_else(refused)?;
        if self.sandbox_id != snapshot.sandbox.id
            || self.runtime_id != runtime.runtime_id
            || self.runtime_generation != snapshot.runtime_generation()?
            || self.home_identity != snapshot.home_identity()?
            || self.boot_id.as_ref().is_some_and(|boot| !hex(boot, 32))
            || (!snapshot.agents.is_empty() && self.boot_id.is_none())
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
