use std::future::Future;

use layer2_core::agent_ownership::observed_transport;
use layer2_core::sandbox::store::{ReopenRefusalFence, SandboxStoreError};
use layer2_core::sandbox::{Agent, Sandbox, SandboxImageRotationPhase, SandboxLifecycleOperation};
use layer3_adapter::ReleasedRuntime;
use tokio::time::{timeout_at, Instant};

use super::super::support::{adapter_failure, WorkerFailure};
use super::reopen_output::output_refused;
use super::SandboxWorker;

impl SandboxWorker {
    pub(super) fn reopen_output_refusal_enabled(&self, sandbox: &Sandbox) -> bool {
        self.reopen_output_refusal_enabled_for(sandbox, self.config.release().as_ref())
    }

    fn reopen_output_refusal_enabled_for(
        &self,
        sandbox: &Sandbox,
        release: Option<&ReleasedRuntime>,
    ) -> bool {
        if !self.config.reopen_output_refusal_enabled || !sandbox.supports_retained_runtime_update()
        {
            return false;
        }
        let Some(SandboxLifecycleOperation::ImageRotation {
            phase: SandboxImageRotationPhase::ConversationReopen,
            target_runtime_version,
            ..
        }) = &sandbox.lifecycle.operation
        else {
            return false;
        };
        let Some(runtime) = &sandbox.runtime else {
            return false;
        };
        runtime.runtime_version == *target_runtime_version
            && sandbox.runtime_update_request().is_some_and(|request| {
                request.started_at.is_some()
                    && request.completed_at.is_none()
                    && request.target_version == *target_runtime_version
            })
            && self.config.default_version(release) == *target_runtime_version
    }

    pub(super) async fn reopen_refusal_fence(
        &self,
        sandbox: &Sandbox,
        agent: &Agent,
    ) -> Result<Option<ReopenRefusalFence>, SandboxStoreError> {
        let released = self.config.release();
        if !self.reopen_output_refusal_enabled_for(sandbox, released.as_ref()) {
            return Ok(None);
        }
        let snapshot = self
            .dependencies
            .store
            .reopen_refusal_snapshot(&agent.id)
            .await?
            .filter(|snapshot| snapshot.agent == *agent)
            .ok_or(SandboxStoreError::Conflict)?;
        Ok(Some(ReopenRefusalFence {
            agent_id: agent.id.clone(),
            agent_revision: agent.revision,
            checkpoint: snapshot.checkpoint.map(Box::new),
            owner: None,
            released,
        }))
    }
}

pub(super) async fn within_deadline<T>(
    deadline: Instant,
    operation: impl Future<Output = layer3_adapter::Result<T>>,
    fence: Option<ReopenRefusalFence>,
    relayed: bool,
) -> Result<T, WorkerFailure> {
    let expired =
        || WorkerFailure::terminal_validation("retained conversation reopen deadline elapsed");
    if Instant::now() >= deadline {
        return Err(expired());
    }
    timeout_at(deadline, operation)
        .await
        .map_err(|_| expired())?
        .map_err(|error| {
            if !output_refused(&error) {
                return adapter_failure(error);
            }
            let Some(mut fence) = fence else {
                return WorkerFailure::retryable("reopen refusal checkpoint could not be verified");
            };
            if relayed {
                let Some(owner) = observed_transport(&fence.agent_id) else {
                    return WorkerFailure::retryable(
                        "reopen refusal owner authority could not be verified",
                    );
                };
                fence.owner = Some(owner);
            }
            let mut failure =
                WorkerFailure::terminal_validation("retained conversation output is discontinuous");
            failure.reopen_refusal = Some(Box::new(fence));
            failure
        })
}
