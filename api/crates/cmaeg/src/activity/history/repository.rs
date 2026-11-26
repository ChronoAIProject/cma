//! Storage boundary: no caller outside the adapter depends on Mongo collections.
use super::Error;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Position {
    pub received_ms: i64,
    pub event_key: String,
}
#[derive(Clone, Debug)]
pub struct StoredEvent {
    pub scope: String,
    pub key: Position,
    pub sequence: i64,
    pub version: i64,
    pub expires_ms: i64,
    pub blob: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct StoredChunk {
    pub id: String,
    pub scope: String,
    pub event_key: String,
    pub expires_ms: i64,
    pub blob: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureBoundary {
    pub revision: i64,
    pub retention_days: u16,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
}
impl Default for CaptureBoundary {
    fn default() -> Self {
        Self {
            revision: 1,
            retention_days: 7,
            since_ms: None,
            until_ms: None,
        }
    }
}
#[derive(Clone)]
pub struct Scan {
    pub scope: String,
    pub from_ms: i64,
    pub until_ms: i64,
    pub watermark: i64,
    pub after: Option<Position>,
    pub limit: usize,
    pub now_ms: i64,
}
#[derive(Clone)]
pub struct Snapshot {
    pub watermark: i64,
    pub earliest_expiry_ms: Option<i64>,
    pub boundary: CaptureBoundary,
}
/// First insert and per-scope sequence allocation commit together. CAS changes
/// only the sealed summary/version, never ordering, membership or fixed expiry.
pub trait ActivityHistoryRepository: Send + Sync {
    fn insert(&self, event: StoredEvent) -> BoxFuture<'_, Result<StoredEvent, Error>>;
    fn get<'a>(
        &'a self,
        scope: &'a str,
        key: &'a str,
        now_ms: i64,
    ) -> BoxFuture<'a, Result<Option<StoredEvent>, Error>>;
    fn compare_exchange(
        &self,
        event: StoredEvent,
        expected: i64,
    ) -> BoxFuture<'_, Result<bool, Error>>;
    fn put_chunk(&self, chunk: StoredChunk) -> BoxFuture<'_, Result<(), Error>>;
    fn chunk<'a>(
        &'a self,
        scope: &'a str,
        id: &'a str,
        now_ms: i64,
    ) -> BoxFuture<'a, Result<Option<StoredChunk>, Error>>;
    fn snapshot(&self, scan: Scan) -> BoxFuture<'_, Result<Snapshot, Error>>;
    fn scan(&self, scan: Scan) -> BoxFuture<'_, Result<Vec<StoredEvent>, Error>>;
    fn boundary(&self) -> BoxFuture<'_, Result<CaptureBoundary, Error>>;
}
