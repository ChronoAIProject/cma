//! Fixed-label read accounting at the driver's borrowed raw response boundary.
use mongodb::{
    event::response_summary::{ResponseObserver, ResponseSummary},
    options::ClientOptions,
};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, LazyLock,
};

// Never derive labels from a request, namespace, cursor, database, or error.
const COLLECTIONS: &[&str] = super::isolation::COLLECTIONS;
const OPERATIONS: [&str; 7] = [
    "find",
    "aggregate",
    "getMore",
    "findAndModify",
    "count",
    "distinct",
    "other",
];
const CLASSES: usize = (COLLECTIONS.len() + 1) * OPERATIONS.len();

#[derive(Debug, Default)]
struct Totals {
    responses: AtomicU64,
    failures: AtomicU64,
    documents: AtomicU64,
    bytes: AtomicU64,
    document_bytes: AtomicU64,
    incomplete: AtomicU64,
    changed: AtomicBool,
}
#[derive(Debug)]
struct Observer([Totals; CLASSES]);
impl Default for Observer {
    fn default() -> Self {
        Self(std::array::from_fn(|_| Totals::default()))
    }
}
static OBSERVER: LazyLock<Arc<Observer>> = LazyLock::new(Arc::default);

pub(crate) fn observe(options: &mut ClientOptions) {
    options.response_observer = Some(OBSERVER.clone());
}
pub(crate) fn emit() {
    OBSERVER.emit();
}

impl ResponseObserver for Observer {
    fn classify(&self, operation: &str, collection: Option<&str>) -> u32 {
        let collection = COLLECTIONS
            .iter()
            .position(|name| Some(*name) == collection)
            .unwrap_or(COLLECTIONS.len());
        let operation = OPERATIONS
            .iter()
            .position(|name| *name == operation)
            .unwrap_or(OPERATIONS.len() - 1);
        (collection * OPERATIONS.len() + operation) as u32
    }
    fn observe(&self, class: u32, summary: ResponseSummary) {
        let Some(totals) = self.0.get(class as usize) else {
            return;
        };
        add(&totals.responses, 1);
        add(&totals.failures, u64::from(!summary.succeeded));
        add(&totals.documents, summary.documents);
        add(&totals.bytes, summary.reply_bytes);
        add(&totals.document_bytes, summary.document_bytes);
        add(
            &totals.incomplete,
            u64::from(!summary.complete && summary.reply_bytes > 0),
        );
        totals.changed.store(true, Ordering::Release);
    }
}
fn add(counter: &AtomicU64, count: u64) {
    if count != 0 {
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            Some(v.saturating_add(count))
        });
    }
}
impl Observer {
    fn emit(&self) {
        for (class, totals) in self.0.iter().enumerate() {
            if !totals.changed.swap(false, Ordering::Acquire) {
                continue;
            }
            let collection = COLLECTIONS
                .get(class / OPERATIONS.len())
                .copied()
                .unwrap_or("other");
            let operation = OPERATIONS[class % OPERATIONS.len()];
            for (metric, counter) in [
                ("cma_mongo_read_responses_total", &totals.responses),
                ("cma_mongo_read_failures_total", &totals.failures),
                ("cma_mongo_read_documents_total", &totals.documents),
                ("cma_mongo_read_bytes_total", &totals.bytes),
                (
                    "cma_mongo_read_document_bytes_total",
                    &totals.document_bytes,
                ),
                (
                    "cma_mongo_read_incomplete_summaries_total",
                    &totals.incomplete,
                ),
            ] {
                tracing::info!(parent: None, metric, collection, operation,
                    value = counter.load(Ordering::Relaxed), "Mongo read counter");
            }
        }
    }
}

#[cfg(test)]
#[path = "mongo_reads_tests.rs"]
mod tests;
