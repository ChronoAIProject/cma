//! Durable, restart-resumable sandbox pause and recovery state.

use serde::{Deserialize, Serialize};

mod validation;
use sha2::{Digest, Sha256};

use crate::seconds_from_rfc3339;

use super::{
    Sandbox, SandboxCheckpointCapture, SandboxCheckpointHistory, SandboxModelError,
    SandboxReviveCause, SandboxRevivePhase, SandboxRotationAccounting, SandboxRotationIdentity,
    SandboxRuntimeProvisionReservation, SandboxStatus,
};

const MAX_LIFECYCLE_OPERATION_ID_BYTES: usize = 256;
const MAX_PAUSED_NOTICE_BYTES: usize = 512;
const MAX_RUNTIME_VERSION_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPausePhase {
    CaptureAndPublishCheckpoint,
    PauseAgents,
    TeardownRuntime,
    PublishPaused,
    ResumeWriters,
}

impl SandboxPausePhase {
    pub const fn first() -> Self {
        Self::CaptureAndPublishCheckpoint
    }

    pub const fn next(self) -> Option<Self> {
        match self {
            Self::CaptureAndPublishCheckpoint => Some(Self::PauseAgents),
            Self::PauseAgents => Some(Self::TeardownRuntime),
            Self::TeardownRuntime => Some(Self::PublishPaused),
            Self::PublishPaused | Self::ResumeWriters => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxImageRotationPhase {
    CaptureAndPublishCheckpoint,
    PauseAgents,
    TeardownRuntime,
    CheckpointVerification,
    RuntimeProvisioning,
    CredentialDelivery,
    HomeRestore,
    ConversationReopen,
    QueuedMessageDelivery,
    PublishActive,
}

impl SandboxImageRotationPhase {
    pub const fn first() -> Self {
        Self::CaptureAndPublishCheckpoint
    }

    pub const fn next(self) -> Option<Self> {
        match self {
            Self::CaptureAndPublishCheckpoint => Some(Self::PauseAgents),
            Self::PauseAgents => Some(Self::TeardownRuntime),
            Self::TeardownRuntime => Some(Self::CheckpointVerification),
            Self::CheckpointVerification => Some(Self::RuntimeProvisioning),
            Self::RuntimeProvisioning => Some(Self::CredentialDelivery),
            Self::CredentialDelivery => Some(Self::HomeRestore),
            Self::HomeRestore => Some(Self::ConversationReopen),
            Self::ConversationReopen => Some(Self::QueuedMessageDelivery),
            Self::QueuedMessageDelivery => Some(Self::PublishActive),
            Self::PublishActive => None,
        }
    }
}

/// Stable operation identity plus the token needed to release a held writer fence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxLifecycleAuthority {
    pub operation_id: String,
    pub generation: u64,
    pub requested_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer_resume_token: Option<String>,
    #[serde(default)]
    pub writer_resume_generation: u64,
}

impl SandboxLifecycleAuthority {
    pub(crate) fn expected_writer_resume_token(&self) -> String {
        let identity = if self.writer_resume_generation == 0 {
            self.operation_id.clone()
        } else {
            format!(
                "{}:writer-hold:{}",
                self.operation_id, self.writer_resume_generation
            )
        };
        format!("{:x}", Sha256::digest(identity.as_bytes()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "purpose", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxLifecycleOperation {
    Pause {
        authority: SandboxLifecycleAuthority,
        phase: SandboxPausePhase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint_epoch: Option<u64>,
    },
    Revive {
        authority: SandboxLifecycleAuthority,
        phase: SandboxRevivePhase,
        cause: SandboxReviveCause,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint_epoch: Option<u64>,
    },
    ImageRotation {
        authority: SandboxLifecycleAuthority,
        phase: SandboxImageRotationPhase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint_epoch: Option<u64>,
        target_runtime_version: String,
        identity: SandboxRotationIdentity,
    },
}

impl SandboxLifecycleOperation {
    pub const fn authority(&self) -> &SandboxLifecycleAuthority {
        match self {
            Self::Pause { authority, .. }
            | Self::Revive { authority, .. }
            | Self::ImageRotation { authority, .. } => authority,
        }
    }

    pub const fn checkpoint_epoch(&self) -> Option<u64> {
        match self {
            Self::Pause {
                checkpoint_epoch, ..
            }
            | Self::ImageRotation {
                checkpoint_epoch, ..
            } => *checkpoint_epoch,
            Self::Revive {
                checkpoint_epoch, ..
            } => *checkpoint_epoch,
        }
    }

    pub const fn expected_status(&self) -> SandboxStatus {
        match self {
            Self::Pause { .. } => SandboxStatus::Pausing,
            Self::Revive { .. } => SandboxStatus::Reviving,
            Self::ImageRotation { .. } => SandboxStatus::Rotating,
        }
    }
}

/// All durable lifecycle data that is not part of runtime or credential authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxLifecycleState {
    pub next_authority: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retirement_archive: Option<super::retirement_archive::RetirementArchiveRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interruption: Option<super::SandboxInterruption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<Box<super::SandboxHome>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_recovery: Option<Box<super::SandboxHomeRecovery>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_restore: Option<Box<super::SandboxHomeRestore>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub superseded_homes: Vec<super::SandboxSupersededHome>,
    #[serde(default)]
    pub runtime_generation_high_water: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_provision: Option<SandboxRuntimeProvisionReservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_provision: Option<Box<super::SandboxFailedRuntimeProvision>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspended_runtime: Option<Box<super::RuntimeBinding>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_capture: Option<SandboxCheckpointCapture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_checkpoint: Option<Box<super::SandboxBackgroundCheckpoint>>,
    #[serde(default, skip_serializing_if = "checkpoint_epoch_is_unreserved")]
    pub checkpoint_epoch_high_water: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_capture_completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<SandboxLifecycleOperation>,
    #[serde(default)]
    pub checkpoints: SandboxCheckpointHistory,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_notice: Option<String>,
    #[serde(default)]
    pub rotation: SandboxRotationAccounting,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_update: Option<super::SandboxRuntimeUpdateState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_image_origin: Option<super::SandboxRuntimeImageOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_startup_policy: Option<Box<super::RetainedRuntimePolicyBinding>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_runtime: Option<Box<super::SandboxCustomRuntimeState>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_migration_admission: Option<Box<super::store::RuntimeMigrationAdmissionBasis>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unpublished_recovery: Option<Box<super::SandboxUnpublishedRecoveryState>>,
    /// Remains set until this recovery incident succeeds or is explicitly retired.
    #[serde(default, skip_serializing_if = "recovery_not_attempted")]
    pub unpublished_recovery_attempted: bool,
}

fn checkpoint_epoch_is_unreserved(epoch: &u64) -> bool {
    *epoch == 0
}

fn recovery_not_attempted(attempted: &bool) -> bool {
    !attempted
}

impl Default for SandboxLifecycleState {
    fn default() -> Self {
        Self {
            next_authority: 1,
            retirement_archive: None,
            interruption: None,
            home: None,
            home_recovery: None,
            home_restore: None,
            superseded_homes: Vec::new(),
            runtime_generation_high_water: 0,
            runtime_provision: None,
            failed_provision: None,
            suspended_runtime: None,
            checkpoint_capture: None,
            background_checkpoint: None,
            checkpoint_epoch_high_water: 0,
            checkpoint_capture_completed_at: None,
            operation: None,
            checkpoints: SandboxCheckpointHistory::default(),
            paused_notice: None,
            rotation: SandboxRotationAccounting::default(),
            runtime_update: None,
            runtime_image_origin: None,
            retained_startup_policy: None,
            custom_runtime: None,
            runtime_migration_admission: None,
            unpublished_recovery: None,
            unpublished_recovery_attempted: false,
        }
    }
}

impl SandboxLifecycleState {
    pub fn validate(&self, sandbox: &Sandbox) -> Result<(), SandboxModelError> {
        self.validate_home_restore(sandbox)?;
        sandbox.validate_retirement_archive()?;
        sandbox.validate_runtime_image_origin()?;
        sandbox.validate_custom_runtime()?;
        sandbox.validate_suspended_runtime()?;
        if let Some(failed) = &self.failed_provision {
            failed.validate_for(sandbox)?;
        }
        if let Some(recovery) = &self.unpublished_recovery {
            recovery.validate_for(sandbox)?;
        }
        if self.unpublished_recovery_attempted
            && !matches!(
                self.operation,
                Some(SandboxLifecycleOperation::ImageRotation { .. })
            )
        {
            return Err(invalid_lifecycle());
        }
        if let Some(update) = &self.runtime_update {
            update.validate()?;
            if let Some(request) = &update.request {
                if let Some(retained) = &request.retained_transition {
                    retained.validate_for(sandbox, request)?;
                }
            }
        }
        if let Some(interruption) = &self.interruption {
            interruption.validate()?;
        }
        if let Some(capture) = &self.background_checkpoint {
            capture.validate(sandbox)?;
        }
        if let Some(recovery) = &self.home_recovery {
            recovery.validate_for(sandbox)?;
            let operation_allows_home = matches!(
                self.operation,
                Some(
                    SandboxLifecycleOperation::Pause { .. }
                        | SandboxLifecycleOperation::Revive { .. }
                        | SandboxLifecycleOperation::ImageRotation { .. }
                )
            ) || (sandbox.status == SandboxStatus::Paused
                && self.operation.is_none());
            if !operation_allows_home
                || (recovery.resume_pause.is_some()
                    && !matches!(
                        self.operation,
                        Some(
                            SandboxLifecycleOperation::Revive {
                                cause: SandboxReviveCause::RuntimeLost,
                                ..
                            } | SandboxLifecycleOperation::ImageRotation { .. }
                        )
                    ))
            {
                return Err(invalid_lifecycle());
            }
        }
        if let Some(home) = &self.home {
            home.validate(&sandbox.id)?;
            if (sandbox.runtime.is_some() && home.attachment().is_none())
                || (home.released
                    && !matches!(
                        sandbox.status,
                        SandboxStatus::Retiring | SandboxStatus::Retired
                    ))
                || (sandbox.status == SandboxStatus::Retired && !home.released)
            {
                return Err(invalid_lifecycle());
            }
        }
        if self.next_authority == 0
            || self
                .checkpoint_capture_completed_at
                .as_deref()
                .is_some_and(|at| seconds_from_rfc3339(at).is_none())
        {
            return Err(invalid_lifecycle());
        }
        self.checkpoints.validate()?;
        self.rotation.validate()?;
        if paused_notice_is_invalid(self.paused_notice.as_deref()) {
            return Err(invalid_lifecycle());
        }
        if self.paused_notice.is_some()
            && !matches!(
                sandbox.status,
                SandboxStatus::Pausing
                    | SandboxStatus::Paused
                    | SandboxStatus::Reviving
                    | SandboxStatus::Rotating
            )
        {
            return Err(invalid_lifecycle());
        }
        match &self.operation {
            Some(operation) => self.validate_operation(sandbox, operation)?,
            None if matches!(
                sandbox.status,
                SandboxStatus::Pausing | SandboxStatus::Reviving | SandboxStatus::Rotating
            ) =>
            {
                return Err(invalid_lifecycle());
            }
            None => {}
        }
        if sandbox.status == SandboxStatus::Paused
            && self.checkpoints.latest.is_none()
            && self.home_recovery.is_none()
        {
            return Err(invalid_lifecycle());
        }
        Ok(())
    }

    pub(crate) fn start_pause(
        &mut self,
        sandbox_id: &crate::SandboxId,
        requested_at: String,
        notice: Option<String>,
    ) -> Result<(), SandboxModelError> {
        if paused_notice_is_invalid(notice.as_deref()) {
            return Err(invalid_lifecycle());
        }
        let authority = self.allocate_authority(sandbox_id, requested_at, true)?;
        // A durable home is the pause/revive source. Native suspension retains
        // its CRD and PVC; legacy replacement retains the same home authority.
        self.home_recovery = self.select_home_recovery()?;
        self.operation = Some(SandboxLifecycleOperation::Pause {
            authority,
            phase: SandboxPausePhase::first(),
            checkpoint_epoch: None,
        });
        self.paused_notice = notice;
        Ok(())
    }

    pub(crate) fn start_revive(
        &mut self,
        sandbox_id: &crate::SandboxId,
        requested_at: String,
        cause: SandboxReviveCause,
    ) -> Result<(), SandboxModelError> {
        let home_recovery = self.select_home_recovery()?;
        let checkpoint_epoch = self
            .checkpoints
            .latest
            .as_ref()
            .map(|checkpoint| checkpoint.epoch);
        if home_recovery.is_none() && checkpoint_epoch.is_none() {
            return Err(invalid_lifecycle());
        }
        let authority = self.allocate_authority(sandbox_id, requested_at, false)?;
        self.home_recovery = home_recovery;
        self.operation = Some(SandboxLifecycleOperation::Revive {
            authority,
            phase: SandboxRevivePhase::first(),
            cause,
            checkpoint_epoch,
        });
        Ok(())
    }

    pub(crate) fn start_image_rotation(
        &mut self,
        sandbox_id: &crate::SandboxId,
        requested_at: String,
        target_runtime_version: String,
        identity: SandboxRotationIdentity,
    ) -> Result<(), SandboxModelError> {
        let home_recovery = self.select_home_recovery()?;
        let authority = self.allocate_authority(sandbox_id, requested_at, true)?;
        self.home_recovery = home_recovery;
        self.operation = Some(SandboxLifecycleOperation::ImageRotation {
            authority,
            phase: SandboxImageRotationPhase::first(),
            checkpoint_epoch: None,
            target_runtime_version,
            identity,
        });
        Ok(())
    }

    pub(crate) fn cancel_operation(&mut self) {
        self.failed_provision = None;
        self.unpublished_recovery = None;
        self.unpublished_recovery_attempted = false;
        self.home_recovery = None;
        self.operation = None;
        self.runtime_provision = None;
        self.paused_notice = None;
    }

    pub(super) fn allocate_authority(
        &mut self,
        sandbox_id: &crate::SandboxId,
        requested_at: String,
        hold_writers: bool,
    ) -> Result<SandboxLifecycleAuthority, SandboxModelError> {
        if seconds_from_rfc3339(&requested_at).is_none() {
            return Err(invalid_lifecycle());
        }
        let generation = self.next_authority;
        self.next_authority = generation.checked_add(1).ok_or_else(invalid_lifecycle)?;
        self.runtime_provision = None;
        self.failed_provision = None;
        let operation_id = format!("sandbox-lifecycle-v1:{}:{generation}", sandbox_id.as_str());
        let writer_resume_token =
            hold_writers.then(|| format!("{:x}", Sha256::digest(operation_id.as_bytes())));
        Ok(SandboxLifecycleAuthority {
            operation_id,
            generation,
            requested_at,
            writer_resume_token,
            writer_resume_generation: 0,
        })
    }
}

fn paused_notice_is_invalid(notice: Option<&str>) -> bool {
    notice.is_some_and(|notice| {
        notice.trim().is_empty()
            || notice.len() > MAX_PAUSED_NOTICE_BYTES
            || notice.chars().any(char::is_control)
    })
}

const fn invalid_lifecycle() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.lifecycle")
}
