use super::{
    support::{claimed_fence, validate_runtime_skills, validate_skills},
    AgentWorker,
};
use layer2_core::{
    rfc3339_at,
    sandbox::store::{AgentSkillResultGuard, AgentWorkMutation, SandboxStoreError},
    sandbox::{
        Agent, AgentLogKind, AgentSkillInstallStatus as Status, AgentWorkStep, SandboxStatus,
        WorkspaceStatus, WorktreeStatus,
    },
};
#[path = "skill_publication.rs"]
mod publication;
use publication::SkillPublication;

#[path = "skill_request.rs"]
mod skill_request;
use std::time::SystemTime;

impl AgentWorker {
    pub(super) async fn install_skill(
        &self,
        mut agent: Agent,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let context = match self.context(&agent).await {
            Ok(context)
                if (matches!(
                    context.sandbox.status,
                    SandboxStatus::Active | SandboxStatus::CredentialRotating
                ) || context.sandbox.restored_connection_is_ready(&agent))
                    && context.sandbox.credential.delivered
                    && context.workspace.status == WorkspaceStatus::Ready
                    && context.worktree.status == WorktreeStatus::Ready =>
            {
                context
            }
            Err(failure) if failure.is_runtime_hold() => {
                return self
                    .handle_failure(agent, failure, now + started.elapsed())
                    .await;
            }
            _ => return self.retry_skill(agent, now + started.elapsed()).await,
        };
        let guard = AgentSkillResultGuard::capture(
            &agent,
            &context.sandbox,
            &context.workspace,
            &context.worktree,
        )?;
        let installation = agent.work.live_skills.last().cloned().ok_or_else(|| {
            SandboxStoreError::Corrupt("skill work has no durable request".into())
        })?;
        let references = vec![installation.reference.clone()];
        let material = match installation.material {
            Some(material) => vec![material],
            None => match self
                .dependencies
                .skills
                .resolve(&context.sandbox, &references)
                .await
            {
                Ok(material) => material,
                Err(super::super::agent_skills::AgentSkillResolutionError::Unavailable(_)) => {
                    return self.retry_skill(agent, now + started.elapsed()).await
                }
                Err(_) => {
                    return self
                        .finish_skill(
                            agent,
                            Some("skill_not_accessible_or_version_invalid"),
                            &guard,
                            now + started.elapsed(),
                        )
                        .await
                }
            },
        };
        if validate_skills(&references, &material).is_err()
            || material.first().is_none_or(|skill| {
                references[0].version.as_deref() != Some(skill.version.as_str())
            })
        {
            return self
                .finish_skill(
                    agent,
                    Some("skill_material_invalid"),
                    &guard,
                    now + started.elapsed(),
                )
                .await;
        }
        let runtime_skills = match self.dependencies.skills.runtime_skills(&material) {
            Ok(skills) if validate_runtime_skills(&material, &skills).is_ok() => skills,
            _ => {
                return self
                    .finish_skill(
                        agent,
                        Some("skill_material_invalid"),
                        &guard,
                        now + started.elapsed(),
                    )
                    .await
            }
        };
        // Commit immutable material before the first runtime request. A worker crash
        // or lost response subsequently replays the identical operation and hash.
        if agent
            .work
            .live_skills
            .last()
            .expect("present")
            .material
            .is_none()
        {
            let previous = agent.clone();
            agent.work.live_skills.last_mut().expect("present").material =
                Some(material[0].clone());
            agent = self
                .commit_skill(previous, agent, Vec::new(), now + started.elapsed())
                .await?;
        }
        let previous = match &installation.previous {
            Some(material) => match self
                .dependencies
                .skills
                .runtime_skills(std::slice::from_ref(material))
            {
                Ok(skills)
                    if validate_runtime_skills(std::slice::from_ref(material), &skills).is_ok() =>
                {
                    Some(skills[0].clone())
                }
                _ => {
                    return self
                        .finish_skill(
                            agent,
                            Some("skill_material_invalid"),
                            &guard,
                            now + started.elapsed(),
                        )
                        .await
                }
            },
            None => None,
        };
        let request = skill_request::request(&agent, runtime_skills[0].clone(), previous);
        let fence = claimed_fence(&agent, &self.config.holder)?;
        let observed = SkillPublication::capture(&agent, &context)?;
        let error = match self
            .dependencies
            .adapter
            .install_agent_skill(&context.runtime, fence, &request)
            .await
        {
            Ok(()) => None,
            Err(layer3_adapter::AdapterError::Unsupported(_)) => Some(
                if request.upgrade.is_some() {
                    "runtime_skill_version_upgrade_unsupported"
                } else {
                    "runtime_skill_install_unsupported"
                }
                .to_owned(),
            ),
            Err(layer3_adapter::AdapterError::SkillUpgradeRefused(reason)) => {
                Some(format!("{}: {reason}", reason.code()))
            }
            Err(layer3_adapter::AdapterError::Rejected(_)) => Some(
                if request.upgrade.is_some() {
                    "runtime_skill_upgrade_rejected"
                } else {
                    "runtime_skill_install_rejected"
                }
                .to_owned(),
            ),
            Err(layer3_adapter::AdapterError::OperationIdentityConflict(_))
                if request.upgrade.is_some() =>
            {
                Some("runtime_skill_upgrade_conflict".to_owned())
            }
            Err(
                error @ (layer3_adapter::AdapterError::RuntimeLedgerHold(_)
                | layer3_adapter::AdapterError::RuntimeStartupHold(_)),
            ) => {
                return self
                    .handle_failure(
                        agent,
                        super::support::adapter_failure(error),
                        now + started.elapsed(),
                    )
                    .await;
            }
            Err(_) => return self.retry_skill(agent, now + started.elapsed()).await,
        };
        self.publish_skill_result(observed, error.as_deref(), now + started.elapsed())
            .await
    }
    async fn retry_skill(&self, agent: Agent, now: SystemTime) -> Result<(), SandboxStoreError> {
        // Lifecycle reconnection can reset work-step attempts; the admitted skill
        // owns its preparation budget. Pinned material requires runtime evidence.
        let preparing = agent
            .work
            .live_skills
            .last()
            .filter(|item| item.previous.is_some() && item.material.is_none());
        if preparing.is_some_and(|item| {
            item.preparation_retries
                >= layer3_adapter::MAX_AGENT_SKILL_UPGRADE_PREPARATION_ATTEMPTS - 1
        }) {
            let context = self
                .context(&agent)
                .await
                .map_err(|_| SandboxStoreError::Conflict)?;
            let guard = AgentSkillResultGuard::capture(
                &agent,
                &context.sandbox,
                &context.workspace,
                &context.worktree,
            )?;
            let reason = layer3_adapter::AgentSkillUpgradeRefusal::PreparationExhausted;
            return self
                .finish_skill(
                    agent,
                    Some(&format!("{}: {reason}", reason.code())),
                    &guard,
                    now,
                )
                .await;
        }
        let failure = super::support::WorkerFailure::retryable(
            "agent_skill_installation_unavailable",
            "live skill installation requires retry",
        );
        let decision = self.retry_decision(&agent, &failure, self.config.retry_delay);
        let telemetry = self.config.work_retry_policy_enabled.then(|| {
            crate::sandbox::work_retry::RetryTelemetry::agent(
                &agent,
                failure.retry_kind(),
                decision,
                now + decision.delay,
            )
        });
        let mut next = agent.clone();
        if preparing.is_some() {
            next.work
                .live_skills
                .last_mut()
                .expect("present")
                .preparation_retries += 1;
        }
        let fence = claimed_fence(&agent, &self.config.holder)?;
        next.work
            .retry(&self.config.holder, fence, rfc3339_at(now + decision.delay))
            .map_err(|_| SandboxStoreError::Conflict)?;
        self.commit_skill(agent, next, Vec::new(), now).await?;
        if let Some(telemetry) = telemetry {
            telemetry.published();
        }
        Ok(())
    }
    async fn finish_skill(
        &self,
        agent: Agent,
        error: Option<&str>,
        guard: &AgentSkillResultGuard,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let mut next = agent.clone();
        let installation = next
            .work
            .live_skills
            .last_mut()
            .expect("durable skill intent");
        installation.status = if error.is_some() {
            Status::Failed
        } else {
            Status::Installed
        };
        installation.error = error.map(str::to_owned);
        let log = AgentLogKind::SkillInstallationChanged {
            skill_id: installation.reference.id.as_str().into(),
            status: installation.status,
        };
        next.work.schedule(AgentWorkStep::None, rfc3339_at(now));
        let mutation = self.skill_mutation(agent, next, vec![log], now)?;
        self.dependencies
            .store
            .commit_agent_skill_result(mutation, guard.clone())
            .await
    }
    async fn commit_skill(
        &self,
        current: Agent,
        next: Agent,
        logs: Vec<AgentLogKind>,
        now: SystemTime,
    ) -> Result<Agent, SandboxStoreError> {
        let mutation = self.skill_mutation(current, next, logs, now)?;
        let next = mutation.agent.clone();
        self.dependencies.store.commit_agent_work(mutation).await?;
        Ok(next)
    }

    fn skill_mutation(
        &self,
        current: Agent,
        mut next: Agent,
        logs: Vec<AgentLogKind>,
        now: SystemTime,
    ) -> Result<AgentWorkMutation, SandboxStoreError> {
        let stamp = rfc3339_at(now);
        next.updated_at = stamp.clone();
        next.revision = current
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        next.conversation.last_seq = current
            .conversation
            .last_seq
            .checked_add(logs.len() as u64)
            .ok_or(SandboxStoreError::Conflict)?;
        Ok(AgentWorkMutation {
            expected_agent_revision: current.revision,
            holder: self.config.holder.clone(),
            fence: claimed_fence(&current, &self.config.holder)?,
            agent: next,
            worktree: None,
            appended_at: stamp,
            sandbox_event: None,
            agent_logs: logs,
        })
    }
}
