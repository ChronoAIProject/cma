//! Private persisted records. No record or checkpoint implements Debug.
use super::{Checkpoint, GatewayError};
use cmaeg_core::channel::{AdmissionPolicy, ChannelDefinition, Secret, Source};
use serde::{Deserialize, Serialize};

pub(super) const LEASE_MS: i64 = 30_000;
pub(super) const RETENTION_SECONDS: i64 = 7 * 86_400;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Lease {
    pub kind: String,
    pub bot_id: String,
    pub owner: String,
    pub channel_record: String,
    pub revision: String,
    pub worker: String,
    pub epoch: u64,
    pub lease_until: i64,
    pub remaining: u64,
    pub reset_at: i64,
    pub identify_after: i64,
    pub authorization: Option<String>,
    pub checkpoint: Option<Checkpoint>,
}
#[derive(Clone)]
pub(super) struct Authority {
    pub owner: String,
    pub record: String,
    pub bot_id: String,
    pub intents: u64,
    pub revision: String,
    pub token: Secret,
    pub admission: AdmissionPolicy,
    pub public_definition: serde_json::Value,
}
impl Authority {
    pub fn from_definition(definition: &ChannelDefinition) -> Result<Self, GatewayError> {
        definition.validate().map_err(|_| GatewayError::Authority)?;
        if definition
            .creation
            .as_ref()
            .is_some_and(|creation| !creation.ready)
        {
            return Err(GatewayError::Authority);
        }
        let source = definition
            .sources
            .iter()
            .find(|s| matches!(s, Source::Discord { .. }))
            .ok_or(GatewayError::Authority)?;
        let Source::Discord {
            bot_id,
            intents,
            admission,
            ..
        } = source
        else {
            unreachable!()
        };
        let token = definition
            .secrets
            .discord_bot_token
            .clone()
            .ok_or(GatewayError::Authority)?;
        let revision = crate::channels::definition::hash_secret(&serde_json::json!({"owner":definition.owner,"source":source,"credential":crate::channels::definition::hash_secret(token.as_str())}).to_string());
        Ok(Self {
            owner: definition.owner.subject.clone(),
            record: definition.record_id.to_string(),
            bot_id: bot_id.clone(),
            intents: *intents,
            revision,
            token,
            admission: admission.clone(),
            public_definition: definition.to_public_json(),
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Inbox {
    pub kind: String,
    pub channel_record: String,
    pub bot_id: String,
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_id: Option<String>,
    pub source_revision: String,
    pub payload_hash: String,
    pub parts: Vec<String>,
    /// Immutable first durable receipt; old native records without it are refused.
    pub received_at: i64,
    pub retained_until: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Chunk {
    pub kind: String,
    pub bytes: String,
}

impl Inbox {
    pub(super) fn id(&self) -> Result<String, GatewayError> {
        let (domain, identity) = match (self.kind.as_str(), &self.change_id) {
            ("discord_inbox_v1", None) => ("discord-native-message-v1", self.message_id.as_str()),
            ("discord_reaction_pending_v1", Some(change))
                if !change.is_empty() && change.len() <= 512 =>
            {
                ("discord-native-reaction-pending-v1", change.as_str())
            }
            ("discord_message_event_inbox_v1", Some(change))
                if !change.is_empty() && change.len() <= 512 =>
            {
                ("discord-native-message-event-v1", change.as_str())
            }
            _ => return Err(GatewayError::Protocol),
        };
        Ok(crate::connections::store::index_id(
            domain,
            &[&self.channel_record, &self.bot_id, identity],
        ))
    }
}
