//! Circuit pressure measures individual pool checkouts and connection holds.
//! Multi-query projection wall time is diagnostic only. Never inspect commands.
use super::allocation_deadline::REPORTING_HOLD_DEADLINE;
use super::*;
use mongodb::{
    event::{cmap::CmapEvent, EventHandler},
    options::ClientOptions,
};
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
    slow_checkouts: Mutex<(u8, Option<Instant>)>,
}
impl Pressure {
    fn checkout(&self, key: u64, duration: Duration, failed: bool, at: Instant) {
        #[cfg(test)]
        tracing::info!(
            reason = "checkout_sample",
            probe = true,
            client_key = Arc::as_ptr(&self.circuit) as usize as u64,
            connection_key = key,
            elapsed_ms = duration.as_millis() as u64,
            failed
        );
        if !failed {
            let mut pending = self.checked_out.lock().unwrap_or_else(|e| e.into_inner());
            // Bound diagnostic state across configured pool sizes, topology
            // changes and missing driver events.
            if pending.len() < 32 {
                pending.insert(key, at);
            }
        }
        // A successful checkout just above the soft threshold is not an
        // outage. Require a short consecutive streak before rejecting work
        // and cached pages for 30 seconds; failures/hard stalls remain immediate.
        let sustained = {
            let mut streak = self
                .slow_checkouts
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if duration > Duration::from_millis(250) {
                if streak
                    .1
                    .is_none_or(|last| at.saturating_duration_since(last) > Duration::from_secs(1))
                {
                    streak.0 = 0;
                }
                streak.0 = streak.0.saturating_add(1);
                streak.1 = Some(at);
            } else {
                *streak = (0, None);
            }
            streak.0 >= 3
        };
        if failed || duration > DB_DEADLINE || sustained {
            let reason = if failed {
                "checkout_failed"
            } else if duration > DB_DEADLINE {
                "checkout_deadline"
            } else {
                "sustained_slow_checkout"
            };
            self.open("checkout", reason, duration, failed);
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
            #[cfg(test)]
            tracing::info!(
                reason = "connection_hold_sample",
                probe = true,
                client_key = Arc::as_ptr(&self.circuit) as usize as u64,
                connection_key = key,
                elapsed_ms = duration.as_millis() as u64
            );
            if duration > REPORTING_HOLD_DEADLINE {
                self.open(
                    "connection_hold",
                    "connection_hold_deadline",
                    duration,
                    false,
                );
            }
        }
    }
    fn open(
        &self,
        operation: &'static str,
        reason: &'static str,
        duration: Duration,
        failed: bool,
    ) {
        // Serialize publication so concurrent callbacks cannot regress the gauge.
        let _publication = self
            .slow_checkouts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let deadline = now() + 30;
        let until = self
            .circuit
            .fetch_max(deadline, Ordering::Relaxed)
            .max(deadline);
        metrics::gauge!("cma_mongo_circuit_until_timestamp_seconds", "pool" => "reporting")
            .set(until as f64);
        metrics::counter!("cma_mongo_circuit_opened_total", "pool" => "reporting", "reason" => reason).increment(1);
        super::super::mongo_diagnostics::output::send(
            super::super::mongo_diagnostics::output::Record::Circuit {
                operation,
                reason,
                elapsed_ms: duration.as_millis() as u64,
                failed,
            },
        );
    }
}
pub(super) fn observe(options: &mut ClientOptions, circuit: Arc<AtomicU64>) {
    metrics::gauge!("cma_mongo_circuit_until_timestamp_seconds", "pool" => "reporting").set(0);
    let pool_observer = super::super::mongo_latency::observer(options, "reporting");
    let pressure = Pressure {
        circuit,
        checked_out: Mutex::new(HashMap::new()),
        slow_checkouts: Mutex::new((0, None)),
    };
    use super::super::mongo_latency::connection_key;
    let diagnostics = super::super::mongo_diagnostics::install(options, "reporting");
    options.cmap_event_handler = Some(EventHandler::callback(move |event: CmapEvent| {
        if let Some(diagnostics) = &diagnostics {
            diagnostics.cmap(&event);
        }
        pool_observer(event.clone());
        match event {
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
        }
    }));
}
pub(super) fn check(circuit: &AtomicU64, at: u64) -> Result<()> {
    if circuit.load(Ordering::Relaxed) > at {
        metrics::counter!("cma_mongo_circuit_rejections_total", "pool" => "reporting").increment(1);
        Err(MetricsError::Busy)
    } else {
        Ok(())
    }
}

