//! Fixed-cardinality observations; record content never enters logs or labels.
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) enum Reason {
    QueueFull,
    ByteBudget,
    Shutdown,
}
impl Reason {
    fn index(&self) -> usize {
        match self {
            Self::QueueFull => 0,
            Self::ByteBudget => 1,
            Self::Shutdown => 2,
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Self::QueueFull => "queue_full",
            Self::ByteBudget => "byte_budget",
            Self::Shutdown => "shutdown",
        }
    }
}

pub(super) struct QueueMetrics {
    pub records: metrics::Gauge,
    pub bytes: metrics::Gauge,
    pub oldest: metrics::Gauge,
    pub batch_size: metrics::Histogram,
    pub write_seconds: metrics::Histogram,
    drops: [metrics::Counter; 3],
    totals: [AtomicU64; 3],
}

impl QueueMetrics {
    pub fn new() -> Self {
        metrics::describe_gauge!(
            "cma_activity_queue_records",
            "Outstanding request activity, including the batch in flight"
        );
        metrics::describe_gauge!(
            "cma_activity_queue_bytes",
            "Estimated serialized bytes of outstanding request activity"
        );
        metrics::describe_gauge!(
            "cma_activity_queue_oldest_seconds",
            "Age of oldest outstanding activity, refreshed every 250 ms"
        );
        metrics::describe_histogram!(
            "cma_activity_batch_records",
            "Records per batch write attempt"
        );
        metrics::describe_histogram!(
            "cma_activity_batch_write_seconds",
            "Duration of each completed batch write attempt, including failures"
        );
        metrics::describe_counter!(
            "cma_activity_dropped_total",
            "Dropped request telemetry by bounded reason"
        );
        let metrics = Self {
            records: metrics::gauge!("cma_activity_queue_records"),
            bytes: metrics::gauge!("cma_activity_queue_bytes"),
            oldest: metrics::gauge!("cma_activity_queue_oldest_seconds"),
            batch_size: metrics::histogram!("cma_activity_batch_records"),
            write_seconds: metrics::histogram!("cma_activity_batch_write_seconds"),
            drops: [Reason::QueueFull, Reason::ByteBudget, Reason::Shutdown].map(|reason| {
                metrics::counter!("cma_activity_dropped_total", "reason" => reason.label())
            }),
            totals: std::array::from_fn(|_| AtomicU64::new(0)),
        };
        metrics.records.set(0);
        metrics.bytes.set(0);
        metrics.oldest.set(0);
        for counter in &metrics.drops {
            counter.absolute(0);
        }
        metrics
    }

    pub fn drop_records(&self, reason: Reason, count: u64) {
        let index = reason.index();
        self.drops[index].increment(count);
        let total = self.totals[index]
            .fetch_add(count, Ordering::Relaxed)
            .wrapping_add(count);
        if total.is_power_of_two() {
            tracing::warn!(
                reason = reason.label(),
                dropped = total,
                "request activity dropped"
            );
        }
    }
}
