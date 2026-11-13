//! One wake source for runtime metrics and opt-in readiness evidence.
use super::{micros, record::Record, Recorder};
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

const INTERVAL: Duration = Duration::from_secs(1);

/// Run in the telemetry server's owned sampler task. Both consumers receive
/// the same observed wake; dispatch time cannot become a second lag sample.
pub(crate) async fn run(
    recorder: Option<Arc<Recorder>>,
    mut observe: impl FnMut(Instant, Duration),
) {
    let mut scheduled = Instant::now() + INTERVAL;
    loop {
        tokio::time::sleep_until(scheduled).await;
        let now = Instant::now();
        let lag = now.saturating_duration_since(scheduled);
        observe(now, lag);
        if let Some(recorder) = &recorder {
            metrics::gauge!("cma_readiness_diagnostic_timer_lag_seconds").set(lag.as_secs_f64());
            metrics::histogram!("cma_readiness_diagnostic_timer_lag_duration_seconds")
                .record(lag.as_secs_f64());
            recorder.record(Record::Timer {
                stamp: recorder.stamp(),
                lag_us: micros(lag),
            });
        }
        // Preserve cadence without replaying missed samples after starvation.
        scheduled = now + (INTERVAL - Duration::from_nanos(lag.subsec_nanos().into()));
    }
}
