//! Consume only driver span metadata; never install the eager BSON command observer.
use super::{metadata, Diagnostics};
use mongodb::{options::ClientOptions, otel::OpentelemetryOptions};
use opentelemetry::{
    trace::{Span, SpanBuilder, SpanContext, Status, Tracer, TracerProvider},
    Context, InstrumentationScope, KeyValue, Value,
};
use std::{
    borrow::Cow,
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::{Instant, SystemTime},
};

pub(super) fn disabled() -> OpentelemetryOptions {
    OpentelemetryOptions::builder()
        .enabled(false)
        .query_text_max_length(0usize)
        .build()
}

pub(super) fn install(options: &mut ClientOptions, diagnostics: Diagnostics) {
    // A public command handler forces complete reply decoding in driver 3.9.1.
    options.command_event_handler = None;
    options.tracing = Some(
        OpentelemetryOptions::builder()
            .enabled(true)
            .query_text_max_length(0usize)
            .tracer_provider(Provider(diagnostics))
            .build(),
    );
}

#[derive(Clone)]
struct Provider(Diagnostics);
impl TracerProvider for Provider {
    type Tracer = Self;
    fn tracer_with_scope(&self, _scope: InstrumentationScope) -> Self {
        self.clone()
    }
}
impl Tracer for Provider {
    type Span = MetadataSpan;
    fn build_with_context(&self, builder: SpanBuilder, _parent: &Context) -> MetadataSpan {
        let attributes = builder.attributes.as_deref().unwrap_or_default();
        // Operation/transaction spans are not command attempts; don't double count.
        let command = text(attributes, "db.command.name");
        let tracked = command.zip(connection(attributes)).and_then(|(name, key)| {
            let collection = text(attributes, "db.collection.name").unwrap_or("");
            let session = attributes
                .iter()
                .any(|a| a.key.as_str() == "db.mongodb.lsid");
            self.0
                .started(key, metadata(name, collection, session, false))
                .map(|id| (key, id))
        });
        // No attributes, span name, namespace, lsid, error or query text survives.
        MetadataSpan {
            diagnostics: self.0.clone(),
            tracked,
            started: Instant::now(),
            context: SpanContext::NONE,
            failed: false,
            succeeded: false,
        }
    }
}
fn text<'a>(attributes: &'a [KeyValue], key: &str) -> Option<&'a str> {
    attributes.iter().find_map(|a| {
        if a.key.as_str() == key {
            if let Value::String(value) = &a.value {
                return Some(value.as_str());
            }
        }
        None
    })
}
fn number(attributes: &[KeyValue], key: &str) -> Option<i64> {
    attributes.iter().find_map(|a| {
        if a.key.as_str() == key {
            if let Value::I64(value) = a.value {
                return Some(value);
            }
        }
        None
    })
}
fn connection(attributes: &[KeyValue]) -> Option<u64> {
    let id = u32::try_from(number(attributes, "db.mongodb.driver_connection_id")?).ok()?;
    let host = text(attributes, "server.address")?;
    let mut hash = DefaultHasher::new();
    match text(attributes, "network.transport")? {
        "tcp" => {
            host.hash(&mut hash);
            u16::try_from(number(attributes, "server.port")?)
                .ok()?
                .hash(&mut hash);
        }
        #[cfg(unix)]
        "unix" => std::path::Path::new(host).hash(&mut hash),
        _ => return None,
    }
    id.hash(&mut hash);
    Some(hash.finish())
}
struct MetadataSpan {
    diagnostics: Diagnostics,
    tracked: Option<(u64, u64)>,
    started: Instant,
    context: SpanContext,
    failed: bool,
    succeeded: bool,
}
impl MetadataSpan {
    fn complete(&mut self) {
        if let Some((connection, id)) = self.tracked.take() {
            self.diagnostics.finished(
                connection,
                id,
                self.started.elapsed(),
                None,
                self.failed,
                self.failed || self.succeeded,
            );
        }
    }
}
impl Drop for MetadataSpan {
    fn drop(&mut self) {
        self.complete();
    }
}
impl Span for MetadataSpan {
    fn add_event_with_timestamp<T: Into<Cow<'static, str>>>(
        &mut self,
        _name: T,
        _timestamp: SystemTime,
        _attributes: Vec<KeyValue>,
    ) {
    }
    fn span_context(&self) -> &SpanContext {
        &self.context
    }
    fn is_recording(&self) -> bool {
        self.tracked.is_some()
    }
    fn set_attribute(&mut self, attribute: KeyValue) {
        // The driver sets this only after a successful cursor command. Never
        // retain its value. Other successful commands and cancellation have no
        // positive outcome marker in this API and remain explicitly unknown.
        if attribute.key.as_str() == "db.mongodb.cursor_id" {
            self.succeeded = true;
        }
    }
    fn set_status(&mut self, status: Status) {
        self.failed |= matches!(status, Status::Error { .. });
    }
    fn record_error(&mut self, _error: &dyn std::error::Error) {
        self.failed = true;
    }
    fn update_name<T: Into<Cow<'static, str>>>(&mut self, _name: T) {}
    fn add_link(&mut self, _context: SpanContext, _attributes: Vec<KeyValue>) {}
    fn end_with_timestamp(&mut self, _timestamp: SystemTime) {
        self.complete();
    }
}

#[cfg(test)]
mod tests;
