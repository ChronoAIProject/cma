//! A saved reservation bounds every mutation send after asynchronous preparation.
use super::*;
use std::time::SystemTime;

pub(super) struct EffectAdmission {
    bot_id: String,
    work: BotWork,
    deadline: SystemTime,
    monotonic_deadline: tokio::time::Instant,
}

impl EffectAdmission {
    pub(super) fn from_bot(bot: &Bot) -> Result<Option<Self>, BotError> {
        let work = bot.work.as_ref().ok_or(BotError::Conflict)?;
        if work.effect_protocol == 0 {
            return Ok(None);
        }
        let effect = work.effect.as_ref().ok_or(BotError::Conflict)?;
        let seconds =
            layer2_core::seconds_from_rfc3339(&effect.deadline).ok_or(BotError::Conflict)?;
        let deadline = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        let remaining = deadline
            .duration_since(SystemTime::now())
            .map_err(|_| BotError::Conflict)?;
        if work.effect_protocol != 1 || remaining.is_zero() {
            return Err(BotError::Conflict);
        }
        Ok(Some(Self {
            bot_id: bot.bot_id.clone(),
            work: work.clone(),
            deadline,
            // A wall-clock rollback cannot extend this invocation's window.
            monotonic_deadline: tokio::time::Instant::now() + remaining,
        }))
    }

    pub(super) fn check(&self, bot: &Bot) -> Result<(), BotError> {
        if bot.bot_id != self.bot_id
            || bot.work.as_ref() != Some(&self.work)
            || SystemTime::now() >= self.deadline
            || tokio::time::Instant::now() >= self.monotonic_deadline
        {
            return Err(BotError::Conflict);
        }
        Ok(())
    }
}
