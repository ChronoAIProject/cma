//! Explicit, renewable NyxID authority linked to an authenticated event sender.
//! Neither a browser callback nor a model-supplied sender can activate a grant.

use super::{
    persistence::{SecretDigest, SecretValue},
    profile::Profile,
    AuthMode, AuthState,
};

use layer2_core::{
    bots::BotError,
    resource_access::{
        CallerContext, OrganizationAuthority, OrganizationRole, ResourceActor, ResourceOwner,
    },
    Caller, StoreError, SubjectId,
};
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

const LINK_LIFETIME: u64 = 15 * 60;
const RENEW_INTERVAL: u64 = 24 * 60 * 60;
mod browser;
mod model;
mod renewal;
mod services;
pub(super) use browser::routes;
pub use model::*;

impl AuthState {
    pub(crate) async fn owner_authority_for_grant(
        &self,
        grant_id: &str,
        actor: &EventActor,
        owner: &ResourceOwner,
    ) -> Result<layer2_core::bots::BotOwnerAuthority, BotError> {
        let (context, _) = self.context_for_grant(grant_id, actor, owner).await?;
        let grant = self.link_record(grant_id).await?.grant()?;
        layer2_core::bots::BotOwnerAuthority::new(
            context,
            owner.clone(),
            Some(layer2_core::bots::BotDelegation {
                authorization_grant_id: grant.grant_id,
                credential_incarnation: grant.credential_incarnation,
            }),
        )
    }
    pub(crate) fn with_bot_father_username(
        mut self: Arc<Self>,
        username: Option<String>,
    ) -> Result<Arc<Self>, String> {
        if username.as_ref().is_some_and(|value| {
            value.len() < 5
                || value.len() > 32
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        }) {
            return Err("Bot Father Telegram username is invalid".into());
        }
        Arc::get_mut(&mut self)
            .ok_or("configure Bot Father before sharing auth state")?
            .bot_father_username = username;
        Ok(self)
    }

    pub(crate) async fn ensure_actor_link(
        &self,
        actor: &EventActor,
    ) -> Result<ActorLinkStatus, BotError> {
        let key = actor.key().map_err(|error| {
            tracing::warn!(stage = "actor_key", error = ?error, "Bot Father actor identity was rejected");
            BotError::Forbidden
        })?;
        for _ in 0..4 {
            let current = self
                .store
                .actor_link_for_actor(&key)
                .await
                .map_err(|error| {
                    tracing::warn!(stage = "actor_link_read", error = ?error, "Bot Father actor link could not be read");
                    BotError::Unavailable
                })?;
            if let Some(row) = &current {
                row.validate().map_err(|error| {
                    tracing::warn!(stage = "actor_link_validate", error = ?error, "Bot Father actor link was malformed");
                    BotError::Unavailable
                })?;
                if row.actor != *actor {
                    return Err(BotError::Forbidden);
                }
                if row.active && !row.revoked {
                    match self.link_token(row).await {
                        Ok(_) => return self.link_status(row),
                        Err(BotError::Forbidden) => {}
                        Err(error) => return Err(error),
                    }
                    // A replacement login or explicit provider revocation cannot
                    // silently replace the authority of an established grant.
                    self.revoke_actor_link(actor, &row.link_id).await?;
                    continue;
                }
                if !row.revoked && row.expires_at > now() {
                    return self.link_status(row);
                }
            }
            let row = ActorLinkRecord {
                actor_key: key.clone(),
                actor: actor.clone(),
                link_id: uuid::Uuid::new_v4().simple().to_string(),
                revision: current.as_ref().map_or(1, |row| row.revision + 1),
                expires_at: now() + LINK_LIFETIME,
                subject: None,
                incarnation: None,
                profile: Profile::default(),
                confirmation_digest: None,
                confirmation_failures: 0,
                active: false,
                revoked: false,
                renewed_at: 0,
            };
            if self
                .store
                .replace_actor_link(current.as_ref().map(|row| row.revision), row.clone())
                .await
                .map_err(|error| {
                    tracing::warn!(stage = "actor_link_write", error = ?error, "Bot Father actor link could not be written");
                    BotError::Unavailable
                })?
            {
                return self.link_status(&row);
            }
        }
        Err(BotError::Conflict)
    }

