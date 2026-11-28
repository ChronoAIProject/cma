use super::*;
use crate::resource_access::{
    CallerContext, ResourceOwner, ResourceSelector, ShareChange, ShareCommand, ShareMutation,
    ShareMutationReceipt,
};
use std::time::{SystemTime, UNIX_EPOCH};

/// This retained request authorizes only completion of its original owner
/// operation. It is never accepted by command/ticket admission methods.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevocationIntentV1 {
    pub selector: ResourceSelector,
    pub owner: ResourceOwner,
    pub actor: ResourceActor,
    pub key_sha256: wire::Digest,
    pub change: ShareChange,
    pub expected_share_revision: u64,
    pub sandbox_id: crate::SandboxId,
    pub sandbox_created_at: String,
    pub ancestry: wire::Digest,
    pub original_policy_revision: u64,
    pub proposed_id: wire::OpaqueId,
    pub requested_at: String,
    pub fingerprint: wire::Digest,
}

impl RevocationIntentV1 {
    pub fn mutation(&self, policy_revision: u64) -> ShareMutation {
        ShareMutation {
            selector: self.selector.clone(),
            owner: self.owner.clone(),
            actor: self.actor.clone(),
            key_sha256: self.key_sha256.as_str().into(),
            change: self.change.clone(),
            expected_share_revision: Some(self.expected_share_revision),
            policy_fence: crate::resource_access::ResourcePolicyFence {
                sandbox_id: self.sandbox_id.clone(),
                sandbox_created_at: self.sandbox_created_at.clone(),
                policy_revision,
            },
            proposed_id: self.proposed_id.as_str().into(),
            at: self.requested_at.clone(),
        }
    }

    pub fn validate(&self) -> ExecutionResult<()> {
        if !matches!(
            self.change,
            ShareChange::Revoke { .. } | ShareChange::SetMode { .. }
        ) {
            return Err(ExecutionError::Corrupt);
        }
        let request = self.mutation(self.original_policy_revision);
        request.validate()?;
        if request.fingerprint() != self.fingerprint.as_str() {
            return Err(ExecutionError::Corrupt);
        }
        Ok(())
    }

    pub fn validate_incarnation(&self, snapshot: &ResourceAccessSnapshot) -> ExecutionResult<()> {
        self.validate()?;
        if snapshot.scope(&self.selector.target())?.owner != self.owner
            || snapshot.lineage.sandbox.id != self.sandbox_id
            || snapshot.lineage.sandbox.created_at != self.sandbox_created_at
            || ancestry_digest(snapshot)? != self.ancestry
        {
            return Err(ExecutionError::AuthorityChanged);
        }
        Ok(())
    }

    pub fn reason(&self) -> wire::StopReason {
        match self.change {
            ShareChange::SetMode { .. } => wire::StopReason::CapabilityNarrowed,
            _ => wire::StopReason::ShareRevoked,
        }
    }
}

pub struct RevocationAuthority {
    pub(super) snapshot: ResourceAccessSnapshot,
    pub(super) intent: RevocationIntentV1,
    pub(super) family: wire::GrantFamilyKey,
    pub(super) epoch: wire::Generation,
    pub(super) credential_deadline: Option<tokio::time::Instant>,
}

