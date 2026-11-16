//! Ordinary worker writes retain the claim captured before the effect.

use layer2_core::sandbox::store::{ClaimedWork, SandboxStoreError, WorkAuthority};
use mongodb::bson::{doc, Document};

use super::sandbox_mongo::MongoSandboxStore;

pub(super) mod workspace;

#[cfg(test)]
mod authority_mongo_tests;

pub(super) fn sandbox(filter: Document, id: &str) -> Result<Document, SandboxStoreError> {
    constrain(filter, id, true)
}

pub(super) fn workspace(filter: Document, id: &str) -> Result<Document, SandboxStoreError> {
    constrain(filter, id, false)
}

fn constrain(filter: Document, id: &str, sandbox: bool) -> Result<Document, SandboxStoreError> {
    let mut conditions = vec![filter];
    for authority in WorkAuthority::current()? {
        let claim = match authority.claimed() {
            ClaimedWork::Sandbox(row) if sandbox && row.id.as_str() == id => {
                Some(super::work_claim_authority::sandbox_claim(row)?)
            }
            ClaimedWork::Workspace(row) if !sandbox && row.id.as_str() == id => {
                Some(super::work_claim_authority::workspace_claim(row)?)
            }
            _ => None,
        };
        if let Some(claim) = claim {
            conditions.push(claim);
        }
    }
    if conditions.len() == 1 {
        Ok(conditions.remove(0))
    } else {
        Ok(doc! {"$and":conditions})
    }
}

/// Workspace effects retain the parent runtime observed before dispatch.
/// Metadata revisions and ordinary parent renewal may advance independently.
pub(super) fn parent(filter: Document, id: &str) -> Result<Document, SandboxStoreError> {
    let mut conditions = vec![filter];
    for authority in WorkAuthority::current()? {
        let Some(parent) = authority.parent().filter(|row| row.id.as_str() == id) else {
            continue;
        };
        let bson = |value| {
            mongodb::bson::serialize_to_bson(value)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
        };
        conditions.push(doc! {
            "_id":id, "owner":parent.owner.as_str(), "record.owner":parent.owner.as_str(),
            "record.runtime":bson(&parent.runtime)?,
            "record.status":mongodb::bson::serialize_to_bson(&parent.status)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?,
            "record.credential.key_generation":i64::try_from(parent.credential.key_generation)
                .map_err(|_| SandboxStoreError::Conflict)?,
            "record.credential.delivered":parent.credential.delivered,
            "record.lifecycle.operation.authority.operation_id":parent.lifecycle.operation.as_ref()
                .map(|operation| mongodb::bson::Bson::String(operation.authority().operation_id.clone()))
                .unwrap_or(mongodb::bson::Bson::Null),
            "record.lifecycle.runtime_generation_high_water":i64::try_from(parent.lifecycle.runtime_generation_high_water)
                .map_err(|_| SandboxStoreError::Conflict)?,
        });
    }
    Ok(if conditions.len() == 1 {
        conditions.remove(0)
    } else {
        doc! {"$and":conditions}
    })
}

/// Apply the original claim to generic aggregate writes without restricting
/// unrelated rows or borrowing an Agent's separate ownership authority.
pub(super) fn for_collection(
    rows: &mongodb::Collection<Document>,
    filter: Document,
) -> Result<Document, SandboxStoreError> {
    let mut conditions = vec![super::work_schema::for_collection(rows, filter)];
    for authority in WorkAuthority::current()? {
        let (id, claim) = match authority.claimed() {
            ClaimedWork::Sandbox(row) if rows.name() == "sandboxes" => (
                row.id.as_str(),
                super::work_claim_authority::sandbox_claim(row)?,
            ),
            ClaimedWork::Workspace(row) if rows.name() == "workspaces" => (
                row.id.as_str(),
                super::work_claim_authority::workspace_claim(row)?,
            ),
            _ => continue,
        };
        conditions.push(doc! {"$or":[{"_id":{"$ne":id}},claim]});
    }
    Ok(if conditions.len() == 1 {
        conditions.remove(0)
    } else {
        doc! {"$and":conditions}
    })
}

/// Each worker transaction writes its original authority row. This includes
/// transactions that publish only to a parent/child or an event collection.
/// A competing renewal/takeover then conflicts with the same transaction;
/// revision CAS on a different aggregate is never substituted for work custody.
pub(super) async fn fence(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
) -> Result<(), SandboxStoreError> {
    for authority in WorkAuthority::current()? {
        let (rows, identity) = match authority.claimed() {
            ClaimedWork::Sandbox(row) => (
                &store.sandboxes,
                super::work_claim_authority::sandbox_claim(row)?,
            ),
            ClaimedWork::Workspace(row) => {
                if !super::work_lease_parent::fence_parent(store, row, session).await? {
                    return Err(SandboxStoreError::Conflict);
                }
                (
                    &store.workspaces,
                    super::work_claim_authority::workspace_claim(row)?,
                )
            }
        };
        let result = rows
            .update_one(
                doc! {"$and":[identity,{
                    "document_schema_version":super::work_schema::versions(),
                    "$or":[{"work_publication_fence":{"$exists":false}},
                        {"work_publication_fence":{"$type":"long","$lt":i64::MAX}}],
                }]},
                doc! {"$inc":{"work_publication_fence":1_i64}},
            )
            .session(&mut *session)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
    }
    Ok(())
}

tokio::task_local! { static STORE: MongoSandboxStore; }

pub(super) async fn scope<T>(
    store: &MongoSandboxStore,
    work: impl std::future::Future<Output = T>,
) -> T {
    STORE.scope(store.clone(), work).await
}

/// The shared transaction entry point covers dependent Agent, event, metering
/// and parent writers invoked by a worker, including nested service calls.
pub(super) async fn fence_current(
    session: &mut mongodb::ClientSession,
) -> Result<(), mongodb::error::Error> {
    if let Ok(store) = STORE.try_with(Clone::clone) {
        fence(&store, session)
            .await
            .map_err(mongodb::error::Error::custom)?;
    }
    Ok(())
}
