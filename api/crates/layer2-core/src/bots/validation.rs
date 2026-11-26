use super::*;
use crate::SubjectId;

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
        if let Some(prior) = previous.work.as_ref().filter(|work| work.effect.is_some()) {
            let next = self.work.as_ref().ok_or(super::BotError::Conflict)?;
            // Only the exact effect holder may publish its result. It cannot
            // change operation, generation, state, or replace the reservation.
            if next.id != prior.id
                || next.holder != prior.holder
                || next.operation != prior.operation
                || next.generation != prior.generation
                || next.effect_protocol != prior.effect_protocol
                || next.step != prior.step
                || self.generation != previous.generation
                || self.state != previous.state
                || next
                    .effect
                    .as_ref()
                    .is_some_and(|effect| Some(effect) != prior.effect.as_ref())
            {
                return Err(super::BotError::Conflict);
            }
        }
        if previous
            .work
            .as_ref()
            .is_some_and(|work| work.effect_protocol == 1)
            && self
                .work
                .as_ref()
                .is_some_and(|work| work.effect_protocol != 1)
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
        if let Some(work) = &self.work {
            if work.effect_protocol > 1 {
                return Err(super::BotError::Invalid(
                    "unsupported Bot effect protocol".into(),
                ));
            }
            if let Some(effect) = &work.effect {
                if work.effect_protocol != 1
                    || work.holder.as_deref().is_none_or(str::is_empty)
                    || uuid::Uuid::parse_str(&effect.id).is_err()
                    || effect.step != work.step
                    || effect.step.is_empty()
                    || work.generation != self.generation
                    || work.expires_at.as_deref() != Some(effect.deadline.as_str())
                    || crate::seconds_from_rfc3339(&effect.deadline).is_none()
                {
                    return Err(super::BotError::Invalid(
                        "invalid Bot effect reservation".into(),
                    ));
                }
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
