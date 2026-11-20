//! Shared ingress measurements; only transport IO may set the launch timestamp.
use super::names;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

/// What a turn's attachments did to its path to the provider. A turn only
/// ever moves up this order, so the label names the largest delay source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MediaLabel {
    /// No preparation and no order fence: the latency SLO applies.
    None,
    /// Eager media items were prepared before the provider turn started.
    Prepared,
    /// The turn waited for an earlier turn of its conversation.
    Fenced,
}

impl MediaLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Prepared => "prepared",
            Self::Fenced => "fenced",
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Prepared,
            _ => Self::Fenced,
        }
    }
}

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
    media: AtomicU8,
    restore: Mutex<Duration>,
    verify: Mutex<Duration>,
    provider_start: Option<super::provider_start::Attempt>,
}
impl IngressTiming {
    pub fn new(source: &'static str) -> Self {
        Self::with_provider_start(source, None)
    }
    pub(crate) fn with_provider_start(
        source: &'static str,
        provider_start: Option<super::provider_start::Attempt>,
    ) -> Self {
        Self(Arc::new(Inner {
            provider_start,
            source,
            request_span: tracing::Span::current(),
            started: Instant::now(),
            received: OnceLock::new(),
            written: OnceLock::new(),
            cold: AtomicBool::new(false),
            media: AtomicU8::new(MediaLabel::None as u8),
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
    /// Raise the media label; a lower label never replaces a higher one.
    pub fn mark_media(&self, label: MediaLabel) {
        self.0.media.fetch_max(label as u8, Ordering::Relaxed);
    }
    pub fn media(&self) -> MediaLabel {
        MediaLabel::from_u8(self.0.media.load(Ordering::Relaxed))
    }
    /// Called after a successful socket write containing this request's first byte.
    pub fn written(&self, at: Instant) {
        if self.0.written.set(at).is_ok() {
            if let Some(attempt) = &self.0.provider_start {
                attempt.written(at);
            }
            if let Some(received) = self.0.received.get() {
                let seconds = at.saturating_duration_since(*received).as_secs_f64();
                let media = self.media().as_str();
                metrics::histogram!(names::INGRESS_TO_PROVIDER,
                    names::SOURCE => self.0.source, names::COLD => if self.is_cold() { "true" } else { "false" },
                    names::MEDIA => media)
                    .record(seconds);
                tracing::info!(
                    parent: &self.0.request_span,
                    source = self.0.source,
                    cold = self.is_cold(),
                    media,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_media_label_only_rises() {
        let timing = IngressTiming::new("custom");
        assert_eq!(timing.media(), MediaLabel::None);
        timing.mark_media(MediaLabel::Prepared);
        assert_eq!(timing.media(), MediaLabel::Prepared);
        timing.mark_media(MediaLabel::Fenced);
        timing.mark_media(MediaLabel::Prepared);
        timing.mark_media(MediaLabel::None);
        assert_eq!(timing.media(), MediaLabel::Fenced);
        assert_eq!(
            [MediaLabel::None, MediaLabel::Prepared, MediaLabel::Fenced].map(MediaLabel::as_str),
            ["none", "prepared", "fenced"]
        );
    }
}
