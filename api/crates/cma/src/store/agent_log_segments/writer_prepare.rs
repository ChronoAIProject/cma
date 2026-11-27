//! Assemble a publication only after the caller's capture context is certified.
use super::*;
use crate::store::agent_log_segments::{
    capture::CapturedFormat,
    collections::collection,
    corrupt, digest, projection, source_keys,
    writer_admission::{self, AdmissionKind},
    writer_plan::{self, RolloverPermission},
    writer_projection, EVIDENCE, PUBLICATIONS,
};
use mongodb::{
    bson::{self, doc},
    ClientSession,
};

pub(super) struct EvidenceWrite {
    pub before: Option<Document>,
    pub after: Document,
}

pub(super) async fn assemble(
    store: &MongoSandboxStore,
    scope: &mut TransactionScope<'_>,
    captured: &CapturedLog,
    input: PublicationInput<'_>,
    bootstrap: Option<&BootstrapPlan>,
    admission: Option<&Admission>,
) -> Result<PreparedPublication, TransactionLogError> {
    captured.validate_transaction(store, scope)?;
    let transaction = scope.identity();
    input.operation.validate()?;
    crate::store::agent_log_integrity::ensure_append_batch_size(input.logs.len())?;
    let current = input.current;
    let next = input.next;
    if current != captured.agent()
        || current.id != next.id
        || digest::incarnation(current) != captured.incarnation()
        || digest::incarnation(current) != digest::incarnation(next)
        || current.creation_fingerprint != next.creation_fingerprint
        || current.creation_worktree_id() != next.creation_worktree_id()
        || current.owner != next.owner
        || current.sandbox_id != next.sandbox_id
        || current.workspace_id != next.workspace_id
        || next.revision < current.revision
        || current
            .conversation
            .last_seq
            .checked_add(input.logs.len() as u64)
            != Some(next.conversation.last_seq)
        || captured.projection().max_opened_generation != captured.max_opened_generation()
        || captured.projection().max_retired_generation != captured.max_retired_generation()
        || input
            .logs
            .iter()
            .any(|entry| entry.appended_at != input.operation.appended_at)
    {
        return Err(corrupt().into());
    }
    let publications = collection(store, PUBLICATIONS);
    if let Some(row) = scope
        .find_one(
            &publications,
            doc! {"_id":super::super::publication::identity(
                current.id.as_str(), captured.incarnation(), &input.operation.key,
            )},
        )
        .await
        .map_err(TransactionLogError::Driver)?
    {
        Publication::read(row)?.matches_request(current, input.operation)?;
        return Err(SandboxStoreError::Unavailable(
            "Agent log publication is already recorded; reconcile the original result".into(),
        )
        .into());
    }
    let source = match captured.format() {
        CapturedFormat::Legacy => SourceFormat::Legacy,
        CapturedFormat::Segmented => SourceFormat::Segmented,
    };
    let stored_admission = if let Some(supplied) = admission {
        let database = store.client.database(&store.agents.namespace().db);
        let stored = writer_admission::read(&database, current, input.operation, scope.session())
            .await?
            .ok_or(SandboxStoreError::Conflict)?;
        if bson::serialize_to_document(&stored).map_err(|_| corrupt())?
            != bson::serialize_to_document(supplied).map_err(|_| corrupt())?
        {
            return Err(SandboxStoreError::Conflict.into());
        }
        stored.permits(current, input.operation, stored.kind)?;
        Some(stored)
    } else {
        None
    };
    let (before, permit, permission) = match source {
        SourceFormat::Legacy => {
            let admitted = stored_admission.as_ref().ok_or_else(closed)?;
            let permission = match admitted.kind {
                AdmissionKind::Activation => RolloverPermission::Normal,
                AdmissionKind::CapacityRecovery => {
                    if !input.logs.is_empty()
                        || next.conversation != current.conversation
                        || next.worktree_id != current.worktree_id
                        || next.status == layer2_core::sandbox::AgentStatus::Ready
                    {
                        return Err(SandboxStoreError::InvalidTransaction(
                            "storage recovery must preserve conversation state and remain non-ready",
                        ).into());
                    }
                    RolloverPermission::LegacyRecovery
                }
                AdmissionKind::Rollover => return Err(SandboxStoreError::Conflict.into()),
            };
            let (head, permit) = bootstrap.ok_or_else(closed)?.publication_parts(
                captured,
                input.operation,
                admitted,
            )?;
            // The first segmented head must satisfy the same source-key
            // checks as its readers, even if the catalog changed after admission.
            let database = store.client.database(&store.agents.namespace().db);
            source_keys::prove(&database, &head, scope.session(), false).await?;
            (head, Some(permit), permission)
        }
        SourceFormat::Segmented => {
            if bootstrap.is_some() {
                return Err(SandboxStoreError::Conflict.into());
            }
            let permission = match stored_admission.as_ref().map(|admission| admission.kind) {
                Some(AdmissionKind::Rollover) => RolloverPermission::Normal,
                Some(AdmissionKind::CapacityRecovery) => {
                    if !input.logs.is_empty()
                        || next.conversation != current.conversation
                        || next.worktree_id != current.worktree_id
                        || next.status == layer2_core::sandbox::AgentStatus::Ready
                    {
                        return Err(SandboxStoreError::InvalidTransaction(
                            "segmented storage recovery must preserve conversation state and remain non-ready",
                        )
                        .into());
                    }
                    RolloverPermission::SegmentedRecovery
                }
                Some(AdmissionKind::Activation) => return Err(SandboxStoreError::Conflict.into()),
                None => RolloverPermission::Closed,
            };
            (captured.active_head().clone(), None, permission)
        }
    };
    certify_queued_sources(store, scope, captured, &before, input.logs).await?;
    let batch = EncodedBatch::new(&before, input.logs)?;
    let plan = writer_plan::plan(
        &before,
        captured.active_leaf_digests().clone(),
        &batch,
        input.capacity,
        permission,
        source,
        &input.operation.key,
    )?;
    let mut delta = writer_projection::plan(
        current,
        next,
        &before,
        &plan.head,
        captured.projection(),
        input.logs,
        source,
    )?;
    let receipt = Publication::new(
        current,
        next,
        &plan.head,
        input.operation,
        input.result_digest,
    )?;
    let mut evidence = certify_evidence(
        store,
        scope.session(),
        &before,
        captured,
        std::mem::take(&mut delta.evidence),
    )
    .await?;
    evidence.extend(
        super::output::plan(store, scope, captured, &before, &plan.head, input.logs).await?,
    );
    let active_proof = if store.active_log_proofs {
        Some(super::super::active_proof::ActiveProof::successor(
            &before,
            current.revision,
            next.revision,
            &plan,
            captured.active_leaf_digests(),
            batch.rows(),
        )?)
    } else {
        None
    };
    Ok(PreparedPublication {
        transaction,
        database: store.agents.namespace().db,
        current: current.clone(),
        next: next.clone(),
        source,
        before,
        batch,
        plan,
        active_proof,
        delta,
        evidence,
        receipt,
        admission: stored_admission,
        permit,
    })
}

