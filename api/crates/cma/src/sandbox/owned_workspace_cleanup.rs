//! Immutable consent and physical-stage proof for one owned Workspace.
use super::channel_reuse_cleanup::{ChannelReuseRemovalPhase, ChannelReuseRemovalState};
use layer2_core::{bridge::cleanup::*, sandbox::RuntimeBinding};
use layer3_adapter::{
    WorkspaceRemovalObservation, WorkspaceRemovalOutcome, WorkspaceRemovalRequest,
    WorkspaceRemoveRequest,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceCleanupState {
    pub schema_version: u16,
    pub admission: WorkspaceCleanupJobAdmission,
    pub child: ChannelReuseRemovalState,
    pub progress: WorkspaceCleanupProgress,
    pub revision: u64,
    pub runtime: Option<RuntimeBinding>,
    pub removal: Option<WorkspaceRemovalRequest>,
    pub observation: Option<WorkspaceRemovalObservation>,
    pub terminal_workspace_revision: Option<u64>,
    pub updated_at_ms: i64,
    pub digest: String,
}
impl WorkspaceCleanupState {
    pub(crate) fn new(
        admission: WorkspaceCleanupJobAdmission,
        child: ChannelReuseRemovalState,
    ) -> Result<Self, CleanupError> {
        let mut value = Self {
            updated_at_ms: admission.receipt.accepted_at_ms,
            schema_version: 1,
            admission,
            child,
            progress: WorkspaceCleanupProgress::Requested,
            revision: 1,
            runtime: None,
            removal: None,
            observation: None,
            terminal_workspace_revision: None,
            digest: String::new(),
        };
        value.seal()?;
        Ok(value)
    }
    pub(crate) fn id(&self) -> &str {
        &self.admission.receipt.id
    }
    pub(crate) fn canonical_request(&self) -> WorkspaceRemoveRequest {
        let id = &self.child.seal.target.workspace_id;
        WorkspaceRemoveRequest::new(
            format!("workspace:{id}:remove:v1"),
            id.to_string(),
            self.child.workspace_name(),
        )
    }
    pub(crate) fn advance(&self, progress: WorkspaceCleanupProgress) -> Result<Self, CleanupError> {
        self.validate()?;
        if rank(progress) != rank(self.progress) + 1 {
            return Err(CleanupError::Conflict);
        }
        let mut next = self.clone();
        next.progress = progress;
        next.revision += 1;
        next.updated_at_ms = mongodb::bson::DateTime::now()
            .timestamp_millis()
            .max(self.updated_at_ms);
        // Caller supplies that stage's matching typed proof before sealing.
        Ok(next)
    }
    pub(crate) fn seal(&mut self) -> Result<(), CleanupError> {
        self.digest = self.compute_digest()?;
        self.validate()
    }
    pub(crate) fn validate(&self) -> Result<(), CleanupError> {
        self.admission.validate()?;
        self.child.validate().map_err(|_| CleanupError::Corrupt)?;
        let receipt = &self.admission.receipt;
        if self.schema_version != 1
            || self.revision != rank(self.progress)
            || self.child.phase != ChannelReuseRemovalPhase::Complete
            || !self.child.seal.workspace_owned
            || self.child.digest != self.admission.child_removal_sha256
            || self.child.seal.workspace_creation_sha256.as_ref()
                != Some(&self.admission.workspace_creation_sha256)
            || self.child.seal.allocation_id != receipt.allocation_id
            || self.child.seal.target != receipt.target
            || self.child.seal.owner != self.admission.owner
            || self.updated_at_ms < receipt.accepted_at_ms
            || self.digest != self.compute_digest()?
            || (self.progress >= WorkspaceCleanupProgress::RemovalStarted) != self.runtime.is_some()
            || self.runtime.is_some() != self.removal.is_some()
            || (self.progress >= WorkspaceCleanupProgress::RuntimeRemoved)
                != self.observation.is_some()
            || (self.progress == WorkspaceCleanupProgress::Complete)
                != self.terminal_workspace_revision.is_some()
        {
            return Err(CleanupError::Corrupt);
        }
        if let (Some(binding), Some(removal)) = (&self.runtime, &self.removal) {
            removal.validate().map_err(|_| CleanupError::Corrupt)?;
            if removal.request != self.canonical_request()
                || removal.runtime.sandbox_id != receipt.target.sandbox_id.as_str()
                || Some(removal.runtime.generation) != binding.provision_generation
                || Some(&removal.runtime.boot_id) != binding.boot_id.as_ref()
                || binding.runtime_id.is_empty()
                || binding.runtime_version.is_empty()
                || layer2_core::seconds_from_rfc3339(&binding.bound_at).is_none()
            {
                return Err(CleanupError::Corrupt);
            }
            if let Some(observation) = &self.observation {
                observation
                    .validate_for(removal)
                    .map_err(|_| CleanupError::Corrupt)?;
                if observation.outcome != WorkspaceRemovalOutcome::CurrentlyAbsent {
                    return Err(CleanupError::Corrupt);
                }
            }
        }
        if self.terminal_workspace_revision.is_some_and(|revision| {
            revision <= receipt.request.expected_workspace_revision || revision > i64::MAX as u64
        }) {
            return Err(CleanupError::Corrupt);
        }
        Ok(())
    }
    fn compute_digest(&self) -> Result<String, CleanupError> {
        layer2_core::profiles::content_digest(&(
            self.schema_version,
            &self.admission,
            &self.child,
            self.progress,
            self.revision,
            &self.runtime,
            &self.removal,
            &self.observation,
            self.terminal_workspace_revision,
            self.updated_at_ms,
        ))
        .map_err(|_| CleanupError::Corrupt)
    }
}
fn rank(progress: WorkspaceCleanupProgress) -> u64 {
    match progress {
        WorkspaceCleanupProgress::NotRequested => 0,
        WorkspaceCleanupProgress::Requested => 1,
        WorkspaceCleanupProgress::AdmissionClosed => 2,
        WorkspaceCleanupProgress::RemovalStarted => 3,
        WorkspaceCleanupProgress::RuntimeRemoved => 4,
        WorkspaceCleanupProgress::Complete => 5,
    }
}
