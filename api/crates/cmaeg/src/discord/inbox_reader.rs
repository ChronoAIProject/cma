//! Restore an immutable native inbox item into the ordinary routed admission contract.
use super::{
    journal_model::{Authority, Chunk, Inbox},
    normalize::{normalize, normalize_with_content, normalize_with_projections, Normalized},
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
    Launch(Box<VerifiedInboxLaunch>),
    Refused(Refusal),
}
/// Verified immutable intake for durable routed admission. These bytes are the
/// stored dispatch + public channel snapshot, not original WebSocket wire octets.
pub struct VerifiedInboxLaunch {
    pub(crate) quote_projection: Option<crate::ingress::quoted_context::Snapshot>,
    pub request: LaunchRequest,
    pub source_bytes: Vec<u8>,
    pub received_at: i64,
    pub(crate) outbound_consumer: Option<cmaeg_core::ids::ConsumerId>,
    pub(crate) original_message: Option<crate::ingress::routing_store::EventIdentity>,
}
pub struct InboxReader {
    pub(super) store: Store,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Body {
    #[serde(default)]
    pub content_projection: Option<super::content::Snapshot>,
    #[serde(default)]
    pub forward_projection: Option<super::forward::Snapshot>,
    #[serde(default)]
    pub quote_projection: Option<crate::ingress::quoted_context::Snapshot>,
    #[serde(default)]
    pub pending_reaction: Option<super::reaction_pending::Pending>,
    pub dispatch: serde_json::Value,
    pub channel: ChannelDefinition,
    #[serde(default)]
    pub native_channel: Option<super::thread_context::ChannelEvidence>,
    #[serde(default)]
    pub component: Option<super::components::Snapshot>,
    #[serde(default)]
    pub message_event: Option<super::message_event::Snapshot>,
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
        let (inbox, body, bytes) = self.restore(id, &current, now).await?;
        let expires_at = inbox
            .received_at
            .checked_add(1800)
            .ok_or(GatewayError::Protocol)?
            .min(inbox.retained_until);
        if expires_at <= now {
            return Err(GatewayError::Authority);
        }
        let body = self.resolve_reaction(body, &inbox, now).await?;
        let activity = match body.activity(&inbox.bot_id)? {
            Normalized::Activity(activity) => *activity,
            Normalized::Refused(reason) => return Ok(InboxLaunch::Refused(reason)),
        };
        self.launch(current, inbox, body, bytes, activity)
    }

    /// Receipt recovery does not renew or require an unexpired reply capability.
    /// Its payload and current source authority are verified before observation.
    pub(super) async fn observe(
        &self,
        id: &str,
        current: &ResolvedChannel,
        now: i64,
        observer: &dyn crate::turns::observer::ActivityObserver,
    ) -> Result<(), GatewayError> {
        let (inbox, body, _) = self.restore(id, current, now).await?;
        if let Ok(Normalized::Activity(activity)) = body.activity(&inbox.bot_id) {
            observer
                .receive_at(
                    &body.channel.owner.subject,
                    &body.channel.record_id,
                    &activity,
                    inbox.received_at,
                )
                .await
                .map_err(|_| GatewayError::Unavailable)?;
        }
        Ok(())
    }

