//! Generic child saves must join the parent's durable writer barrier.
//! Shared Agent/Workspace ports delegate here through A's integration hooks.

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, Workspace};
use layer2_core::{SandboxId, SubjectId};
use mongodb::{bson::doc, ClientSession};

use super::sandbox_events::TransactionEventError as Error;
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    MongoSandboxStore, TRANSACTION_ATTEMPTS,
};

#[path = "ledger_maintenance_child_save_agent.rs"]
mod agent;
#[path = "ledger_maintenance_child_save_workspace.rs"]
mod workspace;

pub(super) enum Mutation<'a> {
    Agent(&'a Agent),
    Workspace(&'a Workspace),
}

/// Preserve the original CAS predicates. Only definitely aborted transient
/// transactions may restart; an unknown commit cannot become a new save or a
/// success without a durable outcome. These legacy ports have no such receipt.
pub(super) async fn save(
    store: &MongoSandboxStore,
    expected_revision: u64,
    mutation: Mutation<'_>,
) -> Result<(), SandboxStoreError> {
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store.client.start_session().await.map_err(unavailable)?;
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                continue;
            }
            return Err(unavailable(error));
        }
        match apply(store, expected_revision, &mutation, &mut session).await {
            Ok(()) => {}
            Err(Error::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                tokio::task::yield_now().await;
                continue;
            }
            Err(error) => {
                let _ = session.abort_transaction().await;
                return Err(match error {
                    Error::Domain(error) => error,
                    Error::Driver(error) if is_duplicate_key(&error) => {
                        SandboxStoreError::Duplicate
                    }
                    Error::Driver(error) => unavailable(error),
                });
            }
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(()),
            CommitOutcome::Retry(_) => tokio::task::yield_now().await,
            CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                return Err(unavailable(error));
            }
        }
    }
    Err(SandboxStoreError::Unavailable(
        "child save transaction retries exhausted".into(),
    ))
}

pub(super) async fn apply(
    store: &MongoSandboxStore,
    expected_revision: u64,
    mutation: &Mutation<'_>,
    session: &mut ClientSession,
) -> Result<(), Error> {
    match mutation {
        Mutation::Agent(next) => agent::apply(store, expected_revision, next, session).await,
        Mutation::Workspace(next) => {
            workspace::apply(store, expected_revision, next, session).await
        }
    }
}

async fn fence_parent(
    store: &MongoSandboxStore,
    sandbox_id: &SandboxId,
    owner: &SubjectId,
    session: &mut ClientSession,
) -> Result<(), Error> {
    let row = store
        .sandboxes
        .find_one(doc! {"_id": sandbox_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(Error::Driver)?
        .ok_or(Error::Domain(SandboxStoreError::NotFound))?;
    let sandbox = super::sandboxes::read_sandbox(&row).map_err(Error::Domain)?;
    if sandbox.id != *sandbox_id || sandbox.owner != *owner {
        return Err(Error::Domain(SandboxStoreError::Conflict));
    }
    super::ledger_maintenance_writer_ports::parent_save(store, sandbox_id, session).await
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

#[cfg(test)]
#[path = "ledger_maintenance_child_save_tests.rs"]
mod tests;
