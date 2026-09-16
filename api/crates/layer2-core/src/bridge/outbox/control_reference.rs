//! Retained exact-source references. Their existence grants no control effect.
use serde::{Deserialize, Serialize};

use crate::bridge::{
    allocation::{AllocationIdentity, AllocationRecord, AllocationState},
    ingress::StoredInbox,
    invoke::{BridgeInvokeGrant, SourceControlPolicy, SourceControlReceipt},
    partition::{NormalizedPartitionSource, StableThreadKind, ThreadEvidence},
};

use super::{
    ChannelReplyTurnAuthorization, ChannelReplyTurnObservation, OutboxError, OutboxSource,
};

pub struct ChannelControlReferenceObservation<'a> {
    pub turn: ChannelReplyTurnObservation<'a>,
    pub inbox: &'a StoredInbox,
    pub grant: &'a BridgeInvokeGrant,
    pub consent: &'a SourceControlPolicy,
    pub consent_receipt: &'a SourceControlReceipt,
}

/// Protected retained source and consent correlation, never tool output or an
/// executable permission. The original source pins owner, issuer, selected bot,
/// platform, chat, Bridge and complete retained message/admission evidence.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelControlReferenceBinding {
    pub source: OutboxSource,
    pub sender_id: String,
    pub thread: ChannelControlThread,
    pub provider_message_id: String,
    pub turn_sequence: u64,
    pub agent_created_at: String,
    pub creation_fingerprint: String,
    pub allocation_identity: AllocationIdentity,
    pub allocation_created_at_ms: i64,
    pub allocation_operation_id: String,
    pub allocation_placement_sha256: String,
    pub consent: SourceControlPolicy,
    pub consent_receipt: SourceControlReceipt,
    pub consent_sha256: String,
}

/// Retained thread correlation is data, never a reconstructed trusted observation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChannelControlThread {
    Unknown,
    Unsupported,
    Missing,
    Verified {
        thread_kind: StableThreadKind,
        id: String,
    },
}
impl ChannelControlThread {
    pub fn capture(evidence: &ThreadEvidence) -> Self {
        match evidence {
            ThreadEvidence::Unknown => Self::Unknown,
            ThreadEvidence::Unsupported => Self::Unsupported,
            ThreadEvidence::Missing => Self::Missing,
            ThreadEvidence::Verified(value) => Self::Verified {
                thread_kind: value.kind(),
                id: value.id().into(),
            },
        }
    }
}

/// Opaque current issuance/recovery authority. It is not deserializable and
/// does not authorize cancellation, steering, approval or external delivery.
pub struct ChannelControlReferenceAuthorization {
    turn: ChannelReplyTurnAuthorization,
    binding: ChannelControlReferenceBinding,
}

impl ChannelControlReferenceAuthorization {
    pub fn from_current(
        observed: ChannelControlReferenceObservation<'_>,
    ) -> Result<Self, OutboxError> {
        let ChannelControlReferenceObservation {
            turn,
            inbox,
            grant,
            consent,
            consent_receipt,
        } = observed;
        grant
            .require_active()
            .map_err(|_| OutboxError::AuthorityUnavailable)?;
        consent_receipt
            .require_policy(consent)
            .map_err(|_| OutboxError::AuthorityUnavailable)?;
        consent
            .view
            .require_grant(grant)
            .map_err(|_| OutboxError::AuthorityUnavailable)?;
        let normalized = NormalizedPartitionSource::from_stored(inbox, turn.bridge)
            .map_err(|_| OutboxError::SourceMismatch)?;
        let basis = grant.channel.as_ref().ok_or(OutboxError::ToolUnavailable)?;
        let source_matches = OutboxSource::capture(
            turn.bridge,
            inbox,
            &basis.allocation,
            &turn.command.receipt,
            turn.sidecar,
        )? == turn.source;
        let actor_matches = matches!(&turn.command.receipt.envelope.actor,
            crate::resource_access::ResourceActor::Channel { grant_id, .. } if grant_id == grant.id.as_str());
        if !consent.view.enabled
            || !source_matches
            || !actor_matches
            || turn.bridge.emergency.execution_stopped
            || turn
                .bridge
                .admission_pin()
                .map_err(|_| OutboxError::AuthorityUnavailable)?
                != grant.pin
            || grant.pin != *turn.source.pin()
            || grant.owner != *turn.source.owner()
            || grant.target.ids != *turn.source.target()
            || grant.config != *turn.source.config()
            || inbox.id != turn.source.inbox_id()
            || basis.creation != *turn.provenance
            || basis.allocation.identity != turn.allocation.identity
            || basis.allocation.placement != turn.allocation.placement
            || basis.allocation.created_at_ms != turn.allocation.created_at_ms
            || turn.allocation.state != AllocationState::Ready
        {
            return Err(OutboxError::AuthorityUnavailable);
        }
        let provider_message_id = normalized
            .provider_message_id()
            .ok_or(OutboxError::ToolUnavailable)?
            .to_owned();
        let allocation_identity = turn.allocation.identity.clone();
        let allocation_created_at_ms = turn.allocation.created_at_ms;
        let allocation_operation_id = turn.allocation.operation_id.clone();
        let allocation_placement_sha256 = super::hash(&turn.allocation.placement)?;
        let turn = ChannelReplyTurnAuthorization::from_current(turn)?;
        let binding = ChannelControlReferenceBinding {
            source: turn.source().clone(),
            sender_id: normalized.sender_id().into(),
            thread: ChannelControlThread::capture(normalized.thread()),
            provider_message_id,
            turn_sequence: turn.turn_sequence(),
            agent_created_at: turn.agent_created_at().into(),
            creation_fingerprint: turn.creation_fingerprint().into(),
            allocation_identity,
            allocation_created_at_ms,
            allocation_operation_id,
            allocation_placement_sha256,
            consent: consent.clone(),
            consent_receipt: consent_receipt.clone(),
            consent_sha256: super::hash(&(consent, consent_receipt))?,
        };
        binding.validate()?;
        Ok(Self { turn, binding })
    }

