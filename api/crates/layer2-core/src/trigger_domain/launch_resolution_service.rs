use crate::sandbox::{Agent, AgentOrigin, Sandbox, SandboxStatus, Workspace, WorkspaceStatus};
use crate::{seconds_from_rfc3339, Caller};

use super::launch_service::{ResolvedLaunch, MAX_EXISTING_MESSAGE_EXCERPT_SCALARS};
use super::{
    AgentReusePolicy, ExistingAgentChoice, LaunchResolution, LaunchResolutionError, LaunchService,
    LaunchServiceError, PublicTrigger, RepeatChoice, ResolvedResource, ResolvedTriggerProfiles,
    SandboxReusePolicy, WorkspaceReusePolicy,
};

impl LaunchService {
    pub(super) async fn resolve(
        &self,
        caller: &Caller,
        trigger: &PublicTrigger,
        profiles: &ResolvedTriggerProfiles,
        choice: Option<RepeatChoice>,
    ) -> Result<ResolvedLaunch, LaunchServiceError> {
        let policy = &trigger.revision.content.reuse;
        if policy.agent != AgentReusePolicy::Ask && choice.is_some() {
            return Err(invalid_choice());
        }

        let existing_agent = if policy.agent == AgentReusePolicy::AlwaysNew {
            None
        } else {
            self.contexts
                .newest_live_trigger_agent(&caller.subject, &trigger.trigger_id)
                .await
                .map_err(resolution_error)?
        };
        if let Some(agent) = existing_agent.as_ref() {
            validate_agent(agent, caller, trigger)?;
        }
        match (policy.agent, choice, existing_agent) {
            (AgentReusePolicy::ResumeExisting, None, Some(agent))
            | (AgentReusePolicy::Ask, Some(RepeatChoice::Resume), Some(agent)) => {
                return Ok(ResolvedLaunch::new(existing_resolution(agent), true));
            }
            (AgentReusePolicy::Ask, None, Some(agent)) => {
                return Err(LaunchServiceError::RepeatChoiceRequired {
                    existing: existing_choice(&agent),
                });
            }
            (AgentReusePolicy::Ask, Some(RepeatChoice::New), Some(_))
            | (AgentReusePolicy::Ask, Some(RepeatChoice::New), None)
            | (AgentReusePolicy::ResumeExisting, None, None)
            | (AgentReusePolicy::Ask, None, None)
            | (AgentReusePolicy::AlwaysNew, None, _) => {}
            (AgentReusePolicy::Ask, Some(RepeatChoice::Resume), None)
            | (AgentReusePolicy::AlwaysNew, Some(_), _)
            | (AgentReusePolicy::ResumeExisting, Some(_), _) => return Err(invalid_choice()),
        }

        if policy.sandbox == SandboxReusePolicy::AlwaysNew {
            return Ok(ResolvedLaunch::pending());
        }

        let Some(sandbox) = self
            .contexts
            .newest_matching_sandbox(
                &caller.subject,
                &trigger.revision.content.target.sandbox_profile.id,
            )
            .await
            .map_err(resolution_error)?
        else {
            return Ok(ResolvedLaunch::pending());
        };
        validate_sandbox(&sandbox, caller, profiles)?;
        let mut resolution = LaunchResolution {
            sandbox: ResolvedResource::Reused(sandbox.id.clone()),
            workspace: ResolvedResource::Pending,
            agent: ResolvedResource::Pending,
            choice_required: false,
        };

        if policy.workspace == WorkspaceReusePolicy::AlwaysNew {
            return Ok(ResolvedLaunch::new(resolution, false));
        }

        let Some(workspace) = self
            .contexts
            .matching_workspace(
                &caller.subject,
                &sandbox.id,
                &trigger.revision.content.target.workspace_profile.id,
            )
            .await
            .map_err(resolution_error)?
        else {
            return Ok(ResolvedLaunch::new(resolution, false));
        };
        validate_workspace(&workspace, &sandbox, caller, profiles)?;
        resolution.workspace = ResolvedResource::Reused(workspace.id.clone());

        Ok(ResolvedLaunch::new(resolution, false))
    }
}

fn validate_sandbox(
    sandbox: &Sandbox,
    caller: &Caller,
    profiles: &ResolvedTriggerProfiles,
) -> Result<(), LaunchServiceError> {
    if sandbox.owner != caller.subject
        || sandbox.profile.id != profiles.sandbox.profile_id
        || sandbox.status == SandboxStatus::Retired
    {
        return Err(corrupt("matching Sandbox violates the launch context"));
    }
    Ok(())
}

fn validate_workspace(
    workspace: &Workspace,
    sandbox: &Sandbox,
    caller: &Caller,
    profiles: &ResolvedTriggerProfiles,
) -> Result<(), LaunchServiceError> {
    if workspace.owner != caller.subject
        || workspace.sandbox_id != sandbox.id
        || workspace.profile.id != profiles.workspace.profile_id
        || workspace.status == WorkspaceStatus::Removed
    {
        return Err(corrupt("matching Workspace violates the launch context"));
    }
    Ok(())
}

fn validate_agent(
    agent: &Agent,
    caller: &Caller,
    trigger: &PublicTrigger,
) -> Result<(), LaunchServiceError> {
    let trigger_origin = matches!(
        &agent.origin,
        AgentOrigin::Trigger {
            trigger_id,
            launch_id,
        } if trigger_id == trigger.trigger_id.as_str()
            && !launch_id.trim().is_empty()
            && launch_id.len() <= 128
            && !launch_id.chars().any(char::is_control)
    );
    if agent.owner != caller.subject
        || !agent.status.is_live()
        || !trigger_origin
        || !valid_id(agent.id.as_str(), "agt_")
        || !valid_id(agent.sandbox_id.as_str(), "sbx_")
        || !valid_id(agent.workspace_id.as_str(), "wks_")
        || seconds_from_rfc3339(&agent.created_at).is_none()
    {
        return Err(corrupt("matching Agent violates the launch context"));
    }
    Ok(())
}

fn existing_resolution(agent: Agent) -> LaunchResolution {
    LaunchResolution {
        sandbox: ResolvedResource::Reused(agent.sandbox_id),
        workspace: ResolvedResource::Reused(agent.workspace_id),
        agent: ResolvedResource::Reused(agent.id),
        choice_required: false,
    }
}

fn existing_choice(agent: &Agent) -> ExistingAgentChoice {
    let first_message_excerpt = agent
        .conversation
        .first_message
        .as_ref()
        .map(|message| {
            message
                .text
                .chars()
                .take(MAX_EXISTING_MESSAGE_EXCERPT_SCALARS)
                .collect()
        })
        .unwrap_or_default();
    ExistingAgentChoice {
        agent_id: agent.id.clone(),
        title: agent.title.clone(),
        created_at: agent.created_at.clone(),
        first_message_excerpt,
    }
}

fn invalid_choice() -> LaunchServiceError {
    LaunchServiceError::InvalidRequest("repeat choice is not applicable".into())
}

fn valid_id(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix) && value.len() > prefix.len()
}

fn resolution_error(error: LaunchResolutionError) -> LaunchServiceError {
    match error {
        LaunchResolutionError::Unavailable(detail) => LaunchServiceError::Unavailable(detail),
        LaunchResolutionError::Corrupt(detail) => LaunchServiceError::Corrupt(detail),
    }
}

fn corrupt(detail: &str) -> LaunchServiceError {
    LaunchServiceError::Corrupt(detail.into())
}
