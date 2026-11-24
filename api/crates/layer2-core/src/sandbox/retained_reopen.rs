//! Owner-authorized continuation on an already restored retained runtime.
//! This transition never provisions, suspends, terminates, or updates an image.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    store::SandboxStoreError, AgentOperationDelivery, AgentStatus, RetainedRecoverySnapshot,
    RuntimeBinding, Sandbox, SandboxEventKind, SandboxLifecycleOperation, SandboxReviveCause,
    SandboxRevivePhase, SandboxStatus, SandboxWorkStep, WorkspaceStatus, Worktree, WorktreeStatus,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainedReopenSnapshot {
    pub retained: RetainedRecoverySnapshot,
    pub worktrees: Vec<Worktree>,
    pub event_seq: u64,
    /// Digest of the complete audited durable history before native inspection.
    pub history_state_sha256: String,
    /// Complete terminal command inventory, hashed by the store after decoding.
    pub command_state_sha256: String,
    pub command_lanes: Vec<(crate::AgentId, super::agent_io::CommandLane)>,
}

impl RetainedReopenSnapshot {
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        let source = self.retained.source().map_err(|_| refused())?;
        self.validate_common()?;
        for agent in &self.retained.agents {
            if agent.status != AgentStatus::Paused
                || agent
                    .conversation
                    .runtime_generation
                    .is_none_or(|old| old == 0 || old >= source.generation)
                || agent
                    .conversation
                    .operation
                    .as_ref()
                    .is_some_and(|operation| {
                        operation.runtime_generation >= source.generation
                            || !matches!(
                                operation.state,
                                AgentOperationDelivery::ExecutionObserved
                                    | AgentOperationDelivery::PreWriteFailed
                                    | AgentOperationDelivery::RuntimeLost
                            )
                    })
                || agent
                    .work
                    .live_skills
                    .iter()
                    .any(|skill| skill.status == super::AgentSkillInstallStatus::Pending)
                || agent.failure.is_some()
            {
                return Err(refused());
            }
        }
        Ok(())
    }

    pub(super) fn validate_common(&self) -> Result<(), SandboxStoreError> {
        let sandbox = &self.retained.sandbox;
        if self.retained.agents.is_empty()
            || self.retained.agents.len() > 16
            || self.retained.workspaces.len() > 16
            || self.worktrees.len() > 16
            || !sandbox.credential.delivered
            || sandbox.credential.pending.is_some()
            || sandbox.lifecycle.suspended_runtime.is_some()
            || sandbox.lifecycle.unpublished_recovery_attempted
            || sandbox
                .runtime_update_request()
                .is_some_and(|r| r.completed_at.is_none())
        {
            return Err(refused());
        }
        let agent_ids = self
            .retained
            .agents
            .iter()
            .map(|a| &a.id)
            .collect::<BTreeSet<_>>();
        let lane_ids = self
            .command_lanes
            .iter()
            .map(|(id, _)| id)
            .collect::<BTreeSet<_>>();
        if !hex(&self.history_state_sha256, 64)
            || !hex(&self.command_state_sha256, 64)
            || agent_ids != lane_ids
            || lane_ids.len() != self.command_lanes.len()
            || self
                .command_lanes
                .iter()
                .any(|(_, lane)| lane.lease.is_some() || lane.next_fence == 0)
        {
            return Err(refused());
        }
        let mut trees = BTreeSet::new();
        for tree in &self.worktrees {
            let workspace = self
                .retained
                .workspaces
                .iter()
                .find(|workspace| workspace.id == tree.workspace_id)
                .ok_or_else(refused)?;
            tree.validate_prerequisites(workspace)
                .map_err(|_| refused())?;
            if !trees.insert(&tree.id) || tree.status != WorktreeStatus::Ready {
                return Err(refused());
            }
        }
        for workspace in &self.retained.workspaces {
            if workspace.status != WorkspaceStatus::Ready {
                return Err(refused());
            }
        }
        for agent in &self.retained.agents {
            let tree = self
                .worktrees
                .iter()
                .find(|tree| tree.id == agent.worktree_id)
                .ok_or_else(refused)?;
            agent.validate_prerequisites(tree).map_err(|_| refused())?;
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, SandboxStoreError> {
        digest(self)
    }
}

/// Provider evidence is produced internally, never decoded from an HTTP request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedReopenProof {
    pub expected_runtime: RuntimeBinding,
    pub observed_boot: String,
    pub home_identity: String,
    pub provider_plan: serde_json::Value,
    pub provider_plan_sha256: String,
    pub histories: Vec<layer3_adapter::RecoveryConversationHistory>,
}

impl RetainedReopenProof {
    pub fn validate(&self, snapshot: &RetainedReopenSnapshot) -> Result<(), SandboxStoreError> {
        snapshot.validate()?;
        let source = snapshot.retained.source().map_err(|_| refused())?;
        let expected = snapshot
            .retained
            .agents
            .iter()
            .map(|a| {
                (
                    a.id.as_str(),
                    a.conversation
                        .conversation_id
                        .as_deref()
                        .unwrap_or_default(),
                )
            })
            .collect::<BTreeSet<_>>();
        let actual = self
            .histories
            .iter()
            .map(|h| (h.agent_id.as_str(), h.conversation_id.as_str()))
            .collect::<BTreeSet<_>>();
        if self.expected_runtime != *source.binding
            || self.home_identity != source.home_identity
            || !hex(&self.observed_boot, 32)
            || expected != actual
            || actual.len() != self.histories.len()
            || self.histories.iter().any(|h| !hex(&h.history_sha256, 64))
            || super::SandboxUnpublishedRecoveryProof::provider_plan_digest(&self.provider_plan)
                .map_err(|_| refused())?
                != self.provider_plan_sha256
        {
            return Err(refused());
        }
        Ok(())
    }
}

