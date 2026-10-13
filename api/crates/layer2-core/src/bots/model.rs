//! Owner-managed external channels with durable resource and credential identities.
use crate::{
    profiles::{
        AgentProfileRef, AgentProfileRevision, SandboxProfileRef, SandboxProfileRevision,
        WorkspaceProfileRef, WorkspaceProfileRevision,
    },
    SubjectId,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BotAdmission {
    All,
    Allowlist {
        sender_ids: Vec<String>,
    },
    Scoped {
        senders: IdentityAdmission,
        chats: IdentityAdmission,
        groups: GroupAdmission,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "ids",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum IdentityAdmission {
    Deny,
    Open,
    Allowlist(Vec<String>),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupAdmission {
    Deny,
    All,
    Mention,
    ReplyToBot,
    MentionOrReplyToBot,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotReply {
    pub auto_final: bool,
    pub ack_after_ms: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotImageReference {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotRepository {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotInlineSkill {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BotInlinePermissions {
    #[serde(default)]
    pub services: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BotInlineAgentProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub skills: Vec<BotInlineSkill>,
    #[serde(default)]
    pub permissions: BotInlinePermissions,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_worker_profile: Option<crate::event_workers::EventWorkerProfileRef>,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub external_bot_id: String,
    /// Opaque gateway-owned channel connection. Mutually exclusive with legacy external_bot_id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<String>,
    /// Immutable Image Manager selection used by the inline consumer authoring path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<BotImageReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<BotRepository>,
    /// Inline Agent configuration. Profile references remain for historical Bot records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_agent: Option<BotInlineAgentProfile>,
    pub profile: AgentProfileRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_profile: Option<SandboxProfileRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_profile: Option<WorkspaceProfileRef>,
    pub admission: BotAdmission,
    pub reply: BotReply,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotProfiles {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_worker_profile: Option<crate::event_workers::EventWorkerProfilePin>,
    pub resolved_model: String,
    pub agent: AgentProfileRevision,
    pub sandbox: SandboxProfileRevision,
    pub workspace: WorkspaceProfileRevision,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotState {
    Creating,
    Active,
    Disabled,
    Failed { reason: String },
    Deleting,
    Deleted,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotOperation {
    Create,
    Disable,
    Enable,
    Rotate,
    Delete,
    Cleanup,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotWork {
    pub id: String,
    pub operation: BotOperation,
    pub step: String,
    pub generation: u64,
    pub holder: Option<String>,
    pub expires_at: Option<String>,
}
/// Metadata only. Credential bytes are never serializable through the Bot aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BotCredentialSet {
    pub agent_key_id: Option<String>,
    pub agent_key_hash: Option<String>,
    pub route_key_id: Option<String>,
    pub route_key_hash: Option<String>,
    pub expires_at: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotChannel {
    pub channel_id: String,
    pub record_id: String,
    pub version: u64,
    pub binding_id: String,
    pub level: String,
    pub callback_url: String,
    pub event_url: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BotResources {
    pub channel: Option<BotChannel>,
    pub route_id: Option<String>,
    pub credentials: BotCredentialSet,
    pub replacement: Option<BotCredentialSet>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotTestResult {
    pub accepted_after_ms: u64,
    pub first_text: String,
    pub level: String,
    pub tested_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<super::BotOwnership>,
    pub bot_id: String,
    pub owner: SubjectId,
    pub request: CreateBot,
    pub profiles: BotProfiles,
    pub provider: String,
    pub record_id: String,
    pub resources: BotResources,
    pub state: BotState,
    pub work: Option<BotWork>,
    pub generation: u64,
    pub revision: u64,
    pub creation_key_hash: String,
    pub request_fingerprint: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_test: Option<BotTestResult>,
}
impl CreateBot {
    pub fn validate(&self) -> Result<(), super::BotError> {
        let inline = self.inline_agent.is_some();
        if self.name.trim().is_empty()
            || self.name.len() > 80
            || self.name.chars().any(char::is_control)
            || (self.external_bot_id.is_empty() == self.connection_ref.is_none())
            || self.connection_ref.as_ref().is_some_and(|value| {
                value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control)
            })
            || self.external_bot_id.len() > 512
            || self.external_bot_id.chars().any(char::is_control)
            || self.profile.revision == 0
            || (!inline && (self.image.is_some() || self.repository.is_some()))
            || self.image.as_ref().is_some_and(|image| {
                image.id.trim().is_empty()
                    || image.id.len() > 256
                    || image.id.chars().any(char::is_control)
                    || image.version.as_ref().is_some_and(|version| {
                        version.is_empty()
                            || version.len() > 256
                            || version.chars().any(char::is_control)
                    })
            })
            || self.repository.as_ref().is_some_and(|repository| {
                repository.url.trim().is_empty()
                    || repository.url.len() > 2048
                    || repository.url.chars().any(char::is_control)
                    || repository.reference.as_ref().is_some_and(|reference| {
                        reference.is_empty()
                            || reference.len() > 255
                            || reference.chars().any(char::is_control)
                    })
            })
            || self.inline_agent.as_ref().is_some_and(|profile| {
                profile.model.as_ref().is_some_and(|model| {
                    model.is_empty() || model.len() > 128 || model.chars().any(char::is_control)
                }) || profile.reasoning_effort.as_ref().is_some_and(|effort| {
                    effort.is_empty() || effort.len() > 32 || effort.chars().any(char::is_control)
                }) || profile.permission_mode.as_ref().is_some_and(|mode| {
                    mode.is_empty() || mode.len() > 32 || mode.chars().any(char::is_control)
                }) || profile
                    .system_prompt
                    .as_ref()
                    .is_some_and(|prompt| prompt.len() > 32768)
                    || profile.skills.len() > 128
                    || profile.skills.iter().any(|skill| {
                        skill.id.trim().is_empty()
                            || skill.id.len() > 256
                            || skill.id.chars().any(char::is_control)
                            || skill.version.as_ref().is_some_and(|version| {
                                version.is_empty()
                                    || version.len() > 256
                                    || version.chars().any(char::is_control)
                            })
                    })
                    || profile.permissions.services.len() > 64
                    || profile.permissions.services.iter().any(|service| {
                        service.trim().is_empty()
                            || service.len() > 256
                            || service.chars().any(char::is_control)
                    })
            })
            || self
                .sandbox_profile
                .as_ref()
                .is_some_and(|p| p.revision == 0)
            || self
                .workspace_profile
                .as_ref()
                .is_some_and(|p| p.revision == 0)
            || self.reply.ack_after_ms > 60_000
        {
            return Err(super::BotError::Invalid("invalid bot configuration".into()));
        }
        if let Some(reference) = &self.event_worker_profile {
            crate::event_workers::validate_id(&reference.id)?;
            if reference.revision == 0 {
                return Err(super::BotError::Invalid(
                    "worker revision must be positive".into(),
                ));
            }
        }
        let validate = |ids: &[String]| {
            if ids.is_empty()
                || ids.len() > 512
                || ids
                    .iter()
                    .any(|id| id.is_empty() || id.len() > 512 || id.chars().any(char::is_control))
                || ids.iter().collect::<std::collections::BTreeSet<_>>().len() != ids.len()
            {
                Err(super::BotError::Invalid(
                    "invalid admission identifiers".into(),
                ))
            } else {
                Ok(())
            }
        };
        match &self.admission {
            BotAdmission::Allowlist { sender_ids } => validate(sender_ids)?,
            BotAdmission::Scoped { senders, chats, .. } => {
                for value in [senders, chats] {
                    if let IdentityAdmission::Allowlist(ids) = value {
                        validate(ids)?;
                    }
                }
            }
            BotAdmission::All => {}
        }
        Ok(())
    }
}
impl Bot {
    pub fn resource_owner(&self) -> crate::resource_access::ResourceOwner {
        self.ownership
            .as_ref()
            .map(|value| value.owner.clone())
            .unwrap_or_else(|| crate::resource_access::ResourceOwner::Person {
                subject: self.owner.clone(),
            })
    }
    pub fn credential_subject(&self) -> &SubjectId {
        self.ownership
            .as_ref()
            .map_or(&self.owner, |value| &value.credential_subject)
    }
    pub fn credential_incarnation(&self) -> Option<&str> {
        self.ownership
            .as_ref()
            .and_then(|value| value.credential_incarnation.as_deref())
    }
    pub fn authorization_grant_id(&self) -> Option<&str> {
        self.ownership
            .as_ref()
            .and_then(|value| value.authorization_grant_id.as_deref())
    }
    pub fn validate_replacement(&self, previous: &Self) -> Result<(), super::BotError> {
        self.validate()?;
        if self.revision
            != previous
                .revision
                .checked_add(1)
                .ok_or(super::BotError::Conflict)?
            || self.bot_id != previous.bot_id
            || self.owner != previous.owner
            || self.ownership != previous.ownership
            || self.request != previous.request
            || self.profiles != previous.profiles
            || self.provider != previous.provider
            || self.record_id != previous.record_id
            || self.creation_key_hash != previous.creation_key_hash
            || self.request_fingerprint != previous.request_fingerprint
            || self.created_at != previous.created_at
            || self.generation < previous.generation
            || self.generation > previous.generation.saturating_add(1)
        {
            return Err(super::BotError::Conflict);
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), super::BotError> {
        self.request.validate()?;
        if let Some(ownership) = &self.ownership {
            ownership.validate()?;
            if super::authority::owner_subject(&ownership.owner) != &self.owner {
                return Err(super::BotError::Forbidden);
            }
        }
        match (
            &self.request.event_worker_profile,
            &self.profiles.event_worker_profile,
        ) {
            (None, None) => {}
            (Some(reference), Some(pin)) if reference == &pin.reference => {
                crate::event_workers::validate_id(&reference.id)?;
                let name = pin.snapshot["metadata"]["name"]
                    .as_str()
                    .ok_or(super::BotError::Unavailable)?;
                let mut projected = crate::event_workers::projection::snapshot_for_owner(
                    &crate::event_workers::projection::WorkerProjection {
                        name,
                        owner: &self.owner,
                        provider: &self.provider,
                        profiles: &self.profiles,
                    },
                    &self.resource_owner(),
                )?;
                projected["metadata"]["description"] =
                    pin.snapshot["metadata"]["description"].clone();
                if projected != pin.snapshot
                    || reference.revision == 0
                    || pin.snapshot["metadata"]["owner"]["subject"].as_str()
                        != Some(self.owner.as_str())
                    || pin.snapshot["metadata"]["provider"].as_str() != Some(self.provider.as_str())
                    || pin.digest
                        != format!(
                            "sha256:{}",
                            crate::profiles::content_digest(&pin.snapshot)
                                .map_err(|_| super::BotError::Unavailable)?
                        )
                {
                    return Err(super::BotError::Invalid(
                        "invalid Event Worker Profile pin".into(),
                    ));
                }
            }
            _ => {
                return Err(super::BotError::Invalid(
                    "Event Worker Profile pin differs from request".into(),
                ))
            }
        }
        if self.profiles.resolved_model.trim().is_empty()
            || crate::profiles::content_digest(&self.profiles.agent.content)
                .map_err(|_| super::BotError::Unavailable)?
                != self.profiles.agent.digest
            || crate::profiles::content_digest(&self.profiles.sandbox.content)
                .map_err(|_| super::BotError::Unavailable)?
                != self.profiles.sandbox.digest
            || crate::profiles::content_digest(&self.profiles.workspace.content)
                .map_err(|_| super::BotError::Unavailable)?
                != self.profiles.workspace.digest
        {
            return Err(super::BotError::Invalid("invalid pinned profiles".into()));
        }
        if (self.state == BotState::Creating && self.work.is_none())
            || (self.state == BotState::Deleting
                && self
                    .work
                    .as_ref()
                    .is_none_or(|work| work.operation != BotOperation::Delete))
            || (self.state == BotState::Deleted
                && (self.work.is_some() || self.resources != BotResources::default()))
        {
            return Err(super::BotError::Invalid("invalid bot lifecycle".into()));
        }
        if !self.bot_id.starts_with("bot_")
            || self.revision == 0
            || self.generation == 0
            || self.owner.as_str().is_empty()
            || uuid::Uuid::parse_str(&self.record_id).is_err()
            || !hash(&self.creation_key_hash)
            || !hash(&self.request_fingerprint)
            || self.profiles.agent.profile_id != self.request.profile.id
            || self.profiles.agent.revision != self.request.profile.revision
            || self.request.sandbox_profile.as_ref().is_some_and(|p| {
                p.id != self.profiles.sandbox.profile_id
                    || p.revision != self.profiles.sandbox.revision
            })
            || self.request.workspace_profile.as_ref().is_some_and(|p| {
                p.id != self.profiles.workspace.profile_id
                    || p.revision != self.profiles.workspace.revision
            })
        {
            return Err(super::BotError::Invalid("invalid bot record".into()));
        }
        for keys in
            std::iter::once(&self.resources.credentials).chain(self.resources.replacement.iter())
        {
            if keys
                .agent_key_hash
                .as_ref()
                .is_some_and(|value| !hash(value))
                || keys
                    .route_key_hash
                    .as_ref()
                    .is_some_and(|value| !hash(value))
            {
                return Err(super::BotError::Invalid("invalid credential hash".into()));
            }
        }
        if self
            .resources
            .channel
            .as_ref()
            .is_some_and(|channel| channel.record_id != self.record_id || channel.version == 0)
        {
            return Err(super::BotError::Invalid(
                "channel reservation differs".into(),
            ));
        }
        Ok(())
    }
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
