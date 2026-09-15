//! Separate allocation-owned Workspace consent and exact permanent proof.
use super::jobs::JobError;
use super::*;
use crate::bridge::provisioning::workspace_cleanup::WorkspaceCleanupAuthority;
use layer2_core::bridge::cleanup::*;
use layer2_core::sandbox::{Sandbox, Workspace, WorkspaceStatus, WorkspaceWorkStep};

mod access;
mod admission;
mod child;
mod creation_inventory;
mod effects;
mod publication;
mod restore;
mod scheduling;
pub(in crate::store) use restore::verified_workspace_restore_paths;
mod ownership;
mod proof;
mod records;

pub(crate) const WORKSPACE_JOB_FIELD: &str = "owned_workspace_cleanup_job_v1";
pub(crate) const WORKSPACE_ADMISSION_FIELD: &str = "owned_workspace_cleanup_admission_v1";
pub(crate) const WORKSPACE_JOBS: &str = "owned_workspace_cleanup_jobs_v1";
pub(crate) const WORKSPACE_JOB_AUDITS: &str = "owned_workspace_cleanup_consents_v1";
pub(crate) const WORKSPACE_TOMBSTONES: &str = "owned_workspace_cleanup_tombstones_v1";
pub(crate) const WORKSPACE_AUDITS: &str = "owned_workspace_cleanup_audits_v1";

fn core_error(error: JobError) -> MutationError {
    match error {
        JobError::Mongo(error) => MutationError::Mongo(error),
        JobError::Core(error) => MutationError::Core(match error {
            CleanupError::Corrupt | CleanupError::Invalid => AllocationError::Corrupt,
            CleanupError::Conflict | CleanupError::NotFound => AllocationError::Conflict,
            _ => AllocationError::AuthorityUnavailable,
        }),
    }
}
