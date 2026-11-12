//! Bounded phase telemetry: never accepts resource IDs, credentials or errors.
use std::{future::Future, sync::Arc, time::Instant};

use super::HierarchyReadService;
use crate::diagnostic_context::DiagnosticContext;

#[derive(Clone, Copy)]
pub enum SandboxDetailPhase {
    Auth,
    Hierarchy,
    Colors,
    Names,
    FinalFence,
}

impl SandboxDetailPhase {
    fn label(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::Hierarchy => "hierarchy",
            Self::Colors => "colors",
            Self::Names => "names",
            Self::FinalFence => "final_fence",
        }
    }
}

pub struct SandboxDetailTimer {
    start: Option<Instant>,
    phase: SandboxDetailPhase,
    context: DiagnosticContext,
}

impl HierarchyReadService {
    pub fn with_detail_timings(self: &Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            store: self.store.clone(),
            access: self.access.clone(),
            detail_timings: enabled,
        })
    }

    pub fn detail_phase(&self, phase: SandboxDetailPhase) -> SandboxDetailTimer {
        SandboxDetailTimer {
            start: self.detail_timings.then(Instant::now),
            phase,
            context: DiagnosticContext::current().with_phase(phase.label()),
        }
    }
}

impl SandboxDetailTimer {
    pub async fn result<T, E>(self, future: impl Future<Output = Result<T, E>>) -> Result<T, E> {
        let result = self.context.clone().scope(future).await;
        self.record(&result);
        result
    }

    pub async fn scope<T>(&self, future: impl Future<Output = T>) -> T {
        self.context.clone().scope(future).await
    }

    pub fn record<T, E>(mut self, result: &Result<T, E>) {
        self.emit(if result.is_ok() { "completed" } else { "error" });
    }

    /// Completion includes optional enrichment's existing empty-data fallback.
    /// It does not assert that a dependency was available.
    pub fn complete(mut self) {
        self.emit("completed");
    }

    fn emit(&mut self, outcome: &'static str) {
        if let Some(start) = self.start.take() {
            tracing::info!(
                target: "sandbox_read_phase",
                parent: None,
                phase = self.phase.label(),
                route = %self.context.route,
                request_hash = %self.context.request_hash,
                worker = self.context.worker,
                outcome,
                elapsed_us = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX),
                "sandbox read phase"
            );
        }
    }
}

impl Drop for SandboxDetailTimer {
    fn drop(&mut self) {
        self.emit("cancelled");
    }
}

#[cfg(test)]
#[path = "timing_tests.rs"]
mod tests;
