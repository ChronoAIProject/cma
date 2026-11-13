use std::collections::{HashMap, HashSet};

use super::agent_lineage::DetailAgent;
use super::*;

impl From<&Worktree> for WorktreeHierarchySummary {
    fn from(value: &Worktree) -> Self {
        Self {
            id: value.id.clone(),
            workspace_id: value.workspace_id.clone(),
            sandbox_id: value.sandbox_id.clone(),
            name: value.name.clone(),
            path: value.path.clone(),
            branch: value.branch.clone(),
            base_commit: value.base_commit.clone(),
            status: value.status,
            agent_id: value.agent_id.clone(),
        }
    }
}

impl From<&Agent> for AgentHierarchySummary {
    fn from(value: &Agent) -> Self {
        Self {
            task_status: super::super::AgentTaskStatus::Unknown,
            task_started_at: None,
            runtime_metadata: (!value.conversation.observations.is_empty())
                .then(|| value.conversation.observations.metadata.clone()),
            id: value.id.clone(),
            worktree_id: value.worktree_id.clone(),
            workspace_id: value.workspace_id.clone(),
            sandbox_id: value.sandbox_id.clone(),
            status: value.status,
            profile_id: value.profile.id.to_string(),
            profile_revision: value.profile.revision,
            owner_title: value.conversation.preferences.title.clone(),
            title: value.title.clone(),
            updated_at: value.updated_at.clone(),
        }
    }
}

pub(super) fn sandbox_view(
    record: SandboxDetailRecord,
) -> Result<SandboxHierarchyView, HierarchyReadError> {
    match record {
        SandboxDetailRecord::Complete(record) => sandbox_record_view(record),
        SandboxDetailRecord::Projected(record) => sandbox_record_view(record),
    }
}

fn sandbox_record_view<A: DetailAgent>(
    record: SandboxHierarchyRecord<A>,
) -> Result<SandboxHierarchyView, HierarchyReadError> {
    if record.sandbox.id.as_str().trim().is_empty()
        || record.sandbox.owner.as_str().trim().is_empty()
        || record.sandbox.revision == 0
    {
        return Err(corrupt("invalid Sandbox identity"));
    }
    let sandbox_id = &record.sandbox.id;
    let owner = &record.sandbox.owner;
    if record.workspaces.iter().any(|workspace| {
        &workspace.sandbox_id != sandbox_id
            || &workspace.owner != owner
            || workspace.revision == 0
            || workspace.id.as_str().trim().is_empty()
    }) || record
        .worktrees
        .iter()
        .any(|worktree| &worktree.sandbox_id != sandbox_id || &worktree.owner != owner)
        || record.agents.iter().any(|agent| {
            let agent = agent.lineage();
            agent.sandbox_id != sandbox_id || agent.owner != owner
        })
    {
        return Err(corrupt("sandbox child lineage does not match"));
    }
    let workspace_ids: HashSet<_> = record.workspaces.iter().map(|value| &value.id).collect();
    if workspace_ids.len() != record.workspaces.len() {
        return Err(corrupt("duplicate Workspace identity"));
    }
    if record
        .worktrees
        .iter()
        .any(|value| !workspace_ids.contains(&value.workspace_id))
        || record
            .agents
            .iter()
            .any(|value| !workspace_ids.contains(value.lineage().workspace_id))
    {
        return Err(corrupt("sandbox child references an unknown workspace"));
    }
    validate_agent_worktrees(&record.worktrees, &record.agents)?;
    let mut workspaces: Vec<_> = record
        .workspaces
        .iter()
        .map(|workspace| workspace_summary(workspace, &record.worktrees, &record.agents))
        .collect();
    workspaces.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    Ok(SandboxHierarchyView {
        counts: counts(
            record.workspaces.len(),
            record.worktrees.len(),
            record.agents.len(),
        ),
        sandbox: record.sandbox,
        workspaces,
    })
}

