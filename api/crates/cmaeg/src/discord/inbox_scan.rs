//! Bounded discovery and immutable acknowledgements over the shared record domain.
use super::{journal_model::Inbox, record_store::Store, GatewayError};
use crate::{
    connections::store::index_id,
    records::{RecordClient, RecordError, RecordHead},
};
use cmaeg_core::seal::{open_consumer_interaction, open_interaction, MasterKeyRing};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub(super) struct Entry {
    pub id: String,
    pub owner: String,
    pub inbox: Inbox,
}
impl Entry {
    pub fn ack_id(&self) -> String {
        index_id(
            "discord-native-inbox-ack-v1",
            &[&self.id, &self.inbox.payload_hash],
        )
    }
    pub fn expired(&self, now: i64) -> bool {
        self.inbox
            .received_at
            .saturating_add(1800)
            .min(self.inbox.retained_until)
            <= now
    }
}
pub(super) struct Page {
    pub entries: Vec<Entry>,
    pub next_cursor: Option<String>,
    pub invalid: usize,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Disposition {
    Admitted { admission_id: String },
    Refused { reason: String },
    Expired,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ack {
    kind: String,
    inbox_id: String,
    payload_hash: String,
    source_revision: String,
    disposition: Disposition,
}
pub(super) struct InboxStore {
    store: Store,
}
impl InboxStore {
    pub fn new(records: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            store: Store::new(records, keys),
        }
    }
    pub async fn page(&self, after: Option<&str>, now: i64) -> Result<Page, GatewayError> {
        let heads = self
            .store
            .channels
            .clone()
            .interactions()
            .list(None, after, 32)
            .await
            .map_err(|_| GatewayError::Unavailable)?;
        if heads.len() > 32 {
            return Err(GatewayError::Protocol);
        }
        let mut previous = after;
        for head in &heads {
            if head.owner.is_empty()
                || head.owner.len() > 512
                || previous.is_some_and(|last| head.record_id.as_str() <= last)
            {
                return Err(GatewayError::Protocol);
            }
            previous = Some(&head.record_id);
        }
        let next_cursor = (heads.len() == 32).then(|| heads.last().unwrap().record_id.clone());
        let mut page = Page {
            entries: Vec::new(),
            next_cursor,
            invalid: 0,
        };
        for head in heads {
            match self.entry(head, now).await {
                Ok(Some(entry)) => page.entries.push(entry),
                Ok(None) => {}
                Err(GatewayError::Unavailable) => return Err(GatewayError::Unavailable),
                Err(_) => page.invalid += 1,
            }
        }
        Ok(page)
    }
    async fn entry(&self, head: RecordHead, now: i64) -> Result<Option<Entry>, GatewayError> {
        let records = self.store.channels.clone().interactions();
        let url = records
            .record_url(&head.record_id)
            .map_err(|_| GatewayError::Protocol)?;
        let row = match records.get(url.as_str()).await {
            Ok(row) => row,
            Err(RecordError::NotFound) => return Ok(None),
            Err(_) => return Err(GatewayError::Unavailable),
        };
        if row.blob.starts_with(b"cmaeg1.ic.") {
            open_consumer_interaction(&self.store.keys, &head.record_id, &row.blob)
                .map_err(|_| GatewayError::Protocol)?;
            return Ok(None);
        }
        let opened = open_interaction(&self.store.keys, &head.record_id, &row.blob)
            .map_err(|_| GatewayError::Protocol)?;
        let value: serde_json::Value =
            serde_json::from_slice(&opened.payload).map_err(|_| GatewayError::Protocol)?;
        if !matches!(
            value["kind"].as_str(),
            Some(
                "discord_inbox_v1"
                    | "discord_message_event_inbox_v1"
                    | "discord_reaction_pending_v1"
            )
        ) {
            return Ok(None);
        }
        let inbox: Inbox = serde_json::from_value(value).map_err(|_| GatewayError::Protocol)?;
        if row.version != 1
            || head.version != 1
            || inbox.received_at < 0
            || inbox.received_at > now
            || inbox.received_at >= inbox.retained_until
            || inbox.retained_until <= now
            || inbox.parts.is_empty()
            || inbox.parts.len() > 64
            || inbox.id()? != head.record_id
        {
            return Err(GatewayError::Protocol);
        }
        Ok(Some(Entry {
            id: head.record_id,
            owner: head.owner,
            inbox,
        }))
    }
    pub async fn settled(&self, entry: &Entry) -> Result<bool, GatewayError> {
        let Some((ack, version)) = self.store.load::<Ack>(&entry.ack_id(), true).await? else {
            return Ok(false);
        };
        if version != 1
            || ack.kind != "discord_inbox_ack_v1"
            || ack.inbox_id != entry.id
            || ack.payload_hash != entry.inbox.payload_hash
            || ack.source_revision != entry.inbox.source_revision
        {
            return Err(GatewayError::Protocol);
        }
        Ok(true)
    }
    pub async fn acknowledge(
        &self,
        entry: &Entry,
        disposition: Disposition,
    ) -> Result<(), GatewayError> {
        self.store
            .immutable(
                &entry.ack_id(),
                &entry.owner,
                &Ack {
                    kind: "discord_inbox_ack_v1".into(),
                    inbox_id: entry.id.clone(),
                    payload_hash: entry.inbox.payload_hash.clone(),
                    source_revision: entry.inbox.source_revision.clone(),
                    disposition,
                },
                entry.inbox.retained_until,
            )
            .await
    }
}