pub(crate) struct ReadTimer {
    start: Instant,
    operation: &'static str,
}
impl Drop for ReadTimer {
    fn drop(&mut self) {
        #[cfg(test)]
        tracing::info!(
            reason = "read_wall_sample",
            probe = true,
            operation = self.operation,
            elapsed_ms = self.start.elapsed().as_millis() as u64
        );
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
    #[test]
    fn reporting_callback_preserves_pool_metrics_and_circuit_with_expiry() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let circuit = Arc::new(AtomicU64::new(0));
            let mut options = ClientOptions::default();
            options.max_pool_size = Some(4);
            observe(&mut options, circuit.clone());
            let Some(EventHandler::Callback(callback)) = options.cmap_event_handler else {
                panic!("synchronous callback required");
            };
            let event = serde_json::from_value(serde_json::json!({ "reason": "timeout" })).unwrap();
            callback(CmapEvent::ConnectionCheckoutFailed(event));
            let until = circuit.load(Ordering::Relaxed);
            assert!(until > now());
            assert!(check(&circuit, until - 1).is_err());
            assert!(check(&circuit, until).is_ok());
            let text = handle.render();
            assert!(text.contains("cma_mongo_pool_events_total{pool=\"reporting\",operation=\"checkout\",outcome=\"failed\"} 1"));
            assert!(text.contains("cma_mongo_circuit_rejections_total{pool=\"reporting\"} 1"));
            assert!(text.contains(&format!(
                "cma_mongo_circuit_until_timestamp_seconds{{pool=\"reporting\"}} {until}"
            )));
        });
    }

    fn pressure() -> Pressure {
        super::super::super::mongo_diagnostics::output::initialize();
        Pressure {
            circuit: Arc::default(),
            checked_out: Mutex::new(HashMap::new()),
            slow_checkouts: Mutex::new((0, None)),
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
    fn allocation_interval_hold_slack_keeps_checkout_protection_and_cooldown() {
        let pressure = pressure();
        let start = Instant::now();
        for duration in [
            Duration::from_millis(2001),
            ALLOCATION_DB_DEADLINE,
            REPORTING_HOLD_DEADLINE,
        ] {
            pressure.checkout(1, Duration::from_millis(10), false, start);
            pressure.returned(1, start + duration);
            assert_eq!(pressure.circuit.load(Ordering::Relaxed), 0);
        }
        pressure.checkout(1, Duration::ZERO, false, start);
        pressure.returned(
            1,
            start + REPORTING_HOLD_DEADLINE + Duration::from_millis(1),
        );
        let until = pressure.circuit.swap(0, Ordering::Relaxed);
        assert!(until > now());
        assert!(matches!(
            check(&AtomicU64::new(until), until - 1),
            Err(MetricsError::Busy)
        ));
        assert!(check(&AtomicU64::new(until), until).is_ok());
        pressure.checkout(2, DB_DEADLINE + Duration::from_millis(1), false, start);
        assert!(pressure.circuit.swap(0, Ordering::Relaxed) > now());
        pressure.checkout(0, Duration::ZERO, true, start);
        assert!(pressure.circuit.load(Ordering::Relaxed) > now());
    }
    #[test]
    fn soft_checkout_spikes_recover_but_sustained_pressure_still_opens_circuit() {
        let pressure = pressure();
        let start = Instant::now();
        for n in 0..4 {
            let at = start + Duration::from_millis(n * 500);
            pressure.checkout(1, Duration::from_millis(252), false, at);
            pressure.checkout(1, Duration::from_millis(10), false, at);
        }
        assert_eq!(pressure.circuit.load(Ordering::Relaxed), 0);
        for n in 0..2 {
            pressure.checkout(
                1,
                Duration::from_millis(252),
                false,
                start + Duration::from_secs(10 + n),
            );
        }
        assert_eq!(pressure.circuit.load(Ordering::Relaxed), 0);
        // An expired streak cannot accumulate across unrelated traffic bursts.
        pressure.checkout(
            1,
            Duration::from_millis(252),
            false,
            start + Duration::from_secs(20),
        );
        assert_eq!(pressure.circuit.load(Ordering::Relaxed), 0);
        pressure.checkout(
            1,
            Duration::from_millis(252),
            false,
            start + Duration::from_secs(20),
        );
        assert_eq!(pressure.circuit.load(Ordering::Relaxed), 0);
        pressure.checkout(
            1,
            Duration::from_millis(252),
            false,
            start + Duration::from_secs(20),
        );
        assert!(pressure.circuit.load(Ordering::Relaxed) > now());
    }
    #[test]
    fn slow_individual_read_or_checkout_and_failed_checkout_still_open_circuit() {
        let pressure = pressure();
        let start = Instant::now();
        pressure.checkout(1, Duration::ZERO, false, start);
        pressure.returned(
            1,
            start + REPORTING_HOLD_DEADLINE + Duration::from_millis(1),
        );
        assert!(pressure.circuit.swap(0, Ordering::Relaxed) > now());
        pressure.checkout(2, DB_DEADLINE + Duration::from_millis(1), false, start);
        assert!(pressure.circuit.swap(0, Ordering::Relaxed) > now());
        pressure.checkout(0, Duration::ZERO, true, start);
        assert!(pressure.circuit.load(Ordering::Relaxed) > now());
    }
    #[test]
    fn genuine_pressure_is_diagnosed_rejected_and_recovers_at_expiry() {
        use super::super::tests::circuit_capture::Capture;
        use tracing_subscriber::prelude::*;
        for (reason, duration, failed) in [
            ("checkout_failed", Duration::from_millis(7), true),
            ("checkout_deadline", Duration::from_millis(2001), false),
            (
                "connection_hold_deadline",
                REPORTING_HOLD_DEADLINE + Duration::from_millis(1),
                false,
            ),
            ("sustained_slow_checkout", Duration::from_millis(251), false),
        ] {
            let pressure = pressure();
            let capture = Capture::default();
            let subscriber = tracing_subscriber::registry().with(capture.clone());
            tracing::subscriber::with_default(subscriber, || {
                let at = Instant::now();
                if reason == "connection_hold_deadline" {
                    pressure.checkout(1, Duration::ZERO, false, at);
                    pressure.returned(1, at + duration);
                } else {
                    for _ in 0..if reason == "sustained_slow_checkout" {
                        3
                    } else {
                        1
                    } {
                        pressure.checkout(1, duration, failed, at);
                    }
                }
            });
            let until = pressure.circuit.load(Ordering::Relaxed);
            assert!(until > now());
            assert!(matches!(
                check(&pressure.circuit, until - 1),
                Err(MetricsError::Busy)
            ));
            assert!(check(&pressure.circuit, until).is_ok());
            assert!(check(&pressure.circuit, until + 1).is_ok());
            let (count, event) = capture.snapshot();
            assert_eq!(count, 1);
            let event = event.unwrap();
            assert_eq!(event.reason, reason);
            assert_eq!(event.elapsed_ms, duration.as_millis() as u64);
            assert_eq!(event.failed, failed);
        }
    }
}
