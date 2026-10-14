//! Follow only immediately due progress; retries and external waits yield the sweep.
use layer2_core::sandbox::{Sandbox, SandboxLifecycleOperation, SandboxStatus, SandboxWorkStep};

pub(super) fn can_follow(sandbox: &Sandbox) -> bool {
    (matches!(
        sandbox.status,
        SandboxStatus::Provisioning | SandboxStatus::Installing
    ) && sandbox.runtime.is_some())
        || (sandbox.work.step == SandboxWorkStep::RunLifecycleOperation
            && sandbox.lifecycle.operation.is_some())
        || (sandbox.status == SandboxStatus::Retiring
            && matches!(
                sandbox.work.step,
                SandboxWorkStep::StopAgents
                    | SandboxWorkStep::RevokeRetirementCredentials
                    | SandboxWorkStep::TeardownRuntime
                    | SandboxWorkStep::MarkRetired
            ))
}

/// Only enum phase names enter telemetry, never operation authority or credentials.
pub(super) fn phase(sandbox: &Sandbox) -> Option<String> {
    sandbox
        .lifecycle
        .operation
        .as_ref()
        .map(|operation| match operation {
            SandboxLifecycleOperation::Pause { phase, .. } => format!("pause.{phase:?}"),
            SandboxLifecycleOperation::Revive { phase, .. } => format!("revive.{phase:?}"),
            SandboxLifecycleOperation::ImageRotation { phase, .. } => format!("rotation.{phase:?}"),
        })
}
