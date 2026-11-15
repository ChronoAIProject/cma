//! Restricted hierarchy handles: no writable aggregate or provider escapes.
use std::time::Duration;

use layer2_core::sandbox::{
    store::{Page, PageRequest, SandboxStoreError},
    HierarchyCounts, HierarchyReadStore, SandboxHierarchyRecord, SandboxInventoryRecord,
    SandboxStatus, WorkspaceHierarchyRecord, Worktree,
};
use layer2_core::{SandboxId, SubjectId, WorkspaceId, WorktreeId};
use mongodb::{
    bson::{doc, Document},
    Client, ClientSession, Collection, Database,
};

use super::{
    sandbox_mongo::{AGENTS, RESOURCE_POLICIES, SANDBOXES, WORKSPACES, WORKTREES},
    snapshot_collection::collection,
};

mod snapshot;
use snapshot::GuardedSnapshot;

const DEADLINE: Duration = Duration::from_secs(10);

pub(crate) struct ReadOnlyHierarchyStore {
    client: Client,
    sandboxes: Collection<Document>,
    workspaces: Collection<Document>,
    worktrees: Collection<Document>,
    agents: Collection<Document>,
    policies: Collection<Document>,
}

impl ReadOnlyHierarchyStore {
    pub(crate) async fn open(
        client: Client,
        database: Database,
    ) -> Result<Self, SandboxStoreError> {
        let names = database
            .list_collection_names()
            .await
            .map_err(unavailable)?;
        if [SANDBOXES, WORKSPACES, WORKTREES, AGENTS, RESOURCE_POLICIES]
            .iter()
            .any(|name| !names.iter().any(|n| n == name))
        {
            return Err(closed());
        }
        // These existing floor readers only inspect metadata. Never invoke
        // initialize_collections, migration application or index preparation.
        super::work_schema::floor::verify(&database)
            .await
            .map_err(|_| closed())?;
        super::agent_log_segments::schema::verify(&database)
            .await
            .map_err(|_| closed())?;
        Ok(Self {
            client,
            sandboxes: super::durability::majority_collection(&database, SANDBOXES),
            workspaces: super::durability::majority_collection(&database, WORKSPACES),
            worktrees: super::durability::majority_collection(&database, WORKTREES),
            agents: super::durability::majority_collection(&database, AGENTS),
            policies: super::durability::majority_collection(&database, RESOURCE_POLICIES),
        })
    }

    async fn session(&self) -> Result<ClientSession, SandboxStoreError> {
        self.client
            .start_session()
            .snapshot(true)
            .await
            .map_err(unavailable)
    }

    async fn inventory(
        &self,
        owner: &SubjectId,
        status: Option<SandboxStatus>,
        exclude_retired: bool,
        page: &PageRequest,
    ) -> Result<Page<SandboxInventoryRecord>, SandboxStoreError> {
        PageRequest::new(page.limit, page.cursor.clone())?;
        let mut filter = doc! {"owner": owner.as_str()};
        if let Some(status) = status {
            filter.insert("status", status.name());
        }
        if exclude_retired {
            filter.insert("$and", vec![doc! {"status": {"$ne": "retired"}}]);
        }
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! {"$gt": cursor});
        }
        let mut session = self.session().await?;
        let mut cursor = collection(&self.sandboxes, true).aggregate([
            doc! {"$match": filter}, doc! {"$sort": {"_id": 1}},
            doc! {"$lookup": {"from": RESOURCE_POLICIES, "localField": "_id", "foreignField": "_id", "as": "inert_policy"}},
            doc! {"$match": {"inert_policy.owner_kind": {"$ne": "organization"}}},
            doc! {"$limit": i64::from(page.limit) + 1},
        ]).max_time(DEADLINE).session(&mut session).await.map_err(unavailable)?;
        let mut ids = Vec::new();
        while cursor.advance(&mut session).await.map_err(unavailable)? {
            let document = cursor.deserialize_current().map_err(unavailable)?;
            ids.push(super::sandboxes::read_sandbox(&document)?.id);
        }
        drop(cursor);
        let has_more = ids.len() > usize::from(page.limit);
        if has_more {
            ids.pop();
        }
        let next = has_more.then(|| ids.last().expect("nonzero page").to_string());
        let mut records = Vec::new();
        for id in ids {
            let record = snapshot::read(self, &id, &mut session)
                .await?
                .ok_or_else(closed)?;
            records.push(record);
        }
        drop(session);
        let mut items = Vec::new();
        for record in records {
            self.recheck(&record).await?;
            let record = record.record;
            items.push(SandboxInventoryRecord {
                counts: HierarchyCounts {
                    workspaces: record.workspaces.len() as u64,
                    worktrees: record.worktrees.len() as u64,
                    agents: record.agents.len() as u64,
                },
                sandbox: record.sandbox,
            });
        }
        Ok(Page { items, next })
    }

    async fn recheck(&self, snapshot: &GuardedSnapshot) -> Result<(), SandboxStoreError> {
        // A new snapshot, acquired after projection IO, fences personal ownership
        // and the exact aggregate revision; a transfer causes refusal.
        let mut session = self.session().await?;
        let root = collection(&self.sandboxes, true)
            .find_one(doc! {"_id": snapshot.record.sandbox.id.as_str()})
            .max_time(DEADLINE)
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        let policy = collection(&self.policies, true)
            .find_one(doc! {"_id": snapshot.record.sandbox.id.as_str()})
            .max_time(DEADLINE)
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        if root.as_ref() != Some(&snapshot.root) || policy != snapshot.policy {
            return Err(closed());
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl HierarchyReadStore for ReadOnlyHierarchyStore {
    async fn list_owner_sandbox_inventory(
        &self,
        owner: &SubjectId,
        status: Option<SandboxStatus>,
        exclude_retired: bool,
        page: &PageRequest,
    ) -> Result<Page<SandboxInventoryRecord>, SandboxStoreError> {
        tokio::time::timeout(
            Duration::from_secs(30),
            self.inventory(owner, status, exclude_retired, page),
        )
        .await
        .map_err(|_| closed())?
    }

    async fn read_sandbox_hierarchy(
        &self,
        id: &SandboxId,
    ) -> Result<Option<SandboxHierarchyRecord>, SandboxStoreError> {
        tokio::time::timeout(Duration::from_secs(30), async {
            let mut session = self.session().await?;
            let record = snapshot::read(self, id, &mut session).await?;
            drop(session);
            if let Some(record) = &record {
                self.recheck(record).await?;
            }
            Ok(record.map(|r| r.record))
        })
        .await
        .map_err(|_| closed())?
    }

    async fn read_snapshot_archive(
        &self,
        _: &SandboxId,
        _: &layer2_core::sandbox::SandboxCheckpointRef,
    ) -> Result<layer2_core::sandbox::SandboxSnapshotDownload, SandboxStoreError> {
        Err(closed())
    }

    async fn read_workspace_hierarchy(
        &self,
        _: &WorkspaceId,
    ) -> Result<Option<WorkspaceHierarchyRecord>, SandboxStoreError> {
        Err(closed())
    }

    async fn read_hierarchy_worktree(
        &self,
        _: &WorktreeId,
    ) -> Result<Option<Worktree>, SandboxStoreError> {
        Err(closed())
    }
}

fn closed() -> SandboxStoreError {
    SandboxStoreError::Unavailable("inert hierarchy read is unavailable".into())
}
fn unavailable(_: mongodb::error::Error) -> SandboxStoreError {
    closed()
}

#[cfg(test)]
mod tests;
