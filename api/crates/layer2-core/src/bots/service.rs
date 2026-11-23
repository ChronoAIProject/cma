mod creation;
use super::*;
use crate::{Caller, Capability, SubjectId};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

pub struct BotService {
    pub(super) store: Arc<dyn BotStore>,
    profiles: Arc<dyn BotProfileResolver>,
    inline_profiles: Option<Arc<dyn BotInlineProfileResolver>>,
    provider: String,
    pub(super) effect_reservations: bool,
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
            inline_profiles: None,
            effect_reservations: false,
            provider,
        })
    }
    /// Opt in only after every writer understands the durable effect format.
    /// Closing admission preserves protected work and its exact settlement.
    pub fn with_effect_reservations(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("BotService effect policy must be configured before sharing")
            .effect_reservations = enabled;
        self
    }
    pub fn with_inline_profiles(
        mut self: Arc<Self>,
        resolver: Arc<dyn BotInlineProfileResolver>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("BotService inline resolver must be configured before sharing")
            .inline_profiles = Some(resolver);
        self
    }
    pub async fn create(
        &self,
        caller: &Caller,
        key: &str,
        request: CreateBot,
    ) -> Result<Bot, BotError> {
        require(caller)?;
        self.create_owned(&caller.subject, None, None, key, request)
            .await
    }
    pub async fn create_with_credential(
        &self,
        caller: &Caller,
        delegated_credential: Option<&str>,
        key: &str,
        request: CreateBot,
    ) -> Result<Bot, BotError> {
        require(caller)?;
        self.create_owned(&caller.subject, None, delegated_credential, key, request)
            .await
    }
    pub async fn create_for_owner(
        &self,
        authority: &BotOwnerAuthority,
        key: &str,
        request: CreateBot,
    ) -> Result<Bot, BotError> {
        authority.require(Capability::ManageOwnBots)?;
        self.create_owned(
            authority.owner_subject(),
            Some(authority),
            None,
            key,
            request,
        )
        .await
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
        bot.work = Some(work(operation, bot.generation, self.effect_reservations));
        if operation == BotOperation::Delete {
            bot.state = BotState::Deleting;
        }
        self.save(&mut bot).await?;
        Ok(bot)
    }
    /// Save progress only while the exact operation claim still owns the record.
    pub async fn checkpoint(
        &self,
        bot: &mut Bot,
        holder: &str,
        step: &str,
    ) -> Result<(), BotError> {
        let work = bot.work.as_mut().ok_or(BotError::Conflict)?;
        if work.holder.as_deref() != Some(holder) || !live(work) || work.effect.is_some() {
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
        if work.holder.as_deref() != Some(holder) || !live(work) || work.effect.is_some() {
            return Err(BotError::Conflict);
        }
        let allowed = match (&work.operation, &state) {
            (
                BotOperation::Create | BotOperation::Enable | BotOperation::Rotate,
                BotState::Active,
            )
            | (BotOperation::Disable | BotOperation::Rotate, BotState::Disabled) => {
                bot.resources.channel.is_some()
                    && bot.resources.credentials.agent_key_id.is_some()
                    && bot.resources.credentials.expires_at.is_some()
                    && if bot.request.connection_ref.is_some() {
                        bot.resources.route_id.is_none()
                            && bot.resources.credentials.route_key_id.is_none()
                            && bot.resources.credentials.route_key_hash.is_none()
                    } else {
                        bot.resources.route_id.is_some()
                            && bot.resources.credentials.route_key_id.is_some()
                    }
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
    pub(super) async fn save(&self, bot: &mut Bot) -> Result<(), BotError> {
        let prior = bot.revision;
        let mut next = bot.clone();
        next.revision = prior.checked_add(1).ok_or(BotError::Conflict)?;
        next.updated_at = crate::now_rfc3339();
        self.store.replace(prior, &next).await?;
        *bot = next;
        Ok(())
    }
}
fn work(operation: BotOperation, generation: u64, effect_reservations: bool) -> BotWork {
    BotWork {
        id: uuid::Uuid::new_v4().to_string(),
        effect_protocol: u8::from(effect_reservations),
        effect: None,
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
pub(super) fn live(work: &BotWork) -> bool {
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
