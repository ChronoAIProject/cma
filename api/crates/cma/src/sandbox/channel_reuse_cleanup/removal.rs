//! Permanent child tombstones and monotonic physical-removal progress.
use super::*;

pub(crate) const CLEANUP_EXECUTION_FIELD: &str = "channel_reuse_cleanup_execution";
pub(crate) const CLEANUP_TOMBSTONES: &str = "channel_reuse_cleanup_tombstones";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChannelReuseRemovalPhase {
    Intent,
    ProcessClosed,
    HomeRemoved,
    WorktreeRemoved,
    Complete,
}
impl ChannelReuseRemovalPhase {
    pub(crate) fn next(self) -> Option<Self> {
        use ChannelReuseRemovalPhase::*;
        match self {
            Intent => Some(ProcessClosed),
            ProcessClosed => Some(HomeRemoved),
            HomeRemoved => Some(WorktreeRemoved),
            WorktreeRemoved => Some(Complete),
            Complete => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelReuseRemovalState {
    pub schema_version: u16,
    pub id: String,
    pub request_sha256: String,
    pub admission_sha256: String,
    pub seal: ChannelReuseCleanupSeal,
    pub runtime: RuntimeBinding,
    pub authority_fence: u64,
    pub phase: ChannelReuseRemovalPhase,
    pub revision: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub digest: String,
}
impl ChannelReuseRemovalState {
    pub(crate) fn new(
        saved: &ChannelReuseCleanupState,
        observed: &ChannelReuseCleanupObservation,
        authority_fence: u64,
    ) -> Result<Self, AllocationError> {
        observed.admission.require(saved)?;
        let mut value = Self {
            schema_version: 1,
            id: format!("{}:child-removal:v1", saved.id),
            request_sha256: saved.digest.clone(),
            admission_sha256: observed.admission.digest.clone(),
            seal: saved.seal.clone(),
            runtime: observed.runtime.clone(),
            authority_fence,
            phase: ChannelReuseRemovalPhase::Intent,
            revision: 1,
            created_at_ms: observed.observed_at_ms,
            updated_at_ms: observed.observed_at_ms,
            digest: String::new(),
        };
        value.digest = value.compute_digest()?;
        value.validate()?;
        Ok(value)
    }
    pub(crate) fn advance(&self, now: i64) -> Result<Self, AllocationError> {
        self.validate()?;
        let mut next = self.clone();
        next.phase = self
            .phase
            .next()
            .ok_or(AllocationError::InvalidTransition)?;
        next.revision = self
            .revision
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        next.updated_at_ms = now.max(self.updated_at_ms);
        next.digest = next.compute_digest()?;
        next.validate()?;
        Ok(next)
    }
    pub(crate) fn validate(&self) -> Result<(), AllocationError> {
        self.seal.validate()?;
        let expected_revision = match self.phase {
            ChannelReuseRemovalPhase::Intent => 1,
            ChannelReuseRemovalPhase::ProcessClosed => 2,
            ChannelReuseRemovalPhase::HomeRemoved => 3,
            ChannelReuseRemovalPhase::WorktreeRemoved => 4,
            ChannelReuseRemovalPhase::Complete => 5,
        };
        if self.schema_version != 1
            || self.id
                != format!(
                    "{}:child-removal:v1",
                    ChannelReuseCleanupState::id(&self.seal.allocation_id)
                )
            || self.revision != expected_revision
            || self.authority_fence == 0
            || self.authority_fence >= i64::MAX as u64
            || self.created_at_ms < 0
            || self.updated_at_ms < self.created_at_ms
            || self.request_sha256.len() != 64
            || self.admission_sha256.len() != 64
            || self
                .runtime
                .provision_generation
                .is_none_or(|generation| generation == 0)
            || self.runtime.boot_id.as_ref().is_none_or(String::is_empty)
            || self.digest != self.compute_digest()?
        {
            return Err(AllocationError::Corrupt);
        }
        Ok(())
    }
    fn compute_digest(&self) -> Result<String, AllocationError> {
        digest(&(
            self.schema_version,
            &self.id,
            &self.request_sha256,
            &self.admission_sha256,
            &self.seal,
            &self.runtime,
            self.authority_fence,
            self.phase,
            self.revision,
            self.created_at_ms,
            self.updated_at_ms,
        ))
    }
    pub(crate) fn workspace_name(&self) -> &str {
        self.seal
            .workspace_path
            .strip_prefix("/home/cma/")
            .expect("validated workspace path")
    }
    pub(crate) fn worktree_name(&self) -> &str {
        self.seal
            .worktree_path
            .rsplit('/')
            .next()
            .expect("validated worktree path")
    }
}
