//! Same-snapshot proofs, periodic audits, and invocation-local conflict reuse.
use super::retry_audit::telemetry::{self, CloseRetry, ReadDecision, Verification};
use super::*;
use crate::store::agent_log_integrity::{
    trusted_summary, AppendCapacity, LogSummary, INTEGRITY_FIELD, NORMAL_AGENT_LOG_ENTRIES,
    QUARANTINE_FIELD,
};
use layer2_core::sandbox::store::validate_agent_repeated_failure;
use layer2_core::sandbox::AgentLogKind;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;

pub(super) struct History {
    pub(super) summary: LogSummary,
    pub(super) entries: Vec<AgentLogEntry>,
    pub(super) last: Option<AgentLogEntry>,
    audit_at: Option<Instant>,
    audit_recorded: AtomicBool,
}

impl History {
    pub(super) fn audited(&self, current: &Agent) {
        // Conflict reuse can revisit semantic validation for this same scan.
        // Count its completed audit once, independently of commit certainty.
        if self.audit_at.is_some() && !self.audit_recorded.swap(true, Ordering::Relaxed) {
            telemetry::record(current.work.step, Verification::CompletedFull);
        }
    }
}

#[derive(Default)]
pub(super) struct AttemptHistory {
    validated: Option<(Agent, Vec<u8>, Arc<History>)>,
}

impl AttemptHistory {
    pub(super) fn committed(&self, store: &MongoSandboxStore) {
        if let Some((agent, _, history)) = &self.validated {
            if let Some(at) = history.audit_at {
                store.retry_audits.record(agent, at);
            }
        }
    }
}

pub(super) struct Selection<'a> {
    pub ordinary: bool,
    pub repeated_marker: Option<&'a AgentLogEntry>,
}

impl MongoSandboxStore {
    pub(crate) fn with_retry_single_marker(mut self, enabled: bool) -> Self {
        self.retry_single_marker_enabled = enabled;
        self
    }

    pub(crate) fn with_bounded_close_retries(mut self, enabled: bool) -> Self {
        self.bounded_close_retries = enabled;
        CloseRetry::initialize(enabled);
        self
    }
}

pub(super) async fn read(
    store: &MongoSandboxStore,
    current: &Agent,
    row: &Document,
    mutation: &AgentWorkMutation,
    selection: Selection<'_>,
    cache: &mut AttemptHistory,
    session: &mut mongodb::ClientSession,
) -> Result<Arc<History>, ApplyError> {
    let repeated = selection.repeated_marker.is_some();
    let needs_marker = agent_cleanup_capacity_retry(current, mutation) || repeated;
    let repeat = needs_marker && mutation.agent_logs.is_empty();
    let caller = if needs_marker && !repeated
        || crate::store::agent_worker_capacity::capacity_failure_evidence_matches(current, mutation)
    {
        ScanCaller::CapacitySettlement(current.work.step)
    } else {
        ScanCaller::WorkerPublication(current.work.step)
    };
    let ordinary_append =
        selection.ordinary && !store.retry_single_marker_enabled && eligible(current, mutation);
    // Compare proposals before the switch and audit clock affect verification.
    // The retained marker still has to match in the existing snapshot check.
    let eligible_close_retry = selection.ordinary
        && caller == ScanCaller::WorkerPublication(AgentWorkStep::CloseProcess)
        && (ordinary_append
            || selection.repeated_marker.is_some_and(|marker| {
                validate_agent_repeated_failure(current, mutation, marker, Some(marker)).is_ok()
            }));
    let selected = selection.ordinary
        && (eligible_close_retry || store.bounded_close_retries)
        && store.bounded_log_enabled(&current.id);
    let close_retry =
        eligible_close_retry.then(|| CloseRetry::observe(store.bounded_close_retries, selected));
    let bounded = store.bounded_close_retries && selected;
    let key = bounded.then(|| snapshot_key(row)).transpose()?;
    if let Some(key) = &key {
        if let Some((agent, proof, history)) = &cache.validated {
            if agent == current && proof == key {
                if repeat {
                    telemetry::record(current.work.step, Verification::SkippedConflictReuse);
                }
                telemetry::read(caller, close_retry, ReadDecision::ConflictReuse);
                tracing::debug!(
                    step = current.work.step.operation_name(),
                    "Reused Agent history after aborted publication"
                );
                return Ok(Arc::clone(history));
            }
        }
    }
    cache.validated = None;
    let work = if matches!(caller, ScanCaller::CapacitySettlement(_)) {
        "capacity_settlement"
    } else {
        "worker_publication"
    };
    let history = layer2_core::diagnostic_context::DiagnosticContext::current()
        .with_worker(work)
        .with_phase(current.work.step.operation_name())
        .scope(async {
            if bounded && (ordinary_append || repeat && store.retry_audits.recent(current)) {
                let proof =
                    Box::pin(trusted_summary(store, current, row, caller, session, false)).await;
                if proof.is_err() {
                    telemetry::read(caller, close_retry, ReadDecision::ProofError);
                }
                if let Some(summary) = proof? {
                    let last = if needs_marker {
                        let result = last_entry(store, current, session).await;
                        if let (Err(_), Some(retry)) = (&result, close_retry) {
                            retry.verification(ReadDecision::ProofError);
                        }
                        result?
                    } else {
                        None
                    };
                    if repeat {
                        telemetry::record(current.work.step, Verification::SkippedProof);
                    }
                    let decision = if close_retry.is_some()
                        && repeated
                        && last.as_ref() != selection.repeated_marker
                    {
                        ReadDecision::MarkerConflict
                    } else {
                        ReadDecision::Proof
                    };
                    telemetry::read(caller, close_retry, decision);
                    return Ok::<_, ApplyError>(History {
                        summary,
                        entries: vec![],
                        last,
                        audit_at: None,
                        audit_recorded: AtomicBool::new(false),
                    });
                }
                if repeat {
                    telemetry::fallback(current, row).map_err(ApplyError::Domain)?;
                }
                telemetry::read(caller, close_retry, ReadDecision::ProofFallback);
            } else {
                telemetry::read(
                    caller,
                    close_retry,
                    if !selection.ordinary || !repeat && !ordinary_append {
                        ReadDecision::FullRequired
                    } else if !store.bounded_close_retries {
                        ReadDecision::FlagDisabled
                    } else if !selected {
                        ReadDecision::CohortExcluded
                    } else {
                        ReadDecision::AuditDue
                    },
                );
            }
            // Audit age is process-local, so restart and eviction force a complete
            // verification. First failures, effects and untrusted proofs retain it.
            let audit_at = repeat.then(Instant::now);
            let mut entries = Vec::new();
            let mut last = None;
            let summary = Box::pin(scan_complete_log_with_session(
                caller,
                store,
                &current.id,
                current.conversation.last_seq,
                session,
                |entry| {
                    entries.push(entry.clone());
                    if needs_marker && entry.seq == current.conversation.last_seq {
                        last = Some(entry);
                    }
                    Ok(())
                },
            ))
            .await?;
            Ok(History {
                summary,
                entries,
                last,
                audit_at,
                audit_recorded: AtomicBool::new(false),
            })
        })
        .await?;
    let history = Arc::new(history);
    if let Some(key) = key {
        cache.validated = Some((current.clone(), key, Arc::clone(&history)));
    }
    Ok(history)
}

