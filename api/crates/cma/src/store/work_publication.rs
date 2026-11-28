//! Ordinary worker writes retain the claim captured before the effect.

use layer2_core::sandbox::store::{ClaimedWork, SandboxStoreError, WorkAuthority};
use layer2_core::sandbox::Workspace;
use mongodb::bson::{doc, Document};
use tokio::time::Instant;

use super::sandbox_mongo::MongoSandboxStore;
use super::work_claim_authority::without_ledger_maintenance;

pub(super) mod workspace;

pub(super) fn workspace_deadline(
    workspace: &Workspace,
) -> Result<Option<Instant>, SandboxStoreError> {
    let chain = WorkAuthority::current()?;
    if !chain.iter().any(|authority| {
        matches!(authority.claimed(), ClaimedWork::Workspace(claimed)
            if claimed.id == workspace.id
                && claimed.owner == workspace.owner
                && claimed.sandbox_id == workspace.sandbox_id
                && claimed.work.step == workspace.work.step
                && claimed.work.lease.as_ref().is_some_and(|lease| workspace.work.owns(&lease.holder, lease.fence)))
    }) {
        return Ok(None);
    }
    chain
        .iter()
        .map(WorkAuthority::deadline)
        .try_fold(None, |shortest, deadline| {
            deadline.map(|until| Some(shortest.map_or(until, |old: Instant| old.min(until))))
        })
}

pub(super) async fn retry_workspace_write<T, F>(
    deadline: Option<Instant>,
    stage: &'static str,
    mut write: impl FnMut() -> F,
) -> Result<T, SandboxStoreError>
where
    F: std::future::Future<Output = Result<T, FenceError>>,
{
    let Some(deadline) = deadline else {
        return write().await.map_err(FenceError::into_domain);
    };
    tokio::time::timeout_at(deadline, async {
        for attempt in 0..super::sandbox_mongo::TRANSACTION_ATTEMPTS {
            super::contention::backoff(attempt).await;
            WorkAuthority::current()?;
            match write().await {
                Err(FenceError::Driver(error))
                    if attempt + 1 < super::sandbox_mongo::TRANSACTION_ATTEMPTS
                        && super::sandbox_mongo::is_transient(&error) =>
                {
                    tracing::debug!(
                        stage,
                        attempt = attempt + 1,
                        remaining_ms = deadline
                            .saturating_duration_since(Instant::now())
                            .as_millis() as u64,
                        "Retrying known-aborted Workspace transaction"
                    );
                }
                Err(FenceError::Driver(error)) => {
                    // The shared transaction opener wraps a durable fence
                    // refusal in a driver custom error.
                    return Err(error
                        .get_custom::<SandboxStoreError>()
                        .cloned()
                        .unwrap_or_else(|| FenceError::Driver(error).into_domain()));
                }
                result => return result.map_err(FenceError::into_domain),
            }
        }
        unreachable!("the final transaction attempt returns its result")
    })
    .await
    .map_err(|_| SandboxStoreError::Conflict)?
}

pub(super) fn sandbox(filter: Document, id: &str) -> Result<Document, SandboxStoreError> {
    constrain(without_ledger_maintenance(filter), id, true)
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
    let filter = super::work_schema::for_collection(rows, filter);
    let mut conditions = vec![if rows.name() == "sandboxes" {
        without_ledger_maintenance(filter)
    } else {
        filter
    }];
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

/// Preserve driver labels until the transaction root decides whether an abort
/// permits a fresh snapshot. Domain authority refusals are never retry evidence.
#[derive(Debug)]
pub(super) enum FenceError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl From<SandboxStoreError> for FenceError {
    fn from(error: SandboxStoreError) -> Self {
        Self::Domain(error)
    }
}

impl From<mongodb::error::Error> for FenceError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}

impl FenceError {
    pub(super) fn into_domain(self) -> SandboxStoreError {
        match self {
            Self::Domain(error) => error,
            Self::Driver(_) => {
                SandboxStoreError::Unavailable("work authority fence unavailable".into())
            }
        }
    }

    fn into_driver(self) -> mongodb::error::Error {
        match self {
            Self::Domain(error) => mongodb::error::Error::custom(error),
            Self::Driver(error) => error,
        }
    }
}

/// Each worker transaction writes its original authority row. This includes
/// transactions that publish only to a parent/child or an event collection.
/// A competing renewal/takeover then conflicts with the same transaction;
/// revision CAS on a different aggregate is never substituted for work custody.
pub(super) async fn fence(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
) -> Result<(), SandboxStoreError> {
    fence_transaction(store, session)
        .await
        .map_err(FenceError::into_domain)
}

/// Typed transaction callers retain the original driver error and all labels.
pub(super) async fn fence_transaction(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
) -> Result<(), FenceError> {
    for authority in WorkAuthority::current()? {
        let (rows, identity) = match authority.claimed() {
            ClaimedWork::Sandbox(row) => (
                &store.sandboxes,
                super::work_claim_authority::sandbox_claim(row)?,
            ),
            ClaimedWork::Workspace(row) => {
                if !super::work_lease_parent::fence_parent_transaction(store, row, session).await? {
                    return Err(SandboxStoreError::Conflict.into());
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
            .await?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict.into());
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
        fence_transaction(&store, session)
            .await
            .map_err(FenceError::into_driver)?;
    }
    Ok(())
}