async fn certify_queued_sources(
    store: &MongoSandboxStore,
    scope: &mut TransactionScope<'_>,
    captured: &CapturedLog,
    head: &Head,
    logs: &[AgentLogEntry],
) -> Result<(), TransactionLogError> {
    use layer2_core::sandbox::AgentLogProjectionState;
    let sources = logs
        .iter()
        .filter_map(AgentLogProjectionState::queued_owner)
        .filter(|seq| *seq <= captured.through())
        .collect::<std::collections::BTreeSet<_>>();
    for sequence in sources {
        let entries =
            super::super::capture::points_transaction(store, captured, &[sequence], scope).await?;
        let entry = entries.first().ok_or_else(corrupt)?;
        captured
            .projection()
            .validate_queued_source(entry)
            .map_err(SandboxStoreError::Corrupt)?;
        match projection::evidence(
            store,
            head,
            "queued",
            &sequence.to_string(),
            scope.session(),
        )
        .await?
        {
            Some(projection::EvidenceRecord::Queued {
                sequence: stored,
                consumed_by: None,
            }) if stored == sequence => {}
            _ => return Err(corrupt().into()),
        }
    }
    Ok(())
}

async fn certify_evidence(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    head: &Head,
    captured: &CapturedLog,
    changes: Vec<Document>,
) -> Result<Vec<EvidenceWrite>, TransactionLogError> {
    let collection = collection(store, EVIDENCE);
    let mut writes = Vec::with_capacity(changes.len());
    for after in changes {
        let id = after.get_str("_id").map_err(|_| corrupt())?;
        let kind = after.get_str("kind").map_err(|_| corrupt())?;
        let key = after.get_str("entity_key").map_err(|_| corrupt())?;
        let before = crate::store::stale_turns::find_with_proof_budget(
            &collection,
            doc! {"_id":id},
            &mut *session,
            false,
        )
        .await
        .map_err(TransactionLogError::Driver)?;
        let record = before
            .as_ref()
            .map(|row| projection::read_evidence(head, kind, key, row.clone()))
            .transpose()?;
        if kind == "turn" {
            let sequence = key.parse::<u64>().map_err(|_| corrupt())?;
            let expected = captured
                .projection()
                .active
                .as_ref()
                .into_iter()
                .chain(captured.projection().latest_completed.as_ref())
                .find(|facts| facts.turn_sequence == sequence);
            match (expected, record.as_ref()) {
                (Some(expected), Some(projection::EvidenceRecord::Turn { facts }))
                    if expected == facts.as_ref() => {}
                (None, None) if sequence > head.last_seq as u64 => {}
                _ => return Err(corrupt().into()),
            }
        }
        writes.push(EvidenceWrite { before, after });
    }
    Ok(writes)
}

fn closed() -> SandboxStoreError {
    SandboxStoreError::Unavailable(
        "Agent log adoption requires its original admission and complete staged proof".into(),
    )
}