    pub fn turn(&self) -> &ChannelReplyTurnAuthorization {
        &self.turn
    }
    pub fn lookup_sha256(&self) -> Result<String, OutboxError> {
        self.turn.require_current()?;
        self.binding.lookup_sha256()
    }
    pub fn retain(&self, reference: String) -> Result<ChannelControlReferenceRecord, OutboxError> {
        self.turn.require_current()?;
        let record = ChannelControlReferenceRecord {
            schema_version: 1,
            reference,
            lookup_sha256: self.binding.lookup_sha256()?,
            binding: self.binding.clone(),
        };
        record.validate()?;
        Ok(record)
    }
    pub fn require_record(
        &self,
        record: &ChannelControlReferenceRecord,
    ) -> Result<(), OutboxError> {
        self.turn.require_current()?;
        record.validate()?;
        if record.binding != self.binding {
            return Err(OutboxError::SourceMismatch);
        }
        Ok(())
    }
}

impl ChannelControlReferenceBinding {
    /// Match the original allocation using the retained reference producer's
    /// digest convention. Callers must reread and fence this live record.
    pub fn require_allocation(&self, allocation: &AllocationRecord) -> Result<(), OutboxError> {
        self.validate()?;
        allocation.validate().map_err(|_| OutboxError::Corrupt)?;
        if allocation.state != AllocationState::Ready
            || allocation.owner != *self.source.owner()
            || allocation.placement.target() != self.source.target()
            || allocation.identity != self.allocation_identity
            || allocation.created_at_ms != self.allocation_created_at_ms
            || allocation.operation_id != self.allocation_operation_id
            || super::hash(&allocation.placement)? != self.allocation_placement_sha256
        {
            return Err(OutboxError::SourceMismatch);
        }
        Ok(())
    }

    fn lookup_sha256(&self) -> Result<String, OutboxError> {
        super::hash(&("cma.channel-control-reference.v1", self))
    }
    fn validate(&self) -> Result<(), OutboxError> {
        self.source.validate()?;
        self.consent_receipt
            .require_policy(&self.consent)
            .map_err(|_| OutboxError::Corrupt)?;
        if !self.consent.view.enabled
            || self.consent.view.owner != *self.source.owner()
            || self.consent.view.pin != *self.source.pin()
            || self.consent.view.target.ids != *self.source.target()
            || self.consent_sha256 != super::hash(&(&self.consent, &self.consent_receipt))?
            || self
                .allocation_identity
                .id()
                .map_err(|_| OutboxError::Corrupt)?
                != self.source.allocation_id()
            || self.allocation_created_at_ms < 0
            || self.turn_sequence == 0
            || crate::seconds_from_rfc3339(&self.agent_created_at).is_none()
            || !super::digest(&self.creation_fingerprint)
            || !super::digest(&self.allocation_placement_sha256)
            || ![
                &self.sender_id,
                &self.provider_message_id,
                &self.allocation_operation_id,
            ]
            .iter()
            .all(|value| super::id(value))
        {
            return Err(OutboxError::Corrupt);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelControlReferenceRecord {
    pub schema_version: u8,
    pub reference: String,
    pub lookup_sha256: String,
    binding: ChannelControlReferenceBinding,
}
impl ChannelControlReferenceRecord {
    pub fn binding(&self) -> &ChannelControlReferenceBinding {
        &self.binding
    }
    pub fn validate(&self) -> Result<(), OutboxError> {
        self.binding.validate()?;
        if self.schema_version != 1
            || self.lookup_sha256 != self.binding.lookup_sha256()?
            || !self.reference.strip_prefix("ccr_").is_some_and(|suffix| {
                suffix.len() == 32
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err(OutboxError::Corrupt);
        }
        Ok(())
    }
}
