//! Replay both prepared resource fences under one fresh runtime observation.
use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{AgentWorkMutation, SandboxStoreError};
use layer2_core::sandbox::{Agent, AgentCleanupDebt, AgentLogKind, AgentWorkStep};
use layer3_adapter::{AgentHomeRequest, WorktreeRequest};

use super::startup_support::valid_worktree_receipt;
use super::support::{
    adapter_failure, bump_log_sequence, claimed_fence, record_receipt, request_evidence,
    runtime_options, AgentContext, RequestEvidence, WorkerFailure,
};
use super::AgentWorker;

impl AgentWorker {
    #[cfg(test)]
    pub(in crate::sandbox) async fn replay_prepared_for_test(
        &self,
        agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let context = self
            .startup_context(&agent)
            .await
            .map_err(|_| SandboxStoreError::Conflict)?;
        self.replay_prepared_resources(agent, context, now).await
    }

    pub(super) async fn replay_prepared_resources(
        &self,
        agent: Agent,
        context: AgentContext,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let mut timing = super::startup_timing::StartupTiming::new(&agent);
        // The store proved the immutable prepared hierarchy. This path still
        // executes both requests: their distinct runtime fences are not receipts.
        if agent.work.resolved_skills.as_deref() != Some(&[])
            || !agent.profile.content.skills.is_empty()
            || !crate::agent_lifecycle::guidance::is_prepared_prompt(
                &agent.profile.content.system_prompt,
            )
        {
            return Err(SandboxStoreError::Conflict);
        }
        let worktree = WorktreeRequest::new(
            format!(
                "worktree:{}:{}:add:v1",
                context.workspace.name, context.worktree.name
            ),
            context.workspace.name.clone(),
            context.worktree.name.clone(),
            context.worktree.base_commit.clone(),
        );
        let home = AgentHomeRequest::new(
            format!("agent:{}:home:create:v1", agent.id),
            runtime_options(&agent, &context.sandbox, &context.worktree),
            agent.profile.content.system_prompt.clone(),
            Vec::new(),
        );
        let evidence = [
            request_evidence(
                AgentWorkStep::AddWorktree,
                &worktree.operation_id,
                &worktree.request_sha256,
            ),
            request_evidence(
                AgentWorkStep::MaterializeHome,
                &home.operation_id,
                &home.request_sha256,
            ),
        ];
        let agent = self
            .prepare_replay_pair(agent, &context, &evidence, now)
            .await?;
        timing.completed("prepared_replay_intent");
        let fence = claimed_fence(&agent, &self.config.holder)?;
        if !super::startup_publication::holds_startup_authority(
            &agent,
            &self.config.holder,
            now + started.elapsed(),
        ) {
            return Err(SandboxStoreError::Conflict);
        }
        let result = if context.agent_resource_preparation {
            // One request retains both fenced effects and both receipts. Any
            // ambiguous failure leaves the paired intents for durable recovery;
            // it must never trigger a speculative fallback to separate requests.
            self.dependencies
                .adapter
                .prepare_agent_resources(&context.runtime, fence, &worktree, &home)
                .await
        } else {
            self.dependencies
                .adapter
                .add_worktree(&context.runtime, fence, &worktree)
                .await
        };
        match result {
            Ok(receipt) if valid_worktree_receipt(&context, &worktree, &receipt) => {}
            Ok(_) => {
                return self
                    .handle_failure(
                        agent,
                        WorkerFailure::terminal(
                            "agent_runtime_receipt_invalid",
                            "prepared worktree replay returned invalid evidence",
                        ),
                        now + started.elapsed(),
                    )
                    .await
            }
            Err(error) => {
                return self
                    .handle_failure(agent, adapter_failure(error), now + started.elapsed())
                    .await
            }
        }
        timing.completed(if context.agent_resource_preparation {
            "prepared_resources_replay"
        } else {
            "prepared_worktree_replay"
        });
        // Retained runtimes keep the second request and its fresh exact Pod proof.
        if !super::startup_publication::holds_startup_authority(
            &agent,
            &self.config.holder,
            now + started.elapsed(),
        ) {
            return Err(SandboxStoreError::Conflict);
        }
        if !context.agent_resource_preparation {
            if let Err(error) = self
                .dependencies
                .adapter
                .materialize_agent_home(&context.runtime, fence, &home)
                .await
            {
                return self
                    .handle_failure(agent, adapter_failure(error), now + started.elapsed())
                    .await;
            }
        }
        if !context.agent_resource_preparation {
            timing.completed("prepared_home_replay");
        }
        let result = self
            .retry_startup_publication(
                agent,
                &context.sandbox,
                now + started.elapsed(),
                |mut agent, at| {
                    let evidence = evidence.clone();
                    async move {
                        let revision = agent.revision;
                        let fence = claimed_fence(&agent, &self.config.holder)?;
                        let stamp = rfc3339_at(at);
                        for item in evidence {
                            // These intermediate steps exist only in this atomic mutation.
                            agent.work.step = item.step;
                            record_receipt(&mut agent, item, &stamp)?;
                        }
                        agent
                            .work
                            .advance(
                                &self.config.holder,
                                fence,
                                AgentWorkStep::OpenConversation,
                                stamp.clone(),
                            )
                            .map_err(|_| SandboxStoreError::Conflict)?;
                        agent.updated_at = stamp.clone();
                        agent.revision =
                            revision.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
                        bump_log_sequence(&mut agent, 2)?;
                        self.dependencies
                            .store
                            .commit_agent_work(AgentWorkMutation {
                                expected_agent_revision: revision,
                                holder: self.config.holder.clone(),
                                fence,
                                agent,
                                worktree: None,
                                appended_at: stamp,
                                sandbox_event: None,
                                agent_logs: vec![
                                    AgentLogKind::WorkStepCompleted {
                                        step: AgentWorkStep::AddWorktree,
                                    },
                                    AgentLogKind::WorkStepCompleted {
                                        step: AgentWorkStep::MaterializeHome,
                                    },
                                ],
                            })
                            .await
                    }
                },
            )
            .await;
        timing.completed("prepared_replay_publication");
        result
    }

    async fn prepare_replay_pair(
        &self,
        agent: Agent,
        context: &AgentContext,
        evidence: &[RequestEvidence; 2],
        now: SystemTime,
    ) -> Result<Agent, SandboxStoreError> {
        self.retry_startup_publication(agent, &context.sandbox, now, |mut agent, at| {
            let debts: Vec<_> = evidence
                .iter()
                .map(|item| AgentCleanupDebt {
                    step: item.step,
                    operation_id: item.operation_id.clone(),
                    request_sha256: item.request_sha256.clone(),
                })
                .collect();
            async move {
                if agent.work.cleanup_debt == debts {
                    return Ok(agent);
                }
                if !agent.work.cleanup_debt.is_empty() || !agent.work.receipts.is_empty() {
                    return Err(SandboxStoreError::Conflict);
                }
                let revision = agent.revision;
                let fence = claimed_fence(&agent, &self.config.holder)?;
                agent.work.cleanup_debt = debts;
                agent.updated_at = rfc3339_at(at);
                agent.revision = revision.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
                self.dependencies
                    .store
                    .commit_agent_work(AgentWorkMutation {
                        expected_agent_revision: revision,
                        holder: self.config.holder.clone(),
                        fence,
                        agent: agent.clone(),
                        worktree: None,
                        appended_at: rfc3339_at(at),
                        sandbox_event: None,
                        agent_logs: Vec::new(),
                    })
                    .await?;
                Ok(agent)
            }
        })
        .await
    }
}
