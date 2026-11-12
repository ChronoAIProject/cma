//! Ephemeral diagnostic labels. Callers supply only registered operation names,
//! route templates and opaque hashes, never resource IDs or payload values.
use std::{future::Future, sync::Arc};

tokio::task_local! { static CONTEXT: DiagnosticContext; }

#[derive(Clone, Debug)]
pub struct DiagnosticContext {
    pub route: Arc<str>,
    pub request_hash: Arc<str>,
    pub worker: &'static str,
    pub phase: &'static str,
}

impl Default for DiagnosticContext {
    fn default() -> Self {
        Self {
            route: "none".into(),
            request_hash: "none".into(),
            worker: "unattributed",
            phase: "unspecified",
        }
    }
}

impl DiagnosticContext {
    pub fn current() -> Self {
        CONTEXT.try_with(Clone::clone).unwrap_or_default()
    }

    pub fn background(worker: &'static str) -> Self {
        Self {
            worker,
            ..Self::default()
        }
    }

    pub fn with_worker(mut self, worker: &'static str) -> Self {
        self.worker = worker;
        self
    }

    pub fn with_phase(mut self, phase: &'static str) -> Self {
        self.phase = phase;
        self
    }

    pub async fn scope<F: Future>(self, future: F) -> F::Output {
        CONTEXT.scope(self, future).await
    }

    /// For streaming bodies whose polls occur after the request future returns.
    pub fn sync_scope<T>(&self, operation: impl FnOnce() -> T) -> T {
        CONTEXT.sync_scope(self.clone(), operation)
    }
}
