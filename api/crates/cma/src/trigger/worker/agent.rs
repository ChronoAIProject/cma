use std::time::SystemTime;

use layer2_core::profiles::content_digest;
use layer2_core::rfc3339_at;
use layer2_core::sandbox::{Agent, AgentOrigin, AgentStatus, CreateAgent, FirstMessageRecord};
use layer2_core::trigger_domain::{Launch, LaunchEffectKind, LaunchState, ResolvedResource};

use super::driver::{LaunchResourceEffect, LaunchResourceError};
use super::effects::EffectOutcome;
use super::support::{domain_error, missing_resource, valid_origin_id, validate_agent_id};
use super::{FrozenLaunchContext, LaunchWorker, LaunchWorkerError};

impl LaunchWorker {
    pub(super) async fn resolve_agent(
        &self,
        launch: Launch,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        if let Some(effect) = launch.work.effect.as_ref() {
            return if effect.kind == LaunchEffectKind::CreateAgent {
                self.create_agent(launch, context, now).await
            } else {
                self.fail_invalid_agent(launch, now).await
            };
        }
        match &launch.resolution.agent {
            ResolvedResource::Pending => self.create_agent(launch, context, now).await,
            ResolvedResource::Reused(id) => {
                let Some(agent) = self.read_agent(&launch, id, true, context, now).await? else {
                    return Ok(());
                };
                self.record_agent(launch, agent.id, true, now).await
            }
            ResolvedResource::Created(_) => self.fail_invalid_agent(launch, now).await,
        }
    }

    pub(super) async fn wait_for_agent(
        &self,
        launch: Launch,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        if launch.work.effect.is_some() {
            return self.fail_invalid_agent(launch, now).await;
        }
        let (id, reused) = match &launch.resolution.agent {
            ResolvedResource::Reused(id) => (id.clone(), true),
            ResolvedResource::Created(id) => (id.clone(), false),
            ResolvedResource::Pending => return self.fail_invalid_agent(launch, now).await,
        };
        let Some(agent) = self.read_agent(&launch, &id, reused, context, now).await? else {
            return Ok(());
        };
        match agent.status {
            AgentStatus::Starting | AgentStatus::Paused => self.defer(launch, now).await,
            AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval => {
                self.record_agent_ready(launch, &agent, context, now).await
            }
            AgentStatus::Stopped | AgentStatus::Deleted if agent.ready_at.is_some() => {
                self.record_agent_ready(launch, &agent, context, now).await
            }
            AgentStatus::Stopped | AgentStatus::Deleted | AgentStatus::Failed => {
                self.settle_resource_failure(launch, agent_terminal(), now)
                    .await
            }
        }
    }

    async fn create_agent(
        &self,
        launch: Launch,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        if !launch.resolution.agent.is_pending() {
            return self.fail_invalid_agent(launch, now).await;
        }
        let Some(workspace_id) = launch.resolution.workspace.id().cloned() else {
            return self.fail_invalid_agent(launch, now).await;
        };
        let (caller, authority) = self.caller(&launch);
        let first_message = (!context.revision.content.first_message.is_empty())
            .then(|| context.revision.content.first_message.clone());
        let request = CreateAgent {
            profile: context.revision.content.target.agent_profile.clone(),
            title: None,
            first_message,
            origin: AgentOrigin::Trigger {
                trigger_id: launch.trigger_id.to_string(),
                launch_id: launch.id.to_string(),
            },
        };
        let initiation = launch.initiation.clone().map(Box::new);
        let outcome = self
            .apply_effect(
                launch,
                LaunchResourceEffect::CreateAgent {
                    initiation,
                    caller,
                    authority,
                    workspace_id,
                    request,
                    reuse_existing: context.revision.content.reuse.agent
                        == layer2_core::trigger_domain::AgentReusePolicy::ResumeExisting,
                },
                now,
            )
            .await?;
        let EffectOutcome::Applied { launch, result_id } = outcome else {
            return Ok(());
        };
        let mut launch = *launch;
        let agent_id = match validate_agent_id(&result_id) {
            Ok(id) => id,
            Err(failure) => return self.settle_resource_failure(launch, failure, now).await,
        };
        let reused = match self.resources.agent(&agent_id).await {
            Ok(Some(agent)) => {
                !matches!(&agent.origin, AgentOrigin::Trigger { launch_id, .. } if launch_id == launch.id.as_str())
            }
            Ok(None) => {
                return self
                    .settle_resource_failure(launch, missing_resource("agent"), now)
                    .await
            }
            Err(failure) => return self.settle_resource_failure(launch, failure, now).await,
        };
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = launch
            .record_worker_agent(&self.config.holder, fence, agent_id, reused, at.clone())
            .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }

