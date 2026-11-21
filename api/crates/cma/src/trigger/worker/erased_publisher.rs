//! An existing visitor Agent supplies its retained pins after publisher erasure.
use super::agent::valid_creation_key;
use super::support::{corrupt, missing_resource, valid_origin_id};
use super::{LaunchResourceError, LaunchWorker, LaunchWorkerError};
use layer2_core::profiles::{
    content_digest, AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef,
};
use layer2_core::sandbox::{Agent, AgentOrigin};
use layer2_core::trigger_domain::{
    Launch, LaunchEffectKind, LaunchEffectState, LaunchWorkStep, ResolvedResource, TriggerTarget,
};

pub(super) fn retired() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "trigger_retired",
        "The Trigger publisher is unavailable; choose another Trigger to start a new Launch",
    )
}

impl LaunchWorker {
    pub(super) async fn recheck_publisher_refusal(
        &self,
        launch: &Launch,
        failure: LaunchResourceError,
    ) -> Result<LaunchResourceError, LaunchWorkerError> {
        if !matches!(
            failure.code,
            "sandbox_launch_rejected" | "workspace_launch_rejected" | "agent_launch_rejected"
        ) {
            return Ok(failure);
        }
        // Temporary uncertainty must retain the retry path for an admission
        // that may already have committed. Only terminal creation refusals are
        // attributed to an independently observed erased publisher.
        if failure.retryable {
            return Ok(failure);
        }
        self.recheck_publisher_erasure(launch, failure).await
    }

    pub(super) async fn missing_trigger_revision(
        &self,
        launch: &Launch,
    ) -> Result<LaunchResourceError, LaunchWorkerError> {
        match self.resources.trigger_publisher_erased(launch).await {
            Ok(true) => Ok(retired()),
            Ok(false) => Err(corrupt("frozen Trigger revision is unexpectedly missing")),
            Err(failure) if failure.retryable => Ok(failure),
            Err(_) => Err(corrupt("publisher erasure evidence is not usable")),
        }
    }

    pub(super) async fn recheck_publisher_erasure(
        &self,
        launch: &Launch,
        failure: LaunchResourceError,
    ) -> Result<LaunchResourceError, LaunchWorkerError> {
        let missing = match self
            .resources
            .trigger_revision(&launch.trigger_id, &launch.revision_digest)
            .await
        {
            Ok(revision) => revision.is_none(),
            Err(failure) if failure.retryable => return Ok(failure),
            Err(_) => return Err(corrupt("frozen Trigger revision is not usable")),
        };
        match self.resources.trigger_publisher_erased(launch).await {
            Ok(true) => Ok(retired()),
            Ok(false) if missing => Err(corrupt("frozen Trigger revision is unexpectedly missing")),
            Ok(false) => Ok(failure),
            Err(failure) if failure.retryable => Ok(failure),
            Err(_) => Err(corrupt("publisher erasure evidence is not usable")),
        }
    }

    pub(super) async fn retained_agent_target(
        &self,
        launch: &Launch,
    ) -> Result<TriggerTarget, LaunchResourceError> {
        let (Some(sandbox_id), Some(workspace_id)) = (
            launch.resolution.sandbox.id(),
            launch.resolution.workspace.id(),
        ) else {
            return Err(retired());
        };
        let (agent, reused) = self.retained_agent(launch).await?;
        let valid_origin = match &agent.origin {
            AgentOrigin::Trigger {
                trigger_id,
                launch_id,
            } => {
                trigger_id == launch.trigger_id.as_str()
                    && if reused {
                        valid_origin_id(launch_id)
                    } else {
                        launch_id == launch.id.as_str()
                    }
            }
            _ => reused,
        };
        let sandbox = self
            .resources
            .sandbox(sandbox_id)
            .await?
            .ok_or_else(|| missing_resource("sandbox"))?;
        let workspace = self
            .resources
            .workspace(workspace_id)
            .await?
            .ok_or_else(|| missing_resource("workspace"))?;
        if agent.owner != launch.owner
            || agent.sandbox_id != *sandbox_id
            || agent.workspace_id != *workspace_id
            || !valid_origin
            || !valid_creation_key(&agent)
            || agent.validate_conversation().is_err()
            || sandbox.id != *sandbox_id
            || sandbox.owner != launch.owner
            || workspace.id != *workspace_id
            || workspace.owner != launch.owner
            || workspace.sandbox_id != *sandbox_id
            || content_digest(&agent.profile.content).ok().as_ref() != Some(&agent.profile.digest)
            || content_digest(&sandbox.profile.content).ok().as_ref()
                != Some(&sandbox.profile.digest)
            || content_digest(&workspace.profile.content).ok().as_ref()
                != Some(&workspace.profile.digest)
        {
            return Err(LaunchResourceError::terminal(
                "launch_existing_conversation_invalid",
                "The retained conversation selected for this Launch is not usable",
            ));
        }
        Ok(TriggerTarget {
            sandbox_profile: SandboxProfileRef::new(sandbox.profile.id, sandbox.profile.revision),
            workspace_profile: WorkspaceProfileRef::new(
                workspace.profile.id,
                workspace.profile.revision,
            ),
            agent_profile: AgentProfileRef::new(agent.profile.id, agent.profile.revision),
            inline: None,
        })
    }

