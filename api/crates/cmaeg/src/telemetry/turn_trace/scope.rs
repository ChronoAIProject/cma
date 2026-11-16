//! Typed context travels in existing spans, never in execution keys or bodies.
use super::Context;
use tracing::{Span, Subscriber};
use tracing_subscriber::{filter::filter_fn, registry::LookupSpan, Layer, Registry};

const TARGET: &str = "cmaeg::turn_context";

struct ContextLayer;
impl<S: Subscriber> Layer<S> for ContextLayer {}

/// Keep the context carrier alive even when ordinary service logs are filtered.
/// The layer has no event writer and never records surrounding span fields.
pub(crate) fn layer<S: Subscriber + for<'a> LookupSpan<'a>>() -> Option<impl Layer<S>> {
    super::runtime::writer().map(|_| testable_layer())
}

pub(super) fn testable_layer<S: Subscriber + for<'a> LookupSpan<'a>>() -> impl Layer<S> {
    ContextLayer.with_filter(filter_fn(|metadata| {
        metadata.is_span() && metadata.target() == TARGET
    }))
}

pub(crate) fn span() -> Span {
    tracing::info_span!(target: "cmaeg::turn_context", "turn_context")
}

pub(crate) fn attach(span: &Span, context: Context) {
    span.with_subscriber(|(id, dispatch)| {
        if let Some(registry) = dispatch.downcast_ref::<Registry>() {
            if let Some(span) = registry.span(id) {
                // Each carrier belongs to one operation. Replays retain the
                // existing winner rather than replacing it with attempt IDs.
                let mut extensions = span.extensions_mut();
                if extensions.get_mut::<Context>().is_none() {
                    extensions.insert(context);
                }
            }
        }
    });
}

pub(crate) fn context(span: &Span) -> Option<Context> {
    span.with_subscriber(|(id, dispatch)| {
        let registry = dispatch.downcast_ref::<Registry>()?;
        registry
            .span(id)?
            .scope()
            .find_map(|span| span.extensions().get::<Context>().copied())
    })
    .flatten()
}
