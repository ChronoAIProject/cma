use super::{model::*, ChannelActivity};
use crate::{
    ingress::launch::LaunchRequest,
    records::{RecordClient, RecordError},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use cmaeg_core::{
    ids::RecordId,
    seal::{open_blob, open_interaction, seal_interaction, MasterKeyRing},
};
use futures_util::future::try_join_all;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{broadcast, Mutex};

#[path = "batch.rs"]
mod batch;
#[path = "history_backfill.rs"]
mod history_backfill;
#[path = "receipt.rs"]
mod receipt;
#[path = "refresh.rs"]
mod refresh;
#[cfg(test)]
#[path = "semantic_tests.rs"]
mod semantic_tests;

const KIND: &str = "channel_recent_activity_v1";
const MAX_PLAINTEXT: usize = 44_000;
type Contexts = BTreeMap<(String, String), (String, i64)>;

#[derive(Clone)]
pub struct ActivityStore {
    pub history: Arc<super::history::ActivityHistory>,
    execution_attribution_enabled: bool,
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    writes: Arc<[Mutex<()>; 64]>,
    updates: broadcast::Sender<String>,
    contexts: Arc<std::sync::Mutex<Contexts>>,
}
#[derive(Serialize, Deserialize)]
struct Bucket {
    kind: String,
    owner: String,
    record: String,
    day: i64,
    has_more: bool,
    items: Vec<ChannelActivity>,
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    record: String,
    day: i64,
    versions: Vec<u64>,
}

impl ActivityStore {
    pub fn new(records: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        let (updates, _) = broadcast::channel(256);
        Self {
            history: Arc::new(super::history::ActivityHistory::new(
                None,
                keys.clone(),
                Default::default(),
            )),
            execution_attribution_enabled: false,
            records,
            keys,
            writes: Arc::new(std::array::from_fn(|_| Mutex::new(()))),
            updates,
            contexts: Arc::default(),
        }
    }
    pub(crate) fn with_execution_attribution(mut self, enabled: bool) -> Self {
        self.execution_attribution_enabled = enabled;
        self
    }
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.updates.subscribe()
    }

    pub async fn latest(&self, owner: &str, record: &str) -> Result<ActivityPage, RecordError> {
        let today = today();
        let buckets = try_join_all(
            (0..RETENTION_DAYS).map(|offset| self.read(owner, record, today - offset)),
        )
        .await?;
        let versions = buckets
            .iter()
            .map(|value| value.as_ref().map_or(0, |(_, version)| *version))
            .collect();
        let mut items = Vec::new();
        let mut has_more = false;
        for (bucket, _) in buckets.into_iter().flatten() {
            has_more |= bucket.has_more;
            items.extend(bucket.items);
        }
        items.sort_by(|a, b| {
            b.received_at
                .cmp(&a.received_at)
                .then_with(|| a.event_id.cmp(&b.event_id))
        });
        has_more |= items.len() > LIMIT;
        items.truncate(LIMIT);
        for consumer in items.iter_mut().flat_map(|row| &mut row.consumers) {
            // A projected field is never authoritative storage.
            consumer.execution_reference = None;
            if !self.execution_attribution_enabled {
                consumer.execution_attributions.clear();
            }
        }
        let cursor = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&Cursor {
                record: record_hash(record),
                day: today,
                versions,
            })
            .map_err(invalid)?,
        );
        Ok(ActivityPage {
            items,
            cursor,
            retention_days: RETENTION_DAYS as u8,
            limit: LIMIT as u16,
            has_more,
        })
    }

    pub fn cursor_reset(&self, record: &str, cursor: &str) -> Result<bool, RecordError> {
        if cursor.len() > 1024 {
            return Err(malformed());
        }
        let parsed: Cursor =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cursor).map_err(invalid)?)
                .map_err(invalid)?;
        if parsed.record != record_hash(record)
            || parsed.versions.len() != RETENTION_DAYS as usize
            || parsed.day > today()
        {
            return Err(malformed());
        }
        Ok(parsed.day < today() - RETENTION_DAYS + 1)
    }

    pub async fn received(&self, request: &LaunchRequest) -> Result<(), RecordError> {
        let elapsed =
            time::Duration::try_from(request.received_at.elapsed()).unwrap_or(time::Duration::ZERO);
        let received = time::OffsetDateTime::now_utc() - elapsed;
        self.received_at(
            &request.channel.definition.owner.subject,
            request.channel.definition.record_id.as_str(),
            &request.activity,
            received,
        )
        .await
    }

    pub(crate) async fn received_at(
        &self,
        owner: &str,
        record: &str,
        activity: &cmaeg_core::activity::Activity,
        received: time::OffsetDateTime,
    ) -> Result<(), RecordError> {
        let received_at = received
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(invalid)?;
        let mut actor = activity.actor.clone();
        actor.id = bounded(&actor.id, 512);
        actor.display_name = actor.display_name.map(|v| bounded(&v, 256));
        let mut conversation = activity.conversation.clone();
        conversation.id = bounded(&conversation.id, 512);
        use cmaeg_core::activity::ActivityKind;
        let (kind, event_type, source_occurred_at) = match &activity.kind {
            ActivityKind::Message { .. } => ("message", None, None),
            ActivityKind::Interaction(_) => ("interaction", None, None),
            ActivityKind::Command { .. } => ("command", None, None),
            ActivityKind::Event { ce, .. } => (
                "event",
                Some(bounded(&ce.r#type, 256)),
                ce.time.as_ref().map(|at| bounded(at, 128)),
            ),
        };
        let row = ChannelActivity {
            quoted_context: super::model::QuotedContextSummary::from_activity(activity),
            kind: Some(kind.into()),
            event_type,
            source_occurred_at,
            ..ChannelActivity::metadata(
                activity.event_id.to_string(),
                activity.source.clone(),
                actor,
                conversation,
                activity.occurred_at.clone(),
                received_at,
            )
        };
        self.persist_receipt(owner, record, row, received).await
    }

    pub(crate) async fn observe(&self, record: &str, event: &str, change: &Change) {
        if !matches!(
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                self.change(record, event, change)
            )
            .await,
            Ok(Ok(()))
        ) {
            // Observation failure cannot alter a transport outcome or retry it.
            tracing::warn!("channel activity progress persistence unavailable");
        }
    }

    pub(crate) async fn change(
        &self,
        record: &str,
        event: &str,
        change: &Change,
    ) -> Result<(), RecordError> {
        if change.execution_attribution.is_some() && !self.execution_attribution_enabled {
            return Ok(());
        }
        let context = self
            .contexts
            .lock()
            .unwrap()
            .get(&(record.into(), event.into()))
            .cloned();
        if let Some((owner, day)) = context.filter(|(_, day)| *day > today() - RETENTION_DAYS) {
            self.history
                .change(&owner, record, event, change)
                .await
                .map_err(history_error)?;
            return self
                .modify(&owner, record, day, |bucket| {
                    if let Some(row) = bucket.items.iter_mut().find(|row| row.event_id == event) {
                        change.apply(row);
                    }
                })
                .await;
        }
        // Ownership is recovered from the sealed channel record, never supplied
        // by an observer or cached across channel deletion.
        let url = self.records.record_url(record)?;
        let value = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.records.get(url.as_str()),
        )
        .await
        .map_err(|_| unavailable())??;
        let opened = open_blob(&self.keys, &value.blob).map_err(invalid)?;
        let channel: cmaeg_core::channel::ChannelDefinition =
            serde_json::from_slice(&opened.payload).map_err(invalid)?;
        if channel.record_id.as_str() != record {
            return Err(malformed());
        }
        self.history
            .change(&channel.owner.subject, record, event, change)
            .await
            .map_err(history_error)?;
        for offset in 0..RETENTION_DAYS {
            let day = today() - offset;
            let Some((bucket, _)) = self.read(&channel.owner.subject, record, day).await? else {
                continue;
            };
            if bucket.items.iter().any(|row| row.event_id == event) {
                return self
                    .modify(&channel.owner.subject, record, day, |bucket| {
                        if let Some(row) = bucket.items.iter_mut().find(|row| row.event_id == event)
                        {
                            change.apply(row);
                        }
                    })
                    .await;
            }
        }
        // Bounded recent activity is intentionally not a complete audit log.
        Ok(())
    }

    async fn read(
        &self,
        owner: &str,
        record: &str,
        day: i64,
    ) -> Result<Option<(Bucket, u64)>, RecordError> {
        let id = identifier(owner, record, day);
        let records = self.records.clone().interactions();
        let url = records.record_url(&id)?;
        let value = match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            records.get(url.as_str()),
        )
        .await
        .map_err(|_| unavailable())?
        {
            Ok(value) => value,
            Err(RecordError::NotFound) => return Ok(None),
            Err(error) => return Err(error),
        };
        if value.version == 0 || value.version > i64::MAX as u64 {
            return Err(malformed());
        }
        let opened = open_interaction(&self.keys, &id, &value.blob).map_err(invalid)?;
        let bucket: Bucket = serde_json::from_slice(&opened.payload).map_err(invalid)?;
        if bucket.kind != KIND
            || bucket.owner != owner
            || bucket.record != record
            || bucket.day != day
            || bucket.items.len() > LIMIT
        {
            return Err(malformed());
        }
        Ok(Some((bucket, value.version)))
    }

    async fn modify(
        &self,
        owner: &str,
        record: &str,
        day: i64,
        edit: impl Fn(&mut Bucket),
    ) -> Result<(), RecordError> {
        let _guard = self.writes[stripe(record)].lock().await;
        self.modify_inner(owner, record, day, edit).await
    }
    async fn modify_inner(
        &self,
        owner: &str,
        record: &str,
        day: i64,
        edit: impl Fn(&mut Bucket),
    ) -> Result<(), RecordError> {
        let id = identifier(owner, record, day);
        for _ in 0..4 {
            let loaded = self.read(owner, record, day).await?;
            let version = loaded.as_ref().map(|(_, v)| *v);
            let mut bucket = loaded.map(|(b, _)| b).unwrap_or_else(|| Bucket {
                kind: KIND.into(),
                owner: owner.into(),
                record: record.into(),
                day,
                has_more: false,
                items: Vec::new(),
            });
            let before = serde_json::to_vec(&bucket).map_err(invalid)?;
            edit(&mut bucket);
            bucket
                .items
                .sort_by(|a, b| a.received_at.cmp(&b.received_at));
            while bucket.items.len() > LIMIT {
                bucket.items.remove(0);
                bucket.has_more = true;
            }
            let mut bytes = serde_json::to_vec(&bucket).map_err(invalid)?;
            while bytes.len() > MAX_PLAINTEXT && bucket.items.len() > 1 {
                bucket.items.remove(0);
                bucket.has_more = true;
                bytes = serde_json::to_vec(&bucket).map_err(invalid)?;
            }
            if bytes == before {
                return Ok(());
            }
            if bytes.len() > MAX_PLAINTEXT {
                return Err(malformed());
            }
            let blob = seal_interaction(&self.keys, &id, &bytes).map_err(invalid)?;
            match self
                .records
                .clone()
                .interactions()
                .put_expiring(
                    &RecordId::new(&id).map_err(invalid)?,
                    owner,
                    &blob,
                    version,
                    Some((day + RETENTION_DAYS) * 86400),
                )
                .await
            {
                Ok(_) => {
                    let _ = self.updates.send(record.into());
                    return Ok(());
                }
                Err(RecordError::Conflict { .. }) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(RecordError::Unavailable(
            "channel activity contention".into(),
        ))
    }
}

fn history_error(error: super::history::Error) -> RecordError {
    RecordError::Unavailable(error.to_string())
}

fn identifier(owner: &str, record: &str, day: i64) -> String {
    let digest =
        Sha256::digest(serde_json::to_vec(&(KIND, owner, record, day)).expect("activity identity"));
    let mut bytes: [u8; 16] = digest[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    uuid::Uuid::from_bytes(bytes).to_string()
}
fn record_hash(record: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(record.as_bytes()))
}
fn today() -> i64 {
    time::OffsetDateTime::now_utc()
        .unix_timestamp()
        .div_euclid(86400)
}
fn stripe(record: &str) -> usize {
    Sha256::digest(record.as_bytes())[0] as usize % 64
}
fn malformed() -> RecordError {
    RecordError::Malformed("invalid channel activity snapshot".into())
}
fn unavailable() -> RecordError {
    RecordError::Unavailable("channel activity read timed out".into())
}
fn invalid<T>(_: T) -> RecordError {
    malformed()
}
fn bounded(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
#[path = "activity_tests.rs"]
mod tests;
