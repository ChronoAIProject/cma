//! Prepare and publish the original outcome of one failed lifecycle step.
use std::time::Duration;

use super::*;

fn can_rebuild(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::WaitForWorkspace
            | AgentWorkStep::AddWorktree
            | AgentWorkStep::MaterializeHome
            | AgentWorkStep::OpenConversation
            | AgentWorkStep::PublishReady
    )
}

impl AgentWorker {
    pub(in crate::sandbox) async fn handle_failure(
        &self,
        agent: Agent,
        failure: WorkerFailure,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.handle_failure_with_parents(agent, failure, now, None)
            .await
    }

    pub(in crate::sandbox::agent_worker) async fn handle_context_failure(
        &self,
        context: &AgentContext,
        agent: Agent,
        failure: WorkerFailure,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.handle_failure_with_parents(
            agent,
            failure,
            now,
            Some(PublicationParents {
                sandbox: context.sandbox.clone(),
                workspace: context.workspace.clone(),
                worktree: context.worktree.clone(),
            }),
        )
        .await
    }

    // Keep failure publication out of the successful step's stack frame. A
    // step can call this path from several independent error branches.
    #[inline(never)]
    pub(in crate::sandbox::agent_worker) fn handle_failure_with_parents(
        &self,
        agent: Agent,
        failure: WorkerFailure,
        now: SystemTime,
        parents: Option<PublicationParents>,
    ) -> impl std::future::Future<Output = Result<(), SandboxStoreError>> + Send + '_ {
        Box::pin(async move {
            if failure.class == super::super::inspection_park::INSPECTION_HANDOFF {
                return Err(SandboxStoreError::Conflict);
            }
            super::super::ledger_telemetry::observe(&failure);
            if agent.work.step == AgentWorkStep::PublishConnection {
                self.retry_connection_failure(&agent, failure, now).await
            } else if failure.retryable || agent.status != AgentStatus::Starting {
                self.retry(agent, failure, now, parents).await
            } else {
                self.fail_startup(agent, failure, now, parents).await
            }
        })
    }

    async fn retry(
        &self,
        agent: Agent,
        failure: WorkerFailure,
        now: SystemTime,
        parents: Option<PublicationParents>,
    ) -> Result<(), SandboxStoreError> {
        let delay = self.failure_retry_delay(&agent, &failure);
        let rebuild = can_rebuild(agent.work.step);
        self.retry_at_inner(agent, failure, now, now + delay, rebuild, parents)
            .await
    }

    pub(in crate::sandbox::agent_worker) async fn retry_at(
        &self,
        agent: Agent,
        failure: WorkerFailure,
        now: SystemTime,
        ready_at: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.retry_at_inner(agent, failure, now, ready_at, false, None)
            .await
    }

    async fn retry_at_inner(
        &self,
        agent: Agent,
        failure: WorkerFailure,
        now: SystemTime,
        ready_at: SystemTime,
        rebuild: bool,
        parents: Option<PublicationParents>,
    ) -> Result<(), SandboxStoreError> {
        let telemetry = self.config.work_retry_policy_enabled.then(|| {
            crate::sandbox::work_retry::RetryTelemetry::agent(
                &agent,
                failure.retry_kind(),
                self.retry_decision(&agent, &failure, self.config.retry_delay),
                ready_at,
            )
        });
        self.commit_retry_at(agent, failure, now, ready_at, rebuild, parents)
            .await?;
        if let Some(telemetry) = telemetry {
            telemetry.published();
        }
        Ok(())
    }

    async fn commit_retry_at(
        &self,
        agent: Agent,
        failure: WorkerFailure,
        now: SystemTime,
        ready_at: SystemTime,
        rebuild: bool,
        parents: Option<PublicationParents>,
    ) -> Result<(), SandboxStoreError> {
        if failure.is_runtime_hold() {
            return self
                .retry_runtime_hold_at(agent, now, ready_at, failure.is_startup_hold())
                .await;
        }
        if self.retry_marked_capacity_at(&agent, now, ready_at).await? {
            return Ok(());
        }
        if self
            .retry_repeated_failure_at(&agent, &failure, now, ready_at)
            .await?
        {
            return Ok(());
        }
        if rebuild && failure.observation.is_none() {
            let typed = failure.typed();
            // Failure retries must be due strictly after publication at whole-second precision.
            let retry_delay = ready_at
                .duration_since(now)
                .unwrap_or_default()
                .max(Duration::from_secs(1));
            return self
                .publish_lifecycle_work(agent, parents, now, |agent, at, parents| {
                    let typed = typed.clone();
                    async move {
                        // Parent reads and known-abort retries advance publication time.
                        // Keep the selected interval after that committed timestamp.
                        let mutation = self.retry_proposal(agent, typed, at, at + retry_delay)?;
                        self.commit_lifecycle_publication(mutation, parents).await
                    }
                })
                .await;
        }
        let mutation = self.retry_proposal(agent, failure.typed(), now, ready_at)?;
        match failure.observation {
            Some(observed) => {
                self.dependencies
                    .store
                    .commit_agent_work_retry(mutation, observed)
                    .await
            }
            None => self.dependencies.store.commit_agent_work(mutation).await,
        }
    }

    fn retry_proposal(
        &self,
        mut agent: Agent,
        failure: TypedFailure,
        now: SystemTime,
        ready_at: SystemTime,
    ) -> Result<AgentWorkMutation, SandboxStoreError> {
        let expected = agent.revision;
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let step = agent.work.step;
        let stamp = rfc3339_at(now);
        agent
            .work
            .retry(&self.config.holder, fence, rfc3339_at(ready_at))
            .map_err(|_| SandboxStoreError::Conflict)?;
        agent.updated_at = stamp.clone();
        agent.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
        bump_log_sequence(&mut agent, 1)?;
        Ok(AgentWorkMutation {
            expected_agent_revision: expected,
            holder: self.config.holder.clone(),
            fence,
            agent,
            worktree: None,
            appended_at: stamp,
            sandbox_event: None,
            agent_logs: vec![AgentLogKind::WorkStepFailed { step, failure }],
        })
    }

    async fn fail_startup(
        &self,
        agent: Agent,
        failure: WorkerFailure,
        now: SystemTime,
        parents: Option<PublicationParents>,
    ) -> Result<(), SandboxStoreError> {
        let rejected_lineage = if failure.class == "agent_lineage_invalid" {
            if parents.is_some()
                || agent
                    .work
                    .cleanup_debt
                    .iter()
                    .any(|debt| debt.step == agent.work.step)
            {
                return Err(SandboxStoreError::Conflict);
            }
            Some(
                failure
                    .rejected_lineage
                    .as_deref()
                    .copied()
                    .ok_or(SandboxStoreError::Conflict)?,
            )
        } else {
            None
        };
        let typed = failure.typed();
        let mut worktree = match &parents {
            Some(parents) => parents.worktree.clone(),
            None => self
                .dependencies
                .store
                .get_worktree(&agent.worktree_id)
                .await?
                .ok_or(SandboxStoreError::NotFound)?,
        };
        if rejected_lineage.is_some_and(|proof| proof.worktree_revision != worktree.revision) {
            return Err(SandboxStoreError::Conflict);
        }
        let versioned = if worktree.status == WorktreeStatus::Creating {
            let expected_revision = worktree.revision;
            worktree
                .transition(WorktreeStatus::Failed)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            Some(VersionedWorktree {
                expected_revision,
                worktree,
            })
        } else {
            None
        };
        let id = agent.id.clone();
        let runtime_generation = agent.conversation.runtime_generation;
        let fence = claimed_fence(&agent, &self.config.holder)?;
        if rejected_lineage.is_none() && !can_rebuild(agent.work.step) {
            let mutation = self.startup_failure_proposal(agent, typed, versioned, now)?;
            self.dependencies
                .registry
                .fence(&id, runtime_generation, fence);
            return self.dependencies.store.commit_agent_work(mutation).await;
        }
        let publish = |agent, at, parents| {
            let typed = typed.clone();
            let worktree = versioned.clone();
            async move {
                let mutation = self.startup_failure_proposal(agent, typed, worktree, at)?;
                self.commit_lifecycle_publication(mutation, parents).await
            }
        };
        if let Some(rejection) = rejected_lineage {
            self.publish_rejected_lineage(agent, rejection, now, publish)
                .await?;
        } else {
            self.publish_lifecycle_work(agent, parents, now, publish)
                .await?;
        }
        self.dependencies
            .registry
            .fence(&id, runtime_generation, fence);
        Ok(())
    }

    fn startup_failure_proposal(
        &self,
        mut agent: Agent,
        typed: TypedFailure,
        worktree: Option<VersionedWorktree>,
        now: SystemTime,
    ) -> Result<AgentWorkMutation, SandboxStoreError> {
        let expected = agent.revision;
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let step = agent.work.step;
        let stamp = rfc3339_at(now);
        if matches!(
            step,
            AgentWorkStep::OpenConversation | AgentWorkStep::PublishReady
        ) {
            if let Some(first) = agent.conversation.first_message.as_mut() {
                first
                    .fail_startup(stamp.clone())
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            }
        }
        agent
            .work
            .advance(
                &self.config.holder,
                fence,
                AgentWorkStep::None,
                stamp.clone(),
            )
            .map_err(|_| SandboxStoreError::Conflict)?;
        agent
            .fail(typed.clone(), stamp.clone())
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        bump_log_sequence(&mut agent, 1)?;
        let id = agent.id.clone();
        Ok(AgentWorkMutation {
            expected_agent_revision: expected,
            holder: self.config.holder.clone(),
            fence,
            agent,
            worktree,
            appended_at: stamp,
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: id,
                from: AgentStatus::Starting,
                to: AgentStatus::Failed,
            }),
            agent_logs: vec![AgentLogKind::WorkStepFailed {
                step,
                failure: typed,
            }],
        })
    }
}
