use async_trait::async_trait;
use serde::Serialize;

use crate::ids::{AgentId, SubjectId, WorkspaceId, WorktreeId};
use crate::profiles::{content_digest, AgentProfileSnapshot};
use crate::RuntimeCapabilities;

use super::SandboxStoreError;
use crate::sandbox::{
    Agent, AgentLogEntry, AgentOrigin, AgentStatus, AgentWorkState, AgentWorkStep,
    ConversationState, FirstMessageRecord, Workspace, Worktree, WorktreeStatus,
};

#[derive(Debug, Clone)]
pub struct AgentAdmission {
    pub binding: Option<crate::sandbox::binding::BindingAdmissionFence>,
    /// Server-attributed creating actor; absent for legacy/internal profile work.
    pub created_by: Option<crate::resource_access::ResourceActor>,
    pub owner: SubjectId,
    pub workspace_id: WorkspaceId,
    pub agent_id: AgentId,
    pub worktree_id: WorktreeId,
    pub profile: AgentProfileSnapshot,
    pub capabilities: RuntimeCapabilities,
    pub title: Option<String>,
    pub first_message: Option<FirstMessageRecord>,
    pub origin: AgentOrigin,
    pub creation_key: String,
    pub event_at: String,
}

impl AgentAdmission {
    pub fn admits_bootstrap(
        &self,
        workspace: &Workspace,
        parent: &crate::sandbox::Sandbox,
    ) -> bool {
        matches!(&self.origin, AgentOrigin::Lifecycle { operation_id }
            if parent.accepts_lifecycle_bootstrap(operation_id))
            && self.binding.is_none()
            && self.owner == parent.owner
            && self.workspace_id == workspace.id
            && crate::sandbox::lifecycle_bootstrap::bootstrap_workspace(parent, workspace)
    }

    pub fn record_parent_admission(
        &self,
        parent: &mut crate::sandbox::Sandbox,
        workspace: &Workspace,
        cause: crate::sandbox::SandboxReviveCause,
        managed_runtime_version: Option<&str>,
    ) -> Result<Option<crate::sandbox::SandboxEventKind>, SandboxStoreError> {
        if self.admits_bootstrap(workspace, parent) {
            parent.revision = parent
                .revision
                .checked_add(1)
                .ok_or(SandboxStoreError::Conflict)?;
            parent.updated_at.clone_from(&self.event_at);
            parent.last_activity_at.clone_from(&self.event_at);
            parent
                .validate()
                .map_err(|e| SandboxStoreError::Corrupt(e.to_string()))?;
            Ok(None)
        } else {
            parent
                .record_creation_admission_with_managed_runtime(
                    cause,
                    managed_runtime_version,
                    self.event_at.clone(),
                )
                .map_err(|e| SandboxStoreError::Corrupt(e.to_string()))
        }
    }
    /// A pre-created profile session without user activity is a fresh session
    /// for the Trigger, including when the Trigger requests a new conversation.
    pub fn claims_unused_prepared_agent(&self, agent: &Agent) -> bool {
        matches!(self.origin, AgentOrigin::Trigger { .. })
            && matches!(agent.origin, AgentOrigin::WorkspaceProfile { .. })
            && agent.work.warm_conversation_id.is_some()
            && agent.conversation.first_message.is_none()
            && agent.conversation.active_turn.is_none()
            && agent.conversation.operation.is_none()
            && agent.conversation.queued_messages.is_empty()
    }

    pub fn can_reuse(&self, agent: &Agent, workspace: &Workspace, reuse_existing: bool) -> bool {
        let explicit_reuse = reuse_existing && matches!(self.origin, AgentOrigin::Trigger { .. });
        let automatic_slot = match self.origin {
            AgentOrigin::WorkspaceProfile { index } => {
                matches!(agent.origin, AgentOrigin::Trigger { .. })
                    && (agent.creation_key.starts_with("trigger-profile:")
                        || agent.work.warm_conversation_id.is_some())
                    && workspace
                        .profile
                        .content
                        .agent_profiles
                        .iter()
                        .position(|reference| {
                            reference.id == self.profile.id
                                && reference.revision == self.profile.revision
                        })
                        == Some(usize::from(index))
            }
            _ => false,
        };
        (explicit_reuse || automatic_slot || self.claims_unused_prepared_agent(agent))
            && agent.owner == self.owner
            && workspace.owner == self.owner
            && agent.workspace_id == workspace.id
            && workspace.id == self.workspace_id
            && agent.sandbox_id == workspace.sandbox_id
            && agent.profile == self.profile
            && (automatic_slot || agent.status.is_live())
            && (automatic_slot || agent.failure.is_none())
            && (automatic_slot || agent.requested_stop_reason.is_none())
    }

