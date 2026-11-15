//! Hold only bounded diagnostic headers until existing authority checks finish.
use super::{runtime, scope, Context, Operation, Outcome, RemoteParent, Stage, Writer};
use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use std::sync::Mutex;
use tracing::Instrument;

tokio::task_local! {
    static REQUEST: RequestTrace;
}

struct RequestTrace {
    headers: HeaderMap,
    writer: Writer,
    span: tracing::Span,
    operation: Mutex<Option<Operation>>,
}

pub(crate) async fn observe(request: Request, next: Next) -> Response {
    let Some(writer) = runtime::writer() else {
        return next.run(request).await;
    };
    let trace = RequestTrace::new(writer, request.headers());
    let span = trace.span.clone();
    REQUEST
        .scope(
            trace,
            async move {
                let response = next.run(request).await;
                REQUEST.with(|trace| trace.finish(response.status()));
                response
            }
            .instrument(span),
        )
        .await
}

/// Call after normal credential, sealed-reference and ownership validation.
/// Parsing a bearer token alone does not authorize diagnostic context.
pub(crate) fn authorized() {
    let _ = REQUEST.try_with(RequestTrace::authorize);
}

impl RequestTrace {
    fn new(writer: Writer, incoming: &HeaderMap) -> Self {
        let mut headers = HeaderMap::new();
        for name in ["traceparent", "x-request-id"] {
            let mut values = incoming.get_all(name).iter();
            if let Some(value) = values.next() {
                if values.next().is_none() && value.as_bytes().len() <= 256 {
                    headers.insert(name, value.clone());
                }
            }
        }
        Self {
            headers,
            writer,
            span: scope::span(),
            operation: Mutex::new(None),
        }
    }

    fn authorize(&self) {
        let Ok(mut operation) = self.operation.try_lock() else {
            return;
        };
        if operation.is_some() {
            return;
        }
        let Some(parent) = RemoteParent::from_headers(&self.headers) else {
            return;
        };
        // W3C accepts arbitrary nonzero trace IDs. NyxID's schema-1 turn
        // identity is specifically UUID v4; another W3C ID cannot establish it.
        if parent.trace_id.get_version_num() != 4
            || parent.trace_id.get_variant() != uuid::Variant::RFC4122
        {
            return;
        }
        let context = Context {
            turn_id: Some(parent.trace_id),
            span_id: parent.span_id,
            parent_span_id: None,
            // Only the child operation generates and logs a local request ID.
            request_id: parent.ingress_request_id.unwrap_or(parent.trace_id),
            ingress_request_id: parent.ingress_request_id,
        };
        let started = Operation::start(self.writer.clone(), Stage::Gateway, Some(context), None);
        scope::attach(&self.span, started.context());
        *operation = Some(started);
    }

    fn finish(&self, status: StatusCode) {
        if let Ok(mut operation) = self.operation.try_lock() {
            if let Some(operation) = operation.take() {
                // This measures HTTP transport only. A 502 can follow a
                // delivered multipart prefix, so it cannot prove rejection.
                let outcome = if status.is_success() {
                    Outcome::Completed
                } else {
                    Outcome::Unknown
                };
                operation.finish(outcome, Some(status.as_u16()));
            }
        }
    }
}

#[cfg(test)]
mod tests;
