//! Bounded retry-verification labels; no Agent identities or failure contents.
use crate::store::agent_log_integrity::{parse_integrity, INTEGRITY_FIELD};
use crate::store::agent_log_telemetry::ScanCaller;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, AgentWorkStep};
use mongodb::bson::Document;

#[derive(Clone, Copy)]
pub(in super::super) enum ReadDecision {
    FlagDisabled,
    CohortExcluded,
    FullRequired,
    AuditDue,
    Proof,
    ProofFallback,
    ProofError,
    ConflictReuse,
    MarkerConflict,
}

impl ReadDecision {
    fn label(self) -> &'static str {
        match self {
            Self::FlagDisabled => "flag_disabled",
            Self::CohortExcluded => "cohort_excluded",
            Self::FullRequired => "full_required",
            Self::AuditDue => "audit_due",
            Self::Proof => "proof",
            Self::ProofFallback => "proof_fallback",
            Self::ProofError => "proof_error",
            Self::ConflictReuse => "conflict_reuse",
            Self::MarkerConflict => "marker_conflict",
        }
    }
}

#[derive(Clone, Copy)]
pub(in super::super) struct CloseRetry {
    mode: &'static str,
    cohort: &'static str,
}

impl CloseRetry {
    fn new(enabled: bool, selected: bool) -> Self {
        Self {
            mode: if enabled { "on" } else { "off" },
            cohort: if selected { "included" } else { "excluded" },
        }
    }

    fn eligible_counter(self) -> metrics::Counter {
        metrics::counter!("cma_agent_close_retry_eligible_attempts_total",
            "mode" => self.mode, "cohort" => self.cohort)
    }

    pub(in super::super) fn initialize(enabled: bool) {
        for selected in [false, true] {
            Self::new(enabled, selected).eligible_counter().absolute(0);
        }
    }

    /// An eligible proposal can still fail snapshot verification or publication.
    pub(in super::super) fn observe(enabled: bool, selected: bool) -> Self {
        let retry = Self::new(enabled, selected);
        retry.eligible_counter().increment(1);
        retry
    }

    pub(in super::super) fn verification(self, decision: ReadDecision) {
        metrics::counter!("cma_agent_close_retry_verification_total",
            "mode" => self.mode, "cohort" => self.cohort, "decision" => decision.label())
        .increment(1);
    }
}

/// Counts verification decisions per transaction attempt, independently of commit.
pub(in super::super) fn read(
    caller: ScanCaller,
    close_retry: Option<CloseRetry>,
    decision: ReadDecision,
) {
    if let Some(retry) = close_retry {
        retry.verification(decision);
    }
    if let ScanCaller::CapacitySettlement(step) = caller {
        metrics::counter!("cma_agent_capacity_settlement_reads_total",
            "worker_step" => step.operation_name(), "decision" => decision.label())
        .increment(1);
    }
}

pub(in super::super) enum Verification {
    CompletedFull,
    SkippedProof,
    SkippedConflictReuse,
}

pub(in super::super) fn record(step: AgentWorkStep, verification: Verification) {
    let outcome = match verification {
        Verification::CompletedFull => "completed_full",
        Verification::SkippedProof => "skipped_proof",
        Verification::SkippedConflictReuse => "skipped_conflict_reuse",
    };
    metrics::counter!("cma_agent_retry_audits_total",
        "worker_step" => step.operation_name(), "outcome" => outcome)
    .increment(1);
}

