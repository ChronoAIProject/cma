//! Stop evidence for one logical Event Gateway provider operation.
use serde::{Deserialize, Serialize};

use super::{
    version, Digest, EffectJournalV1, Generation, GrantKey, OpaqueId, ProtocolError,
    ProtocolResult, RevocationKeyV1,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SenderIdentityV1 {
    pub instance: OpaqueId,
    pub incarnation: Digest,
    pub boot: OpaqueId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelOwnerKindV1 {
    Person,
    Organization,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelAuthorityV1 {
    pub issuer: Digest,
    pub record_incarnation: Digest,
    pub owner_kind: ChannelOwnerKindV1,
    pub owner_subject: Digest,
    pub generation: Generation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SenderBasisV1 {
    Owner {},
    Share { grant: GrantKey },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SenderClosingV1 {
    Channel {
        operation_id: Digest,
        authority: ChannelAuthorityV1,
        watermark: Generation,
    },
    Share {
        revocation: RevocationKeyV1,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SenderStopRequestV1 {
    pub protocol: u32,
    pub registration: Digest,
    pub sender: SenderIdentityV1,
    pub closing: SenderClosingV1,
    pub channel: ChannelAuthorityV1,
    pub basis: SenderBasisV1,
    pub source_consumer: Digest,
    pub custody: Digest,
    pub origin: Digest,
    pub credential_config: Digest,
    pub target: Digest,
    pub effect_id: OpaqueId,
    pub request_digest: Digest,
}

impl SenderStopRequestV1 {
    fn closing_watermark(&self) -> ProtocolResult<Generation> {
        version(self.protocol)?;
        if matches!(&self.basis, SenderBasisV1::Share { grant }
            if grant.share_revision > grant.policy_revision)
        {
            return Err(ProtocolError::InvalidIdentity);
        }
        match (&self.closing, &self.basis) {
            (
                SenderClosingV1::Channel {
                    authority,
                    watermark,
                    ..
                },
                _,
            ) if authority == &self.channel => Ok(*watermark),
            (SenderClosingV1::Share { revocation }, SenderBasisV1::Share { grant })
                if revocation.family == grant.family =>
            {
                Ok(revocation.watermark)
            }
            _ => Err(ProtocolError::EvidenceMismatch),
        }
    }

    pub fn validate_for(
        &self,
        expected: &Self,
        authenticated_sender: &SenderIdentityV1,
    ) -> ProtocolResult<Generation> {
        if &self.sender != authenticated_sender {
            return Err(ProtocolError::AudienceMismatch);
        }
        if self != expected {
            return Err(ProtocolError::EvidenceMismatch);
        }
        self.closing_watermark()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SenderStopReceiptV1 {
    pub protocol: u32,
    pub request: SenderStopRequestV1,
    pub durable_watermark: Generation,
    pub barrier_sequence: Generation,
    pub stop_sequence: Generation,
    pub submitters_closed: bool,
    pub effects: EffectJournalV1,
}

impl SenderStopReceiptV1 {
    /// Check evidence against the original request, retained barrier and sender
    /// identity obtained from authenticated transport.
    pub fn validate_for(
        &self,
        expected: &SenderStopRequestV1,
        authenticated_sender: &SenderIdentityV1,
        barrier_sequence: Generation,
        effects: &EffectJournalV1,
    ) -> ProtocolResult<()> {
        version(self.protocol)?;
        let watermark = self.request.validate_for(expected, authenticated_sender)?;
        self.effects.validate()?;
        if self.durable_watermark != watermark
            || self.barrier_sequence != barrier_sequence
            || self.stop_sequence <= barrier_sequence
            || !self.submitters_closed
            || &self.effects != effects
        {
            return Err(ProtocolError::EvidenceMismatch);
        }
        Ok(())
    }

    /// Compare against the original receipt after validating both bindings.
    pub fn is_exact_replay_of(&self, original: &Self) -> bool {
        self == original
    }
}

#[cfg(test)]
mod tests;
