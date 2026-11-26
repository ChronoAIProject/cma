//! Durable keyed append. Admission/staging precede the caller's publication
//! transaction; an unknown publication outcome never starts another append.
use super::*;
use crate::store::agent_log_segments::{
    self as segments, capture,
    publication::{self, Publication, PublicationIdentity},
    transaction_scope::TransactionScope,
    writer_admission::{Admission, AdmissionKind},
    writer_bootstrap::BootstrapPlan,
    writer_commit::{self, AggregateUpdate, PublicationInput},
    writer_plan, writer_reconcile,
};
use layer2_core::sandbox::{store::AgentLogPublicationIdentity, Agent};
use mongodb::bson::{spec::BinarySubtype, Binary};

mod claim;
pub(in crate::store) mod preparation;
pub(in crate::store) mod settlement;

pub(in crate::store) struct Attempt {
    pub(in crate::store) next: Agent,
    pub(in crate::store) publication: Publication,
}

pub(super) async fn append(
    store: &MongoSandboxStore,
    id: &AgentId,
    identity: &AgentLogPublicationIdentity,
    kind: AgentLogKind,
) -> Result<AgentLogEntry, SandboxStoreError> {
    let operation = PublicationIdentity {
        key: identity.key.clone(),
        fingerprint: Binary {
            subtype: BinarySubtype::Generic,
            bytes: identity.fingerprint.to_vec(),
        },
        appended_at: identity.appended_at.clone(),
    };
    operation.validate()?;
    let _writer = store.agent_writes.lock(id).await;
    for _ in 0..TRANSACTION_ATTEMPTS {
        if let Some(entry) = writer_reconcile::replay(store, id, &operation, &kind).await? {
            return Ok(entry);
        }
        claim::ensure_clear(store, id).await?;
        let (current, entry, admission_kind) = source(store, id, &operation, &kind).await?;
        let (admission, bootstrap) =
            preparation::prepare(store, &current, &operation, admission_kind).await?;
        let mut session = match store.client.start_session().await {
            Ok(session) => session,
            Err(error) if is_transient(&error) => continue,
            Err(error) => return Err(driver(error)),
        };
        // Starting may fence work and establish the transaction snapshot. The
        // independently durable marker must exist before that snapshot begins.
        let claim = claim::Claim::acquire(store, &current, &operation, &entry).await?;
        let mut scope = match TransactionScope::start(&mut session).await {
            Ok(scope) => scope,
            Err(error) if is_transient(&error) => {
                let _ = session.abort_guarded().await;
                claim.release_known_abort(store).await?;
                drop(session);
                tokio::task::yield_now().await;
                continue;
            }
            Err(error) => {
                // A typed start refusal precedes consumption and publication;
                // no commit or protected effect can have been sent. Unknown
                // driver results still retain this exact durable claim.
                if !error.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT) {
                    if let Some(refusal) = error.get_custom::<SandboxStoreError>().cloned() {
                        let _ = session.abort_guarded().await;
                        claim.release_known_abort(store).await?;
                        return Err(refusal);
                    }
                }
                return Err(driver(error));
            }
        };
        let result = async {
            claim.consume(store, &mut scope).await?;
            publish(
                store,
                &mut scope,
                &current,
                &entry,
                &operation,
                admission.as_ref(),
                bootstrap.as_ref(),
            )
            .await
        }
        .await;
        let attempted = match result {
            Ok(attempted) => attempted,
            Err(error) => {
                if matches!(&error, TransactionLogError::Driver(error)
                    if error.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT))
                {
                    let _ = scope.release().abort_guarded().await;
                    return Err(domain(error));
                }
                scope.release().abort_guarded().await.map_err(driver)?;
                claim.release_known_abort(store).await?;
                if let Some(entry) = writer_reconcile::replay(store, id, &operation, &kind).await? {
                    return Ok(entry);
                }
                if matches!(
                    &error,
                    TransactionLogError::Domain(SandboxStoreError::Conflict)
                ) || matches!(&error, TransactionLogError::Driver(error) if is_transient(error))
                {
                    tokio::task::yield_now().await;
                    continue;
                }
                return Err(domain(error));
            }
        };
        // All captured/prepared capabilities are consumed before raw-session
        // domain work. Any domain error still aborts the entire transaction.
        let session = scope.release();
        if matches!(entry.kind, AgentLogKind::TurnCompleted { .. }) {
            if let Err(error) = crate::store::suggestions::enqueue(
                store,
                &attempted.next,
                std::slice::from_ref(&entry),
                session,
            )
            .await
            {
                let _ = session.abort_guarded().await;
                if matches!(&error, TransactionLogError::Driver(error)
                    if error.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT))
                {
                    return Err(domain(error));
                }
                claim.release_known_abort(store).await?;
                if matches!(&error, TransactionLogError::Driver(error) if is_transient(error)) {
                    tokio::task::yield_now().await;
                    continue;
                }
                return Err(domain(error));
            }
        }
        let outcome =
            match crate::store::sandbox_mongo::commit_transaction_with_refusal(session).await {
                Ok(outcome) => outcome,
                Err(crate::store::sandbox_mongo::PrecommitRefusal::Aborted(refusal)) => {
                    claim.release_known_abort(store).await?;
                    return Err(refusal);
                }
                Err(crate::store::sandbox_mongo::PrecommitRefusal::AbortUnacknowledged(error)) => {
                    return Err(driver(error));
                }
            };
        match outcome {
            CommitOutcome::Committed => return Ok(entry),
            CommitOutcome::Retry(_) => {
                // TransientTransactionError positively establishes KnownAbort.
                let _ = session.abort_guarded().await;
                claim.release_known_abort(store).await?;
            }
            CommitOutcome::Unknown(_) => {
                return settlement::after_unknown(
                    store,
                    &operation,
                    &entry,
                    &attempted,
                    session.operation_time(),
                )
                .await;
            }
            CommitOutcome::Failed(error) => return Err(driver(error)),
        }
    }
    Err(SandboxStoreError::Unavailable(
        "keyed Agent log publication exhausted its known-abort retry budget".into(),
    ))
}

