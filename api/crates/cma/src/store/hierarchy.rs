//! Mongo owner-facing hierarchy projections.

use futures_util::TryStreamExt;
use layer2_core::ids::{SandboxId, SubjectId, WorkspaceId, WorktreeId};
use layer2_core::sandbox::store::{Page, PageRequest, SandboxStoreError, MAX_PAGE_SIZE};
use layer2_core::sandbox::{
    Agent, HierarchyCounts, HierarchyReadStore, SandboxHierarchyRecord, SandboxInventoryRecord,
    SandboxStatus, Workspace, WorkspaceHierarchyRecord, Worktree,
};
use mongodb::bson::{doc, Document};
use mongodb::Collection;

use super::agents::read_agent;
use super::sandbox_mongo::MongoSandboxStore;
use super::sandboxes::read_sandbox;
use super::workspaces::read_workspace;
use super::worktrees::read_worktree;

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

fn owner_filter(
    owner: &SubjectId,
    status: Option<SandboxStatus>,
    cursor: Option<&str>,
) -> Document {
    let mut filter = doc! { "owner": owner.as_str() };
    if let Some(status) = status {
        filter.insert("status", status.name());
    }
    if let Some(cursor) = cursor {
        filter.insert("_id", doc! { "$gt": cursor });
    }
    filter
}

async fn count(
    collection: &Collection<Document>,
    filter: Document,
) -> Result<u64, SandboxStoreError> {
    collection
        .count_documents(filter)
        .await
        .map_err(unavailable)
}

async fn inventory_counts(
    store: &MongoSandboxStore,
    id: &SandboxId,
) -> Result<HierarchyCounts, SandboxStoreError> {
    let filter = || doc! { "sandbox_id": id.as_str() };
    let (workspaces, worktrees, agents) = tokio::try_join!(
        count(&store.workspaces, filter()),
        count(&store.worktrees, filter()),
        count(&store.agents, filter()),
    )?;
    Ok(HierarchyCounts {
        workspaces,
        worktrees,
        agents,
    })
}

async fn sandbox_workspaces(
    store: &MongoSandboxStore,
    id: &SandboxId,
) -> Result<Vec<Workspace>, SandboxStoreError> {
    let mut cursor = store
        .workspaces
        .find(doc! { "sandbox_id": id.as_str() })
        .await
        .map_err(unavailable)?;
    let mut values = Vec::new();
    while let Some(document) = cursor.try_next().await.map_err(unavailable)? {
        values.push(read_workspace(&document)?);
    }
    Ok(values)
}

async fn sandbox_worktrees(
    store: &MongoSandboxStore,
    id: &SandboxId,
) -> Result<Vec<Worktree>, SandboxStoreError> {
    let mut cursor = store
        .worktrees
        .find(doc! { "sandbox_id": id.as_str() })
        .await
        .map_err(unavailable)?;
    let mut values = Vec::new();
    while let Some(document) = cursor.try_next().await.map_err(unavailable)? {
        values.push(read_worktree(&document)?);
    }
    Ok(values)
}

async fn sandbox_agents(
    store: &MongoSandboxStore,
    id: &SandboxId,
) -> Result<Vec<Agent>, SandboxStoreError> {
    let mut cursor = store
        .agents
        .find(doc! { "sandbox_id": id.as_str() })
        .await
        .map_err(unavailable)?;
    let mut values = Vec::new();
    while let Some(document) = cursor.try_next().await.map_err(unavailable)? {
        values.push(read_agent(&document)?);
    }
    Ok(values)
}

async fn workspace_worktrees(
    store: &MongoSandboxStore,
    id: &WorkspaceId,
) -> Result<Vec<Worktree>, SandboxStoreError> {
    let mut cursor = store
        .worktrees
        .find(doc! { "workspace_id": id.as_str() })
        .await
        .map_err(unavailable)?;
    let mut values = Vec::new();
    while let Some(document) = cursor.try_next().await.map_err(unavailable)? {
        values.push(read_worktree(&document)?);
    }
    Ok(values)
}

async fn workspace_agents(
    store: &MongoSandboxStore,
    id: &WorkspaceId,
) -> Result<Vec<Agent>, SandboxStoreError> {
    let mut cursor = store
        .agents
        .find(doc! { "workspace_id": id.as_str() })
        .await
        .map_err(unavailable)?;
    let mut values = Vec::new();
    while let Some(document) = cursor.try_next().await.map_err(unavailable)? {
        values.push(read_agent(&document)?);
    }
    Ok(values)
}

