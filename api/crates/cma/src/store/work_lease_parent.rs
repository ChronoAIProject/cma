//! Workspace renewal retains the parent boundary of its original acquisition.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::store::{SandboxStoreError, WorkLeaseGrant, WORK_RENEW_SECONDS};
use layer2_core::sandbox::{SandboxStatus, Workspace, WorkspaceWorkStep};
use mongodb::{bson::doc, options::ReadConcern};
use std::time::Duration;
use tokio::time::Instant;

use super::sandbox_mongo::{
    commit_transaction, signed, start_transaction, CommitOutcome, MongoSandboxStore,
    DOCUMENT_SCHEMA_VERSION,
};
use super::sandboxes::read_sandbox;
use super::work_publication::{retry_workspace_write, workspace_deadline, FenceError};

pub(super) async fn grant(
    store: &MongoSandboxStore,
    claimed: &Workspace,
    requested_until: &str,
    short: bool,
) -> Result<Option<WorkLeaseGrant>, SandboxStoreError> {
    let deadline = if short
        && store.workspace_authority_retry
        && matches!(
            claimed.work.step,
            WorkspaceWorkStep::Prepare | WorkspaceWorkStep::Setup
        ) {
        workspace_deadline(claimed)?
            .map(|until| until.min(Instant::now() + Duration::from_secs(WORK_RENEW_SECONDS)))
    } else {
        None
    };
    retry_workspace_write(deadline, "renewal", || {
        grant_once(store, claimed, requested_until, short, deadline.is_some())
    })
    .await
}

async fn grant_once(
    store: &MongoSandboxStore,
    claimed: &Workspace,
    requested_until: &str,
    short: bool,
    observe_unknown: bool,
) -> Result<Option<WorkLeaseGrant>, FenceError> {
    let mut session = store.client.start_session().await?;
    start_transaction(&mut session).await?;
    let outcome = async {
        if !fence_parent_transaction(store, claimed, &mut session).await? {
            return Ok(None);
        }
        let identity = super::work_claim_authority::workspace_claim(claimed)?;
        if short {
            super::work_lease_renewal::grant_in_transaction(
                &store.workspaces,
                identity,
                requested_until,
                &mut session,
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
            .map_err(FenceError::Domain)
        }
    }
    .await;
    match outcome {
        Ok(Some(until)) => match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(Some(until)),
            CommitOutcome::Retry(error) => Err(FenceError::Driver(error)),
            CommitOutcome::Unknown(error) => {
                if observe_unknown {
                    let durable_target_observed =
                        observe_renewal(store, claimed, &until.expires_at).await;
                    tracing::warn!(
                        workspace_id = %claimed.id,
                        step = ?claimed.work.step,
                        durable_target_observed,
                        "Workspace renewal commit remained unknown"
                    );
                }
                Err(FenceError::Domain(SandboxStoreError::Unavailable(
                    error.to_string(),
                )))
            }
            CommitOutcome::Failed(error) => Err(FenceError::Domain(
                SandboxStoreError::Unavailable(error.to_string()),
            )),
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

async fn observe_renewal(store: &MongoSandboxStore, claimed: &Workspace, expires_at: &str) -> bool {
    let Ok(identity) = super::work_claim_authority::workspace_claim(claimed) else {
        return false;
    };
    store
        .workspaces
        .find_one(doc! {"$and":[identity, {"work_lease_expires_at":expires_at}]})
        .read_concern(ReadConcern::majority())
        .await
        .is_ok_and(|row| row.is_some())
}

pub(super) async fn fence_parent_transaction(
    store: &MongoSandboxStore,
    claimed: &Workspace,
    session: &mut mongodb::ClientSession,
) -> Result<bool, FenceError> {
    let parent = store
        .sandboxes
        .find_one(doc! {"_id":claimed.sandbox_id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(SandboxStoreError::NotFound)?;
    let parent = read_sandbox(&parent)?;
    if parent.owner != claimed.owner
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
