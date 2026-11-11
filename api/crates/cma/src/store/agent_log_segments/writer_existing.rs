//! Publication and exact receipt for original callers on an adopted Agent.
use super::*;
use crate::store::agent_log_segments::{
    capture::{self, CapturedFormat},
    writer_admission::{self, AdmissionKind},
    writer_plan, writer_reconcile,
};

#[derive(Clone)]
pub(in crate::store) struct Recorded {
    pub operation: PublicationIdentity,
    pub publication: Publication,
    pub next: Agent,
    pub logs: Vec<AgentLogEntry>,
    pub latest_before: Option<AgentLogEntry>,
}

impl Recorded {
    pub async fn matches(
        &self,
        store: &MongoSandboxStore,
        session: &mut mongodb::ClientSession,
    ) -> Result<bool, SandboxStoreError> {
        Ok(matches!(writer_reconcile::reconcile(
            store, &self.operation, &self.next, &self.logs,
            &self.publication.result_digest, session,
        ).await?, writer_reconcile::Outcome::Recorded(actual) if *actual == self.publication))
    }
}

/// Domain validation and other aggregate writes may follow scope release. Any
/// failure must abort this same transaction; the root alone commits it.
pub(in crate::store) async fn publish_existing(
    store: &MongoSandboxStore,
    scope: &mut TransactionScope<'_>,
    input: PublicationInput<'_>,
    aggregate: AggregateUpdate,
) -> Result<Recorded, TransactionLogError> {
    let captured = capture::capture_transaction(store, input.current, scope).await?;
    if captured.format() != CapturedFormat::Segmented {
        return Err(SandboxStoreError::Conflict.into());
    }
    // Rollover admission is serialized with the gate and consumed in this same
    // transaction. Existing-segment appends require no new activation admission.
    let admission = if input.capacity == crate::store::agent_log_integrity::AppendCapacity::Normal
        && writer_plan::needs_normal_rollover(captured.active_head(), input.logs)?
    {
        Some(
            writer_admission::admit(
                &store.client.database(&store.agents.namespace().db),
                input.current,
                input.operation,
                AdmissionKind::Rollover,
                &layer2_core::now_rfc3339(),
                scope.session(),
            )
            .await?,
        )
    } else {
        None
    };
    let operation = input.operation.clone();
    let next = input.next.clone();
    let logs = input.logs.to_vec();
    let latest_before = captured.latest_entry().cloned();
    let plan = prepare(store, scope, &captured, input, None, admission.as_ref()).await?;
    let publication = persist(store, scope, plan, aggregate).await?;
    Ok(Recorded {
        operation,
        publication,
        next,
        logs,
        latest_before,
    })
}