/// Called only after the authoritative trusted-summary check chose a fallback.
/// Reuse its parser for classification; telemetry never grants bounded access.
pub(in super::super) fn fallback(agent: &Agent, row: &Document) -> Result<(), SandboxStoreError> {
    let reason = match parse_integrity(row)? {
        None if row.contains_key(INTEGRITY_FIELD) => "unsupported",
        None => "missing",
        Some(proof)
            if proof.head_seq != agent.conversation.last_seq
                || proof.bound_agent_revision != agent.revision =>
        {
            "stale"
        }
        Some(_) => "untrusted",
    };
    metrics::counter!("cma_agent_retry_audit_fallbacks_total",
        "worker_step" => agent.work.step.operation_name(), "reason" => reason)
    .increment(1);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_retry_exports_empty_exposure_and_separate_verification_decisions() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let metrics = recorder.handle();
        let _observation = metrics::set_default_local_recorder(&recorder);
        CloseRetry::initialize(false);
        CloseRetry::initialize(true);
        let caller = ScanCaller::WorkerPublication(AgentWorkStep::CloseProcess);
        let off = CloseRetry::observe(false, true);
        read(caller, Some(off), ReadDecision::FlagDisabled);
        let on = CloseRetry::observe(true, true);
        read(caller, Some(on), ReadDecision::AuditDue);
        let hit = CloseRetry::observe(true, true);
        read(caller, Some(hit), ReadDecision::Proof);

        let rendered = metrics.render();
        let counter = |name: &str, mode: &str, cohort: &str, decision: Option<&str>| {
            rendered
                .lines()
                .find(|line| {
                    line.starts_with(name)
                        && line.contains(&format!("mode=\"{mode}\""))
                        && line.contains(&format!("cohort=\"{cohort}\""))
                        && decision
                            .is_none_or(|value| line.contains(&format!("decision=\"{value}\"")))
                })
                .unwrap()
                .rsplit_once(' ')
                .unwrap()
                .1
        };
        let eligible = "cma_agent_close_retry_eligible_attempts_total{";
        let verification = "cma_agent_close_retry_verification_total{";
        assert_eq!(counter(eligible, "off", "included", None), "1");
        assert_eq!(counter(eligible, "on", "included", None), "2");
        assert_eq!(counter(eligible, "on", "excluded", None), "0");
        assert_eq!(
            counter(verification, "off", "included", Some("flag_disabled")),
            "1"
        );
        assert_eq!(
            counter(verification, "on", "included", Some("audit_due")),
            "1"
        );
        assert_eq!(counter(verification, "on", "included", Some("proof")), "1");
        assert!(!rendered.contains("cma_agent_capacity_settlement_reads_total"));
    }

    #[test]
    fn settlement_decisions_are_fixed_labels_and_exclude_other_callers() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let metrics = recorder.handle();
        let observation = metrics::set_default_local_recorder(&recorder);
        for (decision, label) in [
            (ReadDecision::FlagDisabled, "flag_disabled"),
            (ReadDecision::CohortExcluded, "cohort_excluded"),
            (ReadDecision::FullRequired, "full_required"),
            (ReadDecision::AuditDue, "audit_due"),
            (ReadDecision::Proof, "proof"),
            (ReadDecision::ProofFallback, "proof_fallback"),
            (ReadDecision::ProofError, "proof_error"),
            (ReadDecision::ConflictReuse, "conflict_reuse"),
        ] {
            read(
                ScanCaller::CapacitySettlement(AgentWorkStep::CloseProcess),
                None,
                decision,
            );
            let rendered = metrics.render();
            let line = rendered
                .lines()
                .find(|line| {
                    line.starts_with("cma_agent_capacity_settlement_reads_total{")
                        && line.contains(&format!("decision=\"{label}\""))
                })
                .unwrap();
            assert!(line.contains("worker_step=\"close_process\""), "{line}");
            assert!(line.ends_with(" 1"), "{line}");
        }
        let before = metrics.render();
        read(
            ScanCaller::WorkerPublication(AgentWorkStep::CloseProcess),
            None,
            ReadDecision::Proof,
        );
        drop(observation);
        // Prometheus series ordering is unspecified; compare the exact series
        // and values without making their export order part of this contract.
        let after = metrics.render();
        let mut before_lines: Vec<_> = before.lines().collect();
        let mut after_lines: Vec<_> = after.lines().collect();
        before_lines.sort_unstable();
        after_lines.sort_unstable();
        assert_eq!(after_lines, before_lines);
        assert_eq!(
            before
                .lines()
                .filter(|line| line.starts_with("cma_agent_capacity_settlement_reads_total{"))
                .count(),
            8
        );
    }
}
