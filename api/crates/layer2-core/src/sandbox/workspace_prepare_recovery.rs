//! Explicit, immutable admission for never-ready Workspace preparation retries.
use super::store::SandboxStoreError;
use super::{
    Sandbox, SandboxStatus, SandboxWorkStep, TypedFailure, Workspace, WorkspaceSource,
    WorkspaceStatus, WorkspaceWorkStep,
};
use crate::{profiles::content_digest, Caller, Capability, SubjectId};
use layer3_adapter::{WorkspaceReceipt, WorkspaceRequest, WorkspaceSource as RuntimeSource};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePrepareRecoveryCompletion {
    pub prepared: WorkspaceReceipt,
    pub preserved_path: String,
    pub original_journal_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePrepareRecoveryAttempt {
    pub operation_id: String,
    pub requested_by: SubjectId,
    pub admitted_at: String,
    pub original_failure: TypedFailure,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<TypedFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<WorkspacePrepareRecoveryCompletion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePrepareRecovery {
    pub binding_sha256: String,
    pub original: WorkspaceRequest,
    pub attempts: Vec<WorkspacePrepareRecoveryAttempt>,
}

impl Workspace {
    fn prepare_recovery_binding(&self) -> Result<String, SandboxStoreError> {
        let source = match &self.source {
            WorkspaceSource::RepositorySnapshot { url, reference, .. } => {
                WorkspaceSource::RepositorySnapshot {
                    url: url.clone(),
                    reference: reference.clone(),
                    source_commit: None,
                }
            }
            WorkspaceSource::PublicRepositorySnapshot { url, commit, .. } => {
                WorkspaceSource::PublicRepositorySnapshot {
                    url: url.clone(),
                    commit: commit.clone(),
                    source_commit: None,
                }
            }
            source => source.clone(),
        };
        content_digest(&serde_json::json!({
            "id": self.id, "sandbox_id": self.sandbox_id, "owner": self.owner,
            "origin": self.origin, "profile": self.profile, "name": self.name,
            "path": self.path, "source": source, "created_at": self.created_at,
            "execution": self.work.execution,
        }))
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
    }

    pub fn original_prepare_request(&self) -> Result<WorkspaceRequest, SandboxStoreError> {
        let execution = self
            .work
            .execution
            .ok_or(SandboxStoreError::WorkspaceRecoveryConflict)?;
        let source = match &self.source {
            WorkspaceSource::RepositorySnapshot { url, reference, .. } => {
                RuntimeSource::RepositorySnapshot {
                    url: url.clone(),
                    reference: reference.clone(),
                }
            }
            WorkspaceSource::PublicRepositorySnapshot { url, commit, .. } => {
                RuntimeSource::PublicRepositorySnapshot {
                    url: url.clone(),
                    commit: commit.clone(),
                }
            }
            WorkspaceSource::GitRepository { url, reference } => RuntimeSource::Clone {
                url: url.clone(),
                reference: reference.clone(),
            },
            WorkspaceSource::Empty => RuntimeSource::Init,
        };
        let connection = match &self.profile.content.location {
            crate::profiles::WorkspaceLocation::RepositorySnapshot { connection_id, .. } => {
                connection_id.clone()
            }
            _ => None,
        };
        Ok(WorkspaceRequest::with_clone_depth(
            self.operation_id(WorkspaceWorkStep::Prepare),
            self.id.as_str(),
            self.name.clone(),
            source,
            execution.clone_depth,
        )
        .with_repository_connection(connection))
    }

    pub fn validate_prepare_recovery_binding(&self) -> Result<(), SandboxStoreError> {
        if let Some(recovery) = &self.work.prepare_recovery {
            if let WorkspaceSource::RepositorySnapshot { source_commit, .. }
            | WorkspaceSource::PublicRepositorySnapshot { source_commit, .. } = &self.source
            {
                let completion = recovery
                    .attempts
                    .last()
                    .and_then(|attempt| attempt.completion.as_ref());
                let matches_completion = match completion {
                    Some(completion) => {
                        source_commit.is_some()
                            && source_commit == &completion.prepared.source_commit
                    }
                    None => source_commit.is_none(),
                };
                if !matches_completion {
                    return Err(SandboxStoreError::WorkspaceRecoveryConflict);
                }
            }
            if recovery.binding_sha256 != self.prepare_recovery_binding()?
                || recovery.original != self.original_prepare_request()?
                || recovery.attempts.is_empty()
                || recovery.attempts.len() > 64
            {
                return Err(SandboxStoreError::WorkspaceRecoveryConflict);
            }
        }
        Ok(())
    }

    /// Store implementations call under the parent and Workspace admission fence.
    /// Child-absence checks belong to the same transaction, never to a prior read.
    pub fn admit_prepare_recovery(
        &mut self,
        parent: &Sandbox,
        caller: &Caller,
        operation_id: &str,
        has_agents: bool,
        has_worktrees: bool,
        now: String,
    ) -> Result<(), SandboxStoreError> {
        let admin = caller.authorization.allows(Capability::ControlAnySandbox);
        if !admin
            && (!caller.authorization.allows(Capability::UseOwnSandboxes)
                || caller.subject != self.owner)
        {
            return Err(SandboxStoreError::NotFound);
        }
        if operation_id.is_empty()
            || operation_id.len() > 128
            || !operation_id
                .bytes()
                .all(|v| v.is_ascii_alphanumeric() || matches!(v, b'_' | b'-'))
        {
            return Err(SandboxStoreError::WorkspaceRecoveryConflict);
        }
        self.validate_prepare_recovery_binding()?;
        if self.work.prepare_recovery.as_ref().is_some_and(|recovery| {
            recovery.attempts.iter().any(|attempt| {
                attempt.operation_id == operation_id && attempt.requested_by == caller.subject
            })
        }) {
            return Ok(());
        }
        if parent.id != self.sandbox_id
            || parent.owner != self.owner
            || parent.status != SandboxStatus::Active
            || parent.lifecycle.operation.is_some()
            || parent.lifecycle.checkpoint_capture.is_some()
            || parent.lifecycle.runtime_provision.is_some()
            || parent.lifecycle.home_recovery.is_some()
            || parent.lifecycle.unpublished_recovery.is_some()
            || parent.lifecycle.failed_provision.is_some()
            || parent.work.lease.is_some()
            || !matches!(
                parent.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
            )
            || self.status != WorkspaceStatus::Failed
            || self.ready_at.is_some()
            || self.resolved_commit.is_some()
            || self.removed_at.is_some()
            || self.work.lease.is_some()
            || self.work.step != WorkspaceWorkStep::None
            || self.next_worktree_number != 1
            || has_agents
            || has_worktrees
        {
            return Err(SandboxStoreError::WorkspaceRecoveryConflict);
        }
        self.validate_prerequisites(parent)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        if content_digest(&self.profile.content)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?
            != self.profile.digest
        {
            return Err(SandboxStoreError::WorkspaceRecoveryConflict);
        }
        let profile_source = match &self.profile.content.location {
            crate::profiles::WorkspaceLocation::RepositorySnapshot { url, reference, .. } => {
                WorkspaceSource::RepositorySnapshot {
                    url: url.clone(),
                    reference: reference.clone(),
                    source_commit: None,
                }
            }
            crate::profiles::WorkspaceLocation::PublicRepositorySnapshot {
                url, commit, ..
            } => WorkspaceSource::PublicRepositorySnapshot {
                url: url.clone(),
                commit: commit.clone(),
                source_commit: None,
            },
            crate::profiles::WorkspaceLocation::GitRepository { url, reference } => {
                WorkspaceSource::GitRepository {
                    url: url.clone(),
                    reference: reference.clone(),
                }
            }
            crate::profiles::WorkspaceLocation::Directory { .. } => WorkspaceSource::Empty,
        };
        if profile_source != self.source
            || self.profile.content.workspace_name().ok().as_deref() != Some(self.name.as_str())
            || !self.origin_matches_sandbox_profile(parent)
        {
            return Err(SandboxStoreError::WorkspaceRecoveryConflict);
        }
        let failure = self
            .failed
            .clone()
            .ok_or(SandboxStoreError::WorkspaceRecoveryConflict)?;
        let mut recovery = match &self.work.prepare_recovery {
            Some(value) => {
                if value.attempts.len() >= 64
                    || value
                        .attempts
                        .iter()
                        .any(|a| a.operation_id == operation_id)
                    || value
                        .attempts
                        .last()
                        .is_none_or(|a| a.failed.is_none() || a.completion.is_some())
                {
                    return Err(SandboxStoreError::WorkspaceRecoveryConflict);
                }
                value.as_ref().clone()
            }
            None => WorkspacePrepareRecovery {
                binding_sha256: self.prepare_recovery_binding()?,
                original: self.original_prepare_request()?,
                attempts: Vec::new(),
            },
        };
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        recovery.attempts.push(WorkspacePrepareRecoveryAttempt {
            operation_id: operation_id.into(),
            requested_by: caller.subject.clone(),
            admitted_at: now.clone(),
            original_failure: failure,
            failed: None,
            completion: None,
        });
        self.work.prepare_recovery = Some(Box::new(recovery));
        self.work.schedule(WorkspaceWorkStep::RecoverPrepare, now);
        // This guarded admission is the only Failed -> Preparing path.
        self.status = WorkspaceStatus::Preparing;
        self.failed = None;
        self.revision = revision;
        Ok(())
    }
}