    fn link_status(&self, row: &ActorLinkRecord) -> Result<ActorLinkStatus, BotError> {
        let AuthMode::Provider { public_url, .. } = &self.mode else {
            tracing::warn!(
                stage = "auth_mode",
                "Bot Father actor link requires provider authentication mode"
            );
            return Err(BotError::Forbidden);
        };
        Ok(ActorLinkStatus {
            link_id: row.link_id.clone(),
            revision: row.revision,
            login_url: (!row.active && row.subject.is_none()).then(|| {
                format!(
                    "{}/auth/bot-father/link/{}",
                    public_url.trim_end_matches('/'),
                    row.link_id
                )
            }),
            candidate: row.subject.clone().map(|subject| ActorCandidate {
                subject,
                profile: row.profile.clone(),
            }),
            grant_id: (row.active && !row.revoked).then(|| row.link_id.clone()),
        })
    }

    /// Root must supply the code taken from this exact actor's verified event.
    pub(crate) async fn confirm_actor_link(
        &self,
        actor: &EventActor,
        expected_revision: u64,
        expected_subject: &SubjectId,
        confirmation_code: &str,
    ) -> Result<ActorGrant, BotError> {
        let key = actor.key().map_err(|_| BotError::Forbidden)?;
        let mut row = self
            .store
            .actor_link_for_actor(&key)
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Forbidden)?;
        row.validate().map_err(|_| BotError::Unavailable)?;
        if row.active
            && !row.revoked
            && row.actor == *actor
            && row.subject.as_ref() == Some(expected_subject)
            && expected_revision > 0
            && expected_revision <= row.revision
            && opaque(confirmation_code)
            && row.confirmation_digest.as_deref()
                == Some(digest("actor-link-confirmation", confirmation_code).as_str())
        {
            self.link_token(&row).await?;
            return row.grant();
        }
        if row.actor != *actor
            || row.revision != expected_revision
            || row.subject.as_ref() != Some(expected_subject)
            || row.active
            || row.revoked
            || row.expires_at <= now()
            || row.confirmation_failures >= 5
        {
            return Err(BotError::Forbidden);
        }
        if !opaque(confirmation_code)
            || row.confirmation_digest.as_deref()
                != Some(digest("actor-link-confirmation", confirmation_code).as_str())
        {
            row.confirmation_failures += 1;
            row.revision += 1;
            self.store
                .replace_actor_link(Some(expected_revision), row)
                .await
                .map_err(|_| BotError::Unavailable)?;
            return Err(BotError::Forbidden);
        }
        let subject = row.subject.as_ref().ok_or(BotError::Forbidden)?;
        self.bot_owner_token_fenced(subject, row.incarnation.as_deref())
            .await?;
        row.active = true;
        row.revision += 1;
        row.renewed_at = now();
        if !self
            .store
            .replace_actor_link(Some(expected_revision), row.clone())
            .await
            .map_err(|_| BotError::Unavailable)?
        {
            return Err(BotError::Conflict);
        }
        row.grant()
    }

    #[allow(dead_code)] // Used by the deferred key-rotation adapter when a renewal is admitted.
    pub(crate) async fn credential_for_grant(
        &self,
        grant_id: &str,
        actor: &EventActor,
    ) -> Result<SecretValue, BotError> {
        let row = self.link_record(grant_id).await?;
        if row.actor != *actor || row.actor_key != actor.key().map_err(|_| BotError::Forbidden)? {
            return Err(BotError::Forbidden);
        }
        self.link_token(&row).await
    }

    pub(crate) async fn context_for_grant(
        &self,
        grant_id: &str,
        actor: &EventActor,
        owner: &ResourceOwner,
    ) -> Result<(CallerContext, SecretValue), BotError> {
        let row = self.link_record(grant_id).await?;
        if row.actor != *actor {
            return Err(BotError::Forbidden);
        }
        let token = self.link_token(&row).await?;
        let grant = row.grant()?;
        let state = self
            .store
            .subject(&grant.subject)
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Forbidden)?;
        let request = super::cookie_authorization::request_for_cookie(self, &grant.subject, state)
            .await
            .map_err(|_| BotError::Unavailable)?;
        let caller = Caller::new(grant.subject.clone(), request.authorization.clone());
        let mut context = match owner {
            ResourceOwner::Person { subject } if subject == &grant.subject => {
                self.resource_request_context(&request)
            }
            ResourceOwner::Organization { subject } => {
                let context = self
                    .organization_context(&caller, token.expose(), subject)
                    .await
                    .map_err(|_| BotError::Forbidden)?;
                if !context.memberships.iter().any(|membership| {
                    membership.organization == *subject
                        && membership.role == OrganizationRole::Admin
                        && membership.is_current(&context.credential, SystemTime::now())
                }) {
                    return Err(BotError::Forbidden);
                }
                context
            }
            _ => return Err(BotError::Forbidden),
        };
        context.actor = ResourceActor::Api {
            subject: grant.subject,
            delegation_id: Some(grant.grant_id),
        };
        // Recheck the exact grant after any provider IO before returning authority.
        let current = self.link_record(grant_id).await?;
        if current.revision != row.revision || !current.active || current.revoked {
            return Err(BotError::Forbidden);
        }
        self.bot_owner_token_fenced(&context.caller.subject, Some(&grant.credential_incarnation))
            .await?;
        Ok((context, token))
    }

    pub(crate) async fn revoke_actor_link(
        &self,
        actor: &EventActor,
        grant_id: &str,
    ) -> Result<(), BotError> {
        for _ in 0..4 {
            let mut row = self.link_record(grant_id).await?;
            if row.actor != *actor {
                return Err(BotError::Forbidden);
            }
            if row.revoked {
                return Ok(());
            }
            let revision = row.revision;
            row.active = false;
            row.revoked = true;
            row.confirmation_digest = None;
            row.revision += 1;
            if self
                .store
                .replace_actor_link(Some(revision), row)
                .await
                .map_err(|_| BotError::Unavailable)?
            {
                return Ok(());
            }
        }
        Err(BotError::Conflict)
    }

    pub(crate) async fn linked_bot_token(
        &self,
        grant_id: &str,
        subject: &SubjectId,
        incarnation: &str,
    ) -> Result<SecretValue, BotError> {
        let row = self.link_record(grant_id).await?;
        let grant = row.grant()?;
        if grant.subject != *subject || grant.credential_incarnation != incarnation {
            return Err(BotError::Forbidden);
        }
        self.link_token(&row).await
    }

    async fn link_record(&self, link_id: &str) -> Result<ActorLinkRecord, BotError> {
        if !opaque(link_id) {
            return Err(BotError::Forbidden);
        }
        let row = self
            .store
            .actor_link_by_id(link_id)
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Forbidden)?;
        row.validate().map_err(|_| BotError::Unavailable)?;
        if row.link_id != link_id {
            return Err(BotError::Forbidden);
        }
        Ok(row)
    }
    async fn link_token(&self, row: &ActorLinkRecord) -> Result<SecretValue, BotError> {
        let grant = row.grant()?;
        let token = self
            .bot_owner_token_fenced(&grant.subject, Some(&grant.credential_incarnation))
            .await?;
        let latest = self.link_record(&grant.grant_id).await?;
        if latest.revision != row.revision || !latest.active || latest.revoked {
            return Err(BotError::Forbidden);
        }
        Ok(token)
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn opaque(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn digest(domain: &str, value: &str) -> String {
    SecretDigest::identify(domain, value)
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn unavailable() -> StoreError {
    StoreError::Unavailable("actor link store is unavailable".into())
}
