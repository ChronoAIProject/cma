//! Durable resource orchestration. Adapters perform bounded, owner-scoped effects.
use super::*;
use crate::{sandbox::binding::ProvidedCredential, SubjectId};
use std::{sync::Arc, time::SystemTime};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExternalBot {
    pub id: String,
    pub name: String,
    pub platform: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotKeyRole {
    Agent,
    Route,
}
#[derive(Debug)]
pub struct BotKey {
    pub id: String,
    pub value: ProvidedCredential,
    pub expires_at: String,
}
#[derive(Debug, Clone)]
pub struct BotKeyRef {
    pub id: String,
    pub role: BotKeyRole,
}
pub struct BotSecrets {
    pub agent: ProvidedCredential,
    pub route: Option<ProvidedCredential>,
}
#[derive(Debug, Clone)]
pub struct BotChannelObservation {
    pub channel: BotChannel,
    pub agent_key_hash: String,
    pub route_key_id: String,
    pub route_ids: Vec<String>,
}
impl BotChannelObservation {
    pub fn uses(&self, keys: &BotCredentialSet) -> bool {
        keys.agent_key_hash.as_deref() == Some(self.agent_key_hash.as_str())
            && keys.route_key_id.as_deref().unwrap_or_default() == self.route_key_id
    }
}

/// Mutations must never be retried by the transport. With effect reservations
/// enabled, mutating calls require the persisted reservation and use its absolute
/// admission deadline. Success must mean the operation is terminal, not merely
/// queued remotely. Timeout/error/cancellation retains custody beyond lease expiry.
/// Enumeration must be complete and reject ambiguous ownership; a read does not
/// prove that a previously accepted mutation is terminal.
#[async_trait::async_trait]
pub trait BotExternal: Send + Sync {
    async fn available(&self, owner: &SubjectId) -> Result<Vec<ExternalBot>, BotError>;
    async fn verify(&self, bot: &Bot) -> Result<(), BotError>;
    async fn mint(&self, bot: &Bot, role: BotKeyRole) -> Result<BotKey, BotError>;
    /// All generations, restricted to this Bot's exact owner and name namespace.
    async fn keys(&self, bot: &Bot) -> Result<Vec<BotKeyRef>, BotError>;
    async fn revoke(&self, bot: &Bot, id: &str) -> Result<(), BotError>;
    async fn create_channel(
        &self,
        bot: &Bot,
        secrets: &BotSecrets,
    ) -> Result<BotChannelObservation, BotError>;
    /// Recover through the Bot's preallocated record identity, including pending creation.
    async fn channel(&self, bot: &Bot) -> Result<Option<BotChannelObservation>, BotError>;
    /// Success confirms both the retained channel and provider binding use the
    /// selected credentials. With no secrets, reconcile the retained credentials
    /// even if an earlier failed update changed the provider before persistence.
    async fn configure_channel(
        &self,
        bot: &Bot,
        current: &BotChannelObservation,
        keys: &BotCredentialSet,
        secrets: Option<&BotSecrets>,
        enabled: bool,
    ) -> Result<BotChannelObservation, BotError>;
    async fn delete_channel(
        &self,
        bot: &Bot,
        current: &BotChannelObservation,
    ) -> Result<(), BotError>;
    async fn set_callback(&self, bot: &Bot, key_id: &str, url: &str) -> Result<(), BotError>;
    async fn create_route(&self, bot: &Bot, key_id: &str) -> Result<String, BotError>;
    async fn set_route_key(&self, bot: &Bot, route: &str, key: &str) -> Result<(), BotError>;
    /// Read the exact retained route from complete, owner-scoped active inventory.
    /// Absence permits credential cleanup only; it never proves a transfer or
    /// authorizes route creation. Ambiguous or conflicting ownership is an error.
    async fn active_route_key(&self, bot: &Bot) -> Result<Option<String>, BotError>;
    /// Discover and delete routes only for the selected external bot and these
    /// owned keys. Refuse any conflicting use rather than deleting another route.
    async fn delete_routes(&self, bot: &Bot, key_ids: &[String]) -> Result<(), BotError>;
    async fn test(&self, bot: &Bot) -> Result<BotTestResult, BotError>;
}

pub struct BotWorker {
    pub(super) service: Arc<BotService>,
    pub(super) external: Arc<dyn BotExternal>,
    pub(super) holder: String,
}
impl BotWorker {
    /// A previously admitted Bot renews its own expiring credentials. Adapters
    /// still verify the owner's current authority before each external effect.
    pub async fn run_due(&self, id: &str) -> Result<(), BotError> {
        let bot = self
            .service
            .store
            .get(id)
            .await?
            .ok_or(BotError::NotFound)?;
        if bot.work.is_none() {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_err(|_| BotError::Unavailable)?
                .as_secs();
            if !matches!(bot.state, BotState::Active | BotState::Disabled)
                || bot
                    .resources
                    .credentials
                    .expires_at
                    .as_deref()
                    .and_then(crate::seconds_from_rfc3339)
                    .is_none_or(|expiry| expiry > now.saturating_add(3 * 24 * 60 * 60))
            {
                return Ok(());
            }
            let caller = crate::Caller::new(
                bot.owner,
                crate::Authorization::new([crate::Capability::ManageOwnBots]),
            );
            self.service
                .admit(&caller, id, BotOperation::Rotate)
                .await?;
        }
        self.run(id).await
    }
    pub fn new(service: Arc<BotService>, external: Arc<dyn BotExternal>, holder: String) -> Self {
        Self {
            service,
            external,
            holder,
        }
    }
    pub async fn run(&self, id: &str) -> Result<(), BotError> {
        let Some(mut bot) = self
            .service
            .claim(id, &self.holder, SystemTime::now())
            .await?
        else {
            return Ok(());
        };
        let operation = bot.work.as_ref().ok_or(BotError::Conflict)?.operation;
        let result = match operation {
            BotOperation::Create => self.create(&mut bot).await,
            BotOperation::Cleanup | BotOperation::Delete => self.cleanup(&mut bot).await,
            BotOperation::Disable | BotOperation::Enable => self.policy(&mut bot, operation).await,
            BotOperation::Rotate => self.rotate(&mut bot).await,
        };
        if let Err(error) = result {
            // A rejected revision fence belongs to another worker. Never issue
            // compensating effects from this stale copy of the aggregate.
            if error == BotError::Conflict
                || bot.work.as_ref().is_some_and(|work| work.effect.is_some())
            {
                return Err(error);
            }
            if operation == BotOperation::Create {
                self.service.begin_cleanup(&mut bot, &self.holder).await?;
                // Keep admission failures reviewable and cleanup durable. The
                // next claim waits for in-flight dependency deadlines to settle.
                self.service
                    .defer(&mut bot, &self.holder, "cleanup_pending", 120)
                    .await?;
            } else {
                self.service
                    .retry(&mut bot, &self.holder, "dependency_retry")
                    .await?;
            }
            return Err(error);
        }
        Ok(())
    }
    pub(super) async fn step(&self, bot: &mut Bot, step: &str) -> Result<(), BotError> {
        self.service.checkpoint(bot, &self.holder, step).await
    }
    async fn policy(&self, bot: &mut Bot, operation: BotOperation) -> Result<(), BotError> {
        self.step(bot, "read_channel").await?;
        let current = self
            .external
            .channel(bot)
            .await?
            .ok_or(BotError::Unavailable)?;
        if !current.uses(&bot.resources.credentials) {
            return Err(BotError::Unavailable);
        }
        let enabled = operation == BotOperation::Enable;
        if enabled && bot.request.connection_ref.is_none() {
            // Disabled renewal may have retained a key still used by an old
            // route. Reconcile that route before admitting any new messages.
            // A removed or transferred route leaves this Bot disabled.
            self.effect(bot, "reconcile_route_before_enable").await?;
            self.external
                .set_route_key(
                    bot,
                    bot.resources
                        .route_id
                        .as_deref()
                        .ok_or(BotError::Unavailable)?,
                    bot.resources
                        .credentials
                        .route_key_id
                        .as_deref()
                        .ok_or(BotError::Unavailable)?,
                )
                .await?;
            self.settled(bot).await?;
            self.retire_unused_keys(bot, &bot.resources.credentials.clone())
                .await?;
        }
        self.effect(bot, "configure_admission").await?;
        let channel = self
            .external
            .configure_channel(bot, &current, &bot.resources.credentials, None, enabled)
            .await?;
        if !channel.uses(&bot.resources.credentials) {
            return Err(BotError::Unavailable);
        }
        bot.resources.channel = Some(channel.channel);
        self.settled(bot).await?;
        self.service
            .complete(
                bot,
                &self.holder,
                if enabled {
                    BotState::Active
                } else {
                    BotState::Disabled
                },
            )
            .await
    }
}
