use layer2_core::sandbox::{
    Sandbox, SandboxCheckpointCapturePhase, SandboxImageRotationPhase, SandboxLifecycleBlocker,
    SandboxLifecycleOperation, SandboxPausePhase, SandboxRevivePhase,
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SandboxLifecycleEligibilityView {
    pub allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub reason: Option<SandboxLifecycleBlocker>,
}

impl From<Result<(), SandboxLifecycleBlocker>> for SandboxLifecycleEligibilityView {
    fn from(value: Result<(), SandboxLifecycleBlocker>) -> Self {
        Self {
            allowed: value.is_ok(),
            reason: value.err(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SandboxCheckpointView {
    #[schema(value_type = String)]
    pub phase: SandboxCheckpointCapturePhase,
    pub started_at: String,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SandboxBackgroundSaveView {
    pub phase: String,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "purpose", rename_all = "snake_case")]
pub enum SandboxLifecycleOperationView {
    Pause {
        #[schema(value_type = String)]
        phase: SandboxPausePhase,
        started_at: String,
    },
    Revive {
        #[schema(value_type = String)]
        phase: SandboxRevivePhase,
        started_at: String,
    },
    ImageRotation {
        #[schema(value_type = String)]
        phase: SandboxImageRotationPhase,
        started_at: String,
    },
}

impl From<&SandboxLifecycleOperation> for SandboxLifecycleOperationView {
    fn from(value: &SandboxLifecycleOperation) -> Self {
        let started_at = value.authority().requested_at.clone();
        match value {
            SandboxLifecycleOperation::Pause { phase, .. } => Self::Pause {
                phase: *phase,
                started_at,
            },
            SandboxLifecycleOperation::Revive { phase, .. } => Self::Revive {
                phase: *phase,
                started_at,
            },
            SandboxLifecycleOperation::ImageRotation { phase, .. } => Self::ImageRotation {
                phase: *phase,
                started_at,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SandboxInterruptionView {
    #[schema(value_type = String)]
    pub cause: layer2_core::sandbox::SandboxInterruptionCause,
    pub detected_at: String,
    pub memory_limit_mib: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SandboxLifecycleView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<SandboxLifecycleResultView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interruption: Option<SandboxInterruptionView>,
    pub pause: SandboxLifecycleEligibilityView,
    pub revive: SandboxLifecycleEligibilityView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<SandboxCheckpointView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_save: Option<SandboxBackgroundSaveView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<SandboxLifecycleOperationView>,
}

impl From<&Sandbox> for SandboxLifecycleView {
    fn from(value: &Sandbox) -> Self {
        Self {
            result: layer2_core::sandbox::SandboxLifecycleResult::observe(
                value,
                None,
                std::time::SystemTime::now(),
                false,
            )
            .map(Into::into),
            interruption: value.lifecycle.interruption.as_ref().map(|interruption| {
                SandboxInterruptionView {
                    cause: interruption.cause,
                    detected_at: interruption.detected_at.clone(),
                    memory_limit_mib: interruption.memory_limit_mib,
                }
            }),
            pause: value.pause_eligibility().into(),
            revive: value.revive_eligibility().into(),
            background_save: value
                .lifecycle
                .background_checkpoint
                .as_ref()
                .map(|capture| {
                    let progress = &capture.progress;
                    SandboxBackgroundSaveView {
                        phase: capture.public_phase().into(),
                        started_at: capture.started_at.clone(),
                        captured_at: progress
                            .snapshot
                            .as_ref()
                            .and_then(|snapshot| snapshot.captured_at.clone()),
                        epoch: capture.plan.snapshot.epoch,
                    }
                }),
            checkpoint: value.lifecycle.checkpoint_capture.as_ref().map(|capture| {
                SandboxCheckpointView {
                    phase: capture.phase,
                    started_at: capture.authority.requested_at.clone(),
                    epoch: capture.checkpoint_epoch,
                }
            }),
            operation: value.lifecycle.operation.as_ref().map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SandboxLifecycleResultView {
    #[schema(value_type = String)]
    pub state: layer2_core::sandbox::SandboxLifecycleResultState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_due_at: Option<String>,
    pub continuing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_slot_released: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_retirement_step: Option<&'static str>,
    #[schema(value_type = Vec<String>)]
    pub actions: Vec<layer2_core::sandbox::SandboxLifecycleAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
}

impl From<layer2_core::sandbox::SandboxLifecycleResult> for SandboxLifecycleResultView {
    fn from(value: layer2_core::sandbox::SandboxLifecycleResult) -> Self {
        Self {
            state: value.state,
            code: value.code.map(Into::into),
            operation_id: value.operation_id,
            started_at: value.started_at,
            result_due_at: value.result_due_at,
            continuing: value.continuing,
            owner_slot_released: value.owner_slot_released,
            blocked_retirement_step: value.blocked_retirement_step,
            actions: value.actions,
            retry_after_seconds: value.retry_after_seconds,
        }
    }
}
