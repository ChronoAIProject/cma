//! Replacement-only proof of the authenticated Custom producer's current custody.
use super::{codec, types::*};
use cmaeg_core::{
    activity::{Activity, ActivityKind, ActorKind, ConversationKind, SourceKind},
    channel::{ChannelDefinition, CustomAuth, Source},
    seal::MasterKeyRing,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CustomSourceCustody {
    version: u8,
    kid: u8,
    fingerprint: String,
}

pub(super) fn eligible(activity: &Activity, source: &Source) -> bool {
    match (
        &activity.kind,
        &activity.source,
        source,
        &activity.actor.kind,
    ) {
        (ActivityKind::Message { .. }, source, _, ActorKind::Human) => {
            !matches!(source, SourceKind::Custom)
        }
        (
            ActivityKind::Event { ce, .. },
            SourceKind::Custom,
            Source::Custom { .. },
            ActorKind::System,
        ) => {
            !ce.r#type.to_ascii_lowercase().starts_with("cma.")
                && ce.id == activity.event_id.as_str()
                && ce.source == activity.actor.id
                && activity.conversation.kind == ConversationKind::Custom
                && activity.thread.is_none()
        }
        _ => false,
    }
}

impl CustomSourceCustody {
    pub(super) fn capture(
        identity: &EventIdentity,
        source: &Source,
        definition: &ChannelDefinition,
        keys: &MasterKeyRing,
    ) -> Result<Self, AdmissionError> {
        let kid = keys.current_kid();
        Ok(Self {
            version: 1,
            kid,
            fingerprint: fingerprint(identity, source, definition, keys, kid)?,
        })
    }
    fn valid(&self) -> bool {
        self.version == 1
            && self.fingerprint.len() == 64
            && self
                .fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

impl AdmittedPlan {
    pub(super) fn valid_custom_source_custody(&self) -> bool {
        let enrolled = self
            .targets
            .iter()
            .any(|target| target.binding_replacement_version.is_some());
        match &self.binding_custom_source {
            Some(custody) => {
                custody.valid()
                    && enrolled
                    && self.binding_preparation_version == Some(1)
                    && self.kind == PAYLOAD_KIND
                    && matches!(self.activity.source, SourceKind::Custom)
                    && eligible(&self.activity, &self.source)
            }
            None => !(enrolled && matches!(self.activity.source, SourceKind::Custom)),
        }
    }
    /// No additional check or read for existing non-replacement traffic.
    pub(crate) fn current_preparation_source(
        &self,
        definition: &ChannelDefinition,
        keys: &MasterKeyRing,
    ) -> bool {
        let Some(custody) = &self.binding_custom_source else {
            return true;
        };
        fingerprint(&self.identity, &self.source, definition, keys, custody.kid)
            .is_ok_and(|current| current == custody.fingerprint)
    }
}

fn fingerprint(
    identity: &EventIdentity,
    source: &Source,
    definition: &ChannelDefinition,
    keys: &MasterKeyRing,
    kid: u8,
) -> Result<String, AdmissionError> {
    if definition.record_id != identity.record_id
        || definition.owner.subject != identity.owner
        || !definition.sources.contains(source)
    {
        return Err(AdmissionError::Invalid);
    }
    let Source::Custom { auth, .. } = source else {
        return Err(AdmissionError::Invalid);
    };
    let credential = match auth {
        CustomAuth::Hmac { key_id } => definition
            .secrets
            .hmac_secrets
            .iter()
            .find(|(id, _)| id == key_id)
            .map(|(_, secret)| secret.as_str().as_bytes())
            .ok_or(AdmissionError::Invalid)?,
        CustomAuth::NyxidKey { .. } => &[],
    };
    let context = codec::canonical(&(
        &identity.owner,
        &identity.channel_id,
        &identity.record_id,
        "custom",
        auth,
    ))?;
    keys.custody_fingerprint(
        kid,
        "binding-preparation.custom-source.v1",
        &context,
        credential,
    )
    .ok_or(AdmissionError::Invalid)
}
