//! Conservative retirement observations; no lifecycle or effect authority.
use std::collections::{BTreeMap, BTreeSet};

use layer3_adapter::runtime_ledger_retirement::{
    ledger_retired_agent_key, ledger_retired_worktree_key, RuntimeLedgerRetirementSnapshot,
    LEDGER_RETIREMENT_SNAPSHOT_FORMAT, MAX_LEDGER_RETIREMENT_RESOURCES,
};

use super::{
    Agent, AgentSkillInstallStatus, AgentStatus, AgentWorkStep, SandboxHierarchyRecord,
    SandboxModelError, WorktreeStatus,
};

impl SandboxHierarchyRecord {
    pub fn ledger_retirement_observation(
        &self,
        observed_at: String,
        source_sha256: String,
    ) -> Result<RuntimeLedgerRetirementSnapshot, SandboxModelError> {
        self.sandbox.validate()?;
        if crate::seconds_from_rfc3339(&observed_at).is_none()
            || self.agents.len() > MAX_LEDGER_RETIREMENT_RESOURCES
            || self.worktrees.len() > MAX_LEDGER_RETIREMENT_RESOURCES
            || self.workspaces.len() > MAX_LEDGER_RETIREMENT_RESOURCES
        {
            return Err(invalid());
        }
        let home = self
            .sandbox
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .ok_or_else(invalid)?;
        let runtime = self
            .sandbox
            .retained_runtime_binding()
            .ok_or_else(invalid)?;
        let generation = runtime.provision_generation.ok_or_else(invalid)?;
        let mut workspaces = BTreeMap::new();
        let mut worktrees = BTreeMap::new();
        let mut agents = BTreeMap::new();
        let mut unsettled_worktrees = BTreeSet::new();
        let mut workspace_names = BTreeSet::new();
        let mut worktree_names = BTreeSet::new();
        for workspace in &self.workspaces {
            if workspace.sandbox_id != self.sandbox.id
                || workspace.owner != self.sandbox.owner
                || workspace.revision == 0
                || !workspace_names.insert(&workspace.name)
                || workspaces.insert(&workspace.id, workspace).is_some()
            {
                return Err(invalid());
            }
        }
        for worktree in &self.worktrees {
            if worktree.sandbox_id != self.sandbox.id
                || worktree.owner != self.sandbox.owner
                || worktree.revision == 0
                || !workspaces.contains_key(&worktree.workspace_id)
                || !worktree_names.insert((&worktree.workspace_id, &worktree.name))
                || worktrees.insert(&worktree.id, worktree).is_some()
            {
                return Err(invalid());
            }
        }
        for agent in &self.agents {
            let worktree = worktrees.get(&agent.worktree_id).ok_or_else(invalid)?;
            agent.validate_conversation()?;
            agent.validate_worktree_recoveries()?;
            if !settled(agent) {
                unsettled_worktrees.insert(&agent.worktree_id);
            }
            if agent.sandbox_id != self.sandbox.id
                || agent.owner != self.sandbox.owner
                || agent.workspace_id != worktree.workspace_id
                || agent.revision == 0
                || agents.insert(&agent.id, agent).is_some()
            {
                return Err(invalid());
            }
        }
        let mut snapshot = RuntimeLedgerRetirementSnapshot {
            format: LEDGER_RETIREMENT_SNAPSHOT_FORMAT.into(),
            sandbox_id: self.sandbox.id.to_string(),
            home_identity: home.identity().map_err(|_| invalid())?,
            generation,
            sandbox_revision: self.sandbox.revision,
            observed_at,
            source_sha256,
            agents_observed: self.agents.len() as u64,
            worktrees_observed: self.worktrees.len() as u64,
            deleted_agents: Vec::new(),
            removed_worktrees: Vec::new(),
        };
        let mut settled_worktrees = BTreeSet::new();
        for worktree in &self.worktrees {
            let named_agent_settled = worktree
                .agent_id
                .as_ref()
                .and_then(|id| agents.get(id))
                .is_some_and(|agent| agent.worktree_id == worktree.id && settled(agent));
            if worktree.status == WorktreeStatus::Removed
                && named_agent_settled
                && !unsettled_worktrees.contains(&worktree.id)
            {
                let workspace = workspaces[&worktree.workspace_id];
                snapshot
                    .removed_worktrees
                    .push(ledger_retired_worktree_key(&workspace.name, &worktree.name));
                settled_worktrees.insert(&worktree.id);
            }
        }
        for agent in &self.agents {
            if settled(agent) && settled_worktrees.contains(&agent.worktree_id) {
                snapshot
                    .deleted_agents
                    .push(ledger_retired_agent_key(agent.id.as_str()));
            }
        }
        snapshot.deleted_agents.sort();
        snapshot.removed_worktrees.sort();
        if !snapshot.validate() {
            return Err(invalid());
        }
        Ok(snapshot)
    }
}

fn settled(agent: &Agent) -> bool {
    agent.status == AgentStatus::Deleted
        && agent
            .deleted_at
            .as_deref()
            .is_some_and(|at| crate::seconds_from_rfc3339(at).is_some())
        && agent.work.step == AgentWorkStep::None
        && agent.work.lease.is_none()
        && agent.work.cleanup_debt.is_empty()
        && agent.work.cleanup_not_before.is_none()
        && agent.work.worktree_recoveries.is_empty()
        && agent
            .work
            .live_skills
            .iter()
            .all(|skill| skill.status != AgentSkillInstallStatus::Pending)
        && agent.conversation.active_turn.is_none()
        && agent.conversation.queued_messages.is_empty()
        && agent
            .conversation
            .operation
            .as_ref()
            .is_none_or(|operation| !operation.blocks_dispatch())
        && agent
            .conversation
            .first_message
            .as_ref()
            .is_none_or(|message| message.is_terminal())
}

fn invalid() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.ledger_retirement_observation")
}
