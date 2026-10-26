use super::{
    support::{claimed_fence, validate_runtime_skills, validate_skills},
    AgentWorker,
};
use layer2_core::{
    rfc3339_at,
    sandbox::store::{AgentWorkMutation, SandboxStoreError},
    sandbox::{
        Agent, AgentLogKind, AgentSkillInstallStatus as Status, AgentWorkStep, SandboxStatus,
        WorkspaceStatus, WorktreeStatus,
    },
};
use layer3_adapter::{AgentSkillInstallRequest, AgentSlot};
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
            _ => return self.retry_skill(agent, now + started.elapsed()).await,
        };
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
        let request = AgentSkillInstallRequest::new(
            AgentSlot::new(agent.id.as_str()),
            runtime_skills[0].clone(),
        );
        let fence = claimed_fence(&agent, &self.config.holder)?;
        match self
            .dependencies
            .adapter
            .install_agent_skill(&context.runtime, fence, &request)
            .await
        {
            Ok(()) => {
                self.finish_skill(agent, None, now + started.elapsed())
                    .await
            }
            Err(layer3_adapter::AdapterError::Unsupported(_)) => {
                self.finish_skill(
                    agent,
                    Some("runtime_skill_install_unsupported"),
                    now + started.elapsed(),
                )
                .await
            }
            Err(layer3_adapter::AdapterError::Rejected(_)) => {
                self.finish_skill(
                    agent,
                    Some("runtime_skill_install_rejected"),
                    now + started.elapsed(),
                )
                .await
            }
            Err(_) => self.retry_skill(agent, now + started.elapsed()).await,
        }
    }
    async fn retry_skill(&self, agent: Agent, now: SystemTime) -> Result<(), SandboxStoreError> {
        let mut next = agent.clone();
        let fence = claimed_fence(&agent, &self.config.holder)?;
        next.work
            .retry(
                &self.config.holder,
                fence,
                rfc3339_at(now + self.config.retry_delay),
            )
            .map_err(|_| SandboxStoreError::Conflict)?;
        self.commit_skill(agent, next, Vec::new(), now)
            .await
            .map(|_| ())
    }
    async fn finish_skill(
        &self,
        agent: Agent,
        error: Option<&str>,
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
        self.commit_skill(agent, next, vec![log], now)
            .await
            .map(|_| ())
    }
    async fn commit_skill(
        &self,
        current: Agent,
        mut next: Agent,
        logs: Vec<AgentLogKind>,
        now: SystemTime,
    ) -> Result<Agent, SandboxStoreError> {
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
        self.dependencies
            .store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: current.revision,
                holder: self.config.holder.clone(),
                fence: claimed_fence(&current, &self.config.holder)?,
                agent: next.clone(),
                worktree: None,
                appended_at: stamp,
                sandbox_event: None,
                agent_logs: logs,
            })
            .await?;
        Ok(next)
    }
}
