//! Creation facts live beside the retained aggregate so old releases can read
//! the same Agent during rollback. Facts never establish ownership or access.

use serde::{Deserialize, Serialize};

use crate::resource_access::{ResourceActor, ResourceOwner};
use crate::{AgentId, SandboxId, WorkspaceId, WorktreeId};

use super::{Agent, AgentLogEntry, AgentLogKind, AgentOrigin, Sandbox, SandboxStoreError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentCreationOrigin {
    Manual,
    Trigger {
        trigger_id: String,
        launch_id: String,
        launching_actor: Option<ResourceActor>,
    },
    ChannelBridge {
        bridge_id: String,
        partition_id: String,
        generation: u64,
        source_reference: String,
    },
    LegacyUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCreationCategory {
    Manual,
    Trigger,
    ChannelBridge,
    LegacyUnknown,
}

impl AgentCreationOrigin {
    pub fn category(&self) -> AgentCreationCategory {
        match self {
            Self::Manual => AgentCreationCategory::Manual,
            Self::Trigger { .. } => AgentCreationCategory::Trigger,
            Self::ChannelBridge { .. } => AgentCreationCategory::ChannelBridge,
            Self::LegacyUnknown => AgentCreationCategory::LegacyUnknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCreationProvenance {
    pub schema_version: u16,
    pub agent_id: AgentId,
    pub agent_created_at: String,
    pub sandbox_id: SandboxId,
    pub sandbox_created_at: String,
    pub workspace_id: WorkspaceId,
    pub worktree_id: WorktreeId,
    pub owner: ResourceOwner,
    pub creation_origin: AgentCreationOrigin,
    /// None means no creating principal was captured; never infer one from owner.
    pub created_by: Option<ResourceActor>,
    /// Retain the original provisioning category independently of later usage.
    pub provisioning_origin: AgentOrigin,
    pub creation_fingerprint: String,
}

impl AgentCreationProvenance {
    /// Trusted creation transaction only. Legacy aggregate formats are retained.
    /// Channel creation requires its separate bridge allocation authority and is
    /// deliberately unavailable through ordinary manual/Trigger admission.
    pub fn for_admission(
        agent: &Agent,
        parent: &Sandbox,
        owner: ResourceOwner,
        created_by: Option<&ResourceActor>,
    ) -> Result<Self, SandboxStoreError> {
        if agent.revision != 1 || agent.conversation.last_seq != 1 {
            return Err(SandboxStoreError::Conflict);
        }
        if !valid_creator(created_by) {
            return Err(SandboxStoreError::Conflict);
        }
        let origin = match &agent.origin {
            AgentOrigin::Manual | AgentOrigin::Lifecycle { .. } => AgentCreationOrigin::Manual,
            AgentOrigin::Trigger {
                trigger_id,
                launch_id,
            } => AgentCreationOrigin::Trigger {
                trigger_id: trigger_id.clone(),
                launch_id: launch_id.clone(),
                launching_actor: launching_actor(created_by),
            },
            // A profile slot establishes provisioning, not a human creator or
            // a channel/Trigger origin. Preserve the slot while that is unknown.
            AgentOrigin::WorkspaceProfile { .. } => AgentCreationOrigin::LegacyUnknown,
        };
        let record = Self::capture(agent, parent, owner, origin, created_by.cloned());
        record.validate_against(agent, parent)?;
        Ok(record)
    }

    /// Called only while creating the Agent, with exact admitted Launch evidence.
    pub fn for_trigger_admission(
        admission: &super::store::AgentAdmission,
        initiation: &crate::trigger_domain::LaunchInitiation,
        agent: &Agent,
        parent: &Sandbox,
        owner: ResourceOwner,
    ) -> Result<Self, SandboxStoreError> {
        initiation.require_agent_admission(admission)?;
        if agent.origin != admission.origin
            || agent.owner != admission.owner
            || agent.workspace_id != admission.workspace_id
        {
            return Err(SandboxStoreError::Conflict);
        }
        let mut record = Self::for_admission(agent, parent, owner, admission.created_by.as_ref())?;
        let AgentCreationOrigin::Trigger {
            launching_actor, ..
        } = &mut record.creation_origin
        else {
            return Err(SandboxStoreError::Conflict);
        };
        *launching_actor = Some(initiation.actor().clone());
        record.validate_against(agent, parent)?;
        Ok(record)
    }

    /// Read-only online interpretation for retained records. No caller, current
    /// owner, Trigger reuse binding or display label may invent a creating actor.
    pub fn from_legacy(
        agent: &Agent,
        parent: &Sandbox,
        owner: ResourceOwner,
        first_log: Option<&AgentLogEntry>,
    ) -> Result<Self, SandboxStoreError> {
        let mut origin = AgentCreationOrigin::LegacyUnknown;
        if let Some(log) = first_log {
            if log.agent_id != agent.id
                || log.seq != 1
                || log.appended_at != agent.created_at
                || !matches!(&log.kind, AgentLogKind::Created { workspace_id, worktree_id }
                    if workspace_id == &agent.workspace_id && worktree_id == agent.creation_worktree_id())
            {
                return Err(corrupt());
            }
            match &agent.origin {
                AgentOrigin::Manual if agent.creation_key == format!("manual:{}", agent.id) => {
                    origin = AgentCreationOrigin::Manual
                }
                AgentOrigin::Trigger {
                    trigger_id,
                    launch_id,
                } => {
                    let digest =
                        crate::profiles::content_digest(&agent.origin).map_err(|_| corrupt())?;
                    if agent.creation_key == format!("trigger:{digest}")
                        || agent.creation_key == format!("trigger-profile:{digest}")
                    {
                        origin = AgentCreationOrigin::Trigger {
                            trigger_id: trigger_id.clone(),
                            launch_id: launch_id.clone(),
                            launching_actor: None,
                        };
                    }
                }
                _ => {}
            }
        }
        let record = Self::capture(agent, parent, owner, origin, None);
        record.validate_against(agent, parent)?;
        Ok(record)
    }

    fn capture(
        agent: &Agent,
        parent: &Sandbox,
        owner: ResourceOwner,
        creation_origin: AgentCreationOrigin,
        created_by: Option<ResourceActor>,
    ) -> Self {
        Self {
            schema_version: 1,
            agent_id: agent.id.clone(),
            agent_created_at: agent.created_at.clone(),
            sandbox_id: parent.id.clone(),
            sandbox_created_at: parent.created_at.clone(),
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.creation_worktree_id().clone(),
            owner,
            creation_origin,
            created_by,
            provisioning_origin: agent.origin.clone(),
            creation_fingerprint: agent.creation_fingerprint.clone(),
        }
    }

    pub fn validate_against(
        &self,
        agent: &Agent,
        parent: &Sandbox,
    ) -> Result<(), SandboxStoreError> {
        agent
            .validate_worktree_recoveries()
            .map_err(|_| corrupt())?;
        if !valid_creator(self.created_by.as_ref())
            || self.schema_version != 1
            || self.agent_id != agent.id
            || self.agent_created_at != agent.created_at
            || self.sandbox_id != agent.sandbox_id
            || self.sandbox_id != parent.id
            || self.sandbox_created_at != parent.created_at
            || self.workspace_id != agent.workspace_id
            || &self.worktree_id != agent.creation_worktree_id()
            || agent.owner != parent.owner
            || self.creation_fingerprint != agent.creation_fingerprint
            || self.provisioning_origin != agent.origin
            || crate::seconds_from_rfc3339(&self.agent_created_at).is_none()
            || crate::seconds_from_rfc3339(&self.sandbox_created_at).is_none()
        {
            return Err(corrupt());
        }
        let subject = match &self.owner {
            ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
        };
        if subject != &agent.owner {
            return Err(corrupt());
        }
        if let Some(ResourceActor::Trigger { launch_id, .. }) = &self.created_by {
            if !matches!(&agent.origin, AgentOrigin::Trigger { launch_id: expected, .. } if launch_id.as_str() == expected)
            {
                return Err(corrupt());
            }
        }
        match (&self.creation_origin, &agent.origin) {
            (
                AgentCreationOrigin::ChannelBridge {
                    bridge_id,
                    partition_id,
                    generation,
                    source_reference,
                },
                AgentOrigin::Manual,
            ) if !bridge_id.trim().is_empty()
                && bridge_id.len() <= 256
                && (1..=256).contains(generation)
                && [partition_id, source_reference].iter().all(|id| {
                    id.len() == 64
                        && id
                            .bytes()
                            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                })
                && agent.creation_key.starts_with("channel-allocation:bral_")
                && self.created_by.is_some() => {}
            (AgentCreationOrigin::Manual, AgentOrigin::Manual | AgentOrigin::Lifecycle { .. }) => {}
            (
                AgentCreationOrigin::Trigger {
                    trigger_id,
                    launch_id,
                    launching_actor,
                },
                AgentOrigin::Trigger {
                    trigger_id: expected_trigger,
                    launch_id: expected_launch,
                },
            ) if trigger_id == expected_trigger
                && launch_id == expected_launch
                && valid_launching_actor(launching_actor.as_ref(), self.created_by.as_ref()) => {}
            (AgentCreationOrigin::LegacyUnknown, _) if self.created_by.is_none() => {}
            _ => return Err(corrupt()),
        }
        Ok(())
    }
}

fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("Agent creation provenance does not match retained identity".into())
}

fn valid_creator(actor: Option<&ResourceActor>) -> bool {
    actor.is_none_or(|actor| match actor {
        // Provenance records attribution after the creation service validates
        // authority. Retain its delegation ID without treating it as authority.
        ResourceActor::Human { .. } | ResourceActor::Api { .. } => {
            actor.ordinary_subject().is_some()
        }
        ResourceActor::Trigger { subject, launch_id } => {
            !subject.as_str().trim().is_empty() && !launch_id.as_str().trim().is_empty()
        }
        _ => false,
    })
}

fn launching_actor(actor: Option<&ResourceActor>) -> Option<ResourceActor> {
    actor
        .filter(|actor| {
            matches!(
                actor,
                ResourceActor::Human { .. } | ResourceActor::Api { .. }
            )
        })
        .cloned()
}

fn valid_launching_actor(
    initiator: Option<&ResourceActor>,
    creator: Option<&ResourceActor>,
) -> bool {
    match (initiator, creator) {
        (Some(initiator), Some(ResourceActor::Trigger { subject, .. })) => {
            valid_creator(Some(initiator))
                && match initiator {
                    ResourceActor::Human { subject: actual }
                    | ResourceActor::Api {
                        subject: actual, ..
                    }
                    | ResourceActor::Trigger {
                        subject: actual, ..
                    } => actual == subject,
                    ResourceActor::Channel { .. } => false,
                }
        }
        _ => initiator.cloned() == launching_actor(creator),
    }
}
