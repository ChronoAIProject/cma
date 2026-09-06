use super::*;
use crate::sandbox::store::{AgentAdmission, AgentAdmissionResult, AgentAdmissionStore};
use crate::sandbox::{AgentOrigin, SandboxEventKind, SandboxReviveCause, WorkspaceStatus};

#[async_trait]
impl AgentAdmissionStore for InMemorySandboxStore {
    async fn admit_agent(
        &self,
        admission: AgentAdmission,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        let mut state = self.lock();
        if let Some(existing) = state
            .agents
            .values()
            .find(|agent| {
                agent.owner == admission.owner && agent.creation_key == admission.creation_key
            })
            .cloned()
        {
            let worktree = state
                .worktrees
                .get(&existing.worktree_id)
                .cloned()
                .ok_or_else(|| SandboxStoreError::Corrupt("agent worktree is missing".into()))?;
            let initial_log = state
                .agent_logs
                .get(&existing.id)
                .and_then(|entries| entries.first())
                .cloned()
                .ok_or_else(|| SandboxStoreError::Corrupt("agent initial log is missing".into()))?;
            let creation = crate::sandbox::store::AgentCreation {
                agent: existing,
                worktree,
                initial_log,
            };
            return if admission.matches(&creation)? {
                Ok(AgentAdmissionResult::Existing(creation))
            } else {
                Err(SandboxStoreError::AgentCreationConflict)
            };
        }

        let workspace = state
            .workspaces
            .get(&admission.workspace_id)
            .cloned()
            .ok_or(SandboxStoreError::WorkspaceNotReady)?;
        let sandbox = state
            .sandboxes
            .get(&workspace.sandbox_id)
            .cloned()
            .ok_or(SandboxStoreError::SandboxNotReady)?;
        if workspace.owner != admission.owner
            || workspace.owner != sandbox.owner
            || workspace.sandbox_id != sandbox.id
            || workspace.status != WorkspaceStatus::Ready
        {
            return Err(SandboxStoreError::WorkspaceNotReady);
        }
        if !sandbox.accepts_creation_admission() {
            return Err(SandboxStoreError::SandboxNotReady);
        }
        let live = state
            .agents
            .values()
            .filter(|agent| {
                agent.workspace_id == workspace.id
                    && agent.status.is_live()
                    && agent.deleted_at.is_none()
            })
            .count();
        if live >= usize::from(sandbox.limits.max_agents_per_workspace) {
            return Err(SandboxStoreError::AgentLimitReached {
                limit: sandbox.limits.max_agents_per_workspace,
                live: u8::try_from(live).unwrap_or(u8::MAX),
            });
        }

        let creation = admission.materialize(&workspace)?;
        if state.worktrees.contains_key(&creation.worktree.id)
            || state.agents.contains_key(&creation.agent.id)
            || state.worktrees.values().any(|stored| {
                stored.workspace_id == workspace.id && stored.name == creation.worktree.name
            })
        {
            return Err(SandboxStoreError::Duplicate);
        }

        let mut candidate = state.clone();
        let mut next_workspace = workspace;
        next_workspace.next_worktree_number = next_workspace
            .next_worktree_number
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        next_workspace.revision = next_workspace
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        candidate
            .workspaces
            .insert(next_workspace.id.clone(), next_workspace);

        let mut next_sandbox = sandbox;
        next_sandbox
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        let cause = match admission.origin {
            AgentOrigin::Manual => SandboxReviveCause::AgentCreation,
            AgentOrigin::Trigger { .. } => SandboxReviveCause::TriggerLaunch,
        };
        let revival = next_sandbox
            .record_creation_admission(cause, admission.event_at.clone())
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        candidate
            .sandboxes
            .insert(next_sandbox.id.clone(), next_sandbox);

        candidate
            .worktrees
            .insert(creation.worktree.id.clone(), creation.worktree.clone());
        candidate
            .agents
            .insert(creation.agent.id.clone(), creation.agent.clone());
        candidate.agent_logs.insert(
            creation.agent.id.clone(),
            vec![creation.initial_log.clone()],
        );
        if let Some(event) = revival {
            append_sandbox_event(
                &mut candidate,
                &creation.agent.sandbox_id,
                admission.event_at.clone(),
                event,
            )?;
        }
        append_sandbox_event(
            &mut candidate,
            &creation.agent.sandbox_id,
            admission.event_at.clone(),
            SandboxEventKind::WorktreeCreated {
                workspace_id: creation.agent.workspace_id.clone(),
                worktree_id: creation.worktree.id.clone(),
            },
        )?;
        append_sandbox_event(
            &mut candidate,
            &creation.agent.sandbox_id,
            admission.event_at,
            SandboxEventKind::AgentCreated {
                workspace_id: creation.agent.workspace_id.clone(),
                worktree_id: creation.worktree.id.clone(),
                agent_id: creation.agent.id.clone(),
            },
        )?;
        *state = candidate;
        Ok(AgentAdmissionResult::Created(creation))
    }
}
