//! Process observations after confirmed persistence, never a durable SLI ledger.
use layer2_core::sandbox::agent_io::{CommandLedgerError, CommandReservationResult};
use layer2_core::sandbox::{AgentLogEntry, AgentLogKind};
use std::time::Instant;

pub(crate) fn initialize() {
    for outcome in ["completed", "failed", "cancelled"] {
        for confirmation in ["committed", "reconciled"] {
            metrics::counter!("cma_terminal_publications_total", "outcome" => outcome, "confirmation" => confirmation).absolute(0);
        }
        metrics::counter!("cma_terminal_native_timestamp_missing_total", "outcome" => outcome)
            .absolute(0);
        let _ =
            metrics::histogram!("cma_terminal_observation_to_commit_seconds", "outcome" => outcome);
    }
    metrics::counter!("cma_terminal_timestamp_invalid_total").absolute(0);
}

pub(crate) struct Admission {
    operation: &'static str,
    started: Instant,
    outcome: &'static str,
}
impl Admission {
    pub(crate) fn new(operation: &'static str) -> Self {
        Self {
            operation,
            started: Instant::now(),
            outcome: "cancelled",
        }
    }
    pub(crate) fn result<T>(&mut self, result: &Result<T, CommandLedgerError>) {
        self.outcome = match result {
            Ok(_) => "confirmed",
            Err(CommandLedgerError::Unavailable) => "unconfirmed",
            Err(error) => error.code(),
        };
    }
    pub(crate) fn reservation(
        &mut self,
        result: &Result<CommandReservationResult, CommandLedgerError>,
    ) {
        self.result(result);
        match result {
            Ok(CommandReservationResult::Created(_)) => self.outcome = "created",
            Ok(CommandReservationResult::Existing(_)) => self.outcome = "existing",
            Err(_) => {}
        }
    }
}
impl Drop for Admission {
    fn drop(&mut self) {
        metrics::counter!("cma_admission_attempts_total", "operation" => self.operation, "outcome" => self.outcome).increment(1);
        metrics::histogram!("cma_admission_seconds", "operation" => self.operation, "outcome" => self.outcome).record(self.started.elapsed().as_secs_f64());
    }
}

pub(crate) fn published(entries: &[AgentLogEntry], confirmation: &'static str) {
    let now = super::timestamp();
    for entry in entries {
        let Some(outcome) = terminal(&entry.kind) else {
            continue;
        };
        metrics::counter!("cma_terminal_publications_total", "outcome" => outcome, "confirmation" => confirmation).increment(1);
        // appended_at is AM observation/preparation time, NOT native completion.
        if let Some(delay) = observed_delay(&entry.appended_at, now) {
            metrics::histogram!("cma_terminal_observation_to_commit_seconds", "outcome" => outcome)
                .record(delay);
        } else {
            metrics::counter!("cma_terminal_timestamp_invalid_total").increment(1);
        }
        metrics::counter!("cma_terminal_native_timestamp_missing_total", "outcome" => outcome)
            .increment(1);
    }
}
fn terminal(kind: &AgentLogKind) -> Option<&'static str> {
    match kind {
        AgentLogKind::TurnCompleted { .. } => Some("completed"),
        AgentLogKind::TurnFailed { .. } => Some("failed"),
        AgentLogKind::TurnInterrupted { .. } => Some("cancelled"),
        _ => None,
    }
}
fn observed_delay(at: &str, now: f64) -> Option<f64> {
    let at = layer2_core::seconds_from_rfc3339(at)? as f64;
    (now >= at).then_some(now - at)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_real_terminal_kinds_and_valid_clock_order_produce_samples() {
        assert_eq!(
            terminal(&AgentLogKind::TurnCompleted { turn_sequence: 1 }),
            Some("completed")
        );
        assert_eq!(
            terminal(&AgentLogKind::TurnInterrupted { turn_sequence: 1 }),
            Some("cancelled")
        );
        assert_eq!(
            terminal(&AgentLogKind::AgentMessage {
                content: "private".into()
            }),
            None
        );
        assert_eq!(
            observed_delay("2026-11-11T00:00:00Z", 1794355205.),
            Some(5.)
        );
        assert_eq!(observed_delay("2026-11-11T00:00:00Z", 0.), None);
        assert_eq!(observed_delay("invalid", 1794355205.), None);
    }
    #[test]
    fn cancellation_and_unconfirmed_results_are_never_accepted_counts() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            {
                let _cancelled = Admission::new("reserve_command");
            }
            let mut unconfirmed = Admission::new("reserve_command");
            unconfirmed.result::<()>(&Err(CommandLedgerError::Unavailable));
        });
        let text = handle.render();
        assert!(text.contains("outcome=\"cancelled\""));
        assert!(text.contains("outcome=\"unconfirmed\""));
        assert!(!text.contains("outcome=\"created\""));
    }
}
