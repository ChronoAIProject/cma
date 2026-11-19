use super::{CommandAdmissionError, CommandLedgerError};
use crate::diagnostic_context::DiagnosticContext;
use std::{future::Future, sync::Arc};
use tokio::time::Instant;

tokio::task_local! { static OBSERVATION: ExternalContextObservation; }

/// Opt-in, operation-scoped observation. A hash never grants read authority.
#[derive(Clone)]
pub struct ExternalContextObservation(Arc<str>);

impl ExternalContextObservation {
    pub fn from_hash(hash: Arc<str>) -> Option<Self> {
        (hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then_some(Self(hash))
    }

    pub fn current() -> Option<Self> {
        OBSERVATION.try_with(Clone::clone).ok()
    }

    pub fn hash(&self) -> &str {
        &self.0
    }

    pub async fn scope<F: Future>(self, work: F) -> F::Output {
        let mut context = DiagnosticContext::current();
        context.request_hash = self.0.clone();
        context.scope(OBSERVATION.scope(self, work)).await
    }
}

struct BaselinePhase {
    phase: &'static str,
    started: std::time::Instant,
    completed: bool,
}
impl Drop for BaselinePhase {
    fn drop(&mut self) {
        if self.completed || self.phase == "total" {
            let span = tracing::info_span!(target: "cma::responses::event_context", parent: None, "retained_context_diagnostic");
            let _entered = span.enter();
            tracing::info!(
                target: "cma::responses::event_context", parent: None,
                phase = self.phase,
                elapsed_ms = self.started.elapsed().as_millis() as u64,
                "retained event context phase"
            );
        }
    }
}

struct Phase {
    phase: &'static str,
    hash: Arc<str>,
    started: Instant,
    outcome: &'static str,
    reason: &'static str,
}

impl Phase {
    fn log(&self, outcome: &'static str) {
        // JSON formatters may inspect the current span even for root events.
        // Enter an empty root only while emitting; never across an await.
        let span = tracing::info_span!(target: "cma::responses::event_context", parent: None, "retained_context_diagnostic");
        let _entered = span.enter();
        tracing::info!(
            target: "cma::responses::event_context",
            parent: None,
            attempt_hash = self.hash.as_ref(),
            phase = self.phase,
            outcome,
            reason = if outcome == "started" { "none" } else { self.reason },
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "retained event context phase"
        );
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        self.log(self.outcome);
    }
}

pub(super) async fn observe<T>(
    phase: &'static str,
    work: impl Future<Output = Result<T, CommandAdmissionError>>,
) -> Result<T, CommandAdmissionError> {
    let Some(observation) = ExternalContextObservation::current() else {
        // The default path retains completion-only phase records and total-on-drop.
        let mut timing = BaselinePhase {
            phase,
            started: std::time::Instant::now(),
            completed: false,
        };
        let result = work.await;
        timing.completed = true;
        return result;
    };
    let mut context = DiagnosticContext::current().with_phase(phase);
    context.request_hash = observation.0.clone();
    let mut timing = Phase {
        phase,
        hash: observation.0,
        started: Instant::now(),
        outcome: "cancelled",
        reason: "future_dropped",
    };
    timing.log("started");
    let result = context.scope(work).await;
    (timing.outcome, timing.reason) = match &result {
        Ok(_) => ("completed", "none"),
        Err(error) => (
            "error",
            match error {
                CommandAdmissionError::Ledger(CommandLedgerError::NotFound) => "not_found",
                CommandAdmissionError::Ledger(CommandLedgerError::Unavailable) => {
                    "store_unavailable"
                }
                CommandAdmissionError::Ledger(_) => "ledger_refusal",
                CommandAdmissionError::Access(error) => {
                    crate::sandbox::agent_io::command_access_error_kind(error)
                }
                CommandAdmissionError::Authority(
                    crate::sandbox::agent_io::CommandAuthorizationError::Unavailable,
                ) => "authority_unavailable",
                CommandAdmissionError::Authority(_) => "authority_refusal",
                CommandAdmissionError::Input(_) => "invalid_input",
                CommandAdmissionError::ModelMismatch => "model_mismatch",
            },
        ),
    };
    result
}

#[cfg(test)]
#[path = "external_event_read_diagnostics_tests.rs"]
mod tests;
