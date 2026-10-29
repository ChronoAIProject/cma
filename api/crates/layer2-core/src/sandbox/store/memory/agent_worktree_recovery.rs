use super::*;
use crate::sandbox::store::AgentControlStoreError as Error;
use crate::sandbox::{plan_agent_worktree_recovery, AgentWorktreeRecoveryContext};
use crate::{Caller, Capability};

pub(super) fn begin(
    store: &InMemorySandboxStore,
    id: &AgentId,
    caller: &Caller,
    operation_id: &str,
    successor_id: &WorktreeId,
    now: String,
) -> Result<Agent, Error> {
    let mut state = store.lock();
    let agent = state.agents.get(id).ok_or(Error::NotFound)?;
    if agent.id != *id {
        return Err(Error::Corrupt(
            "Agent storage identity does not match".into(),
        ));
    }
    if !caller.authorization.allows(Capability::ControlAnySandbox)
        && (caller.subject != agent.owner
            || !caller.authorization.allows(Capability::UseOwnSandboxes))
    {
        return Err(Error::NotFound);
    }
    let worktree = state
        .worktrees
        .get(&agent.worktree_id)
        .ok_or_else(|| Error::Corrupt("Agent worktree is missing".into()))?;
    let workspace = state
        .workspaces
        .get(&agent.workspace_id)
        .ok_or_else(|| Error::Corrupt("Agent workspace is missing".into()))?;
    let sandbox = state
        .sandboxes
        .get(&agent.sandbox_id)
        .ok_or_else(|| Error::Corrupt("Agent sandbox is missing".into()))?;
    let live = state
        .agents
        .values()
        .filter(|candidate| {
            candidate.workspace_id == workspace.id
                && candidate.status.is_live()
                && candidate.deleted_at.is_none()
        })
        .count();
    let mutation = plan_agent_worktree_recovery(
        AgentWorktreeRecoveryContext {
            agent,
            worktree,
            workspace,
            sandbox,
            caller,
            live_agents: u8::try_from(live).unwrap_or(u8::MAX),
        },
        operation_id,
        successor_id,
        now,
    )?;
    let Some(mutation) = mutation else {
        return Ok(agent.clone());
    };
    if state.worktrees.contains_key(successor_id)
        || state.worktrees.values().any(|old| {
            old.workspace_id == mutation.workspace.id
                && old.name == mutation.successor_worktree.name
        })
    {
        return Err(Error::Conflict);
    }
    let result = mutation.agent.clone();
    state.worktrees.insert(
        mutation.retained_worktree.id.clone(),
        mutation.retained_worktree,
    );
    state.worktrees.insert(
        mutation.successor_worktree.id.clone(),
        mutation.successor_worktree,
    );
    state
        .workspaces
        .insert(mutation.workspace.id.clone(), mutation.workspace);
    state.agents.insert(result.id.clone(), mutation.agent);
    Ok(result)
}
