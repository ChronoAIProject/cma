//! Identities and immutable profile inputs reserved before a user owns the runtime.

use serde::{Deserialize, Serialize};

use crate::profiles::{content_digest, AgentProfileSnapshot, WorkspaceProfileSnapshot};
use crate::sandbox::{ResolvedAgentSkill, SandboxStoreError};
use crate::{AgentId, WorkspaceId, WorktreeId};

use super::{corrupt, WarmSandbox, WarmSandboxPlan, WarmSandboxState};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WarmWorkspacePlan {
    pub profile: WorkspaceProfileSnapshot,
    pub agents: Vec<AgentProfileSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WarmWorkspace {
    pub id: WorkspaceId,
    pub resolved_commit: Option<String>,
    pub setup_ready: bool,
    pub agents: Vec<WarmAgent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WarmAgent {
    pub id: AgentId,
    pub worktree_id: WorktreeId,
    /// Persist immutable skill selection before installing the Agent home.
    pub skills: Option<Vec<ResolvedAgentSkill>>,
    pub conversation_id: Option<String>,
}

impl WarmWorkspace {
    pub(super) fn new(plan: &WarmWorkspacePlan) -> Self {
        Self {
            id: WorkspaceId::generate(),
            resolved_commit: None,
            setup_ready: false,
            agents: plan
                .agents
                .iter()
                .map(|_| WarmAgent {
                    id: AgentId::generate(),
                    worktree_id: WorktreeId::generate(),
                    skills: None,
                    conversation_id: None,
                })
                .collect(),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.resolved_commit.is_some()
            && self.setup_ready
            && self
                .agents
                .iter()
                .all(|agent| agent.skills.is_some() && agent.conversation_id.is_some())
    }
}

impl WarmSandboxPlan {
    /// Legacy installation-only reservations can be read for retirement, never claimed.
    pub fn has_complete_hierarchy(&self) -> bool {
        self.workspaces.len() == self.profile.content.workspace_profiles.len()
    }

    pub(super) fn validate_hierarchy(&self) -> Result<(), SandboxStoreError> {
        if self.workspaces.is_empty() {
            return Ok(());
        }
        if !self.has_complete_hierarchy()
            || self
                .workspaces
                .iter()
                .map(|workspace| workspace.agents.len())
                .sum::<usize>()
                > 16
        {
            return Err(corrupt("workspace plan does not match the Sandbox Profile"));
        }
        let mut names = std::collections::BTreeSet::new();
        for (reference, workspace) in self
            .profile
            .content
            .workspace_profiles
            .iter()
            .zip(&self.workspaces)
        {
            let profile = &workspace.profile;
            profile
                .content
                .validate()
                .map_err(|e| corrupt(&e.to_string()))?;
            if reference.id != profile.id
                || reference.revision != profile.revision
                || content_digest(&profile.content).map_err(|e| corrupt(&e.to_string()))?
                    != profile.digest
                || !names.insert(
                    profile
                        .content
                        .workspace_name()
                        .map_err(|e| corrupt(&e.to_string()))?,
                )
                || profile.content.agent_profiles.len() != workspace.agents.len()
                || workspace.agents.len()
                    > usize::from(self.profile.content.max_agents_per_workspace)
            {
                return Err(corrupt("workspace plan changed its pinned profile"));
            }
            for (reference, agent) in profile.content.agent_profiles.iter().zip(&workspace.agents) {
                agent
                    .content
                    .validate()
                    .map_err(|e| corrupt(&e.to_string()))?;
                if reference.id != agent.id
                    || reference.revision != agent.revision
                    || content_digest(&agent.content).map_err(|e| corrupt(&e.to_string()))?
                        != agent.digest
                {
                    return Err(corrupt("Agent plan changed its pinned profile"));
                }
            }
        }
        Ok(())
    }
}

impl WarmSandbox {
    pub(super) fn validate_hierarchy(&self) -> Result<(), SandboxStoreError> {
        if self.workspaces.len() != self.plan.workspaces.len() {
            return Err(corrupt("workspace reservations do not match the plan"));
        }
        let mut ids = std::collections::BTreeSet::new();
        for (workspace, plan) in self.workspaces.iter().zip(&self.plan.workspaces) {
            if !ids.insert(workspace.id.as_str())
                || workspace.id.as_str().is_empty()
                || workspace.agents.len() != plan.agents.len()
                || workspace.resolved_commit.as_ref().is_some_and(|commit| {
                    !matches!(commit.len(), 40 | 64)
                        || !commit.bytes().all(|c| c.is_ascii_hexdigit())
                })
                || workspace.setup_ready && workspace.resolved_commit.is_none()
            {
                return Err(corrupt("invalid prepared workspace"));
            }
            for (agent, plan) in workspace.agents.iter().zip(&plan.agents) {
                if !ids.insert(agent.id.as_str())
                    || agent.id.as_str().is_empty()
                    || !ids.insert(agent.worktree_id.as_str())
                    || agent.worktree_id.as_str().is_empty()
                    || agent.skills.as_ref().is_some_and(|skills| {
                        skills.len() != plan.content.skills.len()
                            || skills
                                .iter()
                                .zip(&plan.content.skills)
                                .any(|(skill, reference)| {
                                    skill.id != reference.id
                                        || skill.source != reference.source
                                        || skill.validate().is_err()
                                        || reference
                                            .version
                                            .as_ref()
                                            .filter(|version| {
                                                !version.is_empty() && !version.starts_with('@')
                                            })
                                            .is_some_and(|version| version != &skill.version)
                                })
                    })
                    || agent.conversation_id.as_ref().is_some_and(|id| {
                        id.trim().is_empty()
                            || id.len() > 512
                            || id.trim() != id
                            || id.chars().any(char::is_control)
                            || agent.skills.is_none()
                            || !workspace.setup_ready
                    })
                {
                    return Err(corrupt("invalid prepared Agent"));
                }
            }
            if matches!(
                self.state,
                WarmSandboxState::Ready | WarmSandboxState::Claimed
            ) && !workspace.is_ready()
            {
                return Err(corrupt(
                    "warm environment published before Agent sessions were ready",
                ));
            }
        }
        Ok(())
    }
}
