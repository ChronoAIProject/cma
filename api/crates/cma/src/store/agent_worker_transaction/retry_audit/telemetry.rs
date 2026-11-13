//! Bounded retry-verification labels; no Agent identities or failure contents.
use crate::store::agent_log_integrity::{parse_integrity, INTEGRITY_FIELD};
use crate::store::agent_log_telemetry::ScanCaller;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, AgentWorkStep};
use mongodb::bson::Document;

pub(in super::super) enum SettlementRead {
    FlagDisabled,
    CohortExcluded,
    FullRequired,
    AuditDue,
    Proof,
    ProofFallback,
    ProofError,
    ConflictReuse,
}

/// Counts verification decisions per transaction attempt, not completed work.
/// Keep capacity markers distinct from ordinary repeated-failure publications.
pub(in super::super) fn settlement(caller: ScanCaller, decision: SettlementRead) {
    let ScanCaller::CapacitySettlement(step) = caller else {
        return;
    };
    let decision = match decision {
        SettlementRead::FlagDisabled => "flag_disabled",
        SettlementRead::CohortExcluded => "cohort_excluded",
        SettlementRead::FullRequired => "full_required",
        SettlementRead::AuditDue => "audit_due",
        SettlementRead::Proof => "proof",
        SettlementRead::ProofFallback => "proof_fallback",
        SettlementRead::ProofError => "proof_error",
        SettlementRead::ConflictReuse => "conflict_reuse",
    };
    metrics::counter!("cma_agent_capacity_settlement_reads_total",
        "worker_step" => step.operation_name(), "decision" => decision)
    .increment(1);
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
    fn settlement_decisions_are_fixed_labels_and_exclude_other_callers() {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let metrics = recorder.handle();
        let observation = metrics::set_default_local_recorder(&recorder);
        for (decision, label) in [
            (SettlementRead::FlagDisabled, "flag_disabled"),
            (SettlementRead::CohortExcluded, "cohort_excluded"),
            (SettlementRead::FullRequired, "full_required"),
            (SettlementRead::AuditDue, "audit_due"),
            (SettlementRead::Proof, "proof"),
            (SettlementRead::ProofFallback, "proof_fallback"),
            (SettlementRead::ProofError, "proof_error"),
            (SettlementRead::ConflictReuse, "conflict_reuse"),
        ] {
            settlement(
                ScanCaller::CapacitySettlement(AgentWorkStep::CloseProcess),
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
        settlement(
            ScanCaller::WorkerPublication(AgentWorkStep::CloseProcess),
            SettlementRead::Proof,
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
