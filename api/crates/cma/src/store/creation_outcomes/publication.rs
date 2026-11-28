use super::*;
use layer2_core::sandbox::{store::SandboxAdmission, SandboxOrigin};
use mongodb::bson::Bson;

const LIFECYCLES: &str = super::super::agent_lifecycle_operations::COLLECTION;

fn pending(owner: &str, fence: &CreationFence) -> Document {
    doc! {"_id": &fence.record_id, "document_schema_version": VERSION, "owner": owner,
    "generation": &fence.generation, "epoch": &fence.epoch, "state.kind": "pending"}
}

fn live_direct(mut filter: Document) -> Document {
    filter.insert("operation_id", doc! {"$exists": false});
    filter.insert("$expr", doc! {"$gt": ["$lease_until", "$$NOW"]});
    filter
}

pub(in crate::store) async fn link_operation(
    database: &Database,
    owner: &str,
    fence: &CreationFence,
    id: &str,
    session: &mut ClientSession,
) -> Result<bool, mongodb::error::Error> {
    let mut filter = live_direct(pending(owner, fence));
    filter.insert("operation", "agent_bootstrap");
    database
        .collection::<Document>(COLLECTION)
        .update_one(filter, doc! {"$set":{"operation_id":id}})
        .session(session)
        .await
        .map(|result| result.matched_count == 1)
}

fn live_operation(owner: &str, id: &str, token: &str) -> Document {
    doc! {"_id": id, "owner": owner, "status":"running", "lease_token":token, "document_schema_version": schema::DOCUMENT_SCHEMA_VERSION,
    "$expr":{"$gt":["$lease_expires_at","$$NOW"]}}
}

pub(in crate::store) async fn publish_sandbox(
    database: &Database,
    admission: &SandboxAdmission,
    sandbox_id: &str,
    session: &mut ClientSession,
) -> Result<bool, mongodb::error::Error> {
    let Some(fence) = &admission.creation_fence else {
        return Ok(true);
    };
    let owner = admission.sandbox.owner.as_str();
    let mut filter = pending(owner, fence);
    match (&admission.sandbox.origin, &fence.lifecycle_lease) {
        (SandboxOrigin::Manual, None) => {
            filter = live_direct(filter);
            filter.insert("operation", "sandbox");
        }
        (SandboxOrigin::Lifecycle { operation_id }, Some((id, lease))) if operation_id == id => {
            // A write to the operation fences lease takeover and terminal refusal.
            let fenced = database
                .collection::<Document>(LIFECYCLES)
                .update_one(
                    live_operation(owner, id, lease),
                    doc! {"$inc":{"creation_publication_fence":1_i64}},
                )
                .session(&mut *session)
                .await?;
            if fenced.matched_count != 1 {
                return Ok(false);
            }
            filter.insert("operation_id", id);
            filter.insert("operation", "agent_bootstrap");
        }
        _ => return Ok(false),
    }
    database
        .collection::<Document>(COLLECTION)
        .update_one(
            filter,
            doc! {"$set":{"state":{"kind":"created","sandbox_id":sandbox_id}}},
        )
        .session(session)
        .await
        .map(|result| result.matched_count == 1)
}

fn refusal(code: RefusalCode) -> Document {
    let code = code.code();
    doc! {"$set":{"state":{"kind":"refused","code":code}}}
}

pub(super) async fn refuse_validation(
    records: &Collection<Document>,
    owner: &SubjectId,
    fence: &CreationFence,
    session: &mut ClientSession,
) -> Result<bool, mongodb::error::Error> {
    let mut filter = live_direct(pending(owner.as_str(), fence));
    filter.insert("operation", "agent_bootstrap");
    records
        .update_one(filter, refusal(RefusalCode::CreationFailed))
        .session(session)
        .await
        .map(|result| result.matched_count == 1)
}

pub(super) async fn refuse_direct(
    database: &Database,
    owner: &SubjectId,
    fence: &CreationFence,
    code: RefusalCode,
) -> Result<bool, mongodb::error::Error> {
    let filter = live_direct(pending(owner.as_str(), fence));
    let settled = majority_collection::<Document>(database, COLLECTION)
        .update_one(filter, refusal(code))
        .await?
        .matched_count
        == 1;
    if settled {
        tracing::info!(creation_proof = %fence.record_id, reason = code.code(), "Sandbox creation refusal committed");
    }
    Ok(settled)
}

pub(in crate::store) async fn refuse_with_operation(
    database: &Database,
    owner: &SubjectId,
    fence: &CreationFence,
    id: &str,
    lease: &str,
    operation_failure: Document,
    code: RefusalCode,
) -> Result<bool, StoreError> {
    settle_operation(
        database,
        owner,
        fence,
        id,
        live_operation(owner.as_str(), id, lease),
        operation_failure,
        code,
    )
    .await
}

