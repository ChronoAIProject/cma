use axum::http::StatusCode;
use layer2_core::{
    diagnostic_context::DiagnosticContext, sandbox::agent_io::ExternalContextObservation,
};
use tokio::time::Instant;

pub(in crate::responses::v2) struct Phase(Option<ActivePhase>);

struct ActivePhase {
    phase: &'static str,
    hash: std::sync::Arc<str>,
    started: Instant,
    outcome: &'static str,
    reason: &'static str,
    status: Option<u16>,
}

impl Phase {
    pub fn new(phase: &'static str) -> Self {
        Self(ExternalContextObservation::current().map(|observation| {
            let value = ActivePhase {
                phase,
                hash: observation.hash().into(),
                started: Instant::now(),
                outcome: "cancelled",
                reason: "future_dropped",
                status: None,
            };
            value.log("started");
            value
        }))
    }

    pub fn response(&mut self, status: StatusCode) {
        let Some(active) = &mut self.0 else {
            return;
        };
        active.status = Some(status.as_u16());
        (active.outcome, active.reason) = if status.is_success() {
            ("completed", "retained")
        } else {
            (
                "error",
                match status.as_u16() {
                    404 => "not_found",
                    401 | 403 => "authority_refusal",
                    _ => "unavailable",
                },
            )
        };
    }
}

impl ActivePhase {
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
            status = self.status,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "retained event context phase"
        );
    }
}

impl Drop for ActivePhase {
    fn drop(&mut self) {
        self.log(self.outcome);
    }
}

pub(in crate::responses::v2) async fn setup<T>(
    work: impl std::future::Future<Output = Result<Option<T>, layer2_core::bots::BotError>>,
) -> Result<Option<T>, layer2_core::bots::BotError> {
    let Some(observation) = ExternalContextObservation::current() else {
        return work.await;
    };
    let mut timing = Phase::new("setup_context");
    let mut context = DiagnosticContext::current().with_phase("setup_context");
    context.request_hash = observation.hash().into();
    let result = context.scope(work).await;
    if let Some(active) = &mut timing.0 {
        (active.outcome, active.reason) = match &result {
            Ok(Some(_)) => ("completed", "retained"),
            Ok(None) => ("completed", "absent"),
            Err(layer2_core::bots::BotError::Forbidden) => ("error", "authority_refusal"),
            Err(_) => ("error", "unavailable"),
        };
    }
    result
}
