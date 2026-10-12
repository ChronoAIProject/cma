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
pub(super) enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
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
            expected_revision,
            sandbox,
            event_at,
            event,
        } => {
            let next = expected_revision
                .checked_add(1)
                .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
            if sandbox.revision != next {
                return Err(ApplyError::Domain(SandboxStoreError::Conflict));
            }
            let aggregate = store
                .sandboxes
                .find_one(doc! {
                    "_id": sandbox.id.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                })
                .session(&mut *session)
                .await
                .map_err(ApplyError::Driver)?;
            let Some(aggregate) = aggregate else {
                let exists = store
                    .sandboxes
                    .find_one(doc! { "_id": sandbox.id.as_str() })
                    .projection(doc! { "_id": 1 })
                    .session(&mut *session)
                    .await
                    .map_err(ApplyError::Driver)?
                    .is_some();
                return Err(ApplyError::Domain(if exists {
                    SandboxStoreError::Conflict
                } else {
                    SandboxStoreError::NotFound
                }));
            };
            let current = read_sandbox(&aggregate).map_err(ApplyError::Domain)?;
            if current.revision != *expected_revision {
                return Err(ApplyError::Domain(SandboxStoreError::Conflict));
            }
            channel_reuse::require(store, &current, sandbox, session).await?;
            let revival_receipt = command_revival::fence(
                store,
                session,
                command_revival.as_deref(),
                sandbox,
                event_at,
                event,
            )
            .await?;
            current
                .validate_service_access_transaction(sandbox, event, event_at)
                .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
            current
                .validate_checkpoint_capture_successor(sandbox)
                .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
            layer2_core::sandbox::ProvisioningExecution::validate_successor(
                current.work.execution,
                sandbox.work.execution,
            )
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
            current
                .validate_runtime_generation_successor(sandbox)
                .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
            let previous = aggregate
                .get_i64("last_seq")
                .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))
                .and_then(|value| {
                    unsigned(value, "sandbox event sequence")
                        .map_err(SandboxStoreError::Corrupt)
                        .map_err(ApplyError::Domain)
                })?;
            let seq = previous
                .checked_add(1)
                .ok_or(ApplyError::Domain(SandboxStoreError::SequenceConflict))?;
            let mut fields = sandbox_replacement_fields(sandbox).map_err(ApplyError::Domain)?;
            let (origin_type, origin_launch_id, origin_operation_id) =
                sandbox_origin_fields(sandbox).map_err(ApplyError::Domain)?;
            fields.insert(
                "last_seq",
                signed(seq, "sandbox event sequence")
                    .map_err(SandboxStoreError::Corrupt)
                    .map_err(ApplyError::Domain)?,
            );
            let advanced = store
                .sandboxes
                .update_one(
                    doc! {
                        "_id": sandbox.id.as_str(),
                        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                        "channel_reuse_cleanup_execution": { "$exists": false },
                        "revision": signed(*expected_revision, "expected sandbox revision")
                            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                        "last_seq": signed(previous, "sandbox event sequence")
                            .map_err(SandboxStoreError::Corrupt)
                            .map_err(ApplyError::Domain)?,
                        "origin_type": origin_type, "origin_launch_id": origin_launch_id,
                        "origin_operation_id": origin_operation_id,
                    },
                    doc! { "$set": fields },
                )
                .session(&mut *session)
                .await
                .map_err(classify_write)?;
            if advanced.matched_count != 1 {
                return Err(ApplyError::Domain(SandboxStoreError::Conflict));
            }
            let event = SandboxEvent {
                sandbox_id: sandbox.id.clone(),
                seq,
                appended_at: event_at.clone(),
                kind: event.clone(),
            };
            store
                .sandbox_events
                .insert_one(event_document(&event).map_err(ApplyError::Domain)?)
                .session(session)
                .await
                .map_err(classify_sequence_write)?;
            Ok(Receipt::SaveSandbox(event, revival_receipt))
        }
        SandboxTransaction::SaveWorkspaceAndAppend {
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
            layer2_core::sandbox::ProvisioningExecution::validate_successor(
                current.work.execution,
                workspace.work.execution,
            )
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
            let advanced = store
                .workspaces
                .update_one(
                    doc! {
                            "_id": workspace.id.as_str(),
                            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                            "channel_reuse_cleanup_execution": { "$exists": false },
                    "owned_workspace_cleanup_admission_v1": { "$exists": false },
                            "revision": signed(*expected_revision, "expected workspace revision")
                                .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                        },
                    doc! { "$set": workspace_replacement_fields(workspace)
                    .map_err(ApplyError::Domain)? },
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
                    && stored.as_ref() == Some(sandbox.as_ref())
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
            Ok(stored.as_ref() == Some(workspace.as_ref())
                && reconcile_event_with_snapshot_session(store, event, &mut session)
                    .await?
                    .is_some())
        }
        _ => Err(SandboxStoreError::Corrupt(
            "transaction receipt kind does not match its command".into(),
        )),
    }
}

#[async_trait::async_trait]
impl SandboxTransactionStore for MongoSandboxStore {
    async fn commit(&self, transaction: SandboxTransaction) -> Result<(), SandboxStoreError> {
        validate_transaction(&transaction)?;
        let mut last_transient = None;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::task::yield_now().await;
            }
            let mut session = match self.client.start_session().await {
                Ok(session) => session,
                Err(error) if is_transient(&error) => {
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(error) => return Err(SandboxStoreError::Unavailable(error.to_string())),
            };
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    last_transient = Some(error.to_string());
                    continue;
                }
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
            let receipt = match apply_transaction(self, &mut session, &transaction).await {
                Ok(receipt) => receipt,
                Err(ApplyError::Domain(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
                Err(ApplyError::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_transaction().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(ApplyError::Driver(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            };
            if let Err(error) = command_revival::require_current(&transaction) {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
                CommitOutcome::Unknown(error) => {
                    let snapshot_time = session.operation_time().ok_or_else(|| {
                        SandboxStoreError::Unavailable(format!(
                            "sandbox transaction commit remained unknown without an operation time: {error}"
                        ))
                    })?;
                    let commit_error = error.to_string();
                    let unresolved = commit_error.clone();
                    let store = self.clone();
                    return wait_for_reconciliation("sandbox transaction", async move {
                        if reconcile(&store, &transaction, &receipt, snapshot_time).await? {
                            Ok(())
                        } else {
                            Err(SandboxStoreError::Unavailable(format!(
                                "sandbox transaction commit remained unknown: {unresolved}"
                            )))
                        }
                    })
                    .await
                    .map_err(|deadline| {
                        SandboxStoreError::Unavailable(format!(
                            "{deadline}; ambiguous commit: {commit_error}"
                        ))
                    })?;
                }
                CommitOutcome::Failed(error) => {
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            }
        }
        Err(SandboxStoreError::Unavailable(format!(
            "sandbox transaction exhausted its retry budget: {}",
            last_transient.unwrap_or_else(|| "no driver result".into())
        )))
    }
}
