//! Persistent sender mapping and public, secret-free status.
use layer2_core::{bots::BotError, StoreError, SubjectId};
use serde::{Deserialize, Serialize};

use super::{profile::Profile, SecretDigest};

/// Supplied only after authenticating and normalizing the ingress event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventActor {
    pub binding_id: String,
    pub channel_id: String,
    pub conversation_id: String,
    pub actor_id: String,
    pub platform: String,
}
impl EventActor {
    pub fn key(&self) -> Result<String, StoreError> {
        // These values are copied from CMAEG's verified event context.  Its
        // public identifier types allow 1..=512 bytes, including sealed
        // channel pointers (the Bot Father channel is commonly 278 bytes).
        // Keep the platform bound specific to the small, enumerated wire
        // value instead of imposing the old 256-byte limit on every field.
        for (value, max_bytes) in [
            (&self.binding_id, 512),
            (&self.channel_id, 512),
            (&self.conversation_id, 512),
            (&self.actor_id, 512),
            (&self.platform, 32),
        ] {
            if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
                return Err(StoreError::Malformed("event actor is invalid".into()));
            }
        }
        let value = serde_json::to_string(self).map_err(|_| unavailable())?;
        Ok(digest("event-actor-link", &value))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ActorCandidate {
    pub subject: SubjectId,
    pub profile: Profile,
}
#[derive(Debug, Clone, Serialize)]
pub struct ActorLinkStatus {
    pub link_id: String,
    pub revision: u64,
    pub login_url: Option<String>,
    pub candidate: Option<ActorCandidate>,
    pub grant_id: Option<String>,
}
#[derive(Debug, Clone)]
pub struct ActorGrant {
    pub grant_id: String,
    pub subject: SubjectId,
    pub credential_incarnation: String,
    pub scope: ActorGrantScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorGrantScope {
    BotManagement,
    OrdinaryCma,
}

/// Retained only from a verified event and CMA's resolved provider conversation.
/// Never deserialize this structure from a model's claimed routing identifiers.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorLinkOrigin {
    pub binding_generation: u64,
    pub provider_conversation_id: String,
    pub agent_id: layer2_core::AgentId,
    pub worktree_id: layer2_core::WorktreeId,
    pub source_event: layer2_core::sandbox::agent_io::ExternalEventContext,
    pub reply_target_ref: String,
    pub reply_expires_at: u64,
}
impl ActorLinkOrigin {
    pub fn validate(&self, actor: &EventActor) -> Result<(), StoreError> {
        self.source_event.validate().map_err(|_| unavailable())?;
        if self.binding_generation == 0
            || self.provider_conversation_id.is_empty()
            || self.provider_conversation_id.len() > 512
            || self.agent_id.as_str().is_empty()
            || self.worktree_id.as_str().is_empty()
            || self.source_event.source_id != actor.channel_id
            || self.source_event.conversation_id != actor.conversation_id
            || self
                .source_event
                .provider_conversation_id
                .as_ref()
                .is_some_and(|conversation| conversation != &self.provider_conversation_id)
            || self.reply_target_ref.is_empty()
            || self.reply_target_ref.len() > 8192
            || self.reply_expires_at == 0
        {
            return Err(StoreError::Malformed("actor link origin is invalid".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorLoginIntent {
    pub link_id: String,
    pub actor_key: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorActivation {
    #[default]
    LegacyChatProof,
    BrowserConsent,
}

/// This encrypted evidence is produced only by the consumed PKCE callback.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorBrowserConsent {
    pub attempt_digest: String,
    pub browser_digest: String,
    pub csrf_token: String,
    pub review_revision: u64,
    pub approved_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActorWakeupState {
    Pending { retry_at: u64 },
    Claimed { holder: String, lease_until: u64 },
    Acknowledged { receipt: String },
    Cancelled,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorWakeup {
    pub notification_id: String,
    pub state: ActorWakeupState,
}
impl ActorWakeup {
    pub fn due_at(&self) -> Option<u64> {
        match &self.state {
            ActorWakeupState::Pending { retry_at } => Some(*retry_at),
            ActorWakeupState::Claimed { lease_until, .. } => Some(*lease_until),
            _ => None,
        }
    }
}
#[derive(Clone)]
pub struct ActorWakeupNotification {
    pub notification_id: String,
    pub grant: ActorGrant,
    pub actor: EventActor,
    pub origin: ActorLinkOrigin,
    pub lease_holder: String,
}
#[derive(Debug, Clone)]
pub struct ActorWakeupCandidate {
    pub grant_id: String,
    pub notification_id: String,
}

/// Encrypted by the auth store. The confirmation secret is retained only as a hash.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorLinkRecord {
    pub actor_key: String,
    pub actor: EventActor,
    pub link_id: String,
    pub revision: u64,
    pub expires_at: u64,
    pub subject: Option<SubjectId>,
    pub incarnation: Option<String>,
    pub profile: Profile,
    pub confirmation_digest: Option<String>,
    pub confirmation_failures: u8,
    pub active: bool,
    pub revoked: bool,
    pub renewed_at: u64,
    #[serde(default)]
    pub activation: ActorActivation,
    #[serde(default)]
    pub browser_consent: Option<ActorBrowserConsent>,
    #[serde(default)]
    pub origin: Option<ActorLinkOrigin>,
    #[serde(default)]
    pub wakeup: Option<ActorWakeup>,
}
impl std::fmt::Debug for ActorLinkRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActorLinkRecord")
            .field("link_id", &self.link_id)
            .field("revision", &self.revision)
            .field("active", &self.active)
            .field("revoked", &self.revoked)
            .finish_non_exhaustive()
    }
}
impl ActorLinkRecord {
    /// Worker leases/renewal timestamps may change without changing authority.
    pub fn same_active_authority(&self, other: &Self) -> bool {
        self.active
            && other.active
            && !self.revoked
            && !other.revoked
            && self.link_id == other.link_id
            && self.actor == other.actor
            && self.subject == other.subject
            && self.incarnation == other.incarnation
            && self.activation == other.activation
    }
    pub fn validate(&self) -> Result<(), StoreError> {
        if self.actor_key != self.actor.key()?
            || !opaque(&self.link_id)
            || self.revision == 0
            || self.revision > i64::MAX as u64
            || self.subject.is_some() != self.incarnation.is_some()
            || self
                .incarnation
                .as_ref()
                .is_some_and(|value| value.is_empty())
            || self.active && (self.subject.is_none() || self.revoked)
            || self.active
                && match self.activation {
                    ActorActivation::LegacyChatProof => self.confirmation_digest.is_none(),
                    ActorActivation::BrowserConsent => self
                        .browser_consent
                        .as_ref()
                        .is_none_or(|consent| consent.approved_at.is_none()),
                }
            || self.confirmation_digest.as_ref().is_some_and(|value| {
                value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            return Err(StoreError::Malformed(
                "actor link authority is invalid".into(),
            ));
        }
        if let Some(origin) = &self.origin {
            origin.validate(&self.actor)?;
        }
        if let Some(consent) = &self.browser_consent {
            if self.activation != ActorActivation::BrowserConsent
                || self.subject.is_none()
                || !hex_digest(&consent.attempt_digest)
                || !hex_digest(&consent.browser_digest)
                || !opaque(&consent.csrf_token)
                || consent.review_revision == 0
                || consent.review_revision > self.revision
            {
                return Err(StoreError::Malformed(
                    "browser delegation evidence is invalid".into(),
                ));
            }
        }
        if let Some(wakeup) = &self.wakeup {
            if self.origin.is_none() || !hex_digest(&wakeup.notification_id) {
                return Err(StoreError::Malformed("actor wakeup is invalid".into()));
            }
        }
        Ok(())
    }
    pub fn grant(&self) -> Result<ActorGrant, BotError> {
        if !self.active || self.revoked {
            return Err(BotError::Forbidden);
        }
        Ok(ActorGrant {
            grant_id: self.link_id.clone(),
            subject: self.subject.clone().ok_or(BotError::Forbidden)?,
            credential_incarnation: self.incarnation.clone().ok_or(BotError::Forbidden)?,
            scope: match self.activation {
                ActorActivation::LegacyChatProof => ActorGrantScope::BotManagement,
                ActorActivation::BrowserConsent => ActorGrantScope::OrdinaryCma,
            },
        })
    }
}

fn hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn opaque(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
pub fn digest(domain: &str, value: &str) -> String {
    SecretDigest::identify(domain, value)
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub fn unavailable() -> StoreError {
    StoreError::Unavailable("actor link store is unavailable".into())
}