    /// Attach a first prompt only to an untouched Agent. Preserve its origin and history.
    pub fn initialize_reused_agent(
        &self,
        agent: &mut Agent,
        has_owner_input: bool,
    ) -> Result<Option<crate::sandbox::AgentLogKind>, SandboxStoreError> {
        if has_owner_input
            || agent.conversation.first_message.is_some()
            || self.first_message.is_none()
        {
            return Ok(None);
        }
        if agent.conversation.active_turn.is_some()
            || agent.conversation.operation.is_some()
            || !agent.conversation.queued_messages.is_empty()
        {
            return Ok(None);
        }
        let request = self.first_message.as_ref().expect("checked first prompt");
        let mut record = FirstMessageRecord::new(
            agent.creation_key.clone(),
            request.text.clone(),
            self.event_at.clone(),
        )
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        record.purpose = request.purpose;
        if agent.status != AgentStatus::Starting {
            if agent.work.lease.is_some() || agent.work.step != AgentWorkStep::None {
                return Err(SandboxStoreError::Conflict);
            }
            agent
                .work
                .schedule(AgentWorkStep::DispatchFirstMessage, self.event_at.clone());
        }
        agent.conversation.first_message = Some(record);
        agent.conversation.last_seq = agent
            .conversation
            .last_seq
            .checked_add(1)
            .ok_or(SandboxStoreError::SequenceConflict)?;
        agent.revision = agent
            .revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        agent.updated_at = self.event_at.clone();
        Ok(Some(crate::sandbox::AgentLogKind::FirstMessageChanged {
            state: crate::sandbox::FirstMessageDelivery::Queued,
            recovery_generation: 0,
        }))
    }

    pub fn materialize(&self, workspace: &Workspace) -> Result<AgentCreation, SandboxStoreError> {
        self.materialize_with_parent_wait(workspace, false)
    }

    /// Keep a fresh bootstrap reservation effect-free even if its Workspace
    /// finished just before the parent publishes Active.
    pub fn materialize_for_parent(
        &self,
        workspace: &Workspace,
        parent: &crate::sandbox::Sandbox,
    ) -> Result<AgentCreation, SandboxStoreError> {
        self.materialize_with_parent_wait(workspace, self.admits_bootstrap(workspace, parent))
    }

