use std::time::{Duration, SystemTime};

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    PageRequest, SandboxLifecycleAgentMutation, SandboxLifecycleAgentScope,
    SandboxLifecycleMutation, SandboxMutationGuard, SandboxStore, SandboxStoreError,
    MAX_SANDBOX_LIFECYCLE_AGENTS,
};
use layer2_core::sandbox::{Agent, Sandbox, SandboxEventKind, SandboxStatus, SandboxWorkStep};

use super::super::support::WorkerFailure;
use super::super::SandboxWorker;

impl SandboxWorker {
    pub(super) async fn lifecycle_agents(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Vec<Agent>, SandboxStoreError> {
        let mut agents = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .dependencies
                .store
                .sandbox_agents(
                    &sandbox.id,
                    &PageRequest::new(200, cursor).map_err(|_| SandboxStoreError::InvalidPage)?,
                )
                .await?;
            agents.extend(page.items);
            if agents.len() > MAX_SANDBOX_LIFECYCLE_AGENTS {
                return Err(SandboxStoreError::Corrupt(
                    "sandbox has too many lifecycle agents".into(),
                ));
            }
            let Some(next) = page.next else {
                break;
            };
            cursor = Some(next);
        }
        if agents
            .iter()
            .any(|agent| agent.sandbox_id != sandbox.id || agent.owner != sandbox.owner)
        {
            return Err(SandboxStoreError::Corrupt(
                "sandbox lifecycle agent lineage is inconsistent".into(),
            ));
        }
        Ok(agents)
    }

    pub(super) async fn commit_lifecycle_phase(
        &self,
        current: &Sandbox,
        next: Sandbox,
        agents: Vec<SandboxLifecycleAgentMutation>,
        agent_scope: SandboxLifecycleAgentScope,
        at: String,
        events: Vec<SandboxEventKind>,
    ) -> Result<(), SandboxStoreError> {
        let mutation = SandboxLifecycleMutation {
            guard: SandboxMutationGuard::capture(current),
            sandbox: next,
            agents,
            agent_scope,
            appended_at: at,
            events,
        };
        // Pure retry publication has no runtime/credential effect and remains
        // possible after human authority disappears. Every advancing revival
        // phase fences the same human grant inside its Agent/Sandbox commit.
        let retry = mutation.sandbox.lifecycle == current.lifecycle
            && mutation.sandbox.runtime == current.runtime
            && mutation.sandbox.credential == current.credential
            && mutation.sandbox.status == current.status
            && mutation.agents.is_empty()
            && mutation
                .events
                .iter()
                .all(|event| matches!(event, SandboxEventKind::WorkStepFailed { .. }));
        if matches!(
            current.lifecycle.operation,
            Some(layer2_core::sandbox::SandboxLifecycleOperation::Revive { .. })
        ) && !retry
        {
            match self
                .dependencies
                .credentials
                .commit_channel_phase(current, &mutation)
                .await
            {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(_) => {
                    return Err(SandboxStoreError::Unavailable(
                        "Channel revival publication requires current original human authority"
                            .into(),
                    ))
                }
            }
        }
        self.dependencies
            .store
            .commit_sandbox_lifecycle(mutation)
            .await
    }

    pub(super) async fn complete_sandbox_only_phase(
        &self,
        current: Sandbox,
        next: Sandbox,
        at: String,
    ) -> Result<(), SandboxStoreError> {
        self.commit_lifecycle_phase(
            &current,
            next,
            Vec::new(),
            SandboxLifecycleAgentScope::Touched,
            at,
            vec![SandboxEventKind::WorkStepCompleted {
                step: SandboxWorkStep::RunLifecycleOperation,
            }],
        )
        .await
    }

    pub(in crate::sandbox::worker) async fn handle_lifecycle_failure(
        &self,
        sandbox: Sandbox,
        failure: WorkerFailure,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.handle_lifecycle_failure_after(sandbox, failure, now, self.config.retry_delay)
            .await
    }

