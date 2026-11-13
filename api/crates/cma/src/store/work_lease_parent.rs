//! Workspace renewal retains the parent boundary of its original acquisition.

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{SandboxStatus, Workspace};
use mongodb::bson::doc;

use super::sandbox_mongo::{
    commit_transaction, signed, start_transaction, CommitOutcome, MongoSandboxStore,
    DOCUMENT_SCHEMA_VERSION,
};
use super::sandboxes::read_sandbox;

pub(super) async fn renew(
    store: &MongoSandboxStore,
    claimed: &Workspace,
    requested_until: &str,
) -> Result<Option<String>, SandboxStoreError> {
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    start_transaction(&mut session).await.map_err(unavailable)?;
    let outcome = async {
        let parent = store
            .sandboxes
            .find_one(doc! {"_id":claimed.sandbox_id.as_str()})
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .ok_or(SandboxStoreError::NotFound)?;
        let parent = read_sandbox(&parent)?;
        if parent.owner != claimed.owner
            || parent.retirement_archive_pending()
            || parent.lifecycle.checkpoint_capture.is_some()
            || parent.status == SandboxStatus::Rotating
            || parent.defers_lifecycle_workspace(claimed)
        {
            return Ok(None);
        }
        let locked = store
            .sandboxes
            .update_one(
                doc! {
                    "_id":parent.id.as_str(), "document_schema_version":DOCUMENT_SCHEMA_VERSION,
                    "revision":signed(parent.revision,"renewal parent revision")
                        .map_err(|_| SandboxStoreError::Conflict)?,
                    "$or":[{"runtime_writer_claim_fence":{"$exists":false}},
                        {"runtime_writer_claim_fence":{"$type":"long","$lt":i64::MAX}}],
                },
                doc! {"$inc":{"runtime_writer_claim_fence":1_i64}},
            )
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        if locked.matched_count != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        super::work_lease_renewal::renew(
            &store.workspaces,
            super::work_claim_authority::workspace_claim(claimed)?,
            requested_until,
            Some(&mut session),
        )
        .await
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
            if let Err(error) = session.abort_transaction().await {
                tracing::warn!(error = %error, "Workspace renewal abort was not acknowledged");
            }
            other
        }
    }
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
