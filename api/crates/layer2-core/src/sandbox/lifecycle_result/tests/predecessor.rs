// Frozen lifecycle reader/writer from 02cde5df6, before #2393.
use crate::sandbox::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PredecessorLifecycle {
    pub next_authority: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retirement_archive:
        Option<crate::sandbox::retirement_archive::RetirementArchiveRequirement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interruption: Option<crate::sandbox::SandboxInterruption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<Box<crate::sandbox::SandboxHome>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_recovery: Option<Box<crate::sandbox::SandboxHomeRecovery>>,
    #[serde(default)]
    pub runtime_generation_high_water: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_provision: Option<SandboxRuntimeProvisionReservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_provision: Option<Box<crate::sandbox::SandboxFailedRuntimeProvision>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspended_runtime: Option<Box<crate::sandbox::RuntimeBinding>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_capture: Option<SandboxCheckpointCapture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_checkpoint: Option<Box<crate::sandbox::SandboxBackgroundCheckpoint>>,
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
    pub runtime_update: Option<crate::sandbox::SandboxRuntimeUpdateState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unpublished_recovery: Option<Box<crate::sandbox::SandboxUnpublishedRecoveryState>>,
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
