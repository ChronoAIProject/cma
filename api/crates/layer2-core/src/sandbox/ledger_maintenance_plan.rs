//! Bounded original maintenance plans and complete admission snapshots.

use crate::sandbox::agent_io::CommandLane;
use crate::sandbox::{Agent, Sandbox, SandboxHierarchyRecord, Workspace, Worktree};
use crate::{seconds_from_rfc3339, AgentId};
use layer3_adapter::runtime_ledger_maintenance::{valid_sha256, LedgerBindingV1};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_LEDGER_MAINTENANCE_PLAN_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_LEDGER_MAINTENANCE_RESOURCES: usize = 16_384;

/// None means absence proved by the authoritative transaction, not lookup failure.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerMaintenanceLaneV1 {
    pub agent_id: AgentId,
    pub lane: Option<CommandLane>,
}

/// Complete one-home records, including non-live children. Full values preserve
/// work leases and input state which a synthetic "lane revision" would omit.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerMaintenanceSnapshotV1 {
    pub sandbox: Sandbox,
    pub sandbox_event_head: u64,
    pub agents: Vec<Agent>,
    pub workspaces: Vec<Workspace>,
    pub worktrees: Vec<Worktree>,
    pub command_lanes: Vec<LedgerMaintenanceLaneV1>,
}

impl LedgerMaintenanceSnapshotV1 {
    pub fn validate_for(
        &self,
        binding: &LedgerBindingV1,
        at: &str,
    ) -> Result<(), LedgerMaintenancePlanRefusal> {
        binding.validate().map_err(|_| invalid())?;
        if self.agents.len() + self.workspaces.len() + self.worktrees.len()
            > MAX_LEDGER_MAINTENANCE_RESOURCES
            || self.command_lanes.len() != self.agents.len()
            || !ordered(self.agents.iter().map(|row| row.id.as_str()))
            || !ordered(self.workspaces.iter().map(|row| row.id.as_str()))
            || !ordered(self.worktrees.iter().map(|row| row.id.as_str()))
            || !ordered(self.command_lanes.iter().map(|row| row.agent_id.as_str()))
        {
            return Err(invalid());
        }
        // Reuse the existing bounded complete-hierarchy identity validation.
        // The resulting observation is not maintenance or deactivation authority.
        let observation = SandboxHierarchyRecord {
            sandbox: self.sandbox.clone(),
            agents: self.agents.clone(),
            workspaces: self.workspaces.clone(),
            worktrees: self.worktrees.clone(),
        }
        .ledger_retirement_observation(at.into(), binding.candidate_sha256.clone())
        .map_err(|_| invalid())?;
        if observation.sandbox_id != binding.sandbox_id
            || observation.home_identity != binding.home_identity
            || observation.generation != binding.generation
            || observation.sandbox_revision != binding.sandbox_revision
            || self
                .sandbox
                .retained_runtime_binding()
                .and_then(|runtime| runtime.boot_id.as_ref())
                != Some(&binding.boot_id)
        {
            return Err(invalid());
        }
        for (agent, row) in self.agents.iter().zip(&self.command_lanes) {
            if agent.id != row.agent_id {
                return Err(invalid());
            }
            if let Some(lane) = &row.lane {
                if lane.last_order == 0
                    || lane.next_fence == 0
                    || lane.lease.as_ref().is_some_and(|lease| {
                        lease.claim.agent_id != agent.id
                            || lease.claim.fence == 0
                            || lease.claim.fence >= lane.next_fence
                            || seconds_from_rfc3339(&lease.expires_at).is_none()
                    })
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerMaintenancePlanV1 {
    pub format: String,
    pub binding: LedgerBindingV1,
    pub source: LedgerMaintenanceSnapshotV1,
    pub consent_receipt_sha256: String,
    pub requested_at: String,
}

impl LedgerMaintenancePlanV1 {
    pub fn validate(&self) -> Result<(), LedgerMaintenancePlanRefusal> {
        if self.format != "ledger-maintenance-plan-v1"
            || !valid_sha256(&self.consent_receipt_sha256)
            || seconds_from_rfc3339(&self.requested_at).is_none()
        {
            return Err(invalid());
        }
        self.source.validate_for(&self.binding, &self.requested_at)
    }
}

/// Accepted bytes are immutable. Retry reads this artifact; it never recaptures
/// a snapshot or serializes the decoded plan and calls it the accepted request.
#[derive(Clone)]
pub struct LedgerMaintenancePlanArtifactV1 {
    bytes: Vec<u8>,
    sha256: String,
    plan: LedgerMaintenancePlanV1,
}

impl LedgerMaintenancePlanArtifactV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, LedgerMaintenancePlanRefusal> {
        if bytes.len() > MAX_LEDGER_MAINTENANCE_PLAN_BYTES {
            return Err(LedgerMaintenancePlanRefusal::Oversized);
        }
        let plan: LedgerMaintenancePlanV1 = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        plan.validate()?;
        Ok(Self {
            bytes: bytes.to_vec(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            plan,
        })
    }

    pub fn original_bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub fn plan(&self) -> &LedgerMaintenancePlanV1 {
        &self.plan
    }
}

impl std::fmt::Debug for LedgerMaintenancePlanArtifactV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedgerMaintenancePlanArtifactV1")
            .field("bytes", &self.bytes.len())
            .field("sha256", &self.sha256)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LedgerMaintenancePlanRefusal {
    #[error("ledger maintenance plan exceeds the supported bound")]
    Oversized,
    #[error("ledger maintenance plan or complete source binding is invalid")]
    InvalidEvidence,
}

fn invalid() -> LedgerMaintenancePlanRefusal {
    LedgerMaintenancePlanRefusal::InvalidEvidence
}

fn ordered<'a>(values: impl Iterator<Item = &'a str>) -> bool {
    let mut last = None;
    for value in values {
        if value.is_empty() || last.is_some_and(|previous| previous >= value) {
            return false;
        }
        last = Some(value);
    }
    true
}
