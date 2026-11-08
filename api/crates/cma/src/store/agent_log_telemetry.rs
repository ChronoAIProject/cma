//! Complete-scan accounting from the raw BSON lengths already used by integrity checks.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, LazyLock,
};

#[derive(Default)]
struct Totals {
    scans: AtomicU64,
    documents: AtomicU64,
    bytes: AtomicU64,
}
static TOTALS: LazyLock<Arc<[Totals; 2]>> = LazyLock::new(Arc::default);

pub(super) struct Scan {
    totals: Arc<[Totals; 2]>,
    documents: u64,
    bytes: u64,
    success: bool,
}
impl Scan {
    pub(super) fn new() -> Self {
        #[cfg(test)]
        if let Ok(totals) = TEST_TOTALS.try_with(Arc::clone) {
            return Self::with_totals(totals);
        }
        Self::with_totals(Arc::clone(&TOTALS))
    }
    fn with_totals(totals: Arc<[Totals; 2]>) -> Self {
        Self {
            totals,
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
impl Drop for Scan {
    fn drop(&mut self) {
        let totals = &self.totals[usize::from(self.success)];
        add(&totals.scans, 1);
        add(&totals.documents, self.documents);
        add(&totals.bytes, self.bytes);
    }
}
fn add(counter: &AtomicU64, delta: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
        Some(v.saturating_add(delta))
    });
}
pub(crate) fn emit() {
    for (totals, outcome) in TOTALS.iter().zip(["failed", "success"]) {
        for (metric, counter) in [
            ("cma_agent_log_scans_total", &totals.scans),
            ("cma_agent_log_scan_documents_total", &totals.documents),
            ("cma_agent_log_scan_bytes_total", &totals.bytes),
        ] {
            tracing::info!(parent: None, metric, outcome, value = counter.load(Ordering::Relaxed),
                "complete Agent log scan counter");
        }
    }
}

#[cfg(test)]
tokio::task_local! { static TEST_TOTALS: Arc<[Totals; 2]>; }
#[cfg(test)]
#[path = "agent_log_telemetry_tests.rs"]
mod tests;