    async fn retained_agent(&self, launch: &Launch) -> Result<(Agent, bool), LaunchResourceError> {
        if let Some(id) = launch.resolution.agent.id() {
            let agent = self
                .resources
                .agent(id)
                .await?
                .ok_or_else(|| missing_resource("agent"))?;
            if agent.id != *id {
                return Err(invalid_existing());
            }
            return Ok((
                agent,
                matches!(launch.resolution.agent, ResolvedResource::Reused(_)),
            ));
        }
        let Some(effect) = launch.work.effect.as_ref().filter(|effect| {
            launch.work.step == LaunchWorkStep::ResolveAgent
                && effect.kind == LaunchEffectKind::CreateAgent
                && matches!(
                    effect.state,
                    LaunchEffectState::Applied
                        | LaunchEffectState::Executing
                        | LaunchEffectState::RecoveryRequired
                )
        }) else {
            return Err(retired());
        };
        let agent = self
            .resources
            .admitted_trigger_agent(launch)
            .await?
            .ok_or_else(retired)?;
        if effect.state == LaunchEffectState::Applied
            && effect.result_id.as_deref() != Some(agent.id.as_str())
        {
            return Err(invalid_existing());
        }
        let created_here = matches!(&agent.origin, AgentOrigin::Trigger {trigger_id,launch_id}
            if trigger_id==launch.trigger_id.as_str() && launch_id==launch.id.as_str());
        Ok((agent, !created_here))
    }

    /// Recover only an already committed, exact admission. The empty publisher
    /// tombstone can never authorize a new CreateAgent effect.
    pub(super) async fn recover_retained_agent_effect(
        &self,
        mut launch: Launch,
        now: std::time::SystemTime,
    ) -> Result<Option<Launch>, LaunchWorkerError> {
        let (agent, _) = match self.retained_agent(&launch).await {
            Ok(agent) => agent,
            Err(error) => {
                self.settle_resource_failure(launch, error, now).await?;
                return Ok(None);
            }
        };
        let Some(effect) = launch.work.effect.as_ref() else {
            self.settle_resource_failure(launch, retired(), now).await?;
            return Ok(None);
        };
        if effect.state == LaunchEffectState::Applied {
            return Ok(Some(launch));
        }
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = layer2_core::rfc3339_at(now);
        let events = if effect.state == LaunchEffectState::RecoveryRequired {
            launch
                .reconcile_worker_effect(
                    &self.config.holder,
                    fence,
                    LaunchEffectKind::CreateAgent,
                    agent.id.to_string(),
                    at.clone(),
                )
                .map_err(super::support::domain_error)?
        } else {
            launch
                .mark_worker_effect_applied(
                    &self.config.holder,
                    fence,
                    agent.id.to_string(),
                    at.clone(),
                )
                .map_err(super::support::domain_error)?;
            vec![]
        };
        Ok(Some(self.commit(expected, launch, at, events).await?))
    }
}

fn invalid_existing() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "launch_existing_conversation_invalid",
        "The retained conversation selected for this Launch is not usable",
    )
}
