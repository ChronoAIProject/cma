//! Durable native inbox with immutable chunk bodies and source/message dedup.
use super::{
    journal::DurableJournal,
    journal_model::{Authority, Chunk, Inbox, RETENTION_SECONDS},
    Dispatch, GatewayError,
};
use crate::{channels::definition::hash_secret, connections::store::index_id};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::json;
impl DurableJournal {
    pub(super) async fn persist_message(
        &self,
        authority: &Authority,
        event: &Dispatch,
    ) -> Result<(), GatewayError> {
        let message = event.message_id().ok_or(GatewayError::Protocol)?;
        // Discord IDs embed their creation time. Once the seven-day dedup
        // window expires, old replays are refused instead of launching twice.
        let created = (message.parse::<u64>().map_err(|_| GatewayError::Protocol)? >> 22) as i64
            + 1_420_070_400_000;
        let expiry = created / 1000 + RETENTION_SECONDS;
        if expiry <= (self.store.clock)() / 1000 || created > (self.store.clock)() + 300_000 {
            return Err(GatewayError::Protocol);
        }
        // The record API caps expiry at seven days from its current clock.
        // Wait for a small clock skew rather than writing an overlong TTL.
        if created / 1000 > (self.store.clock)() / 1000 {
            return Err(GatewayError::Unavailable);
        }
        let id = index_id(
            "discord-native-message-v1",
            &[&authority.record, &authority.bot_id, message],
        );
        if let Some((existing, 1)) = self.store.load::<Inbox>(&id, true).await? {
            return self.verify_inbox(&existing, authority, message).await;
        }
        let bytes = serde_json::to_vec(
            &json!({"dispatch":event.data,"channel":authority.public_definition}),
        )
        .map_err(|_| GatewayError::Protocol)?;
        if bytes.len() > 2 * 1024 * 1024 {
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
            kind: "discord_inbox_v1".into(),
            channel_record: authority.record.clone(),
            bot_id: authority.bot_id.clone(),
            message_id: message.into(),
            source_revision: authority.revision.clone(),
            payload_hash,
            parts,
            retained_until: expiry,
        };
        match self
            .store
            .put(&id, &authority.owner, &inbox, None, Some(expiry))
            .await
        {
            Ok(1) => Ok(()),
            Ok(_) => Err(GatewayError::Protocol),
            Err(error) => match self.store.load::<Inbox>(&id, true).await? {
                Some((existing, 1)) => self.verify_inbox(&existing, authority, message).await,
                Some(_) => Err(GatewayError::Protocol),
                None => Err(error),
            },
        }
    }
    async fn verify_inbox(
        &self,
        inbox: &Inbox,
        authority: &Authority,
        message: &str,
    ) -> Result<(), GatewayError> {
        if inbox.kind != "discord_inbox_v1"
            || inbox.channel_record != authority.record
            || inbox.bot_id != authority.bot_id
            || inbox.message_id != message
            || inbox.retained_until <= (self.store.clock)() / 1000
            || inbox.parts.is_empty()
            || inbox.parts.len() > 64
        {
            return Err(GatewayError::Protocol);
        }
        // Restore the original snapshot even after a policy or credential edit;
        // a duplicate must never route again using the new policy.
        let mut bytes = Vec::new();
        for part in &inbox.parts {
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
        Ok(())
    }
}
