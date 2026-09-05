//! Atomic Mongo admission for an agent and its owned worktree.

use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionResult, AgentAdmissionStore, SandboxStoreError,
};
use layer2_core::sandbox::{SandboxEventKind, SandboxStatus, WorkspaceStatus};
use mongodb::bson::doc;

use super::agent_admission_records::{
    reconcile_created, replay_after_duplicate, replay_in_transaction, AdmissionReceipt,
    CreatedReceipt, LoadError,
};
use super::agent_log::log_document;
use super::agents::{agent_document, LIVE_STATUSES};
use super::sandbox_events::{append_in_transaction, TransactionEventError};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, start_transaction, CommitOutcome,
    MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};
use super::sandboxes::{read_sandbox, sandbox_replacement_fields};
use super::workspaces::{read_workspace, workspace_replacement_fields};
use super::worktrees::worktree_document;

enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
    AgentDuplicate,
}

impl From<LoadError> for ApplyError {
    fn from(error: LoadError) -> Self {
        match error {
            LoadError::Domain(error) => Self::Domain(error),
            LoadError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<TransactionEventError> for ApplyError {
    fn from(error: TransactionEventError) -> Self {
        match error {
            TransactionEventError::Domain(error) => Self::Domain(error),
            TransactionEventError::Driver(error) => Self::Driver(error),
        }
    }
}

#[async_trait::async_trait]
impl AgentAdmissionStore for MongoSandboxStore {
    async fn admit_agent(
        &self,
        admission: AgentAdmission,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
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
            let receipt = match apply_admission(self, &admission, &mut session).await {
                Ok(AdmissionReceipt::Existing(creation)) => {
                    let _ = session.abort_transaction().await;
                    return Ok(AgentAdmissionResult::Existing(*creation));
                }
                Ok(AdmissionReceipt::Created(receipt)) => receipt,
                Err(ApplyError::Domain(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
                Err(ApplyError::AgentDuplicate) => {
                    let _ = session.abort_transaction().await;
                    return replay_after_duplicate(self, &admission)
                        .await?
                        .ok_or(SandboxStoreError::Duplicate);
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
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => {
                    return Ok(AgentAdmissionResult::Created(receipt.creation));
                }
                CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
                CommitOutcome::Unknown(error) => {
                    return if reconcile_created(self, &receipt).await? {
                        Ok(AgentAdmissionResult::Created(receipt.creation))
                    } else {
                        Err(SandboxStoreError::Unavailable(format!(
                            "agent admission commit remained unknown: {error}"
                        )))
                    };
                }
                CommitOutcome::Failed(error) => {
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            }
        }
        Err(SandboxStoreError::Unavailable(format!(
            "agent admission transaction exhausted its retry budget: {}",
            last_transient.unwrap_or_else(|| "no driver result".into())
        )))
    }
}

async fn apply_admission(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    session: &mut mongodb::ClientSession,
) -> Result<AdmissionReceipt, ApplyError> {
    if let Some(replay) = replay_in_transaction(store, admission, session).await? {
        return Ok(replay);
    }

    let workspace = store
        .workspaces
        .find_one(doc! { "_id": admission.workspace_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(read_workspace)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::WorkspaceNotReady))?;
    if workspace.owner != admission.owner || workspace.status != WorkspaceStatus::Ready {
        return Err(ApplyError::Domain(SandboxStoreError::WorkspaceNotReady));
    }
    let sandbox = store
        .sandboxes
        .find_one(doc! { "_id": workspace.sandbox_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(read_sandbox)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::SandboxNotReady))?;
    if sandbox.owner != workspace.owner {
        return Err(ApplyError::Domain(SandboxStoreError::WorkspaceNotReady));
    }
    if sandbox.status != SandboxStatus::Active {
        return Err(ApplyError::Domain(SandboxStoreError::SandboxNotReady));
    }

    let live = store
        .agents
        .count_documents(doc! {
            "workspace_id": workspace.id.as_str(),
            "status": { "$in": LIVE_STATUSES },
            "record.deleted_at": { "$exists": false },
        })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if live >= u64::from(sandbox.limits.max_agents_per_workspace) {
        return Err(ApplyError::Domain(SandboxStoreError::AgentLimitReached {
            limit: sandbox.limits.max_agents_per_workspace,
            live: u8::try_from(live).unwrap_or(u8::MAX),
        }));
    }

    let creation = admission
        .materialize(&workspace)
        .map_err(ApplyError::Domain)?;
    let next_workspace = next_workspace(&workspace)?;
    let next_sandbox = next_sandbox(&sandbox, &admission.event_at)?;
    advance_workspace(store, &workspace, &next_workspace, session).await?;
    advance_sandbox(store, &sandbox, &next_sandbox, session).await?;

    store
        .worktrees
        .insert_one(worktree_document(&creation.worktree).map_err(ApplyError::Domain)?)
        .session(&mut *session)
        .await
        .map_err(classify_aggregate_insert)?;
    store
        .agents
        .insert_one(agent_document(&creation.agent).map_err(ApplyError::Domain)?)
        .session(&mut *session)
        .await
        .map_err(classify_agent_insert)?;
    store
        .agent_log
        .insert_one(log_document(&creation.initial_log).map_err(ApplyError::Domain)?)
        .session(&mut *session)
        .await
        .map_err(classify_log_insert)?;

    let worktree_event = append_in_transaction(
        store,
        session,
        &sandbox.id,
        admission.event_at.clone(),
        SandboxEventKind::WorktreeCreated {
            workspace_id: workspace.id.clone(),
            worktree_id: creation.worktree.id.clone(),
        },
    )
    .await?;
    let agent_event = append_in_transaction(
        store,
        session,
        &sandbox.id,
        admission.event_at.clone(),
        SandboxEventKind::AgentCreated {
            workspace_id: workspace.id,
            worktree_id: creation.worktree.id.clone(),
            agent_id: creation.agent.id.clone(),
        },
    )
    .await?;
    Ok(AdmissionReceipt::Created(Box::new(CreatedReceipt {
        creation,
        workspace: next_workspace,
        sandbox: next_sandbox,
        worktree_event,
        agent_event,
    })))
}

fn next_workspace(
    workspace: &layer2_core::sandbox::Workspace,
) -> Result<layer2_core::sandbox::Workspace, ApplyError> {
    let mut next = workspace.clone();
    next.next_worktree_number = next
        .next_worktree_number
        .checked_add(1)
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    Ok(next)
}

fn next_sandbox(
    sandbox: &layer2_core::sandbox::Sandbox,
    event_at: &str,
) -> Result<layer2_core::sandbox::Sandbox, ApplyError> {
    let mut next = sandbox.clone();
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    next.updated_at = event_at.to_owned();
    next.last_activity_at = event_at.to_owned();
    Ok(next)
}

async fn advance_workspace(
    store: &MongoSandboxStore,
    previous: &layer2_core::sandbox::Workspace,
    next: &layer2_core::sandbox::Workspace,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let advanced = store
        .workspaces
        .update_one(
            doc! {
                "_id": previous.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": signed(previous.revision, "workspace revision")
                    .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                "next_worktree_number": signed(
                    previous.next_worktree_number,
                    "workspace next worktree number",
                )
                .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
            },
            doc! { "$set": workspace_replacement_fields(next).map_err(ApplyError::Domain)? },
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if advanced.matched_count == 1 {
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    }
}

async fn advance_sandbox(
    store: &MongoSandboxStore,
    previous: &layer2_core::sandbox::Sandbox,
    next: &layer2_core::sandbox::Sandbox,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let advanced = store
        .sandboxes
        .update_one(
            doc! {
                "_id": previous.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": signed(previous.revision, "sandbox revision")
                    .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
            },
            doc! { "$set": sandbox_replacement_fields(next).map_err(ApplyError::Domain)? },
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if advanced.matched_count == 1 {
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    }
}

fn classify_aggregate_insert(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::Duplicate)
    } else {
        ApplyError::Driver(error)
    }
}

fn classify_agent_insert(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::AgentDuplicate
    } else {
        ApplyError::Driver(error)
    }
}

fn classify_log_insert(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::SequenceConflict)
    } else {
        ApplyError::Driver(error)
    }
}
