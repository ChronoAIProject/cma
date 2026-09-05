use async_trait::async_trait;
use serde::Serialize;

use crate::ids::{AgentId, SubjectId, WorkspaceId, WorktreeId};
use crate::profiles::{content_digest, AgentProfileSnapshot};

use super::SandboxStoreError;
use crate::sandbox::{
    Agent, AgentLogEntry, AgentOrigin, AgentStatus, AgentWorkState, AgentWorkStep,
    ConversationState, FirstMessageRecord, Workspace, Worktree, WorktreeStatus,
};

#[derive(Debug, Clone)]
pub struct AgentAdmission {
    pub owner: SubjectId,
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub worktree_id: WorktreeId,
    pub profile: AgentProfileSnapshot,
    pub title: Option<String>,
    pub first_message: Option<FirstMessageRecord>,
    pub origin: AgentOrigin,
    pub creation_key: String,
    pub event_at: String,
}

impl AgentAdmission {
    pub fn materialize(&self, workspace: &Workspace) -> Result<AgentCreation, SandboxStoreError> {
        if self.workspace_id != workspace.id
            || self.owner != workspace.owner
            || self.creation_key.trim().is_empty()
            || self.creation_key.len() > 256
        {
            return Err(SandboxStoreError::InvalidTransaction(
                "agent admission identity is invalid",
            ));
        }
        let number = workspace.next_worktree_number;
        let name = format!("wt-{number}");
        let base_commit = workspace
            .resolved_commit
            .clone()
            .ok_or(SandboxStoreError::WorkspaceNotReady)?;
        let worktree = Worktree {
            id: self.worktree_id.clone(),
            workspace_id: workspace.id.clone(),
            sandbox_id: workspace.sandbox_id.clone(),
            owner: workspace.owner.clone(),
            name: name.clone(),
            path: format!("{}/.worktrees/{name}", workspace.path),
            branch: format!("cma/{name}"),
            base_commit,
            status: WorktreeStatus::Creating,
            agent_id: Some(self.agent_id.clone()),
            revision: 1,
        };
        let agent = Agent {
            id: self.agent_id.clone(),
            worktree_id: self.worktree_id.clone(),
            workspace_id: workspace.id.clone(),
            sandbox_id: workspace.sandbox_id.clone(),
            owner: workspace.owner.clone(),
            creation_key: self.creation_key.clone(),
            creation_fingerprint: self.fingerprint()?,
            profile: self.profile.clone(),
            status: AgentStatus::Starting,
            conversation: ConversationState {
                conversation_id: None,
                runtime_generation: None,
                last_seq: 1,
                first_message: self.first_message.clone(),
                active_turn: None,
                operation: None,
                queued_messages: Vec::new(),
            },
            work: AgentWorkState::new(AgentWorkStep::AddWorktree, self.event_at.clone()),
            failure: None,
            title: self.title.clone(),
            created_at: self.event_at.clone(),
            updated_at: self.event_at.clone(),
            ready_at: None,
            stopped_at: None,
            requested_stop_reason: None,
            stop_reason: None,
            deleted_at: None,
            origin: self.origin.clone(),
            revision: 1,
        };
        agent
            .validate_for_admission(&worktree)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        Ok(AgentCreation {
            agent,
            worktree,
            initial_log: AgentLogEntry {
                agent_id: self.agent_id.clone(),
                seq: 1,
                appended_at: self.event_at.clone(),
                kind: crate::sandbox::AgentLogKind::Created {
                    workspace_id: workspace.id.clone(),
                    worktree_id: self.worktree_id.clone(),
                },
            },
        })
    }

    pub fn matches(&self, creation: &AgentCreation) -> Result<bool, SandboxStoreError> {
        creation.validate_replay()?;
        Ok(creation.agent.owner == self.owner
            && creation.agent.workspace_id == self.workspace_id
            && creation.agent.creation_key == self.creation_key
            && creation.agent.creation_fingerprint == self.fingerprint()?)
    }

    fn fingerprint(&self) -> Result<String, SandboxStoreError> {
        let first_message = self
            .first_message
            .as_ref()
            .map(|message| FirstMessageIdentity {
                origin_key: &message.origin_key,
                text: &message.text,
                payload_sha256: &message.payload_sha256,
            });
        content_digest(&AdmissionIdentity {
            workspace_id: &self.workspace_id,
            profile: &self.profile,
            title: self.title.as_deref(),
            first_message,
            origin: &self.origin,
        })
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
    }
}

#[derive(Serialize)]
struct AdmissionIdentity<'a> {
    workspace_id: &'a WorkspaceId,
    profile: &'a AgentProfileSnapshot,
    title: Option<&'a str>,
    first_message: Option<FirstMessageIdentity<'a>>,
    origin: &'a AgentOrigin,
}

#[derive(Serialize)]
struct FirstMessageIdentity<'a> {
    origin_key: &'a str,
    text: &'a str,
    payload_sha256: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCreation {
    pub agent: Agent,
    pub worktree: Worktree,
    pub initial_log: AgentLogEntry,
}

impl AgentCreation {
    pub fn validate_replay(&self) -> Result<(), SandboxStoreError> {
        let created_log_matches = matches!(
            &self.initial_log.kind,
            crate::sandbox::AgentLogKind::Created {
                workspace_id,
                worktree_id,
            } if workspace_id == &self.agent.workspace_id
                && worktree_id == &self.worktree.id
        );
        if self.agent.worktree_id != self.worktree.id
            || self.agent.workspace_id != self.worktree.workspace_id
            || self.agent.sandbox_id != self.worktree.sandbox_id
            || self.agent.owner != self.worktree.owner
            || self.worktree.agent_id.as_ref() != Some(&self.agent.id)
            || self.initial_log.agent_id != self.agent.id
            || self.initial_log.seq != 1
            || !created_log_matches
        {
            return Err(SandboxStoreError::Corrupt(
                "agent creation replay artifacts disagree".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentAdmissionResult {
    Created(AgentCreation),
    Existing(AgentCreation),
}

impl AgentAdmissionResult {
    pub fn creation(&self) -> &AgentCreation {
        match self {
            Self::Created(creation) | Self::Existing(creation) => creation,
        }
    }
}

#[async_trait]
pub trait AgentAdmissionStore: Send + Sync {
    async fn admit_agent(
        &self,
        admission: AgentAdmission,
    ) -> Result<AgentAdmissionResult, SandboxStoreError>;
}
