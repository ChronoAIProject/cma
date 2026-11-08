//! Consumed BSON accounting at selected store read boundaries, not wire traffic.
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, LazyLock,
};

// Reuse the static schema inventory. Never retain an unknown collection name.
const COLLECTIONS: &[&str] = super::isolation::COLLECTIONS;
#[derive(Default)]
struct Totals {
    operations: AtomicU64,
    failures: AtomicU64,
    documents: AtomicU64,
    bytes: AtomicU64,
    changed: AtomicBool,
}
struct Counters([Totals; COLLECTIONS.len() + 1]);
impl Default for Counters {
    fn default() -> Self {
        Self(std::array::from_fn(|_| Totals::default()))
    }
}
static COUNTERS: LazyLock<Arc<Counters>> = LazyLock::new(Arc::default);
fn class(collection: &str) -> usize {
    COLLECTIONS
        .iter()
        .position(|name| *name == collection)
        .unwrap_or(COLLECTIONS.len())
}
fn counters() -> Arc<Counters> {
    #[cfg(test)]
    if let Ok(counters) = TEST_COUNTERS.try_with(Arc::clone) {
        return counters;
    }
    Arc::clone(&COUNTERS)
}

/// One measured find traversal, including store validation. Drop preserves
/// partial consumption on query/decode/domain failure or cancellation.
/// getMore documents belong to this find; this is not a command/batch counter.
pub(super) struct Read {
    counters: Arc<Counters>,
    class: usize,
    documents: u64,
    bytes: u64,
    success: bool,
}
impl Read {
    pub(super) fn find(collection: &str) -> Self {
        Self {
            counters: counters(),
            class: class(collection),
            documents: 0,
            bytes: 0,
            success: false,
        }
    }
    pub(super) fn document(&mut self, encoded_bytes: usize) {
        self.documents = self.documents.saturating_add(1);
        self.bytes = self.bytes.saturating_add(encoded_bytes as u64);
    }
    pub(super) fn complete(&mut self) {
        self.success = true;
    }
}
impl Drop for Read {
    fn drop(&mut self) {
        let totals = &self.counters.0[self.class];
        add(&totals.operations, 1);
        add(&totals.failures, u64::from(!self.success));
        add(&totals.documents, self.documents);
        add(&totals.bytes, self.bytes);
        totals.changed.store(true, Ordering::Release);
    }
}
fn add(counter: &AtomicU64, delta: u64) {
    if delta != 0 {
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            Some(v.saturating_add(delta))
        });
    }
}
pub(crate) fn emit() {
    COUNTERS.emit();
}
impl Counters {
    fn emit(&self) {
        for (class, totals) in self.0.iter().enumerate() {
            if !totals.changed.swap(false, Ordering::Acquire) {
                continue;
            }
            let collection = COLLECTIONS.get(class).copied().unwrap_or("other");
            for (metric, counter) in [
                ("cma_mongo_read_operations_total", &totals.operations),
                ("cma_mongo_read_failures_total", &totals.failures),
                ("cma_mongo_read_documents_total", &totals.documents),
                ("cma_mongo_read_document_bytes_total", &totals.bytes),
            ] {
                tracing::info!(parent: None, metric, collection, operation = "find",
                    value = counter.load(Ordering::Relaxed), "Mongo consumed read counter");
            }
        }
    }
}

#[cfg(test)]
tokio::task_local! { static TEST_COUNTERS: Arc<Counters>; }
#[cfg(test)]
pub(in crate::store) async fn isolated_test<F: std::future::Future>(future: F) -> F::Output {
    TEST_COUNTERS.scope(Arc::default(), future).await
}
#[cfg(test)]
pub(in crate::store) fn snapshot(collection: &str) -> [u64; 4] {
    let counters = counters();
    let totals = &counters.0[class(collection)];
    [
        &totals.operations,
        &totals.failures,
        &totals.documents,
        &totals.bytes,
    ]
    .map(|value| value.load(Ordering::Relaxed))
}
#[cfg(test)]
#[path = "mongo_read_telemetry_tests.rs"]
mod tests;
