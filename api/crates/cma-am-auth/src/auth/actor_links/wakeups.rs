//! Durable source for callback continuations. Execution remains provider-owned.
use super::*;

impl AuthState {
    /// A provider continuation capability selects already-attested CMA context;
    /// it is never a newly signed user event or fresh creation confirmation.
    pub async fn authorized_actor_continuation(
        &self,
        reference: &str,
    ) -> Result<(EventActor, ActorLinkOrigin), BotError> {
        let parts = reference.split('.').collect::<Vec<_>>();
        let ["cma1", "cont", grant_id, notification_id] = parts.as_slice() else {
            return Err(BotError::Forbidden);
        };
        if !opaque(grant_id)
            || notification_id.len() != 64
            || !notification_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(BotError::Forbidden);
        }
        let row = self.link_record(grant_id).await?;
        self.link_token(&row).await?;
        let current = self.link_record(grant_id).await?;
        if !current.same_active_authority(&row)
            || current.grant()?.scope != ActorGrantScope::OrdinaryCma
        {
            return Err(BotError::Forbidden);
        }
        let wakeup = current.wakeup.ok_or(BotError::Forbidden)?;
        let origin = current.origin.ok_or(BotError::Forbidden)?;
        if wakeup.notification_id != *notification_id
            || matches!(wakeup.state, ActorWakeupState::Cancelled)
            || origin.reply_expires_at <= now()
        {
            return Err(BotError::Forbidden);
        }
        Ok((current.actor, origin))
    }

    pub async fn current_actor_grant(
        &self,
        grant_id: &str,
        actor: &EventActor,
    ) -> Result<ActorGrant, BotError> {
        let row = self.link_record(grant_id).await?;
        if row.actor != *actor {
            return Err(BotError::Forbidden);
        }
        self.link_token(&row).await?;
        row.grant()
    }

    pub async fn pending_actor_wakeups(
        &self,
        limit: usize,
    ) -> Result<Vec<ActorWakeupCandidate>, BotError> {
        if limit == 0 {
            return Ok(vec![]);
        }
        let rows = self
            .store
            .due_actor_wakeups(now(), limit.min(100))
            .await
            .map_err(|_| BotError::Unavailable)?;
        rows.into_iter()
            .map(|row| {
                row.validate().map_err(|_| BotError::Unavailable)?;
                let wakeup = row.wakeup.ok_or(BotError::Unavailable)?;
                Ok(ActorWakeupCandidate {
                    grant_id: row.link_id,
                    notification_id: wakeup.notification_id,
                })
            })
            .collect()
    }

    pub async fn claim_actor_wakeup(
        &self,
        grant_id: &str,
        notification_id: &str,
        holder: &str,
        lease_seconds: u64,
    ) -> Result<Option<ActorWakeupNotification>, BotError> {
        validate_holder(holder)?;
        // Every acquisition gets a fresh fencing token, even if a restarted
        // replica reuses its diagnostic holder name.
        let lease_holder = format!("{holder}:{}", uuid::Uuid::new_v4().simple());
        for _ in 0..4 {
            let mut row = self.link_record(grant_id).await?;
            let Some(wakeup) = &row.wakeup else {
                return Ok(None);
            };
            if wakeup.notification_id != notification_id
                || row.revoked
                || !row.active
                || wakeup.due_at().is_none_or(|due| due > now())
            {
                return Ok(None);
            }
            let origin = row.origin.clone().ok_or(BotError::Unavailable)?;
            if origin.reply_expires_at <= now() {
                self.cancel_expired_wakeup(row).await?;
                return Ok(None);
            }
            if let Err(error) = self.link_token(&row).await {
                if matches!(error, BotError::Forbidden) {
                    self.revoke_actor_link(&row.actor, &row.link_id).await?;
                    return Ok(None);
                }
                return Err(error);
            }
            let notification = ActorWakeupNotification {
                notification_id: notification_id.into(),
                grant: row.grant()?,
                actor: row.actor.clone(),
                origin,
                lease_holder: lease_holder.clone(),
            };
            let revision = row.revision;
            row.wakeup.as_mut().ok_or(BotError::Unavailable)?.state = ActorWakeupState::Claimed {
                holder: lease_holder.clone(),
                lease_until: now() + lease_seconds.clamp(10, 300),
            };
            row.revision += 1;
            if self
                .store
                .replace_actor_link(Some(revision), row)
                .await
                .map_err(|_| BotError::Unavailable)?
            {
                return Ok(Some(notification));
            }
        }
        Err(BotError::Conflict)
    }

    async fn cancel_expired_wakeup(&self, mut row: ActorLinkRecord) -> Result<(), BotError> {
        let revision = row.revision;
        row.wakeup.as_mut().ok_or(BotError::Unavailable)?.state = ActorWakeupState::Cancelled;
        row.revision += 1;
        self.store
            .replace_actor_link(Some(revision), row)
            .await
            .map_err(|_| BotError::Unavailable)?;
        Ok(())
    }

    pub async fn renew_actor_wakeup(
        &self,
        notification: &ActorWakeupNotification,
        lease_seconds: u64,
    ) -> Result<(), BotError> {
        self.change_actor_wakeup(
            notification,
            ActorWakeupState::Claimed {
                holder: notification.lease_holder.clone(),
                lease_until: now() + lease_seconds.clamp(10, 300),
            },
        )
        .await
    }

    pub async fn release_actor_wakeup(
        &self,
        notification: &ActorWakeupNotification,
        retry_after_seconds: u64,
    ) -> Result<(), BotError> {
        self.change_actor_wakeup(
            notification,
            ActorWakeupState::Pending {
                retry_at: now() + retry_after_seconds.min(300),
            },
        )
        .await
    }

    pub async fn ack_actor_wakeup(
        &self,
        notification: &ActorWakeupNotification,
        receipt: &str,
    ) -> Result<(), BotError> {
        if receipt.is_empty() || receipt.len() > 512 || receipt.chars().any(char::is_control) {
            return Err(BotError::Forbidden);
        }
        self.change_actor_wakeup(
            notification,
            ActorWakeupState::Acknowledged {
                receipt: receipt.into(),
            },
        )
        .await
    }

    pub async fn cancel_actor_wakeup(
        &self,
        notification: &ActorWakeupNotification,
    ) -> Result<(), BotError> {
        self.change_actor_wakeup(notification, ActorWakeupState::Cancelled)
            .await
    }

    async fn change_actor_wakeup(
        &self,
        notification: &ActorWakeupNotification,
        next: ActorWakeupState,
    ) -> Result<(), BotError> {
        for _ in 0..4 {
            let mut row = self.link_record(&notification.grant.grant_id).await?;
            let grant = row.grant()?;
            if row.actor != notification.actor
                || grant.subject != notification.grant.subject
                || grant.credential_incarnation != notification.grant.credential_incarnation
                || row.origin.as_ref() != Some(&notification.origin)
            {
                return Err(BotError::Forbidden);
            }
            self.link_token(&row).await?;
            let wakeup = row.wakeup.as_mut().ok_or(BotError::Forbidden)?;
            if wakeup.notification_id != notification.notification_id {
                return Err(BotError::Forbidden);
            }
            if wakeup.state == next {
                return Ok(());
            }
            if !matches!(&wakeup.state, ActorWakeupState::Claimed { holder, lease_until }
                if holder == &notification.lease_holder && *lease_until > now())
            {
                return Err(BotError::Conflict);
            }
            let revision = row.revision;
            wakeup.state = next.clone();
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
}

fn validate_holder(holder: &str) -> Result<(), BotError> {
    if holder.is_empty() || holder.len() > 128 || holder.chars().any(char::is_control) {
        return Err(BotError::Forbidden);
    }
    Ok(())
}
