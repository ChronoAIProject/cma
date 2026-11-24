//! Cross-collection transactions required by the sandbox store contract.
use super::agents::{agent_document, read_agent};
use super::sandbox_events::{
    append_in_transaction, event_document, reconcile_event_with_snapshot_session,
    TransactionEventError,
};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, start_transaction, unsigned,
    wait_for_reconciliation, CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
    TRANSACTION_ATTEMPTS,
};
use super::sandboxes::{read_sandbox, sandbox_origin_fields, sandbox_replacement_fields};
use super::workspaces::{read_workspace, workspace_replacement_fields};
use super::worktrees::{read_worktree, worktree_document};
use layer2_core::sandbox::store::{SandboxStoreError, SandboxTransaction, SandboxTransactionStore};
use layer2_core::sandbox::{SandboxEvent, SandboxEventKind, WorkspaceStatus};
use mongodb::bson::{doc, Timestamp};
use mongodb::options::ReadConcern;

mod channel_reuse;
mod command_revival;
mod commit;
mod repository_failure;
mod sandbox_save;
mod workspace_ready;
pub(super) enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

pub(super) async fn save_reviewed_retirement(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
    expected_revision: u64,
    sandbox: &layer2_core::sandbox::Sandbox,
    at: &str,
    event: &SandboxEventKind,
) -> Result<SandboxEvent, ApplyError> {
    crate::store::work_publication::fence_transaction(store, session)
        .await
        .map_err(|error| match error {
            crate::store::work_publication::FenceError::Domain(error) => ApplyError::Domain(error),
            crate::store::work_publication::FenceError::Driver(error) => ApplyError::Driver(error),
        })?;
    match sandbox_save::apply(
        store,
        session,
        (None, None),
        expected_revision,
        sandbox,
        at,
        event,
    )
    .await?
    {
        Receipt::SaveSandbox(event, None) => Ok(event),
        _ => Err(ApplyError::Domain(SandboxStoreError::InvalidTransaction(
            "reviewed retirement did not produce a Sandbox event",
        ))),
    }
}
enum Receipt {
    Create(SandboxEvent),
    SaveSandbox(SandboxEvent, Option<command_revival::Receipt>),
    SaveWorkspace(SandboxEvent),
}

impl From<TransactionEventError> for ApplyError {
    fn from(error: TransactionEventError) -> Self {
        match error {
            TransactionEventError::Domain(error) => Self::Domain(error),
            TransactionEventError::Driver(error) => Self::Driver(error),
        }
    }
}

fn validate_transaction(transaction: &SandboxTransaction) -> Result<(), SandboxStoreError> {
    match transaction {
        SandboxTransaction::CreateWorktreeAndAgent {
            worktree, agent, ..
        } if worktree.revision != 1
            || agent.revision != 1
            || agent.conversation.last_seq != 0
            || agent.worktree_id != worktree.id
            || agent.workspace_id != worktree.workspace_id
            || agent.sandbox_id != worktree.sandbox_id
            || agent.owner != worktree.owner
            || worktree.agent_id.as_ref() != Some(&agent.id) =>
        {
            Err(SandboxStoreError::InvalidTransaction(
                "worktree and agent lineage or initial revisions disagree",
            ))
        }
        SandboxTransaction::SaveWorkspaceAndAppend {
            workspace, event, ..
        } if !workspace_event_matches(workspace, event) => Err(
            SandboxStoreError::InvalidTransaction("workspace state and event disagree"),
        ),
        _ => Ok(()),
    }
}

fn workspace_event_matches(
    workspace: &layer2_core::sandbox::Workspace,
    event: &SandboxEventKind,
) -> bool {
    match event {
        SandboxEventKind::WorkspaceReady {
            workspace_id,
            resolved_commit,
        } => {
            workspace.status == WorkspaceStatus::Ready
                && workspace_id == &workspace.id
                && workspace.resolved_commit.as_ref() == Some(resolved_commit)
        }
        SandboxEventKind::WorkspaceFailed {
            workspace_id,
            failure,
        } => {
            workspace.status == WorkspaceStatus::Failed
                && workspace_id == &workspace.id
                && workspace.failed.as_ref() == Some(failure)
        }
        SandboxEventKind::WorkspaceRemoved { workspace_id } => {
            workspace.status == WorkspaceStatus::Removed && workspace_id == &workspace.id
        }
        _ => false,
    }
}

