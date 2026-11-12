use layer2_core::sandbox::{
    store::SandboxStoreError, SandboxHierarchyRecord, WorkspaceHierarchyRecord,
};
use layer2_core::{SandboxId, WorkspaceId};
use mongodb::{
    bson::{doc, Document},
    options::{CollectionOptions, ReadConcern},
    ClientSession, Collection,
};

use super::super::{
    agents::read_agent,
    sandbox_mongo::{is_transient, start_transaction, MongoSandboxStore, TRANSACTION_ATTEMPTS},
    sandboxes::read_sandbox,
    workspaces::read_workspace,
    worktrees::read_worktree,
};

pub(super) enum Target<'a> {
    Sandbox(&'a SandboxId),
    Workspace(&'a WorkspaceId),
}
pub(super) enum Reading {
    Sandbox(Option<Box<SandboxHierarchyRecord>>),
    Workspace(Option<Box<WorkspaceHierarchyRecord>>),
}
enum Error {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}
impl From<SandboxStoreError> for Error {
    fn from(value: SandboxStoreError) -> Self {
        Self::Domain(value)
    }
}
impl From<mongodb::error::Error> for Error {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}

impl MongoSandboxStore {
    pub(crate) fn with_sandbox_snapshot_reads(mut self, enabled: bool) -> Self {
        self.sandbox_snapshot_reads = enabled;
        self
    }
}

pub(super) async fn read(
    store: &MongoSandboxStore,
    target: Target<'_>,
) -> Result<Reading, SandboxStoreError> {
    let snapshot_session = store.sandbox_snapshot_reads && matches!(target, Target::Sandbox(_));
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store
            .client
            .start_session()
            .snapshot(snapshot_session)
            .await
            .map_err(super::unavailable)?;
        if !snapshot_session {
            start_transaction(&mut session)
                .await
                .map_err(super::unavailable)?;
        }
        let result = read_in_snapshot(store, &target, &mut session).await;
        // Snapshot sessions pin one atClusterTime across all reads without a
        // transaction or an abort checkout. Keep the rollout fallback unchanged.
        if !snapshot_session {
            let _ = session.abort_transaction().await;
        }
        match result {
            Ok(value) => return Ok(value),
            Err(Error::Domain(error)) => return Err(error),
            Err(Error::Driver(error)) if is_transient(&error) => continue,
            Err(Error::Driver(error)) => return Err(super::unavailable(error)),
        }
    }
    Err(SandboxStoreError::Unavailable(
        "hierarchy snapshot retry budget exhausted".into(),
    ))
}

async fn read_in_snapshot(
    store: &MongoSandboxStore,
    target: &Target<'_>,
    session: &mut ClientSession,
) -> Result<Reading, Error> {
    let duration = std::time::Duration::from_secs(10);
    match target {
        Target::Sandbox(id) => {
            // Snapshot sessions preserve an inherited majority concern in the
            // driver. Override it explicitly without cloning the whole store.
            let snapshot = store.sandbox_snapshot_reads;
            let sandboxes = collection(&store.sandboxes, snapshot);
            let workspaces = collection(&store.workspaces, snapshot);
            let worktrees = collection(&store.worktrees, snapshot);
            let agents = collection(&store.agents, snapshot);
            let Some(document) = sandboxes
                .find_one(doc! {"_id":id.as_str()})
                .max_time(duration)
                .session(&mut *session)
                .await?
            else {
                return Ok(Reading::Sandbox(None));
            };
            let sandbox = read_sandbox(&document)?;
            let workspaces = children(
                &workspaces,
                doc! {"sandbox_id":id.as_str()},
                session,
                read_workspace,
            )
            .await?;
            let worktrees = children(
                &worktrees,
                doc! {"sandbox_id":id.as_str()},
                session,
                read_worktree,
            )
            .await?;
            let agents = children(
                &agents,
                doc! {"sandbox_id":id.as_str()},
                session,
                read_agent,
            )
            .await?;
            Ok(Reading::Sandbox(Some(Box::new(SandboxHierarchyRecord {
                sandbox,
                workspaces,
                worktrees,
                agents,
            }))))
        }
        Target::Workspace(id) => {
            let Some(document) = store
                .workspaces
                .find_one(doc! {"_id":id.as_str()})
                .max_time(duration)
                .session(&mut *session)
                .await?
            else {
                return Ok(Reading::Workspace(None));
            };
            let workspace = read_workspace(&document)?;
            let worktrees = children(
                &store.worktrees,
                doc! {"workspace_id":id.as_str()},
                session,
                read_worktree,
            )
            .await?;
            let agents = children(
                &store.agents,
                doc! {"workspace_id":id.as_str()},
                session,
                read_agent,
            )
            .await?;
            let task_started_at =
                super::super::agent_task_status::active_starts(store, &agents, session).await?;
            let task_statuses =
                super::super::agent_task_status::read_many(store, &agents, session).await?;
            Ok(Reading::Workspace(Some(Box::new(
                WorkspaceHierarchyRecord {
                    task_statuses,
                    task_started_at,
                    workspace,
                    worktrees,
                    agents,
                },
            ))))
        }
    }
}

async fn children<T>(
    collection: &Collection<Document>,
    filter: Document,
    session: &mut ClientSession,
    parse: fn(&Document) -> Result<T, SandboxStoreError>,
) -> Result<Vec<T>, Error> {
    let mut cursor = collection
        .find(filter)
        .sort(doc! {"_id":1})
        .max_time(std::time::Duration::from_secs(10))
        .session(&mut *session)
        .await?;
    let mut values = Vec::new();
    while cursor.advance(&mut *session).await? {
        values.push(parse(&cursor.deserialize_current()?)?);
    }
    Ok(values)
}

fn collection(source: &Collection<Document>, snapshot: bool) -> Collection<Document> {
    if snapshot {
        source
            .client()
            .database(&source.namespace().db)
            .collection_with_options(
                source.name(),
                CollectionOptions::builder()
                    .read_concern(ReadConcern::snapshot())
                    .write_concern(source.write_concern().cloned())
                    .selection_criteria(source.selection_criteria().cloned())
                    .build(),
            )
    } else {
        source.clone()
    }
}