    fn materialize_with_parent_wait(
        &self,
        workspace: &Workspace,
        wait_for_parent: bool,
    ) -> Result<AgentCreation, SandboxStoreError> {
        if let AgentOrigin::Lifecycle { operation_id } = &self.origin {
            let digest = content_digest(&self.origin)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            if operation_id.trim().is_empty()
                || operation_id.len() > 128
                || operation_id.chars().any(char::is_control)
                || self.creation_key != format!("lifecycle:{digest}")
            {
                return Err(SandboxStoreError::InvalidTransaction(
                    "lifecycle admission identity is invalid",
                ));
            }
        }
        if self.workspace_id != workspace.id
            || self.owner != workspace.owner
            || self.creation_key.trim().is_empty()
            || self.creation_key.len() > 256
        {
            return Err(SandboxStoreError::InvalidTransaction(
                "agent admission identity is invalid",
            ));
        }
        if let AgentOrigin::WorkspaceProfile { index } = self.origin {
            let expected = workspace
                .profile
                .content
                .agent_profiles
                .get(usize::from(index));
            if expected.is_none_or(|reference| {
                reference.id != self.profile.id || reference.revision != self.profile.revision
            }) || self.creation_key
                != crate::sandbox::agent::profile_agent_creation_key(&workspace.id, index)
                || self.first_message.is_some()
                || self.title.is_some()
            {
                return Err(SandboxStoreError::InvalidTransaction(
                    "automatic agent admission does not match the pinned workspace profile",
                ));
            }
        }
        let number = workspace.next_worktree_number;
        let name = format!("wt-{number}");
        let pending = wait_for_parent
            || (workspace.status == crate::sandbox::WorkspaceStatus::Preparing
                && matches!(self.origin, AgentOrigin::Lifecycle { .. }));
        let base_commit = if pending {
            // Explicit WaitForWorkspace state: this is absence of a base, never a Git revision.
            String::new()
        } else {
            workspace
                .resolved_commit
                .clone()
                .ok_or(SandboxStoreError::WorkspaceNotReady)?
        };
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
            capabilities: self.capabilities.clone(),
            status: AgentStatus::Starting,
            conversation: ConversationState {
                turn_timeout: None,
                preferences: Default::default(),
                managed_guidance_revision: 0,
                observations: Default::default(),
                settings: None,
                conversation_id: None,
                runtime_generation: None,
                last_seq: 1,
                first_message: self.first_message.clone(),
                active_turn: None,
                operation: None,
                queued_messages: Vec::new(),
            },
            work: AgentWorkState::new(
                if pending {
                    AgentWorkStep::WaitForWorkspace
                } else {
                    AgentWorkStep::AddWorktree
                },
                self.event_at.clone(),
            ),
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

    pub fn fingerprint(&self) -> Result<String, SandboxStoreError> {
        let first_message = self
            .first_message
            .as_ref()
            .map(|message| FirstMessageIdentity {
                desktop: message.desktop,
                origin_key: &message.origin_key,
                text: &message.text,
                payload_sha256: &message.payload_sha256,
            });
        content_digest(&AdmissionIdentity {
            workspace_id: &self.workspace_id,
            profile: &self.profile,
            capabilities: &self.capabilities,
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
    capabilities: &'a RuntimeCapabilities,
    title: Option<&'a str>,
    first_message: Option<FirstMessageIdentity<'a>>,
    origin: &'a AgentOrigin,
}

#[derive(Serialize)]
struct FirstMessageIdentity<'a> {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    desktop: bool,
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
        self.agent
            .validate_worktree_recoveries()
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        let created_log_matches = matches!(
            &self.initial_log.kind,
            crate::sandbox::AgentLogKind::Created {
                workspace_id,
                worktree_id,
            } if workspace_id == &self.agent.workspace_id
                && worktree_id == self.agent.creation_worktree_id()
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
    /// Read only an exact already committed creation. Never provision, adopt
    /// a prepared Agent or update Trigger attribution on a refusal path.
    async fn replay_agent_admission(
        &self,
        _admission: &AgentAdmission,
    ) -> Result<Option<AgentAdmissionResult>, SandboxStoreError> {
        Ok(None)
    }

    async fn admit_catalogue_agent_with_trigger_initiator(
        &self,
        admission: AgentAdmission,
        initiation: crate::trigger_domain::LaunchInitiation,
        reuse_existing: bool,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        self.admit_agent_with_trigger_initiator(admission, initiation, reuse_existing)
            .await
    }

    async fn admit_catalogue_agent_reusing_profile(
        &self,
        admission: AgentAdmission,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        self.admit_agent_reusing_profile(admission).await
    }

    async fn admit_catalogue_agent(
        &self,
        admission: AgentAdmission,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        self.admit_agent(admission).await
    }

    /// New Trigger creation must persist its proven initiator in the same write.
    async fn admit_agent_with_trigger_initiator(
        &self,
        _admission: AgentAdmission,
        _initiation: crate::trigger_domain::LaunchInitiation,
        _reuse_existing: bool,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "atomic Trigger attribution is unavailable",
        ))
    }

    async fn admit_agent_reusing_profile(
        &self,
        _admission: AgentAdmission,
    ) -> Result<AgentAdmissionResult, SandboxStoreError> {
        Err(SandboxStoreError::InvalidTransaction(
            "atomic Agent profile reuse is unavailable",
        ))
    }

    async fn admit_agent(
        &self,
        admission: AgentAdmission,
    ) -> Result<AgentAdmissionResult, SandboxStoreError>;
}
