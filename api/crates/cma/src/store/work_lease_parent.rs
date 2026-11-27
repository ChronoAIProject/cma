//! Workspace renewal retains the parent boundary of its original acquisition.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::store::{SandboxStoreError, WorkLeaseGrant};
use layer2_core::sandbox::{SandboxStatus, Workspace};
use mongodb::bson::doc;

use super::sandbox_mongo::{
    commit_transaction, signed, start_transaction, CommitOutcome, MongoSandboxStore,
    DOCUMENT_SCHEMA_VERSION,
};
use super::sandboxes::read_sandbox;
use super::work_claim_authority::LEDGER_MAINTENANCE_FIELD;
use super::work_publication::FenceError;

pub(super) async fn grant(
    store: &MongoSandboxStore,
    claimed: &Workspace,
    requested_until: &str,
    short: bool,
) -> Result<Option<WorkLeaseGrant>, SandboxStoreError> {
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let outcome = async {
        if !fence_parent(store, claimed, &mut session).await? {
            return Ok(None);
        }
        let identity = super::work_claim_authority::workspace_claim(claimed)?;
        if short {
            super::work_lease_renewal::grant(
                &store.workspaces,
                identity,
                requested_until,
                Some(&mut session),
            )
            .await
        } else {
            super::work_lease_renewal::renew(
                &store.workspaces,
                identity,
                requested_until,
                Some(&mut session),
            )
            .await
            .map(|until| {
                until.map(|expires_at| WorkLeaseGrant {
                    expires_at,
                    valid_for: std::time::Duration::ZERO,
                })
            })
        }
    }
    .await;
    match outcome {
        Ok(Some(until)) => match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(Some(until)),
            CommitOutcome::Retry(error)
            | CommitOutcome::Unknown(error)
            | CommitOutcome::Failed(error) => {
                Err(SandboxStoreError::Unavailable(error.to_string()))
            }
        },
        other => {
            // An unacknowledged abort still never grants renewed authority.
            if let Err(error) = session.abort_guarded().await {
                tracing::warn!(error = %error, "Workspace renewal abort was not acknowledged");
            }
            other
        }
    }
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

pub(super) async fn fence_parent(
    store: &MongoSandboxStore,
    claimed: &Workspace,
    session: &mut mongodb::ClientSession,
) -> Result<bool, SandboxStoreError> {
    fence_parent_transaction(store, claimed, session)
        .await
        .map_err(FenceError::into_domain)
}

pub(super) async fn fence_parent_transaction(
    store: &MongoSandboxStore,
    claimed: &Workspace,
    session: &mut mongodb::ClientSession,
) -> Result<bool, FenceError> {
    let parent_row = store
        .sandboxes
        .find_one(doc! {"_id":claimed.sandbox_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(SandboxStoreError::NotFound)?;
    let parent = read_sandbox(&parent_row)?;
    if parent.owner != claimed.owner
        || parent_row.contains_key(LEDGER_MAINTENANCE_FIELD)
        || parent.retirement_archive_pending()
        || parent.lifecycle.checkpoint_capture.is_some()
        || parent.status == SandboxStatus::Rotating
        || parent.defers_lifecycle_workspace(claimed)
    {
        return Ok(false);
    }
    let locked = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(crate::store::work_publication::parent(
                crate::store::work_publication::sandbox(
                    doc! {
                        "_id":parent.id.as_str(), "document_schema_version":DOCUMENT_SCHEMA_VERSION,
                        "revision":signed(parent.revision,"renewal parent revision")
                            .map_err(|_| SandboxStoreError::Conflict)?,
                        "$or":[{"runtime_writer_claim_fence":{"$exists":false}},
                            {"runtime_writer_claim_fence":{"$type":"long","$lt":i64::MAX}}],
                    },
                    parent.id.as_str(),
                )?,
                parent.id.as_str(),
            )?),
            doc! {"$inc":{"runtime_writer_claim_fence":1_i64}},
        )
        .session(&mut *session)
        .await?;
    if locked.matched_count != 1 {
        return Err(SandboxStoreError::Conflict.into());
    }
    Ok(true)
}
