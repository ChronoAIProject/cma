use serde::{Deserialize, Serialize};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevocationKeyV1 {
    pub operation_id: Digest,
    pub family: GrantFamilyKey,
    pub watermark: Generation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    ShareRevoked,
    CapabilityNarrowed,
    ContinuationExpired,
    Recovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BarrierRequestV1 {
    pub protocol: u32,
    pub revocation: RevocationKeyV1,
    pub member: MemberKey,
    pub registration: Digest,
    pub reason: StopReason,
}

impl BarrierRequestV1 {
    pub fn validate(&self) -> ProtocolResult<()> {
        version(self.protocol)?;
        self.member.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BarrierReceiptV1 {
    pub protocol: u32,
    pub request: BarrierRequestV1,
    pub barrier_sequence: Generation,
    pub durable_watermark: Generation,
}

impl BarrierReceiptV1 {
    pub fn validate_for(&self, request: &BarrierRequestV1) -> ProtocolResult<()> {
        version(self.protocol)?;
        request.validate()?;
        if &self.request != request || self.durable_watermark != request.revocation.watermark {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeStartState {
    Entered,
    RejectedBeforeStart,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartObservationV1 {
    pub protocol: u32,
    pub execution: ExecutionKey,
    pub registration: Digest,
    pub nonce: OpaqueId,
    pub state: NativeStartState,
    pub sequence: Generation,
}

impl StartObservationV1 {
    pub fn validate_for(&self, ticket: &ExecutionTicketV1) -> ProtocolResult<()> {
        version(self.protocol)?;
        ticket.validate()?;
        if self.execution != ticket.execution
            || self.registration != ticket.registration
            || self.nonce != ticket.nonce
        {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectJournalV1 {
    pub high_water: u64,
    pub digest: Digest,
    pub unresolved_count: u32,
}

impl EffectJournalV1 {
    pub fn validate(&self) -> ProtocolResult<()> {
        if self.high_water > i64::MAX as u64 || u64::from(self.unresolved_count) > self.high_water {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuiescenceV1 {
    pub queues_purged: bool,
    pub descendants_absent: bool,
    pub output_closed: bool,
    pub submitters_closed: bool,
    pub pending_local_effects_settled: bool,
    pub local_changes_digest: Digest,
    pub effects: EffectJournalV1,
}

impl QuiescenceV1 {
    pub fn validate(&self) -> ProtocolResult<()> {
        self.effects.validate()?;
        if !self.queues_purged
            || !self.descendants_absent
            || !self.output_closed
            || !self.submitters_closed
            || !self.pending_local_effects_settled
        {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopEvidenceKind {
    RejectedBeforeStart,
    CooperativeQuiescence,
    TargetedTermination,
}

/// A durable receiver observation, never a submission or continuation permit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopReceiptV1 {
    pub protocol: u32,
    pub barrier: BarrierReceiptV1,
    pub stop_sequence: Generation,
    pub evidence: StopEvidenceKind,
    pub quiescence: QuiescenceV1,
}

impl StopReceiptV1 {
    pub fn validate_for(&self, request: &BarrierRequestV1) -> ProtocolResult<()> {
        version(self.protocol)?;
        self.barrier.validate_for(request)?;
        if !matches!(request.member, MemberKey::Execution { .. })
            || self.stop_sequence <= self.barrier.barrier_sequence
        {
            return Err(ProtocolError::EvidenceMismatch);
        }
        self.quiescence.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseReceiptV1 {
    pub protocol: u32,
    pub barrier: BarrierReceiptV1,
    pub close_sequence: Generation,
    pub producers_closed: bool,
    pub pending_chunks_discarded: bool,
    pub transport_drained: bool,
    pub released_bytes: u64,
}

impl ReleaseReceiptV1 {
    pub fn validate_for(&self, request: &BarrierRequestV1) -> ProtocolResult<()> {
        version(self.protocol)?;
        self.barrier.validate_for(request)?;
        if !matches!(request.member, MemberKey::Release { .. })
            || self.close_sequence <= self.barrier.barrier_sequence
            || !self.producers_closed
            || !self.pending_chunks_discarded
            || !self.transport_drained
        {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectOperationClass {
    ModelRequest,
    ToolRequest,
    RepositoryRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectIntentV1 {
    pub effect_id: OpaqueId,
    pub execution: ExecutionKey,
    pub registration: Digest,
    pub grant: ExecutionBasisV1,
    pub operation: EffectOperationClass,
    pub request_digest: Digest,
    pub provider_idempotency: Option<Digest>,
}

impl EffectIntentV1 {
    pub fn validate_for(&self, ticket: &ExecutionTicketV1) -> ProtocolResult<()> {
        ticket.validate()?;
        if self.execution != ticket.execution
            || self.registration != ticket.registration
            || self.grant != ticket.basis
        {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectPermissionV1 {
    pub protocol: u32,
    pub intent: EffectIntentV1,
    pub ledger_sequence: Generation,
    pub permission_id: Digest,
}

impl EffectPermissionV1 {
    pub fn validate_for(
        &self,
        intent: &EffectIntentV1,
        ticket: &ExecutionTicketV1,
    ) -> ProtocolResult<()> {
        version(self.protocol)?;
        intent.validate_for(ticket)?;
        if &self.intent != intent {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectOutcome {
    Prepared,
    SubmissionUnknown,
    Submitted,
    CommitConfirmed,
    Rejected,
    ReconciliationRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectObservationV1 {
    pub protocol: u32,
    pub permission_id: Digest,
    pub execution: ExecutionKey,
    pub effect_id: OpaqueId,
    pub sequence: Generation,
    pub outcome: EffectOutcome,
    pub evidence_digest: Digest,
}

impl EffectObservationV1 {
    pub fn validate_for(&self, permission: &EffectPermissionV1) -> ProtocolResult<()> {
        version(self.protocol)?;
        version(permission.protocol)?;
        if self.permission_id != permission.permission_id
            || self.execution != permission.intent.execution
            || self.effect_id != permission.intent.effect_id
            || self.outcome == EffectOutcome::Prepared
        {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePermitV1 {
    pub protocol: u32,
    pub release: ReleaseKey,
    pub basis: ExecutionBasisV1,
    pub registration: Digest,
    pub challenge: ReceiverChallengeV1,
    pub sequence: Generation,
}

impl ReleasePermitV1 {
    pub fn validate(&self) -> ProtocolResult<()> {
        version(self.protocol)?;
        self.basis.validate()?;
        self.challenge.validate()?;
        MemberKey::Release {
            release: self.release.clone(),
        }
        .validate()?;
        if self.release.receiver != self.challenge.receiver
            || !matches!(
                self.basis.capability(),
                ProtectedCapability::ReadDetails
                    | ProtectedCapability::ReadHistory
                    | ProtectedCapability::ReadArtifacts
                    | ProtectedCapability::ReadFiles
            )
        {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(())
    }

    pub fn validate_after(&self, previous: &Self) -> ProtocolResult<()> {
        self.validate()?;
        previous.validate()?;
        if self.release != previous.release
            || self.basis != previous.basis
            || self.registration != previous.registration
            || self.sequence != previous.sequence.successor()?
            || self.challenge.nonce == previous.challenge.nonce
            || self.challenge.created_tick_ms <= previous.challenge.created_tick_ms
            || self.challenge.deadline_tick_ms <= previous.challenge.deadline_tick_ms
        {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(())
    }
}
