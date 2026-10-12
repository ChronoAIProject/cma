//! Shared ingress measurements; only transport IO may set the launch timestamp.
use super::names;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct IngressTiming(Arc<Inner>);
#[derive(Debug)]
struct Inner {
    source: &'static str,
    request_span: tracing::Span,
    started: Instant,
    received: OnceLock<Instant>,
    written: OnceLock<Instant>,
    cold: AtomicBool,
    restore: Mutex<Duration>,
    verify: Mutex<Duration>,
}
impl IngressTiming {
    pub fn new(source: &'static str) -> Self {
        Self(Arc::new(Inner {
            source,
            request_span: tracing::Span::current(),
            started: Instant::now(),
            received: OnceLock::new(),
            written: OnceLock::new(),
            cold: AtomicBool::new(false),
            restore: Mutex::new(Duration::ZERO),
            verify: Mutex::new(Duration::ZERO),
        }))
    }
    /// The bounded ingress source this request was created for.
    pub fn source(&self) -> &'static str {
        self.0.source
    }
    pub fn received(&self) -> Instant {
        *self.0.received.get_or_init(Instant::now)
    }
    pub fn triggered_restore(&self) {
        self.0.cold.store(true, Ordering::Relaxed);
    }
    pub fn is_cold(&self) -> bool {
        self.0.cold.load(Ordering::Relaxed)
    }
    /// Called after a successful socket write containing this request's first byte.
    pub fn written(&self, at: Instant) {
        if self.0.written.set(at).is_ok() {
            if let Some(received) = self.0.received.get() {
                let seconds = at.saturating_duration_since(*received).as_secs_f64();
                metrics::histogram!(names::INGRESS_TO_PROVIDER,
                    names::SOURCE => self.0.source, names::COLD => if self.is_cold() { "true" } else { "false" })
                    .record(seconds);
                tracing::info!(
                    parent: &self.0.request_span,
                    source = self.0.source,
                    cold = self.is_cold(),
                    ingress_to_provider_ms = seconds * 1000.0,
                    "ingress socket write"
                );
            }
        }
    }
    pub fn restore(&self) -> Phase<'_> {
        Phase::new(&self.0.restore)
    }
    pub fn verify(&self) -> Phase<'_> {
        Phase::new(&self.0.verify)
    }
    pub fn header(&self) -> String {
        let launch = self
            .0
            .written
            .get()
            .zip(self.0.received.get())
            .map(|(written, received)| written.saturating_duration_since(*received));
        format!(
            "restore;dur={:.3}, verify;dur={:.3}, launch;dur={:.3}{}, total;dur={:.3}",
            millis(*self.0.restore.lock().unwrap()),
            millis(*self.0.verify.lock().unwrap()),
            millis(launch.unwrap_or_default()),
            if launch.is_none() {
                ";desc=\"not_written\""
            } else {
                ""
            },
            millis(self.0.started.elapsed())
        )
    }
}
pub struct Phase<'a> {
    value: &'a Mutex<Duration>,
    started: Instant,
}
impl<'a> Phase<'a> {
    fn new(value: &'a Mutex<Duration>) -> Self {
        Self {
            value,
            started: Instant::now(),
        }
    }
}
impl Drop for Phase<'_> {
    fn drop(&mut self) {
        *self.value.lock().unwrap() += self.started.elapsed();
    }
}
fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