    pub(super) async fn handle_lifecycle_failure_after(
        &self,
        sandbox: Sandbox,
        failure: WorkerFailure,
        now: SystemTime,
        retry_delay: Duration,
    ) -> Result<(), SandboxStoreError> {
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        let decision = self.retry_decision(&sandbox, &failure, retry_delay);
        let telemetry = if failure.is_retryable() {
            self.retry_telemetry(&sandbox, &failure, decision, now)
        } else {
            None
        };
        let park_rotation = self.should_park_rotation(&sandbox, decision);
        let recorded_failure = if failure.is_retryable() && !park_rotation {
            next.retry_lifecycle(
                rfc3339_at(now + decision.delay),
                &lease.holder,
                lease.fence,
                stamp.clone(),
            )
            .map_err(model_error)?;
            failure.typed()
        } else if sandbox
            .runtime_update_request()
            .is_some_and(|request| request.completed_at.is_none())
            && sandbox.admitted_runtime_update_target().is_some()
        {
            let typed = failure.typed();
            next.park_failed_runtime_update(
                typed.clone(),
                &lease.holder,
                lease.fence,
                stamp.clone(),
            )
            .map_err(model_error)?;
            typed
        } else {
            next.fail_lifecycle(
                failure.detail().into(),
                &lease.holder,
                lease.fence,
                stamp.clone(),
            )
            .map_err(model_error)?
        };
        let event = if sandbox.status != next.status && next.status == SandboxStatus::Failed {
            SandboxEventKind::Failed {
                failure: recorded_failure,
            }
        } else {
            SandboxEventKind::WorkStepFailed {
                step: SandboxWorkStep::RunLifecycleOperation,
                failure: recorded_failure,
            }
        };
        if let Some(refusal) = failure.reopen_refusal {
            if !self.reopen_output_refusal_enabled(&sandbox) {
                return Err(SandboxStoreError::Conflict);
            }
            let result = self
                .dependencies
                .store
                .park_sandbox_reopen_refusal(
                    SandboxLifecycleMutation {
                        guard: SandboxMutationGuard::capture(&sandbox),
                        sandbox: next,
                        agents: Vec::new(),
                        agent_scope: SandboxLifecycleAgentScope::Touched,
                        appended_at: stamp,
                        events: vec![event],
                    },
                    *refusal,
                )
                .await;
            if result == Err(SandboxStoreError::Conflict) {
                if let Some(source) = &self.config.released {
                    source.refresh_after_stale().await;
                }
            }
            return result;
        }
        self.commit_lifecycle_phase(
            &sandbox,
            next,
            Vec::new(),
            SandboxLifecycleAgentScope::Touched,
            stamp,
            vec![event],
        )
        .await?;
        if let Some(telemetry) = telemetry {
            telemetry.published();
        }
        Ok(())
    }

    pub(super) fn should_park_rotation(
        &self,
        sandbox: &Sandbox,
        decision: layer2_core::sandbox::work_retry::WorkRetryDecision,
    ) -> bool {
        decision.parked
            && Self::is_image_rotation(sandbox)
            && sandbox.runtime_update_request().is_some_and(|request| {
                request.completed_at.is_none()
                    && sandbox.admitted_runtime_update_target()
                        == Some(request.target_version.as_str())
            })
    }

    pub(super) async fn lifecycle_state_matches(
        &self,
        expected: &Sandbox,
    ) -> Result<bool, SandboxStoreError> {
        Ok(
            SandboxStore::get_sandbox(self.dependencies.store.as_ref(), &expected.id)
                .await?
                .is_some_and(|stored| {
                    layer2_core::sandbox::store::sandbox_content_matches(Some(&stored), expected)
                }),
        )
    }
}

pub(super) fn lifecycle_lease<'a>(
    sandbox: &'a Sandbox,
    holder: &str,
) -> Result<&'a layer2_core::sandbox::SandboxWorkLease, SandboxStoreError> {
    sandbox
        .work
        .lease
        .as_ref()
        .filter(|lease| lease.holder == holder)
        .ok_or(SandboxStoreError::Conflict)
}

pub(super) fn model_error(error: layer2_core::sandbox::SandboxModelError) -> SandboxStoreError {
    SandboxStoreError::Corrupt(error.to_string())
}

pub(super) fn operation_id(sandbox: &Sandbox, action: &str) -> Result<String, SandboxStoreError> {
    let authority = sandbox
        .lifecycle
        .operation
        .as_ref()
        .map(|operation| operation.authority())
        .ok_or_else(|| SandboxStoreError::Corrupt("lifecycle authority is absent".into()))?;
    Ok(format!("{}:{action}", authority.operation_id))
}

pub(super) fn replacement_runtime_version(sandbox: &Sandbox, deployed: &str) -> String {
    sandbox
        .admitted_runtime_update_target()
        .map(str::to_owned)
        .or_else(|| {
            sandbox
                .lifecycle
                .suspended_runtime
                .as_ref()
                .map(|binding| binding.runtime_version.clone())
        })
        .or_else(|| sandbox.effective_custom_runtime_image().map(str::to_owned))
        .or_else(|| sandbox.profile.content.runtime_image.clone())
        .unwrap_or_else(|| deployed.to_owned())
}
