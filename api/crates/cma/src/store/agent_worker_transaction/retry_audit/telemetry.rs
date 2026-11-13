//! Bounded retry-verification labels; no Agent identities or failure contents.
use crate::store::agent_log_integrity::{parse_integrity, INTEGRITY_FIELD};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, AgentWorkStep};
use mongodb::bson::Document;

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
