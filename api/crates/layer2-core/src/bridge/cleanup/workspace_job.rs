//! Retained cleanup scheduling evidence. These serializable records supply no
//! runtime permission; each effect needs fresh authority and exact Workspace proof.
use super::*;
use crate::resource_access::{ResourceActor, ResourceOwner};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCleanupJobAdmission {
    pub schema_version: u16,
    pub receipt: WorkspaceCleanupReceipt,
    pub owner: ResourceOwner,
    pub issuer: String,
    pub allocation_sha256: String,
    pub request_sha256: String,
    pub child_removal_sha256: String,
    pub workspace_creation_sha256: String,
    pub digest: String,
}
impl WorkspaceCleanupJobAdmission {
    pub fn compute_digest(&self) -> Result<String, CleanupError> {
        digest(&(
            self.schema_version,
            &self.receipt,
            &self.owner,
            &self.issuer,
            &self.allocation_sha256,
            &self.request_sha256,
            &self.child_removal_sha256,
            &self.workspace_creation_sha256,
        ))
    }
    pub fn validate(&self) -> Result<(), CleanupError> {
        let receipt = &self.receipt;
        receipt
            .request
            .validate()
            .map_err(|_| CleanupError::Corrupt)?;
        if self.schema_version != 1
            || !receipt.id.strip_prefix("bwcl_").is_some_and(hash)
            || !allocation_id(&receipt.allocation_id)
            || !(1..=256).contains(&receipt.generation)
            || receipt.accepted_at_ms < 0
            || !matches!(
                receipt.requested_by,
                ResourceActor::Human { .. }
                    | ResourceActor::Api {
                        delegation_id: None,
                        ..
                    }
            )
            || self.issuer.is_empty()
            || self.issuer.len() > 2048
            || !hash(&self.allocation_sha256)
            || !hash(&self.workspace_creation_sha256)
            || !hash(&self.child_removal_sha256)
            || self.request_sha256 != digest(&receipt.request)?
            || self.digest != self.compute_digest()?
        {
            return Err(CleanupError::Corrupt);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCleanupJobClaim {
    pub holder: String,
    pub fence: u64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCleanupJob {
    pub admission: WorkspaceCleanupJobAdmission,
    pub revision: u64,
    pub lease_fence: u64,
    pub lease: Option<WorkspaceCleanupJobClaim>,
    pub progress: WorkspaceCleanupProgress,
    pub holds: Vec<WorkspaceCleanupHold>,
    pub observed_at_ms: i64,
    pub updated_at_ms: i64,
    pub next_attempt_at_ms: i64,
}
impl WorkspaceCleanupJob {
    pub fn new(
        admission: WorkspaceCleanupJobAdmission,
        holds: Vec<WorkspaceCleanupHold>,
        observed_at_ms: i64,
    ) -> Result<Self, CleanupError> {
        let now = admission.receipt.accepted_at_ms;
        let value = Self {
            admission,
            revision: 1,
            lease_fence: 0,
            lease: None,
            progress: WorkspaceCleanupProgress::Requested,
            holds,
            observed_at_ms,
            updated_at_ms: now,
            next_attempt_at_ms: now,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<(), CleanupError> {
        self.admission.validate()?;
        if self.revision == 0
            || self.revision > i64::MAX as u64
            || self.lease_fence > i64::MAX as u64
            || self.progress == WorkspaceCleanupProgress::NotRequested
            || self.updated_at_ms < self.admission.receipt.accepted_at_ms
            || self.next_attempt_at_ms < self.admission.receipt.accepted_at_ms
            || self.observed_at_ms < 0
            || self.observed_at_ms > self.updated_at_ms
            || self.holds.windows(2).any(|pair| pair[0] >= pair[1])
            || (self.progress == WorkspaceCleanupProgress::Complete
                && (self.lease.is_some() || !self.holds.is_empty()))
            || self.lease.as_ref().is_some_and(|lease| {
                lease.holder.is_empty()
                    || lease.holder.len() > 200
                    || lease.fence != self.lease_fence
                    || lease.fence == 0
                    || lease.expires_at_ms <= self.updated_at_ms
            })
        {
            return Err(CleanupError::Corrupt);
        }
        Ok(())
    }
    pub fn claim(
        &mut self,
        holder: &str,
        now: i64,
        expires_at_ms: i64,
    ) -> Result<(), CleanupError> {
        self.validate()?;
        if holder.is_empty()
            || holder.len() > 200
            || expires_at_ms <= now
            || now < self.updated_at_ms
            || now < self.next_attempt_at_ms
            || self.progress == WorkspaceCleanupProgress::Complete
            || self
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at_ms > now)
        {
            return Err(CleanupError::Conflict);
        }
        let mut next = self.clone();
        next.lease_fence = next
            .lease_fence
            .checked_add(1)
            .ok_or(CleanupError::Corrupt)?;
        next.revision = next.revision.checked_add(1).ok_or(CleanupError::Corrupt)?;
        next.lease = Some(WorkspaceCleanupJobClaim {
            holder: holder.into(),
            fence: next.lease_fence,
            expires_at_ms,
        });
        next.updated_at_ms = now;
        next.validate()?;
        *self = next;
        Ok(())
    }
    pub fn require_claim(
        &self,
        claim: &WorkspaceCleanupJobClaim,
        now: i64,
    ) -> Result<(), CleanupError> {
        self.validate()?;
        if self.lease.as_ref() != Some(claim)
            || now >= claim.expires_at_ms
            || now < self.updated_at_ms
        {
            return Err(CleanupError::Conflict);
        }
        Ok(())
    }
    /// Called only after persistence has proved the matching stage outcome.
    /// Unknown effects retain their runtime identity independently of this lease.
    pub fn observe(
        &mut self,
        claim: &WorkspaceCleanupJobClaim,
        progress: WorkspaceCleanupProgress,
        mut holds: Vec<WorkspaceCleanupHold>,
        now: i64,
    ) -> Result<(), CleanupError> {
        self.require_claim(claim, now)?;
        if progress == WorkspaceCleanupProgress::NotRequested
            || rank(progress) < rank(self.progress)
        {
            return Err(CleanupError::Conflict);
        }
        holds.sort();
        holds.dedup();
        let mut next = self.clone();
        next.progress = progress;
        next.holds = holds;
        next.next_attempt_at_ms = now
            .checked_add(if next.holds.is_empty() { 250 } else { 5000 })
            .ok_or(CleanupError::Corrupt)?;
        next.lease = None;
        next.revision = next.revision.checked_add(1).ok_or(CleanupError::Corrupt)?;
        next.updated_at_ms = now;
        next.observed_at_ms = now;
        next.validate()?;
        *self = next;
        Ok(())
    }
    /// Persistence calls this only after proving retained Workspace state. This can
    /// catch up a lost publication without obtaining permission for a new effect.
    pub fn reconcile_progress(
        &mut self,
        claim: &WorkspaceCleanupJobClaim,
        progress: WorkspaceCleanupProgress,
        now: i64,
    ) -> Result<(), CleanupError> {
        self.require_claim(claim, now)?;
        if progress < self.progress {
            return Err(CleanupError::Corrupt);
        }
        if progress == self.progress {
            return Ok(());
        }
        let mut next = self.clone();
        next.progress = progress;
        next.revision = next.revision.checked_add(1).ok_or(CleanupError::Corrupt)?;
        next.updated_at_ms = now;
        // Catch-up proves stage progress, not a fresh nonterminal hold scan.
        // Preserve the original holds' observation time until advance observes.
        if progress == WorkspaceCleanupProgress::Complete {
            next.observed_at_ms = now;
            next.lease = None;
            next.holds.clear();
        }
        next.validate()?;
        *self = next;
        Ok(())
    }
    pub fn status(
        &self,
        workspace_revision: u64,
        runtime_supported: bool,
    ) -> Result<WorkspaceCleanupStatus, CleanupError> {
        self.validate()?;
        let receipt = &self.admission.receipt;
        Ok(WorkspaceCleanupStatus {
            bridge_id: receipt.bridge_id.clone(),
            allocation_id: receipt.allocation_id.clone(),
            generation: receipt.generation,
            allocation_revision: receipt.request.expected_allocation_revision,
            target: receipt.target.clone(),
            workspace_revision: Some(workspace_revision),
            child_cleanup_receipt_id: Some(receipt.request.child_cleanup_receipt_id.clone()),
            eligible: false,
            runtime_supported,
            receipt: Some(receipt.clone()),
            progress: self.progress,
            holds: self.holds.clone(),
            observed_at_ms: Some(self.observed_at_ms),
            workspace_capacity_released: self.progress == WorkspaceCleanupProgress::Complete,
            parent_retained: true,
            history_retained: true,
        })
    }
}

fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn rank(progress: WorkspaceCleanupProgress) -> u8 {
    match progress {
        WorkspaceCleanupProgress::NotRequested => 0,
        WorkspaceCleanupProgress::Requested => 1,
        WorkspaceCleanupProgress::AdmissionClosed => 2,
        WorkspaceCleanupProgress::RemovalStarted => 3,
        WorkspaceCleanupProgress::RuntimeRemoved => 4,
        WorkspaceCleanupProgress::Complete => 5,
    }
}
