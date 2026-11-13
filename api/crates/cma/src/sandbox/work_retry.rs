//! Shared worker classification and confirmed-publication telemetry.
use layer2_core::sandbox::work_retry::{
    WorkRetryClass, WorkRetryDecision, WorkRetryFailure, WORK_RETRY_PROBE_INTERVAL,
};
use layer2_core::sandbox::{
    Agent, AgentWorkStep, Sandbox, SandboxLifecycleOperation, SandboxWorkStep,
};
use layer2_core::{rfc3339_at, seconds_from_rfc3339};
use std::time::SystemTime;

pub(crate) const fn agent_class(step: AgentWorkStep) -> WorkRetryClass {
    match step {
        AgentWorkStep::FenceInteraction
        | AgentWorkStep::CloseProcess
        | AgentWorkStep::PublishStopped
        | AgentWorkStep::EnsureProcessClosed
        | AgentWorkStep::RemoveHome
        | AgentWorkStep::RemoveWorktree
        | AgentWorkStep::PublishDeleted => WorkRetryClass::Maintenance,
        _ => WorkRetryClass::Interactive,
    }
}

pub(crate) fn sandbox_class(sandbox: &Sandbox) -> WorkRetryClass {
    sandbox_step_class(
        sandbox.work.step,
        matches!(
            sandbox.lifecycle.operation,
            Some(SandboxLifecycleOperation::Revive { .. })
        ),
    )
}

pub(crate) fn sandbox_step_class(step: SandboxWorkStep, reviving: bool) -> WorkRetryClass {
    match step {
        SandboxWorkStep::ProvisionRuntime
        | SandboxWorkStep::RunInstall
        | SandboxWorkStep::MintInitialCredential
        | SandboxWorkStep::DeliverInitialCredential
        | SandboxWorkStep::CreateProfileWorkspace(_)
        | SandboxWorkStep::Activate
        | SandboxWorkStep::None => WorkRetryClass::Interactive,
        SandboxWorkStep::RunLifecycleOperation if reviving => WorkRetryClass::Interactive,
        _ => WorkRetryClass::Maintenance,
    }
}

pub(crate) const fn sandbox_step(step: SandboxWorkStep) -> &'static str {
    match step {
        SandboxWorkStep::ProvisionRuntime => "provision_runtime",
        SandboxWorkStep::RunInstall => "run_install",
        SandboxWorkStep::MintInitialCredential => "mint_initial_credential",
        SandboxWorkStep::DeliverInitialCredential => "deliver_initial_credential",
        SandboxWorkStep::CreateProfileWorkspace(_) => "create_profile_workspace",
        SandboxWorkStep::Activate => "activate",
        SandboxWorkStep::WaitForCredentialRotation => "wait_for_credential_rotation",
        SandboxWorkStep::MintRotatedCredential => "mint_rotated_credential",
        SandboxWorkStep::DeliverRotatedCredential => "deliver_rotated_credential",
        SandboxWorkStep::RevokeRotatedCredential => "revoke_rotated_credential",
        SandboxWorkStep::FinishCredentialRotation => "finish_credential_rotation",
        SandboxWorkStep::RunLifecycleOperation => "run_lifecycle_operation",
        SandboxWorkStep::StopAgents => "stop_agents",
        SandboxWorkStep::RevokeRetirementCredentials => "revoke_retirement_credentials",
        SandboxWorkStep::TeardownRuntime => "teardown_runtime",
        SandboxWorkStep::MarkRetired => "mark_retired",
        SandboxWorkStep::RetryRetiredRevocations => "retry_retired_revocations",
        SandboxWorkStep::WaitForPurge => "wait_for_purge",
        SandboxWorkStep::Purge => "purge",
        SandboxWorkStep::None => "none",
    }
}

pub(super) fn replaced_runtime(detail: &str) -> bool {
    matches!(
        detail,
        "bound runtime process was replaced; durable recovery is pending"
            | "retiring native runtime process was replaced"
    )
}

/// A prior long due time identifies an already published probe without a field
/// that would make retained work unreadable to an older image.
pub(crate) fn has_probe_delay(updated_at: &str, ready_at: &str) -> bool {
    seconds_from_rfc3339(updated_at)
        .zip(seconds_from_rfc3339(ready_at))
        .is_some_and(|(updated, ready)| {
            ready.saturating_sub(updated) >= WORK_RETRY_PROBE_INTERVAL.as_secs()
        })
}

pub(super) struct RetryTelemetry {
    worker: &'static str,
    step: &'static str,
    object_id: String,
    attempts: u32,
    failure: WorkRetryFailure,
    parked: bool,
    ready_at: String,
}

impl RetryTelemetry {
    pub(super) fn agent(
        agent: &Agent,
        failure: WorkRetryFailure,
        decision: WorkRetryDecision,
        ready_at: SystemTime,
    ) -> Self {
        Self {
            worker: "agent",
            step: agent.work.step.operation_name(),
            object_id: agent.id.to_string(),
            attempts: agent.work.attempts.saturating_add(1),
            failure,
            parked: decision.parked,
            ready_at: rfc3339_at(ready_at),
        }
    }

    pub(super) fn sandbox(
        sandbox: &Sandbox,
        failure: WorkRetryFailure,
        decision: WorkRetryDecision,
        ready_at: SystemTime,
    ) -> Self {
        Self {
            worker: "sandbox",
            step: sandbox_step(sandbox.work.step),
            object_id: sandbox.id.to_string(),
            attempts: sandbox.work.attempts.saturating_add(1),
            failure,
            parked: decision.parked,
            ready_at: rfc3339_at(ready_at),
        }
    }

    /// Call only after the fenced write is known to have committed.
    pub(super) fn published(self) {
        metrics::counter!("cma_work_retries_total", "worker" => self.worker,
            "step" => self.step, "failure_class" => self.failure.label(),
            "parked" => if self.parked { "true" } else { "false" })
        .increment(1);
        if self.parked {
            // Claims update updated_at, so do not pretend this is an exact
            // entry/exit count. Durable occupancy is sampled separately.
            metrics::counter!("cma_work_parked_retries_total", "worker" => self.worker,
                "step" => self.step, "failure_class" => self.failure.label())
            .increment(1);
            tracing::warn!(worker = self.worker, object_id = %self.object_id, step = self.step,
                failure_class = self.failure.label(), attempts = self.attempts,
                next_probe_at = %self.ready_at, "Work retry budget exhausted; pending work parked");
        }
    }
}
