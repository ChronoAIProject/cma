//! Closed, content-free outcomes. Successful folds count only after commit.
#[derive(Clone, Copy)]
pub(crate) enum Outcome {
    SkippedCurrent,
    Unchanged,
    Advanced,
    Settled,
    Error,
}

impl Outcome {
    fn name(self) -> &'static str {
        match self {
            Self::SkippedCurrent => "skipped_current",
            Self::Unchanged => "unchanged",
            Self::Advanced => "advanced",
            Self::Settled => "settled",
            Self::Error => "error",
        }
    }

    pub(crate) fn record(self) {
        metrics::counter!("cma_command_reconcile_total", "outcome" => self.name()).increment(1);
    }
}

pub(super) fn initialize() {
    for outcome in [
        Outcome::SkippedCurrent,
        Outcome::Unchanged,
        Outcome::Advanced,
        Outcome::Settled,
        Outcome::Error,
    ] {
        metrics::counter!("cma_command_reconcile_total", "outcome" => outcome.name()).absolute(0);
    }
}

pub(crate) fn heads_failed() {
    metrics::counter!("cma_command_reconcile_heads_errors_total").increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_are_closed_and_head_failures_have_no_resource_labels() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            initialize();
            for outcome in [
                Outcome::SkippedCurrent,
                Outcome::Unchanged,
                Outcome::Advanced,
                Outcome::Settled,
                Outcome::Error,
            ] {
                outcome.record();
            }
            heads_failed();
        });
        let rendered = handle.render();
        let lines: Vec<_> = rendered
            .lines()
            .filter(|line| line.starts_with("cma_command_reconcile_total{"))
            .collect();
        assert_eq!(lines.len(), 5);
        for outcome in [
            "skipped_current",
            "unchanged",
            "advanced",
            "settled",
            "error",
        ] {
            assert!(lines.contains(
                &format!("cma_command_reconcile_total{{outcome=\"{outcome}\"}} 1").as_str()
            ));
        }
        assert!(rendered
            .lines()
            .any(|line| line == "cma_command_reconcile_heads_errors_total 1"));
    }
}
