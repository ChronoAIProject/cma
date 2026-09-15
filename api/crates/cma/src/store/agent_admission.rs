//! Atomic Mongo admission for an agent and its owned worktree.

use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionResult, AgentAdmissionStore, SandboxStoreError,
};
use layer2_core::sandbox::{AgentOrigin, SandboxEventKind, SandboxReviveCause, WorkspaceStatus};
use mongodb::bson::doc;

use super::agent_admission_records::{
    reconcile_created, replay_after_duplicate, replay_in_transaction, AdmissionReceipt,
    CreatedReceipt, LoadError,
};
use super::agent_log::{log_document, scan_complete_log_with_session};
use super::agent_log_integrity::{ensure_append_capacity, AppendCapacity, TransactionLogError};
use super::agents::{agent_document, LIVE_STATUSES};
use super::sandbox_events::{append_in_transaction, TransactionEventError};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, start_transaction,
    wait_for_reconciliation, CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
    TRANSACTION_ATTEMPTS,
};
use super::sandboxes::{read_sandbox, sandbox_replacement_fields};
use super::workspaces::{read_workspace, workspace_replacement_fields};
use super::worktrees::worktree_document;

#[path = "agent_admission_reuse.rs"]
mod reuse;

use reuse::reuse_profile_agent;

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

impl From<TransactionLogError> for ApplyError {
    fn from(error: TransactionLogError) -> Self {
        match error {
            TransactionLogError::Domain(error) => Self::Domain(error),
            TransactionLogError::Driver(error) => Self::Driver(error),
        }
    }
}

#[async_trait::async_trait]
impl AgentAdmissionStore for MongoSandboxStore {
    async fn admit_agent_reusing_profile(
        &self,
        admission: AgentAdmission,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        self.admit_agent_with_reuse(admission, true).await
    }

    async fn admit_agent(
        &self,
        admission: AgentAdmission,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        self.admit_agent_with_reuse(admission, false).await
    }
}

impl MongoSandboxStore {
    async fn admit_agent_with_reuse(
        &self,
        admission: AgentAdmission,
        reuse_existing: bool,
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
            let receipt = match apply_admission(self, &admission, reuse_existing, &mut session)
                .await
            {
                Ok(AdmissionReceipt::Existing(creation)) => {
                    match commit_transaction(&mut session).await {
                        CommitOutcome::Committed => {
                            return Ok(AgentAdmissionResult::Existing(*creation));
                        }
                        CommitOutcome::Retry(error) | CommitOutcome::Unknown(error) => {
                            last_transient = Some(error.to_string());
                            continue;
                        }
                        CommitOutcome::Failed(error) => {
                            return Err(SandboxStoreError::Unavailable(error.to_string()));
                        }
                    }
                }
                Ok(AdmissionReceipt::Created(receipt)) => receipt,
                Err(ApplyError::Domain(error)) => {
                    let _ = session.abort_transaction().await;
                    return Err(error);
                }
                Err(ApplyError::AgentDuplicate) => {
                    let replay_time = session.operation_time();
                    let _ = session.abort_transaction().await;
                    let replay_time = replay_time.ok_or_else(|| {
                        SandboxStoreError::Unavailable(
                            "agent admission duplicate replay has no Mongo operation time".into(),
                        )
                    })?;
                    let store = self.clone();
                    return wait_for_reconciliation(
                        "agent admission duplicate replay",
                        async move {
                            replay_after_duplicate(&store, &admission, replay_time)
                                .await?
                                .ok_or(SandboxStoreError::Duplicate)
                        },
                    )
                    .await
                    .map_err(SandboxStoreError::Unavailable)?;
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
                    let reconciliation_time = session.operation_time().ok_or_else(|| {
                        SandboxStoreError::Unavailable(format!(
                            "agent admission commit remained unknown with no Mongo operation time: {error}"
                        ))
                    })?;
                    let commit_error = error.to_string();
                    let unresolved_error = commit_error.clone();
                    let store = self.clone();
                    return wait_for_reconciliation("agent admission commit", async move {
                        if reconcile_created(&store, &receipt, reconciliation_time).await? {
                            Ok(AgentAdmissionResult::Created(receipt.creation))
                        } else {
                            Err(SandboxStoreError::Unavailable(format!(
                                "agent admission commit remained unknown: {unresolved_error}"
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
            "agent admission transaction exhausted its retry budget: {}",
            last_transient.unwrap_or_else(|| "no driver result".into())
        )))
    }
}

async fn apply_admission(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    reuse_existing: bool,
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
    if !sandbox.accepts_creation_admission() {
        return Err(ApplyError::Domain(SandboxStoreError::SandboxNotReady));
    }
    if let Some(creation) = reuse_profile_agent(
        store,
        admission,
        &workspace,
        &sandbox,
        reuse_existing,
        session,
    )
    .await?
    {
        return Ok(AdmissionReceipt::Existing(Box::new(creation)));
    }
    if matches!(admission.origin, AgentOrigin::WorkspaceProfile { .. })
        && sandbox.status != layer2_core::sandbox::SandboxStatus::Active
    {
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

    let creation =
        super::warm_sandboxes::adopt_agent(store, &sandbox, &workspace, admission, session)
            .await
            .map_err(ApplyError::Domain)?;
    let provenance = super::agent_provenance::record_creation(
        store,
        admission,
        &creation.agent,
        &sandbox,
        session,
    )
    .await
    .map_err(|error| match error {
        super::resource_access::ReadError::Driver(error) => classify_agent_insert(error),
        super::resource_access::ReadError::Domain(error) => {
            ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string()))
        }
    })?;
    let log_summary =
        scan_complete_log_with_session(store, &creation.agent.id, 0, session, |_| Ok(())).await?;
    ensure_append_capacity(
        &log_summary,
        std::slice::from_ref(&creation.initial_log),
        AppendCapacity::Normal,
    )
    .map_err(ApplyError::Domain)?;
    let next_workspace = next_workspace(&workspace)?;
    let mut next_sandbox = sandbox.clone();
    next_sandbox
        .revision
        .checked_add(1)
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    let cause = match admission.origin {
        AgentOrigin::Manual | AgentOrigin::WorkspaceProfile { .. } => {
            SandboxReviveCause::AgentCreation
        }
        AgentOrigin::Trigger { .. } => SandboxReviveCause::TriggerLaunch,
    };
    let revival = next_sandbox
        .record_creation_admission(cause, admission.event_at.clone())
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?;
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

    let revival_event = match revival {
        Some(event) => Some(
            append_in_transaction(
                store,
                session,
                &sandbox.id,
                admission.event_at.clone(),
                event,
            )
            .await?,
        ),
        None => None,
    };
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
        provenance,
        creation,
        workspace: next_workspace,
        sandbox: next_sandbox,
        revival_event,
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
            doc! {
                "$set": sandbox_replacement_fields(next).map_err(ApplyError::Domain)?,
                "$inc": { "workspace_admission_fence": 1_i64 },
            },
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

#[cfg(test)]
#[path = "agent_admission_mongo_integration_tests.rs"]
mod mongo_integration_tests;
