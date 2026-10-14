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
};
use super::sandboxes::{read_sandbox, sandbox_replacement_fields};
use super::workspaces::{read_workspace, workspace_replacement_fields};
use super::worktrees::worktree_document;

mod attribution;
mod parent_fences;
mod retry;
#[path = "agent_admission_reuse.rs"]
mod reuse;
mod trigger_protocol;
use parent_fences::{advance_sandbox, advance_workspace};

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

impl MongoSandboxStore {
    async fn admit_agent_with_reuse(
        &self,
        admission: AgentAdmission,
        reuse_existing: bool,
        initiation: Option<layer2_core::trigger_domain::LaunchInitiation>,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        let result = self
            .commit_agent_admission(admission, reuse_existing, initiation)
            .await?;
        // No work or authority travels in this hint; the scheduler discovers and claims.
        self.agent_admitted.notify_one();
        Ok(result)
    }

    async fn commit_agent_admission(
        &self,
        admission: AgentAdmission,
        reuse_existing: bool,
        initiation: Option<layer2_core::trigger_domain::LaunchInitiation>,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        let mut last_transient = None;
        let mut retries = retry::AdmissionRetries::new();
        while retries.next().await {
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
            let receipt = match apply_admission(
                self,
                &admission,
                reuse_existing,
                initiation.as_ref(),
                &mut session,
            )
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
    initiation: Option<&layer2_core::trigger_domain::LaunchInitiation>,
    session: &mut mongodb::ClientSession,
) -> Result<AdmissionReceipt, ApplyError> {
    store
        .bindings
        .admit_agent(admission, session)
        .await
        .map_err(ApplyError::Domain)?;
    let common_launch = trigger_protocol::selected(store, admission, session).await?;
    if let Some(launch) = &common_launch {
        if launch.initiation.as_ref() != initiation {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        launch
            .initiation
            .as_ref()
            .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?
            .require_agent_admission(admission)
            .map_err(ApplyError::Domain)?;
        // An accepted receipt may be replayed after its original worker claim
        // has ended. New work still requires the live initiation fence below.
        if let Some(replay) = replay_in_transaction(store, admission, session).await? {
            return Ok(replay);
        }
    }
    if let Some(initiation) = initiation {
        attribution::fence_initiation(store, admission, initiation, session).await?;
    }
    if let Some(replay) = replay_in_transaction(store, admission, session).await? {
        return Ok(replay);
    }

    let open = store
        .workspaces
        .update_one(
            doc! {"_id":admission.workspace_id.as_str(),
            "channel_reuse_cleanup_execution":{"$exists":false},
            "owned_workspace_cleanup_admission_v1":{"$exists":false}},
            doc! {"$inc":{"owned_workspace_admission_fence":1_i64}},
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if open.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::WorkspaceNotReady));
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
    if workspace.owner != admission.owner {
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
    let bootstrap = admission.admits_bootstrap(&workspace, &sandbox);
    if !bootstrap && workspace.status != WorkspaceStatus::Ready {
        return Err(ApplyError::Domain(SandboxStoreError::WorkspaceNotReady));
    }
    if !bootstrap && !sandbox.accepts_creation_admission() {
        return Err(ApplyError::Domain(SandboxStoreError::SandboxNotReady));
    }
    if let Some(creation) = reuse_profile_agent(
        store,
        admission,
        &workspace,
        &sandbox,
        reuse_existing,
        common_launch.as_ref(),
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

    let live = super::shared_child_capacity::agents(store, &sandbox.id, &workspace.id, session)
        .await
        .map_err(|error| match error {
            super::shared_child_capacity::CapacityError::Domain(error) => ApplyError::Domain(error),
            super::shared_child_capacity::CapacityError::Driver(error) => ApplyError::Driver(error),
        })?;
    if live >= u64::from(sandbox.limits.max_agents_per_workspace) {
        return Err(ApplyError::Domain(SandboxStoreError::AgentLimitReached {
            limit: sandbox.limits.max_agents_per_workspace,
            live: u8::try_from(live).unwrap_or(u8::MAX),
        }));
    }

    let mut creation =
        super::warm_sandboxes::adopt_agent(store, &sandbox, &workspace, admission, session)
            .await
            .map_err(ApplyError::Domain)?;
    let provenance = super::agent_provenance::record_creation(
        store,
        admission,
        &creation.agent,
        &sandbox,
        initiation,
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
        AgentOrigin::Manual
        | AgentOrigin::Lifecycle { .. }
        | AgentOrigin::WorkspaceProfile { .. } => SandboxReviveCause::AgentCreation,
        AgentOrigin::Trigger { .. } => SandboxReviveCause::TriggerLaunch,
    };
    let revival = admission
        .record_parent_admission(&mut next_sandbox, &workspace, cause)
        .map_err(ApplyError::Domain)?;
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

    if let Some(launch) = &common_launch {
        super::trigger_first_messages::attach(
            store,
            launch,
            admission,
            &mut creation.agent,
            session,
        )
        .await
        .map_err(ApplyError::from)?;
    }
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