    async fn record_agent(
        &self,
        mut launch: Launch,
        agent_id: layer2_core::AgentId,
        reused: bool,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = launch
            .record_worker_agent(&self.config.holder, fence, agent_id, reused, at.clone())
            .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }

    async fn record_agent_ready(
        &self,
        mut launch: Launch,
        agent: &Agent,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let first_message = match expected_first_message(agent, context) {
            _ if launch.first_message == layer2_core::sandbox::FirstMessageDelivery::Completed
                && matches!(launch.resolution.agent, ResolvedResource::Reused(_)) =>
            {
                None
            }
            Ok(record) => record,
            Err(_) if matches!(launch.resolution.agent, ResolvedResource::Reused(_)) => None,
            Err(failure) => return self.settle_resource_failure(launch, failure, now).await,
        };
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = if first_message.is_none()
            && matches!(launch.resolution.agent, ResolvedResource::Reused(_))
        {
            launch.record_worker_existing_conversation_ready(
                &self.config.holder,
                fence,
                &agent.id,
                at.clone(),
            )
        } else {
            launch.record_worker_agent_ready(
                &self.config.holder,
                fence,
                &agent.id,
                first_message,
                at.clone(),
            )
        }
        .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }

    pub(super) async fn observe_first_message(
        &self,
        mut launch: Launch,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        let (id, reused) = match &launch.resolution.agent {
            ResolvedResource::Reused(id) => (id.clone(), true),
            ResolvedResource::Created(id) => (id.clone(), false),
            ResolvedResource::Pending => return self.fail_invalid_agent(launch, now).await,
        };
        let Some(agent) = self.read_agent(&launch, &id, reused, context, now).await? else {
            return Ok(());
        };
        let record = match expected_first_message(&agent, context) {
            Ok(Some(record)) => record,
            Ok(None) | Err(_) => {
                return self
                    .settle_resource_failure(launch, invalid_first_message(), now)
                    .await
            }
        };
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = launch
            .record_worker_first_message(
                &self.config.holder,
                fence,
                &agent.id,
                record,
                rfc3339_at(now + self.config.poll_delay),
                at.clone(),
            )
            .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }

    async fn read_agent(
        &self,
        launch: &Launch,
        id: &layer2_core::AgentId,
        reused: bool,
        context: &FrozenLaunchContext,
        now: SystemTime,
    ) -> Result<Option<Agent>, LaunchWorkerError> {
        let agent = match self.resources.agent(id).await {
            Ok(Some(agent)) => agent,
            Ok(None) => {
                self.settle_resource_failure(launch.clone(), missing_resource("agent"), now)
                    .await?;
                return Ok(None);
            }
            Err(failure) => {
                self.settle_resource_failure(launch.clone(), failure, now)
                    .await?;
                return Ok(None);
            }
        };
        if !valid_agent(&agent, launch, context, reused) {
            self.settle_resource_failure(launch.clone(), invalid_agent(), now)
                .await?;
            return Ok(None);
        }
        Ok(Some(agent))
    }

    async fn fail_invalid_agent(
        &self,
        launch: Launch,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        self.settle_resource_failure(launch, invalid_agent(), now)
            .await
    }

    pub(super) async fn finish(
        &self,
        mut launch: Launch,
        now: SystemTime,
    ) -> Result<(), LaunchWorkerError> {
        if launch.state == LaunchState::MessagePending {
            let expected = launch.revision;
            let fence = self.fence(&launch)?;
            let at = rfc3339_at(now);
            let events = launch
                .finish_worker_launch(&self.config.holder, fence, at.clone())
                .map_err(domain_error)?;
            self.commit(expected, launch, at, events).await?;
            return Ok(());
        }
        if launch.state != LaunchState::Ready {
            return Err(LaunchWorkerError::Corrupt(
                "Finish work was claimed outside a ready transition".into(),
            ));
        }
        let Some(id) = launch.resolution.agent.id().cloned() else {
            return self.defer(launch, now).await;
        };
        let agent = match self.resources.agent(&id).await {
            Ok(Some(agent)) if valid_ready_agent(&agent, &launch) => agent,
            Ok(_) => {
                return self
                    .settle_resource_failure(launch, invalid_agent(), now)
                    .await
            }
            Err(failure) => return self.settle_resource_failure(launch, failure, now).await,
        };
        if !matches!(agent.status, AgentStatus::Stopped | AgentStatus::Deleted) {
            return self.defer(launch, now).await;
        }
        let expected = launch.revision;
        let fence = self.fence(&launch)?;
        let at = rfc3339_at(now);
        let events = launch
            .complete_worker_launch(
                &self.config.holder,
                fence,
                &agent.id,
                agent.status,
                at.clone(),
            )
            .map_err(domain_error)?;
        self.commit(expected, launch, at, events).await?;
        Ok(())
    }
}

