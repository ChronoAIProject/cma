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
    async fn resolve_for_owner(
        &self,
        owner: &crate::resource_access::ResourceOwner,
        request: &CreateBot,
    ) -> Result<BotProfiles, BotError> {
        self.resolve(super::authority::owner_subject(owner), request)
            .await
    }
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
        self.create_owned(&caller.subject, None, key, request).await
    }
    pub async fn create_for_owner(
        &self,
        authority: &BotOwnerAuthority,
        key: &str,
        request: CreateBot,
    ) -> Result<Bot, BotError> {
        authority.require(Capability::ManageOwnBots)?;
        self.create_owned(authority.owner_subject(), Some(authority), key, request)
            .await
    }
    async fn create_owned(
        &self,
        owner: &SubjectId,
        authority: Option<&BotOwnerAuthority>,
        key: &str,
        request: CreateBot,
    ) -> Result<Bot, BotError> {
        request.validate()?;
        if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
            return Err(BotError::Invalid("an idempotency key is required".into()));
        }
        let key_hash = if let Some(authority) = authority {
            hash(
                &serde_json::to_vec(&("owned-bot-create-v1", authority.owner(), key))
                    .map_err(|_| BotError::Unavailable)?,
            )
        } else {
            hash(key.as_bytes())
        };
        let fingerprint = crate::profiles::content_digest(&request)
            .map_err(|_| BotError::Invalid("invalid bot request".into()))?;
        if let Some(current) = self.store.creation(owner, &key_hash).await? {
            return replay_owned(current, &fingerprint, authority);
        }
        let profiles = if let Some(authority) = authority {
            self.profiles
                .resolve_for_owner(authority.owner(), &request)
                .await?
        } else {
            self.profiles.resolve(owner, &request).await?
        };
        if let Some(authority) = authority {
            authority.require(Capability::ManageOwnBots)?;
        }
        let now = crate::now_rfc3339();
        let bot = Bot {
            ownership: authority.map(|authority| authority.ownership().clone()),
            bot_id: format!("bot_{}", uuid::Uuid::new_v4().simple()),
            owner: owner.clone(),
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
        bot.validate()?;
        match self.store.insert(&bot).await {
            Ok(()) => Ok(bot),
            Err(BotError::Conflict) => replay_owned(
                self.store
                    .creation(owner, &key_hash)
                    .await?
                    .ok_or(BotError::Conflict)?,
                &fingerprint,
                authority,
            ),
            Err(error) => Err(error),
        }
    }
    pub async fn get_for_owner(
        &self,
        authority: &BotOwnerAuthority,
        id: &str,
    ) -> Result<Bot, BotError> {
        authority.require(Capability::ManageOwnBots)?;
        let bot = self
            .store
            .get(id)
            .await?
            .filter(|row| {
                &row.resource_owner() == authority.owner() && row.state != BotState::Deleted
            })
            .ok_or(BotError::NotFound)?;
        authority.require(Capability::ManageOwnBots)?;
        bot.validate()?;
        Ok(bot)
    }
    pub async fn list_for_owner(
        &self,
        authority: &BotOwnerAuthority,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Bot>, BotError> {
        authority.require(Capability::ManageOwnBots)?;
        if !(1..=200).contains(&limit) {
            return Err(BotError::Invalid("limit must be between 1 and 200".into()));
        }
        let bots = self.list_owned(authority.owner(), after, limit).await?;
        authority.require(Capability::ManageOwnBots)?;
        Ok(bots)
    }
    pub async fn get(&self, caller: &Caller, id: &str) -> Result<Bot, BotError> {
        require(caller)?;
        self.store
            .get(id)
            .await?
            .filter(|row| {
                row.resource_owner()
                    == crate::resource_access::ResourceOwner::Person {
                        subject: caller.subject.clone(),
                    }
                    && row.state != BotState::Deleted
            })
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
        self.list_owned(
            &crate::resource_access::ResourceOwner::Person {
                subject: caller.subject.clone(),
            },
            after,
            limit,
        )
        .await
    }
    async fn list_owned(
        &self,
        owner: &crate::resource_access::ResourceOwner,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Bot>, BotError> {
        let mut cursor = after.map(str::to_owned);
        let mut result = Vec::new();
        loop {
            let rows = self
                .store
                .list(
                    super::authority::owner_subject(owner),
                    cursor.as_deref(),
                    200,
                )
                .await?;
            let complete = rows.len() < 200;
            if let Some(last) = rows.last() {
                if cursor
                    .as_ref()
                    .is_some_and(|previous| &last.bot_id <= previous)
                {
                    return Err(BotError::Unavailable);
                }
                cursor = Some(last.bot_id.clone());
            }
            result.extend(
                rows.into_iter()
                    .filter(|row| &row.resource_owner() == owner),
            );
            if result.len() >= limit || complete {
                result.truncate(limit);
                return Ok(result);
            }
        }
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

fn replay_owned(
    bot: Bot,
    fingerprint: &str,
    authority: Option<&BotOwnerAuthority>,
) -> Result<Bot, BotError> {
    if let Some(authority) = authority {
        authority.require(Capability::ManageOwnBots)?;
    }
    if bot.ownership.as_ref() != authority.map(BotOwnerAuthority::ownership) {
        return Err(BotError::Conflict);
    }
    replay(bot, fingerprint)
}