#[async_trait::async_trait]
impl HierarchyReadStore for MongoSandboxStore {
    async fn read_snapshot_archive(
        &self,
        sandbox_id: &SandboxId,
        checkpoint: &layer2_core::sandbox::SandboxCheckpointRef,
    ) -> Result<layer2_core::sandbox::SandboxSnapshotDownload, SandboxStoreError> {
        let objects = self
            .checkpoint_objects
            .for_sandbox(sandbox_id.as_str())
            .map_err(|_| {
                SandboxStoreError::Corrupt("snapshot storage identity is invalid".into())
            })?;
        let mut archive = objects
            .download(&checkpoint.object_reference)
            .await
            .map_err(|_| {
                SandboxStoreError::Unavailable("snapshot archive is unavailable".into())
            })?;
        if archive.object_reference != checkpoint.object_reference
            || archive.sha256 != checkpoint.sha256
        {
            return Err(SandboxStoreError::Corrupt(
                "snapshot archive identity does not match the completed snapshot".into(),
            ));
        }
        let (send, chunks) = tokio::sync::mpsc::channel(2);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = send.closed() => break,
                    chunk = archive.chunks.recv() => {
                        let Some(chunk) = chunk else { break; };
                        let failed = chunk.is_err();
                        let chunk = chunk.map_err(|_| SandboxStoreError::Unavailable("snapshot download failed integrity verification or transport".into()));
                        if send.send(chunk).await.is_err() || failed { break; }
                    }
                }
            }
        });
        Ok(layer2_core::sandbox::SandboxSnapshotDownload {
            size_bytes: archive.size_bytes,
            chunks,
        })
    }

    async fn list_owner_sandbox_inventory(
        &self,
        owner: &SubjectId,
        status: Option<SandboxStatus>,
        page: &PageRequest,
    ) -> Result<Page<SandboxInventoryRecord>, SandboxStoreError> {
        if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut cursor = self
            .sandboxes
            .find(owner_filter(owner, status, page.cursor.as_deref()))
            .sort(doc! { "_id": 1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(unavailable)?;
        let mut items = Vec::new();
        while let Some(document) = cursor.try_next().await.map_err(unavailable)? {
            let sandbox = read_sandbox(&document)?;
            let counts = inventory_counts(self, &sandbox.id).await?;
            items.push(SandboxInventoryRecord { sandbox, counts });
        }
        let has_more = items.len() > usize::from(page.limit);
        if has_more {
            items.pop();
        }
        let next = has_more.then(|| {
            items
                .last()
                .expect("a bounded page has a last sandbox")
                .sandbox
                .id
                .to_string()
        });
        Ok(Page { items, next })
    }

    async fn read_sandbox_hierarchy(
        &self,
        id: &SandboxId,
    ) -> Result<Option<SandboxHierarchyRecord>, SandboxStoreError> {
        let Some(document) = self
            .sandboxes
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(unavailable)?
        else {
            return Ok(None);
        };
        let sandbox = read_sandbox(&document)?;
        let (workspaces, worktrees, agents) = tokio::try_join!(
            sandbox_workspaces(self, id),
            sandbox_worktrees(self, id),
            sandbox_agents(self, id),
        )?;
        Ok(Some(SandboxHierarchyRecord {
            sandbox,
            workspaces,
            worktrees,
            agents,
        }))
    }

    async fn read_workspace_hierarchy(
        &self,
        id: &WorkspaceId,
    ) -> Result<Option<WorkspaceHierarchyRecord>, SandboxStoreError> {
        let Some(document) = self
            .workspaces
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(unavailable)?
        else {
            return Ok(None);
        };
        let workspace = read_workspace(&document)?;
        let (worktrees, agents) =
            tokio::try_join!(workspace_worktrees(self, id), workspace_agents(self, id),)?;
        Ok(Some(WorkspaceHierarchyRecord {
            workspace,
            worktrees,
            agents,
        }))
    }

    async fn read_hierarchy_worktree(
        &self,
        id: &WorktreeId,
    ) -> Result<Option<Worktree>, SandboxStoreError> {
        self.worktrees
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(unavailable)?
            .as_ref()
            .map(read_worktree)
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_filter_applies_status_before_cursor_pagination() {
        assert_eq!(
            owner_filter(
                &SubjectId::new("owner-a"),
                Some(SandboxStatus::Paused),
                Some("sbx_after"),
            ),
            doc! {
                "owner": "owner-a",
                "status": "paused",
                "_id": { "$gt": "sbx_after" },
            }
        );
        assert_eq!(
            owner_filter(&SubjectId::new("owner-a"), None, None),
            doc! { "owner": "owner-a" }
        );
    }
}
