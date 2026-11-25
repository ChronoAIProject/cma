use super::*;
use crate::resource_access::{AuthorizedResource, CallerContext};

/// Nonrenewing authority retained until the buffered native history is released.
/// Shared callers use the projected transcript; raw history is personal-only.
pub struct AgentHistoryReading {
    lineage: AgentLineage,
    held: Option<AuthorizedResource>,
    caller: Caller,
}

impl AgentHistoryReading {
    pub fn runtime_id(&self) -> Result<&str, AgentReadError> {
        self.lineage
            .sandbox
            .runtime
            .as_ref()
            .map(|runtime| runtime.runtime_id.as_str())
            .ok_or_else(|| {
                tracing::warn!(parent: None, reason = "no_runtime", "Agent raw history export unavailable");
                AgentReadError::HistoryRuntimeUnavailable
            })
    }
}

impl AgentReadService {
    pub async fn begin_history_read_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
    ) -> Result<AgentHistoryReading, AgentReadError> {
        let held = self
            .authorized(context, credential, id, ResourceCapability::ViewHistory)
            .await?;
        if !authorized::is_personal(&held, context) {
            return Err(AgentReadError::Forbidden);
        }
        let lineage = authorized::lineage(held.snapshot.lineage.clone())?;
        history_reading(lineage, Some(held), &context.caller)
    }

    pub async fn begin_history_read(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentHistoryReading, AgentReadError> {
        require(caller)?;
        let held = if let Some(access) = &self.access {
            Some(
                access
                    .authorize_personal(
                        caller,
                        &ResourceTarget::Agent {
                            agent_id: id.clone(),
                        },
                        ResourceCapability::ViewHistory,
                    )
                    .await
                    .map_err(authorized::access_error)?,
            )
        } else {
            None
        };
        let lineage = match &held {
            Some(value) => authorized::lineage(value.snapshot.lineage.clone())?,
            None => self.owned_lineage(caller, id).await?,
        };
        history_reading(lineage, held, caller)
    }

    /// Call after all provider IO and immediately before constructing the body.
    /// Neither fresh authorization nor a changed runtime can replace the held one.
    pub async fn finish_history_read(
        &self,
        reading: AgentHistoryReading,
    ) -> Result<(), AgentReadError> {
        let id = &reading.lineage.agent.id;
        let current = if let Some(held) = &reading.held {
            let access = self
                .access
                .as_ref()
                .ok_or_else(|| AgentReadError::Unavailable("resource access is absent".into()))?;
            let snapshot = access
                .current_read_snapshot(
                    held,
                    &ResourceTarget::Agent {
                        agent_id: id.clone(),
                    },
                    ResourceCapability::ViewHistory,
                )
                .await
                .map_err(authorized::access_error)?;
            authorized::lineage(snapshot.lineage)?
        } else {
            self.owned_lineage(&reading.caller, id).await?
        };
        require_granted(&current.agent, crate::RuntimeCapability::Conversation)?;
        if !same_history_target(&reading.lineage, &current) {
            return Err(AgentReadError::NotFound);
        }
        Ok(())
    }
}

fn history_reading(
    lineage: AgentLineage,
    held: Option<AuthorizedResource>,
    caller: &Caller,
) -> Result<AgentHistoryReading, AgentReadError> {
    require_granted(&lineage.agent, crate::RuntimeCapability::Conversation)?;
    let reading = AgentHistoryReading {
        lineage,
        held,
        caller: caller.clone(),
    };
    reading.runtime_id()?;
    Ok(reading)
}

fn same_history_target(before: &AgentLineage, after: &AgentLineage) -> bool {
    before.agent.id == after.agent.id
        && before.agent.created_at == after.agent.created_at
        && before.agent.owner == after.agent.owner
        && before.agent.deleted_at == after.agent.deleted_at
        && before.agent.conversation.conversation_id == after.agent.conversation.conversation_id
        && before.agent.conversation.runtime_generation
            == after.agent.conversation.runtime_generation
        && before.worktree.id == after.worktree.id
        && before.worktree.path == after.worktree.path
        && before.worktree.branch == after.worktree.branch
        && before.worktree.base_commit == after.worktree.base_commit
        && before.workspace.id == after.workspace.id
        && before.workspace.created_at == after.workspace.created_at
        && before.workspace.status == after.workspace.status
        && before.sandbox.id == after.sandbox.id
        && before.sandbox.created_at == after.sandbox.created_at
        && before.sandbox.retired_at == after.sandbox.retired_at
        && before.sandbox.runtime == after.sandbox.runtime
}
