//! Content-free work observations. A staged write is never counted as a commit.
use std::time::Instant;

use crate::store::agent_log_integrity::TransactionLogError;
use layer2_core::sandbox::store::SandboxStoreError;

pub(super) struct Phase {
    started: Instant,
    phase: &'static str,
    rollover: &'static str,
    outcome: &'static str,
}

impl Phase {
    pub fn new(phase: &'static str, rollover: bool) -> Self {
        if phase == "reconcile" {
            metrics::gauge!("cma_agent_log_segment_reconciliations_in_flight").increment(1);
        }
        Self {
            started: Instant::now(),
            phase,
            rollover: if rollover { "yes" } else { "no" },
            outcome: "cancelled",
        }
    }

    pub fn finish<T>(&mut self, result: &Result<T, TransactionLogError>) {
        self.outcome = match result {
            Ok(_) => "ok",
            Err(TransactionLogError::Domain(error)) => classify(error),
            Err(TransactionLogError::Driver(_)) => "driver",
        };
    }

    pub fn reconciliation(
        &mut self,
        result: &Result<super::writer_reconcile::Outcome, SandboxStoreError>,
    ) {
        self.outcome = match result {
            Ok(super::writer_reconcile::Outcome::Recorded(_)) => "recorded",
            Ok(super::writer_reconcile::Outcome::Unknown) => "unknown",
            Err(error) => classify(error),
        };
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        if self.phase == "reconcile" {
            metrics::gauge!("cma_agent_log_segment_reconciliations_in_flight").decrement(1);
        }
        let elapsed = self.started.elapsed().as_secs_f64();
        metrics::counter!("cma_agent_log_segment_work_total", "phase" => self.phase,
            "rollover" => self.rollover, "outcome" => self.outcome)
        .increment(1);
        metrics::histogram!("cma_agent_log_segment_work_seconds", "phase" => self.phase,
            "rollover" => self.rollover, "outcome" => self.outcome)
        .record(elapsed);
        tracing::debug!(
            parent: None,
            phase = self.phase,
            rollover = self.rollover,
            outcome = self.outcome,
            elapsed_seconds = elapsed,
            "Agent log segment work finished"
        );
    }
}

fn classify(error: &SandboxStoreError) -> &'static str {
    match error {
        SandboxStoreError::Conflict | SandboxStoreError::SequenceConflict => "conflict",
        SandboxStoreError::Corrupt(_) => "integrity",
        SandboxStoreError::Unavailable(_) => "unavailable",
        _ => "refused",
    }
}

pub(super) fn source(head: &super::Head) {
    // Sampled distributions avoid IDs and never pretend to be fleet totals.
    for (quantity, value) in [
        ("active_bytes", head.active_bytes),
        ("active_entries", head.active_count),
        ("lifetime_bytes", head.lifetime_bytes),
        ("lifetime_entries", head.last_seq),
        ("closed_segments", head.active_ordinal),
    ] {
        metrics::histogram!("cma_agent_log_segment_source", "quantity" => quantity)
            .record(value as f64);
    }
}

pub(super) fn lag(kind: &'static str, through: i64, expected: i64) {
    metrics::histogram!("cma_agent_log_segment_evidence_lag_entries", "kind" => kind,
        "direction" => if through > expected { "ahead" } else { "behind" })
    .record(through.abs_diff(expected) as f64);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_work_releases_in_flight_gauge_and_never_reports_a_commit() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let phase = Phase::new("reconcile", false);
            assert!(handle
                .render()
                .contains("cma_agent_log_segment_reconciliations_in_flight 1"));
            drop(phase);
            let mut phase = Phase::new("persist", true);
            phase.finish::<()>(&Err(SandboxStoreError::Conflict.into()));
            drop(phase);
            let mut phase = Phase::new("persist", false);
            phase.finish(&Ok(()));
            drop(phase);
            let mut phase = Phase::new("reconcile", false);
            phase.reconciliation(&Ok(super::super::writer_reconcile::Outcome::Unknown));
            drop(phase);
            lag("projection", 10, 12);
        });
        let text = handle.render();
        assert!(text.contains("cma_agent_log_segment_reconciliations_in_flight 0"));
        for outcome in ["cancelled", "conflict", "unknown", "ok"] {
            assert!(text.contains(&format!("outcome=\"{outcome}\"")));
        }
        assert!(text.contains("cma_agent_log_segment_evidence_lag_entries_sum{kind=\"projection\",direction=\"behind\"} 2"));
        assert!(!text.contains("committed"));
    }
}
