//! Missing target receipts wait durably without holding the Gateway checkpoint.
use super::{
    inbox_reader::{Body, InboxReader},
    journal::DurableJournal,
    journal_model::{Authority, Chunk, Inbox},
    reactions,
    record_store::Store,
    Dispatch, GatewayError,
};
use crate::{channels::definition::hash_secret, connections::store::index_id};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{activity::Actor, ids::EventId};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub(super) const TARGET_WAIT_SECONDS: i64 = 60;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pending {
    pub event_id: EventId,
    pub name: String,
    pub actor: Option<Actor>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Retry {
    kind: String,
    inbox_id: String,
    payload_hash: String,
    attempts: u32,
    next_at: i64,
}
fn pending_id(authority: &Authority, event: &EventId) -> String {
    index_id(
        "discord-native-reaction-pending-v1",
        &[&authority.record, &authority.bot_id, event.as_str()],
    )
}
impl DurableJournal {
    pub(super) async fn pending_reaction_replay(
        &self,
        authority: &Authority,
        event: &Dispatch,
        id: &EventId,
    ) -> Result<bool, GatewayError> {
        let key = pending_id(authority, id);
        let Some((inbox, version)) = self.store.load::<Inbox>(&key, true).await? else {
            return Ok(false);
        };
        let message = super::normalize::snowflake(&event.data["message_id"])?;
        if version != 1 || inbox.id()? != key || inbox.source_revision != authority.revision {
            return Err(GatewayError::Authority);
        }
        let bytes = self.verify_inbox(&inbox, authority, message).await?;
        let body: Body = serde_json::from_slice(&bytes).map_err(|_| GatewayError::Protocol)?;
        if body.dispatch != event.data
            || body
                .pending_reaction
                .as_ref()
                .is_none_or(|pending| pending.event_id != *id || pending.name != event.name)
        {
            return Err(GatewayError::Protocol);
        }
        Ok(true)
    }
    pub(super) async fn persist_pending_reaction(
        &self,
        authority: &Authority,
        event: &Dispatch,
        event_id: EventId,
        actor: Option<Actor>,
    ) -> Result<(), GatewayError> {
        let id = pending_id(authority, &event_id);
        let now = (self.store.clock)() / 1000;
        let expiry = now + super::journal_model::RETENTION_SECONDS;
        let body = json!({"dispatch":event.data,"channel":authority.public_definition,
            "pending_reaction":Pending { event_id:event_id.clone(),name:event.name.clone(),actor }});
        let bytes = serde_json::to_vec(&body).map_err(|_| GatewayError::Protocol)?;
        if bytes.len() > crate::ingress::routing_store::MAX_SOURCE_BYTES {
            return Err(GatewayError::Protocol);
        }
        let payload_hash =
            hash_secret(std::str::from_utf8(&bytes).map_err(|_| GatewayError::Protocol)?);
        let mut parts = Vec::new();
        for (index, chunk) in bytes.chunks(32 * 1024).enumerate() {
            let part = index_id(
                "discord-native-message-chunk-v1",
                &[&id, &payload_hash, &index.to_string()],
            );
            self.store
                .immutable(
                    &part,
                    &authority.owner,
                    &Chunk {
                        kind: "discord_inbox_chunk_v1".into(),
                        bytes: STANDARD.encode(chunk),
                    },
                    expiry,
                )
                .await?;
            parts.push(part);
        }
        let inbox = Inbox {
            kind: "discord_reaction_pending_v1".into(),
            channel_record: authority.record.clone(),
            bot_id: authority.bot_id.clone(),
            message_id: super::normalize::snowflake(&event.data["message_id"])?.into(),
            change_id: Some(event_id.to_string()),
            source_revision: authority.revision.clone(),
            payload_hash,
            parts,
            received_at: now,
            retained_until: expiry,
        };
        match self
            .store
            .put(&id, &authority.owner, &inbox, None, Some(expiry))
            .await
        {
            Ok(1) => Ok(()),
            Ok(_) => Err(GatewayError::Protocol),
            Err(error) => {
                if self
                    .pending_reaction_replay(authority, event, &event_id)
                    .await?
                {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }
}
impl InboxReader {
    pub(super) async fn resolve_reaction(
        &self,
        mut body: Body,
        inbox: &Inbox,
        now: i64,
    ) -> Result<Body, GatewayError> {
        let Some(pending) = body.pending_reaction.take() else {
            return Ok(body);
        };
        if inbox.kind != "discord_reaction_pending_v1"
            || inbox.change_id.as_deref() != Some(pending.event_id.as_str())
            || !reactions::is_event(&pending.name)
            || now >= inbox.received_at + TARGET_WAIT_SECONDS
        {
            return Err(GatewayError::Authority);
        }
        self.store
            .reaction_retry(inbox, &body.channel.owner.subject, now)
            .await?;
        let authority = Authority::from_definition(&body.channel)?;
        let channel = super::normalize::snowflake(&body.dispatch["channel_id"])?;
        let target = self
            .store
            .reaction_target(&authority, channel, &inbox.message_id)
            .await?
            .ok_or(GatewayError::Unavailable)?;
        let snapshot = reactions::normalize(
            &authority,
            &pending.name,
            &body.dispatch,
            target,
            pending.actor,
            pending.event_id,
            inbox.received_at,
        )?;
        body.message_event = Some(snapshot);
        Ok(body)
    }
}
impl Store {
    async fn reaction_retry(
        &self,
        inbox: &Inbox,
        owner: &str,
        now: i64,
    ) -> Result<(), GatewayError> {
        let inbox_id = inbox.id()?;
        let id = index_id(
            "discord-reaction-target-retry-v1",
            &[&inbox_id, &inbox.payload_hash],
        );
        let previous = self.load::<Retry>(&id, true).await?;
        let (attempts, version) = match previous {
            Some((retry, version)) => {
                if retry.kind != "discord_reaction_target_retry_v1"
                    || retry.inbox_id != inbox_id
                    || retry.payload_hash != inbox.payload_hash
                    || retry.attempts > 64
                {
                    return Err(GatewayError::Protocol);
                }
                if retry.next_at > now {
                    return Err(GatewayError::Unavailable);
                }
                (retry.attempts + 1, Some(version))
            }
            None => (1, None),
        };
        let retry = Retry {
            kind: "discord_reaction_target_retry_v1".into(),
            inbox_id,
            payload_hash: inbox.payload_hash.clone(),
            attempts,
            next_at: now + (1_i64 << attempts.saturating_sub(1).min(3)),
        };
        self.put(&id, owner, &retry, version, Some(inbox.retained_until))
            .await
            .map(|_| ())
    }
}
