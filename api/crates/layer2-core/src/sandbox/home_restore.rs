//! Reader preparation for explicit home replacement. No restore admission or writer.
use serde::{Deserialize, Serialize};

use super::{SandboxHome, SandboxModelError};
use crate::{seconds_from_rfc3339, AgentId, SubjectId, WorkspaceId, WorktreeId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxHomeRestoreEvidenceKind {
    Gone,
    Deleting,
    Unbound,
    Present,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxHomeRestoreEvidence {
    pub kind: SandboxHomeRestoreEvidenceKind,
    pub observed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_storage_generation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxHomeRestoreAttestation {
    pub storage_generation: String,
}

/// The pin survives worker handoff; completion retains the original evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxHomeRestore {
    pub restore_id: String,
    pub backup_id: String,
    pub operation_id: String,
    pub requested_at: String,
    pub requested_by: SubjectId,
    pub key_sha256: String,
    pub evidence: SandboxHomeRestoreEvidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_existing_home: Option<SandboxHomeRestoreAttestation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<SandboxHomeRestoreReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxHomeRestoreReceipt {
    pub inventory_sha256: String,
    pub restored_to: String,
    pub completed_at: String,
    pub home_identity: String,
    pub source_generation: u64,
    pub covered_agents: Vec<AgentId>,
    pub uncovered_agents: Vec<AgentId>,
    pub missing_workspaces: Vec<WorkspaceId>,
    pub missing_worktrees: Vec<WorktreeId>,
    pub removed_agent_homes: Vec<AgentId>,
    pub removed_worktrees: Vec<WorktreeId>,
}

/// Retain the full old allocation authority for later reviewed disposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxSupersededHome {
    pub restore_id: String,
    pub home: Box<SandboxHome>,
    pub home_identity: String,
    pub source_generation: u64,
    pub superseded_at: String,
}

impl SandboxHomeRestore {
    pub fn validate(&self) -> Result<(), SandboxModelError> {
        if [&self.restore_id, &self.backup_id, &self.operation_id]
            .into_iter()
            .any(|value| !identifier(value))
            || !identifier(self.requested_by.as_str())
            || !digest(&self.key_sha256)
            || seconds_from_rfc3339(&self.requested_at).is_none()
            || seconds_from_rfc3339(&self.evidence.observed_at).is_none()
            || self
                .evidence
                .observed_storage_generation
                .as_ref()
                .is_some_and(|v| !identifier(v))
            || self
                .replace_existing_home
                .as_ref()
                .is_some_and(|v| !identifier(&v.storage_generation))
        {
            return Err(invalid());
        }
        if matches!(
            self.evidence.kind,
            SandboxHomeRestoreEvidenceKind::Present | SandboxHomeRestoreEvidenceKind::Unbound
        ) && self
            .replace_existing_home
            .as_ref()
            .zip(self.evidence.observed_storage_generation.as_deref())
            .is_none_or(|(attestation, observed)| attestation.storage_generation != observed)
        {
            return Err(invalid());
        }
        if let Some(receipt) = &self.receipt {
            receipt.validate()?;
        }
        Ok(())
    }
}

impl SandboxHomeRestoreReceipt {
    fn validate(&self) -> Result<(), SandboxModelError> {
        if !digest(&self.inventory_sha256)
            || !identifier(&self.home_identity)
            || self.source_generation == 0
            || seconds_from_rfc3339(&self.restored_to).is_none()
            || seconds_from_rfc3339(&self.completed_at).is_none()
            || self
                .covered_agents
                .iter()
                .any(|id| self.uncovered_agents.contains(id))
            || !unique(&self.covered_agents)
            || !unique(&self.uncovered_agents)
            || !unique(&self.missing_workspaces)
            || !unique(&self.missing_worktrees)
            || !unique(&self.removed_agent_homes)
            || !unique(&self.removed_worktrees)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl super::SandboxLifecycleState {
    pub(super) fn validate_home_restore(
        &self,
        sandbox: &super::Sandbox,
    ) -> Result<(), SandboxModelError> {
        if let Some(restore) = &self.home_restore {
            restore.validate()?;
        }
        let current = self.home.as_ref().and_then(|home| home.attachment());
        for (index, old) in self.superseded_homes.iter().enumerate() {
            old.home.validate(&sandbox.id)?;
            let attachment = layer3_adapter::HomeVolumeAttachment {
                request: old.home.request.clone(),
                receipt: old.home.receipt.clone().ok_or_else(invalid)?,
            };
            if !identifier(&old.restore_id)
                || old.source_generation == 0
                || old.source_generation > self.runtime_generation_high_water
                || seconds_from_rfc3339(&old.superseded_at).is_none()
                || attachment.identity().map_err(|_| invalid())? != old.home_identity
                || current.as_ref().is_some_and(|home| {
                    home.request.scope == attachment.request.scope
                        && home.receipt.storage_reference == attachment.receipt.storage_reference
                        && home.receipt.storage_generation == attachment.receipt.storage_generation
                })
                || self.superseded_homes[..index]
                    .iter()
                    .any(|other| other.home_identity == old.home_identity)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

fn unique<T: PartialEq + std::fmt::Display>(values: &[T]) -> bool {
    values
        .iter()
        .enumerate()
        .all(|(i, value)| identifier(&value.to_string()) && !values[..i].contains(value))
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_hexdigit() && !v.is_ascii_uppercase())
}

fn invalid() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.home_restore")
}

#[cfg(test)]
#[path = "home_restore_tests.rs"]
mod tests;
