use std::{future::Future, sync::Arc};

tokio::task_local! {
    static REQUEST_EVIDENCE: OwnerCommandRoutingEvidence;
}

/// Ephemeral original-request proof owned and interpreted by the application
/// adapter. These bytes are not authorization and must never be logged or stored.
/// Deliberately does not implement Debug or serialization.
#[derive(Clone)]
pub struct OwnerCommandRoutingEvidence(Arc<[u8]>);

impl OwnerCommandRoutingEvidence {
    pub fn new(bytes: Arc<[u8]>) -> Self {
        Self(bytes)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn current() -> Option<Self> {
        REQUEST_EVIDENCE.try_with(Clone::clone).ok()
    }

    pub fn scope<F: Future>(&self, future: F) -> impl Future<Output = F::Output> + use<'_, F> {
        // Pin before constructing async state: nested request scopes must not
        // multiply the admission future retained by every caller. Enter the
        // scope only when polled, including the original cancellation behavior.
        let future = Box::pin(future);
        async move { REQUEST_EVIDENCE.scope(self.clone(), future).await }
    }

    /// Capture before spawning and re-enter inside the child task. Tokio task
    /// locals are intentionally not inherited by detached admission tasks.
    pub fn scope_optional<F: Future>(
        evidence: Option<Self>,
        future: F,
    ) -> impl Future<Output = F::Output> {
        let future = Box::pin(future);
        async move {
            match evidence {
                Some(evidence) => evidence.scope(future).await,
                None => future.await,
            }
        }
    }
}

#[cfg(test)]
#[path = "evidence_tests.rs"]
mod tests;
