//! Telegram provisioning remains wholly in the gateway.
use super::{
    model::*,
    service::{effective_status, random_nonce},
    store::index_id,
    ConnectionService, Error,
};
use crate::{
    channels::resolver::ResolvedChannel,
    telegram::{BotIdentity, TelegramClient},
};
use cmaeg_core::{
    activity::{Activity, ActivityKind},
    channel::{Platform, Secret, Source},
};
use std::{sync::Arc, time::Duration};

impl ConnectionService {
    pub(super) fn telegram(&self) -> Result<TelegramClient, Error> {
        match &self.management.telegram {
            Some(client) => Ok((**client).clone()),
            None => {
                TelegramClient::production(Duration::from_secs(8)).map_err(|_| Error::Unavailable)
            }
        }
    }
    pub(super) async fn origin_channel(
        &self,
        record: &Connection,
    ) -> Result<Arc<ResolvedChannel>, Error> {
        let channel = self
            .management
            .resolver
            .resolve(record.origin.event.channel_id.as_str())
            .await
            .map_err(|_| Error::NotFound)?;
        if channel.definition.record_id != record.origin.event.record_id
            || channel.definition.provider.binding_id.as_str() != record.origin.binding_id
            || channel.definition.secrets.agent_key_hash != record.origin.agent_key_hash
        {
            return Err(Error::NotFound);
        }
        Ok(channel)
    }
    pub async fn manager(&self, record: &Connection) -> Result<(Secret, BotIdentity), Error> {
        let channel = self.origin_channel(record).await?;
        if let Some(reference) = &channel.definition.connection_ref {
            let (connection, _) = self.store.connection(reference).await?;
            if !connection.requested_manager || connection.status != Status::Ready {
                return Err(Error::Invalid(
                    "Bot management is not enabled for this connection.",
                ));
            }
        }
        let token = channel
            .definition
            .secrets
            .telegram_bot_token
            .clone()
            .ok_or(Error::Invalid(
                "The creation bot is not configured for bot management yet.",
            ))?;
        let bot = self
            .telegram()?
            .get_me(&token)
            .await
            .map_err(|_| Error::Unavailable)?;
        let matches = channel
            .definition
            .sources
            .iter()
            .any(|source| match source {
                Source::Telegram { bot_id, .. } => bot_id == &bot.id,
                Source::NyxidRelay {
                    platform: Platform::Telegram,
                    bot_id,
                    bot_username,
                    ..
                } => {
                    bot_id.as_ref().is_some_and(|id| id == &bot.id)
                        || (bot_id.is_none()
                            && bot_username
                                .as_ref()
                                .is_some_and(|username| Some(username) == bot.username.as_ref()))
                }
                _ => false,
            });
        if !matches || !bot.can_manage_bots {
            return Err(Error::Invalid(
                "Enable Bot Management Mode for the creation bot in official @BotFather first.",
            ));
        }
        Ok((token, bot))
    }
    pub async fn connect_existing(
        &self,
        record: &Connection,
        version: u64,
        token: String,
        label: Option<String>,
        requested_manager: bool,
    ) -> Result<(), Error> {
        if effective_status(record) != Status::Draft {
            return Err(Error::Conflict);
        }
        if token.len() > 512 {
            return Err(Error::Invalid("The bot token is invalid."));
        }
        self.origin_channel(record).await?;
        let token = Secret::new(token.trim());
        let mut bot = self
            .telegram()?
            .get_me(&token)
            .await
            .map_err(|_| Error::Invalid("Telegram could not verify that bot token."))?;
        if let Some(label) = label {
            if label.len() > 128 || label.chars().any(char::is_control) {
                return Err(Error::Invalid(
                    "Connection name must be at most 128 printable bytes.",
                ));
            }
            bot.first_name = label;
        }
        if requested_manager && !bot.can_manage_bots {
            return Err(Error::Invalid(
                "Enable Bot Management Mode in official @BotFather, then submit again.",
            ));
        }
        let mut updated = record.clone();
        updated.requested_manager = requested_manager;
        self.complete(updated, version, bot, token).await
    }
    pub async fn create_managed(
        &self,
        record: &Connection,
        version: u64,
        parent: &str,
        name: Option<String>,
        username: Option<String>,
        requested_manager: bool,
    ) -> Result<(), Error> {
        if effective_status(record) != Status::Draft
            || parent != record.origin.event.record_id.as_str()
        {
            return Err(Error::Conflict);
        }
        if name.as_ref().is_some_and(|n| {
            n.is_empty() || n.chars().count() > 64 || n.chars().any(char::is_control)
        }) || username.as_ref().is_some_and(|n| {
            !(5..=32).contains(&n.len())
                || !n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        }) {
            return Err(Error::Invalid("Use a bot name of at most 64 characters and a username of 5–32 letters, digits or underscores."));
        }
        let (token, _) = self.manager(record).await?;
        let pending_id = index_id("manager-actor", &[parent, &record.origin.actor]);
        let pending = Pending {
            owner: record.owner.clone(),
            manager_record: parent.into(),
            actor: record.origin.actor.clone(),
            connection_id: record.id.clone(),
            created_at: now(),
            expires_at: record.expires_at,
            prompt_message: None,
            receipt: None,
        };
        let pending_version = match self
            .store
            .save(
                &pending_id,
                &record.owner,
                &Stored::Pending(pending.clone()),
                None,
            )
            .await
        {
            Ok(v) => v,
            Err(Error::Conflict) => match self.store.load(&pending_id).await? {
                (Stored::Pending(old), v)
                    if old.connection_id == record.id && old.owner == record.owner =>
                {
                    v
                }
                (Stored::Pending(old), v) => {
                    let (old_connection, _) = self.store.connection(&old.connection_id).await?;
                    if !matches!(
                        effective_status(&old_connection),
                        Status::Cancelled | Status::Ready | Status::Expired
                    ) {
                        return Err(Error::Invalid(
                            "Finish or cancel your previous connection request first.",
                        ));
                    }
                    self.store
                        .save(
                            &pending_id,
                            &record.owner,
                            &Stored::Pending(pending.clone()),
                            Some(v),
                        )
                        .await?
                }
                _ => return Err(Error::Conflict),
            },
            Err(e) => return Err(e),
        };
        // Reserve before sending. A lost send response does not create another
        // connection and the authenticated receipt can recover the pending slot.
        let mut current = record.clone();
        current.status = Status::AwaitingPlatform;
        current.requested_manager = requested_manager;
        let awaiting_version = self.store.save_connection(&current, Some(version)).await?;
        let sent = self
            .telegram()?
            .send_managed_keyboard(
                &token,
                &record.origin.conversation.id,
                request_id(&record.id),
                name.as_deref(),
                username.as_deref(),
            )
            .await;
        if matches!(sent, Err(crate::telegram::Error::Rejected)) {
            current.status = Status::Draft;
            self.store
                .save_connection(&current, Some(awaiting_version))
                .await?;
            return Err(Error::Invalid("Telegram rejected the creation button. Check that this is a private chat with the creation bot, then try again."));
        }
        if let Ok(message_id) = sent {
            let mut pending = pending;
            pending.prompt_message = message_id.parse().ok();
            // A receipt may win this CAS; its immutable bot identity must survive.
            match self
                .store
                .save(
                    &pending_id,
                    &record.owner,
                    &Stored::Pending(pending),
                    Some(pending_version),
                )
                .await
            {
                Ok(_) | Err(Error::Conflict) => {}
                Err(e) => return Err(e),
            }
        }
        // An uncertain send remains visible and recoverable. Never invent that
        // Telegram accepted it or rotate a bot token to recover a read.
        Ok(())
    }
    pub(super) async fn complete(
        &self,
        mut record: Connection,
        version: u64,
        bot: BotIdentity,
        token: Secret,
    ) -> Result<(), Error> {
        if !matches!(
            effective_status(&record),
            Status::Draft | Status::AwaitingPlatform
        ) {
            return Err(Error::Conflict);
        }
        if record.material.as_ref().is_some_and(|m| m.bot_id != bot.id) {
            return Err(Error::Conflict);
        }
        let username = bot.username.ok_or(Error::Invalid(
            "The connected bot needs a Telegram username.",
        ))?;
        record.material = Some(Material {
            bot_id: bot.id.clone(),
            username,
            name: bot.first_name,
            token: token.as_str().into(),
            webhook_secret: record
                .material
                .as_ref()
                .map_or_else(random_nonce, |m| m.webhook_secret.clone()),
            can_manage_bots: bot.can_manage_bots,
        });
        let reserved = self.store.save_connection(&record, Some(version)).await?;
        self.store
            .claim_bot(&bot.id, &record.owner, &record.id)
            .await?;
        record.status =
            if record.requested_manager && !record.material.as_ref().unwrap().can_manage_bots {
                Status::AwaitingPlatform
            } else {
                Status::Ready
            };
        self.store.save_connection(&record, Some(reserved)).await?;
        Ok(())
    }
    pub async fn observe_creation(
        &self,
        channel: &ResolvedChannel,
        activity: &Activity,
    ) -> Result<(), Error> {
        let ActivityKind::Event { ce } = &activity.kind else {
            return Ok(());
        };
        if ce.r#type != "telegram.managed_bot_created" {
            return Ok(());
        }
        let pending_id = index_id(
            "manager-actor",
            &[channel.definition.record_id.as_str(), &activity.actor.id],
        );
        let (mut pending, version) = match self.store.load(&pending_id).await {
            Ok((Stored::Pending(pending), version)) => (pending, version),
            Ok(_) | Err(Error::NotFound) => return Ok(()),
            Err(e) => return Err(e),
        };
        let (record, record_version) = self.store.connection(&pending.connection_id).await?;
        if record.status == Status::Ready || effective_status(&record) != Status::AwaitingPlatform {
            return Ok(());
        }
        if pending.expires_at <= now()
            || pending.actor != record.origin.actor
            || activity.conversation != record.origin.conversation
            || pending.manager_record != record.origin.event.record_id.as_str()
        {
            return Ok(());
        }
        let timestamp = time::OffsetDateTime::parse(
            &activity.occurred_at,
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|_| Error::Invalid("invalid platform receipt"))?
        .unix_timestamp();
        if timestamp < pending.created_at || timestamp > pending.expires_at {
            return Ok(());
        }
        let data = ce
            .data
            .as_ref()
            .ok_or(Error::Invalid("invalid platform receipt"))?;
        let bot_id = data["bot"]["id"]
            .as_i64()
            .filter(|id| *id > 0)
            .map(|id| id.to_string())
            .ok_or(Error::Invalid("invalid created bot"))?;
        let message_id = data["message_id"]
            .as_str()
            .and_then(|id| id.parse::<i64>().ok())
            .ok_or(Error::Invalid("invalid platform receipt"))?;
        if pending
            .prompt_message
            .is_some_and(|prompt| message_id <= prompt)
        {
            return Ok(());
        }
        if let Some(receipt) = &pending.receipt {
            if receipt.bot_id != bot_id || receipt.event_id != activity.event_id.as_str() {
                return Ok(());
            }
        } else {
            pending.receipt = Some(Receipt {
                event_id: activity.event_id.to_string(),
                bot_id: bot_id.clone(),
                message_id,
            });
            self.store
                .save(
                    &pending_id,
                    &pending.owner,
                    &Stored::Pending(pending.clone()),
                    Some(version),
                )
                .await?;
        }
        let (manager, _) = self.manager(&record).await?;
        let telegram = self.telegram()?;
        let token = telegram
            .managed_bot_token(&manager, &bot_id)
            .await
            .map_err(|_| Error::Unavailable)?;
        let bot = telegram
            .get_me(&token)
            .await
            .map_err(|_| Error::Unavailable)?;
        if bot.id != bot_id {
            return Err(Error::Invalid("created bot identity changed"));
        }
        self.complete(record, record_version, bot, token).await
    }
}
fn request_id(id: &str) -> i32 {
    use sha2::{Digest, Sha256};
    i32::from_be_bytes(Sha256::digest(id.as_bytes())[..4].try_into().unwrap())
}
impl crate::ingress::receipts::PlatformReceiptObserver for ConnectionService {
    fn observe<'a>(
        &'a self,
        channel: &'a ResolvedChannel,
        activity: &'a Activity,
    ) -> futures_util::future::BoxFuture<'a, Result<(), cmaeg_core::problem::Problem>> {
        Box::pin(async move {
            self.observe_creation(channel, activity)
                .await
                .map_err(Error::problem)
        })
    }
}
