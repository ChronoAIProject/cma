//! Mongo owner-facing hierarchy projections.

use futures_util::TryStreamExt;
use layer2_core::ids::{SandboxId, SubjectId, WorkspaceId, WorktreeId};
use layer2_core::sandbox::store::{Page, PageRequest, SandboxStoreError, MAX_PAGE_SIZE};
use layer2_core::sandbox::{
    HierarchyCounts, HierarchyReadStore, SandboxHierarchyRecord, SandboxInventoryRecord,
    SandboxStatus, WorkspaceHierarchyRecord, Worktree,
};
use mongodb::bson::{doc, Document};
use mongodb::Collection;

use super::sandbox_mongo::{MongoSandboxStore, RESOURCE_POLICIES};
mod snapshot;
use super::sandboxes::read_sandbox;
use super::worktrees::read_worktree;

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

fn owner_filter(
    owner: &SubjectId,
    status: Option<SandboxStatus>,
    exclude_retired: bool,
    cursor: Option<&str>,
) -> Document {
    let mut filter = doc! { "owner": owner.as_str() };
    if let Some(status) = status {
        filter.insert("status", status.name());
    }
    if exclude_retired {
        filter.insert("$and", vec![doc! { "status": { "$ne": "retired" } }]);
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
        exclude_retired: bool,
        page: &PageRequest,
    ) -> Result<Page<SandboxInventoryRecord>, SandboxStoreError> {
        if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let filter = super::sandbox_quarantine::exclude(owner_filter(
            owner,
            status,
            exclude_retired,
            page.cursor.as_deref(),
        ));
        let mut cursor = self.sandboxes.aggregate(vec![
            doc! {"$match":filter},
            doc! {"$sort":{"_id":1}},
            doc! {"$lookup":{"from":RESOURCE_POLICIES,"localField":"_id","foreignField":"_id","as":"inventory_policy"}},
            doc! {"$match":{"inventory_policy.owner_kind":{"$ne":"organization"}}},
            doc! {"$limit":i64::from(page.limit)+1},
        ]).max_time(std::time::Duration::from_secs(10)).await.map_err(unavailable)?;
        let mut items = Vec::new();
        while let Some(document) = cursor.try_next().await.map_err(unavailable)? {
            let sandbox = read_sandbox(&document)?;
            let policies = document.get_array("inventory_policy").map_err(|_| {
                SandboxStoreError::Corrupt("missing inventory policy lookup".into())
            })?;
            if policies.len() > 1 {
                return Err(SandboxStoreError::Corrupt(
                    "duplicate inventory policy".into(),
                ));
            }
            if let Some(policy) = policies.first() {
                let policy = policy
                    .as_document()
                    .ok_or_else(|| SandboxStoreError::Corrupt("invalid inventory policy".into()))?;
                let personal = super::resource_access::read_inventory_policy(policy)
                    .and_then(|policy| policy.personal_owner_of(&sandbox))
                    .map_err(|_| {
                        SandboxStoreError::Corrupt("invalid inventory ownership policy".into())
                    })?;
                if !personal {
                    return Err(SandboxStoreError::Corrupt(
                        "personal inventory selected an organization".into(),
                    ));
                }
            }
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
        match snapshot::read(self, snapshot::Target::Sandbox(id)).await? {
            snapshot::Reading::Sandbox(value) => Ok(value.map(|value| *value)),
            _ => unreachable!("requested Sandbox hierarchy"),
        }
    }

    async fn read_sandbox_detail(
        &self,
        id: &SandboxId,
    ) -> Result<Option<layer2_core::sandbox::SandboxDetailRecord>, SandboxStoreError> {
        if !self.sandbox_agent_projection {
            return self
                .read_sandbox_hierarchy(id)
                .await
                .map(|record| record.map(layer2_core::sandbox::SandboxDetailRecord::Complete));
        }
        tracing::debug!(parent: None, "reading projected Sandbox Agent lineage");
        match snapshot::read(self, snapshot::Target::Detail(id)).await? {
            snapshot::Reading::Detail(value) => Ok(value.map(|value| *value)),
            _ => unreachable!("requested Sandbox detail"),
        }
    }

    async fn read_workspace_hierarchy(
        &self,
        id: &WorkspaceId,
    ) -> Result<Option<WorkspaceHierarchyRecord>, SandboxStoreError> {
        match snapshot::read(self, snapshot::Target::Workspace(id)).await? {
            snapshot::Reading::Workspace(value) => Ok(value.map(|value| *value)),
            _ => unreachable!("requested Workspace hierarchy"),
        }
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
                false,
                Some("sbx_after"),
            ),
            doc! {
                "owner": "owner-a",
                "status": "paused",
                "_id": { "$gt": "sbx_after" },
            }
        );
        assert_eq!(
            owner_filter(&SubjectId::new("owner-a"), None, false, None),
            doc! { "owner": "owner-a" }
        );
    }
}