async fn apply_transaction(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
    transaction: &SandboxTransaction,
) -> Result<Receipt, ApplyError> {
    crate::store::work_publication::fence_transaction(store, session)
        .await
        .map_err(|error| match error {
            crate::store::work_publication::FenceError::Domain(error) => ApplyError::Domain(error),
            crate::store::work_publication::FenceError::Driver(error) => ApplyError::Driver(error),
        })?;
    match transaction {
        SandboxTransaction::CreateWorktreeAndAgent {
            worktree,
            agent,
            event_at,
            event,
        } => {
            store
                .worktrees
                .insert_one(worktree_document(worktree).map_err(ApplyError::Domain)?)
                .session(&mut *session)
                .await
                .map_err(classify_write)?;
            store
                .agents
                .insert_one(agent_document(agent).map_err(ApplyError::Domain)?)
                .session(&mut *session)
                .await
                .map_err(classify_write)?;
            let event = append_in_transaction(
                store,
                session,
                &worktree.sandbox_id,
                event_at.clone(),
                event.clone(),
            )
            .await?;
            Ok(Receipt::Create(event))
        }
        SandboxTransaction::SaveSandboxAndAppend {
            command_revival,
            released_runtime,
            expected_revision,
            sandbox,
            event_at,
            event,
        } => {
            sandbox_save::apply(
                store,
                session,
                (command_revival.as_deref(), released_runtime.as_ref()),
                *expected_revision,
                sandbox,
                event_at,
                event,
            )
            .await
        }
        SandboxTransaction::SaveWorkspaceAndAppend {
            repository_runtime,
            expected_revision,
            workspace,
            event_at,
            event,
        } => {
            let next = expected_revision
                .checked_add(1)
                .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
            if workspace.revision != next {
                return Err(ApplyError::Domain(SandboxStoreError::Conflict));
            }
            let aggregate = store
                .workspaces
                .find_one(doc! { "_id": workspace.id.as_str() })
                .session(&mut *session)
                .await
                .map_err(ApplyError::Driver)?
                .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
            let current = read_workspace(&aggregate).map_err(ApplyError::Domain)?;
            workspace_ready::check_parent(store, session, &current, workspace).await?;
            layer2_core::sandbox::ProvisioningExecution::validate_successor(
                current.work.execution,
                workspace.work.execution,
            )
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
            let advanced = store
                .workspaces
                .update_one(
                    crate::store::work_schema::query(crate::store::work_publication::workspace(doc! {
                            "_id": workspace.id.as_str(),
                            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                            "channel_reuse_cleanup_execution": { "$exists": false },
                    "owned_workspace_cleanup_admission_v1": { "$exists": false },
                            "revision": signed(*expected_revision, "expected workspace revision")
                                .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                        }, workspace.id.as_str()).map_err(ApplyError::Domain)?),
                    crate::store::work_lease_update::replacement(
                        workspace_replacement_fields(workspace).map_err(ApplyError::Domain)?,
                    ),
                )
                .session(&mut *session)
                .await
                .map_err(classify_write)?;
            if advanced.matched_count != 1 {
                return Err(ApplyError::Domain(SandboxStoreError::Conflict));
            }
            let event = append_in_transaction(
                store,
                session,
                &workspace.sandbox_id,
                event_at.clone(),
                event.clone(),
            )
            .await?;
            if let Some(runtime) = repository_runtime {
                repository_failure::settle(store, session, &current, workspace, runtime, event_at)
                    .await?;
            }
            Ok(Receipt::SaveWorkspace(event))
        }
    }
}

fn classify_write(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::Duplicate)
    } else {
        ApplyError::Driver(error)
    }
}

fn classify_sequence_write(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::SequenceConflict)
    } else {
        ApplyError::Driver(error)
    }
}

async fn reconcile(
    store: &MongoSandboxStore,
    transaction: &SandboxTransaction,
    receipt: &Receipt,
    snapshot_time: Timestamp,
) -> Result<bool, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    match (transaction, receipt) {
        (
            SandboxTransaction::CreateWorktreeAndAgent {
                worktree, agent, ..
            },
            Receipt::Create(event),
        ) => {
            let stored_worktree = store
                .worktrees
                .find_one(doc! { "_id": worktree.id.as_str() })
                .read_concern(ReadConcern::snapshot())
                .session(&mut session)
                .await
                .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
                .as_ref()
                .map(read_worktree)
                .transpose()?;
            let stored_agent = store
                .agents
                .find_one(doc! { "_id": agent.id.as_str() })
                .read_concern(ReadConcern::snapshot())
                .session(&mut session)
                .await
                .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
                .as_ref()
                .map(read_agent)
                .transpose()?;
            Ok(stored_worktree.as_ref() == Some(worktree.as_ref())
                && stored_agent.as_ref() == Some(agent.as_ref())
                && reconcile_event_with_snapshot_session(store, event, &mut session)
                    .await?
                    .is_some())
        }
        (
            SandboxTransaction::SaveSandboxAndAppend { sandbox, .. },
            Receipt::SaveSandbox(event, revival),
        ) => {
            let stored = store
                .sandboxes
                .find_one(doc! { "_id": sandbox.id.as_str() })
                .read_concern(ReadConcern::snapshot())
                .session(&mut session)
                .await
                .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
                .as_ref()
                .map(read_sandbox)
                .transpose()?;
            Ok(
                command_revival::matches(store, &mut session, revival.as_ref()).await?
                    && crate::store::work_lease_update::sandbox_matches(stored.as_ref(), sandbox)
                    && reconcile_event_with_snapshot_session(store, event, &mut session)
                        .await?
                        .is_some(),
            )
        }
        (
            SandboxTransaction::SaveWorkspaceAndAppend { workspace, .. },
            Receipt::SaveWorkspace(event),
        ) => {
            let stored = store
                .workspaces
                .find_one(doc! { "_id": workspace.id.as_str() })
                .read_concern(ReadConcern::snapshot())
                .session(&mut session)
                .await
                .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
                .as_ref()
                .map(read_workspace)
                .transpose()?;
            Ok(
                crate::store::work_lease_update::workspace_matches(stored.as_ref(), workspace)
                    && reconcile_event_with_snapshot_session(store, event, &mut session)
                        .await?
                        .is_some(),
            )
        }
        _ => Err(SandboxStoreError::Corrupt(
            "transaction receipt kind does not match its command".into(),
        )),
    }
}
