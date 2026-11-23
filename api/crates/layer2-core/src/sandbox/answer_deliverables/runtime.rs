use super::*;
use crate::sandbox::agent_access::AgentAccessError;
use crate::sandbox::agent_access_tool::AgentAccessToolAuthenticator;
use crate::{Authorization, Capability};

impl AnswerDeliverableService {
    pub async fn describe_for_agent(
        &self,
        auth: &dyn AgentAccessToolAuthenticator,
        capability: &str,
        resource: DeliverableResource,
    ) -> Result<DeliverableDescription, DeliverableError> {
        if matches!(resource, DeliverableResource::Document { .. }) {
            return Err(DeliverableError::Invalid);
        }
        let requester = auth
            .authenticate(capability)
            .await
            .map_err(authority_error)?;
        let caller = Caller::new(
            requester.owner.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        );
        let lineage = self.reads.lineage(&caller, &requester.agent_id).await?;
        if lineage.agent.created_at != requester.creation.agent_created_at
            || lineage.agent.creation_fingerprint != requester.creation.creation_fingerprint
            || lineage.agent.conversation.conversation_id.as_ref() != Some(&requester.conversation)
            || lineage.agent.workspace_id != requester.creation.workspace_id
            || lineage.agent.worktree_id != requester.creation.worktree_id
            || lineage.sandbox.id != requester.sandbox_id
            || lineage.sandbox.created_at != requester.creation.sandbox_created_at
            || lineage.sandbox.runtime.as_ref() != Some(&requester.runtime)
        {
            return Err(DeliverableError::NotFound);
        }
        let access = DeliverableAccess {
            caller: &caller,
            context: None,
            credential: None,
            document_token: None,
        };
        let result = match resource {
            DeliverableResource::LiveFile { path } => {
                self.describe_agent_file(&access, &requester.agent_id, &lineage.worktree, &path)
                    .await?
            }
            resource => {
                self.describe(&access, &requester.agent_id, resource)
                    .await?
            }
        };
        if auth
            .authenticate(capability)
            .await
            .map_err(authority_error)?
            != requester
        {
            return Err(DeliverableError::NotFound);
        }
        Ok(result)
    }
}

impl AnswerDeliverableService {
    /// An Agent works inside its own worktree and is not told that worktree's
    /// home-relative prefix, so it naturally names a file it produced relative
    /// to its working directory, as `./name`, or by its absolute path. Accept
    /// those forms by resolving them to the exact home-relative path. The
    /// documented home-relative form is tried first and wins when both exist.
    /// Every candidate stays within the Agent's own home and authority, and the
    /// verified reference records the resolved path.
    async fn describe_agent_file(
        &self,
        access: &DeliverableAccess<'_>,
        agent: &AgentId,
        worktree: &crate::sandbox::Worktree,
        path: &str,
    ) -> Result<DeliverableDescription, DeliverableError> {
        let live = |path: String| DeliverableResource::LiveFile { path };
        if let Some(home_relative) = path.strip_prefix(SANDBOX_HOME) {
            return self
                .describe(access, agent, live(home_relative.to_owned()))
                .await;
        }
        let relative = path.strip_prefix("./").unwrap_or(path);
        let first = self
            .describe(access, agent, live(relative.to_owned()))
            .await;
        let Some(prefix) = worktree.path.strip_prefix(SANDBOX_HOME) else {
            return first;
        };
        match first {
            Err(DeliverableError::File(AgentFileError::NotFound))
                if !relative.is_empty() && !relative.starts_with(&format!("{prefix}/")) =>
            {
                self.describe(access, agent, live(format!("{prefix}/{relative}")))
                    .await
            }
            result => result,
        }
    }
}

/// The Sandbox home every Agent file path is relative to.
const SANDBOX_HOME: &str = "/home/cma/";

fn authority_error(error: AgentAccessError) -> DeliverableError {
    if matches!(error, AgentAccessError::Unavailable) {
        DeliverableError::Unavailable
    } else {
        DeliverableError::NotFound
    }
}