pub(in crate::store) async fn cancel_with_operation(
    database: &Database,
    owner: &SubjectId,
    fence: &CreationFence,
    id: &str,
    operation_failure: Document,
) -> Result<bool, StoreError> {
    let filter = doc! {"_id":id, "owner":owner.as_str(), "document_schema_version":schema::DOCUMENT_SCHEMA_VERSION,
    "status":{"$in":["queued","running","waiting_for_connection"]}};
    settle_operation(
        database,
        owner,
        fence,
        id,
        filter,
        operation_failure,
        RefusalCode::CreationCancelled,
    )
    .await
}

pub(super) async fn recover_failed_operation(
    database: &Database,
    owner: &SubjectId,
    fence: &CreationFence,
    id: &str,
) -> Result<bool, StoreError> {
    let filter = doc! {"_id":id, "owner":owner.as_str(), "document_schema_version":schema::DOCUMENT_SCHEMA_VERSION,
    "status":"failed", "lease_token":Bson::Null, "lease_expires_at":Bson::Null};
    settle_operation(
        database,
        owner,
        fence,
        id,
        filter,
        doc! {"$inc":{"creation_publication_fence":1_i64}},
        RefusalCode::CreationFailed,
    )
    .await
}

pub(in crate::store) async fn cancellation_committed(
    database: &Database,
    owner: &SubjectId,
    fence: &CreationFence,
    id: &str,
) -> Result<bool, StoreError> {
    let mut filter = pending(owner.as_str(), fence);
    filter.insert("operation_id", id);
    filter.insert("operation", "agent_bootstrap");
    filter.insert("state.kind", "refused");
    filter.insert("state.code", RefusalCode::CreationCancelled.code());
    majority_collection::<Document>(database, COLLECTION)
        .find_one(filter)
        .projection(doc! {"_id":1})
        .max_time(IO_BUDGET)
        .await
        .map(|row| row.is_some())
        .map_err(driver)
}

async fn settle_operation(
    database: &Database,
    owner: &SubjectId,
    fence: &CreationFence,
    id: &str,
    mut operation_filter: Document,
    operation_failure: Document,
    code: RefusalCode,
) -> Result<bool, StoreError> {
    operation_filter.insert("sandbox_id", Bson::Null);
    operation_filter.insert("agent_id", Bson::Null);
    operation_filter.insert("creation_fence.record_id", &fence.record_id);
    operation_filter.insert("creation_fence.generation", &fence.generation);
    operation_filter.insert("creation_fence.epoch", &fence.epoch);
    for _ in 0..4 {
        let mut session = database.client().start_session().await.map_err(driver)?;
        transaction::start_transaction(&mut session)
            .await
            .map_err(driver)?;
        let attempt = async {
            let mut filter = pending(owner.as_str(), fence);
            filter.insert("operation_id", id);
            filter.insert("operation", "agent_bootstrap");
            let result = database
                .collection::<Document>(COLLECTION)
                .update_one(filter, refusal(code))
                .session(&mut session)
                .await?;
            if result.matched_count != 1 {
                return Ok(false);
            }
            let result = database
                .collection::<Document>(LIFECYCLES)
                .update_one(operation_filter.clone(), operation_failure.clone())
                .session(&mut session)
                .await?;
            Ok(result.matched_count == 1)
        }
        .await;
        match attempt {
            Ok(true) => {}
            Ok(false) => {
                session.abort_guarded().await.map_err(driver)?;
                return Ok(false);
            }
            Err(error) if transaction::is_transient(&error) => {
                session.abort_guarded().await.map_err(driver)?;
                continue;
            }
            Err(error) => {
                session.abort_guarded().await.map_err(driver)?;
                return Err(driver(error));
            }
        }
        match transaction::commit_transaction(&mut session).await {
            transaction::CommitOutcome::Committed => {
                tracing::info!(creation_proof = %fence.record_id, reason = code.code(), "Sandbox lifecycle creation refusal committed");
                return Ok(true);
            }
            transaction::CommitOutcome::Retry(_) => continue,
            transaction::CommitOutcome::Unknown(error) => {
                tracing::warn!(creation_proof = %fence.record_id, reason = code.code(), "Sandbox lifecycle creation refusal commit acknowledgement unavailable");
                return Err(driver(error));
            }
            transaction::CommitOutcome::Failed(error) => return Err(driver(error)),
        }
    }
    Err(StoreError::Unavailable(
        "Sandbox creation refusal retry budget exhausted".into(),
    ))
}
