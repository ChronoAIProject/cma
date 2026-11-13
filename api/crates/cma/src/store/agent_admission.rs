//! Atomic Mongo admission for an agent and its owned worktree.

use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionResult, AgentAdmissionStore, SandboxStoreError,
};
use mongodb::bson::doc;

use super::agent_admission_records::{
    reconcile_created, replay_after_duplicate, AdmissionReceipt, LoadError,
};
use super::agent_log_integrity::{
    prepare_append, AppendCapacity, SemanticCheckpoint, TransactionLogError,
};
use super::sandbox_events::TransactionEventError;
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, wait_for_reconciliation,
    CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};
use super::sandboxes::sandbox_replacement_fields;
use super::workspaces::workspace_replacement_fields;

pub(in crate::store) mod initial_integrity;

pub(in crate::store) mod attribution;
pub(in crate::store) mod parent_fences;
mod retry;
pub(in crate::store) mod trigger_protocol;

use super::agent_log_segments::{
    admission::apply as apply_admission, transaction_scope::TransactionScope,
};

pub(in crate::store) enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
    AgentDuplicate,
}

mod errors;

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
            let mut scope = match TransactionScope::start(&mut session).await {
                Ok(scope) => scope,
                Err(error) => {
                    if is_transient(&error) {
                        last_transient = Some(error.to_string());
                        continue;
                    }
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            };
            let applied = apply_admission(
                self,
                &admission,
                reuse_existing,
                initiation.as_ref(),
                &mut scope,
            )
            .await;
            let session = scope.release();
            let receipt = match applied {
                Ok(AdmissionReceipt::Existing(creation)) => {
                    match commit_transaction(session).await {
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
            match commit_transaction(session).await {
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

pub(in crate::store) fn next_workspace(
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

pub(in crate::store) fn classify_aggregate_insert(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::Duplicate)
    } else {
        ApplyError::Driver(error)
    }
}

pub(in crate::store) fn classify_agent_insert(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::AgentDuplicate
    } else {
        ApplyError::Driver(error)
    }
}

pub(in crate::store) fn classify_log_insert(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::SequenceConflict)
    } else {
        ApplyError::Driver(error)
    }
}

#[cfg(test)]
#[path = "agent_admission_mongo_integration_tests.rs"]
mod mongo_integration_tests;
