//! An uncertain external mutation retains custody independently of work expiry.
use super::*;
use std::time::{Duration, SystemTime};

impl BotService {
    /// Claim one operation. An expired creator cannot recover plaintext and must clean up.
    pub async fn claim(
        &self,
        id: &str,
        holder: &str,
        now: SystemTime,
    ) -> Result<Option<Bot>, BotError> {
        if holder.is_empty() {
            return Err(BotError::Invalid("worker identity is required".into()));
        }
        let mut bot = self.store.get(id).await?.ok_or(BotError::NotFound)?;
        let Some(work) = bot.work.as_mut() else {
            return Ok(None);
        };
        if work.effect.is_some()
            || (self.effect_reservations
                && work.effect_protocol == 0
                && (work.holder.is_some() || work.step != "admitted"))
        {
            // Neither elapsed time nor a fresh inventory proves that an old
            // admitted mutation cannot still arrive. Keep custody, including
            // legacy interrupted work which never recorded such a reservation.
            return Ok(None);
        }
        let now_seconds = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| BotError::Unavailable)?
            .as_secs();
        if work
            .expires_at
            .as_ref()
            .and_then(|value| crate::seconds_from_rfc3339(value))
            .is_some_and(|expires| expires > now_seconds)
        {
            return Ok(None);
        }
        if work.holder.is_some() && work.operation == BotOperation::Create {
            work.operation = BotOperation::Cleanup;
            work.step = "recover_interrupted_creation".into();
            bot.state = BotState::Failed {
                reason: "interrupted_creation".into(),
            };
        }
        work.holder = Some(holder.into());
        work.effect_protocol = work.effect_protocol.max(u8::from(self.effect_reservations));
        work.expires_at = Some(crate::rfc3339_at(now + Duration::from_secs(120)));
        self.save(&mut bot).await?;
        Ok(Some(bot))
    }

    pub async fn reserve_effect(
        &self,
        bot: &mut Bot,
        holder: &str,
        step: &str,
    ) -> Result<(), BotError> {
        if bot
            .work
            .as_ref()
            .is_some_and(|work| work.effect_protocol == 0)
        {
            return self.checkpoint(bot, holder, step).await;
        }
        let work = bot.work.as_mut().ok_or(BotError::Conflict)?;
        if work.holder.as_deref() != Some(holder)
            || work.effect_protocol != 1
            || work.effect.is_some()
            || !super::service::live(work)
        {
            return Err(BotError::Conflict);
        }
        let deadline = crate::rfc3339_at(SystemTime::now() + Duration::from_secs(120));
        work.step = step.into();
        work.expires_at = Some(deadline.clone());
        work.effect = Some(BotEffect {
            id: uuid::Uuid::new_v4().to_string(),
            step: step.into(),
            deadline,
        });
        // If acknowledgement is lost, the caller must not start the effect.
        // The possibly persisted reservation remains for explicit recovery.
        self.save(bot).await
    }

    /// Called only after the external operation returns a definitive success.
    /// Resource metadata already applied to `bot` commits with this settlement.
    /// Expiry does not reject an exact successful result: no successor may take
    /// custody while this reservation is present, and the revision still fences.
    pub async fn settle_effect(&self, bot: &mut Bot, holder: &str) -> Result<(), BotError> {
        if bot
            .work
            .as_ref()
            .is_some_and(|work| work.effect_protocol == 0)
        {
            return Ok(());
        }
        let work = bot.work.as_mut().ok_or(BotError::Conflict)?;
        if work.holder.as_deref() != Some(holder)
            || work.effect_protocol != 1
            || work.effect.is_none()
        {
            return Err(BotError::Conflict);
        }
        let pending = work.effect.take().unwrap();
        work.expires_at = Some(crate::rfc3339_at(
            SystemTime::now() + Duration::from_secs(120),
        ));
        let result = self.save(bot).await;
        if result.is_err() {
            // A failed/unknown publication must keep the local copy blocked as
            // well; the worker cannot compensate or start another external call.
            let work = bot.work.as_mut().ok_or(BotError::Conflict)?;
            work.expires_at = Some(pending.deadline.clone());
            work.effect = Some(pending);
        }
        result
    }
}

impl BotWorker {
    pub(super) async fn effect(&self, bot: &mut Bot, step: &str) -> Result<(), BotError> {
        self.service.reserve_effect(bot, &self.holder, step).await
    }

    pub(super) async fn settled(&self, bot: &mut Bot) -> Result<(), BotError> {
        self.service.settle_effect(bot, &self.holder).await
    }
}
