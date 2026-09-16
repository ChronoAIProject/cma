//! Shared callback/provider choice: v1 Message/refusal and v2 accepted intent.
use layer2_core::{
    bridge::{
        ingress::{CallbackAcceptance, CallbackError, InboxWrite},
        BridgeWorkPin,
    },
    resource_access::ResourceOwner,
};
use mongodb::bson::{doc, Document};
use serde::{Deserialize, Serialize};

use crate::store::sandbox_mongo::{read_record, record_bson, DOCUMENT_SCHEMA_VERSION};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    inbox_id: String,
    owner: ResourceOwner,
    pin: BridgeWorkPin,
    bot_id: String,
    key_id: String,
    message_id: String,
    correlation_id: String,
    issuer: String,
    body_sha256: String,
}

impl Source {
    fn capture(write: &InboxWrite) -> Self {
        let inbox = write.record();
        Self {
            inbox_id: inbox.id.clone(),
            owner: inbox.owner.clone(),
            pin: inbox.pin.clone(),
            bot_id: inbox.bot_id.clone(),
            key_id: inbox.key_id.clone(),
            message_id: inbox.message_id.clone(),
            correlation_id: inbox.correlation_id.clone(),
            issuer: inbox.attestation.issuer.clone(),
            body_sha256: inbox.attestation.body_sha256.clone(),
        }
    }
    fn validate(&self) -> Result<(), CallbackError> {
        if !hash(&self.inbox_id)
            || !hash(&self.body_sha256)
            || self.pin.config_revision == 0
            || [
                &self.bot_id,
                &self.key_id,
                &self.message_id,
                &self.correlation_id,
                &self.issuer,
            ]
            .iter()
            .any(|s| s.is_empty() || s.len() > 2048 || s.chars().any(char::is_control))
        {
            return Err(CallbackError::Unavailable);
        }
        Ok(())
    }
    fn same_scope(&self, other: &Self) -> bool {
        self.owner == other.owner
            && self.pin.bridge_id == other.pin.bridge_id
            && self.bot_id == other.bot_id
            && self.issuer == other.issuer
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderIdentity {
    key_sha256: String,
    semantic_sha256: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CallbackDisposition {
    version: u16,
    source: Source,
    original: Source,
    receipt: CallbackAcceptance,
    provider: Option<ProviderIdentity>,
}

impl CallbackDisposition {
    pub(super) fn capture(
        write: &InboxWrite,
        receipt: CallbackAcceptance,
    ) -> Result<Self, CallbackError> {
        let source = Source::capture(write);
        let value = Self {
            // Fresh ingress still supports only Message/refusal. Reading a v2
            // receipt cannot enable an accepted-control writer through here.
            version: 1,
            original: source.clone(),
            source,
            receipt,
            provider: write.platform_redelivery().map(|p| ProviderIdentity {
                key_sha256: p.key_sha256().into(),
                semantic_sha256: p.semantic_sha256().into(),
            }),
        };
        value.validate()?;
        Ok(value)
    }

    /// Only the dedicated atomic control writer calls this with its actual
    /// held reservation result. Generic capture deliberately remains v1-only.
    pub(super) fn capture_control(
        write: &layer2_core::bridge::channel_control::ChannelControlWrite,
        control: &layer2_core::bridge::channel_control::AdmittedChannelControl,
    ) -> Result<Self, CallbackError> {
        use layer2_core::bridge::channel_control::{ChannelControlCallback, ChannelControlHold};
        control.validate().map_err(|_| CallbackError::Unavailable)?;
        let callback =
            ChannelControlCallback::capture(write).map_err(|_| CallbackError::Payload)?;
        let intent = &control.sidecar.intent;
        if intent.callback != callback
            || intent.request != *write.request()
            || control.sidecar.execution_hold != ChannelControlHold::WorkerUnavailable
        {
            return Err(CallbackError::ReplayConflict);
        }
        let source = Source::capture(write.inbox());
        let value = Self {
            version: 2,
            source: source.clone(),
            original: source,
            receipt: CallbackAcceptance::ControlAccepted(
                intent
                    .expected_acceptance()
                    .map_err(|_| CallbackError::Unavailable)?,
            ),
            provider: Some(ProviderIdentity {
                key_sha256: callback.provider_key_sha256,
                semantic_sha256: callback.provider_semantic_sha256,
            }),
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), CallbackError> {
        self.source.validate()?;
        self.original.validate()?;
        if !self.source.same_scope(&self.original)
            || self.receipt.callback_id() != self.original.inbox_id
            || (self.source != self.original && self.provider.is_none())
            || self
                .provider
                .as_ref()
                .is_some_and(|p| !hash(&p.key_sha256) || !hash(&p.semantic_sha256))
        {
            return Err(CallbackError::Unavailable);
        }
        let (bridge, message) = match &self.receipt {
            CallbackAcceptance::Message(receipt) if self.version == 1 => {
                (&receipt.bridge_id, &receipt.message_id)
            }
            CallbackAcceptance::ControlRefused(receipt) => {
                if self.version != 1 || receipt.version != 1 {
                    return Err(CallbackError::Unavailable);
                }
                (&receipt.bridge_id, &receipt.message_id)
            }
            CallbackAcceptance::ControlAccepted(receipt) => {
                if self.version != 2
                    || receipt.version != 2
                    || !receipt.intent_id.strip_prefix("cci_").is_some_and(hash)
                {
                    return Err(CallbackError::Unavailable);
                }
                (&receipt.bridge_id, &receipt.message_id)
            }
            _ => return Err(CallbackError::Unavailable),
        };
        if bridge != &self.original.pin.bridge_id || message != &self.original.message_id {
            return Err(CallbackError::Unavailable);
        }
        Ok(())
    }

    pub(super) fn require_control_intent(
        &self,
        intent: &layer2_core::bridge::channel_control::ChannelControlIntent,
    ) -> Result<(), CallbackError> {
        self.validate()?;
        let callback = &intent.callback;
        let expected_source = Source {
            inbox_id: callback.callback_id.clone(),
            owner: callback.owner.clone(),
            pin: callback.pin.clone(),
            bot_id: callback.bot_id.clone(),
            key_id: callback.key_id.clone(),
            message_id: callback.message_id.clone(),
            correlation_id: callback.correlation_id.clone(),
            issuer: callback.issuer.clone(),
            body_sha256: callback.body_sha256.clone(),
        };
        let expected_receipt = CallbackAcceptance::ControlAccepted(
            intent
                .expected_acceptance()
                .map_err(|_| CallbackError::Unavailable)?,
        );
        if self.version != 2
            || self.source != expected_source
            || self.original != expected_source
            || self.receipt != expected_receipt
            || self.provider.as_ref().is_none_or(|provider| {
                provider.key_sha256 != callback.provider_key_sha256
                    || provider.semantic_sha256 != callback.provider_semantic_sha256
            })
        {
            return Err(CallbackError::ReplayConflict);
        }
        Ok(())
    }

    pub(super) fn require_retained_control(
        &self,
        intent: &layer2_core::bridge::channel_control::ChannelControlIntent,
    ) -> Result<(), CallbackError> {
        self.validate()?;
        let mut original = self.clone();
        original.source = original.original.clone();
        original.require_control_intent(intent)
    }

    pub(super) fn require_control_alias(
        &self,
        intent: &layer2_core::bridge::channel_control::ChannelControlIntent,
        callback: &layer2_core::bridge::channel_control::ChannelControlCallback,
    ) -> Result<(), CallbackError> {
        self.validate()?;
        intent
            .callback
            .require_alias(callback)
            .map_err(|_| CallbackError::ReplayConflict)?;
        let mut original = self.clone();
        original.source = original.original.clone();
        original.require_control_intent(intent)?;
        let source = Source {
            inbox_id: callback.callback_id.clone(),
            owner: callback.owner.clone(),
            pin: callback.pin.clone(),
            bot_id: callback.bot_id.clone(),
            key_id: callback.key_id.clone(),
            message_id: callback.message_id.clone(),
            correlation_id: callback.correlation_id.clone(),
            issuer: callback.issuer.clone(),
            body_sha256: callback.body_sha256.clone(),
        };
        if self.source != source {
            return Err(CallbackError::ReplayConflict);
        }
        Ok(())
    }

    pub(super) fn require_exact(
        &self,
        write: &InboxWrite,
    ) -> Result<CallbackAcceptance, CallbackError> {
        self.validate()?;
        write.require_current(write.bridge())?;
        let mut source = Source::capture(write);
        source.pin.config_revision = self.source.pin.config_revision;
        if source != self.source {
            return Err(CallbackError::ReplayConflict);
        }
        Ok(self.receipt.clone())
    }

    pub(super) fn alias(&self, write: &InboxWrite) -> Result<Self, CallbackError> {
        self.validate()?;
        let proof = write
            .platform_redelivery()
            .ok_or(CallbackError::ReplayConflict)?;
        let provider = self.provider.as_ref().ok_or(CallbackError::Unavailable)?;
        let source = Source::capture(write);
        if !source.same_scope(&self.original)
            || provider.key_sha256 != proof.key_sha256()
            || provider.semantic_sha256 != proof.semantic_sha256()
        {
            return Err(CallbackError::ReplayConflict);
        }
        let mut alias = self.clone();
        alias.source = source;
        alias.validate()?;
        Ok(alias)
    }

    pub(super) fn document(&self) -> Result<Document, CallbackError> {
        self.validate()?;
        document(&callback_id(&self.source.inbox_id), "callback", self)
    }

    pub(super) fn read(document: &Document) -> Result<Self, CallbackError> {
        let value: Self = read_record(document).map_err(|_| CallbackError::Unavailable)?;
        if !matches_document(document, &value.document()?) {
            return Err(CallbackError::Unavailable);
        }
        Ok(value)
    }

    pub(super) fn provider_document(&self) -> Result<Option<Document>, CallbackError> {
        self.validate()?;
        if self.source != self.original {
            return Err(CallbackError::Unavailable);
        }
        self.provider
            .as_ref()
            .map(|p| document(&provider_id(&p.key_sha256), "provider", self))
            .transpose()
    }

    pub(super) fn read_provider(document: &Document, key: &str) -> Result<Self, CallbackError> {
        let value: Self = read_record(document).map_err(|_| CallbackError::Unavailable)?;
        let expected = value
            .provider_document()?
            .ok_or(CallbackError::Unavailable)?;
        if !matches_document(document, &expected)
            || expected.get_str("_id").ok() != Some(provider_id(key).as_str())
        {
            return Err(CallbackError::Unavailable);
        }
        Ok(value)
    }
}

fn document(id: &str, kind: &str, value: &CallbackDisposition) -> Result<Document, CallbackError> {
    Ok(
        doc! {"_id":id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,"kind":kind,
        "record_sha256":layer2_core::profiles::content_digest(value).map_err(|_|CallbackError::Unavailable)?,
        "record":record_bson(value).map_err(|_|CallbackError::Unavailable)?},
    )
}

fn matches_document(actual: &Document, expected: &Document) -> bool {
    ["_id", "kind", "record_sha256"]
        .iter()
        .all(|key| actual.get(*key) == expected.get(*key))
}

pub(super) fn callback_id(id: &str) -> String {
    format!("callback:{id}")
}
pub(super) fn provider_id(key: &str) -> String {
    format!("provider:{key}")
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests;
