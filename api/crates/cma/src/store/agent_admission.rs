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

pub(in crate::store) mod helper_error;
pub(in crate::store) mod initial_integrity;

pub(in crate::store) mod attribution;
mod commit;
pub(in crate::store) mod parent_fences;
mod retry;
pub(in crate::store) mod trigger_protocol;

use super::agent_log_segments::{admission::apply_catalogue, transaction_scope::TransactionScope};

#[cfg(test)]
use super::agent_log_segments::admission::apply as apply_admission;

pub(in crate::store) enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
    AgentDuplicate,
}

mod errors;
mod reuse_reconciliation;

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
    // An unknown commit outcome dominates a duplicate key: the earlier write may
    // have applied, so it must reconcile rather than report a sequence conflict.
    if error.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT) {
        ApplyError::Driver(error)
    } else if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::SequenceConflict)
    } else {
        ApplyError::Driver(error)
    }
}

#[cfg(test)]
#[path = "agent_admission_mongo_integration_tests.rs"]
pub(in crate::store) mod mongo_integration_tests;
