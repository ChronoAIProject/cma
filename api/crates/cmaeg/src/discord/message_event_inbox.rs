//! Immutable intake of message edits/deletions; ordinary inbox IDs never collide.
use super::{
    inbox_reader::Body,
    journal::DurableJournal,
    journal_model::{Authority, Chunk, Inbox},
    message_event::normalize_change,
    Dispatch, GatewayError,
};
use crate::{channels::definition::hash_secret, connections::store::index_id};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};

impl DurableJournal {
    pub(super) async fn persist_message_event(
        &self,
        authority: &Authority,
        event: &Dispatch,
    ) -> Result<(), GatewayError> {
        if event.name == "MESSAGE_DELETE_BULK" {
            let Some(ids) = event.data["ids"]
                .as_array()
                .filter(|ids| !ids.is_empty() && ids.len() <= 100)
            else {
                return Ok(());
            };
            // Validate the whole batch before taking custody of any member.
            let mut seen = std::collections::BTreeSet::new();
            for id in ids {
                let Some(id) = super::protocol::snowflake(id) else {
                    return Ok(());
                };
                if !seen.insert(id) {
                    return Ok(());
                }
            }
            for id in ids {
                let mut raw = event.data.clone();
                raw.as_object_mut()
                    .ok_or(GatewayError::Protocol)?
                    .remove("ids");
                raw["id"] = id.clone();
                self.persist_change(authority, "MESSAGE_DELETE", &raw)
                    .await?;
            }
            return Ok(());
        }
        self.persist_change(authority, &event.name, &event.data)
            .await
    }
    async fn persist_change(
        &self,
        authority: &Authority,
        name: &str,
        raw: &Value,
    ) -> Result<(), GatewayError> {
        let Some(message) = super::protocol::snowflake(&raw["id"]) else {
            return Ok(());
        };
        let original_id = index_id(
            "discord-native-message-v1",
            &[&authority.record, &authority.bot_id, message],
        );
        let Some((original, 1)) = self.store.load::<Inbox>(&original_id, true).await? else {
            return Ok(());
        };
        if original.kind != "discord_inbox_v1" || original.source_revision != authority.revision {
            return Ok(());
        }
        let bytes = self.verify_inbox(&original, authority, message).await?;
        let body: Body = serde_json::from_slice(&bytes).map_err(|_| GatewayError::Protocol)?;
        if body.channel.owner.subject != authority.owner
            || body.channel.record_id.as_str() != authority.record
        {
            return Err(GatewayError::Authority);
        }
        let now = (self.store.clock)() / 1000;
        let change = match normalize_change(&body, &authority.bot_id, name, raw, now) {
            Ok(Some(change)) => change,
            Ok(None) | Err(GatewayError::Protocol) => return Ok(()),
            Err(error) => return Err(error),
        };
        self.persist_change_snapshot(authority, message, raw, change, original.retained_until)
            .await
    }
    pub(super) async fn persist_change_snapshot(
        &self,
        authority: &Authority,
        message: &str,
        raw: &Value,
        change: super::message_event::Snapshot,
        expiry: i64,
    ) -> Result<(), GatewayError> {
        let now = (self.store.clock)() / 1000;
        let change_id = change.activity.event_id.to_string();
        let id = index_id(
            "discord-native-message-event-v1",
            &[&authority.record, &authority.bot_id, &change_id],
        );
        if let Some((existing, 1)) = self.store.load::<Inbox>(&id, true).await? {
            if existing.id()? != id || existing.source_revision != authority.revision {
                return Err(GatewayError::Protocol);
            }
            let bytes = self.verify_inbox(&existing, authority, message).await?;
            return self.observe_inbox(&existing, authority, &bytes).await;
        }
        let body =
            json!({"dispatch":raw,"channel":authority.public_definition,"message_event":change});
        let bytes = serde_json::to_vec(&body).map_err(|_| GatewayError::Protocol)?;
        if bytes.len() > crate::ingress::routing_store::MAX_SOURCE_BYTES {
            return Ok(());
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
            kind: "discord_message_event_inbox_v1".into(),
            channel_record: authority.record.clone(),
            bot_id: authority.bot_id.clone(),
            message_id: message.into(),
            change_id: Some(change_id),
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
            Ok(1) => self.observe_inbox(&inbox, authority, &bytes).await,
            Ok(_) => Err(GatewayError::Protocol),
            Err(error) => match self.store.load::<Inbox>(&id, true).await? {
                Some((existing, 1))
                    if existing.id()? == id && existing.source_revision == authority.revision =>
                {
                    let bytes = self.verify_inbox(&existing, authority, message).await?;
                    self.observe_inbox(&existing, authority, &bytes).await
                }
                Some(_) => Err(GatewayError::Protocol),
                None => Err(error),
            },
        }
    }
}
