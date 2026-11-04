use layer2_core::sandbox::{
    store::SandboxStoreError, SandboxHierarchyRecord, WorkspaceHierarchyRecord,
};
use layer2_core::{SandboxId, WorkspaceId};
use mongodb::{
    bson::{doc, Document},
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

pub(super) async fn read(
    store: &MongoSandboxStore,
    target: Target<'_>,
) -> Result<Reading, SandboxStoreError> {
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(super::unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(super::unavailable)?;
        let result = read_in_snapshot(store, &target, &mut session).await;
        // No writes occurred; releasing this transaction cannot mutate resources.
        let _ = session.abort_transaction().await;
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
            let Some(document) = store
                .sandboxes
                .find_one(doc! {"_id":id.as_str()})
                .max_time(duration)
                .session(&mut *session)
                .await?
            else {
                return Ok(Reading::Sandbox(None));
            };
            let sandbox = read_sandbox(&document)?;
            let workspaces = children(
                &store.workspaces,
                doc! {"sandbox_id":id.as_str()},
                session,
                read_workspace,
            )
            .await?;
            let worktrees = children(
                &store.worktrees,
                doc! {"sandbox_id":id.as_str()},
                session,
                read_worktree,
            )
            .await?;
            let agents = children(
                &store.agents,
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
            let task_statuses =
                super::super::agent_task_status::read_many(store, &agents, session).await?;
            Ok(Reading::Workspace(Some(Box::new(
                WorkspaceHierarchyRecord {
                    task_statuses,
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
