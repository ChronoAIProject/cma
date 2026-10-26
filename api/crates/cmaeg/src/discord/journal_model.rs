//! Private persisted records. No record or checkpoint implements Debug.
use super::{Checkpoint, GatewayError};
use cmaeg_core::channel::{ChannelDefinition, Secret, Source};
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
            bot_id, intents, ..
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
    pub source_revision: String,
    pub payload_hash: String,
    pub parts: Vec<String>,
    pub retained_until: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Chunk {
    pub kind: String,
    pub bytes: String,
}
