use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;
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
    let code = match code {
        RefusalCode::SandboxLimitReached => "sandbox_limit_reached",
        RefusalCode::TrustConfirmationRequired => "trust_confirmation_required",
    };
    doc! {"$set":{"state":{"kind":"refused","code":code}}}
}

pub(super) async fn refuse_direct(
    database: &Database,
    owner: &SubjectId,
    fence: &CreationFence,
    code: RefusalCode,
) -> Result<bool, mongodb::error::Error> {
    let filter = live_direct(pending(owner.as_str(), fence));
    majority_collection::<Document>(database, COLLECTION)
        .update_one(filter, refusal(code))
        .await
        .map(|result| result.matched_count == 1)
}

pub(in crate::store) async fn refuse_with_operation(
    database: &Database,
    owner: &SubjectId,
    fence: &CreationFence,
    id: &str,
    lease: &str,
    operation_failure: Document,
) -> Result<bool, StoreError> {
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
                .update_one(filter, refusal(RefusalCode::SandboxLimitReached))
                .session(&mut session)
                .await?;
            if result.matched_count != 1 {
                return Ok(false);
            }
            let mut operation_filter = live_operation(owner.as_str(), id, lease);
            operation_filter.insert("sandbox_id", Bson::Null);
            let result = database
                .collection::<Document>(LIFECYCLES)
                .update_one(operation_filter, operation_failure.clone())
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
            transaction::CommitOutcome::Committed => return Ok(true),
            transaction::CommitOutcome::Retry(_) => continue,
            transaction::CommitOutcome::Unknown(error)
            | transaction::CommitOutcome::Failed(error) => return Err(driver(error)),
        }
    }
    Err(StoreError::Unavailable(
        "Sandbox creation refusal retry budget exhausted".into(),
    ))
}
