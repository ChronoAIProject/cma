use serde::{Deserialize, Serialize};

use crate::bridge::{
    ingress::{ChannelControlReceipt, ChannelControlRefusal, InboxWrite},
    outbox::ChannelControlReferenceRecord,
    BridgeWorkPin,
};
use crate::resource_access::ResourceOwner;

use super::{ChannelControlError as Error, ChannelControlRequest, ChannelControlWrite};

/// Immutable signed callback identity without payload credentials. Provider
/// equivalence is supplied only by the live verified adapter, never inferred.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelControlCallback {
    pub callback_id: String,
    pub owner: ResourceOwner,
    pub pin: BridgeWorkPin,
    pub bot_id: String,
    pub key_id: String,
    pub message_id: String,
    pub correlation_id: String,
    pub issuer: String,
    pub body_sha256: String,
    pub provider_key_sha256: String,
    pub provider_semantic_sha256: String,
}

impl ChannelControlCallback {
    pub fn capture(write: &ChannelControlWrite) -> Result<Self, Error> {
        write.require_current()?;
        Self::capture_inbox(write.inbox())
    }

    pub(super) fn capture_inbox(write: &InboxWrite) -> Result<Self, Error> {
        write
            .require_current(write.bridge())
            .map_err(|_| Error::AuthorityUnavailable)?;
        let inbox = write.record();
        let provider = write
            .platform_redelivery()
            .ok_or(Error::CapabilityUnsupported)?;
        let value = Self {
            callback_id: inbox.id.clone(),
            owner: inbox.owner.clone(),
            pin: inbox.pin.clone(),
            bot_id: inbox.bot_id.clone(),
            key_id: inbox.key_id.clone(),
            message_id: inbox.message_id.clone(),
            correlation_id: inbox.correlation_id.clone(),
            issuer: inbox.attestation.issuer.clone(),
            body_sha256: inbox.attestation.body_sha256.clone(),
            provider_key_sha256: provider.key_sha256().into(),
            provider_semantic_sha256: provider.semantic_sha256().into(),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), Error> {
        if ![
            &self.callback_id,
            &self.body_sha256,
            &self.provider_key_sha256,
            &self.provider_semantic_sha256,
        ]
        .iter()
        .all(|value| super::digest(value))
            || !(1..=256).contains(&self.pin.config_revision)
            || [
                &self.bot_id,
                &self.key_id,
                &self.message_id,
                &self.correlation_id,
                &self.issuer,
            ]
            .iter()
            .any(|value| {
                value.is_empty() || value.len() > 2048 || value.chars().any(char::is_control)
            })
        {
            return Err(Error::Corrupt);
        }
        Ok(())
    }

    pub fn intent_id(&self) -> Result<String, Error> {
        self.validate()?;
        Ok(format!(
            "cci_{}",
            super::hash(&("cma.channel-control-intent.v1", &self.provider_key_sha256))?
        ))
    }

    pub fn require_alias(&self, incoming: &Self) -> Result<(), Error> {
        self.validate()?;
        incoming.validate()?;
        if self.owner != incoming.owner
            || self.pin.bridge_id != incoming.pin.bridge_id
            || self.bot_id != incoming.bot_id
            || self.issuer != incoming.issuer
            || self.provider_key_sha256 != incoming.provider_key_sha256
            || self.provider_semantic_sha256 != incoming.provider_semantic_sha256
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }

    pub fn require_callback(&self, incoming: &Self) -> Result<(), Error> {
        self.require_alias(incoming)?;
        let mut compared = incoming.clone();
        // A signature replay may resolve the same callback under a newer live
        // config. This never changes the original intent's retained pin.
        compared.pin.config_revision = self.pin.config_revision;
        if self != &compared {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}

/// Permanent refused data, deliberately a different type from an admissible
/// intent. It has no conversion into ChannelControlProof or a reservation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefusedChannelControlIntent {
    pub schema_version: u8,
    pub id: String,
    pub callback: ChannelControlCallback,
    pub request: ChannelControlRequest,
    pub receipt: ChannelControlReceipt,
}

impl RefusedChannelControlIntent {
    pub fn capture(write: &ChannelControlWrite) -> Result<Self, Error> {
        let callback = ChannelControlCallback::capture(write)?;
        let value = Self {
            schema_version: 1,
            id: callback.intent_id()?,
            receipt: disabled_receipt(write.inbox()),
            callback,
            request: write.request().clone(),
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<(), Error> {
        self.callback.validate()?;
        self.request.validate().map_err(|_| Error::Corrupt)?;
        if self.schema_version != 1
            || self.id != self.callback.intent_id()?
            || self.receipt.version != 1
            || self.receipt.callback_id != self.callback.callback_id
            || self.receipt.bridge_id != self.callback.pin.bridge_id
            || self.receipt.message_id != self.callback.message_id
            || self.receipt.persisted_at < 0
            || self.receipt.code != ChannelControlRefusal::ChannelControlsDisabled
        {
            return Err(Error::Corrupt);
        }
        Ok(())
    }
}

fn disabled_receipt(write: &InboxWrite) -> ChannelControlReceipt {
    ChannelControlReceipt {
        version: 1,
        callback_id: write.record().id.clone(),
        bridge_id: write.record().pin.bridge_id.clone(),
        message_id: write.record().message_id.clone(),
        persisted_at: write.record().accepted_at,
        code: ChannelControlRefusal::ChannelControlsDisabled,
    }
}

/// Source, consent, target and revision data captured once before a possible
/// admission. Deserialization grants no permission and never refreshes a pin.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelControlIntent {
    pub schema_version: u8,
    pub id: String,
    pub callback: ChannelControlCallback,
    pub request: ChannelControlRequest,
    pub reference: ChannelControlReferenceRecord,
    pub expected_agent_revision: u64,
    pub observed_source_command_revision: u64,
    pub runtime_generation: u64,
    pub runtime_conversation_id: String,
    pub captured_at: i64,
}

impl ChannelControlIntent {
    pub fn validate(&self) -> Result<(), Error> {
        self.callback.validate()?;
        self.reference.validate().map_err(|_| Error::Corrupt)?;
        let source = &self.reference.binding().source;
        if self.schema_version != 1
            || self.id != self.callback.intent_id()?
            || self.request.reference != self.reference.reference
            || self.callback.owner != *source.owner()
            || self.callback.pin != *source.pin()
            || self.callback.bot_id != source.config().connector.bot_id
            || self.callback.key_id != source.config().connector.key_id
            || self.callback.issuer != source.config().connector.issuer
            || ![
                self.expected_agent_revision,
                self.observed_source_command_revision,
                self.runtime_generation,
            ]
            .iter()
            .all(|value| (1..=i64::MAX as u64).contains(value))
            || self.captured_at < 0
            || self.runtime_conversation_id.is_empty()
            || self.runtime_conversation_id.len() > 2048
            || self.runtime_conversation_id.chars().any(char::is_control)
        {
            return Err(Error::Corrupt);
        }
        self.request.validate().map_err(|_| Error::Corrupt)?;
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<String, Error> {
        self.validate()?;
        super::hash(&("cma.channel-control-resolved-intent.v1", self))
    }

    /// Expected historical receipt fields only. This value is never evidence
    /// that an admission or its callback disposition actually committed.
    pub fn expected_acceptance(
        &self,
    ) -> Result<crate::bridge::ingress::ChannelControlAcceptedReceipt, Error> {
        self.validate()?;
        Ok(crate::bridge::ingress::ChannelControlAcceptedReceipt {
            version: 2,
            callback_id: self.callback.callback_id.clone(),
            bridge_id: self.callback.pin.bridge_id.clone(),
            message_id: self.callback.message_id.clone(),
            intent_id: self.id.clone(),
            persisted_at: self.captured_at,
        })
    }

    pub fn command_id(&self) -> Result<crate::ids::AgentCommandId, Error> {
        Ok(crate::ids::AgentCommandId(format!(
            "cmd_{}",
            super::hash(&("cma.channel-control-command.v1", &self.id))?
        )))
    }
}