    async fn restore(
        &self,
        id: &str,
        current: &ResolvedChannel,
        now: i64,
    ) -> Result<(Inbox, Body, Vec<u8>), GatewayError> {
        let (inbox, version) = self
            .store
            .load::<Inbox>(id, true)
            .await?
            .ok_or(GatewayError::Unavailable)?;
        let authority = Authority::from_definition(&current.definition)?;
        if version != 1
            || inbox.id().is_err()
            || inbox.channel_record != authority.record
            || inbox.bot_id != authority.bot_id
            || inbox.source_revision != authority.revision
            || inbox.received_at < 0
            || inbox.received_at > now
            || inbox.received_at >= inbox.retained_until
            || inbox.retained_until <= now
            || inbox.parts.is_empty()
            || inbox.parts.len() > 64
            || inbox.id()? != id
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
            || body.message_id() != Some(&inbox.message_id)
        {
            return Err(GatewayError::Authority);
        }
        if (body.message_event.is_some() || body.pending_reaction.is_some())
            != inbox.change_id.is_some()
        {
            return Err(GatewayError::Protocol);
        }
        if body.pending_reaction.is_some() {
            if inbox.kind != "discord_reaction_pending_v1"
                || body.message_event.is_some()
                || body.component.is_some()
                || body.native_channel.is_some()
            {
                return Err(GatewayError::Protocol);
            }
            body.channel = current.definition.clone();
        } else if let Some(change) = &body.message_event {
            change.validate(&inbox.bot_id, &inbox.message_id)?;
            if inbox.change_id.as_deref() != Some(change.activity.event_id.as_str()) {
                return Err(GatewayError::Protocol);
            }
            // Retained target selection is loaded from the original admission;
            // only current credentials and source authority may execute it.
            body.channel = current.definition.clone();
        } else if let Some(component) = &body.component {
            component.validate(&inbox.bot_id, &inbox.message_id)?;
            if component.identity.source_revision != inbox.source_revision
                || current.definition.routing.is_none()
                || current
                    .consumer(Some(&component.identity.consumer))
                    .is_none_or(|consumer| {
                        consumer.pending
                            || consumer.credential.is_none()
                            || consumer.provider != &component.identity.provider
                    })
            {
                return Err(GatewayError::Authority);
            }
            // The component owns its immutable single target, conversation and
            // partition; routing is never evaluated again. Use the current
            // serving definition so retired unrelated consumers are absent and
            // rotated credentials agree with their public/decoded key hashes.
            // The original bytes below remain the durable admission identity.
            body.channel = current.definition.clone();
        } else {
            if body.channel.secrets.agent_key_hash != current.definition.secrets.agent_key_hash {
                return Err(GatewayError::Authority);
            }
            // Ordinary messages retain their accepted routing/profile/admission
            // snapshot, with only currently authorized credentials supplied.
            body.channel.secrets = current.definition.secrets.clone();
        }
        body.channel
            .validate()
            .map_err(|_| GatewayError::Authority)?;
        crate::channels::definition::validate_consumer_keys(&body.channel)
            .map_err(|_| GatewayError::Authority)?;
        Ok((inbox, body, bytes))
    }

    fn launch(
        &self,
        current: Arc<ResolvedChannel>,
        inbox: Inbox,
        body: Body,
        bytes: Vec<u8>,
        activity: cmaeg_core::activity::Activity,
    ) -> Result<InboxLaunch, GatewayError> {
        let expires_at = inbox
            .received_at
            .checked_add(1800)
            .ok_or(GatewayError::Protocol)?
            .min(inbox.retained_until);
        let partition_key = body.component.as_ref().map_or_else(
            || activity.partition_key(&body.channel.partition),
            |component| component.identity.partition_key.clone(),
        );
        let conversation_id = body.component.as_ref().map_or_else(
            || {
                ConversationId::derive(
                    &self.store.keys,
                    &current.id,
                    body.channel.partition.version,
                    &partition_key,
                )
            },
            |component| component.identity.conversation_id.clone(),
        );
        let consumer = body.component.as_ref().and_then(|component| {
            (!component.identity.consumer.is_default())
                .then_some(component.identity.consumer.clone())
        });
        let reply_message = body.component.as_ref().map_or_else(
            || inbox.message_id.clone(),
            |component| component.message_id.clone(),
        );
        let event_ref = EventRef::seal(
            &EventRefPayload {
                telegram_admission: None,
                turn_trace: None,
                v: 1,
                activity_sha256: Some(cmaeg_core::seal::event_ref::activity_digest(&activity)),
                channel_id: current.id.clone(),
                record_id: current.pointer.record_id.clone(),
                event_id: activity.event_id.clone(),
                partition_key,
                conversation_id: conversation_id.clone(),
                expires_at,
                reply: ReplyAddress::Discord {
                    bot_id: inbox.bot_id,
                    channel_id: activity.conversation.id.clone(),
                    message_id: reply_message,
                    thread_id: activity.conversation.thread_id.clone(),
                },
                consumer: consumer.clone(),
                lineage: None,
            },
            &self.store.keys,
        )
        .map_err(|_| GatewayError::Protocol)?;
        let original_message = body.message_event.as_ref().map(|change| {
            crate::ingress::routing_store::EventIdentity {
                owner: body.channel.owner.subject.clone(),
                channel_id: current.id.clone(),
                record_id: current.pointer.record_id.clone(),
                event_id: change.original_event_id.clone(),
            }
        });
        let outbound_consumer = body
            .message_event
            .as_ref()
            .and_then(|event| event.outbound_consumer.clone());
        let quote_projection = body.quote_projection.clone();
        let request = LaunchRequest {
            idempotency_key: consumer.as_ref().map_or_else(
                || IdempotencyKey::from_event(&activity.event_id),
                |consumer| IdempotencyKey::for_consumer(&activity.event_id, consumer),
            ),
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
            // Local processing latency has a monotonic timer; the immutable
            // UTC receipt below survives restarts with a different clock epoch.
            received_at: Instant::now(),
            trace: tracing::Span::current(),
            timing: None,
            turn_trace: None,
            consumer,
        };
        Ok(InboxLaunch::Launch(Box::new(VerifiedInboxLaunch {
            quote_projection,
            request,
            source_bytes: bytes,
            received_at: inbox.received_at,
            original_message,
            outbound_consumer,
        })))
    }
}

