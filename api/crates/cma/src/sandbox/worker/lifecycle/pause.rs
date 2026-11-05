use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    SandboxLifecycleAgentMutation, SandboxLifecycleAgentScope, SandboxStoreError,
};
use layer2_core::sandbox::{
    AgentLogKind, AgentStatus, Sandbox, SandboxEventKind, SandboxStatus, SandboxWorkStep,
};

use super::super::support::{adapter_failure, funding_failure, runtime, WorkerFailure};
use super::super::SandboxWorker;
use super::checkpoint::require_quiet_agent;
use super::support::{lifecycle_lease, model_error};

impl SandboxWorker {
    pub(super) async fn cancel_pause(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        if !self.lifecycle_state_matches(&sandbox).await? {
            return Err(SandboxStoreError::Conflict);
        }
        if !Self::uses_retained_home(&sandbox)
            && self.resume_lifecycle_writers(&sandbox).await.is_err()
        {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::retryable("pause cancellation is waiting for writer resumption"),
                    now,
                )
                .await;
        }
        let rotation_at = if sandbox.credential.source.is_managed() {
            super::super::support::credential_rotation_at(&sandbox)
        } else {
            Some(now)
        }
        .ok_or_else(|| SandboxStoreError::Corrupt("active credential expiry is absent".into()))?;
        let stamp = rfc3339_at(now + started.elapsed());
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.complete_pause_cancellation(
            rfc3339_at(rotation_at),
            &lease.holder,
            lease.fence,
            stamp.clone(),
        )
        .map_err(model_error)?;
        self.commit_lifecycle_phase(
            &sandbox,
            next,
            Vec::new(),
            SandboxLifecycleAgentScope::Touched,
            stamp,
            vec![SandboxEventKind::StatusChanged {
                from: SandboxStatus::Pausing,
                to: SandboxStatus::Active,
            }],
        )
        .await
    }

    pub(super) async fn pause_lifecycle_agents(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let retained_home = Self::uses_retained_home(&sandbox);
        if !retained_home {
            if let Err(failure) = self.ensure_lifecycle_checkpoint_hold(&sandbox).await {
                return self.abort_lifecycle_hold(sandbox, failure, now).await;
            }
        }
        let agents = self.lifecycle_agents(&sandbox).await?;
        if let Some(failure) = agents
            .iter()
            .filter(|agent| agent.status.is_live())
            .find_map(|agent| require_quiet_agent(agent).err())
        {
            return if retained_home {
                self.handle_lifecycle_failure(sandbox, failure, now).await
            } else {
                self.abort_lifecycle_hold(sandbox, failure, now).await
            };
        }

        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.complete_lifecycle_agent_pause(&lease.holder, lease.fence, stamp.clone())
            .map_err(model_error)?;
        let mut events = Vec::new();
        let mut mutations = Vec::new();
        for current in agents.iter().filter(|agent| agent.status.is_live()) {
            let mut agent = current.clone();
            if current.status == AgentStatus::Paused {
                agent.revision = agent
                    .revision
                    .checked_add(1)
                    .ok_or(SandboxStoreError::Conflict)?;
                agent.updated_at = stamp.clone();
                mutations.push(SandboxLifecycleAgentMutation {
                    expected_revision: current.revision,
                    agent,
                    logs: Vec::new(),
                });
                continue;
            }
            agent
                .transition(AgentStatus::Paused, stamp.clone(), None)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            agent.conversation.last_seq = agent
                .conversation
                .last_seq
                .checked_add(1)
                .ok_or(SandboxStoreError::SequenceConflict)?;
            let log = AgentLogKind::StatusChanged {
                from: current.status,
                to: AgentStatus::Paused,
            };
            events.push(SandboxEventKind::AgentStatusChanged {
                agent_id: current.id.clone(),
                from: current.status,
                to: AgentStatus::Paused,
            });
            mutations.push(SandboxLifecycleAgentMutation {
                expected_revision: current.revision,
                agent,
                logs: vec![log],
            });
        }
        events.push(SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunLifecycleOperation,
        });
        let committed = self
            .commit_lifecycle_phase(
                &sandbox,
                next.clone(),
                mutations,
                SandboxLifecycleAgentScope::AllLive,
                stamp,
                events,
            )
            .await;
        if committed.is_ok() || self.lifecycle_state_matches(&next).await? {
            for agent in agents.iter().filter(|agent| agent.status.is_live()) {
                self.dependencies.registry.fence(
                    &agent.id,
                    agent.conversation.runtime_generation,
                    agent.work.next_fence,
                );
            }
            return Ok(());
        }
        if retained_home {
            self.handle_lifecycle_failure(
                sandbox,
                WorkerFailure::retryable("agent parking publication is unconfirmed"),
                now,
            )
            .await
        } else {
            self.fail_after_writer_resume(
                sandbox,
                WorkerFailure::retryable("agent parking publication is unconfirmed"),
                now,
            )
            .await
        }
    }

    pub(super) async fn teardown_lifecycle_runtime(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let retained_home = Self::uses_retained_home(&sandbox);
        if !retained_home {
            if let Err(failure) = self.ensure_lifecycle_checkpoint_hold(&sandbox).await {
                return self.abort_lifecycle_hold(sandbox, failure, now).await;
            }
        }
        let Some(handle) = runtime(&sandbox) else {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::terminal_security("lifecycle runtime binding is absent"),
                    now,
                )
                .await;
        };
        let suspend = crate::sandbox::runtime_attachment::native_pause_teardown(&sandbox);
        let stopped = if suspend {
            if !self.lifecycle_state_matches(&sandbox).await? {
                return Err(SandboxStoreError::Conflict);
            }
            let request = crate::sandbox::runtime_attachment::retained_runtime_request(
                &sandbox,
                sandbox.runtime.as_ref().expect("bound runtime"),
                &self.config.deployment_id,
            );
            match request {
                Ok(request) => self.dependencies.adapter.suspend_runtime(&request).await,
                Err(error) => Err(error),
            }
        } else {
            self.dependencies.adapter.teardown(&handle).await
        };
        if let Err(error) = stopped {
            return if retained_home {
                self.handle_lifecycle_failure(sandbox, adapter_failure(error), now)
                    .await
            } else {
                self.abort_lifecycle_hold(sandbox, adapter_failure(error), now)
                    .await
            };
        }
        if let Err(error) = self.funding.settle(&sandbox, now).await {
            return self
                .handle_lifecycle_failure(sandbox, funding_failure(error), now)
                .await;
        }
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        if suspend {
            next.complete_lifecycle_runtime_suspension(&lease.holder, lease.fence, stamp.clone())
        } else {
            next.complete_lifecycle_runtime_teardown(&lease.holder, lease.fence, stamp.clone())
        }
        .map_err(model_error)?;
        let committed = self
            .complete_sandbox_only_phase(sandbox, next.clone(), stamp)
            .await;
        if committed.is_ok() || self.lifecycle_state_matches(&next).await? {
            Ok(())
        } else {
            committed
        }
    }

    pub(super) async fn publish_paused(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let stamp = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.publish_lifecycle_paused(&lease.holder, lease.fence, stamp.clone())
            .map_err(model_error)?;
        self.commit_lifecycle_phase(
            &sandbox,
            next,
            Vec::new(),
            SandboxLifecycleAgentScope::Touched,
            stamp,
            vec![SandboxEventKind::StatusChanged {
                from: SandboxStatus::Pausing,
                to: SandboxStatus::Paused,
            }],
        )
        .await
    }

    async fn abort_lifecycle_hold(
        &self,
        sandbox: Sandbox,
        failure: WorkerFailure,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.fail_after_writer_resume(sandbox, failure, now).await
    }
}
