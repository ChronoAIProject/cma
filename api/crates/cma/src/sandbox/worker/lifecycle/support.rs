use std::time::{Duration, SystemTime};

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    PageRequest, SandboxLifecycleAgentMutation, SandboxLifecycleAgentScope,
    SandboxLifecycleMutation, SandboxMutationGuard, SandboxStore, SandboxStoreError,
    MAX_SANDBOX_LIFECYCLE_AGENTS,
};
use layer2_core::sandbox::{Agent, Sandbox, SandboxEventKind, SandboxWorkStep};

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
        let recorded_failure = if failure.is_retryable() {
            next.retry_lifecycle(
                rfc3339_at(now + retry_delay),
                &lease.holder,
                lease.fence,
                stamp.clone(),
            )
            .map_err(model_error)?;
            failure.typed()
        } else {
            next.fail_lifecycle(
                failure.detail().into(),
                &lease.holder,
                lease.fence,
                stamp.clone(),
            )
            .map_err(model_error)?
        };
        self.commit_lifecycle_phase(
            &sandbox,
            next,
            Vec::new(),
            SandboxLifecycleAgentScope::Touched,
            stamp,
            vec![SandboxEventKind::WorkStepFailed {
                step: SandboxWorkStep::RunLifecycleOperation,
                failure: recorded_failure,
            }],
        )
        .await
    }

    pub(super) async fn lifecycle_state_matches(
        &self,
        expected: &Sandbox,
    ) -> Result<bool, SandboxStoreError> {
        Ok(
            SandboxStore::get_sandbox(self.dependencies.store.as_ref(), &expected.id)
                .await?
                .is_some_and(|stored| stored == *expected),
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
