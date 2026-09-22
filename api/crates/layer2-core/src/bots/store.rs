use super::{Bot, BotError};
use crate::SubjectId;
use std::{collections::BTreeMap, sync::Mutex};

#[async_trait::async_trait]
pub trait BotStore: Send + Sync {
    /// Enforce unique creation keys per owner in the same admission as the record.
    async fn insert(&self, bot: &Bot) -> Result<(), BotError>;
    async fn get(&self, id: &str) -> Result<Option<Bot>, BotError>;
    async fn creation(&self, owner: &SubjectId, key_hash: &str) -> Result<Option<Bot>, BotError>;
    async fn list(
        &self,
        owner: &SubjectId,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Bot>, BotError>;
    /// Atomic revision fence for admission, claims, checkpoints, and completion.
    async fn replace(&self, expected_revision: u64, bot: &Bot) -> Result<(), BotError>;
    /// Pending operations and credentials due for renewal, in identity order.
    async fn pending(&self, after: Option<&str>, limit: usize) -> Result<Vec<Bot>, BotError>;
}

#[derive(Default)]
pub struct InMemoryBotStore(Mutex<BTreeMap<String, Bot>>);
#[async_trait::async_trait]
impl BotStore for InMemoryBotStore {
    async fn insert(&self, bot: &Bot) -> Result<(), BotError> {
        bot.validate()?;
        let mut rows = self.0.lock().map_err(|_| BotError::Unavailable)?;
        if rows.contains_key(&bot.bot_id)
            || rows
                .values()
                .any(|row| row.owner == bot.owner && row.creation_key_hash == bot.creation_key_hash)
        {
            return Err(BotError::Conflict);
        }
        rows.insert(bot.bot_id.clone(), bot.clone());
        Ok(())
    }
    async fn get(&self, id: &str) -> Result<Option<Bot>, BotError> {
        Ok(self
            .0
            .lock()
            .map_err(|_| BotError::Unavailable)?
            .get(id)
            .cloned())
    }
    async fn creation(&self, owner: &SubjectId, key_hash: &str) -> Result<Option<Bot>, BotError> {
        Ok(self
            .0
            .lock()
            .map_err(|_| BotError::Unavailable)?
            .values()
            .find(|row| &row.owner == owner && row.creation_key_hash == key_hash)
            .cloned())
    }
    async fn list(
        &self,
        owner: &SubjectId,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Bot>, BotError> {
        Ok(self
            .0
            .lock()
            .map_err(|_| BotError::Unavailable)?
            .values()
            .filter(|row| {
                &row.owner == owner
                    && row.state != super::BotState::Deleted
                    && after.is_none_or(|after| row.bot_id.as_str() > after)
            })
            .take(limit.min(200))
            .cloned()
            .collect())
    }
    async fn replace(&self, expected: u64, bot: &Bot) -> Result<(), BotError> {
        bot.validate()?;
        let mut rows = self.0.lock().map_err(|_| BotError::Unavailable)?;
        let current = rows.get(&bot.bot_id).ok_or(BotError::NotFound)?;
        bot.validate_replacement(current)?;
        if current.revision != expected
            || bot.revision != expected.checked_add(1).ok_or(BotError::Conflict)?
            || current.owner != bot.owner
            || current.creation_key_hash != bot.creation_key_hash
            || current.record_id != bot.record_id
        {
            return Err(BotError::Conflict);
        }
        rows.insert(bot.bot_id.clone(), bot.clone());
        Ok(())
    }
    async fn pending(&self, after: Option<&str>, limit: usize) -> Result<Vec<Bot>, BotError> {
        Ok(self
            .0
            .lock()
            .map_err(|_| BotError::Unavailable)?
            .values()
            .filter(|row| {
                (row.work.is_some()
                    || matches!(
                        row.state,
                        super::BotState::Active | super::BotState::Disabled
                    ))
                    && after.is_none_or(|after| row.bot_id.as_str() > after)
            })
            .take(limit.min(200))
            .cloned()
            .collect())
    }
}
