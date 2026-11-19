//! A route-scoped clock starts before authentication and survives cancellation.
use axum::{extract::Request, http::Method};
use layer2_core::{
    diagnostic_context::DiagnosticContext, sandbox::agent_io::ExternalContextObservation,
};
use sha2::{Digest, Sha256};
use std::sync::{Arc, OnceLock};
use tokio::time::Instant;

const ROUTE: &str =
    "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}";
const HEADER: &str = "x-cma-context-attempt";

#[derive(Clone, Copy, Default)]
pub(crate) struct Enabled(pub bool);

pub(super) struct Read {
    pub hash: Arc<str>,
    pub status: Option<u16>,
    started: Instant,
    entered: Arc<OnceLock<Instant>>,
}

impl Read {
    pub fn start(request: &mut Request, route: &str) -> Option<Self> {
        if !request
            .extensions()
            .get::<Enabled>()
            .is_some_and(|flag| flag.0)
            || request.method() != Method::GET
            || route != ROUTE
        {
            return None;
        }
        let mut values = request.headers().get_all(HEADER).iter();
        let value = values.next();
        let valid = value.filter(|value| {
            value.as_bytes().len() == 32
                && value
                    .as_bytes()
                    .iter()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
                && values.next().is_none()
        });
        let correlation = if valid.is_some() {
            "propagated"
        } else if value.is_some() {
            "invalid"
        } else {
            "absent"
        };
        // Untrusted correlation is diagnostic data only. Invalid/duplicate
        // headers cannot join another attempt and never affect authorization.
        let fallback = valid
            .is_none()
            .then(|| uuid::Uuid::new_v4().simple().to_string());
        let hash: Arc<str> = format!(
            "{:x}",
            Sha256::digest(
                valid.map_or_else(|| fallback.as_ref().unwrap().as_bytes(), |v| v.as_bytes())
            )
        )
        .into();
        let entered = Arc::new(OnceLock::<Instant>::new());
        // The retained-context handler marks entry through this request-local
        // clock; it carries no identity or authorization state across layers.
        request.extensions_mut().insert(entered.clone());
        let read = Self {
            hash,
            started: Instant::now(),
            entered,
            status: None,
        };
        let context = DiagnosticContext::current();
        let api_request_hash = context.request_hash.as_ref();
        let api_request_hash = if api_request_hash.len() == 64
            && api_request_hash.bytes().all(|b| b.is_ascii_hexdigit())
        {
            api_request_hash
        } else {
            "none"
        };
        // Keep JSON current-span rendering independent of request ancestry.
        let span = tracing::info_span!(target: "cma::responses::event_context", parent: None, "retained_context_diagnostic");
        let _entered = span.enter();
        tracing::info!(
            target: "cma::responses::event_context",
            parent: None,
            attempt_hash = read.hash.as_ref(),
            api_request_hash,
            phase = "pre_handler", outcome = "started", correlation,
            "retained event context request"
        );
        Some(read)
    }

    pub async fn scope<F: std::future::Future>(&self, work: F) -> F::Output {
        let observation = ExternalContextObservation::from_hash(self.hash.clone())
            .expect("a SHA-256 digest is a valid observation hash");
        DiagnosticContext::current()
            .with_phase("pre_handler")
            .scope(observation.scope(work))
            .await
    }
}

impl Drop for Read {
    fn drop(&mut self) {
        let span = tracing::info_span!(target: "cma::responses::event_context", parent: None, "retained_context_diagnostic");
        let _entered = span.enter();
        let entered = self.entered.get();
        let now = Instant::now();
        let outcome = if entered.is_some() {
            "completed"
        } else if self.status.is_some() {
            "refused"
        } else {
            "cancelled"
        };
        tracing::info!(
            target: "cma::responses::event_context",
            parent: None,
            attempt_hash = self.hash.as_ref(), phase = "pre_handler", outcome,
            elapsed_ms = entered.unwrap_or(&now).duration_since(self.started).as_millis() as u64,
            "retained event context phase"
        );
        // Completion means headers are ready, before the body is consumed.
        tracing::info!(
            target: "cma::responses::event_context",
            parent: None,
            attempt_hash = self.hash.as_ref(),
            phase = "http_response_headers",
            outcome = if self.status.is_some() { "completed" } else { "cancelled" },
            status = self.status,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "retained event context phase"
        );
    }
}

#[cfg(test)]
#[path = "context/tests.rs"]
mod tests;
