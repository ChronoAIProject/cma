//! Exact member input for a reviewed Sandbox retirement batch.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::store::valid_admission_fingerprint;
use super::{RuntimeBinding, Sandbox, SandboxAgentActivity, SandboxHome, SandboxStatus};
use crate::{seconds_from_rfc3339, SandboxId, SubjectId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedRetirementMember {
    pub batch_sha256: String,
    pub member_sha256: String,
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub created_at: String,
    pub revision: u64,
    pub status: SandboxStatus,
    pub last_activity_at: String,
    pub idle_before: String,
    pub runtime: RuntimeBinding,
    pub home: Box<SandboxHome>,
}

impl ReviewedRetirementMember {
    pub fn fingerprint(&self) -> Result<String, super::store::SandboxStoreError> {
        let bytes = serde_json::to_vec(self).map_err(|_| {
            super::store::SandboxStoreError::Corrupt("invalid reviewed retirement guard".into())
        })?;
        let mut hash = Sha256::new();
        hash.update(b"cma-reviewed-retirement-v1\0");
        hash.update(bytes);
        Ok(format!("{:x}", hash.finalize()))
    }

    /// Check the approved member before collecting transactional child evidence.
    pub fn validate(&self, now: &str) -> bool {
        let Some((activity, cutoff, now)) = seconds_from_rfc3339(&self.last_activity_at)
            .zip(seconds_from_rfc3339(&self.idle_before))
            .zip(seconds_from_rfc3339(now))
            .map(|((activity, cutoff), now)| (activity, cutoff, now))
        else {
            return false;
        };
        let image_pinned = self
            .runtime
            .runtime_version
            .rsplit_once("@sha256:")
            .is_some_and(|(name, digest)| !name.is_empty() && valid_admission_fingerprint(digest));
        valid_admission_fingerprint(&self.batch_sha256)
            && valid_admission_fingerprint(&self.member_sha256)
            && !self.owner.as_str().is_empty()
            && self.revision > 0
            && matches!(self.status, SandboxStatus::Active | SandboxStatus::Failed)
            && seconds_from_rfc3339(&self.created_at).is_some_and(|created| created <= activity)
            && activity > 0
            && activity < cutoff
            && cutoff < now
            && image_pinned
            && self.home.validate(&self.sandbox_id).is_ok()
            && self.matches_native_home()
    }

    fn matches_native_home(&self) -> bool {
        let Some(home) = self.home.attachment() else {
            return false;
        };
        let Some(origin) = home.receipt.native_authority.as_deref() else {
            return false;
        };
        let Some(name) = origin.claim.sandbox_name.as_deref() else {
            return false;
        };
        home.request.policy.is_native_vct()
            && self.runtime.runtime_id == format!("agent-sandbox:{name}:{}", origin.sandbox_uid)
            && self.home.native_pool.as_ref().is_some_and(|native| {
                !native.cleaned
                    && native.cleanup.is_none()
                    && native.request == origin.claim.request
                    && native.claim.as_ref() == Some(&origin.claim)
                    && native.request.pool.spec.image.uri == self.runtime.runtime_version
            })
    }

    /// Match the same logical Sandbox even if the worker cleared runtime fields
    /// after an existing retirement. A release receipt still proves attribution.
    pub fn same_sandbox(&self, current: &Sandbox) -> bool {
        current.id == self.sandbox_id
            && current.owner == self.owner
            && current.created_at == self.created_at
    }

    pub fn matches(&self, current: &Sandbox) -> bool {
        self.same_sandbox(current)
            && current.revision == self.revision
            && current.status == self.status
            && current.last_activity_at == self.last_activity_at
            && current.runtime.as_ref() == Some(&self.runtime)
            && current.lifecycle.home.as_ref() == Some(&self.home)
    }

    /// Use the existing conversation and lease checks for the complete live set.
    /// The store separately fences Agent-less sessions, commands and Workspaces.
    pub fn live_agents_are_idle(
        &self,
        current: &Sandbox,
        live_agents: &[SandboxAgentActivity],
        now: &str,
    ) -> bool {
        let Some(idle_after_seconds) = seconds_from_rfc3339(now)
            .zip(seconds_from_rfc3339(&self.idle_before))
            .and_then(|(now, cutoff)| now.checked_sub(cutoff))
        else {
            return false;
        };
        live_agents.is_empty()
            || super::inactivity::is_quiet_pause_candidate(
                current,
                live_agents,
                now,
                idle_after_seconds,
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedRetirementReceipt {
    pub batch_sha256: String,
    pub member_sha256: String,
    pub guard_sha256: String,
    pub sandbox_id: SandboxId,
    pub accepted_revision: u64,
    pub event_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "receipt")]
pub enum ReviewedRetirementOutcome {
    Accepted(ReviewedRetirementReceipt),
    ObserveExisting,
    NotEligible,
}

#[async_trait::async_trait]
pub trait ReviewedRetirementStore: Send + Sync {
    async fn admit_reviewed_retirement(
        &self,
        member: &ReviewedRetirementMember,
        now: &str,
    ) -> Result<ReviewedRetirementOutcome, super::store::SandboxStoreError>;

    async fn reviewed_retirement_receipt(
        &self,
        id: &SandboxId,
        batch: &str,
        member: &str,
    ) -> Result<Option<ReviewedRetirementReceipt>, super::store::SandboxStoreError>;
}

impl ReviewedRetirementReceipt {
    pub fn belongs_to(
        &self,
        member: &ReviewedRetirementMember,
    ) -> Result<bool, super::store::SandboxStoreError> {
        Ok(self.sandbox_id == member.sandbox_id
            && self.batch_sha256 == member.batch_sha256
            && self.member_sha256 == member.member_sha256
            && self.guard_sha256 == member.fingerprint()?
            && member.revision.checked_add(1) == Some(self.accepted_revision))
    }
}