impl Body {
    pub(super) fn message_id(&self) -> Option<&str> {
        let reaction = self.pending_reaction.is_some() || self.message_event.as_ref().is_some_and(|event| {
            matches!(&event.activity.kind, cmaeg_core::activity::ActivityKind::Event { ce, .. } if ce.r#type == "cma.message.reaction.updated.v1")
        });
        self.dispatch[if reaction { "message_id" } else { "id" }].as_str()
    }
    pub(super) fn activity(&self, bot: &str) -> Result<Normalized, GatewayError> {
        if (self.content_projection.is_some() || self.forward_projection.is_some())
            && (self.component.is_some()
                || self.message_event.is_some()
                || self.pending_reaction.is_some())
        {
            return Err(GatewayError::Protocol);
        }
        if self.pending_reaction.is_some() {
            return Err(GatewayError::Unavailable);
        }
        if self.message_event.is_some()
            && (self.component.is_some() || self.native_channel.is_some())
        {
            return Err(GatewayError::Protocol);
        }
        if let Some(change) = &self.message_event {
            let id = self.message_id().ok_or(GatewayError::Protocol)?;
            change.validate(bot, id)?;
            return Ok(Normalized::Activity(Box::new(change.activity.clone())));
        }
        if self.component.is_some() && self.native_channel.is_some() {
            return Err(GatewayError::Protocol);
        }
        if let Some(component) = &self.component {
            let id = self.message_id().ok_or(GatewayError::Protocol)?;
            component.validate(bot, id)?;
            return Ok(Normalized::Activity(Box::new(component.activity.clone())));
        }
        let Source::Discord { admission, .. } = self
            .channel
            .sources
            .iter()
            .find(|source| matches!(source, Source::Discord { bot_id, .. } if bot_id == bot))
            .ok_or(GatewayError::Authority)?
        else {
            unreachable!()
        };
        let mut activity = match (&self.content_projection, &self.forward_projection) {
            (None, None) => normalize(&self.dispatch, bot, admission)?,
            (content, None) => {
                normalize_with_content(&self.dispatch, bot, admission, content.as_ref())?
            }
            (content, forward) => normalize_with_projections(
                &self.dispatch,
                bot,
                admission,
                content.as_ref(),
                forward.as_ref(),
            )?,
        };
        if let Normalized::Activity(activity) = &mut activity {
            if let Some(snapshot) = &self.quote_projection {
                snapshot
                    .apply(activity)
                    .map_err(|_| GatewayError::Protocol)?;
            }
            let proven = self
                .native_channel
                .as_ref()
                .is_some_and(|evidence| evidence.apply(&self.dispatch, activity));
            if !proven
                && self.channel.partition.key_for(&activity.source)
                    == cmaeg_core::channel::PartitionKey::Thread
            {
                return Ok(Normalized::Refused(Refusal::ThreadEvidenceUnavailable));
            }
        }
        Ok(activity)
    }
}