#[async_trait::async_trait]
pub trait RetainedReopenVerifier: Send + Sync {
    async fn verify(
        &self,
        snapshot: &RetainedReopenSnapshot,
    ) -> Result<RetainedReopenProof, SandboxStoreError>;

    async fn verify_suspended(
        &self,
        _snapshot: &RetainedReopenSnapshot,
    ) -> Result<super::RetainedSuspendedReopenProof, SandboxStoreError> {
        Err(refused())
    }
}

#[derive(Debug, Clone)]
pub struct RetainedReopenRequest {
    pub snapshot: RetainedReopenSnapshot,
    pub proof: RetainedReopenProof,
    pub key_sha256: String,
    pub requested_at: String,
    pub repeat_authorized: bool,
}

/// Immutable accepted response, independent of subsequent lifecycle progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedReopenAccepted {
    pub operation_id: String,
    pub revision: u64,
    pub accepted_at: String,
}

pub struct RetainedReopenReplay {
    pub owner: crate::SubjectId,
    pub expected_revision: u64,
    pub key_sha256: String,
    pub accepted: RetainedReopenAccepted,
}

#[derive(Default)]
pub struct RetainedReopenHistory {
    /// Present only when the parent and receipt lookup used one durable snapshot.
    pub observed_parent: Option<(crate::SubjectId, u64)>,
    pub latest_revision: Option<u64>,
    pub replay: Option<RetainedReopenReplay>,
}

/// Stored outside the serialized Sandbox record. Old readers ignore this receipt;
/// old $set aggregate saves preserve it. It never authorizes an old worker effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedReopenReceipt {
    pub version: u32,
    pub owner: crate::SubjectId,
    pub sandbox_id: crate::SandboxId,
    pub expected_revision: u64,
    pub key_sha256: String,
    pub snapshot_sha256: String,
    pub result_sha256: String,
    pub original_failure: super::TypedFailure,
    pub proof: RetainedReopenProof,
    pub accepted: RetainedReopenAccepted,
}

impl From<RetainedReopenReceipt> for RetainedReopenReplay {
    fn from(receipt: RetainedReopenReceipt) -> Self {
        Self {
            owner: receipt.owner,
            expected_revision: receipt.expected_revision,
            key_sha256: receipt.key_sha256,
            accepted: receipt.accepted,
        }
    }
}

pub fn apply_retained_reopen(
    current: &RetainedReopenSnapshot,
    request: &RetainedReopenRequest,
) -> Result<(Sandbox, RetainedReopenReceipt, SandboxEventKind), SandboxStoreError> {
    if current != &request.snapshot || !hex(&request.key_sha256, 64) {
        return Err(SandboxStoreError::Conflict);
    }
    request.proof.validate(current)?;
    let previous = &current.retained.sandbox;
    let now = crate::seconds_from_rfc3339(&request.requested_at).ok_or_else(refused)?;
    if crate::seconds_from_rfc3339(&previous.updated_at).is_none_or(|at| at > now) {
        return Err(refused());
    }
    let mut next = previous.clone();
    // The provider generation is already restored and newer than all published
    // conversations. Start only the existing reopen/queue/publication suffix.
    next.lifecycle
        .start_revive(
            &next.id,
            request.requested_at.clone(),
            SandboxReviveCause::RuntimeLost,
        )
        .map_err(|_| refused())?;
    let Some(SandboxLifecycleOperation::Revive {
        phase, authority, ..
    }) = &mut next.lifecycle.operation
    else {
        return Err(refused());
    };
    *phase = SandboxRevivePhase::ConversationReopen;
    let operation_id = authority.operation_id.clone();
    next.runtime.as_mut().ok_or_else(refused)?.boot_id = Some(request.proof.observed_boot.clone());
    next.status = SandboxStatus::Reviving;
    next.failure = None;
    next.lifecycle.paused_notice =
        Some("Reopening your original conversations on the same saved environment.".into());
    next.work.schedule(
        SandboxWorkStep::RunLifecycleOperation,
        request.requested_at.clone(),
    );
    next.updated_at = request.requested_at.clone();
    next.revision = next.revision.checked_add(1).ok_or_else(refused)?;
    next.validate().map_err(|_| {
        SandboxStoreError::InvalidTransaction("retained reopening result is invalid")
    })?;
    let receipt = RetainedReopenReceipt {
        version: 1,
        owner: previous.owner.clone(),
        sandbox_id: previous.id.clone(),
        expected_revision: previous.revision,
        key_sha256: request.key_sha256.clone(),
        snapshot_sha256: current.digest()?,
        result_sha256: digest(&next)?,
        original_failure: previous.failure.clone().ok_or_else(refused)?,
        proof: request.proof.clone(),
        accepted: RetainedReopenAccepted {
            operation_id,
            revision: next.revision,
            accepted_at: request.requested_at.clone(),
        },
    };
    Ok((
        next,
        receipt,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Failed,
            to: SandboxStatus::Reviving,
        },
    ))
}

pub(super) fn digest(value: &impl Serialize) -> Result<String, SandboxStoreError> {
    let bytes = serde_json::to_vec(value).map_err(|_| refused())?;
    if bytes.len() > 1024 * 1024 {
        return Err(refused());
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn hex(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn refused() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction("retained conversation reopening is not proved")
}
