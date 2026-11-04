//! Circuit pressure comes from measured DB checkout/read latency, never a
//! clipped whole-worker duration. Command bodies and replies are not logged.
use super::*;
use mongodb::event::{cmap::CmapEvent, EventHandler};
use std::sync::atomic::{AtomicU64, Ordering};
pub(super) fn observe(options: &mut ClientOptions, circuit: Arc<AtomicU64>) {
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        let (duration, failed) = match event {
            CmapEvent::ConnectionCheckedOut(e) => (e.duration, false),
            CmapEvent::ConnectionCheckoutFailed(e) => (e.duration, true),
            _ => return,
        };
        if failed || duration > Duration::from_millis(250) {
            circuit.store(now() + 30, Ordering::Relaxed);
            tracing::warn!(
                elapsed_ms = duration.as_millis() as u64,
                failed,
                "sandbox reporting checkout circuit opened"
            );
        }
    }));
}
pub(crate) struct ReadTimer {
    start: std::time::Instant,
    operation: &'static str,
    circuit: Arc<AtomicU64>,
}
impl Drop for ReadTimer {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        if elapsed > DB_DEADLINE {
            self.circuit.store(now() + 30, Ordering::Relaxed);
        }
        tracing::debug!(
            operation = self.operation,
            elapsed_ms = elapsed.as_millis() as u64,
            "sandbox reporting database read"
        );
    }
}
impl ReportingStore {
    pub(crate) fn time_read(&self, operation: &'static str) -> Result<ReadTimer> {
        self.check_circuit(now())?;
        Ok(ReadTimer {
            start: std::time::Instant::now(),
            operation,
            circuit: self.circuit.clone(),
        })
    }
}
