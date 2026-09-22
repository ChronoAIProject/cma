use super::*;
use crate::{Caller, Capability, SubjectId};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

#[async_trait::async_trait]
pub trait BotProfileResolver: Send + Sync {
    async fn resolve(
        &self,
        owner: &SubjectId,
        request: &CreateBot,
    ) -> Result<BotProfiles, BotError>;
}
pub struct BotService {
    pub(super) store: Arc<dyn BotStore>,
    profiles: Arc<dyn BotProfileResolver>,
    provider: String,
}
impl BotService {
    pub fn new(
        store: Arc<dyn BotStore>,
        profiles: Arc<dyn BotProfileResolver>,
        provider: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            profiles,
            provider,
        })
    }
    pub async fn create(
        &self,
        caller: &Caller,
        key: &str,
        request: CreateBot,
    ) -> Result<Bot, BotError> {
        require(caller)?;
        request.validate()?;
        if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
            return Err(BotError::Invalid("an idempotency key is required".into()));
        }
        let key_hash = hash(key.as_bytes());
        let fingerprint = crate::profiles::content_digest(&request)
            .map_err(|_| BotError::Invalid("invalid bot request".into()))?;
        if let Some(current) = self.store.creation(&caller.subject, &key_hash).await? {
            return replay(current, &fingerprint);
        }
        let profiles = self.profiles.resolve(&caller.subject, &request).await?;
        let now = crate::now_rfc3339();
        let bot = Bot {
            bot_id: format!("bot_{}", uuid::Uuid::new_v4().simple()),
            owner: caller.subject.clone(),
            request,
            profiles,
            provider: self.provider.clone(),
            record_id: uuid::Uuid::new_v4().to_string(),
            resources: BotResources::default(),
            state: BotState::Creating,
            work: Some(work(BotOperation::Create, 1)),
            generation: 1,
            revision: 1,
            creation_key_hash: key_hash.clone(),
            request_fingerprint: fingerprint.clone(),
            created_at: now.clone(),
            updated_at: now,
            last_test: None,
        };
        match self.store.insert(&bot).await {
            Ok(()) => Ok(bot),
            Err(BotError::Conflict) => replay(
                self.store
                    .creation(&caller.subject, &key_hash)
                    .await?
                    .ok_or(BotError::Conflict)?,
                &fingerprint,
            ),
            Err(error) => Err(error),
        }
    }
    pub async fn get(&self, caller: &Caller, id: &str) -> Result<Bot, BotError> {
        require(caller)?;
        self.store
            .get(id)
            .await?
            .filter(|row| row.owner == caller.subject && row.state != BotState::Deleted)
            .ok_or(BotError::NotFound)
    }
    pub async fn list(
        &self,
        caller: &Caller,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Bot>, BotError> {
        require(caller)?;
        if !(1..=200).contains(&limit) {
            return Err(BotError::Invalid("limit must be between 1 and 200".into()));
        }
        self.store.list(&caller.subject, after, limit).await
    }
    pub async fn admit(
        &self,
        caller: &Caller,
        id: &str,
        operation: BotOperation,
    ) -> Result<Bot, BotError> {
        let mut bot = self.get(caller, id).await?;
        if let Some(work) = &bot.work {
            return if work.operation == operation {
                Ok(bot)
            } else {
                Err(BotError::Conflict)
            };
        }
        if operation == BotOperation::Create || operation == BotOperation::Cleanup {
            return Err(BotError::Invalid("operation cannot be requested".into()));
        }
        if (operation == BotOperation::Disable && bot.state == BotState::Disabled)
            || (operation == BotOperation::Enable && bot.state == BotState::Active)
        {
            return Ok(bot);
        }
        if operation != BotOperation::Delete
            && !matches!(bot.state, BotState::Active | BotState::Disabled)
        {
            return Err(BotError::Conflict);
        }
        if operation == BotOperation::Rotate {
            bot.generation = bot.generation.checked_add(1).ok_or(BotError::Conflict)?;
            bot.resources.replacement = Some(BotCredentialSet::default());
        }
        bot.work = Some(work(operation, bot.generation));
        if operation == BotOperation::Delete {
            bot.state = BotState::Deleting;
        }
        self.save(&mut bot).await?;
        Ok(bot)
    }
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
        work.expires_at = Some(crate::rfc3339_at(now + Duration::from_secs(120)));
        self.save(&mut bot).await?;
        Ok(Some(bot))
    }
    /// Save progress only while the exact operation claim still owns the record.
    pub async fn checkpoint(
        &self,
        bot: &mut Bot,
        holder: &str,
        step: &str,
    ) -> Result<(), BotError> {
        let work = bot.work.as_mut().ok_or(BotError::Conflict)?;
        if work.holder.as_deref() != Some(holder) || !live(work) {
            return Err(BotError::Conflict);
        }
        work.step = step.into();
        work.expires_at = Some(crate::rfc3339_at(
            SystemTime::now() + Duration::from_secs(120),
        ));
        self.save(bot).await
    }
    pub async fn complete(
        &self,
        bot: &mut Bot,
        holder: &str,
        state: BotState,
    ) -> Result<(), BotError> {
        let work = bot.work.as_ref().ok_or(BotError::Conflict)?;
        if work.holder.as_deref() != Some(holder) || !live(work) {
            return Err(BotError::Conflict);
        }
        let allowed = match (&work.operation, &state) {
            (
                BotOperation::Create | BotOperation::Enable | BotOperation::Rotate,
                BotState::Active,
            )
            | (BotOperation::Disable | BotOperation::Rotate, BotState::Disabled) => {
                bot.resources.channel.is_some()
                    && bot.resources.route_id.is_some()
                    && bot.resources.credentials.agent_key_id.is_some()
                    && bot.resources.credentials.route_key_id.is_some()
                    && bot.resources.credentials.expires_at.is_some()
            }
            (BotOperation::Delete, BotState::Deleted) => bot.resources == BotResources::default(),
            (_, BotState::Failed { .. }) => bot.resources == BotResources::default(),
            _ => false,
        };
        if !allowed {
            return Err(BotError::Conflict);
        }
        bot.state = state;
        bot.work = None;
        self.save(bot).await
    }
    pub async fn retry(&self, bot: &mut Bot, holder: &str, step: &str) -> Result<(), BotError> {
        self.defer(bot, holder, step, 15).await
    }
    pub async fn defer(
        &self,
        bot: &mut Bot,
        holder: &str,
        step: &str,
        seconds: u64,
    ) -> Result<(), BotError> {
        self.checkpoint(bot, holder, step).await?;
        let work = bot.work.as_mut().ok_or(BotError::Conflict)?;
        work.holder = None;
        work.expires_at = Some(crate::rfc3339_at(
            SystemTime::now() + Duration::from_secs(seconds),
        ));
        self.save(bot).await
    }
    pub async fn begin_cleanup(&self, bot: &mut Bot, holder: &str) -> Result<(), BotError> {
        let reason = bot.work.as_ref().ok_or(BotError::Conflict)?.step.clone();
        self.checkpoint(bot, holder, "creation_failed").await?;
        bot.work.as_mut().ok_or(BotError::Conflict)?.operation = BotOperation::Cleanup;
        bot.state = BotState::Failed { reason };
        self.save(bot).await
    }
    pub async fn record_test(
        &self,
        caller: &Caller,
        id: &str,
        result: BotTestResult,
    ) -> Result<Bot, BotError> {
        let mut bot = self.get(caller, id).await?;
        if bot.work.is_some() || !matches!(bot.state, BotState::Active | BotState::Disabled) {
            return Err(BotError::Conflict);
        }
        bot.last_test = Some(result);
        self.save(&mut bot).await?;
        Ok(bot)
    }
    pub async fn pending(&self, after: Option<&str>, limit: usize) -> Result<Vec<Bot>, BotError> {
        self.store.pending(after, limit).await
    }
    async fn save(&self, bot: &mut Bot) -> Result<(), BotError> {
        let prior = bot.revision;
        let mut next = bot.clone();
        next.revision = prior.checked_add(1).ok_or(BotError::Conflict)?;
        next.updated_at = crate::now_rfc3339();
        self.store.replace(prior, &next).await?;
        *bot = next;
        Ok(())
    }
}
fn work(operation: BotOperation, generation: u64) -> BotWork {
    BotWork {
        id: uuid::Uuid::new_v4().to_string(),
        operation,
        step: "admitted".into(),
        generation,
        holder: None,
        expires_at: None,
    }
}
fn require(caller: &Caller) -> Result<(), BotError> {
    caller
        .require(Capability::ManageOwnBots)
        .map_err(|_| BotError::Forbidden)
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn replay(bot: Bot, fingerprint: &str) -> Result<Bot, BotError> {
    if bot.request_fingerprint != fingerprint {
        Err(BotError::Conflict)
    } else {
        Ok(bot)
    }
}
fn live(work: &BotWork) -> bool {
    work.expires_at
        .as_ref()
        .and_then(|value| crate::seconds_from_rfc3339(value))
        .is_some_and(|expires| {
            expires > crate::seconds_from_rfc3339(&crate::now_rfc3339()).unwrap_or(u64::MAX)
        })
}