impl RevocationAuthority {
    pub fn intent(&self) -> &RevocationIntentV1 {
        &self.intent
    }
    pub fn family(&self) -> &wire::GrantFamilyKey {
        &self.family
    }
    pub fn epoch(&self) -> wire::Generation {
        self.epoch
    }
    pub fn snapshot(&self) -> &ResourceAccessSnapshot {
        &self.snapshot
    }
    pub fn validate_current(&self, snapshot: &ResourceAccessSnapshot) -> ExecutionResult<()> {
        if self
            .credential_deadline
            .is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
        {
            return Err(ExecutionError::AuthorityChanged);
        }
        self.intent.validate_incarnation(snapshot)?;
        if snapshot.fence() != self.snapshot.fence() {
            return Err(ExecutionError::AuthorityChanged);
        }
        let share = snapshot
            .shares
            .iter()
            .find(|share| share.id == self.family.share_id.as_str())
            .ok_or(ExecutionError::AuthorityChanged)?;
        if share.revoked_at.is_some()
            || share.revision != self.intent.expected_share_revision
            || share.selector != self.intent.selector
            || share.owner != self.intent.owner
            || origin_digest(snapshot, share)? != self.family.origin
        {
            return Err(ExecutionError::AuthorityChanged);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum RevocationPhaseV1 {
    Pending,
    Effective { receipt: Box<ShareMutationReceipt> },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevocationOperationV1 {
    pub protocol: u32,
    pub key: wire::RevocationKeyV1,
    pub epoch: wire::Generation,
    pub intent: RevocationIntentV1,
    pub frozen_members: u32,
    /// Original ordinals whose settled records were erased in a storage transaction.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub erased_ordinals: std::collections::BTreeSet<u32>,
    pub frozen_at_unix_ms: u64,
    pub deadline_unix_ms: u64,
    pub phase: RevocationPhaseV1,
}

impl RevocationOperationV1 {
    pub fn validate(&self) -> ExecutionResult<()> {
        wire::version(self.protocol)?;
        self.intent.validate()?;
        let share_id = match &self.intent.change {
            ShareChange::Revoke { share_id } | ShareChange::SetMode { share_id, .. } => share_id,
            _ => return Err(ExecutionError::Corrupt),
        };
        if self.key.family.share_id.as_str() != share_id {
            return Err(ExecutionError::Corrupt);
        }
        if self.key.operation_id.as_str()
            != self
                .intent
                .mutation(self.intent.original_policy_revision)
                .operation_id()
            || self.frozen_members > MAX_FAMILY_MEMBERS
            || (!self.erased_ordinals.is_empty()
                && matches!(self.phase, RevocationPhaseV1::Pending))
            || self
                .erased_ordinals
                .iter()
                .any(|ordinal| *ordinal == 0 || *ordinal > self.frozen_members)
            || self.frozen_at_unix_ms > i64::MAX as u64
            || self.frozen_at_unix_ms.checked_add(wire::STOP_BOUND_MS)
                != Some(self.deadline_unix_ms)
        {
            return Err(ExecutionError::Corrupt);
        }
        if let RevocationPhaseV1::Effective { receipt } = &self.phase {
            self.intent
                .mutation(self.intent.original_policy_revision)
                .replay(*receipt.clone())?;
        }
        Ok(())
    }

    /// Returns the next retained ordinal in a validated operation.
    pub fn next_retained_ordinal(&self, after: u32) -> Option<u32> {
        (after.checked_add(1)?..=self.frozen_members)
            .find(|ordinal| !self.erased_ordinals.contains(ordinal))
    }

    pub fn deadline_exceeded_at(&self, unix_ms: u64) -> ExecutionResult<bool> {
        self.validate()?;
        Ok(matches!(self.phase, RevocationPhaseV1::Pending) && unix_ms >= self.deadline_unix_ms)
    }

    /// Call inside the final transaction after rereading the frozen generation.
    /// Timeouts, expired memberships and missing rows never count as evidence.
    pub fn validate_cohort(&self, members: &[ExecutionRecordV1]) -> ExecutionResult<()> {
        self.validate()?;
        if members.len() != self.frozen_members as usize {
            return Err(ExecutionError::EvidenceMismatch);
        }
        let mut seen = std::collections::BTreeSet::new();
        for member in members {
            let request = member.barrier_request(self)?;
            if !seen.insert(member.ordinal) || member.epoch != self.epoch {
                return Err(ExecutionError::EvidenceMismatch);
            }
            let barrier = member
                .barrier
                .as_ref()
                .ok_or(ExecutionError::EvidenceMismatch)?;
            barrier.validate_for(&request)?;
            if member.completion.is_none() {
                return Err(ExecutionError::EvidenceMismatch);
            }
        }
        Ok(())
    }
}

impl ResourceExecutionCoordinator {
    pub async fn begin_revocation(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        command: ShareCommand,
    ) -> ExecutionResult<RevocationOperationV1> {
        let credential_deadline = context.credential_deadline();
        if credential_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            return Err(ExecutionError::AuthorityChanged);
        }
        let snapshot = self
            .access
            .personal_owner(context, credential, &command.selector.target())
            .await?;
        let share_id = match &command.change {
            ShareChange::SetMode { share_id, .. } | ShareChange::Revoke { share_id } => share_id,
            _ => return Err(ExecutionError::AuthorityChanged),
        };
        if command.idempotency_key.is_empty()
            || command.idempotency_key.len() > 256
            || command.idempotency_key.chars().any(char::is_control)
        {
            return Err(ExecutionError::AuthorityChanged);
        }
        let request = ShareMutation {
            selector: command.selector.clone(),
            owner: snapshot.scope(&command.selector.target())?.owner,
            actor: context.actor.clone(),
            key_sha256: format!("{:x}", Sha256::digest(command.idempotency_key.as_bytes())),
            change: command.change.clone(),
            expected_share_revision: command.expected_share_revision,
            policy_fence: snapshot.fence(),
            proposed_id: format!("shr_{}", uuid::Uuid::new_v4().simple()),
            at: crate::rfc3339_at(SystemTime::now()),
        };
        request.validate()?;
        let operation_id = wire::Digest::new(&request.operation_id())?;
        if let Some(old) = self.policy.store.revocation(&operation_id).await? {
            old.validate()?;
            old.intent.validate_incarnation(&snapshot)?;
            if old.intent.fingerprint.as_str() != request.fingerprint() {
                return Err(ExecutionError::IdempotencyConflict);
            }
            if credential_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                return Err(ExecutionError::AuthorityChanged);
            }
            return Ok(old);
        }
        let share = snapshot
            .shares
            .iter()
            .find(|share| &share.id == share_id)
            .ok_or(ExecutionError::AuthorityChanged)?;
        let family = self.policy.open_family(&snapshot, share).await?;
        let fence = snapshot.fence();
        let intent = RevocationIntentV1 {
            selector: request.selector.clone(),
            owner: request.owner.clone(),
            actor: request.actor.clone(),
            key_sha256: wire::Digest::new(&request.key_sha256)?,
            change: request.change.clone(),
            expected_share_revision: request
                .expected_share_revision
                .ok_or(ExecutionError::AuthorityChanged)?,
            sandbox_id: fence.sandbox_id,
            sandbox_created_at: fence.sandbox_created_at,
            ancestry: ancestry_digest(&snapshot)?,
            original_policy_revision: fence.policy_revision,
            proposed_id: wire::OpaqueId::new(&request.proposed_id)?,
            requested_at: request.at.clone(),
            fingerprint: wire::Digest::new(&request.fingerprint())?,
        };
        let authority = RevocationAuthority {
            snapshot,
            intent,
            family,
            epoch: self.policy.epoch,
            credential_deadline,
        };
        authority.validate_current(authority.snapshot())?;
        self.policy.store.begin_revocation(&authority).await
    }

    pub async fn revocation_status(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        selector: &ResourceSelector,
        operation: &wire::Digest,
    ) -> ExecutionResult<RevocationOperationV1> {
        let credential_deadline = context.credential_deadline();
        if credential_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            return Err(ExecutionError::AuthorityChanged);
        }
        let snapshot = self
            .access
            .personal_owner(context, credential, &selector.target())
            .await?;
        let value = self
            .policy
            .store
            .revocation(operation)
            .await?
            .ok_or(ExecutionError::AuthorityChanged)?;
        value.validate()?;
        if &value.intent.selector != selector {
            return Err(ExecutionError::AuthorityChanged);
        }
        value.intent.validate_incarnation(&snapshot)?;
        if credential_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            return Err(ExecutionError::AuthorityChanged);
        }
        Ok(value)
    }

    pub async fn observe_barrier(
        &self,
        authenticated: &wire::ReceiverKey,
        receipt: &wire::BarrierReceiptV1,
    ) -> ExecutionResult<()> {
        if authenticated != receipt.request.member.receiver() {
            return Err(ExecutionError::EvidenceMismatch);
        }
        self.policy.store.observe_barrier(receipt).await
    }

    pub async fn observe_completion(
        &self,
        authenticated: &wire::ReceiverKey,
        completion: &MemberCompletionV1,
    ) -> ExecutionResult<()> {
        let barrier = match completion {
            MemberCompletionV1::Execution { receipt } => &receipt.barrier,
            MemberCompletionV1::Release { receipt } => &receipt.barrier,
        };
        if authenticated != barrier.request.member.receiver() {
            return Err(ExecutionError::EvidenceMismatch);
        }
        self.policy.store.observe_completion(completion).await
    }

    pub async fn finish_revocation(
        &self,
        operation: &wire::Digest,
    ) -> ExecutionResult<RevocationOperationV1> {
        self.policy.store.finish_revocation(operation).await
    }
}

pub fn unix_millis_now() -> ExecutionResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| duration.as_millis().try_into().ok())
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or(ExecutionError::Unavailable)
}
