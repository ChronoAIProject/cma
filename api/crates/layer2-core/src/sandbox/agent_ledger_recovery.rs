//! Private, owner-authorized recovery plans. Structural validity is not eligibility.
//! No plan changes a lifecycle, replays input, or grants permission to remove a home.

use crate::ids::{AgentId, SandboxId};
use layer3_adapter::runtime_ledger_maintenance::{valid_sha256, LedgerBindingV1};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const LEDGER_RECOVERY_PLAN_FORMAT: &str = "ledger-recovery-plan-v1";
pub const MAX_LEDGER_RECOVERY_PLAN_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerRecoveryDispositionV1 {
    PreserveHeld,
    MigrateToNewResources,
    SameAgentRetry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerRecoveryPhaseV1 {
    Accepted,
    Salvaged,
    SuccessorPrepared,
    HeldVerified,
    Completed,
    AbortedHeld,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerRecoverySuccessorV1 {
    pub sandbox_id: SandboxId,
    pub agent_id: AgentId,
}

/// One immutable logical recovery request. Takeover keeps its original bytes,
/// operation identity, input disposition and consent. Progress is a separate receipt.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerRecoveryPlanV1 {
    pub format: String,
    pub operation_id: String,
    pub binding: LedgerBindingV1,
    pub source_agent_id: AgentId,
    pub source_agent_revision: u64,
    pub source_log_head: u64,
    pub source_command_lane_revision: u64,
    pub failure_evidence_sha256: String,
    pub attempt_evidence_sha256: String,
    pub disposition: LedgerRecoveryDispositionV1,
    pub backup_sha256: String,
    pub salvage_sha256: String,
    pub source_revisions_sha256: String,
    pub consent_receipt_sha256: String,
    pub successor: Option<LedgerRecoverySuccessorV1>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LedgerRecoveryRefusal {
    #[error("ledger recovery plan exceeds the supported byte bound")]
    Oversized,
    #[error("ledger recovery plan evidence is invalid")]
    InvalidEvidence,
    #[error("ledger recovery request differs from its accepted bytes")]
    RequestChanged,
}

impl LedgerRecoveryPlanV1 {
    pub fn validate(&self) -> Result<(), LedgerRecoveryRefusal> {
        if self.format != LEDGER_RECOVERY_PLAN_FORMAT
            || !identity(&self.operation_id)
            || self.binding.validate().is_err()
            || !identity(self.source_agent_id.as_str())
            || self.source_agent_revision == 0
            || self.source_command_lane_revision == 0
            || [
                &self.failure_evidence_sha256,
                &self.attempt_evidence_sha256,
                &self.backup_sha256,
                &self.salvage_sha256,
                &self.source_revisions_sha256,
                &self.consent_receipt_sha256,
            ]
            .into_iter()
            .any(|hash| !valid_sha256(hash))
        {
            return Err(LedgerRecoveryRefusal::InvalidEvidence);
        }
        match (&self.disposition, &self.successor) {
            (LedgerRecoveryDispositionV1::MigrateToNewResources, Some(next))
                if identity(next.sandbox_id.as_str())
                    && identity(next.agent_id.as_str())
                    && next.sandbox_id.as_str() != self.binding.sandbox_id
                    && next.agent_id != self.source_agent_id => {}
            (
                LedgerRecoveryDispositionV1::PreserveHeld
                | LedgerRecoveryDispositionV1::SameAgentRetry,
                None,
            ) => {}
            _ => return Err(LedgerRecoveryRefusal::InvalidEvidence),
        }
        Ok(())
    }
}

impl std::fmt::Debug for LedgerRecoveryPlanV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedgerRecoveryPlanV1")
            .field("binding", &self.binding)
            .field("disposition", &self.disposition)
            .finish()
    }
}

/// Exact accepted bytes for durable put/readback before recording acceptance.
/// This value does not claim the bytes have already been persisted.
#[derive(Clone)]
pub struct LedgerRecoveryArtifactV1 {
    bytes: Vec<u8>,
    sha256: String,
    plan: LedgerRecoveryPlanV1,
}

impl LedgerRecoveryArtifactV1 {
    pub fn parse(bytes: &[u8]) -> Result<Self, LedgerRecoveryRefusal> {
        if bytes.len() > MAX_LEDGER_RECOVERY_PLAN_BYTES {
            return Err(LedgerRecoveryRefusal::Oversized);
        }
        let plan: LedgerRecoveryPlanV1 =
            serde_json::from_slice(bytes).map_err(|_| LedgerRecoveryRefusal::InvalidEvidence)?;
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
    pub fn plan(&self) -> &LedgerRecoveryPlanV1 {
        &self.plan
    }

    /// Same logical ID with new bytes is a conflict, even with equivalent JSON.
    pub fn validate_retry(&self, original: &Self) -> Result<(), LedgerRecoveryRefusal> {
        if self.bytes != original.bytes {
            return Err(LedgerRecoveryRefusal::RequestChanged);
        }
        Ok(())
    }
}

impl std::fmt::Debug for LedgerRecoveryArtifactV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedgerRecoveryArtifactV1")
            .field("bytes", &self.bytes.len())
            .field("sha256", &self.sha256)
            .finish()
    }
}

/// Durable progress references the exact accepted plan and never replaces it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerRecoveryReceiptV1 {
    pub format: String,
    pub plan_sha256: String,
    pub record_revision: u64,
    pub maintenance_epoch: u64,
    pub phase: LedgerRecoveryPhaseV1,
    pub outcome_sha256: String,
}

impl std::fmt::Debug for LedgerRecoveryReceiptV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LedgerRecoveryReceiptV1")
            .field("record_revision", &self.record_revision)
            .field("maintenance_epoch", &self.maintenance_epoch)
            .field("phase", &self.phase)
            .finish_non_exhaustive()
    }
}

impl LedgerRecoveryReceiptV1 {
    pub fn validate_for(
        &self,
        accepted: &LedgerRecoveryArtifactV1,
    ) -> Result<(), LedgerRecoveryRefusal> {
        if self.format != "ledger-recovery-receipt-v1"
            || self.plan_sha256 != accepted.sha256()
            || self.record_revision == 0
            || self.maintenance_epoch < accepted.plan().binding.maintenance_epoch
            || !valid_sha256(&self.outcome_sha256)
        {
            return Err(LedgerRecoveryRefusal::InvalidEvidence);
        }
        Ok(())
    }
}

fn identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
#[path = "agent_ledger_recovery_tests.rs"]
mod tests;
