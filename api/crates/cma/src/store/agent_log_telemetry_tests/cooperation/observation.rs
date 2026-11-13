//! Test-only metadata counts; no command observer or document payload capture.
use super::*;
use mongodb::{
    event::{cmap::CmapEvent, EventHandler},
    options::ClientOptions,
    otel::OpentelemetryOptions,
};
use opentelemetry::{
    trace::{Span, SpanBuilder, SpanContext, Status, Tracer, TracerProvider},
    Context, InstrumentationScope, KeyValue, Value,
};
use std::{borrow::Cow, collections::BTreeMap, sync::Mutex, time::SystemTime};

#[derive(Default)]
pub(super) struct Sample {
    pub commands: BTreeMap<&'static str, u64>,
    pub gaps: Vec<Duration>,
    pub checkout: Vec<Duration>,
    pub in_use_max: usize,
    last_event: Option<Instant>,
    in_use: usize,
}
#[derive(Clone, Default)]
pub(super) struct Observation(Arc<Mutex<Option<Sample>>>);
impl Observation {
    pub(super) async fn client(&self) -> Client {
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.max_pool_size = Some(30);
        options.command_event_handler = None;
        options.tracing = Some(
            OpentelemetryOptions::builder()
                .enabled(true)
                .query_text_max_length(0usize)
                .tracer_provider(self.clone())
                .build(),
        );
        let observation = self.clone();
        options.cmap_event_handler =
            Some(EventHandler::callback(move |event| observation.pool(event)));
        Client::with_options(options).unwrap()
    }
    pub(super) fn begin(&self) {
        *self.0.lock().unwrap() = Some(Sample::default());
    }
    pub(super) fn finish(&self) -> Sample {
        self.0.lock().unwrap().take().unwrap()
    }
    pub(super) fn commands(&self, command: &str) -> u64 {
        self.0
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|sample| sample.commands.get(command).copied())
            .unwrap_or(0)
    }
    fn pool(&self, event: CmapEvent) {
        // Exactly the checkout/check-in events used by the production gap evidence.
        if !matches!(
            event,
            CmapEvent::ConnectionCheckoutStarted(_)
                | CmapEvent::ConnectionCheckedOut(_)
                | CmapEvent::ConnectionCheckedIn(_)
                | CmapEvent::ConnectionCheckoutFailed(_)
        ) {
            return;
        }
        let mut guard = self.0.lock().unwrap();
        let Some(sample) = guard.as_mut() else {
            return;
        };
        let now = Instant::now();
        if let Some(last) = sample.last_event.replace(now) {
            sample.gaps.push(now.duration_since(last));
        }
        match event {
            CmapEvent::ConnectionCheckedOut(event) => {
                sample.checkout.push(event.duration);
                sample.in_use += 1;
                sample.in_use_max = sample.in_use_max.max(sample.in_use);
            }
            CmapEvent::ConnectionCheckedIn(_) => sample.in_use = sample.in_use.saturating_sub(1),
            _ => {}
        }
    }
}
impl TracerProvider for Observation {
    type Tracer = Self;
    fn tracer_with_scope(&self, _: InstrumentationScope) -> Self {
        self.clone()
    }
}
impl Tracer for Observation {
    type Span = DiscardSpan;
    fn build_with_context(&self, builder: SpanBuilder, _: &Context) -> DiscardSpan {
        let name = builder
            .attributes
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find_map(|attribute| {
                if attribute.key.as_str() != "db.command.name" {
                    return None;
                }
                if let Value::String(value) = &attribute.value {
                    Some(value.as_str())
                } else {
                    None
                }
            });
        let command = match name {
            Some("find") => Some("find"),
            Some("getMore") => Some("getMore"),
            Some("killCursors") => Some("killCursors"),
            Some("abortTransaction") => Some("abortTransaction"),
            Some("commitTransaction") => Some("commitTransaction"),
            Some(_) => Some("other"),
            None => None,
        };
        if let Some(command) = command {
            if let Some(sample) = self.0.lock().unwrap().as_mut() {
                *sample.commands.entry(command).or_default() += 1;
            }
        }
        DiscardSpan(SpanContext::NONE)
    }
}
pub(super) struct DiscardSpan(SpanContext);
impl Span for DiscardSpan {
    fn add_event_with_timestamp<T: Into<Cow<'static, str>>>(
        &mut self,
        _: T,
        _: SystemTime,
        _: Vec<KeyValue>,
    ) {
    }
    fn span_context(&self) -> &SpanContext {
        &self.0
    }
    fn is_recording(&self) -> bool {
        false
    }
    fn set_attribute(&mut self, _: KeyValue) {}
    fn set_status(&mut self, _: Status) {}
    fn update_name<T: Into<Cow<'static, str>>>(&mut self, _: T) {}
    fn add_link(&mut self, _: SpanContext, _: Vec<KeyValue>) {}
    fn end_with_timestamp(&mut self, _: SystemTime) {}
}

pub(super) async fn polls<F: std::future::Future>(future: F) -> (F::Output, Vec<Duration>) {
    use std::future::poll_fn;
    tokio::pin!(future);
    let mut samples = Vec::new();
    let output = poll_fn(|cx| {
        let start = Instant::now();
        let output = future.as_mut().poll(cx);
        samples.push(start.elapsed());
        output
    })
    .await;
    (output, samples)
}

pub(super) fn distribution(values: &[Duration]) -> serde_json::Value {
    let mut sorted: Vec<_> = values.iter().map(Duration::as_secs_f64).collect();
    sorted.sort_by(f64::total_cmp);
    let quantile = |percent: usize| {
        sorted
            .get((sorted.len() * percent).div_ceil(100).saturating_sub(1))
            .copied()
            .unwrap_or(0.0)
            * 1000.0
    };
    serde_json::json!({"count": sorted.len(), "mean_ms": sorted.iter().sum::<f64>() * 1000.0 / sorted.len().max(1) as f64,
        "p50_ms": quantile(50), "p95_ms": quantile(95), "p99_ms": quantile(99), "max_ms": quantile(100)})
}