fn valid_agent(
    agent: &Agent,
    launch: &Launch,
    context: &FrozenLaunchContext,
    reused: bool,
) -> bool {
    let profile = &context.profiles.agent;
    let origin_valid = match &agent.origin {
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
        AgentOrigin::Manual
        | AgentOrigin::Lifecycle { .. }
        | AgentOrigin::WorkspaceProfile { .. } => reused,
    };
    launch.resolution.agent.id() == Some(&agent.id)
        && launch.resolution.workspace.id() == Some(&agent.workspace_id)
        && launch.resolution.sandbox.id() == Some(&agent.sandbox_id)
        && agent.owner == launch.owner
        && agent.profile.id == profile.profile_id
        && agent.profile.revision == profile.revision
        && agent.profile.digest == profile.digest
        && agent.profile.content == profile.content
        && origin_valid
        && valid_creation_key(agent)
        && agent.validate_conversation().is_ok()
        && (!matches!(
            agent.status,
            AgentStatus::Ready
                | AgentStatus::Working
                | AgentStatus::WaitingApproval
                | AgentStatus::Paused
        ) || agent.ready_at.is_some())
}

fn valid_ready_agent(agent: &Agent, launch: &Launch) -> bool {
    let origin_valid = matches!(launch.resolution.agent, ResolvedResource::Reused(_))
        || matches!(
            &agent.origin,
            AgentOrigin::Trigger { trigger_id, launch_id }
                if trigger_id == launch.trigger_id.as_str() && valid_origin_id(launch_id)
        );
    launch.resolution.agent.id() == Some(&agent.id)
        && launch.resolution.workspace.id() == Some(&agent.workspace_id)
        && launch.resolution.sandbox.id() == Some(&agent.sandbox_id)
        && agent.owner == launch.owner
        && origin_valid
        && valid_creation_key(agent)
        && agent.validate_conversation().is_ok()
        && agent.ready_at.is_some()
}

pub(super) fn valid_creation_key(agent: &Agent) -> bool {
    let expected = match &agent.origin {
        AgentOrigin::Manual => Some(format!("manual:{}", agent.id)),
        AgentOrigin::Lifecycle { operation_id } if valid_origin_id(operation_id) => {
            content_digest(&agent.origin)
                .ok()
                .map(|digest| format!("lifecycle:{digest}"))
        }
        AgentOrigin::Lifecycle { .. } => None,
        AgentOrigin::WorkspaceProfile { index } => {
            Some(format!("workspace-profile:{}:{index}", agent.workspace_id))
        }
        AgentOrigin::Trigger { .. } => content_digest(&agent.origin).ok().map(|digest| {
            if agent.creation_key.starts_with("trigger-profile:") {
                format!("trigger-profile:{digest}")
            } else {
                format!("trigger:{digest}")
            }
        }),
    };
    expected.as_deref() == Some(agent.creation_key.as_str())
}

fn expected_first_message<'a>(
    agent: &'a Agent,
    context: &FrozenLaunchContext,
) -> Result<Option<&'a FirstMessageRecord>, LaunchResourceError> {
    let expected = &context.revision.content.first_message;
    match (
        expected.is_empty(),
        agent.conversation.first_message.as_ref(),
    ) {
        (true, None) => Ok(None),
        (false, Some(record))
            if record.text == *expected
                && record.origin_key == agent.creation_key
                && record.validate().is_ok() =>
        {
            Ok(Some(record))
        }
        _ => Err(invalid_first_message()),
    }
}

fn invalid_agent() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "launch_agent_invalid",
        "The Agent selected for this launch is not usable",
    )
}

fn invalid_first_message() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "launch_first_message_invalid",
        "First-message delivery evidence is not usable",
    )
}

fn agent_terminal() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "agent_start_failed",
        "The Agent could not be started for this launch",
    )
}
