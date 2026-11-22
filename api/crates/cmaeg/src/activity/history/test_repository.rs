//! Deterministic failure injection for service/cursor tests; Mongo tests exercise
//! the adapter's actual transactions, indexes, grants and replica behavior.
use super::{repository::*, Error};
use futures_util::future::BoxFuture;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
#[derive(Default)]
pub(crate) struct Memory {
    pub events: Mutex<BTreeMap<String, StoredEvent>>,
    pub chunks: Mutex<BTreeMap<String, StoredChunk>>,
    pub capture: Mutex<CaptureBoundary>,
    pub fail_chunks: AtomicBool,
    pub fail_insert: AtomicBool,
}
impl ActivityHistoryRepository for Memory {
    fn insert(&self, mut event: StoredEvent) -> BoxFuture<'_, Result<StoredEvent, Error>> {
        Box::pin(async move {
            if self.fail_insert.load(Ordering::SeqCst) {
                return Err(Error::Unavailable);
            }
            let mut rows = self.events.lock().unwrap();
            if let Some(existing) = rows.get(&event.key.event_key) {
                return Ok(existing.clone());
            }
            event.sequence = rows
                .values()
                .filter(|r| r.scope == event.scope)
                .map(|r| r.sequence)
                .max()
                .unwrap_or(0)
                + 1;
            rows.insert(event.key.event_key.clone(), event.clone());
            Ok(event)
        })
    }
    fn get<'a>(
        &'a self,
        scope: &'a str,
        key: &'a str,
        now_ms: i64,
    ) -> BoxFuture<'a, Result<Option<StoredEvent>, Error>> {
        Box::pin(async move {
            Ok(self
                .events
                .lock()
                .unwrap()
                .get(key)
                .filter(|r| r.scope == scope && r.expires_ms > now_ms)
                .cloned())
        })
    }
    fn compare_exchange(
        &self,
        event: StoredEvent,
        expected: i64,
    ) -> BoxFuture<'_, Result<bool, Error>> {
        Box::pin(async move {
            let mut rows = self.events.lock().unwrap();
            if !rows.get(&event.key.event_key).is_some_and(|r| {
                r.scope == event.scope
                    && r.version == expected
                    && r.expires_ms == event.expires_ms
                    && r.key == event.key
            }) {
                return Ok(false);
            }
            rows.insert(event.key.event_key.clone(), event);
            Ok(true)
        })
    }
    fn put_chunk(&self, chunk: StoredChunk) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            if self.fail_chunks.load(Ordering::SeqCst) {
                return Err(Error::Unavailable);
            }
            let mut rows = self.chunks.lock().unwrap();
            assert!(!rows.contains_key(&chunk.id), "immutable chunk identity");
            rows.insert(chunk.id.clone(), chunk);
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
            Ok(self
                .chunks
                .lock()
                .unwrap()
                .get(id)
                .filter(|r| r.scope == scope && r.expires_ms > now_ms)
                .cloned())
        })
    }
    fn snapshot(&self, scan: Scan) -> BoxFuture<'_, Result<Snapshot, Error>> {
        Box::pin(async move {
            let rows = self.events.lock().unwrap();
            Ok(Snapshot {
                watermark: rows
                    .values()
                    .filter(|r| r.scope == scan.scope)
                    .map(|r| r.sequence)
                    .max()
                    .unwrap_or(0),
                earliest_expiry_ms: rows
                    .values()
                    .filter(|r| {
                        r.scope == scan.scope
                            && r.key.received_ms >= scan.from_ms
                            && r.key.received_ms < scan.until_ms
                            && r.expires_ms > scan.now_ms
                    })
                    .map(|r| r.expires_ms)
                    .min(),
                boundary: self.capture.lock().unwrap().clone(),
            })
        })
    }
    fn scan(&self, scan: Scan) -> BoxFuture<'_, Result<Vec<StoredEvent>, Error>> {
        Box::pin(async move {
            let mut rows: Vec<_> = self
                .events
                .lock()
                .unwrap()
                .values()
                .filter(|r| {
                    r.scope == scan.scope
                        && r.key.received_ms >= scan.from_ms
                        && r.key.received_ms < scan.until_ms
                        && r.expires_ms > scan.now_ms
                        && r.sequence <= scan.watermark
                        && scan.after.as_ref().is_none_or(|a| {
                            r.key.received_ms < a.received_ms
                                || (r.key.received_ms == a.received_ms
                                    && r.key.event_key > a.event_key)
                        })
                })
                .cloned()
                .collect();
            rows.sort_by(|a, b| {
                b.key
                    .received_ms
                    .cmp(&a.key.received_ms)
                    .then_with(|| a.key.event_key.cmp(&b.key.event_key))
            });
            rows.truncate(scan.limit);
            Ok(rows)
        })
    }
    fn boundary(&self) -> BoxFuture<'_, Result<CaptureBoundary, Error>> {
        Box::pin(async { Ok(self.capture.lock().unwrap().clone()) })
    }
}