pub(super) fn workspace_view(
    record: WorkspaceHierarchyRecord,
) -> Result<WorkspaceHierarchyView, HierarchyReadError> {
    if record.workspace.id.as_str().trim().is_empty()
        || record.workspace.owner.as_str().trim().is_empty()
        || record.workspace.sandbox_id.as_str().trim().is_empty()
        || record.workspace.revision == 0
    {
        return Err(corrupt("invalid Workspace identity"));
    }
    if record.worktrees.iter().any(|worktree| {
        worktree.workspace_id != record.workspace.id
            || worktree.sandbox_id != record.workspace.sandbox_id
            || worktree.owner != record.workspace.owner
    }) || record.agents.iter().any(|agent| {
        agent.workspace_id != record.workspace.id
            || agent.sandbox_id != record.workspace.sandbox_id
            || agent.owner != record.workspace.owner
    }) {
        return Err(corrupt("workspace child lineage does not match"));
    }
    validate_agent_worktrees(&record.worktrees, &record.agents)?;
    let mut worktrees: Vec<_> = record
        .worktrees
        .iter()
        .map(WorktreeHierarchySummary::from)
        .collect();
    worktrees.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    let mut agents: Vec<_> = record
        .agents
        .iter()
        .map(|agent| {
            let mut summary = AgentHierarchySummary::from(agent);
            summary.task_started_at = record.task_started_at.get(&agent.id).cloned();
            summary.task_status = record
                .task_statuses
                .get(&agent.id)
                .copied()
                .unwrap_or_default();
            summary
        })
        .collect();
    agents.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    Ok(WorkspaceHierarchyView {
        counts: counts(0, worktrees.len(), agents.len()),
        workspace: record.workspace,
        worktrees,
        agents,
    })
}

fn workspace_summary<A: DetailAgent>(
    workspace: &Workspace,
    worktrees: &[Worktree],
    agents: &[A],
) -> WorkspaceHierarchySummary {
    WorkspaceHierarchySummary {
        id: workspace.id.clone(),
        sandbox_id: workspace.sandbox_id.clone(),
        name: workspace.name.clone(),
        status: workspace.status,
        profile_id: workspace.profile.id.to_string(),
        profile_revision: workspace.profile.revision,
        counts: counts(
            0,
            worktrees
                .iter()
                .filter(|value| value.workspace_id == workspace.id)
                .count(),
            agents
                .iter()
                .filter(|value| value.lineage().workspace_id == &workspace.id)
                .count(),
        ),
    }
}

fn validate_agent_worktrees<A: DetailAgent>(
    worktrees: &[Worktree],
    agents: &[A],
) -> Result<(), HierarchyReadError> {
    let by_id: HashMap<_, _> = worktrees.iter().map(|value| (&value.id, value)).collect();
    let agents_by_id: HashMap<_, _> = agents
        .iter()
        .map(|agent| (agent.lineage().id, agent))
        .collect();
    if by_id.len() != worktrees.len()
        || agents_by_id.len() != agents.len()
        || worktrees
            .iter()
            .any(|worktree| worktree.id.as_str().trim().is_empty() || worktree.revision == 0)
    {
        return Err(corrupt("duplicate or invalid child identity"));
    }
    for tree in worktrees {
        if tree.agent_id.as_ref().is_some_and(|id| {
            agents_by_id
                .get(id)
                .is_none_or(|agent| agent.lineage().worktree_id != &tree.id)
        }) {
            return Err(corrupt("Worktree assignment references an unknown Agent"));
        }
    }
    for value in agents {
        let agent = value.lineage();
        let worktree = by_id
            .get(agent.worktree_id)
            .ok_or_else(|| corrupt("agent references an unknown worktree"))?;
        if worktree.agent_id.as_ref() != Some(agent.id)
            || &worktree.workspace_id != agent.workspace_id
            || &worktree.sandbox_id != agent.sandbox_id
            || &worktree.owner != agent.owner
            || agent.revision == 0
            || agent.id.as_str().trim().is_empty()
        {
            return Err(corrupt("agent and worktree assignment disagree"));
        }
        value.validate_payload()?;
    }
    Ok(())
}

fn counts(workspaces: usize, worktrees: usize, agents: usize) -> HierarchyCounts {
    HierarchyCounts {
        workspaces: workspaces as u64,
        worktrees: worktrees as u64,
        agents: agents as u64,
    }
}
