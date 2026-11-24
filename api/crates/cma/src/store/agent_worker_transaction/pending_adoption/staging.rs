//! A short transaction admits the witness before invisible bootstrap staging.
use super::super::super::sandbox_mongo::start_transaction;
use super::*;
use crate::store::agent_log_segments::{writer_admission, writer_bootstrap, writer_gate::slots};
use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::bson::DateTime;

pub(in crate::store::agent_worker_transaction) async fn prepare(
    store: &MongoSandboxStore,
    source: Source,
) -> Result<Option<Prepared>, SandboxStoreError> {
    #[cfg(test)]
    test_hooks::stop(
        &store.agents.namespace().db,
        test_hooks::Boundary::BeforeAdmission,
    )
    .await;
    let mut session = store.client.start_session().await.map_err(driver)?;
    start_transaction(&mut session).await.map_err(driver)?;
    let result = admit(store, &source, &mut session).await;
    let (admission, permit, fresh) = match result {
        Ok(Some(result)) => result,
        Ok(None) => {
            session.abort_guarded().await.map_err(driver)?;
            cohort(Stage::Blocked, Outcome::KnownAbort);
            return Ok(None);
        }
        Err(error) => {
            if let Err(abort) = session.abort_guarded().await {
                tracing::debug!(error_kind = ?abort.kind, "Aborting pending publication staging failed");
            }
            return Err(domain(error));
        }
    };
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => {
            let outcome = if fresh {
                Outcome::Committed
            } else {
                Outcome::Replay
            };
            cohort(Stage::Admitted, outcome);
        }
        CommitOutcome::Unknown(_) => {
            cohort(Stage::Blocked, Outcome::Unknown);
            return Err(witness::unavailable());
        }
        CommitOutcome::Retry(_) => {
            cohort(Stage::Blocked, Outcome::KnownAbort);
            return Err(witness::unavailable());
        }
        CommitOutcome::Failed(error) => return Err(driver(error)),
    }
    #[cfg(test)]
    test_hooks::stop(&store.agents.namespace().db, test_hooks::Boundary::Admitted).await;
    let operation = source.witness.operation();
    let bootstrap = writer_bootstrap::stage(store, &source.current, &operation, &admission, permit)
        .await
        .map_err(|error| domain(error.into()))?;
    Ok(Some(Prepared {
        witness: source.witness,
        admission,
        bootstrap,
    }))
}

async fn admit(
    store: &MongoSandboxStore,
    source: &Source,
    session: &mut mongodb::ClientSession,
) -> Result<Option<(Admission, slots::Permit, bool)>, ApplyError> {
    let current = &source.current;
    let database = store.client.database(&store.agents.namespace().db);
    let witness = &source.witness;
    let operation = witness.operation();
    let expected = witness.document()?;
    let row = store
        .agents
        .find_one(doc! {"_id": current.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(witness::unresolved)?;
    if read_agent(&row).map_err(ApplyError::Domain)? != *current {
        return Err(witness::unresolved());
    }
    let previous = Witness::read(&row)?;
    let now = layer2_core::now_rfc3339();
    let lease = current
        .work
        .lease
        .as_ref()
        .ok_or_else(witness::unresolved)?;
    if lease.expires_at <= now {
        return Err(witness::unresolved());
    }
    let (admission, fresh) = if previous.as_ref() == Some(witness) {
        let admission = writer_admission::read(&database, current, &operation, session)
            .await?
            .ok_or_else(witness::unresolved)?;
        admission
            .permits(current, &operation, AdmissionKind::Activation)
            .map_err(ApplyError::Domain)?;
        (admission, false)
    } else if previous == source.prior_retired {
        if !store.segment_adoption_enabled {
            return Err(witness::unresolved());
        }
        if !writer_gate::selects_pending_worker(&database, current, session).await? {
            if previous.is_some() {
                return Err(witness::unresolved());
            }
            return Ok(None);
        }
        let admission =
            writer_admission::admit_pending_worker(&database, current, &operation, &now, session)
                .await?;
        let marker = match previous {
            Some(ref retired) if retired.permits_replacement(current) => retired.document()?,
            Some(_) => return Err(witness::unresolved()),
            None => doc! {"$exists":false},
        };
        let result = store.agents.update_one(doc! {
            "_id": current.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": signed(current.revision, "pending publication revision").map_err(|_| witness::unresolved())?,
            "last_seq": signed(current.conversation.last_seq, "pending publication sequence").map_err(|_| witness::unresolved())?,
            "work_lease_holder": &lease.holder,
            "work_lease_fence": signed(lease.fence, "pending publication fence").map_err(|_| witness::unresolved())?,
            "work_lease_expires_at": &lease.expires_at,
            witness::FIELD: marker,
        }, doc! {"$set":{witness::FIELD:expected}})
            .session(&mut *session).await.map_err(ApplyError::Driver)?;
        if result.matched_count != 1 {
            return Err(witness::unresolved());
        }
        (admission, true)
    } else {
        return Err(witness::unresolved());
    };
    let permit = slots::claim(
        &database,
        current,
        &operation,
        &admission,
        DateTime::now(),
        session,
    )
    .await?
    .ok_or_else(witness::unresolved)?;
    Ok(Some((admission, permit, fresh)))
}

fn driver(_: mongodb::error::Error) -> SandboxStoreError {
    witness::unavailable()
}

fn domain(error: ApplyError) -> SandboxStoreError {
    match error {
        ApplyError::Domain(error) => error,
        ApplyError::Driver(error) => driver(error),
        _ => witness::unavailable(),
    }
}