async fn source(
    store: &MongoSandboxStore,
    id: &AgentId,
    operation: &PublicationIdentity,
    kind: &AgentLogKind,
) -> Result<(Agent, AgentLogEntry, Option<AdmissionKind>), SandboxStoreError> {
    let mut snapshot = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(driver)?;
    let row = store
        .agents
        .find_one(doc! {"_id":id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut snapshot)
        .await
        .map_err(driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    let current = read_agent(&row)?;
    let entry = AgentLogEntry {
        agent_id: id.clone(),
        seq: current
            .conversation
            .last_seq
            .checked_add(1)
            .ok_or(SandboxStoreError::SequenceConflict)?,
        appended_at: operation.appended_at.clone(),
        kind: kind.clone(),
    };
    let admission = if segments::read_format(&row)?.is_some() {
        let head = segments::read_head(store, &row, &mut snapshot, true)
            .await
            .map_err(domain)?;
        writer_plan::needs_normal_rollover(&head, std::slice::from_ref(&entry))?
            .then_some(AdmissionKind::Rollover)
    } else {
        Some(AdmissionKind::Activation)
    };
    Ok((current, entry, admission))
}

async fn publish(
    store: &MongoSandboxStore,
    scope: &mut TransactionScope<'_>,
    current: &Agent,
    entry: &AgentLogEntry,
    operation: &PublicationIdentity,
    admission: Option<&Admission>,
    bootstrap: Option<&BootstrapPlan>,
) -> Result<Attempt, TransactionLogError> {
    let row = scope
        .find_one(&store.agents, doc! {"_id":current.id.as_str()})
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    if read_agent(&row)? != *current {
        return Err(SandboxStoreError::Conflict.into());
    }
    let captured = capture::capture_transaction(store, current, scope).await?;
    let mut next = current.clone();
    next.conversation.last_seq = entry.seq;
    let plan = writer_commit::prepare(
        store,
        scope,
        &captured,
        PublicationInput {
            operation,
            current,
            next: &next,
            logs: std::slice::from_ref(entry),
            capacity: AppendCapacity::Normal,
            result_digest: publication::result_digest(entry)?,
        },
        bootstrap,
        admission,
    )
    .await?;
    let publication = writer_commit::persist(
        store,
        scope,
        plan,
        AggregateUpdate {
            filter: doc! {"_id":current.id.as_str()},
            fields: crate::store::agents::agent_replacement_fields(&next)?,
        },
    )
    .await?;
    Ok(Attempt { next, publication })
}

fn driver(_: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable("keyed Agent log publication storage is unavailable".into())
}
fn domain(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => driver(error),
    }
}
