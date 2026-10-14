//! Mongo work discovery and fenced lease claims for workspaces.

use futures_util::TryStreamExt as _;
use layer2_core::ids::WorkspaceId;
use layer2_core::sandbox::store::{
    SandboxStoreError, WorkspaceStore, WorkspaceWorkerStore, MAX_PAGE_SIZE,
};
use mongodb::bson::{doc, Bson};

use super::child_writer_claims::{persist, ChildWriterClaim};
use super::sandbox_mongo::{MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS};

#[async_trait::async_trait]
impl WorkspaceWorkerStore for MongoSandboxStore {
    async fn prepared_workspace_completion(
        &self,
        sandbox: &layer2_core::sandbox::Sandbox,
        workspace: &layer2_core::sandbox::Workspace,
    ) -> Result<
        Option<layer2_core::sandbox::store::WorkspacePreparationCompletion>,
        SandboxStoreError,
    > {
        super::native_prepared::completed_workspace(self, sandbox, workspace).await
    }

    async fn due_workspace_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<WorkspaceId>, SandboxStoreError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut cursor = self
            .workspaces
            .find(doc! {
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "work_pending": true,
                "work_ready_at": { "$lte": now },
                "$or": [
                    { "work_lease_expires_at": Bson::Null },
                    { "work_lease_expires_at": { "$lte": now } },
                ],
            })
            .projection(doc! { "_id": 1 })
            .sort(doc! { "work_ready_at": 1, "_id": 1 })
            .limit(i64::from(limit))
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut ids = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            ids.push(WorkspaceId(
                document
                    .get_str("_id")
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?
                    .to_string(),
            ));
        }
        Ok(ids)
    }

    async fn claim_workspace_work(
        &self,
        id: &WorkspaceId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<layer2_core::sandbox::Workspace>, SandboxStoreError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let Some(mut workspace) = self.get_workspace(id).await? else {
                return Ok(None);
            };
            if !workspace.work.is_due(&now) {
                return Ok(None);
            }
            let expected = workspace.revision;
            workspace
                .work
                .claim(holder.to_string(), &now, lease_expires_at.clone())
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            workspace.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
            match persist(
                self,
                expected,
                ChildWriterClaim::Workspace(Box::new(workspace.clone())),
            )
            .await
            {
                Ok(true) => return Ok(Some(workspace)),
                Ok(false) => return Ok(None),
                Err(SandboxStoreError::Conflict) => tokio::task::yield_now().await,
                Err(error) => return Err(error),
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}
