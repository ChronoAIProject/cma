//! Fresh per-attempt correlation, independent of event identity and credentials.
use super::ContextFailure;
use crate::providers::EventContextSnapshot;
use tokio::time::Instant;
use tracing::Span;
use tracing_subscriber::{registry::LookupSpan, Registry};

#[path = "context_diagnostics_config.rs"]
pub(in crate::providers) mod config;

pub(in crate::providers) const HEADER: &str = "x-cma-context-attempt";

struct IngressRequestHash(String);

/// Accept only the ingress middleware's digest, never formatted span fields.
pub(crate) fn attach_request_hash(span: &Span, hash: &str) {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return;
    }
    span.with_subscriber(|(id, dispatch)| {
        if let Some(registry) = dispatch.downcast_ref::<Registry>() {
            if let Some(span) = registry.span(id) {
                let mut extensions = span.extensions_mut();
                if extensions.get_mut::<IngressRequestHash>().is_none() {
                    extensions.insert(IngressRequestHash(hash.to_owned()));
                }
            }
        }
    });
}

fn request_hash() -> Option<String> {
    Span::current()
        .with_subscriber(|(id, dispatch)| {
            let registry = dispatch.downcast_ref::<Registry>()?;
            registry.span(id)?.scope().find_map(|span| {
                span.extensions()
                    .get::<IngressRequestHash>()
                    .map(|hash| hash.0.clone())
            })
        })
        .flatten()
}

pub(in crate::providers) fn span() -> Span {
    // JSON formatters inspect the current span even for parentless events.
    // Enter this empty root only while emitting, never across an await.
    tracing::info_span!(parent: None, "context_diagnostic")
}

pub(in crate::providers) fn lookup(enabled: bool) -> Option<String> {
    enabled.then(|| {
        let hash = crate::telemetry::http::correlation_hash(uuid::Uuid::new_v4().as_bytes());
        let request_hash = request_hash();
        let span = span();
        let _entered = span.enter();
        tracing::info!(parent: None, request_hash, lookup_hash = hash, "provider context lookup started");
        hash
    })
}

pub(in crate::providers) struct Attempt(Option<ActiveAttempt>);

struct ActiveAttempt {
    header: String,
    hash: String,
    lookup: String,
    number: u8,
    started: Instant,
    phase: &'static str,
    outcome: &'static str,
    reason: &'static str,
    status: Option<u16>,
}

impl Attempt {
    pub fn new(lookup: Option<&str>, number: u8) -> Self {
        Self(lookup.map(|lookup| {
            let header = uuid::Uuid::new_v4().simple().to_string();
            let value = ActiveAttempt {
                hash: crate::telemetry::http::correlation_hash(header.as_bytes()),
                header,
                lookup: lookup.to_owned(),
                number,
                started: Instant::now(),
                phase: "credential_tracking",
                outcome: "cancelled",
                reason: "future_dropped",
                status: None,
            };
            value.log("started");
            value
        }))
    }

    pub fn header(&self) -> Option<&str> {
        self.0.as_ref().map(|active| active.header.as_str())
    }

    pub fn phase(&mut self, phase: &'static str) {
        if let Some(active) = &mut self.0 {
            active.phase = phase;
        }
    }

    pub fn status(&mut self, status: u16) {
        if let Some(active) = &mut self.0 {
            active.status = Some(status);
        }
    }

    pub fn finish(&mut self, result: &Result<Option<EventContextSnapshot>, ContextFailure>) {
        if let Some(active) = &mut self.0 {
            (active.outcome, active.reason) = match result {
                Ok(Some(_)) => ("completed", "retained"),
                Ok(None) => ("completed", "absent"),
                Err(failure) => ("error", failure.class),
            };
        }
    }
}

impl ActiveAttempt {
    fn log(&self, state: &'static str) {
        let span = span();
        let _entered = span.enter();
        tracing::info!(
            parent: None,
            lookup_hash = self.lookup,
            attempt_hash = self.hash,
            attempt = self.number,
            phase = self.phase,
            outcome = state,
            reason = if state == "started" { "none" } else { self.reason },
            upstream_status = self.status,
            // The active phase attributes the wait; this clock covers the entire GET.
            attempt_elapsed_ms = self.started.elapsed().as_millis() as u64,
            "provider context attempt"
        );
    }
}

impl Drop for ActiveAttempt {
    fn drop(&mut self) {
        self.log(self.outcome);
    }
}

#[cfg(test)]
#[path = "context_diagnostics_tests.rs"]
mod tests;
