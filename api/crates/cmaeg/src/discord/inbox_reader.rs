//! Restore an immutable native inbox item into the ordinary routed admission contract.
use super::{
    journal_model::{Authority, Chunk, Inbox},
    normalize::{normalize, Normalized},
    record_store::Store,
    GatewayError,
};
use crate::{
    channels::{definition::hash_secret, resolver::ResolvedChannel},
    connections::store::index_id,
    ingress::{admission::Refusal, launch::LaunchRequest},
    records::RecordClient,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{
    channel::{ChannelDefinition, Source},
    ids::{ConversationId, IdempotencyKey},
    seal::{EventRef, EventRefPayload, MasterKeyRing, ReplyAddress},
};
use serde::Deserialize;
use std::{sync::Arc, time::Instant};

pub enum InboxLaunch {
    Launch(Box<LaunchRequest>),
    Refused(Refusal),
}
pub struct InboxReader {
    store: Store,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    dispatch: serde_json::Value,
    channel: ChannelDefinition,
}
impl InboxReader {
    pub fn new(records: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            store: Store::new(records, keys),
        }
    }
    pub async fn load(
        &self,
        id: &str,
        current: Arc<ResolvedChannel>,
        now: i64,
    ) -> Result<InboxLaunch, GatewayError> {
        let (inbox, version) = self
            .store
            .load::<Inbox>(id, true)
            .await?
            .ok_or(GatewayError::Unavailable)?;
        let authority = Authority::from_definition(&current.definition)?;
        if version != 1
            || inbox.kind != "discord_inbox_v1"
            || inbox.channel_record != authority.record
            || inbox.bot_id != authority.bot_id
            || inbox.source_revision != authority.revision
            || inbox.retained_until <= now
            || inbox.parts.is_empty()
            || inbox.parts.len() > 64
            || index_id(
                "discord-native-message-v1",
                &[&inbox.channel_record, &inbox.bot_id, &inbox.message_id],
            ) != id
        {
            return Err(GatewayError::Authority);
        }
        let mut bytes = Vec::new();
        for (index, part) in inbox.parts.iter().enumerate() {
            if *part
                != index_id(
                    "discord-native-message-chunk-v1",
                    &[id, &inbox.payload_hash, &index.to_string()],
                )
            {
                return Err(GatewayError::Protocol);
            }
            let (chunk, version) = self
                .store
                .load::<Chunk>(part, true)
                .await?
                .ok_or(GatewayError::Unavailable)?;
            if version != 1 || chunk.kind != "discord_inbox_chunk_v1" {
                return Err(GatewayError::Protocol);
            }
            let chunk = STANDARD
                .decode(chunk.bytes)
                .map_err(|_| GatewayError::Protocol)?;
            if chunk.len() > 32 * 1024 || bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(GatewayError::Protocol);
            }
            bytes.extend(chunk);
        }
        if hash_secret(std::str::from_utf8(&bytes).map_err(|_| GatewayError::Protocol)?)
            != inbox.payload_hash
        {
            return Err(GatewayError::Protocol);
        }
        let mut body: Body = serde_json::from_slice(&bytes).map_err(|_| GatewayError::Protocol)?;
        if body.channel.record_id != current.pointer.record_id
            || body.channel.owner != current.definition.owner
            || body.dispatch["id"].as_str() != Some(&inbox.message_id)
            || body.channel.secrets.agent_key_hash != current.definition.secrets.agent_key_hash
        {
            return Err(GatewayError::Authority);
        }
        // Routing/profile/admission remain the accepted snapshot. Only currently
        // authorized credentials are supplied, never stale secrets from an inbox.
        body.channel.secrets = current.definition.secrets.clone();
        body.channel
            .validate()
            .map_err(|_| GatewayError::Authority)?;
        crate::channels::definition::validate_consumer_keys(&body.channel)
            .map_err(|_| GatewayError::Authority)?;
        let Source::Discord { admission, .. } = body.channel.sources.iter()
            .find(|source| matches!(source, Source::Discord { bot_id, .. } if *bot_id == inbox.bot_id))
            .ok_or(GatewayError::Authority)? else { unreachable!() };
        let activity = match normalize(&body.dispatch, &inbox.bot_id, admission)? {
            Normalized::Activity(activity) => *activity,
            Normalized::Refused(reason) => return Ok(InboxLaunch::Refused(reason)),
        };
        if body.channel.partition.key_for(&activity.source)
            == cmaeg_core::channel::PartitionKey::Thread
        {
            return Ok(InboxLaunch::Refused(Refusal::ThreadEvidenceUnavailable));
        }
        let partition_key = activity.partition_key(&body.channel.partition);
        let conversation_id = ConversationId::derive(
            &self.store.keys,
            &current.id,
            body.channel.partition.version,
            &partition_key,
        );
        let event_ref = EventRef::seal(
            &EventRefPayload {
                telegram_admission: None,
                v: 1,
                activity_sha256: Some(cmaeg_core::seal::event_ref::activity_digest(&activity)),
                channel_id: current.id.clone(),
                record_id: current.pointer.record_id.clone(),
                event_id: activity.event_id.clone(),
                partition_key,
                conversation_id: conversation_id.clone(),
                expires_at: (now + 1800).min(inbox.retained_until),
                reply: ReplyAddress::Discord {
                    bot_id: inbox.bot_id,
                    channel_id: activity.conversation.id.clone(),
                    message_id: inbox.message_id,
                },
                consumer: None,
                lineage: None,
            },
            &self.store.keys,
        )
        .map_err(|_| GatewayError::Protocol)?;
        Ok(InboxLaunch::Launch(Box::new(LaunchRequest {
            idempotency_key: IdempotencyKey::from_event(&activity.event_id),
            channel: Arc::new(ResolvedChannel {
                id: current.id.clone(),
                pointer: current.pointer.clone(),
                definition: body.channel,
                agent_key_hash: current.agent_key_hash,
                consumer_keys: current.consumer_keys.clone(),
                loaded_at: current.loaded_at,
                version: current.version,
            }),
            activity,
            conversation_id,
            event_ref,
            received_at: Instant::now(),
            trace: tracing::Span::current(),
            timing: None,
            consumer: None,
        })))
    }
}
