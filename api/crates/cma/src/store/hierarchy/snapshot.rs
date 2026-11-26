use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::{
    store::SandboxStoreError, SandboxHierarchyRecord, WorkspaceHierarchyRecord,
};
use layer2_core::{SandboxId, WorkspaceId};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};

use super::super::{
    agent_task_status,
    agents::read_agent,
    sandbox_mongo::{is_transient, start_transaction, MongoSandboxStore, TRANSACTION_ATTEMPTS},
    sandboxes::read_sandbox,
    snapshot_collection::collection,
    workspaces::read_workspace,
    worktrees::read_worktree,
};

mod agent_projection;
mod batched;
mod sandbox;

pub(super) enum Target<'a> {
    Sandbox(&'a SandboxId),
    Detail(&'a SandboxId),
    Workspace(&'a WorkspaceId),
}
pub(super) enum Reading {
    Sandbox(Option<Box<SandboxHierarchyRecord>>),
    Detail(Option<Box<layer2_core::sandbox::SandboxDetailRecord>>),
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
    pub(crate) fn with_sandbox_agent_projection(mut self, enabled: bool) -> Self {
        self.sandbox_agent_projection = enabled;
        self
    }

    pub(crate) fn with_sandbox_batched_reads(mut self, enabled: bool) -> Self {
        self.sandbox_batched_reads = enabled;
        self
    }

    pub(crate) fn with_sandbox_snapshot_reads(mut self, enabled: bool) -> Self {
        self.sandbox_snapshot_reads = enabled;
        self
    }
}

pub(super) async fn read(
    store: &MongoSandboxStore,
    target: Target<'_>,
) -> Result<Reading, SandboxStoreError> {
    let snapshot_session =
        store.sandbox_snapshot_reads && matches!(target, Target::Sandbox(_) | Target::Detail(_));
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
            let _ = session.abort_guarded().await;
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
        Target::Sandbox(id) => Ok(Reading::Sandbox(
            sandbox::read::<agent_projection::Complete>(store, id, session)
                .await?
                .map(Box::new),
        )),
        Target::Detail(id) => Ok(Reading::Detail(
            sandbox::read::<agent_projection::Projected>(store, id, session)
                .await?
                .map(|record| {
                    Box::new(layer2_core::sandbox::SandboxDetailRecord::Projected(record))
                }),
        )),
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
            let evidence = agent_task_status::read_workspace(store, &agents, session).await?;
            let task_started_at = agent_task_status::active_starts(store, &agents, session).await?;
            let mut task_statuses = std::collections::HashMap::with_capacity(evidence.len());
            let mut task_turns = std::collections::HashMap::with_capacity(evidence.len());
            let mut task_activity_at = std::collections::HashMap::new();
            for (id, evidence) in evidence {
                let (status, turn) = evidence.observation;
                task_statuses.insert(id.clone(), status);
                task_turns.insert(id.clone(), turn);
                if let Some(at) = evidence.activity_at {
                    task_activity_at.insert(id, at);
                }
            }
            Ok(Reading::Workspace(Some(Box::new(
                WorkspaceHierarchyRecord {
                    task_statuses,
                    task_turns,
                    task_activity_at,
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
