use layer2_core::profiles::{
    content_digest, AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef,
};
use layer2_core::sandbox::{AgentOrigin, FirstMessageDelivery};
use layer2_core::trigger_domain::{Launch, ResolvedResource, TriggerTarget};

use super::agent::valid_creation_key;
use super::support::{missing_resource, valid_origin_id};
use super::{LaunchResourceError, LaunchWorker};

impl LaunchWorker {
    pub(super) async fn existing_conversation_target(
        &self,
        launch: &Launch,
    ) -> Result<Option<TriggerTarget>, LaunchResourceError> {
        let (
            ResolvedResource::Reused(sandbox_id),
            ResolvedResource::Reused(workspace_id),
            ResolvedResource::Reused(agent_id),
        ) = (
            &launch.resolution.sandbox,
            &launch.resolution.workspace,
            &launch.resolution.agent,
        )
        else {
            return Ok(None);
        };
        if launch.first_message != FirstMessageDelivery::Completed {
            return Ok(None);
        }
        let agent = self
            .resources
            .agent(agent_id)
            .await?
            .ok_or_else(|| missing_resource("agent"))?;
        let origin_matches = match &agent.origin {
            AgentOrigin::Trigger {
                trigger_id,
                launch_id,
            } => trigger_id == launch.trigger_id.as_str() && valid_origin_id(launch_id),
            AgentOrigin::WorkspaceProfile { .. } if agent.work.warm_conversation_id.is_some() => {
                self.launches
                    .source_launch(&launch.owner, &agent.id, Some(&launch.trigger_id))
                    .await
                    .map_err(|_| {
                        LaunchResourceError::retryable(
                            "launch_source_unavailable",
                            "The existing conversation source could not be read",
                        )
                    })?
                    .is_some_and(|source| {
                        source.trigger_id == launch.trigger_id
                            && source.resolved_agent_matches(&agent)
                    })
            }
            _ => return Ok(None),
        };
        if agent.owner != launch.owner
            || agent.id != *agent_id
            || agent.sandbox_id != *sandbox_id
            || agent.workspace_id != *workspace_id
            || !origin_matches
            || !valid_creation_key(&agent)
            || agent.validate_conversation().is_err()
        {
            return Err(invalid_existing());
        }
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
        if sandbox.owner != launch.owner
            || sandbox.id != *sandbox_id
            || workspace.owner != launch.owner
            || workspace.id != *workspace_id
            || workspace.sandbox_id != *sandbox_id
            || content_digest(&sandbox.profile.content).ok().as_ref()
                != Some(&sandbox.profile.digest)
            || content_digest(&workspace.profile.content).ok().as_ref()
                != Some(&workspace.profile.digest)
            || content_digest(&agent.profile.content).ok().as_ref() != Some(&agent.profile.digest)
        {
            return Err(invalid_existing());
        }
        // Reopening work retains its original pins even if the publisher changed targets.
        Ok(Some(TriggerTarget {
            sandbox_profile: SandboxProfileRef {
                id: sandbox.profile.id,
                revision: sandbox.profile.revision,
            },
            inline: None,
            workspace_profile: WorkspaceProfileRef {
                id: workspace.profile.id,
                revision: workspace.profile.revision,
            },
            agent_profile: AgentProfileRef {
                id: agent.profile.id,
                revision: agent.profile.revision,
            },
        }))
    }
}

fn invalid_existing() -> LaunchResourceError {
    LaunchResourceError::terminal(
        "launch_existing_conversation_invalid",
        "The existing conversation selected for this launch is not usable",
    )
}
