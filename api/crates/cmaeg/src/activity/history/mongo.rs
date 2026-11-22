use super::{repository::*, schema::*, Error};
use futures_util::{future::BoxFuture, TryStreamExt};
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, DateTime, Document},
    options::{Acknowledgment, CollectionOptions, Hint, ReadConcern, WriteConcern},
    Collection, Database,
};
use std::time::Duration;

#[derive(Clone)]
pub struct MongoHistory {
    pub(super) db: Database,
    pub(super) events: Collection<Document>,
    pub(super) chunks: Collection<Document>,
    pub(super) scopes: Collection<Document>,
    pub(super) control: Collection<Document>,
}
pub(super) fn majority() -> WriteConcern {
    WriteConcern::builder()
        .w(Acknowledgment::Majority)
        .w_timeout(Duration::from_secs(2))
        .build()
}
impl MongoHistory {
    pub async fn open(db: Database) -> Result<Self, Error> {
        super::schema::verify(&db).await?;
        let options = CollectionOptions::builder()
            .read_concern(ReadConcern::majority())
            .write_concern(majority())
            .build();
        Ok(Self {
            events: db.collection_with_options(EVENTS, options.clone()),
            chunks: db.collection_with_options(CHUNKS, options.clone()),
            scopes: db.collection_with_options(SCOPES, options.clone()),
            control: db.collection_with_options(CONTROL, options),
            db,
        })
    }
}
pub(super) fn binary(bytes: Vec<u8>) -> Binary {
    Binary {
        subtype: BinarySubtype::Generic,
        bytes,
    }
}
pub(super) fn event_doc(event: &StoredEvent) -> Document {
    doc! {"_id":&event.key.event_key,"scope":&event.scope,"received_at":DateTime::from_millis(event.key.received_ms),
    "sequence":event.sequence,"version":event.version,"expires_at":DateTime::from_millis(event.expires_ms),"blob":binary(event.blob.clone())}
}
pub(super) fn decode(row: Document) -> Result<StoredEvent, Error> {
    let invalid = |_| Error::Unreadable;
    Ok(StoredEvent {
        scope: row.get_str("scope").map_err(invalid)?.into(),
        key: Position {
            received_ms: row
                .get_datetime("received_at")
                .map_err(invalid)?
                .timestamp_millis(),
            event_key: row.get_str("_id").map_err(invalid)?.into(),
        },
        sequence: row.get_i64("sequence").map_err(invalid)?,
        version: row.get_i64("version").map_err(invalid)?,
        expires_ms: row
            .get_datetime("expires_at")
            .map_err(invalid)?
            .timestamp_millis(),
        // A malformed payload becomes one unreadable row; its valid ordering
        // metadata still advances the scan past corruption to valid neighbors.
        blob: row.get_binary_generic("blob").cloned().unwrap_or_default(),
    })
}
fn filter(scan: &Scan) -> Document {
    let mut filter = doc! {"scope":&scan.scope,"received_at":{"$gte":DateTime::from_millis(scan.from_ms),"$lt":DateTime::from_millis(scan.until_ms)},
    "sequence":{"$lte":scan.watermark},"expires_at":{"$gt":DateTime::from_millis(scan.now_ms)}};
    if let Some(after) = &scan.after {
        filter.insert("$or", vec![doc! {"received_at":{"$lt":DateTime::from_millis(after.received_ms)}},doc! {"received_at":DateTime::from_millis(after.received_ms),"_id":{"$gt":&after.event_key}}]);
    }
    filter
}
impl ActivityHistoryRepository for MongoHistory {
    fn insert(&self, event: StoredEvent) -> BoxFuture<'_, Result<StoredEvent, Error>> {
        Box::pin(self.insert_event(event))
    }
    fn get<'a>(
        &'a self,
        scope: &'a str,
        key: &'a str,
        now_ms: i64,
    ) -> BoxFuture<'a, Result<Option<StoredEvent>, Error>> {
        Box::pin(async move {
            self.events.find_one(doc! {"_id":key,"scope":scope,"expires_at":{"$gt":DateTime::from_millis(now_ms)}})
                .max_time(Duration::from_secs(2)).await.map_err(|_| Error::Unavailable)?.map(decode).transpose()
        })
    }
    fn compare_exchange(
        &self,
        event: StoredEvent,
        expected: i64,
    ) -> BoxFuture<'_, Result<bool, Error>> {
        Box::pin(async move {
            if event.blob.len() > 65536 || expected < 1 || event.version != expected + 1 {
                return Err(Error::InvalidRequest);
            }
            let result = self.events.update_one(doc! {"_id":&event.key.event_key,"scope":&event.scope,"version":expected,
                "expires_at":DateTime::from_millis(event.expires_ms),"received_at":DateTime::from_millis(event.key.received_ms),"sequence":event.sequence},
                doc! {"$set":{"blob":binary(event.blob.clone()),"version":event.version}}).await;
            match result {
                Ok(result) => Ok(result.matched_count == 1),
                Err(_) => {
                    // Reconcile exactly this attempted summary. A different
                    // winning summary must be reread and merged by the caller.
                    match self
                        .get(&event.scope, &event.key.event_key, super::now_ms())
                        .await?
                    {
                        Some(current)
                            if current.version == event.version && current.blob == event.blob =>
                        {
                            Ok(true)
                        }
                        _ => Err(Error::Unavailable),
                    }
                }
            }
        })
    }
    fn put_chunk(&self, chunk: StoredChunk) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            if chunk.blob.len() > 65536 {
                return Err(Error::DetailLimit);
            }
            let result = self.chunks.insert_one(doc! {"_id":&chunk.id,"scope":&chunk.scope,"event_key":&chunk.event_key,
                "expires_at":DateTime::from_millis(chunk.expires_ms),"blob":binary(chunk.blob.clone())}).await;
            if result.is_err() {
                let found = self
                    .chunk(&chunk.scope, &chunk.id, super::now_ms())
                    .await?
                    .ok_or(Error::Unavailable)?;
                if found.blob != chunk.blob
                    || found.expires_ms != chunk.expires_ms
                    || found.event_key != chunk.event_key
                {
                    return Err(Error::Unavailable);
                }
            }
            Ok(())
        })
    }
    fn chunk<'a>(
        &'a self,
        scope: &'a str,
        id: &'a str,
        now_ms: i64,
    ) -> BoxFuture<'a, Result<Option<StoredChunk>, Error>> {
        Box::pin(async move {
            let Some(row) = self.chunks.find_one(doc! {"_id":id,"scope":scope,"expires_at":{"$gt":DateTime::from_millis(now_ms)}})
                .max_time(Duration::from_secs(2)).await.map_err(|_| Error::Unavailable)? else { return Ok(None) };
            Ok(Some(StoredChunk {
                id: id.into(),
                scope: scope.into(),
                event_key: row
                    .get_str("event_key")
                    .map_err(|_| Error::Unreadable)?
                    .into(),
                expires_ms: row
                    .get_datetime("expires_at")
                    .map_err(|_| Error::Unreadable)?
                    .timestamp_millis(),
                blob: row
                    .get_binary_generic("blob")
                    .map_err(|_| Error::Unreadable)?
                    .clone(),
            }))
        })
    }
    fn snapshot(&self, mut scan: Scan) -> BoxFuture<'_, Result<Snapshot, Error>> {
        Box::pin(async move {
            let watermark = self
                .scopes
                .find_one(doc! {"_id":&scan.scope})
                .max_time(Duration::from_secs(2))
                .await
                .map_err(|_| Error::Unavailable)?
                .map(|row| row.get_i64("sequence").map_err(|_| Error::Unreadable))
                .transpose()?
                .unwrap_or(0);
            scan.watermark = watermark;
            let first = self
                .events
                .find_one(filter(&scan))
                .sort(doc! {"expires_at":1})
                .projection(doc! {"expires_at":1})
                .max_time(Duration::from_secs(2))
                .await
                .map_err(|_| Error::Unavailable)?;
            let earliest_expiry_ms = first
                .map(|row| {
                    row.get_datetime("expires_at")
                        .map(|v| v.timestamp_millis())
                        .map_err(|_| Error::Unreadable)
                })
                .transpose()?;
            Ok(Snapshot {
                watermark,
                earliest_expiry_ms,
                boundary: self.boundary().await?,
            })
        })
    }
    fn scan(&self, scan: Scan) -> BoxFuture<'_, Result<Vec<StoredEvent>, Error>> {
        Box::pin(async move {
            if scan.limit == 0 || scan.limit > 201 {
                return Err(Error::InvalidRequest);
            }
            self.events
                .find(filter(&scan))
                .sort(doc! {"received_at":-1,"_id":1})
                .hint(Hint::Name(PAGE_INDEX.into()))
                .limit(scan.limit as i64)
                .batch_size(scan.limit as u32)
                .max_time(Duration::from_secs(2))
                .await
                .map_err(|_| Error::Unavailable)?
                .try_collect::<Vec<_>>()
                .await
                .map_err(|_| Error::Unavailable)?
                .into_iter()
                .map(decode)
                .collect()
        })
    }
    fn boundary(&self) -> BoxFuture<'_, Result<CaptureBoundary, Error>> {
        Box::pin(async move {
            let row = self
                .control
                .find_one(doc! {"_id":"capture","schema_version":1_i32})
                .max_time(Duration::from_secs(2))
                .await
                .map_err(|_| Error::Unavailable)?
                .ok_or(Error::Unavailable)?;
            let optional = |key| match row.get(key) {
                Some(mongodb::bson::Bson::Null) => Ok(None),
                Some(mongodb::bson::Bson::Int64(value)) => Ok(Some(*value)),
                _ => Err(Error::Unreadable),
            };
            Ok(CaptureBoundary {
                revision: row.get_i64("revision").map_err(|_| Error::Unreadable)?,
                retention_days: row
                    .get_i32("retention_days")
                    .ok()
                    .and_then(|v| u16::try_from(v).ok())
                    .filter(|v| (1..=90).contains(v))
                    .ok_or(Error::Unreadable)?,
                since_ms: optional("since_ms")?,
                until_ms: optional("until_ms")?,
            })
        })
    }
}
