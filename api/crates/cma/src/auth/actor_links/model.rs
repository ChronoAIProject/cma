//! Persistent sender mapping and public, secret-free status.
use super::*;

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
    pub(super) fn key(&self) -> Result<String, StoreError> {
        for value in [
            &self.binding_id,
            &self.channel_id,
            &self.conversation_id,
            &self.actor_id,
            &self.platform,
        ] {
            if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
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
            || self.active
                && (self.subject.is_none() || self.confirmation_digest.is_none() || self.revoked)
            || self.confirmation_digest.as_ref().is_some_and(|value| {
                value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            return Err(StoreError::Malformed(
                "actor link authority is invalid".into(),
            ));
        }
        Ok(())
    }
    pub(super) fn grant(&self) -> Result<ActorGrant, BotError> {
        if !self.active || self.revoked {
            return Err(BotError::Forbidden);
        }
        Ok(ActorGrant {
            grant_id: self.link_id.clone(),
            subject: self.subject.clone().ok_or(BotError::Forbidden)?,
            credential_incarnation: self.incarnation.clone().ok_or(BotError::Forbidden)?,
        })
    }
}
