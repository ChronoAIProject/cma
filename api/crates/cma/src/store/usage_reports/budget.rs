//! Circuit pressure measures individual pool checkouts and connection holds.
//! Multi-query projection wall time is diagnostic only. Never inspect commands.
use super::*;
use mongodb::event::{cmap::CmapEvent, EventHandler};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Instant,
};

struct Pressure {
    circuit: Arc<AtomicU64>,
    checked_out: Mutex<HashMap<u64, Instant>>,
}
impl Pressure {
    fn checkout(&self, key: u64, duration: Duration, failed: bool, at: Instant) {
        if !failed {
            let mut pending = self.checked_out.lock().unwrap_or_else(|e| e.into_inner());
            // The reporting pool has four connections; bound diagnostic state
            // even during topology changes or missing driver events.
            if pending.len() < 32 {
                pending.insert(key, at);
            }
        }
        if failed || duration > Duration::from_millis(250) {
            self.open("checkout", duration, failed);
        }
    }
    fn returned(&self, key: u64, at: Instant) {
        let start = self
            .checked_out
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key);
        if let Some(start) = start {
            let duration = at.saturating_duration_since(start);
            if duration > DB_DEADLINE {
                self.open("connection_hold", duration, false);
            }
        }
    }
    fn open(&self, operation: &'static str, duration: Duration, failed: bool) {
        self.circuit.fetch_max(now() + 30, Ordering::Relaxed);
        tracing::warn!(
            operation,
            elapsed_ms = duration.as_millis() as u64,
            failed,
            "sandbox reporting database circuit opened"
        );
    }
}
pub(super) fn observe(options: &mut ClientOptions, circuit: Arc<AtomicU64>) {
    let pressure = Pressure {
        circuit,
        checked_out: Mutex::new(HashMap::new()),
    };
    use super::super::mongo_latency::connection_key;
    options.cmap_event_handler = Some(EventHandler::callback(move |event| match event {
        CmapEvent::ConnectionCheckedOut(e) => pressure.checkout(
            connection_key(&e.address, e.connection_id),
            e.duration,
            false,
            Instant::now(),
        ),
        CmapEvent::ConnectionCheckoutFailed(e) => {
            pressure.checkout(0, e.duration, true, Instant::now())
        }
        CmapEvent::ConnectionCheckedIn(e) => {
            pressure.returned(connection_key(&e.address, e.connection_id), Instant::now())
        }
        CmapEvent::ConnectionClosed(e) => {
            pressure.returned(connection_key(&e.address, e.connection_id), Instant::now())
        }
        _ => {}
    }));
}
pub(crate) struct ReadTimer {
    start: Instant,
    operation: &'static str,
}
impl Drop for ReadTimer {
    fn drop(&mut self) {
        tracing::debug!(
            operation = self.operation,
            elapsed_ms = self.start.elapsed().as_millis() as u64,
            "sandbox reporting database read"
        );
    }
}
impl ReportingStore {
    pub(crate) fn time_read(&self, operation: &'static str) -> Result<ReadTimer> {
        self.check_circuit(now())?;
        Ok(ReadTimer {
            start: Instant::now(),
            operation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pressure() -> Pressure {
        Pressure {
            circuit: Arc::default(),
            checked_out: Mutex::new(HashMap::new()),
        }
    }
    #[test]
    fn multiple_bounded_reads_do_not_trip_the_individual_read_deadline() {
        let pressure = pressure();
        let start = Instant::now();
        for n in 0..10 {
            let at = start + Duration::from_millis(n * 600);
            pressure.checkout(1, Duration::from_millis(10), false, at);
            pressure.returned(1, at + Duration::from_millis(600));
        }
        assert_eq!(pressure.circuit.load(Ordering::Relaxed), 0);
        assert!(pressure.checked_out.lock().unwrap().is_empty());
    }
    #[test]
    fn slow_individual_read_or_checkout_and_failed_checkout_still_open_circuit() {
        let pressure = pressure();
        let start = Instant::now();
        pressure.checkout(1, Duration::ZERO, false, start);
        pressure.returned(1, start + DB_DEADLINE + Duration::from_millis(1));
        assert!(pressure.circuit.swap(0, Ordering::Relaxed) > now());
        pressure.checkout(2, Duration::from_millis(251), false, start);
        assert!(pressure.circuit.swap(0, Ordering::Relaxed) > now());
        pressure.checkout(0, Duration::ZERO, true, start);
        assert!(pressure.circuit.load(Ordering::Relaxed) > now());
    }
}
