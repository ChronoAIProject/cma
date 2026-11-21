//! Image-drift selection and identity-preserving rotation bookkeeping.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, SandboxId, SandboxProfileId, SubjectId, WorkspaceId, WorktreeId};
use crate::seconds_from_rfc3339;

use super::{Agent, Sandbox, SandboxCheckpointRef, SandboxModelError, SandboxStatus};

/// Select only a live, bound sandbox whose actual image differs from deployment intent.
pub fn image_rotation_required(sandbox: &Sandbox, expected_runtime_version: &str) -> bool {
    matches!(
        sandbox.status,
        SandboxStatus::Active | SandboxStatus::Paused
    ) && !expected_runtime_version.trim().is_empty()
        && sandbox.retained_runtime_binding().is_some_and(|runtime| {
            !runtime.runtime_id.trim().is_empty()
                && !runtime.runtime_version.trim().is_empty()
                && seconds_from_rfc3339(&runtime.bound_at).is_some()
                && runtime.runtime_version != expected_runtime_version
        })
}

/// Stable lineage that must survive replacement of the runtime itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxRotationAgentIdentity {
    pub agent_id: AgentId,
    pub workspace_id: WorkspaceId,
    pub worktree_id: WorktreeId,
    pub conversation_id: String,
}

/// Sandbox, profile, and live-agent identities held across a rotation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxRotationIdentity {
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub profile_id: SandboxProfileId,
    pub profile_revision: u32,
    pub profile_digest: String,
    pub agents: Vec<SandboxRotationAgentIdentity>,
}

impl SandboxRotationIdentity {
    pub fn validate(&self) -> Result<(), SandboxModelError> {
        let mut agent_ids = BTreeSet::new();
        let mut conversation_ids = BTreeSet::new();
        let mut previous_agent: Option<&AgentId> = None;
        if self.sandbox_id.as_str().trim().is_empty()
            || self.owner.as_str().trim().is_empty()
            || self.profile_id.as_str().trim().is_empty()
            || self.profile_revision == 0
            || self.profile_digest.len() != 64
            || !self
                .profile_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid_rotation_identity());
        }
        for agent in &self.agents {
            if agent.agent_id.as_str().trim().is_empty()
                || agent.workspace_id.as_str().trim().is_empty()
                || agent.worktree_id.as_str().trim().is_empty()
                || agent.conversation_id.trim().is_empty()
                || agent.conversation_id.len() > 512
                || previous_agent.is_some_and(|previous| previous >= &agent.agent_id)
                || !agent_ids.insert(&agent.agent_id)
                || !conversation_ids.insert(agent.conversation_id.as_str())
            {
                return Err(invalid_rotation_identity());
            }
            previous_agent = Some(&agent.agent_id);
        }
        Ok(())
    }

    /// Capture canonical identity without retaining prompts, logs, or credentials.
    pub fn capture(sandbox: &Sandbox, agents: &[Agent]) -> Result<Self, SandboxModelError> {
        let mut identities = Vec::new();
        let mut agent_ids = BTreeSet::new();
        let mut conversation_ids = BTreeSet::new();
        for agent in agents {
            if agent.sandbox_id != sandbox.id {
                return Err(invalid_rotation_identity());
            }
            if !agent.status.is_live() {
                continue;
            }
            let (Some(conversation_id), Some(generation)) = (
                agent.conversation.conversation_id.as_deref(),
                agent.conversation.runtime_generation,
            ) else {
                return Err(invalid_rotation_identity());
            };
            if conversation_id.trim().is_empty()
                || conversation_id.len() > 512
                || generation == 0
                || !agent_ids.insert(&agent.id)
                || !conversation_ids.insert(conversation_id)
            {
                return Err(invalid_rotation_identity());
            }
            identities.push(SandboxRotationAgentIdentity {
                agent_id: agent.id.clone(),
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.worktree_id.clone(),
                conversation_id: conversation_id.into(),
            });
        }
        identities.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        let identity = Self {
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            profile_id: sandbox.profile.id.clone(),
            profile_revision: sandbox.profile.revision,
            profile_digest: sandbox.profile.digest.clone(),
            agents: identities,
        };
        identity.validate()?;
        Ok(identity)
    }

    /// Verify that replacement changed runtime authority but no durable identity.
    pub fn preserved_by(&self, sandbox: &Sandbox, agents: &[Agent]) -> bool {
        Self::capture(sandbox, agents).as_ref() == Ok(self)
    }
}

/// Persistable rotation counters; the checkpoint time is the idempotency boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxRotationAccounting {
    pub rotations: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotated_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxRotationAccountingOutcome {
    AlreadyAccounted,
    Accounted { rotations: u32 },
}

impl SandboxRotationAccounting {
    pub fn validate(&self) -> Result<(), SandboxModelError> {
        if (self.rotations == 0) != self.rotated_at.is_none()
            || self
                .rotated_at
                .as_deref()
                .is_some_and(|value| seconds_from_rfc3339(value).is_none())
        {
            return Err(invalid_rotation_accounting());
        }
        Ok(())
    }

    /// Count a completed checkpoint boundary exactly once across worker replay.
    pub fn account(
        &mut self,
        checkpoint: &SandboxCheckpointRef,
    ) -> Result<SandboxRotationAccountingOutcome, SandboxModelError> {
        self.validate()?;
        checkpoint.validate()?;
        self.account_at(&checkpoint.sealed_at)
    }

    pub(super) fn account_at(
        &mut self,
        boundary: &str,
    ) -> Result<SandboxRotationAccountingOutcome, SandboxModelError> {
        self.validate()?;
        if seconds_from_rfc3339(boundary).is_none() {
            return Err(invalid_rotation_accounting());
        }
        if self.rotated_at.as_deref() == Some(boundary) {
            return Ok(SandboxRotationAccountingOutcome::AlreadyAccounted);
        }
        if self
            .rotated_at
            .as_deref()
            .is_some_and(|previous| seconds_from_rfc3339(previous) > seconds_from_rfc3339(boundary))
        {
            return Err(invalid_rotation_accounting());
        }
        self.rotations = self
            .rotations
            .checked_add(1)
            .ok_or_else(invalid_rotation_accounting)?;
        self.rotated_at = Some(boundary.into());
        Ok(SandboxRotationAccountingOutcome::Accounted {
            rotations: self.rotations,
        })
    }
}

const fn invalid_rotation_identity() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.rotation.identity")
}

const fn invalid_rotation_accounting() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.rotation.accounting")
}