fn eligible(current: &Agent, mutation: &AgentWorkMutation) -> bool {
    // This is selection, never mutation authority. The unchanged publication
    // validator and Agent CAS still fence lease, revision, work and identity.
    current.work.step == AgentWorkStep::CloseProcess
        && mutation.agent.work.step == AgentWorkStep::CloseProcess
        && current.status == mutation.agent.status
        && current.conversation.last_seq < NORMAL_AGENT_LOG_ENTRIES
        && mutation.worktree.is_none()
        && mutation.sandbox_event.is_none()
        && publication_capacity(current, mutation) == AppendCapacity::Normal
        && matches!(
            mutation.agent_logs.as_slice(),
            [AgentLogKind::WorkStepFailed {
                step: AgentWorkStep::CloseProcess,
                ..
            }]
        )
}

fn snapshot_key(row: &Document) -> Result<Vec<u8>, ApplyError> {
    let mut proof = Document::new();
    for field in [INTEGRITY_FIELD, QUARANTINE_FIELD] {
        // Absence and BSON null are distinct: even a null quarantine blocks trust.
        if let Some(value) = row.get(field) {
            proof.insert(field, value.clone());
        }
    }
    proof
        .to_vec()
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))
}

async fn last_entry(
    store: &MongoSandboxStore,
    current: &Agent,
    session: &mut mongodb::ClientSession,
) -> Result<Option<AgentLogEntry>, ApplyError> {
    let mut read = crate::store::mongo_read_telemetry::Read::find(store.agent_log.name());
    let raw = store.agent_log.clone_with_type::<mongodb::bson::RawDocumentBuf>()
        .find_one(doc! {"_id": super::super::agent_log::log_id(&current.id, current.conversation.last_seq)})
        .session(session).await.map_err(ApplyError::Driver)?;
    if let Some(raw) = &raw {
        read.document(raw.as_bytes().len());
    }
    read.complete();
    raw.as_ref()
        .map(|raw| {
            let row = Document::try_from(raw).map_err(|error| {
                ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string()))
            })?;
            super::super::agent_log::read_entry(&row).map_err(ApplyError::Domain)
        })
        .transpose()
}
