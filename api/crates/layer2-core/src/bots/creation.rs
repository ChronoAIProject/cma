use super::*;

impl BotWorker {
    pub(super) async fn create(&self, bot: &mut Bot) -> Result<(), BotError> {
        self.step(bot, "verify_external_bot").await?;
        self.external.verify(bot).await?;
        let secrets = self.mint_pair(bot, false).await?;
        self.effect(bot, "create_channel").await?;
        let current = self.external.create_channel(bot, &secrets).await?;
        if !current.uses(&bot.resources.credentials) || current.channel.record_id != bot.record_id {
            return Err(BotError::Unavailable);
        }
        bot.resources.channel = Some(current.channel.clone());
        self.settled(bot).await?;
        if bot.request.connection_ref.is_some() {
            return self
                .service
                .complete(bot, &self.holder, BotState::Active)
                .await;
        }
        self.effect(bot, "set_callback").await?;
        self.external
            .set_callback(bot, &current.route_key_id, &current.channel.callback_url)
            .await?;
        self.settled(bot).await?;
        self.effect(bot, "create_route").await?;
        bot.resources.route_id = Some(
            self.external
                .create_route(bot, &current.route_key_id)
                .await?,
        );
        self.settled(bot).await?;
        self.effect(bot, "attach_route").await?;
        let current = self
            .external
            .configure_channel(bot, &current, &bot.resources.credentials, None, true)
            .await?;
        if !current.uses(&bot.resources.credentials)
            || current.route_ids != bot.resources.route_id.iter().cloned().collect::<Vec<_>>()
        {
            return Err(BotError::Unavailable);
        }
        bot.resources.channel = Some(current.channel);
        self.settled(bot).await?;
        self.service
            .complete(bot, &self.holder, BotState::Active)
            .await
    }
    pub(super) async fn mint_pair(
        &self,
        bot: &mut Bot,
        replacement: bool,
    ) -> Result<BotSecrets, BotError> {
        self.effect(bot, "mint_agent_key").await?;
        let agent = self.external.mint(bot, BotKeyRole::Agent).await?;
        let keys = keys_mut(bot, replacement)?;
        keys.agent_key_id = Some(agent.id);
        keys.agent_key_hash = Some(agent.value.sha256());
        keys.expires_at = Some(agent.expires_at.clone());
        self.settled(bot).await?;
        if bot.request.connection_ref.is_some() {
            self.step(bot, "keys_minted").await?;
            return Ok(BotSecrets {
                agent: agent.value,
                route: None,
            });
        }
        self.effect(bot, "mint_route_key").await?;
        let route = self.external.mint(bot, BotKeyRole::Route).await?;
        let keys = keys_mut(bot, replacement)?;
        keys.route_key_id = Some(route.id);
        keys.route_key_hash = Some(route.value.sha256());
        // Both timestamps are validated by the issuing adapter. Keep the first
        // expiry so renewal can never extend the older key's apparent lifetime.
        let agent_expiry =
            crate::seconds_from_rfc3339(&agent.expires_at).ok_or(BotError::Unavailable)?;
        let route_expiry =
            crate::seconds_from_rfc3339(&route.expires_at).ok_or(BotError::Unavailable)?;
        if route_expiry < agent_expiry {
            keys.expires_at = Some(route.expires_at);
        }
        self.settled(bot).await?;
        self.step(bot, "keys_minted").await?;
        Ok(BotSecrets {
            agent: agent.value,
            route: Some(route.value),
        })
    }
    pub(super) async fn cleanup(&self, bot: &mut Bot) -> Result<(), BotError> {
        self.step(bot, "recover_channel_for_deletion").await?;
        if let Some(channel) = self.external.channel(bot).await? {
            bot.resources.channel = Some(channel.channel.clone());
            self.effect(bot, "delete_channel").await?;
            self.external.delete_channel(bot, &channel).await?;
            bot.resources.channel = None;
            self.settled(bot).await?;
        }
        bot.resources.channel = None;
        self.step(bot, "discover_owned_keys").await?;
        let keys = self.external.keys(bot).await?;
        let mut route_keys: Vec<_> = keys
            .iter()
            .filter(|key| key.role == BotKeyRole::Route)
            .map(|key| key.id.clone())
            .collect();
        for set in
            std::iter::once(&bot.resources.credentials).chain(bot.resources.replacement.iter())
        {
            if let Some(id) = &set.route_key_id {
                if !route_keys.contains(id) {
                    route_keys.push(id.clone());
                }
            }
        }
        if bot.request.connection_ref.is_none() {
            self.effect(bot, "delete_routes").await?;
            self.external.delete_routes(bot, &route_keys).await?;
            bot.resources.route_id = None;
            self.settled(bot).await?;
        }
        bot.resources.route_id = None;
        for key in keys {
            self.effect(bot, "revoke_owned_key").await?;
            self.external.revoke(bot, &key.id).await?;
            self.settled(bot).await?;
        }
        self.step(bot, "verify_cleanup").await?;
        if self.external.channel(bot).await?.is_some() || !self.external.keys(bot).await?.is_empty()
        {
            return Err(BotError::Unavailable);
        }
        bot.resources = BotResources::default();
        let state = if bot
            .work
            .as_ref()
            .is_some_and(|work| work.operation == BotOperation::Delete)
        {
            BotState::Deleted
        } else {
            bot.state.clone()
        };
        self.service.complete(bot, &self.holder, state).await
    }
}
fn keys_mut(bot: &mut Bot, replacement: bool) -> Result<&mut BotCredentialSet, BotError> {
    if replacement {
        bot.resources.replacement.as_mut().ok_or(BotError::Conflict)
    } else {
        Ok(&mut bot.resources.credentials)
    }
}
